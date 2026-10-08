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
use crate::create::{CreateDraft, CreateError, CreateField, CreateMeta};
use crate::jira::client::{JiraClient, JiraError};
use crate::jira::mapping;
use crate::model::{Board, Comment, Issue, RawFields, RichField, SprintRef, Transition};

/// UI → worker requests.
pub enum Command {
    LoadBoard { board_id: u64 },
    /// Workflow transitions from the issue's current status, plus the issue's
    /// current values for every field on their screens (to pre-fill forms).
    FetchTransitions { key: String },
    /// Perform a transition; on success the issue is re-fetched.
    DoTransition { key: String, body: Value },
    FetchComments { key: String },
    /// Description and acceptance criteria as ADF (the board API only has
    /// plain-text renderings).
    FetchRich { key: String },
    /// Replace a rich-text field with an ADF document.
    UpdateRich { key: String, field: RichField, value: Value },
    /// Jira's create rules for a project's Story issue type.
    FetchCreateMeta { project: String },
    /// People who work on a team's issues (for the Assignee picker).
    FetchTeamMembers { team_id: String },
    CreateIssue { draft: CreateDraft },
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
    /// `(description, acceptance criteria)` as ADF.
    Rich { key: String, result: Result<(Value, Value), String> },
    /// On success, the document that was saved.
    RichUpdated { key: String, field: RichField, result: Result<Value, JiraError> },
    CreateMeta(Result<CreateMeta, String>),
    TeamMembers(Result<Vec<(String, String)>, String>),
    /// The new card, fetched back after creating it.
    Created(Result<Issue, CreateError>),
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
        fields.team.as_str(),
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
            Command::FetchRich { key } => {
                let ids = ["description", fields.acceptance_criteria.as_str()];
                let result = client
                    .issue(&key, &ids)
                    .map(|raw| (raw.fields["description"].clone(), raw.fields[ids[1]].clone()))
                    .map_err(|e| format!("{e:#}"));
                Event::Rich { key, result }
            }
            Command::UpdateRich { key, field, value } => {
                let id = match field {
                    RichField::Description => "description".to_string(),
                    RichField::AcceptanceCriteria => fields.acceptance_criteria.clone(),
                };
                let result = client
                    .update_fields(&key, serde_json::json!({ id: value.clone() }))
                    .map(|()| value);
                Event::RichUpdated { key, field, result }
            }
            Command::FetchCreateMeta { project } => {
                Event::CreateMeta(create_meta(&client, &fields, &project).map_err(|e| format!("{e:#}")))
            }
            Command::FetchTeamMembers { team_id } => {
                Event::TeamMembers(client.team_members(&fields.team, &team_id).map_err(|e| format!("{e:#}")))
            }
            Command::CreateIssue { draft } => {
                let result = client
                    .create_issue(create_fields(&fields, &draft))
                    .map_err(|e| sort_create_error(&fields, &e))
                    .and_then(|key| {
                        client
                            .issue(&key, &field_list(&fields))
                            .map(|raw| mapping::map_issue(&raw, &fields, &browse_base))
                            .map_err(|e| CreateError {
                                general: vec![format!("created {key}, but couldn't load it back: {e:#}")],
                                fields: Vec::new(),
                            })
                    });
                Event::Created(result)
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
    let info = client.board_info(board_id)?;
    let list = field_list(fields);
    let mut sprints = Vec::new();
    let (scope, raw) = if info.board_type == "scrum" {
        sprints = client
            .open_sprints(board_id)?
            .into_iter()
            .map(|s| SprintRef { active: s.state == "active", id: s.id, name: s.name })
            .collect();
        let active: Vec<&SprintRef> = sprints.iter().filter(|s| s.active).collect();
        let mut raw = Vec::new();
        for sprint in &active {
            raw.extend(client.sprint_issues(board_id, sprint.id, &list)?);
        }
        let scope = if active.is_empty() {
            "no active sprint".to_string()
        } else {
            active.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" + ")
        };
        (scope, raw)
    } else {
        let sub_query = config.sub_query.as_ref().map(|q| q.query.trim()).filter(|q| !q.is_empty());
        (String::new(), client.board_issues(board_id, &list, sub_query)?)
    };
    let project = info.location.project_key;
    Ok(mapping::build_board(config, scope, project, sprints, raw, fields, browse_base))
}

/// The dialog field a Jira field id belongs to.
fn create_field_for(fields: &FieldMap, id: &str) -> Option<CreateField> {
    match id {
        "summary" => Some(CreateField::Title),
        "description" => Some(CreateField::Description),
        "parent" | "parentId" => Some(CreateField::Epic),
        "assignee" => Some(CreateField::Assignee),
        id if id == fields.acceptance_criteria => Some(CreateField::Acceptance),
        id if id == fields.team => Some(CreateField::Team),
        id if id == fields.sprint => Some(CreateField::Sprint),
        id if id == fields.epic_link => Some(CreateField::Epic),
        _ => None,
    }
}

/// New cards are Stories; read which fields Jira requires for them.
fn create_meta(client: &JiraClient, fields: &FieldMap, project: &str) -> anyhow::Result<CreateMeta> {
    let types = client.create_issue_types(project)?;
    let (issue_type_id, issue_type_name) = types
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case("story"))
        .or_else(|| types.iter().find(|(_, name)| !matches!(name.as_str(), "Epic" | "Sub-task")))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{project} has no issue type jdo can create"))?;
    let mut meta = CreateMeta { issue_type_id: issue_type_id.clone(), issue_type_name, ..Default::default() };
    for field in client.create_fields(project, &issue_type_id)? {
        let required = field["required"].as_bool().unwrap_or(false);
        let defaulted = field["hasDefaultValue"].as_bool().unwrap_or(false);
        let id = field["fieldId"].as_str().or_else(|| field["key"].as_str()).unwrap_or("");
        if !required || matches!(id, "project" | "issuetype" | "reporter") {
            continue;
        }
        match create_field_for(fields, id) {
            Some(f) => meta.required.push(f),
            None if !defaulted => meta.missing.push(field["name"].as_str().unwrap_or(id).to_string()),
            None => {}
        }
    }
    Ok(meta)
}

/// The create request's `fields`, using the instance's field ids.
fn create_fields(fields: &FieldMap, draft: &CreateDraft) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("project".into(), serde_json::json!({ "key": draft.project_key }));
    out.insert("issuetype".into(), serde_json::json!({ "id": draft.issue_type_id }));
    out.insert("summary".into(), Value::String(draft.summary.clone()));
    if let Some(doc) = &draft.description {
        out.insert("description".into(), doc.clone());
    }
    if let Some(doc) = &draft.acceptance {
        out.insert(fields.acceptance_criteria.clone(), doc.clone());
    }
    if let Some(team) = &draft.team {
        out.insert(fields.team.clone(), Value::String(team.clone()));
    }
    if let Some(sprint) = draft.sprint {
        out.insert(fields.sprint.clone(), serde_json::json!(sprint));
    }
    if let Some(epic) = &draft.epic {
        out.insert("parent".into(), serde_json::json!({ "key": epic }));
    }
    if let Some(account) = &draft.assignee {
        out.insert("assignee".into(), serde_json::json!({ "accountId": account }));
    }
    Value::Object(out)
}

/// Put each of Jira's errors on its dialog field; the rest are general.
fn sort_create_error(fields: &FieldMap, error: &JiraError) -> CreateError {
    let mut sorted = CreateError::default();
    for (id, message) in &error.field_errors {
        match create_field_for(fields, id) {
            Some(field) if !sorted.fields.iter().any(|(f, _)| *f == field) => sorted.fields.push((field, message.clone())),
            Some(_) => {}
            None => sorted.general.push(format!("{id}: {message}")),
        }
    }
    // Jira repeats field errors in `errorMessages` ("Summary: Summary is required.").
    for message in &error.messages {
        if !sorted.fields.iter().any(|(_, m)| message.ends_with(m.as_str())) {
            sorted.general.push(message.clone());
        }
    }
    sorted
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
