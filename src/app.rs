//! Application state and input handling.
//!
//! P0 keeps this intentionally small: a board, a selection cursor, and quit.
//! The forked-from-tdo `Mode` state machine, typed `Action` command enum, and
//! the transition/comment/field-edit modals arrive in P2–P3.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::Board;

pub struct App {
    pub board: Board,
    /// Index into `board.columns`.
    pub selected_column: usize,
    /// Index into the selected column's `issues`.
    pub selected_row: usize,
    pub should_quit: bool,
    pub status: String,
}

impl App {
    pub fn new(board: Board) -> Self {
        Self {
            board,
            selected_column: 0,
            selected_row: 0,
            should_quit: false,
            status: "jdo — P0 scaffold · mock data · q to quit".into(),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl-C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('h') | KeyCode::Left => self.select_column(-1),
            KeyCode::Char('l') | KeyCode::Right => self.select_column(1),
            KeyCode::Char('k') | KeyCode::Up => self.select_row(-1),
            KeyCode::Char('j') | KeyCode::Down => self.select_row(1),
            _ => {}
        }
    }

    fn select_column(&mut self, delta: isize) {
        if self.board.columns.is_empty() {
            return;
        }
        let count = self.board.columns.len() as isize;
        let next = (self.selected_column as isize + delta).rem_euclid(count);
        self.selected_column = next as usize;
        // Clamp the row into the new column.
        let rows = self.current_column().issues.len();
        self.selected_row = self.selected_row.min(rows.saturating_sub(1));
    }

    fn select_row(&mut self, delta: isize) {
        let rows = self.current_column().issues.len() as isize;
        if rows == 0 {
            return;
        }
        let next = (self.selected_row as isize + delta).rem_euclid(rows);
        self.selected_row = next as usize;
    }

    pub fn current_column(&self) -> &crate::model::Column {
        &self.board.columns[self.selected_column]
    }
}
