//! Configuration.
//!
//! P0 defines the shape (with defaults seeded from the live field discovery on
//! `your-site.atlassian.net`). File + env loading and validation land in P1, where
//! the board is selected via `JDO_BOARD` / `[jira].board_id`. Secrets
//! (`JIRA_API_TOKEN`, OAuth client creds) come from env only, never the file.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JiraConfig {
    pub base_url: String,
    pub auth: AuthKind,
    /// Team board id, from the board URL `.../boards/<id>`. Overridable by
    /// `--board` / `JDO_BOARD`.
    pub board_id: u64,
    pub refresh_seconds: u64,
    pub fields: FieldMap,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    #[default]
    ApiToken,
    OAuth,
}

/// Custom-field id overrides. Empty by default — resolved at runtime from the
/// project's field catalog / editmeta so jdo works across projects with
/// differing ids. These seeds match project DEMO and act as fallbacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldMap {
    pub story_points: String,
    pub acceptance_criteria: String,
    pub sprint: String,
    pub epic_link: String,
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            base_url: "https://your-site.atlassian.net".into(),
            auth: AuthKind::ApiToken,
            board_id: 0,
            refresh_seconds: 25,
            fields: FieldMap::default(),
        }
    }
}

impl Default for FieldMap {
    fn default() -> Self {
        Self {
            story_points: "customfield_10016".into(),
            acceptance_criteria: "customfield_10035".into(),
            sprint: "customfield_10020".into(),
            epic_link: "customfield_10014".into(),
        }
    }
}
