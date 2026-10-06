//! Transition forms.
//!
//! When a workflow transition has a screen, Jira's web UI pops a dialog with
//! the screen's fields. [`TransitionForm`] is jdo's equivalent: it turns the
//! screen description from `GET /transitions?expand=transitions.fields` into
//! editable fields pre-filled with the issue's current values, validates
//! them, builds the `POST /transitions` body, and maps Jira's validator errors
//! back onto the offending fields so the form can stay open for a retry.
//!
//! Only fields the user changed are sent; untouched pre-filled values are left
//! alone so a transition never rewrites data it didn't need to.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Map, Value, json};
use tui_textarea::TextArea;

use crate::jira::adf;
use crate::jira::client::JiraError;
use crate::model::{ScreenField, Transition};

/// One selectable option and the JSON Jira expects when it is chosen.
#[derive(Debug, Clone)]
pub struct Choice {
    pub label: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub enum FieldKind {
    /// Free text; `rich` fields (description, textarea custom fields) are sent as ADF.
    Text { rich: bool },
    Number,
    /// `YYYY-MM-DD`.
    Date,
    /// Comma-separated strings (e.g. Labels).
    Labels,
    /// Parent issue key.
    Parent,
    Select { options: Vec<Choice> },
    MultiSelect { options: Vec<Choice> },
    /// Optional comment added alongside the transition.
    Comment,
    /// A field type jdo can't edit yet (team, attachment, cascading select, ...).
    Unsupported,
}

#[derive(Debug, Clone)]
pub enum FieldValue {
    Text(String),
    Select(Option<usize>),
    Multi(Vec<usize>),
    None,
}

#[derive(Debug, Clone)]
pub struct FormField {
    pub id: String,
    pub name: String,
    pub required: bool,
    pub kind: FieldKind,
    pub value: FieldValue,
    /// Changed by the user; only dirty fields are sent.
    pub dirty: bool,
    /// The issue already has a value (matters for unsupported required fields).
    pub has_current: bool,
    pub error: Option<String>,
}

/// Single- or multi-select overlay for choosing a field's option(s).
pub struct Picker {
    pub field: usize,
    pub cursor: usize,
    /// Checked state per option (multi-select only).
    pub checked: Vec<bool>,
}

pub struct TransitionForm {
    pub key: String,
    pub transition_id: String,
    pub transition_name: String,
    pub to_status: String,
    pub fields: Vec<FormField>,
    /// `0..fields.len()` selects a field; `fields.len()` is Submit and
    /// `fields.len() + 1` is Cancel.
    pub cursor: usize,
    /// Form-level errors (validator messages, errors on off-screen fields).
    pub errors: Vec<String>,
    pub submitting: bool,
    pub editor: Option<TextArea<'static>>,
    pub picker: Option<Picker>,
}

pub enum FormOutcome {
    None,
    Submit,
    Cancel,
}

/// Whether choosing `t` should open a form. Screens made only of fields jdo
/// can't edit (e.g. just an attachment) are skipped unless one is required.
pub fn needs_form(t: &Transition) -> bool {
    t.fields
        .iter()
        .any(|f| f.required || !matches!(classify(f, &Value::Null, None).0, FieldKind::Unsupported))
}

impl TransitionForm {
    /// `current` is the issue's raw `fields` object for the screen's field
    /// ids; `me` is `(accountId, displayName)` for "assign to me".
    pub fn new(key: &str, t: &Transition, current: &Value, me: Option<&(String, String)>) -> Self {
        let mut fields: Vec<FormField> = t
            .fields
            .iter()
            .map(|f| {
                let cur = current.get(&f.id).unwrap_or(&Value::Null);
                let (kind, value) = classify(f, cur, me);
                FormField {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    required: f.required,
                    kind,
                    value,
                    dirty: false,
                    has_current: !cur.is_null(),
                    error: None,
                }
            })
            .collect();

        // Jira's transition dialog always offers a comment box.
        if !fields.iter().any(|f| matches!(f.kind, FieldKind::Comment)) {
            fields.push(FormField {
                id: "comment".into(),
                name: "Comment".into(),
                required: false,
                kind: FieldKind::Comment,
                value: FieldValue::Text(String::new()),
                dirty: false,
                has_current: false,
                error: None,
            });
        }

        Self {
            key: key.to_string(),
            transition_id: t.id.clone(),
            transition_name: t.name.clone(),
            to_status: t.to_status_name.clone(),
            fields,
            cursor: 0,
            errors: Vec::new(),
            submitting: false,
            editor: None,
            picker: None,
        }
    }

    pub fn submit_index(&self) -> usize {
        self.fields.len()
    }

    pub fn cancel_index(&self) -> usize {
        self.fields.len() + 1
    }

    /// Check required fields and parseable values, annotating offending fields.
    pub fn validate(&mut self) -> bool {
        self.errors.clear();
        for field in &mut self.fields {
            field.error = None;
            if matches!(field.kind, FieldKind::Unsupported) {
                if field.required && !field.has_current {
                    field.error = Some("required — set it in Jira (not editable in jdo yet)".into());
                }
                continue;
            }
            if field.required && field.is_empty() && !(field.has_current && !field.dirty) {
                field.error = Some("required".into());
                continue;
            }
            if let (FieldKind::Number, FieldValue::Text(text)) = (&field.kind, &field.value) {
                if !text.trim().is_empty() && text.trim().parse::<f64>().is_err() {
                    field.error = Some("must be a number".into());
                }
            }
        }
        self.fields.iter().all(|f| f.error.is_none())
    }

    /// The `POST /issue/{key}/transitions` body.
    pub fn payload(&self) -> Value {
        let mut fields = Map::new();
        let mut update = Map::new();
        for field in self.fields.iter().filter(|f| f.dirty) {
            match (&field.kind, &field.value) {
                (FieldKind::Comment, FieldValue::Text(text)) => {
                    if !text.trim().is_empty() {
                        update.insert(
                            "comment".into(),
                            json!([{ "add": { "body": adf::text_to_adf(text) } }]),
                        );
                    }
                }
                (FieldKind::Unsupported, _) => {}
                _ => {
                    fields.insert(field.id.clone(), field.json_value());
                }
            }
        }
        let mut body = Map::new();
        body.insert("transition".into(), json!({ "id": self.transition_id }));
        if !fields.is_empty() {
            body.insert("fields".into(), Value::Object(fields));
        }
        if !update.is_empty() {
            body.insert("update".into(), Value::Object(update));
        }
        Value::Object(body)
    }

    /// Attach a rejected submit's errors to the form so the user can fix and retry.
    pub fn apply_error(&mut self, err: &JiraError) {
        self.submitting = false;
        self.errors = err.messages.clone();
        for (field_id, message) in &err.field_errors {
            match self.fields.iter_mut().find(|f| &f.id == field_id) {
                Some(field) => field.error = Some(message.clone()),
                None => self.errors.push(format!("{field_id}: {message}")),
            }
        }
        if self.errors.is_empty() && err.field_errors.is_empty() {
            self.errors.push("Jira rejected the transition".into());
        }
        // Jump to the first field with an error.
        if let Some(i) = self.fields.iter().position(|f| f.error.is_some()) {
            self.cursor = i;
        }
    }

    /// Handle a key while the form is focused (and not submitting).
    pub fn handle_key(&mut self, key: KeyEvent) -> FormOutcome {
        if self.submitting {
            return FormOutcome::None;
        }
        if self.editor.is_some() {
            self.handle_editor_key(key);
            return FormOutcome::None;
        }
        if self.picker.is_some() {
            self.handle_picker_key(key);
            return FormOutcome::None;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('s') if ctrl => return self.try_submit(),
            KeyCode::Esc | KeyCode::Char('q') => return FormOutcome::Cancel,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.cursor = (self.cursor + 1).min(self.cancel_index());
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Left | KeyCode::Char('h') if self.cursor == self.cancel_index() => {
                self.cursor = self.submit_index();
            }
            KeyCode::Right | KeyCode::Char('l') if self.cursor == self.submit_index() => {
                self.cursor = self.cancel_index();
            }
            KeyCode::Enter | KeyCode::Char('e') | KeyCode::Char(' ') => {
                if self.cursor == self.submit_index() {
                    return self.try_submit();
                }
                if self.cursor == self.cancel_index() {
                    return FormOutcome::Cancel;
                }
                self.activate_field();
            }
            _ => {}
        }
        FormOutcome::None
    }

    fn try_submit(&mut self) -> FormOutcome {
        if self.validate() {
            self.submitting = true;
            FormOutcome::Submit
        } else {
            if let Some(i) = self.fields.iter().position(|f| f.error.is_some()) {
                self.cursor = i;
            }
            FormOutcome::None
        }
    }

    fn activate_field(&mut self) {
        let index = self.cursor;
        let field = &self.fields[index];
        match &field.kind {
            FieldKind::Select { options } | FieldKind::MultiSelect { options } => {
                let mut checked = vec![false; options.len()];
                let cursor = match &field.value {
                    FieldValue::Select(Some(i)) => *i,
                    FieldValue::Multi(selected) => {
                        for &i in selected {
                            checked[i] = true;
                        }
                        selected.first().copied().unwrap_or(0)
                    }
                    _ => 0,
                };
                if !options.is_empty() {
                    self.picker = Some(Picker { field: index, cursor, checked });
                }
            }
            FieldKind::Unsupported => {
                self.fields[index].error =
                    Some("this field type isn't editable in jdo yet — use Jira".into());
            }
            _ => {
                let text = match &field.value {
                    FieldValue::Text(text) => text.clone(),
                    _ => String::new(),
                };
                let mut editor = TextArea::new(text.split('\n').map(str::to_owned).collect());
                editor.move_cursor(tui_textarea::CursorMove::Bottom);
                editor.move_cursor(tui_textarea::CursorMove::End);
                self.editor = Some(editor);
            }
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        let multiline = self.fields[self.cursor].is_multiline();
        let editor = self.editor.as_mut().expect("editor open");
        match key.code {
            KeyCode::Esc => self.editor = None,
            KeyCode::Enter if multiline && key.modifiers.contains(KeyModifiers::ALT) => {
                editor.insert_newline();
            }
            KeyCode::Enter => {
                let text = editor.lines().join("\n");
                let field = &mut self.fields[self.cursor];
                let changed = !matches!(&field.value, FieldValue::Text(old) if *old == text);
                if changed {
                    field.value = FieldValue::Text(text);
                    field.dirty = true;
                    field.error = None;
                }
                self.editor = None;
            }
            _ => {
                editor.input(key);
            }
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let picker = self.picker.as_mut().expect("picker open");
        let field = &mut self.fields[picker.field];
        let (count, multi) = match &field.kind {
            FieldKind::Select { options } => (options.len(), false),
            FieldKind::MultiSelect { options } => (options.len(), true),
            _ => (0, false),
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.picker = None,
            KeyCode::Down | KeyCode::Char('j') => picker.cursor = (picker.cursor + 1).min(count.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => picker.cursor = picker.cursor.saturating_sub(1),
            KeyCode::Char(' ') if multi => picker.checked[picker.cursor] = !picker.checked[picker.cursor],
            KeyCode::Enter => {
                let value = if multi {
                    FieldValue::Multi(
                        picker.checked.iter().enumerate().filter(|(_, c)| **c).map(|(i, _)| i).collect(),
                    )
                } else {
                    FieldValue::Select(Some(picker.cursor))
                };
                field.value = value;
                field.dirty = true;
                field.error = None;
                self.picker = None;
            }
            _ => {}
        }
    }
}

impl FormField {
    pub fn is_multiline(&self) -> bool {
        matches!(self.kind, FieldKind::Text { rich: true } | FieldKind::Comment)
    }

    pub fn is_empty(&self) -> bool {
        match &self.value {
            FieldValue::Text(text) => text.trim().is_empty(),
            FieldValue::Select(choice) => choice.is_none(),
            FieldValue::Multi(selected) => selected.is_empty(),
            FieldValue::None => true,
        }
    }

    /// Human-readable current value for the form.
    pub fn display_value(&self) -> String {
        match (&self.kind, &self.value) {
            (FieldKind::Unsupported, _) => {
                if self.has_current { "(set — edit in Jira)" } else { "(not editable in jdo)" }.into()
            }
            (_, FieldValue::Text(text)) => text.clone(),
            (FieldKind::Select { options }, FieldValue::Select(Some(i))) => options[*i].label.clone(),
            (FieldKind::MultiSelect { options }, FieldValue::Multi(selected)) => selected
                .iter()
                .map(|&i| options[i].label.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        }
    }

    /// The JSON value Jira expects for this field in `fields`.
    fn json_value(&self) -> Value {
        let text = || match &self.value {
            FieldValue::Text(text) => text.trim().to_string(),
            _ => String::new(),
        };
        match (&self.kind, &self.value) {
            (FieldKind::Text { rich: true }, _) => {
                let t = text();
                if t.is_empty() { Value::Null } else { adf::text_to_adf(&t) }
            }
            (FieldKind::Text { rich: false } | FieldKind::Date, _) => {
                let t = text();
                if t.is_empty() { Value::Null } else { Value::String(t) }
            }
            (FieldKind::Number, _) => text().parse::<f64>().map(|n| json!(n)).unwrap_or(Value::Null),
            (FieldKind::Labels, _) => Value::Array(
                text()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
            (FieldKind::Parent, _) => {
                let t = text();
                if t.is_empty() { Value::Null } else { json!({ "key": t }) }
            }
            (FieldKind::Select { options }, FieldValue::Select(choice)) => {
                choice.map(|i| options[i].payload.clone()).unwrap_or(Value::Null)
            }
            (FieldKind::MultiSelect { options }, FieldValue::Multi(selected)) => {
                Value::Array(selected.iter().map(|&i| options[i].payload.clone()).collect())
            }
            _ => Value::Null,
        }
    }
}

/// Decide how a screen field is edited, and pre-fill it from `cur`.
fn classify(f: &ScreenField, cur: &Value, me: Option<&(String, String)>) -> (FieldKind, FieldValue) {
    let str_of = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);

    if f.id == "comment" || f.schema_type == "comment" {
        return (FieldKind::Comment, FieldValue::Text(String::new()));
    }

    if f.schema_type == "user" {
        let mut options = Vec::new();
        let mut value = None;
        let current_id = str_of(cur, "accountId");
        if let Some(id) = &current_id {
            let name = str_of(cur, "displayName").unwrap_or_else(|| id.clone());
            options.push(Choice { label: format!("{name} (current)"), payload: json!({ "accountId": id }) });
            value = Some(0);
        }
        if let Some((id, name)) = me {
            if current_id.as_ref() != Some(id) {
                options.push(Choice { label: format!("Me ({name})"), payload: json!({ "accountId": id }) });
            }
        }
        if !f.required {
            options.push(Choice { label: "Unassigned".into(), payload: Value::Null });
        }
        return (FieldKind::Select { options }, FieldValue::Select(value));
    }

    if !f.allowed.is_empty() {
        let options: Vec<Choice> = f
            .allowed
            .iter()
            .map(|a| Choice { label: a.label.clone(), payload: json!({ "id": a.id }) })
            .collect();
        let index_of = |v: &Value| {
            let id = v.get("id").and_then(Value::as_str)?;
            f.allowed.iter().position(|a| a.id == id)
        };
        return if f.schema_type == "array" {
            let selected = cur
                .as_array()
                .map(|items| items.iter().filter_map(index_of).collect())
                .unwrap_or_default();
            (FieldKind::MultiSelect { options }, FieldValue::Multi(selected))
        } else {
            (FieldKind::Select { options }, FieldValue::Select(index_of(cur)))
        };
    }

    match f.schema_type.as_str() {
        "string" => {
            let rich = matches!(f.system.as_deref(), Some("description" | "environment"))
                || f.custom.as_deref().is_some_and(|c| c.ends_with(":textarea"));
            let text = if rich {
                adf::to_text(Some(cur))
            } else {
                cur.as_str().unwrap_or_default().to_string()
            };
            (FieldKind::Text { rich }, FieldValue::Text(text))
        }
        "number" => {
            let text = cur.as_f64().map(format_number).unwrap_or_default();
            (FieldKind::Number, FieldValue::Text(text))
        }
        "date" => (FieldKind::Date, FieldValue::Text(cur.as_str().unwrap_or_default().to_string())),
        "array" if f.schema_items.as_deref() == Some("string") => {
            let text = cur
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            (FieldKind::Labels, FieldValue::Text(text))
        }
        "issuelink" if f.system.as_deref() == Some("parent") => {
            (FieldKind::Parent, FieldValue::Text(str_of(cur, "key").unwrap_or_default()))
        }
        _ => (FieldKind::Unsupported, FieldValue::None),
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AllowedValue;

    fn field(id: &str, schema_type: &str) -> ScreenField {
        ScreenField {
            id: id.into(),
            name: id.into(),
            required: false,
            schema_type: schema_type.into(),
            schema_items: None,
            system: None,
            custom: None,
            allowed: Vec::new(),
        }
    }

    /// Mirrors the "Define" screen on the real board.
    fn define_transition() -> Transition {
        let mut description = field("description", "string");
        description.system = Some("description".into());
        let mut ac = field("customfield_10035", "string");
        ac.custom = Some("com.atlassian.jira.plugin.system.customfieldtypes:textarea".into());
        let mut parent = field("parent", "issuelink");
        parent.system = Some("parent".into());
        Transition {
            id: "81".into(),
            name: "Define".into(),
            to_status_id: "13131".into(),
            to_status_name: "Defined".into(),
            fields: vec![parent, description, field("customfield_10016", "number"), ac],
        }
    }

    #[test]
    fn prefills_current_values_and_adds_comment_box() {
        let current = json!({
            "customfield_10016": 2.0,
            "description": adf::text_to_adf("Existing description"),
            "parent": {"key": "DEMO-1185"}
        });
        let form = TransitionForm::new("DEMO-1", &define_transition(), &current, None);
        let shown: Vec<String> = form.fields.iter().map(FormField::display_value).collect();
        assert_eq!(shown, ["DEMO-1185", "Existing description", "2", "", ""]);
        assert!(matches!(form.fields[1].kind, FieldKind::Text { rich: true }));
        assert!(matches!(form.fields[3].kind, FieldKind::Text { rich: true }));
        assert!(matches!(form.fields.last().unwrap().kind, FieldKind::Comment));
    }

    #[test]
    fn untouched_form_sends_only_the_transition() {
        let current = json!({"customfield_10016": 2.0});
        let form = TransitionForm::new("K-1", &define_transition(), &current, None);
        assert_eq!(form.payload(), json!({"transition": {"id": "81"}}));
    }

    #[test]
    fn payload_includes_only_dirty_fields_with_jira_shapes() {
        let mut form = TransitionForm::new("K-1", &define_transition(), &json!({}), None);
        form.fields[2].value = FieldValue::Text("3".into());
        form.fields[2].dirty = true;
        form.fields[3].value = FieldValue::Text("Given X\nThen Y".into());
        form.fields[3].dirty = true;
        let comment = form.fields.len() - 1;
        form.fields[comment].value = FieldValue::Text("Starting this".into());
        form.fields[comment].dirty = true;

        let body = form.payload();
        assert_eq!(body["fields"]["customfield_10016"], json!(3.0));
        assert_eq!(body["fields"]["customfield_10035"]["type"], "doc");
        assert!(body["fields"].get("description").is_none());
        assert_eq!(
            body["update"]["comment"][0]["add"]["body"]["content"][0]["content"][0]["text"],
            "Starting this"
        );
    }

    #[test]
    fn validation_flags_required_and_bad_numbers() {
        let mut t = define_transition();
        t.fields[2].required = true;
        let mut form = TransitionForm::new("K-1", &t, &json!({}), None);
        assert!(!form.validate());
        assert_eq!(form.fields[2].error.as_deref(), Some("required"));

        form.fields[2].value = FieldValue::Text("lots".into());
        form.fields[2].dirty = true;
        assert!(!form.validate());
        assert_eq!(form.fields[2].error.as_deref(), Some("must be a number"));

        form.fields[2].value = FieldValue::Text("5".into());
        assert!(form.validate());
    }

    #[test]
    fn required_field_already_set_on_issue_passes_validation() {
        let mut t = define_transition();
        t.fields[2].required = true;
        let mut form = TransitionForm::new("K-1", &t, &json!({"customfield_10016": 1.0}), None);
        assert!(form.validate());
    }

    #[test]
    fn select_fields_send_option_ids_and_users_offer_assign_to_me() {
        let mut resolution = field("resolution", "resolution");
        resolution.allowed = vec![
            AllowedValue { id: "1".into(), label: "Done".into() },
            AllowedValue { id: "2".into(), label: "Won't Do".into() },
        ];
        let t = Transition {
            id: "5".into(),
            name: "Resolve".into(),
            to_status_id: "6".into(),
            to_status_name: "Closed".into(),
            fields: vec![resolution, field("assignee", "user")],
        };
        let me = ("acct-me".to_string(), "Nick".to_string());
        let mut form = TransitionForm::new("K-1", &t, &json!({}), Some(&me));

        let FieldKind::Select { options } = &form.fields[1].kind else { panic!("user is a select") };
        assert_eq!(options[0].label, "Me (Nick)");
        assert_eq!(options[1].label, "Unassigned");

        form.fields[0].value = FieldValue::Select(Some(1));
        form.fields[0].dirty = true;
        form.fields[1].value = FieldValue::Select(Some(0));
        form.fields[1].dirty = true;
        let body = form.payload();
        assert_eq!(body["fields"]["resolution"], json!({"id": "2"}));
        assert_eq!(body["fields"]["assignee"], json!({"accountId": "acct-me"}));
    }

    #[test]
    fn jira_errors_attach_to_fields_or_the_form() {
        let mut form = TransitionForm::new("K-1", &define_transition(), &json!({}), None);
        form.submitting = true;
        let mut err = JiraError::default();
        err.messages.push("Story Points must be set".into());
        err.field_errors.insert("customfield_10016".into(), "required".into());
        err.field_errors.insert("fixVersions".into(), "pick one".into());
        form.apply_error(&err);
        assert!(!form.submitting);
        assert_eq!(form.fields[2].error.as_deref(), Some("required"));
        assert_eq!(form.cursor, 2);
        assert_eq!(form.errors, ["Story Points must be set", "fixVersions: pick one"]);
    }

    #[test]
    fn screens_of_only_unsupported_optional_fields_skip_the_form() {
        let mut attachment = field("attachment", "array");
        attachment.schema_items = Some("attachment".into());
        let t = Transition {
            id: "171".into(),
            name: "Blocked".into(),
            to_status_id: "11431".into(),
            to_status_name: "Blocked".into(),
            fields: vec![attachment],
        };
        assert!(!needs_form(&t));
        assert!(needs_form(&define_transition()));
    }
}
