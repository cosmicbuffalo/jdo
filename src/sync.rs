//! Background Jira worker and channel protocol.
//!
//! Keeps the event loop synchronous: a worker thread owns the blocking
//! [`JiraClient`] and communicates over two `mpsc` channels. The UI sends
//! [`Command`]s and drains [`Event`]s at the top of the event loop, so network
//! latency never blocks rendering.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use serde_json::Value;

use crate::config::FieldMap;
use crate::jira::client::JiraClient;
use crate::jira::mapping;
use crate::model::Board;

/// UI → worker requests.
pub enum Command {
    LoadBoard { board_id: u64 },
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

pub struct Worker {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// Signal shutdown and join the worker thread.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The field-id projection requested for each issue.
pub fn field_list(fields: &FieldMap) -> Vec<&str> {
    vec![
        "summary",
        "status",
        "assignee",
        "priority",
        "labels",
        "description",
        fields.story_points.as_str(),
        fields.acceptance_criteria.as_str(),
        fields.sprint.as_str(),
        fields.epic_link.as_str(),
    ]
}

pub fn spawn(client: JiraClient, fields: FieldMap, browse_base: String) -> Worker {
    let (cmd_tx, cmd_rx) = channel::<Command>();
    let (evt_tx, evt_rx) = channel::<Event>();
    let handle = thread::spawn(move || run(client, fields, browse_base, cmd_rx, evt_tx));
    Worker {
        tx: cmd_tx,
        rx: evt_rx,
        handle: Some(handle),
    }
}

fn run(
    client: JiraClient,
    fields: FieldMap,
    browse_base: String,
    cmd_rx: Receiver<Command>,
    evt_tx: Sender<Event>,
) {
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Command::LoadBoard { board_id } => {
                let event = match load_board(&client, &fields, &browse_base, board_id) {
                    Ok(board) => Event::Board(board),
                    Err(err) => Event::Error(format!("{err:#}")),
                };
                if evt_tx.send(event).is_err() {
                    break; // UI gone
                }
            }
            Command::Shutdown => break,
            // Write commands land in P3.
            Command::RefreshIssue { .. }
            | Command::Transition { .. }
            | Command::UpdateField { .. }
            | Command::AddComment { .. } => {}
        }
    }
}

fn load_board(
    client: &JiraClient,
    fields: &FieldMap,
    browse_base: &str,
    board_id: u64,
) -> anyhow::Result<Board> {
    let config = client.board_configuration(board_id)?;
    let list = field_list(fields);
    let raw = client.board_issues(board_id, &list)?;
    Ok(mapping::build_board(config, raw, fields, browse_base))
}
