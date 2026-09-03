//! Jira integration layer.
//!
//! Populated from P1 onward. Splits into: the REST client,
//! the two auth backends, wire (serde) types, ADF read/write, board↔view-model
//! mapping, and per-project field-id resolution.

pub mod adf;
pub mod auth;
pub mod client;
pub mod fields;
pub mod mapping;
pub mod wire;
