//! Background Jira worker and channel protocol (P1).
//!
//! Keeps the event loop synchronous: a worker thread owns a blocking HTTP
//! client and communicates over two `mpsc` channels. The UI sends [`Command`]s
//! and drains [`Event`]s at the top of the event loop, so network latency never
//! blocks rendering.

use serde_json::Value;

use crate::model::Board;

/// UI → worker requests.
pub enum Command {
    LoadBoard { board_id: u64 },
    Refresh,
    RefreshIssue { key: String },
    Transition { key: String, transition_id: String, fields: Value },
    UpdateField { key: String, field_id: String, value: Value },
    AddComment { key: String, body: Value },
    Shutdown,
}

/// worker → UI results.
pub enum Event {
    Board(Board),
    IssueUpdated { key: String },
    Error(String),
}
