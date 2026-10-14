//! Mapping between Jira and the view model.
//!
//! `board_configuration` → columns (name + status ids); each issue → [`Issue`],
//! placed into the column whose `status_ids` contains its status id. Issues
//! whose status isn't in any board column land in a trailing "Other" column.
//! Transitions and comments map into their view-model types here too.

use std::collections::HashMap;

use serde_json::Value;

use super::adf;
use super::wire::{BoardConfig, RawComment, RawIssue, RawTransition};
use crate::config::FieldMap;
use crate::model::{
    AllowedValue, Board, Column, Comment, Constraint, EpicRef, Issue, ScreenField, SprintRef, TeamRef, Transition,
};

pub fn build_board(
    config: BoardConfig,
    scope: String,
    project_key: String,
    sprints: Vec<SprintRef>,
    raw: Vec<RawIssue>,
    fields: &FieldMap,
    browse_base: &str,
) -> Board {
    let mut columns: Vec<Column> = config
        .column_config
        .columns
        .iter()
        .map(|c| Column {
            name: c.name.clone(),
            status_ids: c.statuses.iter().map(|s| s.id.clone()).collect(),
            issues: Vec::new(),
            min: c.min,
            max: c.max,
        })
        .collect();
    let constraint = match config.column_config.constraint_type.as_deref() {
        Some("issueCount") => Constraint::IssueCount,
        Some("issueCountExclSubs") => Constraint::IssueCountExclSubs,
        _ => Constraint::None,
    };

    let mut status_to_column: HashMap<String, usize> = HashMap::new();
    for (i, column) in columns.iter().enumerate() {
        for status_id in &column.status_ids {
            status_to_column.insert(status_id.clone(), i);
        }
    }

    let mut unmapped: Vec<Issue> = Vec::new();
    for raw_issue in raw {
        let issue = map_issue(&raw_issue, fields, browse_base);
        match status_to_column.get(&issue.status_id) {
            Some(&i) => columns[i].issues.push(issue),
            None => unmapped.push(issue),
        }
    }

    if !unmapped.is_empty() {
        columns.push(Column {
            name: "Other".into(),
            status_ids: Vec::new(),
            issues: unmapped,
            min: None,
            max: None,
        });
    }

    Board {
        name: config.name,
        scope,
        project_key,
        sprints,
        columns,
        constraint,
    }
}

pub fn map_issue(raw: &RawIssue, fields: &FieldMap, browse_base: &str) -> Issue {
    let f = &raw.fields;
    let name_of = |key: &str, attr: &str| {
        f.get(key)
            .and_then(|v| v.get(attr))
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    Issue {
        key: raw.key.clone(),
        summary: f
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        issue_type: name_of("issuetype", "name"),
        is_subtask: f
            .get("issuetype")
            .and_then(|t| t.get("subtask"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status: name_of("status", "name").unwrap_or_default(),
        status_id: name_of("status", "id").unwrap_or_default(),
        assignee: name_of("assignee", "displayName"),
        assignee_id: name_of("assignee", "accountId"),
        priority: name_of("priority", "name"),
        labels: f
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        description: adf::to_text(f.get("description")),
        acceptance_criteria: adf::to_text(f.get(&fields.acceptance_criteria)),
        story_points: f.get(&fields.story_points).and_then(Value::as_f64),
        sprint: current_sprint(f.get(&fields.sprint)).and_then(|s| s.get("name")?.as_str().map(str::to_string)),
        sprint_id: current_sprint(f.get(&fields.sprint)).and_then(|s| s.get("id")?.as_u64()),
        team: f.get(&fields.team).and_then(|t| {
            Some(TeamRef {
                id: t.get("id")?.as_str()?.to_string(),
                name: t.get("name").or_else(|| t.get("title")).and_then(Value::as_str).unwrap_or("team").to_string(),
            })
        }),
        epic: epic_of(f, fields),
        url: format!("{}/browse/{}", browse_base.trim_end_matches('/'), raw.key),
        sync_state: Default::default(),
    }
}

/// The issue's epic. The Agile API's `epic` field carries name and color; the
/// Platform API (used after a transition) only has `parent`, and older data may
/// only have the Epic Link key.
fn epic_of(f: &Value, fields: &FieldMap) -> Option<EpicRef> {
    let text = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    if let Some(epic) = f.get("epic").filter(|e| e.is_object()) {
        let key = text(epic, "key")?;
        let name = text(epic, "name")
            .or_else(|| text(epic, "summary"))
            .unwrap_or_else(|| key.clone());
        let color = epic
            .get("color")
            .and_then(|c| c.get("key"))
            .and_then(Value::as_str)
            .and_then(|k| k.strip_prefix("color_"))
            .and_then(|n| n.parse().ok());
        return Some(EpicRef { key, name, color });
    }
    if let Some(parent) = f.get("parent") {
        let parent_fields = parent.get("fields").unwrap_or(&Value::Null);
        let is_epic = parent_fields
            .get("issuetype")
            .and_then(|t| t.get("name"))
            .and_then(Value::as_str)
            == Some("Epic");
        if is_epic {
            let key = text(parent, "key")?;
            let name = text(parent_fields, "summary").unwrap_or_else(|| key.clone());
            return Some(EpicRef { key, name, color: None });
        }
    }
    let key = text(f, &fields.epic_link)?;
    Some(EpicRef { name: key.clone(), key, color: None })
}

/// The Sprint field is a list of sprint objects (history). Prefer the active
/// one, else the last entry.
fn current_sprint(value: Option<&Value>) -> Option<&Value> {
    let arr = value?.as_array()?;
    let active = arr.iter().find(|s| {
        s.get("state")
            .and_then(Value::as_str)
            .is_some_and(|st| st.eq_ignore_ascii_case("active"))
    });
    active.or_else(|| arr.last())
}

/// Available transitions, in Jira's order.
pub fn map_transitions(raw: Vec<RawTransition>) -> Vec<Transition> {
    raw.into_iter()
        .filter(|t| t.is_available)
        .map(|t| Transition {
            id: t.id,
            name: t.name,
            to_status_id: t.to.id,
            to_status_name: t.to.name,
            fields: t
                .fields
                .iter()
                .map(|(id, meta)| map_screen_field(id, meta))
                .collect(),
        })
        .collect()
}

fn map_screen_field(id: &str, meta: &Value) -> ScreenField {
    let schema = meta.get("schema");
    let schema_str = |key: &str| {
        schema
            .and_then(|s| s.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let allowed = meta
        .get("allowedValues")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|v| {
                    let id = v.get("id").and_then(Value::as_str)?.to_string();
                    let label = ["value", "name", "displayName", "key"]
                        .iter()
                        .find_map(|k| v.get(*k).and_then(Value::as_str))
                        .unwrap_or(&id)
                        .to_string();
                    Some(AllowedValue { id, label })
                })
                .collect()
        })
        .unwrap_or_default();

    ScreenField {
        id: id.to_string(),
        name: meta
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string(),
        required: meta.get("required").and_then(Value::as_bool).unwrap_or(false),
        schema_type: schema_str("type").unwrap_or_default(),
        schema_items: schema_str("items"),
        system: schema_str("system"),
        custom: schema_str("custom"),
        allowed,
    }
}

pub fn map_comments(raw: Vec<RawComment>) -> Vec<Comment> {
    raw.into_iter()
        .map(|c| Comment {
            author: c
                .author
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_string(),
            created: display_timestamp(&c.created),
            body: adf::to_text(Some(&c.body)),
        })
        .collect()
}

/// `2026-09-30T14:05:12.345-0700` → `2026-09-30 14:05` (in the timestamp's own
/// offset; good enough for a comment header).
fn display_timestamp(raw: &str) -> String {
    match (raw.get(0..10), raw.get(11..16)) {
        (Some(date), Some(time)) => format!("{date} {time}"),
        _ => raw.to_string(),
    }
}
