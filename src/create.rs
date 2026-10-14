//! Creating a card.
//!
//! [`CreateForm`] is the "Create new <PROJECT> ticket" dialog: Team, Sprint,
//! Epic, Assignee, Title, Description and Acceptance Criteria. The four
//! choice fields share one type-to-filter [`Picker`]. Team and Sprint default
//! to the board's; Epic offers the board's epics (a typed key that isn't on
//! the board can be used as is, and Jira validates it); Assignee offers the
//! board team's members. Jira's create rules arrive as [`CreateMeta`]:
//! required fields are marked and checked before submitting, and a required
//! field the dialog doesn't offer blocks submitting with an explanation.
//! Jira's rejections come back as [`CreateError`] and land on their fields.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use tui_textarea::TextArea;

use crate::jira::markdown;
use crate::model::{Board, EpicRef, TeamRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CreateField {
    Team,
    Sprint,
    Epic,
    Assignee,
    Title,
    Description,
    Acceptance,
}

impl CreateField {
    pub fn label(self) -> &'static str {
        match self {
            CreateField::Team => "Team",
            CreateField::Sprint => "Sprint",
            CreateField::Epic => "Epic",
            CreateField::Assignee => "Assignee",
            CreateField::Title => "Title",
            CreateField::Description => "Description",
            CreateField::Acceptance => "Acceptance Criteria",
        }
    }

    pub fn is_picker(self) -> bool {
        matches!(self, CreateField::Team | CreateField::Sprint | CreateField::Epic | CreateField::Assignee)
    }
}

/// Where the cursor is: a field, or a footer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateFocus {
    Field(CreateField),
    Submit,
    Cancel,
}

/// Top to bottom, then the buttons left to right.
const ORDER: [CreateFocus; 9] = [
    CreateFocus::Field(CreateField::Team),
    CreateFocus::Field(CreateField::Sprint),
    CreateFocus::Field(CreateField::Epic),
    CreateFocus::Field(CreateField::Assignee),
    CreateFocus::Field(CreateField::Title),
    CreateFocus::Field(CreateField::Description),
    CreateFocus::Field(CreateField::Acceptance),
    CreateFocus::Cancel,
    CreateFocus::Submit,
];

/// Jira's create rules for the project, from the create metadata.
#[derive(Debug, Clone, Default)]
pub struct CreateMeta {
    pub issue_type_id: String,
    pub issue_type_name: String,
    /// Dialog fields Jira requires.
    pub required: Vec<CreateField>,
    /// Required fields the dialog doesn't offer (and Jira won't default).
    pub missing: Vec<String>,
}

/// What gets sent: plain values; the worker maps them to field ids.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateDraft {
    pub project_key: String,
    pub issue_type_id: String,
    pub summary: String,
    pub description: Option<Value>,
    pub acceptance: Option<Value>,
    pub team: Option<String>,
    pub sprint: Option<u64>,
    pub epic: Option<String>,
    pub assignee: Option<String>,
}

/// A rejected create, already sorted onto the dialog's fields.
#[derive(Debug, Clone, Default)]
pub struct CreateError {
    pub general: Vec<String>,
    pub fields: Vec<(CreateField, String)>,
}

/// One choice in a [`Picker`]. `key: None` is the "none" choice ("No epic",
/// "Unassigned", ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickOption {
    /// Team id, sprint id, epic key, or account id.
    pub key: Option<String>,
    pub label: String,
    /// Shown after the label, e.g. "(active)".
    pub note: Option<String>,
    /// Jira epic color slot, for epics.
    pub color: Option<usize>,
    /// A typed epic key that isn't on the board (Jira checks it on submit).
    pub typed: bool,
}

impl PickOption {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self { key: Some(key.into()), label: label.into(), note: None, color: None, typed: false }
    }

    pub fn none(label: impl Into<String>) -> Self {
        Self { key: None, label: label.into(), note: None, color: None, typed: false }
    }

    fn matches(&self, filter: &str) -> bool {
        self.label.to_lowercase().contains(filter) || self.key.as_deref().is_some_and(|k| k.to_lowercase().contains(filter))
    }
}

/// A type-to-filter dropdown: typing filters by label or key, ↑/↓ move the
/// highlight, Enter chooses.
#[derive(Debug, Clone)]
pub struct Picker {
    options: Vec<PickOption>,
    pub chosen: PickOption,
    pub filter: String,
    pub highlight: usize,
    /// Offer a typed issue key that isn't in the list (epics).
    typed_keys: bool,
    /// Options still arriving (the team roster).
    pub loading: bool,
}

impl Picker {
    /// `options[0]` should be the "none" choice; `chosen` indexes `options`.
    pub fn new(options: Vec<PickOption>, chosen: usize) -> Self {
        let chosen = options.get(chosen).cloned().unwrap_or_else(|| PickOption::none("None"));
        Self { options, chosen, filter: String::new(), highlight: 0, typed_keys: false, loading: false }
    }

    /// The options matching the filter. The "none" choice only shows while
    /// the filter is empty.
    pub fn matches(&self) -> Vec<PickOption> {
        let filter = self.filter.trim().to_lowercase();
        if filter.is_empty() {
            return self.options.clone();
        }
        let mut found: Vec<PickOption> = self.options.iter().filter(|o| o.key.is_some() && o.matches(&filter)).cloned().collect();
        let typed = self.filter.trim().to_uppercase();
        if self.typed_keys && looks_like_key(&typed) && !found.iter().any(|o| o.key.as_deref() == Some(typed.as_str())) {
            found.push(PickOption { typed: true, note: Some("not on this board — Jira will check it".into()), ..PickOption::new(typed.clone(), "") });
        }
        found
    }

    /// Index of the option with this key (`None` is the "none" choice).
    pub fn position(&self, key: Option<&str>) -> Option<usize> {
        self.options.iter().position(|o| o.key.as_deref() == key)
    }

    pub fn choose(&mut self, index: usize) -> bool {
        match self.matches().get(index).cloned() {
            Some(option) => {
                self.chosen = option;
                self.filter.clear();
                self.highlight = 0;
                true
            }
            None => false,
        }
    }

    /// Returns true when a choice was made.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.matches().len();
        match key.code {
            KeyCode::Up => self.highlight = self.highlight.saturating_sub(1),
            KeyCode::Down => self.highlight = (self.highlight + 1).min(count.saturating_sub(1)),
            KeyCode::Enter => return self.choose(self.highlight.min(count.saturating_sub(1))),
            KeyCode::Backspace => {
                self.filter.pop();
                self.highlight = 0;
            }
            KeyCode::Char('u') if ctrl => {
                self.filter.clear();
                self.highlight = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.highlight = 0;
            }
            _ => {}
        }
        false
    }

    /// Add an option at the end, keeping the order.
    pub fn push(&mut self, option: PickOption) {
        self.options.push(option);
    }

    /// Add options that arrived later (e.g. the team roster), keeping the
    /// choice and skipping ones already listed.
    pub fn extend(&mut self, options: impl IntoIterator<Item = PickOption>) {
        for option in options {
            if !self.options.iter().any(|o| o.key == option.key) {
                self.options.push(option);
            }
        }
        let (none, mut rest): (Vec<PickOption>, Vec<PickOption>) = self.options.drain(..).partition(|o| o.key.is_none());
        rest.sort_by_key(|o| o.label.to_lowercase());
        self.options = none.into_iter().chain(rest).collect();
        self.loading = false;
    }
}

pub enum CreateOutcome {
    None,
    Submit(CreateDraft),
    Cancel,
    EditExternally,
}

pub struct CreateForm {
    pub project_key: String,
    pub team: Picker,
    pub sprint: Picker,
    pub epic: Picker,
    pub assignee: Picker,
    pub title: TextArea<'static>,
    pub description: TextArea<'static>,
    pub acceptance: TextArea<'static>,
    pub focus: CreateFocus,
    pub meta: Option<CreateMeta>,
    pub meta_error: Option<String>,
    pub field_errors: HashMap<CreateField, String>,
    pub errors: Vec<String>,
    pub submitting: bool,
    /// Esc pressed once with unsaved input; a second Esc discards.
    pub confirm_discard: bool,
}

impl CreateForm {
    /// Defaults come from the board: its most common team, its active sprint,
    /// and `epic` (e.g. the epic section the cursor was in). `me` is
    /// `(accountId, displayName)`.
    pub fn new(board: &Board, epic: Option<&EpicRef>, me: Option<&(String, String)>) -> Self {
        let issues = || board.columns.iter().flat_map(|c| &c.issues);

        // Teams on the board, most common first (the default).
        let mut counts: Vec<(TeamRef, usize)> = Vec::new();
        for team in issues().filter_map(|i| i.team.as_ref()) {
            match counts.iter_mut().find(|(t, _)| t.id == team.id) {
                Some((_, n)) => *n += 1,
                None => counts.push((team.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
        let mut teams = vec![PickOption::none("No team")];
        teams.extend(counts.into_iter().map(|(t, _)| PickOption::new(t.id, t.name)));
        let team = Picker::new(teams, 1);

        let sprints = sprint_options(board);
        let active = board.sprints.iter().position(|s| s.active).map(|i| i + 1).unwrap_or(0);
        let sprint = Picker::new(sprints, active);

        let default_epic = epic.map(|e| e.key.as_str());
        let mut epic = epic_picker(board);
        if let Some(i) = default_epic.and_then(|key| epic.position(Some(key))) {
            epic.chosen = epic.options[i].clone();
        }

        // The board's people now; the team roster is added when it arrives.
        let mut assignee = assignee_picker(board, me);
        assignee.loading = true;

        Self {
            project_key: board.project_key.clone(),
            team,
            sprint,
            epic,
            assignee,
            title: TextArea::default(),
            description: TextArea::default(),
            acceptance: TextArea::default(),
            focus: CreateFocus::Field(CreateField::Epic),
            meta: None,
            meta_error: None,
            field_errors: HashMap::new(),
            errors: Vec::new(),
            submitting: false,
            confirm_discard: false,
        }
    }

    pub fn picker(&self, field: CreateField) -> Option<&Picker> {
        match field {
            CreateField::Team => Some(&self.team),
            CreateField::Sprint => Some(&self.sprint),
            CreateField::Epic => Some(&self.epic),
            CreateField::Assignee => Some(&self.assignee),
            _ => None,
        }
    }

    fn picker_mut(&mut self, field: CreateField) -> Option<&mut Picker> {
        match field {
            CreateField::Team => Some(&mut self.team),
            CreateField::Sprint => Some(&mut self.sprint),
            CreateField::Epic => Some(&mut self.epic),
            CreateField::Assignee => Some(&mut self.assignee),
            _ => None,
        }
    }

    /// The team whose members the Assignee picker should offer.
    pub fn team_id(&self) -> Option<String> {
        self.team.chosen.key.clone()
    }

    /// Add the team roster to the Assignee picker.
    pub fn add_members(&mut self, members: Vec<(String, String)>) {
        self.assignee.extend(members.into_iter().map(|(id, name)| PickOption::new(id, name)));
    }

    pub fn required(&self, field: CreateField) -> bool {
        // Jira always requires a summary, even before the metadata arrives.
        field == CreateField::Title || self.meta.as_ref().is_some_and(|m| m.required.contains(&field))
    }

    fn has_input(&self) -> bool {
        let filled = |t: &TextArea| t.lines().iter().any(|l| !l.trim().is_empty());
        filled(&self.title) || filled(&self.description) || filled(&self.acceptance)
    }

    pub fn text(area: &TextArea) -> String {
        area.lines().join("\n").trim().to_string()
    }

    /// Check what can be checked locally, annotating fields.
    pub fn validate(&mut self) -> bool {
        self.field_errors.clear();
        self.errors.clear();
        let Some(meta) = &self.meta else {
            self.errors.push(match &self.meta_error {
                Some(e) => format!("couldn't load Jira's create rules: {e}"),
                None => "still loading Jira's create rules…".into(),
            });
            return false;
        };
        if !meta.missing.is_empty() {
            self.errors.push(format!(
                "Jira requires fields this dialog doesn't offer ({}) — create this ticket in Jira",
                meta.missing.join(", ")
            ));
        }
        let empty = [
            (CreateField::Title, Self::text(&self.title).is_empty()),
            (CreateField::Description, Self::text(&self.description).is_empty()),
            (CreateField::Acceptance, Self::text(&self.acceptance).is_empty()),
            (CreateField::Team, self.team.chosen.key.is_none()),
            (CreateField::Sprint, self.sprint.chosen.key.is_none()),
            (CreateField::Epic, self.epic.chosen.key.is_none()),
            (CreateField::Assignee, self.assignee.chosen.key.is_none()),
        ];
        for (field, is_empty) in empty {
            if is_empty && self.required(field) {
                self.field_errors.insert(field, format!("{} is required", field.label()));
            }
        }
        self.errors.is_empty() && self.field_errors.is_empty()
    }

    pub fn draft(&self) -> CreateDraft {
        let rich = |area: &TextArea| {
            let text = Self::text(area);
            (!text.is_empty()).then(|| markdown::to_adf(&text, &[]))
        };
        CreateDraft {
            project_key: self.project_key.clone(),
            issue_type_id: self.meta.as_ref().map(|m| m.issue_type_id.clone()).unwrap_or_default(),
            summary: Self::text(&self.title),
            description: rich(&self.description),
            acceptance: rich(&self.acceptance),
            team: self.team.chosen.key.clone(),
            sprint: self.sprint.chosen.key.as_deref().and_then(|k| k.parse().ok()),
            epic: self.epic.chosen.key.clone(),
            assignee: self.assignee.chosen.key.clone(),
        }
    }

    pub fn apply_error(&mut self, error: &CreateError) {
        self.submitting = false;
        self.errors = error.general.clone();
        self.field_errors = error.fields.iter().cloned().collect();
        if self.errors.is_empty() && self.field_errors.is_empty() {
            self.errors.push("Jira rejected the ticket".into());
        }
        if let Some(field) = ORDER.iter().find_map(|f| match f {
            CreateFocus::Field(field) if self.field_errors.contains_key(field) => Some(*field),
            _ => None,
        }) {
            self.focus = CreateFocus::Field(field);
        }
    }

    /// The text area under the cursor, if any.
    pub fn focused_text(&mut self) -> Option<&mut TextArea<'static>> {
        match self.focus {
            CreateFocus::Field(CreateField::Title) => Some(&mut self.title),
            CreateFocus::Field(CreateField::Description) => Some(&mut self.description),
            CreateFocus::Field(CreateField::Acceptance) => Some(&mut self.acceptance),
            _ => None,
        }
    }

    pub fn step_focus(&mut self, forward: bool) {
        // Leaving a picker drops its half-typed filter.
        if let CreateFocus::Field(field) = self.focus {
            if let Some(picker) = self.picker_mut(field) {
                picker.filter.clear();
                picker.highlight = 0;
            }
        }
        let at = ORDER.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward { (at + 1).min(ORDER.len() - 1) } else { at.saturating_sub(1) };
        self.focus = ORDER[next];
    }

    /// Choose row `index` of a picker's filtered list (mouse), then move on.
    pub fn choose(&mut self, field: CreateField, index: usize) {
        if self.picker_mut(field).is_some_and(|p| p.choose(index)) {
            self.field_errors.remove(&field);
            self.focus = CreateFocus::Field(field);
            self.step_focus(true);
        }
    }

    pub fn try_submit(&mut self) -> CreateOutcome {
        if self.submitting || !self.validate() {
            return CreateOutcome::None;
        }
        self.submitting = true;
        CreateOutcome::Submit(self.draft())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CreateOutcome {
        if self.submitting {
            return CreateOutcome::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code != KeyCode::Esc {
            self.confirm_discard = false;
        }
        match key.code {
            KeyCode::Esc => {
                if self.has_input() && !self.confirm_discard {
                    self.confirm_discard = true;
                    return CreateOutcome::None;
                }
                return CreateOutcome::Cancel;
            }
            KeyCode::Char('s') if ctrl => return self.try_submit(),
            KeyCode::Char('g') if ctrl => {
                if matches!(self.focus, CreateFocus::Field(CreateField::Description | CreateField::Acceptance)) {
                    return CreateOutcome::EditExternally;
                }
                return CreateOutcome::None;
            }
            KeyCode::Tab => {
                self.step_focus(true);
                return CreateOutcome::None;
            }
            KeyCode::BackTab => {
                self.step_focus(false);
                return CreateOutcome::None;
            }
            _ => {}
        }

        match self.focus {
            CreateFocus::Field(field) if field.is_picker() => {
                let picker = self.picker_mut(field).expect("picker field");
                // With nothing typed, ↑ on the first row / ↓ on the last leave the field.
                let count = picker.matches().len();
                let idle = picker.filter.is_empty();
                match key.code {
                    KeyCode::Up if idle && picker.highlight == 0 => self.step_focus(false),
                    KeyCode::Down if idle && picker.highlight + 1 >= count => self.step_focus(true),
                    _ => {
                        if picker.handle_key(key) {
                            self.field_errors.remove(&field);
                            self.step_focus(true);
                        }
                    }
                }
            }
            CreateFocus::Field(field) => {
                let single_line = field == CreateField::Title;
                // Up on the first line / Down on the last line leave the field.
                let (row, last_row) = self
                    .focused_text()
                    .map(|area| (area.cursor().0, area.lines().len().saturating_sub(1)))
                    .unwrap_or((0, 0));
                match key.code {
                    // Title is one line: Enter moves on.
                    KeyCode::Enter if single_line => self.step_focus(true),
                    KeyCode::Up if row == 0 => self.step_focus(false),
                    KeyCode::Down if row == last_row => self.step_focus(true),
                    _ => {
                        self.field_errors.remove(&field);
                        if let Some(area) = self.focused_text() {
                            area.input(key);
                        }
                    }
                }
            }
            CreateFocus::Submit | CreateFocus::Cancel => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                    self.focus = if self.focus == CreateFocus::Submit { CreateFocus::Cancel } else { CreateFocus::Submit };
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.focus = CreateFocus::Field(CreateField::Acceptance);
                }
                KeyCode::Enter | KeyCode::Char(' ') if self.focus == CreateFocus::Submit => return self.try_submit(),
                KeyCode::Enter | KeyCode::Char(' ') => return CreateOutcome::Cancel,
                _ => {}
            },
        }
        CreateOutcome::None
    }
}

/// "No sprint (backlog)", then the board's active and future sprints in its order.
pub fn sprint_options(board: &Board) -> Vec<PickOption> {
    let mut sprints = vec![PickOption::none("No sprint (backlog)")];
    sprints.extend(board.sprints.iter().map(|s| PickOption {
        note: s.active.then(|| "(active)".to_string()),
        ..PickOption::new(s.id.to_string(), s.name.clone())
    }));
    sprints
}

/// The board's epics (and "No epic"); a typed key that isn't listed can be used too.
pub fn epic_picker(board: &Board) -> Picker {
    let mut epics = vec![PickOption::none("No epic")];
    for e in board.columns.iter().flat_map(|c| &c.issues).filter_map(|i| i.epic.as_ref()) {
        if !epics.iter().any(|o| o.key.as_deref() == Some(e.key.as_str())) {
            epics.push(PickOption { color: e.color, ..PickOption::new(e.key.clone(), e.name.clone()) });
        }
    }
    epics[1..].sort_by_key(|o| o.label.to_lowercase());
    let mut picker = Picker::new(epics, 0);
    picker.typed_keys = true;
    picker
}

/// "Unassigned", the board's people and me, by name.
pub fn assignee_picker(board: &Board, me: Option<&(String, String)>) -> Picker {
    let mut picker = Picker::new(vec![PickOption::none("Unassigned")], 0);
    let people = board
        .columns
        .iter()
        .flat_map(|c| &c.issues)
        .filter_map(|i| Some(PickOption::new(i.assignee_id.clone()?, i.assignee.clone()?)))
        .chain(me.map(|(id, name)| PickOption::new(id.clone(), name.clone())));
    picker.extend(people);
    picker
}

/// `ABC-123`: letters/digits/underscore, a dash, digits.
pub fn looks_like_key(text: &str) -> bool {
    let Some((project, number)) = text.split_once('-') else { return false };
    !project.is_empty()
        && project.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && project.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, Issue, SprintRef};

    fn team(id: &str) -> TeamRef {
        TeamRef { id: id.into(), name: format!("team-{id}") }
    }

    fn issue(key: &str, epic: Option<(&str, &str)>, team_id: &str, who: Option<(&str, &str)>) -> Issue {
        Issue {
            key: key.into(),
            epic: epic.map(|(k, n)| EpicRef { key: k.into(), name: n.into(), color: Some(3) }),
            team: Some(team(team_id)),
            assignee_id: who.map(|(id, _)| id.to_string()),
            assignee: who.map(|(_, name)| name.to_string()),
            ..Default::default()
        }
    }

    fn board() -> Board {
        Board {
            project_key: "DEMO".into(),
            sprints: vec![
                SprintRef { id: 7, name: "S350".into(), active: true },
                SprintRef { id: 8, name: "S351".into(), active: false },
            ],
            columns: vec![Column {
                issues: vec![
                    issue("D-1", Some(("D-100", "Minor Refactoring")), "main", Some(("acct-riley", "Riley Chen"))),
                    issue("D-2", Some(("D-200", "On-call Support")), "main", Some(("acct-jamie", "Jamie Park"))),
                    issue("D-3", None, "other", None),
                ],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn meta() -> CreateMeta {
        CreateMeta {
            issue_type_id: "20".into(),
            issue_type_name: "Story".into(),
            required: vec![CreateField::Title],
            missing: Vec::new(),
        }
    }

    fn form() -> CreateForm {
        CreateForm::new(&board(), None, Some(&("acct-noor".to_string(), "Noor Haddad".to_string())))
    }

    fn press(form: &mut CreateForm, code: KeyCode) -> CreateOutcome {
        form.handle_key(KeyEvent::from(code))
    }

    fn type_text(form: &mut CreateForm, text: &str) {
        for c in text.chars() {
            press(form, KeyCode::Char(c));
        }
    }

    fn keys(picker: &Picker) -> Vec<Option<String>> {
        picker.matches().into_iter().map(|o| o.key).collect()
    }

    #[test]
    fn defaults_come_from_the_board_and_the_cursor_starts_on_epic() {
        let form = form();
        assert_eq!(form.team.chosen.key.as_deref(), Some("main"), "the most common team");
        assert_eq!(form.sprint.chosen.key.as_deref(), Some("7"), "the active sprint");
        assert_eq!(form.epic.chosen.key, None);
        assert_eq!(form.assignee.chosen.label, "Unassigned");
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Epic));
    }

    #[test]
    fn epic_defaults_to_the_section_the_cursor_was_in() {
        let epic = EpicRef { key: "D-200".into(), name: "On-call Support".into(), color: None };
        let form = CreateForm::new(&board(), Some(&epic), None);
        assert_eq!(form.epic.chosen.key.as_deref(), Some("D-200"));
    }

    #[test]
    fn every_picker_filters_as_you_type() {
        let mut form = form();
        type_text(&mut form, "refac"); // Epic
        assert_eq!(keys(&form.epic), [Some("D-100".to_string())]);

        form.focus = CreateFocus::Field(CreateField::Sprint);
        type_text(&mut form, "351");
        assert_eq!(keys(&form.sprint), [Some("8".to_string())]);
        press(&mut form, KeyCode::Enter);
        assert_eq!(form.sprint.chosen.key.as_deref(), Some("8"));
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Epic), "choosing moves to the next field");

        form.focus = CreateFocus::Field(CreateField::Team);
        type_text(&mut form, "other");
        assert_eq!(keys(&form.team), [Some("other".to_string())]);

        form.focus = CreateFocus::Field(CreateField::Assignee);
        type_text(&mut form, "jam");
        assert_eq!(keys(&form.assignee), [Some("acct-jamie".to_string())]);
    }

    #[test]
    fn typed_epic_keys_are_offered_but_not_for_other_pickers() {
        let mut form = form();
        type_text(&mut form, "demo-9999");
        let options = form.epic.matches();
        assert!(options[0].typed && options[0].key.as_deref() == Some("DEMO-9999"));

        form.focus = CreateFocus::Field(CreateField::Assignee);
        type_text(&mut form, "demo-9999");
        assert!(form.assignee.matches().is_empty());
    }

    #[test]
    fn assignees_are_the_board_people_plus_me_then_the_team_roster() {
        let mut form = form();
        let names: Vec<String> = form.assignee.matches().into_iter().map(|o| o.label).collect();
        assert_eq!(names, ["Unassigned", "Jamie Park", "Noor Haddad", "Riley Chen"]);
        assert!(form.assignee.loading);

        form.add_members(vec![("acct-dana".into(), "Dana Reyes".into()), ("acct-riley".into(), "Riley Chen".into())]);
        let names: Vec<String> = form.assignee.matches().into_iter().map(|o| o.label).collect();
        assert_eq!(names, ["Unassigned", "Dana Reyes", "Jamie Park", "Noor Haddad", "Riley Chen"]);
        assert!(!form.assignee.loading);
    }

    #[test]
    fn idle_pickers_hand_arrows_to_field_navigation() {
        let mut form = form();
        form.focus = CreateFocus::Field(CreateField::Sprint);
        press(&mut form, KeyCode::Up); // highlight already on row 0
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Team));
        press(&mut form, KeyCode::Down); // Team list: highlight 0 of 3 -> moves within
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Team));
        press(&mut form, KeyCode::Down);
        press(&mut form, KeyCode::Down); // last row: on to Sprint
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Sprint));
    }

    #[test]
    fn title_is_required_and_submit_waits_for_jiras_rules() {
        let mut form = form();
        assert!(matches!(form.try_submit(), CreateOutcome::None));
        assert!(form.errors[0].contains("loading"), "{:?}", form.errors);

        form.meta = Some(meta());
        assert!(matches!(form.try_submit(), CreateOutcome::None));
        assert_eq!(form.field_errors.get(&CreateField::Title).map(String::as_str), Some("Title is required"));
    }

    #[test]
    fn required_fields_the_dialog_lacks_block_submitting() {
        let mut form = form();
        form.meta = Some(CreateMeta { missing: vec!["Business Justification".into()], ..meta() });
        form.title = TextArea::new(vec!["A title".into()]);
        assert!(matches!(form.try_submit(), CreateOutcome::None));
        assert!(form.errors[0].contains("Business Justification"));
    }

    #[test]
    fn a_valid_form_submits_a_draft_with_the_chosen_values() {
        let mut form = form();
        form.meta = Some(meta());
        type_text(&mut form, "on-call");
        press(&mut form, KeyCode::Enter); // epic D-200 -> Assignee
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Assignee));
        type_text(&mut form, "riley");
        press(&mut form, KeyCode::Enter); // -> Title
        type_text(&mut form, "New card");
        press(&mut form, KeyCode::Enter); // -> Description
        type_text(&mut form, "Some **detail**");

        let CreateOutcome::Submit(draft) = form.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)) else {
            panic!("should submit: {:?} {:?}", form.errors, form.field_errors);
        };
        assert_eq!(draft.project_key, "DEMO");
        assert_eq!(draft.issue_type_id, "20");
        assert_eq!(draft.summary, "New card");
        assert_eq!(draft.epic.as_deref(), Some("D-200"));
        assert_eq!(draft.assignee.as_deref(), Some("acct-riley"));
        assert_eq!(draft.team.as_deref(), Some("main"));
        assert_eq!(draft.sprint, Some(7));
        assert_eq!(draft.description.unwrap()["content"][0]["content"][1]["marks"][0]["type"], "strong");
        assert_eq!(draft.acceptance, None, "empty fields aren't sent");
        assert!(form.submitting);
    }

    #[test]
    fn choosing_none_sends_none() {
        let mut form = form();
        form.meta = Some(meta());
        form.title = TextArea::new(vec!["t".into()]);
        form.team.chosen = PickOption::none("No team");
        form.sprint.chosen = PickOption::none("No sprint (backlog)");
        let CreateOutcome::Submit(draft) = form.try_submit() else { panic!("should submit") };
        assert_eq!((draft.sprint, draft.team, draft.assignee), (None, None, None));
    }

    #[test]
    fn jira_errors_land_on_fields_or_at_the_top() {
        let mut form = form();
        form.submitting = true;
        form.apply_error(&CreateError {
            general: vec!["Something general".into()],
            fields: vec![(CreateField::Assignee, "User cannot be assigned".into())],
        });
        assert!(!form.submitting);
        assert_eq!(form.errors, ["Something general"]);
        assert_eq!(form.field_errors[&CreateField::Assignee], "User cannot be assigned");
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Assignee), "jumps to the first bad field");
    }

    #[test]
    fn escape_asks_twice_once_something_was_typed() {
        let mut fresh = form();
        assert!(matches!(press(&mut fresh, KeyCode::Esc), CreateOutcome::Cancel), "empty form closes at once");
        let mut form = form();
        form.focus = CreateFocus::Field(CreateField::Title);
        type_text(&mut form, "draft");
        assert!(matches!(press(&mut form, KeyCode::Esc), CreateOutcome::None));
        assert!(matches!(press(&mut form, KeyCode::Esc), CreateOutcome::Cancel));
    }

    #[test]
    fn arrows_leave_multiline_fields_at_their_first_and_last_lines() {
        let mut form = form();
        form.focus = CreateFocus::Field(CreateField::Description);
        form.description = TextArea::new(vec!["one".into(), "two".into()]);
        press(&mut form, KeyCode::Down); // line 0 -> line 1
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Description));
        press(&mut form, KeyCode::Down);
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Acceptance));
        press(&mut form, KeyCode::Up);
        press(&mut form, KeyCode::Up); // cursor on line 1 -> line 0
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Description));
        press(&mut form, KeyCode::Up);
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Title));
    }

    #[test]
    fn tab_order_runs_top_to_bottom_then_the_buttons_left_to_right() {
        let mut form = form();
        let mut seen = vec![form.focus];
        for _ in 0..7 {
            press(&mut form, KeyCode::Tab);
            seen.push(form.focus);
        }
        assert_eq!(seen[1], CreateFocus::Field(CreateField::Assignee), "Assignee sits above Title");
        assert_eq!(seen[2], CreateFocus::Field(CreateField::Title));
        assert_eq!(&seen[5..7], [CreateFocus::Cancel, CreateFocus::Submit]);
    }

    #[test]
    fn typing_in_title_does_not_navigate() {
        let mut form = form();
        form.focus = CreateFocus::Field(CreateField::Title);
        type_text(&mut form, "hjkl jump");
        assert_eq!(CreateForm::text(&form.title), "hjkl jump");
        assert_eq!(form.focus, CreateFocus::Field(CreateField::Title));
    }
}
