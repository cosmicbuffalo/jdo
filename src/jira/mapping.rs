//! Mapping between Jira and the view model.
//!
//! `board_configuration` → columns (name + status ids); each issue → [`Issue`],
//! placed into the column whose `status_ids` contains its status id. Issues
//! whose status isn't in any board column land in a trailing "Other" column.

use std::collections::HashMap;

use serde_json::Value;

use super::adf;
use super::wire::{BoardConfig, RawIssue};
use crate::config::FieldMap;
use crate::model::{Board, Column, Issue};

pub fn build_board(
    config: BoardConfig,
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
        })
        .collect();

    let mut status_to_column: HashMap<String, usize> = HashMap::new();
    for (i, column) in columns.iter().enumerate() {
        for status_id in &column.status_ids {
            status_to_column.insert(status_id.clone(), i);
        }
    }

    let mut unmapped: Vec<Issue> = Vec::new();
    for raw_issue in raw {
        let status_id = raw_issue
            .fields
            .get("status")
            .and_then(|s| s.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let issue = map_issue(&raw_issue, fields, browse_base);
        match status_to_column.get(&status_id) {
            Some(&i) => columns[i].issues.push(issue),
            None => unmapped.push(issue),
        }
    }

    if !unmapped.is_empty() {
        columns.push(Column {
            name: "Other".into(),
            status_ids: Vec::new(),
            issues: unmapped,
        });
    }

    Board {
        name: config.name,
        columns,
    }
}

fn map_issue(raw: &RawIssue, fields: &FieldMap, browse_base: &str) -> Issue {
    let f = &raw.fields;
    let str_at = |ptr: &Value, key: &str| ptr.get(key).and_then(Value::as_str).map(str::to_string);

    Issue {
        key: raw.key.clone(),
        summary: str_at(f, "summary").unwrap_or_default(),
        status: f
            .get("status")
            .and_then(|s| s.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        assignee: f
            .get("assignee")
            .and_then(|a| a.get("displayName"))
            .and_then(Value::as_str)
            .map(str::to_string),
        priority: f
            .get("priority")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        labels: f
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        description: adf::to_text(f.get("description")),
        acceptance_criteria: adf::to_text(f.get(&fields.acceptance_criteria)),
        story_points: f.get(&fields.story_points).and_then(Value::as_f64),
        sprint: active_sprint_name(f.get(&fields.sprint)),
        epic_link: str_at(f, &fields.epic_link),
        comments: Vec::new(), // fetched lazily on detail open (P2)
        url: format!("{}/browse/{}", browse_base.trim_end_matches('/'), raw.key),
        updated: None,
        sync_state: Default::default(),
    }
}

/// The Sprint field is a list of sprint objects (history). Prefer the active
/// one, else the last entry.
fn active_sprint_name(value: Option<&Value>) -> Option<String> {
    let arr = value?.as_array()?;
    let active = arr.iter().find(|s| {
        s.get("state").and_then(Value::as_str).map(|st| st.eq_ignore_ascii_case("active")) == Some(true)
    });
    let chosen = active.or_else(|| arr.last())?;
    chosen.get("name").and_then(Value::as_str).map(str::to_string)
}
