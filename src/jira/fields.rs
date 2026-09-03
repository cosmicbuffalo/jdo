//! Per-project field-id resolution (P1).
//!
//! Custom-field ids vary by project (e.g. Story Points is `customfield_10016`
//! here but `customfield_10028` in team-managed projects). Resolution order:
//! (1) `[jira.fields]` config overrides win; (2) else resolve by name from the
//! field catalog / issue editmeta; (3) else fall back to the seeded defaults in
//! `crate::config::FieldMap`.
