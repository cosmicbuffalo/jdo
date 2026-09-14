//! Serde wire types for the Jira JSON payloads.
//!
//! Only the fields jdo consumes are modeled. Issue `fields` are kept as a raw
//! `serde_json::Value` because the custom-field ids are project-specific and
//! resolved dynamically (see [`crate::jira::mapping`] and [`crate::config::FieldMap`]).

use serde::Deserialize;
use serde_json::Value;

/// `GET /rest/agile/1.0/board/{id}/configuration`
#[derive(Debug, Deserialize)]
pub struct BoardConfig {
    pub name: String,
    #[serde(rename = "columnConfig")]
    pub column_config: ColumnConfig,
}

#[derive(Debug, Deserialize)]
pub struct ColumnConfig {
    pub columns: Vec<BoardColumn>,
}

#[derive(Debug, Deserialize)]
pub struct BoardColumn {
    pub name: String,
    #[serde(default)]
    pub statuses: Vec<StatusRef>,
}

#[derive(Debug, Deserialize)]
pub struct StatusRef {
    pub id: String,
}

/// `GET /rest/agile/1.0/board/{id}/issue` (paginated).
#[derive(Debug, Default, Deserialize)]
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

#[derive(Debug, Deserialize)]
pub struct RawIssue {
    pub key: String,
    #[serde(default)]
    pub fields: Value,
}
