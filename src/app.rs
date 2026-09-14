//! Application state and input handling.
//!
//! P1 keeps this small: a board (populated from Jira), a selection cursor, and
//! refresh/quit. The forked-from-tdo `Mode` state machine and the
//! transition/comment/field-edit modals arrive in P2–P3.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::{Board, Column};

/// What the event loop should do after a key press.
pub enum Action {
    None,
    Quit,
    Refresh,
}

pub struct App {
    pub board: Board,
    /// Index into `board.columns`.
    pub selected_column: usize,
    /// Index into the selected column's `issues`.
    pub selected_row: usize,
    pub status: String,
}

impl App {
    pub fn new(board: Board) -> Self {
        Self {
            board,
            selected_column: 0,
            selected_row: 0,
            status: String::new(),
        }
    }

    /// Swap in a freshly fetched board, keeping the cursor in bounds.
    pub fn replace_board(&mut self, board: Board) {
        self.board = board;
        self.selected_column = self
            .selected_column
            .min(self.board.columns.len().saturating_sub(1));
        let rows = self.current_column().map(|c| c.issues.len()).unwrap_or(0);
        self.selected_row = self.selected_row.min(rows.saturating_sub(1));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }

        match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('r') | KeyCode::Char('R') => Action::Refresh,
            KeyCode::Char('h') | KeyCode::Left => {
                self.select_column(-1);
                Action::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.select_column(1);
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_row(-1);
                Action::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.select_row(1);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn select_column(&mut self, delta: isize) {
        if self.board.columns.is_empty() {
            return;
        }
        let count = self.board.columns.len() as isize;
        self.selected_column = (self.selected_column as isize + delta).rem_euclid(count) as usize;
        let rows = self.current_column().map(|c| c.issues.len()).unwrap_or(0);
        self.selected_row = self.selected_row.min(rows.saturating_sub(1));
    }

    fn select_row(&mut self, delta: isize) {
        let rows = self.current_column().map(|c| c.issues.len()).unwrap_or(0) as isize;
        if rows == 0 {
            return;
        }
        self.selected_row = (self.selected_row as isize + delta).rem_euclid(rows) as usize;
    }

    pub fn current_column(&self) -> Option<&Column> {
        self.board.columns.get(self.selected_column)
    }

    pub fn total_issues(&self) -> usize {
        self.board.columns.iter().map(|c| c.issues.len()).sum()
    }
}
