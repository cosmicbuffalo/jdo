//! Per-site custom-field id resolution.
//!
//! Custom-field ids are assigned by each Jira site (Story Points might be
//! `customfield_10016` on one and `customfield_10028` on another). Resolution
//! order, per field: (1) a `[jira.fields]` override wins; (2) else the site's
//! field catalog (`GET /rest/api/3/field`) is searched — by the field's Jira
//! Software type where it has one, then by name; (3) else the field is left
//! empty and the features that need it stay out of the way.

use crate::config::{FieldMap, FieldOverrides};
use crate::jira::wire::FieldInfo;

/// What one logical field is called and how to recognize it in the catalog.
pub struct Spec {
    /// The `[jira.fields]` key.
    pub key: &'static str,
    /// Field-type ids (`schema.custom`) that identify it, best first.
    pub types: &'static [&'static str],
    /// Field names that identify it (case-insensitive), best first.
    pub names: &'static [&'static str],
}

pub const SPECS: [Spec; 5] = [
    Spec {
        key: "story_points",
        types: &[],
        // Company-managed projects use "Story Points"; team-managed ones
        // "Story point estimate".
        names: &["Story Points", "Story point estimate"],
    },
    Spec {
        key: "acceptance_criteria",
        types: &[],
        names: &["Acceptance Criteria"],
    },
    Spec {
        key: "sprint",
        types: &["com.pyxis.greenhopper.jira:gh-sprint"],
        names: &["Sprint"],
    },
    Spec {
        key: "epic_link",
        types: &["com.pyxis.greenhopper.jira:gh-epic-link"],
        names: &["Epic Link"],
    },
    Spec {
        key: "team",
        types: &[
            "com.atlassian.jira.plugin.system.customfieldtypes:atlassian-team",
            "com.atlassian.teams:rm-teams-custom-field-team",
        ],
        names: &["Team"],
    },
];

/// Where a resolved id came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Config,
    Catalog,
    Missing,
}

/// One field's resolution, kept for `jdo --fields`.
pub struct Resolution {
    pub key: &'static str,
    pub id: String,
    pub source: Source,
    /// Every catalog field that matched, as `(id, name)`, best first — more
    /// than one means the pick may be wrong and is worth pinning.
    pub candidates: Vec<(String, String)>,
}

fn override_for<'a>(overrides: &'a FieldOverrides, key: &str) -> Option<&'a String> {
    match key {
        "story_points" => overrides.story_points.as_ref(),
        "acceptance_criteria" => overrides.acceptance_criteria.as_ref(),
        "sprint" => overrides.sprint.as_ref(),
        "epic_link" => overrides.epic_link.as_ref(),
        "team" => overrides.team.as_ref(),
        _ => None,
    }
}

/// Catalog fields matching `spec`, best first, without duplicates.
fn candidates(spec: &Spec, catalog: &[FieldInfo]) -> Vec<(String, String)> {
    let by_type = spec
        .types
        .iter()
        .flat_map(|t| catalog.iter().filter(move |f| f.schema_custom() == Some(*t)));
    let by_name = spec
        .names
        .iter()
        .flat_map(|n| catalog.iter().filter(move |f| f.name.eq_ignore_ascii_case(n)));
    let mut out: Vec<(String, String)> = Vec::new();
    for field in by_type.chain(by_name) {
        if !out.iter().any(|(id, _)| *id == field.id) {
            out.push((field.id.clone(), field.name.clone()));
        }
    }
    out
}

/// Resolve every field. `catalog` may be empty when every field is pinned.
pub fn resolve(overrides: &FieldOverrides, catalog: &[FieldInfo]) -> (FieldMap, Vec<Resolution>) {
    let resolutions: Vec<Resolution> = SPECS
        .iter()
        .map(|spec| {
            let candidates = candidates(spec, catalog);
            let (id, source) = match override_for(overrides, spec.key) {
                Some(id) => (id.trim().to_string(), Source::Config),
                None => match candidates.first() {
                    Some((id, _)) => (id.clone(), Source::Catalog),
                    None => (String::new(), Source::Missing),
                },
            };
            Resolution { key: spec.key, id, source, candidates }
        })
        .collect();
    let id = |key: &str| {
        resolutions
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.id.clone())
            .unwrap_or_default()
    };
    let map = FieldMap {
        story_points: id("story_points"),
        acceptance_criteria: id("acceptance_criteria"),
        sprint: id("sprint"),
        epic_link: id("epic_link"),
        team: id("team"),
    };
    (map, resolutions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> Vec<FieldInfo> {
        serde_json::from_value(json!([
            {"id": "summary", "name": "Summary"},
            {"id": "customfield_10020", "name": "Sprint", "schema": {"custom": "com.pyxis.greenhopper.jira:gh-sprint"}},
            {"id": "customfield_10014", "name": "Epic Link", "schema": {"custom": "com.pyxis.greenhopper.jira:gh-epic-link"}},
            {"id": "customfield_10016", "name": "Story point estimate"},
            {"id": "customfield_10028", "name": "Story Points"},
            {"id": "customfield_10001", "name": "Team", "schema": {"custom": "com.atlassian.jira.plugin.system.customfieldtypes:atlassian-team"}},
            {"id": "customfield_10050", "name": "Team"},
        ]))
        .unwrap()
    }

    #[test]
    fn fields_are_found_by_type_then_name() {
        let (map, res) = resolve(&FieldOverrides::default(), &catalog());
        assert_eq!(map.sprint, "customfield_10020");
        assert_eq!(map.epic_link, "customfield_10014");
        assert_eq!(map.story_points, "customfield_10028", "\"Story Points\" beats \"Story point estimate\"");
        assert_eq!(map.team, "customfield_10001", "the Team-typed field beats a field merely named Team");
        let team = res.iter().find(|r| r.key == "team").unwrap();
        assert_eq!(team.candidates.len(), 2, "the ambiguity is kept for --fields");
    }

    #[test]
    fn overrides_win_and_absent_fields_stay_empty() {
        let overrides = FieldOverrides { story_points: Some("customfield_10016".into()), ..Default::default() };
        let (map, res) = resolve(&overrides, &catalog());
        assert_eq!(map.story_points, "customfield_10016");
        assert_eq!(map.acceptance_criteria, "", "this site has no such field");
        let ac = res.iter().find(|r| r.key == "acceptance_criteria").unwrap();
        assert_eq!(ac.source, Source::Missing);
    }
}
