//! In-memory view model.
//!
//! A Jira mapping layer (see [`crate::jira::mapping`]) populates these types
//! from the REST API; the UI renders only from here. The `Board / Column /
//! Issue` shape deliberately mirrors tdo's `Board / Column / Task` so the
//! forked rendering code stays close to its source.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default)]
pub struct Board {
    pub name: String,
    pub columns: Vec<Column>,
}

/// A board column. `status_ids` are the Jira status ids this column collects
/// (from the board configuration); an issue lands here when its status is one
/// of them.
#[derive(Debug, Clone, Default)]
pub struct Column {
    pub name: String,
    pub status_ids: Vec<String>,
    pub issues: Vec<Issue>,
}

/// The fixed field set for the MVP (expandable later). Rich-text fields
/// (`description`, `acceptance_criteria`) are stored flattened-to-text for
/// display; authoring converts back to minimal ADF (see [`crate::jira::adf`]).
#[derive(Debug, Clone, Default)]
pub struct Issue {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub assignee: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub description: String,
    pub acceptance_criteria: String,
    pub story_points: Option<f64>,
    pub sprint: Option<String>,
    pub epic_link: Option<String>,
    pub comments: Vec<Comment>,
    /// Browser URL for the issue (used by copy-to-clipboard).
    pub url: String,
    pub updated: Option<DateTime<Utc>>,
    pub sync_state: SyncState,
}

#[derive(Debug, Clone, Default)]
pub struct Comment {
    pub author: String,
    pub created: Option<DateTime<Utc>>,
    pub body: String,
}

/// Optimistic-update lifecycle for an issue: local edits mark it `Pending`
/// until the worker acks (then `Synced`) or fails (`Error`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SyncState {
    #[default]
    Synced,
    Pending,
    Error,
}

impl Board {
    /// Placeholder board for P0 so the shell renders before the Jira layer
    /// exists. Removed once `jira::mapping` produces a real board.
    pub fn mock() -> Self {
        let issue = |key: &str, summary: &str, status: &str, sp: Option<f64>| Issue {
            key: key.into(),
            summary: summary.into(),
            status: status.into(),
            assignee: Some("Noor Haddad".into()),
            priority: Some("Medium".into()),
            story_points: sp,
            url: format!("https://your-site.atlassian.net/browse/{key}"),
            ..Default::default()
        };

        Board {
            name: "DT-BTFUNDING (mock)".into(),
            columns: vec![
                Column {
                    name: "Backlog".into(),
                    status_ids: vec!["10074".into()],
                    issues: vec![
                        issue("DEMO-101", "Wire up board configuration fetch", "Backlog", Some(3.0)),
                        issue("DEMO-102", "Map statuses to columns", "Backlog", Some(2.0)),
                    ],
                },
                Column {
                    name: "In Progress".into(),
                    status_ids: vec!["3".into()],
                    issues: vec![issue(
                        "DEMO-103",
                        "Render mock board in the TUI shell",
                        "In Progress",
                        Some(1.0),
                    )],
                },
                Column {
                    name: "Done".into(),
                    status_ids: vec!["10075".into()],
                    issues: vec![issue("DEMO-100", "Scaffold the jdo repo", "Done", Some(1.0))],
                },
            ],
        }
    }
}
