//! Serde wire types for the Jira JSON payloads.
//!
//! Only the fields jdo consumes are modeled. Issue `fields` are kept as a raw
//! `serde_json::Value` because the custom-field ids are project-specific and
//! resolved dynamically (see [`crate::jira::mapping`] and [`crate::jira::fields`]).

use serde::Deserialize;
use serde_json::Value;

/// `GET /rest/agile/1.0/board/{id}`
#[derive(Debug, Clone, Deserialize)]
pub struct BoardInfo {
    /// `scrum` or `kanban` (or `simple` for team-managed boards).
    #[serde(rename = "type", default)]
    pub board_type: String,
    #[serde(default)]
    pub location: BoardLocation,
}

/// `GET /rest/agile/1.0/board/{id}/configuration`
#[derive(Debug, Clone, Deserialize)]
pub struct BoardConfig {
    pub name: String,
    #[serde(rename = "columnConfig")]
    pub column_config: ColumnConfig,
    /// Kanban boards' extra filter (e.g. hiding old done issues).
    #[serde(rename = "subQuery", default)]
    pub sub_query: Option<SubQuery>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubQuery {
    #[serde(default)]
    pub query: String,
}

/// `GET /rest/agile/1.0/board/{id}/sprint`
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SprintPage {
    #[serde(default)]
    pub values: Vec<Sprint>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sprint {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub state: String,
}

/// `GET /rest/agile/1.0/board/{id}` → `location`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardLocation {
    #[serde(default)]
    pub project_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ColumnConfig {
    pub columns: Vec<BoardColumn>,
    /// WIP-limit mode: `none`, `issueCount`, or `issueCountExclSubs`.
    #[serde(rename = "constraintType", default)]
    pub constraint_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoardColumn {
    pub name: String,
    #[serde(default)]
    pub statuses: Vec<StatusRef>,
    /// WIP limits; the web board flags the header when outside them.
    #[serde(default)]
    pub min: Option<u32>,
    #[serde(default)]
    pub max: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusRef {
    pub id: String,
}

/// `GET /rest/agile/1.0/board/{id}/issue` (paginated).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuePage {
    #[serde(default)]
    pub start_at: u32,
    #[serde(default)]
    pub max_results: u32,
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub issues: Vec<RawIssue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawIssue {
    pub key: String,
    #[serde(default)]
    pub fields: Value,
}

/// `GET /rest/api/3/issue/{key}/transitions?expand=transitions.fields`
#[derive(Debug, Clone, Deserialize)]
pub struct TransitionList {
    #[serde(default)]
    pub transitions: Vec<RawTransition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTransition {
    pub id: String,
    pub name: String,
    pub to: RawStatus,
    #[serde(default = "default_true")]
    pub is_available: bool,
    /// Field id → field metadata (required, schema, name, allowedValues).
    #[serde(default)]
    pub fields: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawStatus {
    pub id: String,
    pub name: String,
}

fn default_true() -> bool {
    true
}

/// `GET /rest/api/3/issue/{key}/comment`
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommentPage {
    #[serde(default)]
    pub comments: Vec<RawComment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawComment {
    #[serde(default)]
    pub author: Value,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub body: Value,
}

/// `GET /rest/api/3/myself`
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Myself {
    pub account_id: String,
    #[serde(default)]
    pub display_name: String,
}

/// One entry of `GET /rest/api/3/field`, the site's field catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct FieldInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub schema: Option<Value>,
}

impl FieldInfo {
    /// The field-type id of a custom field (e.g. `com.pyxis.greenhopper.jira:gh-sprint`).
    pub fn schema_custom(&self) -> Option<&str> {
        self.schema.as_ref()?.get("custom")?.as_str()
    }
}

/// Jira's standard error body: `{"errorMessages": [...], "errors": {field: msg}}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    #[serde(default)]
    pub error_messages: Vec<String>,
    #[serde(default)]
    pub errors: std::collections::BTreeMap<String, String>,
}
