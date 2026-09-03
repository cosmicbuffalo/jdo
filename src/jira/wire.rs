//! Serde wire types for the Jira JSON payloads (P1): issues and fields, board
//! configuration, transitions, sprints, comments, and ADF document nodes. Kept
//! separate from the view model in `crate::model` so the API shape can evolve
//! without touching the UI.
