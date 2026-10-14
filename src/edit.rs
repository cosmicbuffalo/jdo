//! Editing a card's fields from its details.
//!
//! `e` in the details view starts [`FieldEditor`]: a cursor walks the
//! editable fields (`j`/`k`), and `Enter` changes the one under it — a
//! one-line input for the title and story points, a type-to-filter
//! [`Picker`] for assignee, sprint and epic, and for status the workflow
//! transitions open from the card's current status (as in move mode,
//! transitions with screens open their form). Description and acceptance
//! criteria open the Markdown editor instead (see [`crate::app::RichEditor`]).
//! Each change is saved to Jira on its own; the card is re-fetched after.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use tui_textarea::TextArea;

use crate::create::{self, PickOption, Picker};
use crate::model::{Board, Issue, RawFields, RichField, Transition};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditField {
    Summary,
    Status,
    Assignee,
    StoryPoints,
    Sprint,
    Epic,
    Description,
    AcceptanceCriteria,
}

/// In the order the details view shows them.
pub const FIELDS: [EditField; 8] = [
    EditField::Summary,
    EditField::Status,
    EditField::Assignee,
    EditField::StoryPoints,
    EditField::Sprint,
    EditField::Epic,
    EditField::Description,
    EditField::AcceptanceCriteria,
];

impl EditField {
    pub fn label(self) -> &'static str {
        match self {
            EditField::Summary => "Title",
            EditField::Status => "Status",
            EditField::Assignee => "Assignee",
            EditField::StoryPoints => "Story Points",
            EditField::Sprint => "Sprint",
            EditField::Epic => "Epic",
            EditField::Description => "Description",
            EditField::AcceptanceCriteria => "Acceptance Criteria",
        }
    }

    /// The rich-text field this is, for the Markdown editor.
    pub fn rich(self) -> Option<RichField> {
        match self {
            EditField::Description => Some(RichField::Description),
            EditField::AcceptanceCriteria => Some(RichField::AcceptanceCriteria),
            _ => None,
        }
    }
}

/// The change being made to the field under the cursor.
pub enum Active {
    Text(TextArea<'static>),
    Pick(Picker),
    /// The card's workflow: `None` while it loads.
    Status(Option<Workflow>),
}

/// Transitions open from the card's status, as a picker of target statuses.
pub struct Workflow {
    pub picker: Picker,
    pub transitions: Vec<Transition>,
    /// The card's values for the transition screens' fields (to pre-fill forms).
    pub current: RawFields,
}

pub struct FieldEditor {
    pub key: String,
    /// Index into [`FIELDS`].
    pub cursor: usize,
    /// First visible line of the details (kept by the renderer).
    pub scroll: u16,
    pub active: Option<Active>,
    /// The field whose change is on its way to Jira.
    pub saving: Option<EditField>,
    pub error: Option<String>,
}

/// What a key press in the editor asks for.
pub enum EditOutcome {
    None,
    /// Leave edit mode, back to the details view.
    Done,
    /// Open the Markdown editor for a rich-text field.
    Rich(RichField),
    /// Send `value` to Jira for `field`.
    Save(EditField, Value),
    /// Load the card's workflow transitions.
    FetchTransitions,
    /// Perform this transition (its form first, if it has one).
    Transition(Transition, RawFields),
}

impl FieldEditor {
    pub fn new(key: &str, scroll: u16) -> Self {
        Self { key: key.to_string(), cursor: 0, scroll, active: None, saving: None, error: None }
    }

    pub fn field(&self) -> EditField {
        FIELDS[self.cursor]
    }

    pub fn handle_key(&mut self, key: KeyEvent, issue: &Issue, board: &Board, me: Option<&(String, String)>) -> EditOutcome {
        if self.saving.is_some() {
            return EditOutcome::None;
        }
        if self.active.is_some() {
            return self.handle_active_key(key, issue);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('e') => return EditOutcome::Done,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => self.cursor = (self.cursor + 1).min(FIELDS.len() - 1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => self.cursor = FIELDS.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') => return self.activate(issue, board, me),
            _ => {}
        }
        EditOutcome::None
    }

    /// Start changing the field under the cursor.
    fn activate(&mut self, issue: &Issue, board: &Board, me: Option<&(String, String)>) -> EditOutcome {
        self.error = None;
        let field = self.field();
        if let Some(rich) = field.rich() {
            return EditOutcome::Rich(rich);
        }
        if field == EditField::Status {
            self.active = Some(Active::Status(None));
            return EditOutcome::FetchTransitions;
        }
        self.active = Some(match field {
            EditField::Summary => Active::Text(one_line(&issue.summary)),
            EditField::StoryPoints => Active::Text(one_line(&issue.story_points.map(format_points).unwrap_or_default())),
            EditField::Assignee => Active::Pick(highlighting(create::assignee_picker(board, me), issue.assignee_id.as_deref(), || {
                Some(PickOption::new(issue.assignee_id.clone()?, issue.assignee.clone()?))
            })),
            EditField::Sprint => {
                let picker = Picker::new(create::sprint_options(board), 0);
                let current = issue.sprint_id.map(|id| id.to_string());
                Active::Pick(highlighting(picker, current.as_deref(), || {
                    // A closed sprint the board no longer offers.
                    Some(PickOption { note: Some("(current)".into()), ..PickOption::new(current.clone()?, issue.sprint.clone()?) })
                }))
            }
            EditField::Epic => Active::Pick(highlighting(create::epic_picker(board), issue.epic.as_ref().map(|e| e.key.as_str()), || {
                let epic = issue.epic.as_ref()?;
                Some(PickOption { color: epic.color, ..PickOption::new(epic.key.clone(), epic.name.clone()) })
            })),
            EditField::Status | EditField::Description | EditField::AcceptanceCriteria => {
                unreachable!("handled above")
            }
        });
        EditOutcome::None
    }

    fn handle_active_key(&mut self, key: KeyEvent, issue: &Issue) -> EditOutcome {
        let field = self.field();
        if key.code == KeyCode::Esc {
            self.active = None;
            self.error = None;
            return EditOutcome::None;
        }
        match self.active.as_mut().expect("active") {
            Active::Text(area) => {
                if key.code != KeyCode::Enter {
                    // One line: Ctrl-M / Alt-Enter newlines don't apply.
                    if !(key.code == KeyCode::Char('m') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                        area.input(key);
                    }
                    return EditOutcome::None;
                }
                let text = area.lines().join(" ").trim().to_string();
                let value = match field {
                    EditField::Summary if text.is_empty() => {
                        self.error = Some("the title can't be empty".into());
                        return EditOutcome::None;
                    }
                    EditField::Summary if text == issue.summary => return self.unchanged(),
                    EditField::Summary => Value::String(text),
                    _ if text.is_empty() && issue.story_points.is_none() => return self.unchanged(),
                    _ if text.is_empty() => Value::Null,
                    _ => match text.parse::<f64>() {
                        Ok(points) if points >= 0.0 && Some(points) == issue.story_points => return self.unchanged(),
                        Ok(points) if points >= 0.0 => json!(points),
                        _ => {
                            self.error = Some(format!("“{text}” isn't a number of points"));
                            return EditOutcome::None;
                        }
                    },
                };
                self.save(field, value)
            }
            Active::Status(None) => EditOutcome::None,
            Active::Status(Some(workflow)) => {
                if !workflow.picker.handle_key(key) {
                    return EditOutcome::None;
                }
                let id = workflow.picker.chosen.key.clone();
                // The "(current)" row: no change.
                let Some(transition) = workflow.transitions.iter().find(|t| Some(&t.id) == id.as_ref()).cloned() else {
                    return self.unchanged();
                };
                let current = std::mem::take(&mut workflow.current);
                self.active = None;
                EditOutcome::Transition(transition, current)
            }
            Active::Pick(picker) => {
                if !picker.handle_key(key) {
                    return EditOutcome::None;
                }
                let chosen = picker.chosen.key.clone();
                let current = match field {
                    EditField::Assignee => issue.assignee_id.clone(),
                    EditField::Sprint => issue.sprint_id.map(|id| id.to_string()),
                    _ => issue.epic.as_ref().map(|e| e.key.clone()),
                };
                if chosen == current {
                    return self.unchanged();
                }
                let value = match (field, chosen) {
                    (_, None) => Value::Null,
                    (EditField::Assignee, Some(id)) => json!({ "accountId": id }),
                    (EditField::Sprint, Some(id)) => json!(id.parse::<u64>().unwrap_or_default()),
                    (_, Some(key)) => json!({ "key": key }),
                };
                self.save(field, value)
            }
        }
    }

    /// The card's workflow arrived (for the Status field).
    pub fn on_transitions(&mut self, issue: &Issue, result: Result<(Vec<Transition>, RawFields), String>) {
        if !matches!(self.active, Some(Active::Status(None))) {
            return;
        }
        let (transitions, current) = match result {
            Ok(ok) => ok,
            Err(message) => {
                self.active = None;
                self.error = Some(format!("couldn't load the workflow: {message}"));
                return;
            }
        };
        // Looped transitions don't change the status.
        let transitions: Vec<Transition> = transitions.into_iter().filter(|t| t.to_status_id != issue.status_id).collect();
        if transitions.is_empty() {
            self.active = None;
            self.error = Some(format!("no transitions out of {}", issue.status));
            return;
        }
        let mut options = vec![PickOption { note: Some("(current)".into()), ..PickOption::none(issue.status.clone()) }];
        options.extend(transitions.iter().map(|t| PickOption {
            note: (t.name != t.to_status_name).then(|| format!("via “{}”", t.name)),
            ..PickOption::new(t.id.clone(), t.to_status_name.clone())
        }));
        let mut picker = Picker::new(options, 0);
        picker.highlight = 1;
        self.active = Some(Active::Status(Some(Workflow { picker, transitions, current })));
    }

    fn unchanged(&mut self) -> EditOutcome {
        self.active = None;
        EditOutcome::None
    }

    fn save(&mut self, field: EditField, value: Value) -> EditOutcome {
        self.active = None;
        self.saving = Some(field);
        EditOutcome::Save(field, value)
    }
}

/// `picker` with the highlight on `current`, adding it (from `missing`) when
/// the board doesn't offer it.
fn highlighting(mut picker: Picker, current: Option<&str>, missing: impl FnOnce() -> Option<PickOption>) -> Picker {
    if picker.position(current).is_none() {
        if let Some(option) = missing() {
            picker.push(option);
        }
    }
    picker.highlight = picker.position(current).unwrap_or(0);
    picker
}

fn one_line(text: &str) -> TextArea<'static> {
    let mut area = TextArea::new(vec![text.to_string()]);
    area.move_cursor(tui_textarea::CursorMove::End);
    area
}

fn format_points(points: f64) -> String {
    if points.fract() == 0.0 { format!("{points:.0}") } else { points.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, EpicRef, SprintRef};

    fn board() -> Board {
        let mate = Issue {
            key: "P-2".into(),
            assignee: Some("Ana".into()),
            assignee_id: Some("acct-ana".into()),
            epic: Some(EpicRef { key: "P-100".into(), name: "Onboarding".into(), color: Some(3) }),
            ..Default::default()
        };
        Board {
            sprints: vec![
                SprintRef { id: 11, name: "Sprint 11".into(), active: true },
                SprintRef { id: 12, name: "Sprint 12".into(), active: false },
            ],
            columns: vec![Column { issues: vec![mate], ..Default::default() }],
            ..Default::default()
        }
    }

    fn off_board() -> Issue {
        Issue { key: "P-9".into(), summary: "Old title".into(), story_points: Some(3.0), ..Default::default() }
    }

    fn press(editor: &mut FieldEditor, issue: &Issue, code: KeyCode) -> EditOutcome {
        editor.handle_key(KeyEvent::from(code), issue, &board(), Some(&("acct-me".into(), "Me".into())))
    }

    fn type_text(editor: &mut FieldEditor, issue: &Issue, text: &str) {
        for c in text.chars() {
            press(editor, issue, KeyCode::Char(c));
        }
    }

    fn go_to(editor: &mut FieldEditor, field: EditField) {
        editor.cursor = FIELDS.iter().position(|f| *f == field).unwrap();
    }

    #[test]
    fn choosing_the_active_sprint_saves_its_id() {
        let issue = off_board();
        let mut editor = FieldEditor::new("P-9", 0);
        go_to(&mut editor, EditField::Sprint);
        press(&mut editor, &issue, KeyCode::Enter);
        let Some(Active::Pick(picker)) = &editor.active else { panic!("sprint opens a picker") };
        assert_eq!(picker.highlight, 0, "no sprint: the backlog row is highlighted");
        press(&mut editor, &issue, KeyCode::Down); // Sprint 11 (active)
        let EditOutcome::Save(field, value) = press(&mut editor, &issue, KeyCode::Enter) else { panic!("should save") };
        assert_eq!((field, value), (EditField::Sprint, json!(11)));
        assert_eq!(editor.saving, Some(EditField::Sprint));
        assert!(matches!(press(&mut editor, &issue, KeyCode::Char('j')), EditOutcome::None), "keys wait for the save");
    }

    #[test]
    fn the_sprint_picker_filters_and_backlog_clears_the_sprint() {
        let issue = Issue { sprint_id: Some(12), sprint: Some("Sprint 12".into()), ..off_board() };
        let mut editor = FieldEditor::new("P-9", 0);
        go_to(&mut editor, EditField::Sprint);
        press(&mut editor, &issue, KeyCode::Enter);
        let Some(Active::Pick(picker)) = &editor.active else { panic!() };
        assert_eq!(picker.highlight, 2, "starts on the issue's sprint");
        press(&mut editor, &issue, KeyCode::Up);
        press(&mut editor, &issue, KeyCode::Up);
        let EditOutcome::Save(_, value) = press(&mut editor, &issue, KeyCode::Enter) else { panic!() };
        assert_eq!(value, Value::Null);
    }

    #[test]
    fn picking_the_current_value_or_escaping_saves_nothing() {
        let issue = Issue { sprint_id: Some(11), ..off_board() };
        let mut editor = FieldEditor::new("P-9", 0);
        go_to(&mut editor, EditField::Sprint);
        press(&mut editor, &issue, KeyCode::Enter);
        assert!(matches!(press(&mut editor, &issue, KeyCode::Enter), EditOutcome::None));
        assert!(editor.active.is_none() && editor.saving.is_none());
        press(&mut editor, &issue, KeyCode::Enter);
        assert!(matches!(press(&mut editor, &issue, KeyCode::Esc), EditOutcome::None));
        assert!(editor.active.is_none(), "esc closes the picker, not edit mode");
        assert!(matches!(press(&mut editor, &issue, KeyCode::Esc), EditOutcome::Done));
    }

    #[test]
    fn title_and_points_are_validated_one_line_inputs() {
        let issue = off_board();
        let mut editor = FieldEditor::new("P-9", 0);
        press(&mut editor, &issue, KeyCode::Enter); // Title
        type_text(&mut editor, &issue, " v2");
        let EditOutcome::Save(field, value) = press(&mut editor, &issue, KeyCode::Enter) else { panic!() };
        assert_eq!((field, value), (EditField::Summary, json!("Old title v2")));

        let mut editor = FieldEditor::new("P-9", 0);
        go_to(&mut editor, EditField::StoryPoints);
        press(&mut editor, &issue, KeyCode::Enter);
        press(&mut editor, &issue, KeyCode::Backspace);
        type_text(&mut editor, &issue, "lots");
        assert!(matches!(press(&mut editor, &issue, KeyCode::Enter), EditOutcome::None));
        assert!(editor.error.as_deref().unwrap().contains("isn't a number"));
        let Some(Active::Text(area)) = &mut editor.active else { panic!("input stays open") };
        *area = one_line("5");
        let EditOutcome::Save(_, value) = press(&mut editor, &issue, KeyCode::Enter) else { panic!() };
        assert_eq!(value, json!(5.0));
    }

    #[test]
    fn assignee_and_epic_send_jira_shapes_and_rich_fields_open_the_markdown_editor() {
        let issue = off_board();
        let mut editor = FieldEditor::new("P-9", 0);
        go_to(&mut editor, EditField::Assignee);
        press(&mut editor, &issue, KeyCode::Enter);
        type_text(&mut editor, &issue, "ana");
        let EditOutcome::Save(_, value) = press(&mut editor, &issue, KeyCode::Enter) else { panic!() };
        assert_eq!(value, json!({ "accountId": "acct-ana" }));

        let mut editor = FieldEditor::new("P-9", 0);
        go_to(&mut editor, EditField::Epic);
        press(&mut editor, &issue, KeyCode::Enter);
        type_text(&mut editor, &issue, "onb");
        let EditOutcome::Save(_, value) = press(&mut editor, &issue, KeyCode::Enter) else { panic!() };
        assert_eq!(value, json!({ "key": "P-100" }));

        let mut editor = FieldEditor::new("P-9", 0);
        go_to(&mut editor, EditField::AcceptanceCriteria);
        assert!(matches!(press(&mut editor, &issue, KeyCode::Enter), EditOutcome::Rich(RichField::AcceptanceCriteria)));
    }
}
