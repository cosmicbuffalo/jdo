//! In-memory view model.
//!
//! A Jira mapping layer (see [`crate::jira::mapping`]) populates these types
//! from the REST API; the UI renders only from here. The `Board / Column /
//! Issue` shape deliberately mirrors tdo's `Board / Column / Task` so the
//! forked rendering code stays close to its source.

use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct Board {
    pub name: String,
    /// What slice of the board is shown, e.g. the active sprint's name.
    pub scope: String,
    pub columns: Vec<Column>,
    /// How column WIP limits count issues.
    pub constraint: Constraint,
}

/// Board-level WIP-limit mode (from the board's column configuration).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Constraint {
    #[default]
    None,
    IssueCount,
    /// Count issues but not sub-tasks.
    IssueCountExclSubs,
}

/// Where a column sits relative to its WIP limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Load {
    Ok,
    UnderMin,
    OverMax,
}

/// A board column. `status_ids` are the Jira status ids this column collects
/// (from the board configuration); an issue lands here when its status is one
/// of them.
#[derive(Debug, Clone, Default)]
pub struct Column {
    pub name: String,
    pub status_ids: Vec<String>,
    pub issues: Vec<Issue>,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl Column {
    /// The count WIP limits apply to, and how it compares to them.
    pub fn load(&self, constraint: Constraint) -> (usize, Load) {
        let count = match constraint {
            Constraint::None => return (self.issues.len(), Load::Ok),
            Constraint::IssueCount => self.issues.len(),
            Constraint::IssueCountExclSubs => self.issues.iter().filter(|i| !i.is_subtask).count(),
        };
        let load = match (self.min, self.max) {
            (_, Some(max)) if count > max as usize => Load::OverMax,
            (Some(min), _) if count < min as usize => Load::UnderMin,
            _ => Load::Ok,
        };
        (count, load)
    }
}

/// The fixed field set for the MVP (expandable later). Rich-text fields
/// (`description`, `acceptance_criteria`) are stored flattened-to-text for
/// display; authoring converts back to minimal ADF (see [`crate::jira::adf`]).
#[derive(Debug, Clone, Default)]
pub struct Issue {
    pub key: String,
    pub summary: String,
    pub issue_type: Option<String>,
    pub is_subtask: bool,
    pub status: String,
    pub status_id: String,
    pub assignee: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub description: String,
    pub acceptance_criteria: String,
    pub story_points: Option<f64>,
    pub sprint: Option<String>,
    pub epic: Option<EpicRef>,
    /// Browser URL for the issue (used by copy-to-clipboard).
    pub url: String,
    pub sync_state: SyncState,
}

/// The epic an issue belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicRef {
    pub key: String,
    pub name: String,
    /// Jira's epic color slot (`color_1`..`color_14`), when known.
    pub color: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Comment {
    pub author: String,
    /// Display timestamp, e.g. `2026-09-30 14:05`.
    pub created: String,
    pub body: String,
}

/// Optimistic-update lifecycle for an issue: local edits mark it `Pending`
/// until the worker acks (then `Synced`) or fails (`Error`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SyncState {
    #[default]
    Synced,
    Pending,
    Error,
}

/// A workflow transition available from an issue's current status.
#[derive(Debug, Clone)]
pub struct Transition {
    pub id: String,
    pub name: String,
    pub to_status_id: String,
    pub to_status_name: String,
    /// Fields on the transition screen (empty when the transition has none).
    pub fields: Vec<ScreenField>,
}

/// One field on a transition screen, as described by
/// `GET /issue/{key}/transitions?expand=transitions.fields`.
#[derive(Debug, Clone)]
pub struct ScreenField {
    pub id: String,
    pub name: String,
    pub required: bool,
    pub schema_type: String,
    pub schema_items: Option<String>,
    pub system: Option<String>,
    pub custom: Option<String>,
    pub allowed: Vec<AllowedValue>,
}

#[derive(Debug, Clone)]
pub struct AllowedValue {
    pub id: String,
    pub label: String,
}

impl Board {
    /// Column index that collects the given status id.
    pub fn column_for_status(&self, status_id: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.status_ids.iter().any(|s| s == status_id))
    }

    /// `(column, row)` of the issue with this key.
    pub fn find(&self, key: &str) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(ci, c)| {
            c.issues.iter().position(|i| i.key == key).map(|ri| (ci, ri))
        })
    }

    pub fn issue(&self, key: &str) -> Option<&Issue> {
        let (c, r) = self.find(key)?;
        Some(&self.columns[c].issues[r])
    }

    pub fn issue_mut(&mut self, key: &str) -> Option<&mut Issue> {
        let (c, r) = self.find(key)?;
        Some(&mut self.columns[c].issues[r])
    }

    /// Remove the issue with `key` and insert it into `column` at `row`
    /// (clamped). Returns the new `(column, row)`.
    pub fn relocate(&mut self, key: &str, column: usize, row: usize) -> Option<(usize, usize)> {
        let (c, r) = self.find(key)?;
        let issue = self.columns[c].issues.remove(r);
        let target = &mut self.columns[column].issues;
        let row = row.min(target.len());
        target.insert(row, issue);
        Some((column, row))
    }

    pub fn total_issues(&self) -> usize {
        self.columns.iter().map(|c| c.issues.len()).sum()
    }
}

/// Raw issue `fields` object, used to pre-fill transition forms.
pub type RawFields = Value;

#[cfg(test)]
mod tests {
    use super::*;

    fn column(issues: usize, subtasks: usize, min: Option<u32>, max: Option<u32>) -> Column {
        let mut all: Vec<Issue> = (0..issues).map(|_| Issue::default()).collect();
        all.extend((0..subtasks).map(|_| Issue { is_subtask: true, ..Default::default() }));
        Column { issues: all, min, max, ..Default::default() }
    }

    #[test]
    fn wip_limits_flag_over_max_and_under_min() {
        // In Review on the real board: max 5.
        assert_eq!(column(7, 0, None, Some(5)).load(Constraint::IssueCount), (7, Load::OverMax));
        assert_eq!(column(5, 0, None, Some(5)).load(Constraint::IssueCount), (5, Load::Ok));
        // Defined: min 2.
        assert_eq!(column(1, 0, Some(2), Some(5)).load(Constraint::IssueCount), (1, Load::UnderMin));
    }

    #[test]
    fn excluding_subtasks_and_no_constraint() {
        assert_eq!(column(5, 3, None, Some(5)).load(Constraint::IssueCountExclSubs), (5, Load::Ok));
        assert_eq!(column(5, 3, None, Some(5)).load(Constraint::IssueCount), (8, Load::OverMax));
        assert_eq!(column(9, 0, None, Some(5)).load(Constraint::None), (9, Load::Ok));
    }
}
