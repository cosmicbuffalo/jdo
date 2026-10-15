//! Jira integration layer.
//!
//! See `docs/DESIGN.md`. Splits into: the REST client,
//! the two auth backends, wire (serde) types, ADF read/write, board↔view-model
//! mapping, and per-project field-id resolution.

pub mod adf;
pub mod auth;
pub mod client;
pub mod fields;
pub mod mapping;
pub mod markdown;
pub mod wire;
