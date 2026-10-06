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
use crate::jira::client::{JiraClient, JiraError};
use crate::jira::mapping;
use crate::model::{Board, Comment, Issue, RawFields, Transition};

/// UI → worker requests.
pub enum Command {
    LoadBoard { board_id: u64 },
    /// Workflow transitions from the issue's current status, plus the issue's
    /// current values for every field on their screens (to pre-fill forms).
    FetchTransitions { key: String },
    /// Perform a transition; on success the issue is re-fetched.
    DoTransition { key: String, body: Value },
    FetchComments { key: String },
    Shutdown,
}

/// worker → UI results.
pub enum Event {
    Board(Board),
    BoardError(String),
    Myself { account_id: String, display_name: String },
    Transitions {
        key: String,
        result: Result<(Vec<Transition>, RawFields), String>,
    },
    TransitionDone { key: String, result: Result<(), JiraError> },
    IssueRefreshed(Issue),
    Comments { key: String, result: Result<Vec<Comment>, String> },
}

pub struct Worker {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn send(&self, command: Command) {
        // A dead worker surfaces as missing responses; nothing useful to do here.
        let _ = self.tx.send(command);
    }

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
        "issuetype",
        "status",
        "assignee",
        "priority",
        "labels",
        "description",
        fields.story_points.as_str(),
        fields.acceptance_criteria.as_str(),
        fields.sprint.as_str(),
        fields.epic_link.as_str(),
        "epic",
        "parent",
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
    // Needed for "assign to me" in transition forms; failure is non-fatal.
    if let Ok(me) = client.myself() {
        let _ = evt_tx.send(Event::Myself {
            account_id: me.account_id,
            display_name: me.display_name,
        });
    }

    while let Ok(cmd) = cmd_rx.recv() {
        let event = match cmd {
            Command::Shutdown => break,
            Command::LoadBoard { board_id } => {
                match load_board(&client, &fields, &browse_base, board_id) {
                    Ok(board) => Event::Board(board),
                    Err(err) => Event::BoardError(format!("{err:#}")),
                }
            }
            Command::FetchTransitions { key } => {
                let result = fetch_transitions(&client, &key).map_err(|e| format!("{e:#}"));
                Event::Transitions { key, result }
            }
            Command::DoTransition { key, body } => {
                let result = client.do_transition(&key, &body);
                let ok = result.is_ok();
                if evt_tx.send(Event::TransitionDone { key: key.clone(), result }).is_err() {
                    break;
                }
                if !ok {
                    continue;
                }
                match client.issue(&key, &field_list(&fields)) {
                    Ok(raw) => Event::IssueRefreshed(mapping::map_issue(&raw, &fields, &browse_base)),
                    // The move succeeded; a stale card until the next refresh is fine.
                    Err(_) => continue,
                }
            }
            Command::FetchComments { key } => {
                let result = client
                    .comments(&key)
                    .map(|page| mapping::map_comments(page.comments))
                    .map_err(|e| format!("{e:#}"));
                Event::Comments { key, result }
            }
        };
        if evt_tx.send(event).is_err() {
            break; // UI gone
        }
    }
}

/// Load what the Jira web board shows: the active sprint(s) for scrum boards,
/// otherwise the board's issues narrowed by its sub-filter (kanban).
pub fn load_board(
    client: &JiraClient,
    fields: &FieldMap,
    browse_base: &str,
    board_id: u64,
) -> anyhow::Result<Board> {
    let config = client.board_configuration(board_id)?;
    let list = field_list(fields);
    let (scope, raw) = if client.board_info(board_id)?.board_type == "scrum" {
        let sprints = client.active_sprints(board_id)?;
        let mut raw = Vec::new();
        for sprint in &sprints {
            raw.extend(client.sprint_issues(board_id, sprint.id, &list)?);
        }
        let scope = if sprints.is_empty() {
            "no active sprint".to_string()
        } else {
            sprints.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" + ")
        };
        (scope, raw)
    } else {
        let sub_query = config.sub_query.as_ref().map(|q| q.query.trim()).filter(|q| !q.is_empty());
        (String::new(), client.board_issues(board_id, &list, sub_query)?)
    };
    Ok(mapping::build_board(config, scope, raw, fields, browse_base))
}

fn fetch_transitions(client: &JiraClient, key: &str) -> anyhow::Result<(Vec<Transition>, RawFields)> {
    let transitions = mapping::map_transitions(client.transitions(key)?.transitions);
    let mut screen_ids: Vec<&str> = transitions
        .iter()
        .flat_map(|t| t.fields.iter().map(|f| f.id.as_str()))
        .collect();
    screen_ids.sort_unstable();
    screen_ids.dedup();
    let current = if screen_ids.is_empty() {
        Value::Object(Default::default())
    } else {
        client.issue(key, &screen_ids)?.fields
    };
    Ok((transitions, current))
}
