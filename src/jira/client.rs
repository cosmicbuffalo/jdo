//! Blocking Jira Cloud REST client (P1).
//!
//! Wraps a blocking HTTP client and the Agile + Platform REST APIs. All
//! requests route through [`super::auth::Auth`] so the API-token and OAuth
//! backends (which use different base URLs) are transparent to callers.
//!
//! Planned methods:
//! - `board_configuration(board_id)` — columns + status mapping
//! - `board_issues(board_id, fields)` — paginated issues
//! - `issue_editmeta(key)` — editable fields, schema, allowed values
//! - `transitions(key)` / `do_transition(key, id, fields)`
//! - `update_fields(key, map)` / `add_comment(key, adf)`
//! - `field_catalog()` — name→id resolution source
