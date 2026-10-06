//! Swimlanes.
//!
//! Like Jira's "Group by", the board can be split into horizontal lanes — one
//! per epic or per assignee — each spanning every board column. Lanes are a
//! view over the board: a lane's cell for a column lists indices into that
//! column's `issues`, in board order, so moving or refreshing issues never has
//! to touch the grouping.

use crate::model::{Board, EpicRef, Issue};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    None,
    Epic,
    Assignee,
}

impl Grouping {
    pub fn next(self) -> Self {
        match self {
            Grouping::None => Grouping::Epic,
            Grouping::Epic => Grouping::Assignee,
            Grouping::Assignee => Grouping::None,
        }
    }

    /// Stable name, used to carry the grouping across a restart.
    pub fn as_str(self) -> &'static str {
        match self {
            Grouping::None => "none",
            Grouping::Epic => "epic",
            Grouping::Assignee => "assignee",
        }
    }

    pub fn parse(name: &str) -> Self {
        match name {
            "epic" => Grouping::Epic,
            "assignee" => Grouping::Assignee,
            _ => Grouping::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Grouping::None => "ungrouped",
            Grouping::Epic => "grouped by epic",
            Grouping::Assignee => "grouped by assignee",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneKind {
    /// The single lane of an ungrouped board.
    All,
    Epic(EpicRef),
    NoEpic,
    Assignee(String),
    Unassigned,
}

#[derive(Debug, Clone)]
pub struct Lane {
    /// Stable identity, used for folding and focus (e.g. `epic:DEMO-1180`).
    pub key: String,
    pub kind: LaneKind,
    /// Per board column, indices into that column's `issues`.
    pub cells: Vec<Vec<usize>>,
}

impl Lane {
    pub fn count(&self) -> usize {
        self.cells.iter().map(Vec::len).sum()
    }
}

/// The lane an issue belongs to under `grouping`.
pub fn lane_key(issue: &Issue, grouping: Grouping) -> String {
    match grouping {
        Grouping::None => "all".into(),
        Grouping::Epic => match &issue.epic {
            Some(epic) => format!("epic:{}", epic.key),
            None => "epic:-".into(),
        },
        Grouping::Assignee => match &issue.assignee {
            Some(name) => format!("user:{name}"),
            None => "user:-".into(),
        },
    }
}

fn lane_kind(issue: &Issue, grouping: Grouping) -> LaneKind {
    match grouping {
        Grouping::None => LaneKind::All,
        Grouping::Epic => issue.epic.clone().map(LaneKind::Epic).unwrap_or(LaneKind::NoEpic),
        Grouping::Assignee => issue.assignee.clone().map(LaneKind::Assignee).unwrap_or(LaneKind::Unassigned),
    }
}

/// Build lanes, sorted by name with the "no epic" / "unassigned" lane last.
/// An ungrouped board is one lane holding everything.
pub fn build(board: &Board, grouping: Grouping) -> Vec<Lane> {
    let columns = board.columns.len();
    let mut lanes: Vec<Lane> = Vec::new();
    if grouping == Grouping::None {
        lanes.push(Lane { key: "all".into(), kind: LaneKind::All, cells: vec![Vec::new(); columns] });
    }
    for (ci, column) in board.columns.iter().enumerate() {
        for (ri, issue) in column.issues.iter().enumerate() {
            let key = lane_key(issue, grouping);
            let index = match lanes.iter().position(|l| l.key == key) {
                Some(i) => i,
                None => {
                    lanes.push(Lane { key, kind: lane_kind(issue, grouping), cells: vec![Vec::new(); columns] });
                    lanes.len() - 1
                }
            };
            lanes[index].cells[ci].push(ri);
        }
    }
    lanes.sort_by_key(|lane| match &lane.kind {
        LaneKind::All => (0, String::new()),
        LaneKind::Epic(epic) => (0, epic.name.to_lowercase()),
        LaneKind::Assignee(name) => (0, name.to_lowercase()),
        LaneKind::NoEpic | LaneKind::Unassigned => (1, String::new()),
    });
    lanes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Column;

    fn issue(key: &str, epic: Option<&str>, assignee: Option<&str>) -> Issue {
        Issue {
            key: key.into(),
            epic: epic.map(|e| EpicRef { key: format!("E-{e}"), name: e.into(), color: None }),
            assignee: assignee.map(str::to_string),
            ..Default::default()
        }
    }

    fn board() -> Board {
        Board {
            columns: vec![
                Column { issues: vec![issue("A-1", Some("Zeta"), Some("bo")), issue("A-2", None, None)], ..Default::default() },
                Column { issues: vec![issue("A-3", Some("Alpha"), Some("Al")), issue("A-4", Some("Zeta"), Some("bo"))], ..Default::default() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn epic_lanes_sort_by_name_with_no_epic_last() {
        let lanes = build(&board(), Grouping::Epic);
        let keys: Vec<&str> = lanes.iter().map(|l| l.key.as_str()).collect();
        assert_eq!(keys, ["epic:E-Alpha", "epic:E-Zeta", "epic:-"]);
        // Zeta: A-1 in column 0, A-4 (row 1) in column 1.
        assert_eq!(lanes[1].cells, vec![vec![0], vec![1]]);
        assert_eq!(lanes[1].count(), 2);
    }

    #[test]
    fn assignee_lanes_sort_case_insensitively_with_unassigned_last() {
        let keys: Vec<String> = build(&board(), Grouping::Assignee).into_iter().map(|l| l.key).collect();
        assert_eq!(keys, ["user:Al", "user:bo", "user:-"]);
    }

    #[test]
    fn ungrouped_is_one_lane_with_every_issue() {
        let lanes = build(&board(), Grouping::None);
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].cells, vec![vec![0, 1], vec![0, 1]]);
    }

    #[test]
    fn grouping_cycles_through_all_modes() {
        assert_eq!(Grouping::None.next().next().next(), Grouping::None);
    }
}
