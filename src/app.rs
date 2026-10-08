//! Application state and input handling.
//!
//! Follows tdo's modal design: a [`Mode`] state machine where each floating
//! view (help, details, move, transition form) owns its key handling, and key
//! handlers return an [`Action`] that the event loop turns into worker
//! commands. Network results come back through the `on_*` methods.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use serde_json::Value;

use crate::colors::UserColors;
use crate::create::{CreateDraft, CreateError, CreateField, CreateForm, CreateMeta, CreateOutcome};
use crate::form::{FormOutcome, TransitionForm, needs_form};
use crate::jira::adf;
use crate::jira::markdown::{self, Markdown};
use crate::jira::client::JiraError;
use crate::lanes::{self, Grouping, Lane};
use crate::model::{Board, Column, Comment, Issue, RawFields, RichField, SyncState, Transition};
use tui_textarea::TextArea;

/// What the event loop should do after a key press.
pub enum Action {
    None,
    Quit,
    Refresh,
    FetchTransitions(String),
    SubmitTransition { key: String, body: Value },
    /// Load what the details view needs: comments and the rich-text fields.
    FetchDetails(String),
    SaveRich { key: String, field: RichField, value: Value },
    /// Hand the open field editor's text to `$EDITOR`.
    EditExternally,
    /// Several of the above, in order.
    Many(Vec<Action>),
    FetchCreateMeta(String),
    FetchTeamMembers(String),
    CreateIssue(CreateDraft),
}

pub enum Mode {
    Board,
    Help { return_to: Box<Mode> },
    Details { key: String, scroll: u16 },
    Moving(MoveState),
    TransitionForm(Box<TransitionForm>, PendingMove),
    EditRich(Box<RichEditor>),
    Create(Box<CreateForm>),
}

/// Editing a description or acceptance criteria as Markdown.
pub struct RichEditor {
    pub key: String,
    pub field: RichField,
    pub editor: TextArea<'static>,
    /// The Markdown as opened, to skip saving when nothing changed.
    pub original: String,
    /// Placeholder originals and what saving would drop.
    pub markdown: Markdown,
    pub saving: bool,
    pub error: Option<String>,
    /// Details scroll to return to.
    pub details_scroll: u16,
}

/// A card's rich-text fields as ADF, fetched when its details open.
pub enum RichState {
    Loading,
    Loaded { description: Value, acceptance: Value },
    Failed(String),
}

impl RichState {
    pub fn get(&self, field: RichField) -> Option<&Value> {
        match (self, field) {
            (RichState::Loaded { description, .. }, RichField::Description) => Some(description),
            (RichState::Loaded { acceptance, .. }, RichField::AcceptanceCriteria) => Some(acceptance),
            _ => None,
        }
    }
}

/// Where a card came from, so a cancelled or rejected move can put it back.
#[derive(Debug, Clone)]
pub struct PendingMove {
    pub key: String,
    pub origin_column: usize,
    pub origin_row: usize,
    pub old_status: String,
    pub old_status_id: String,
}

/// A column the card may move to, with the transitions that land there.
#[derive(Debug, Clone)]
pub struct MoveTarget {
    pub column: usize,
    pub transitions: Vec<Transition>,
}

pub struct MoveState {
    pub pending: PendingMove,
    /// `None` while the workflow is loading.
    pub targets: Option<Vec<MoveTarget>>,
    /// Current values of the screens' fields, for pre-filling forms.
    pub current: RawFields,
    /// Which of the current column's transitions Enter will use.
    pub choice: usize,
}

/// Something clickable, recorded by the renderer each frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    /// A card at `(column, row)` of the board.
    Card { column: usize, row: usize },
    LaneHeader(String),
    /// The `[×]` on the details modal.
    CloseDetails,
    /// A footer hint: clicking presses its key.
    Key(KeyEvent),
    /// Something in the create dialog.
    Create(CreateHit),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateHit {
    Focus(CreateField),
    /// Row `n` of a picker's open dropdown.
    Pick(CreateField, usize),
    Submit,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub area: Rect,
    pub target: HitTarget,
}

/// What the cursor rests on when the board is grouped into lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneFocus {
    /// The card at `(selected_column, selected_row)`.
    Card,
    /// A lane's header row (lane key).
    Header(String),
    /// An empty cell of a lane in `selected_column` (lane key).
    Empty(String),
}

/// Cursor position within the current lane, for grouped navigation.
enum LanePos {
    Header,
    Card(usize),
    Empty,
}

pub enum CommentsState {
    Loading,
    Loaded(Vec<Comment>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Error,
}

/// Braille spinner frames and how long each is shown.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

pub struct App {
    pub board: Board,
    pub board_id: u64,
    pub board_loaded: bool,
    /// Why the initial board load failed (shown in place of the board).
    pub load_error: Option<String>,
    /// A manual refresh is in flight (status line shows a spinner).
    pub refreshing: bool,
    started: Instant,
    /// Index into `board.columns`.
    pub selected_column: usize,
    /// Index into the selected column's `issues`.
    pub selected_row: usize,
    /// First visible card per column (maintained by the renderer).
    pub column_scroll: Vec<usize>,
    /// First visible column when the board is wider than the terminal.
    pub lane_offset: usize,
    pub grouping: Grouping,
    /// Folded lane keys (kept across refreshes and regrouping).
    pub folded: HashSet<String>,
    pub focus: LaneFocus,
    /// With a header focused: whether the cursor is in the gutter (where
    /// `j`/`k` hop headers) rather than passing through it in a column.
    pub in_gutter: bool,
    /// First visible line of the grouped board (maintained by the renderer).
    pub board_scroll: usize,
    /// Clickable regions from the last frame, in draw order (last is topmost).
    pub hits: Vec<Hit>,
    /// Last known mouse position, for hover highlights.
    pub mouse: Option<(u16, u16)>,
    /// Distinct pill colors for everyone seen this session.
    pub user_colors: UserColors,
    /// Card to select once the board arrives (after a restart).
    pending_select: Option<String>,
    /// Non-card cursor (header/gutter/empty cell) to restore after a restart.
    pending_focus: Option<(LaneFocus, bool, usize)>,
    pub mode: Mode,
    pub status: Option<(String, StatusLevel)>,
    pub comments: HashMap<String, CommentsState>,
    pub rich: HashMap<String, RichState>,
    /// `(accountId, displayName)` of the signed-in user.
    pub me: Option<(String, String)>,
    /// Moves submitted without a form, awaiting Jira's verdict.
    in_flight: HashMap<String, PendingMove>,
    /// A refreshed board that arrived mid-move; applied once the move settles.
    deferred_board: Option<Board>,
}

impl App {
    pub fn new(board: Board, board_id: u64) -> Self {
        Self {
            board,
            board_id,
            board_loaded: false,
            load_error: None,
            refreshing: false,
            started: Instant::now(),
            selected_column: 0,
            selected_row: 0,
            column_scroll: Vec::new(),
            lane_offset: 0,
            grouping: Grouping::None,
            folded: HashSet::new(),
            focus: LaneFocus::Card,
            in_gutter: false,
            board_scroll: 0,
            hits: Vec::new(),
            mouse: None,
            user_colors: UserColors::default(),
            pending_select: None,
            pending_focus: None,
            mode: Mode::Board,
            status: None,
            comments: HashMap::new(),
            rich: HashMap::new(),
            me: None,
            in_flight: HashMap::new(),
            deferred_board: None,
        }
    }

    /// Waiting on the initial board load (drives the board-area spinner).
    pub fn is_loading(&self) -> bool {
        !self.board_loaded && self.load_error.is_none()
    }

    /// Any spinner on screen, so the loop redraws fast enough to animate it.
    pub fn is_animating(&self) -> bool {
        self.is_loading() || self.refreshing
    }

    pub fn spinner(&self) -> &'static str {
        let tick = self.started.elapsed().as_millis() / SPINNER_INTERVAL.as_millis();
        SPINNER[tick as usize % SPINNER.len()]
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), StatusLevel::Info));
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), StatusLevel::Error));
    }

    // ----- selection -------------------------------------------------------

    pub fn current_column(&self) -> Option<&Column> {
        self.board.columns.get(self.selected_column)
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        if self.focus != LaneFocus::Card {
            return None;
        }
        self.current_column()?.issues.get(self.selected_row)
    }

    fn select_key(&mut self, key: &str) -> bool {
        match self.board.find(key) {
            Some((c, r)) => {
                self.selected_column = c;
                self.selected_row = r;
                self.focus = LaneFocus::Card;
                self.ensure_focus();
                true
            }
            None => false,
        }
    }

    // ----- lanes -----------------------------------------------------------

    pub fn lanes(&self) -> Vec<Lane> {
        lanes::build(&self.board, self.grouping)
    }

    pub fn is_grouped(&self) -> bool {
        self.grouping != Grouping::None
    }

    pub fn is_folded(&self, lane: &Lane) -> bool {
        self.is_grouped() && self.folded.contains(&lane.key)
    }

    /// Index of the lane the cursor is in.
    fn focused_lane(&self, lanes: &[Lane]) -> Option<usize> {
        match &self.focus {
            LaneFocus::Card => {
                let issue = self.current_column()?.issues.get(self.selected_row)?;
                let key = lanes::lane_key(issue, self.grouping);
                lanes.iter().position(|l| l.key == key)
            }
            LaneFocus::Header(key) | LaneFocus::Empty(key) => lanes.iter().position(|l| &l.key == key),
        }
    }

    fn focus_card(&mut self, column: usize, row: usize) {
        self.selected_column = column;
        self.selected_row = row;
        self.focus = LaneFocus::Card;
    }

    /// Keep the cursor somewhere valid after the board, grouping, or folds change.
    fn ensure_focus(&mut self) {
        if !self.is_grouped() {
            self.focus = LaneFocus::Card;
            return;
        }
        let lanes = self.lanes();
        match self.focused_lane(&lanes) {
            Some(i) if self.focus == LaneFocus::Card && self.is_folded(&lanes[i]) => {
                self.focus = LaneFocus::Header(lanes[i].key.clone());
            }
            Some(_) => {}
            None => match lanes.first() {
                Some(lane) => self.focus = LaneFocus::Header(lane.key.clone()),
                None => self.focus = LaneFocus::Card,
            },
        }
    }

    /// Key of the lane the cursor is in (grouped boards only).
    pub fn focused_lane_key(&self) -> Option<String> {
        let lanes = self.lanes();
        self.focused_lane(&lanes).map(|i| lanes[i].key.clone())
    }

    /// The cursor's depth within its cell (0 for headers and empty cells).
    fn cell_depth(&self, lane: &Lane) -> usize {
        match self.focus {
            LaneFocus::Card => lane.cells[self.selected_column]
                .iter()
                .position(|&r| r == self.selected_row)
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Put the cursor in a lane's cell at `column`, as close to `depth` as the
    /// cell allows (an empty cell is still a stop).
    fn enter_cell(&mut self, lane: &Lane, column: usize, depth: usize) {
        if self.is_folded(lane) {
            return;
        }
        self.selected_column = column;
        let cell = &lane.cells[column];
        match cell.get(depth.min(cell.len().saturating_sub(1))) {
            Some(&row) => self.focus_card(column, row),
            None => self.focus = LaneFocus::Empty(lane.key.clone()),
        }
    }

    /// Put the cursor on a lane's header: in the gutter (left of the columns),
    /// or passing through it on the way up/down a column.
    fn focus_header(&mut self, lane: &Lane, gutter: bool) {
        self.focus = LaneFocus::Header(lane.key.clone());
        self.in_gutter = gutter;
    }

    /// Enter a lane from below: its last card in this column, or its empty
    /// cell — or just its header when folded.
    fn enter_from_below(&mut self, lane: &Lane, column: usize) {
        if self.is_folded(lane) {
            return self.focus_header(lane, false);
        }
        match lane.cells[column].last() {
            Some(&row) => self.focus_card(column, row),
            None => self.focus = LaneFocus::Empty(lane.key.clone()),
        }
    }

    /// `j`/`k` on a grouped board. In a column they walk its cards and pass
    /// over each lane's header into the same column of the next lane; in the
    /// gutter they hop from header to header.
    fn grouped_vertical(&mut self, down: bool) {
        let lanes = self.lanes();
        let Some(li) = self.focused_lane(&lanes) else { return };
        let column = self.selected_column;
        let below = lanes.get(li + 1);
        let above = li.checked_sub(1).map(|i| &lanes[i]);

        if let LaneFocus::Header(_) = self.focus {
            if self.in_gutter {
                if let Some(lane) = if down { below } else { above } {
                    self.focus_header(lane, true);
                }
            } else if down {
                if !self.is_folded(&lanes[li]) {
                    self.enter_cell(&lanes[li], column, 0);
                } else if let Some(lane) = below {
                    self.focus_header(lane, false);
                }
            } else if let Some(lane) = above {
                self.enter_from_below(lane, column);
            }
            return;
        }

        let cell = &lanes[li].cells[column];
        if self.focus == LaneFocus::Card {
            if let Some(p) = cell.iter().position(|&r| r == self.selected_row) {
                if down && p + 1 < cell.len() {
                    return self.focus_card(column, cell[p + 1]);
                }
                if !down && p > 0 {
                    return self.focus_card(column, cell[p - 1]);
                }
            }
        }
        // Past the cell's edge: onto the next lane's header, or this lane's.
        if down {
            if let Some(lane) = below {
                self.focus_header(lane, false);
            }
        } else {
            self.focus_header(&lanes[li], false);
        }
    }

    /// `h`/`l` on a grouped board. Left of the first column is the gutter,
    /// where the cursor sits on the lane's header.
    fn grouped_step(&mut self, delta: isize) {
        let lanes = self.lanes();
        let Some(li) = self.focused_lane(&lanes) else { return };
        let lane = &lanes[li];
        if let LaneFocus::Header(_) = self.focus {
            match (self.in_gutter, delta > 0) {
                (true, true) => self.enter_cell(lane, 0, 0),
                (false, true) => self.enter_cell(lane, self.selected_column, 0),
                (false, false) => self.in_gutter = true,
                (true, false) => {}
            }
            return;
        }
        if delta < 0 && self.selected_column == 0 {
            return self.focus_header(lane, true);
        }
        let next = self.selected_column as isize + delta;
        if next >= 0 && (next as usize) < self.board.columns.len() {
            let depth = self.cell_depth(lane);
            self.enter_cell(lane, next as usize, depth);
        }
    }

    /// `1`-`9` on a grouped board: that column of the current lane.
    fn grouped_jump(&mut self, column: usize) {
        let lanes = self.lanes();
        let Some(li) = self.focused_lane(&lanes) else { return };
        let depth = self.cell_depth(&lanes[li]);
        self.enter_cell(&lanes[li], column, depth);
    }

    /// `Home`/`End`: first/last header in the gutter, else first/last card
    /// in the column across all unfolded lanes.
    fn grouped_edge(&mut self, top: bool) {
        let lanes = self.lanes();
        if lanes.is_empty() {
            return;
        }
        if let (LaneFocus::Header(_), true) = (&self.focus, self.in_gutter) {
            let lane = if top { &lanes[0] } else { &lanes[lanes.len() - 1] };
            return self.focus_header(lane, true);
        }
        let column = self.selected_column;
        let mut cards = lanes
            .iter()
            .filter(|lane| !self.is_folded(lane))
            .flat_map(|lane| lane.cells[column].iter().copied());
        let row = if top { cards.next() } else { cards.last() };
        if let Some(row) = row {
            self.focus_card(column, row);
        }
    }

    /// Fold or unfold the lane under the cursor.
    fn toggle_fold(&mut self) {
        let lanes = self.lanes();
        let Some(li) = self.focused_lane(&lanes) else { return };
        let key = lanes[li].key.clone();
        if !self.folded.remove(&key) {
            self.folded.insert(key.clone());
            self.focus = LaneFocus::Header(key);
        }
    }

    /// Fold every lane, or unfold them all if they're already folded.
    fn toggle_fold_all(&mut self) {
        let lanes = self.lanes();
        if lanes.iter().all(|l| self.folded.contains(&l.key)) {
            for lane in &lanes {
                self.folded.remove(&lane.key);
            }
        } else {
            self.folded.extend(lanes.iter().map(|l| l.key.clone()));
        }
        self.ensure_focus();
    }

    fn cycle_grouping(&mut self) {
        let selected = self.selected_issue().map(|i| i.key.clone());
        self.grouping = self.grouping.next();
        self.board_scroll = 0;
        self.focus = LaneFocus::Card;
        if !selected.is_some_and(|key| self.select_key(&key)) {
            self.clamp_selection();
            self.ensure_focus();
        }
    }

    fn clamp_selection(&mut self) {
        self.selected_column = self
            .selected_column
            .min(self.board.columns.len().saturating_sub(1));
        let rows = self.current_column().map(|c| c.issues.len()).unwrap_or(0);
        self.selected_row = self.selected_row.min(rows.saturating_sub(1));
    }

    fn select_column(&mut self, delta: isize) {
        if self.board.columns.is_empty() {
            return;
        }
        let count = self.board.columns.len() as isize;
        self.selected_column = (self.selected_column as isize + delta).rem_euclid(count) as usize;
        self.clamp_selection();
    }

    fn select_row(&mut self, delta: isize) {
        let rows = self.current_column().map(|c| c.issues.len()).unwrap_or(0);
        if rows == 0 {
            return;
        }
        self.selected_row = (self.selected_row as isize + delta).clamp(0, rows as isize - 1) as usize;
    }

    // ----- network results -------------------------------------------------

    /// Swap in a freshly fetched board, keeping the selected issue selected.
    pub fn on_board(&mut self, board: Board) {
        if matches!(self.mode, Mode::Moving(_) | Mode::TransitionForm(..)) {
            self.deferred_board = Some(board);
            return;
        }
        let selected = self.selected_issue().map(|i| i.key.clone());
        self.board = board;
        self.board_loaded = true;
        self.refreshing = false;
        let assignees: Vec<String> = self
            .board
            .columns
            .iter()
            .flat_map(|c| c.issues.iter().filter_map(|i| i.assignee.clone()))
            .collect();
        self.user_colors.assign(assignees.iter().map(String::as_str));
        self.load_error = None;
        // Re-apply moves Jira hasn't confirmed yet so cards don't jump back.
        for pending in self.in_flight.values() {
            if let Some(issue) = self.board.issue_mut(&pending.key) {
                issue.sync_state = SyncState::Pending;
            }
        }
        let selected = self.pending_select.take().or(selected);
        if !selected.is_some_and(|key| self.select_key(&key)) {
            self.clamp_selection();
            self.ensure_focus();
        }
        if let Some((focus, gutter, column)) = self.pending_focus.take() {
            self.focus = focus;
            self.in_gutter = gutter;
            self.selected_column = column.min(self.board.columns.len().saturating_sub(1));
            self.ensure_focus();
        }
        if !matches!(&self.status, Some((m, _)) if m.starts_with("reloaded")) {
            self.info(format!("{} issues", self.board.total_issues()));
        }
    }

    pub fn on_board_error(&mut self, message: String) {
        self.refreshing = false;
        if self.board_loaded {
            self.error(format!("refresh failed: {message}"));
        } else {
            self.load_error = Some(message);
        }
    }

    pub fn on_transitions(&mut self, key: &str, result: Result<(Vec<Transition>, RawFields), String>) {
        let Mode::Moving(state) = &self.mode else { return };
        if state.pending.key != key {
            return;
        }
        let old_status = state.pending.old_status.clone();
        let old_status_id = state.pending.old_status_id.clone();
        let (transitions, current) = match result {
            Ok(ok) => ok,
            Err(message) => {
                self.mode = Mode::Board;
                self.error(format!("couldn't load workflow for {key}: {message}"));
                return;
            }
        };

        let mut targets: Vec<MoveTarget> = Vec::new();
        let mut off_board = Vec::new();
        for t in transitions {
            if t.to_status_id == old_status_id {
                continue; // looped transitions don't move the card
            }
            match self.board.column_for_status(&t.to_status_id) {
                Some(column) => match targets.iter_mut().find(|m| m.column == column) {
                    Some(target) => target.transitions.push(t),
                    None => targets.push(MoveTarget { column, transitions: vec![t] }),
                },
                None => off_board.push(t.to_status_name),
            }
        }
        targets.sort_by_key(|t| t.column);

        if targets.is_empty() {
            self.mode = Mode::Board;
            let mut message = format!("{key}: no transitions to other board columns from {old_status}");
            if !off_board.is_empty() {
                message.push_str(&format!(" (off-board: {})", off_board.join(", ")));
            }
            self.info(message);
            return;
        }

        let names: Vec<&str> = targets
            .iter()
            .map(|t| self.board.columns[t.column].name.as_str())
            .collect();
        let message = format!("move {key} → {} · h/l choose · enter confirm · esc cancel", names.join(" / "));
        let Mode::Moving(state) = &mut self.mode else { return };
        state.targets = Some(targets);
        state.current = current;
        self.info(message);
    }

    pub fn on_transition_done(&mut self, key: &str, result: Result<(), JiraError>) {
        // A move submitted through a form: keep the form open on failure.
        if let Mode::TransitionForm(form, pending) = &mut self.mode {
            if form.key == key && form.submitting {
                match result {
                    Ok(()) => {
                        let status = form.to_status.clone();
                        self.in_flight.insert(key.to_string(), pending.clone());
                        self.mode = Mode::Board;
                        self.settle();
                        self.info(format!("{key} → {status}"));
                    }
                    Err(err) => {
                        form.apply_error(&err);
                        self.error(format!("{key}: {err}"));
                    }
                }
                return;
            }
        }

        match result {
            // The worker follows up with `IssueRefreshed`, which clears `in_flight`.
            Ok(()) => {}
            Err(err) => {
                if let Some(pending) = self.in_flight.remove(key) {
                    self.revert(&pending, SyncState::Error);
                }
                self.error(format!("{key}: move rejected — {err}"));
            }
        }
    }

    /// The authoritative issue after a successful transition.
    pub fn on_issue_refreshed(&mut self, mut issue: Issue) {
        let key = issue.key.clone();
        if let Some(name) = &issue.assignee {
            self.user_colors.assign([name.as_str()]);
        }
        // The Platform API only knows the epic's key; keep the color the board API gave.
        if let (Some(new), Some(old)) = (issue.epic.as_mut(), self.board.issue(&key).and_then(|i| i.epic.as_ref())) {
            if new.key == old.key && new.color.is_none() {
                new.color = old.color;
                new.name = old.name.clone();
            }
        }
        let was_selected = self.selected_issue().is_some_and(|i| i.key == key);
        self.in_flight.remove(&key);
        let moving_this = matches!(&self.mode, Mode::Moving(s) if s.pending.key == key)
            || matches!(&self.mode, Mode::TransitionForm(f, _) if f.key == key);
        if moving_this {
            return;
        }
        let target_column = self.board.column_for_status(&issue.status_id);
        match (self.board.find(&key), target_column) {
            (Some((c, r)), Some(target)) if c == target => self.board.columns[c].issues[r] = issue,
            (Some(_), Some(target)) => {
                self.board.relocate(&key, target, 0);
                if let Some(slot) = self.board.issue_mut(&key) {
                    *slot = issue;
                }
            }
            (None, Some(target)) => self.board.columns[target].issues.insert(0, issue),
            (Some((c, r)), None) => {
                self.board.columns[c].issues.remove(r);
            }
            (None, None) => {}
        }
        if was_selected && !self.select_key(&key) {
            self.clamp_selection();
        }
    }

    pub fn on_comments(&mut self, key: &str, result: Result<Vec<Comment>, String>) {
        if let Ok(comments) = &result {
            self.user_colors.assign(comments.iter().map(|c| c.author.as_str()));
        }
        let state = match result {
            Ok(comments) => CommentsState::Loaded(comments),
            Err(message) => CommentsState::Failed(message),
        };
        self.comments.insert(key.to_string(), state);
    }

    pub fn on_rich(&mut self, key: &str, result: Result<(Value, Value), String>) {
        let state = match result {
            Ok((description, acceptance)) => RichState::Loaded { description, acceptance },
            Err(message) => RichState::Failed(message),
        };
        self.rich.insert(key.to_string(), state);
    }

    pub fn on_rich_updated(&mut self, key: &str, field: RichField, result: Result<Value, JiraError>) {
        let Mode::EditRich(editor) = &mut self.mode else { return };
        if editor.key != key || editor.field != field {
            return;
        }
        match result {
            Ok(value) => {
                let text = adf::to_text(Some(&value));
                if let Some(issue) = self.board.issue_mut(key) {
                    match field {
                        RichField::Description => issue.description = text,
                        RichField::AcceptanceCriteria => issue.acceptance_criteria = text,
                    }
                }
                if let Some(RichState::Loaded { description, acceptance }) = self.rich.get_mut(key) {
                    match field {
                        RichField::Description => *description = value,
                        RichField::AcceptanceCriteria => *acceptance = value,
                    }
                }
                let scroll = editor.details_scroll;
                self.mode = Mode::Details { key: key.to_string(), scroll };
                self.info(format!("{key}: saved {}", field.label().to_lowercase()));
            }
            Err(err) => {
                editor.saving = false;
                editor.error = Some(err.to_string());
                self.error(format!("{key}: couldn't save {} — {err}", field.label().to_lowercase()));
            }
        }
    }

    /// The open field editor's text (for `$EDITOR`).
    pub fn rich_editor_text(&mut self) -> Option<String> {
        match &mut self.mode {
            Mode::EditRich(editor) => Some(editor.editor.lines().join("\n")),
            Mode::Create(form) => form.focused_text().map(|t| t.lines().join("\n")),
            _ => None,
        }
    }

    /// Replace the open field editor's text (back from `$EDITOR`).
    pub fn set_rich_editor_text(&mut self, text: &str) {
        match &mut self.mode {
            Mode::EditRich(editor) => editor.editor = new_text_area(text),
            Mode::Create(form) => {
                if let Some(area) = form.focused_text() {
                    *area = new_text_area(text);
                }
            }
            _ => {}
        }
    }

    /// Apply a board that arrived while a move was in progress.
    fn settle(&mut self) {
        if matches!(self.mode, Mode::Board | Mode::Details { .. } | Mode::Help { .. }) {
            if let Some(board) = self.deferred_board.take() {
                self.on_board(board);
            }
        }
    }

    /// Put a card back where it started.
    fn revert(&mut self, pending: &PendingMove, state: SyncState) {
        if self.board.find(&pending.key).is_some() {
            let column = pending.origin_column.min(self.board.columns.len().saturating_sub(1));
            self.board.relocate(&pending.key, column, pending.origin_row);
            if let Some(issue) = self.board.issue_mut(&pending.key) {
                issue.status = pending.old_status.clone();
                issue.status_id = pending.old_status_id.clone();
                issue.sync_state = state;
            }
        }
        if !self.select_key(&pending.key) {
            self.clamp_selection();
        }
    }

    // ----- keys ------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }

        // `?` opens the keymap from anywhere except while typing in a form field.
        let typing = matches!(&self.mode, Mode::TransitionForm(form, _) if form.editor.is_some())
            || matches!(self.mode, Mode::EditRich(_) | Mode::Create(_));
        if key.code == KeyCode::Char('?') && !typing {
            let previous = std::mem::replace(&mut self.mode, Mode::Board);
            self.mode = match previous {
                Mode::Help { return_to } => *return_to,
                other => Mode::Help { return_to: Box::new(other) },
            };
            return Action::None;
        }

        let action = match &self.mode {
            Mode::Board => self.handle_board_key(key),
            Mode::Help { .. } => self.handle_help_key(key),
            Mode::Details { .. } => self.handle_details_key(key),
            Mode::Moving(_) => self.handle_move_key(key),
            Mode::TransitionForm(..) => self.handle_form_key(key),
            Mode::EditRich(_) => self.handle_rich_key(key),
            Mode::Create(_) => self.handle_create_key(key),
        };
        self.settle();
        action
    }

    // ----- restart on reinstall --------------------------------------------

    /// Whether swapping in a new binary would lose nothing: no move, form, or
    /// unconfirmed write in progress.
    pub fn can_restart(&self) -> bool {
        matches!(self.mode, Mode::Board | Mode::Details { .. } | Mode::Help { .. }) && self.in_flight.is_empty()
    }

    /// View state to hand to the restarted process.
    pub fn resume_state(&self) -> String {
        let mut folded: Vec<&String> = self.folded.iter().collect();
        folded.sort();
        let (focus, lane) = match &self.focus {
            LaneFocus::Card => ("card", None),
            LaneFocus::Header(key) => ("header", Some(key)),
            LaneFocus::Empty(key) => ("empty", Some(key)),
        };
        serde_json::json!({
            "grouping": self.grouping.as_str(),
            "folded": folded,
            "selected": self.selected_issue().map(|i| i.key.clone()),
            "focus": focus,
            "lane": lane,
            "gutter": self.in_gutter,
            "column": self.selected_column,
        })
        .to_string()
    }

    /// Restore view state from a previous process (see [`App::resume_state`]).
    pub fn apply_resume(&mut self, state: &str) {
        let Ok(state) = serde_json::from_str::<Value>(state) else { return };
        self.grouping = Grouping::parse(state["grouping"].as_str().unwrap_or_default());
        self.folded = state["folded"]
            .as_array()
            .map(|keys| keys.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        self.pending_select = state["selected"].as_str().map(str::to_string);
        let lane = state["lane"].as_str().map(str::to_string);
        let focus = match (state["focus"].as_str(), lane) {
            (Some("header"), Some(key)) => Some(LaneFocus::Header(key)),
            (Some("empty"), Some(key)) => Some(LaneFocus::Empty(key)),
            _ => None,
        };
        if let Some(focus) = focus {
            let column = state["column"].as_u64().unwrap_or(0) as usize;
            self.pending_focus = Some((focus, state["gutter"].as_bool().unwrap_or(false), column));
        }
        self.info("reloaded: running the newly installed jdo");
    }

    // ----- mouse -----------------------------------------------------------

    pub fn handle_mouse(&mut self, event: MouseEvent) -> Action {
        let (x, y) = (event.column, event.row);
        match event.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.mouse = Some((x, y));
                Action::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse = Some((x, y));
                let target = self
                    .hits
                    .iter()
                    .rev()
                    .find(|hit| contains(hit.area, x, y))
                    .map(|hit| hit.target.clone());
                match target {
                    Some(target) => self.click(target),
                    None => Action::None,
                }
            }
            MouseEventKind::ScrollDown => self.wheel(true),
            MouseEventKind::ScrollUp => self.wheel(false),
            _ => Action::None,
        }
    }

    fn click(&mut self, target: HitTarget) -> Action {
        match target {
            HitTarget::Key(key) => self.handle_key(key),
            HitTarget::CloseDetails => {
                if matches!(self.mode, Mode::Details { .. }) {
                    self.mode = Mode::Board;
                    self.settle();
                }
                Action::None
            }
            // Board clicks only count when no modal covers the board.
            HitTarget::Card { column, row } if matches!(self.mode, Mode::Board) => {
                let already_selected =
                    self.focus == LaneFocus::Card && self.selected_column == column && self.selected_row == row;
                if already_selected {
                    return self.open_details();
                }
                self.focus_card(column, row);
                Action::None
            }
            HitTarget::Create(hit) => {
                let Mode::Create(form) = &mut self.mode else { return Action::None };
                let outcome = match hit {
                    CreateHit::Focus(field) => {
                        form.focus = crate::create::CreateFocus::Field(field);
                        CreateOutcome::None
                    }
                    CreateHit::Pick(field, index) => {
                        form.choose(field, index);
                        CreateOutcome::None
                    }
                    CreateHit::Submit => form.try_submit(),
                    CreateHit::Cancel => CreateOutcome::Cancel,
                };
                self.create_outcome(outcome)
            }
            HitTarget::LaneHeader(key) if matches!(self.mode, Mode::Board) => {
                if self.focus == LaneFocus::Header(key.clone()) {
                    self.toggle_fold();
                } else {
                    self.focus = LaneFocus::Header(key);
                    self.in_gutter = true;
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn wheel(&mut self, down: bool) -> Action {
        match &mut self.mode {
            Mode::Details { scroll, .. } => {
                *scroll = if down { scroll.saturating_add(3) } else { scroll.saturating_sub(3) };
                Action::None
            }
            Mode::Board => self.handle_key(KeyEvent::from(if down { KeyCode::Down } else { KeyCode::Up })),
            _ => Action::None,
        }
    }

    fn handle_board_key(&mut self, key: KeyEvent) -> Action {
        if self.is_grouped() {
            match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    self.grouped_step(-1);
                    return Action::None;
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    self.grouped_step(1);
                    return Action::None;
                }
                KeyCode::Char(c @ '1'..='9') => {
                    let index = c as usize - '1' as usize;
                    if index < self.board.columns.len() {
                        self.grouped_jump(index);
                    }
                    return Action::None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.grouped_vertical(true);
                    return Action::None;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.grouped_vertical(false);
                    return Action::None;
                }
                KeyCode::Home => {
                    self.grouped_edge(true);
                    return Action::None;
                }
                KeyCode::End => {
                    self.grouped_edge(false);
                    return Action::None;
                }
                KeyCode::Enter | KeyCode::Char(' ') if matches!(self.focus, LaneFocus::Header(_)) => {
                    self.toggle_fold();
                    return Action::None;
                }
                KeyCode::Char('z') => {
                    self.toggle_fold();
                    return Action::None;
                }
                KeyCode::Char('Z') => {
                    self.toggle_fold_all();
                    return Action::None;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('g') => self.cycle_grouping(),
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('r') => {
                if self.board_loaded {
                    self.refreshing = true;
                } else {
                    // Retry a failed initial load; the spinner takes over again.
                    self.load_error = None;
                }
                return Action::Refresh;
            }
            KeyCode::Char('h') | KeyCode::Left => self.select_column(-1),
            KeyCode::Char('l') | KeyCode::Right => self.select_column(1),
            KeyCode::Char('k') | KeyCode::Up => self.select_row(-1),
            KeyCode::Char('j') | KeyCode::Down => self.select_row(1),
            KeyCode::Home => self.selected_row = 0,
            KeyCode::End => self.select_row(isize::MAX / 2),
            KeyCode::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if index < self.board.columns.len() {
                    self.selected_column = index;
                    self.clamp_selection();
                }
            }
            KeyCode::Enter => return self.open_details(),
            KeyCode::Char('m') => return self.begin_move(),
            KeyCode::Char('c') => return self.begin_create(),
            _ => {}
        }
        Action::None
    }

    // ----- creating a card --------------------------------------------------

    fn begin_create(&mut self) -> Action {
        if !self.board_loaded || self.board.project_key.is_empty() {
            self.error("can't create yet: the board's project isn't known");
            return Action::None;
        }
        // Grouped by epic: default to the epic section the cursor is in.
        let lane_epic = match (self.grouping, self.focused_lane_key()) {
            (Grouping::Epic, Some(key)) => self
                .lanes()
                .into_iter()
                .find(|l| l.key == key)
                .and_then(|l| match l.kind {
                    lanes::LaneKind::Epic(epic) => Some(epic),
                    _ => None,
                }),
            _ => None,
        };
        let form = CreateForm::new(&self.board, lane_epic.as_ref(), self.me.as_ref());
        let project = form.project_key.clone();
        let team = form.team_id();
        self.mode = Mode::Create(Box::new(form));
        match team {
            Some(team) => Action::Many(vec![Action::FetchCreateMeta(project), Action::FetchTeamMembers(team)]),
            None => {
                if let Mode::Create(form) = &mut self.mode {
                    form.assignee.loading = false;
                }
                Action::FetchCreateMeta(project)
            }
        }
    }

    pub fn on_team_members(&mut self, result: Result<Vec<(String, String)>, String>) {
        let Mode::Create(form) = &mut self.mode else { return };
        match result {
            Ok(members) => {
                self.user_colors.assign(members.iter().map(|(_, name)| name.as_str()));
                form.add_members(members);
            }
            Err(message) => {
                form.assignee.loading = false;
                self.error(format!("couldn't load the team's members: {message}"));
            }
        }
    }

    fn handle_create_key(&mut self, key: KeyEvent) -> Action {
        let Mode::Create(form) = &mut self.mode else { return Action::None };
        let outcome = form.handle_key(key);
        self.create_outcome(outcome)
    }

    fn create_outcome(&mut self, outcome: CreateOutcome) -> Action {
        match outcome {
            CreateOutcome::None => {
                if let Mode::Create(form) = &self.mode {
                    if form.confirm_discard {
                        self.info("press Esc again to discard this ticket");
                    }
                }
                Action::None
            }
            CreateOutcome::Cancel => {
                self.mode = Mode::Board;
                self.info("create cancelled");
                Action::None
            }
            CreateOutcome::EditExternally => Action::EditExternally,
            CreateOutcome::Submit(draft) => {
                self.info(format!("creating {} ticket…", draft.project_key));
                Action::CreateIssue(draft)
            }
        }
    }

    pub fn on_create_meta(&mut self, result: Result<CreateMeta, String>) {
        let Mode::Create(form) = &mut self.mode else { return };
        match result {
            Ok(meta) => form.meta = Some(meta),
            Err(message) => form.meta_error = Some(message),
        }
    }

    pub fn on_created(&mut self, result: Result<Issue, CreateError>) {
        let Mode::Create(form) = &mut self.mode else { return };
        match result {
            Ok(issue) => {
                // Only show it if it belongs to what this board displays.
                let chosen = &form.sprint.chosen;
                let active = chosen
                    .key
                    .as_deref()
                    .and_then(|k| k.parse::<u64>().ok())
                    .is_some_and(|id| self.board.sprints.iter().any(|s| s.id == id && s.active));
                let on_board = self.board.sprints.is_empty() || active;
                let where_to = if chosen.key.is_some() { chosen.label.clone() } else { "the backlog".into() };
                let key = issue.key.clone();
                self.mode = Mode::Board;
                if on_board && self.board.column_for_status(&issue.status_id).is_some() {
                    self.on_issue_refreshed(issue);
                    self.select_key(&key);
                    self.info(format!("created {key}"));
                } else {
                    self.info(format!("created {key} in {where_to} (not shown on this board view)"));
                }
            }
            Err(error) => {
                form.apply_error(&error);
                self.error("Jira didn't create the ticket — see the errors in the form");
            }
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) -> Action {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
            if let Mode::Help { return_to } = std::mem::replace(&mut self.mode, Mode::Board) {
                self.mode = *return_to;
            }
        }
        Action::None
    }

    fn open_details(&mut self) -> Action {
        let Some(issue) = self.selected_issue() else { return Action::None };
        let key = issue.key.clone();
        self.mode = Mode::Details { key: key.clone(), scroll: 0 };
        // Always refetch: comments and text may have changed since last time.
        self.comments.insert(key.clone(), CommentsState::Loading);
        if !matches!(self.rich.get(&key), Some(RichState::Loaded { .. })) {
            self.rich.insert(key.clone(), RichState::Loading);
        }
        Action::FetchDetails(key)
    }

    fn handle_details_key(&mut self, key: KeyEvent) -> Action {
        let Mode::Details { scroll, .. } = &mut self.mode else { return Action::None };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.mode = Mode::Board,
            KeyCode::Char('d') if ctrl => *scroll = scroll.saturating_add(10),
            KeyCode::Char('u') if ctrl => *scroll = scroll.saturating_sub(10),
            KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
            KeyCode::Char('G') | KeyCode::End => *scroll = u16::MAX,
            KeyCode::Char('m') => {
                self.mode = Mode::Board;
                return self.begin_move();
            }
            KeyCode::Char('e') => self.begin_rich_edit(RichField::Description),
            KeyCode::Char('a') => self.begin_rich_edit(RichField::AcceptanceCriteria),
            _ => {}
        }
        Action::None
    }

    fn begin_rich_edit(&mut self, field: RichField) {
        let Mode::Details { key, scroll } = &self.mode else { return };
        let (key, scroll) = (key.clone(), *scroll);
        let value = match self.rich.get(&key) {
            Some(RichState::Loaded { .. }) => self.rich[&key].get(field).cloned().unwrap_or(Value::Null),
            Some(RichState::Failed(message)) => {
                return self.error(format!("can't edit: couldn't load {} — {message}", field.label().to_lowercase()));
            }
            _ => return self.info(format!("still loading the {}…", field.label().to_lowercase())),
        };
        let markdown = markdown::to_markdown(if value.is_null() { None } else { Some(&value) });
        self.mode = Mode::EditRich(Box::new(RichEditor {
            key,
            field,
            editor: new_text_area(&markdown.text),
            original: markdown.text.clone(),
            markdown,
            saving: false,
            error: None,
            details_scroll: scroll,
        }));
    }

    fn handle_rich_key(&mut self, key: KeyEvent) -> Action {
        let Mode::EditRich(editor) = &mut self.mode else { return Action::None };
        if editor.saving {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                let (key, scroll) = (editor.key.clone(), editor.details_scroll);
                self.mode = Mode::Details { key, scroll };
                self.info("edit cancelled");
            }
            KeyCode::Char('g') if ctrl => return Action::EditExternally,
            KeyCode::Char('s') if ctrl => {
                let text = editor.editor.lines().join("\n");
                if text == editor.original {
                    let (key, scroll) = (editor.key.clone(), editor.details_scroll);
                    self.mode = Mode::Details { key, scroll };
                    self.info("no changes");
                    return Action::None;
                }
                editor.saving = true;
                editor.error = None;
                let value = markdown::to_adf(&text, &editor.markdown.tokens);
                return Action::SaveRich { key: editor.key.clone(), field: editor.field, value };
            }
            _ => {
                editor.editor.input(key);
            }
        }
        Action::None
    }

    fn begin_move(&mut self) -> Action {
        let Some(issue) = self.selected_issue() else { return Action::None };
        if self.in_flight.contains_key(&issue.key) {
            self.info(format!("{} is still syncing", issue.key));
            return Action::None;
        }
        let pending = PendingMove {
            key: issue.key.clone(),
            origin_column: self.selected_column,
            origin_row: self.selected_row,
            old_status: issue.status.clone(),
            old_status_id: issue.status_id.clone(),
        };
        let key = pending.key.clone();
        self.mode = Mode::Moving(MoveState {
            pending,
            targets: None,
            current: Value::Null,
            choice: 0,
        });
        self.info(format!("loading workflow for {key}…"));
        Action::FetchTransitions(key)
    }

    fn handle_move_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.cancel_move();
                return Action::None;
            }
            KeyCode::Left | KeyCode::Char('h') => self.step_move(-1),
            KeyCode::Right | KeyCode::Char('l') => self.step_move(1),
            KeyCode::Tab | KeyCode::Char('j') | KeyCode::Down => self.cycle_choice(1),
            KeyCode::BackTab | KeyCode::Char('k') | KeyCode::Up => self.cycle_choice(-1),
            KeyCode::Enter | KeyCode::Char('m') => return self.confirm_move(),
            _ => {}
        }
        Action::None
    }

    /// Columns the card may occupy: its origin plus every reachable column.
    fn move_stops(&self) -> Vec<usize> {
        let Mode::Moving(state) = &self.mode else { return Vec::new() };
        let mut stops: Vec<usize> = state
            .targets
            .iter()
            .flatten()
            .map(|t| t.column)
            .collect();
        stops.push(state.pending.origin_column);
        stops.sort_unstable();
        stops.dedup();
        stops
    }

    /// Jump the card to the previous/next reachable column.
    fn step_move(&mut self, delta: isize) {
        let stops = self.move_stops();
        let Mode::Moving(state) = &mut self.mode else { return };
        if state.targets.is_none() {
            return;
        }
        let Some(here) = stops.iter().position(|&c| c == self.selected_column) else { return };
        let next = here as isize + delta;
        if next < 0 || next >= stops.len() as isize {
            return;
        }
        let column = stops[next as usize];
        let row = if column == state.pending.origin_column { state.pending.origin_row } else { 0 };
        let key = state.pending.key.clone();
        state.choice = 0;
        if let Some((c, r)) = self.board.relocate(&key, column, row) {
            self.focus_card(c, r);
        }
    }

    fn cycle_choice(&mut self, delta: isize) {
        let column = self.selected_column;
        let Mode::Moving(state) = &mut self.mode else { return };
        let Some(target) = state.targets.iter().flatten().find(|t| t.column == column) else { return };
        let n = target.transitions.len() as isize;
        if n > 1 {
            state.choice = (state.choice as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// The transition Enter would perform in the card's current column.
    pub fn chosen_transition(&self) -> Option<(&Transition, usize)> {
        let Mode::Moving(state) = &self.mode else { return None };
        let target = state.targets.iter().flatten().find(|t| t.column == self.selected_column)?;
        let index = state.choice.min(target.transitions.len() - 1);
        Some((&target.transitions[index], target.transitions.len()))
    }

    fn cancel_move(&mut self) {
        if let Mode::Moving(state) = std::mem::replace(&mut self.mode, Mode::Board) {
            self.revert(&state.pending, SyncState::Synced);
        }
        self.info("move cancelled");
    }

    fn confirm_move(&mut self) -> Action {
        let Some((transition, _)) = self.chosen_transition() else {
            // Back at the origin with nothing chosen (or still loading).
            if matches!(&self.mode, Mode::Moving(s) if s.targets.is_some()) {
                self.cancel_move();
            }
            return Action::None;
        };
        let transition = transition.clone();
        let Mode::Moving(state) = std::mem::replace(&mut self.mode, Mode::Board) else {
            return Action::None;
        };
        let key = state.pending.key.clone();

        if needs_form(&transition) {
            let form = TransitionForm::new(&key, &transition, &state.current, self.me.as_ref());
            self.info(format!("{key}: {} — fill in the fields, then Submit", transition.name));
            self.mode = Mode::TransitionForm(Box::new(form), state.pending);
            return Action::None;
        }

        if let Some(issue) = self.board.issue_mut(&key) {
            issue.status = transition.to_status_name.clone();
            issue.status_id = transition.to_status_id.clone();
            issue.sync_state = SyncState::Pending;
        }
        self.in_flight.insert(key.clone(), state.pending);
        self.info(format!("{key} → {}…", transition.to_status_name));
        Action::SubmitTransition {
            key,
            body: serde_json::json!({ "transition": { "id": transition.id } }),
        }
    }

    fn handle_form_key(&mut self, key: KeyEvent) -> Action {
        let Mode::TransitionForm(form, _) = &mut self.mode else { return Action::None };
        match form.handle_key(key) {
            FormOutcome::None => Action::None,
            FormOutcome::Cancel => {
                if let Mode::TransitionForm(_, pending) = std::mem::replace(&mut self.mode, Mode::Board) {
                    self.revert(&pending, SyncState::Synced);
                }
                self.info("move cancelled");
                Action::None
            }
            FormOutcome::Submit => {
                let key = form.key.clone();
                let body = form.payload();
                let status = form.to_status.clone();
                if let Some(issue) = self.board.issue_mut(&key) {
                    issue.sync_state = SyncState::Pending;
                    issue.status = status.clone();
                }
                self.info(format!("{key} → {status}… submitting"));
                Action::SubmitTransition { key, body }
            }
        }
    }
}

/// A text area holding `text`, cursor at the start.
fn new_text_area(text: &str) -> TextArea<'static> {
    TextArea::new(text.replace("\r\n", "\n").split('\n').map(str::to_owned).collect())
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, EpicRef};
    use crossterm::event::KeyEvent;

    fn issue(key: &str, epic: &str) -> Issue {
        Issue {
            key: key.into(),
            epic: Some(EpicRef { key: format!("E-{epic}"), name: epic.into(), color: None }),
            ..Default::default()
        }
    }

    /// Two columns; epic "A" has A-1 (col 0) and A-2 (col 1); epic "B" has B-1, B-2 in col 0.
    fn app() -> App {
        let board = Board {
            columns: vec![
                Column { issues: vec![issue("A-1", "A"), issue("B-1", "B"), issue("B-2", "B")], ..Default::default() },
                Column { issues: vec![issue("A-2", "A")], ..Default::default() },
            ],
            ..Default::default()
        };
        let mut app = App::new(Board::default(), 1);
        app.on_board(board);
        app
    }

    fn press(app: &mut App, c: char) {
        app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }

    fn selected(app: &App) -> Option<String> {
        app.selected_issue().map(|i| i.key.clone())
    }

    #[test]
    fn view_state_survives_a_restart() {
        let mut before = app();
        press(&mut before, 'g');
        press(&mut before, 'j');
        press(&mut before, 'j'); // B-1
        before.folded.insert("epic:E-A".into());
        let state = before.resume_state();

        let mut after = App::new(Board::default(), 1);
        after.apply_resume(&state);
        after.on_board(before.board.clone());
        assert_eq!(after.grouping, Grouping::Epic);
        assert!(after.folded.contains("epic:E-A"));
        assert_eq!(selected(&after).as_deref(), Some("B-1"));
    }

    #[test]
    fn an_empty_cell_cursor_survives_a_restart() {
        let mut before = app();
        press(&mut before, 'g');
        press(&mut before, 'l'); // A-2
        press(&mut before, 'j'); // B's header
        press(&mut before, 'j'); // B's empty cell in column 1
        let mut after = App::new(Board::default(), 1);
        after.apply_resume(&before.resume_state());
        after.on_board(before.board.clone());
        assert_eq!(after.focus, LaneFocus::Empty("epic:E-B".into()));
        assert_eq!(after.selected_column, 1);
    }

    #[test]
    fn restarts_wait_for_moves_and_forms() {
        let mut app = app();
        assert!(app.can_restart());
        app.handle_key(KeyEvent::from(KeyCode::Char('m')));
        assert!(!app.can_restart());
    }

    fn open_with_description(app: &mut App) {
        app.handle_key(KeyEvent::from(KeyCode::Enter)); // details for A-1
        let doc = serde_json::json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Old text"}]}
        ]});
        app.on_rich("A-1", Ok((doc, Value::Null)));
    }

    #[test]
    fn editing_a_description_saves_markdown_as_adf() {
        let mut app = app();
        open_with_description(&mut app);
        press(&mut app, 'e');
        let Mode::EditRich(editor) = &mut app.mode else { panic!("editor should open") };
        assert_eq!(editor.original, "Old text");
        editor.editor = new_text_area("New **bold** text");
        let action = app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let Action::SaveRich { key, field, value } = action else { panic!("should save") };
        assert_eq!((key.as_str(), field), ("A-1", RichField::Description));
        assert_eq!(value["content"][0]["content"][1]["marks"][0]["type"], "strong");

        app.on_rich_updated("A-1", RichField::Description, Ok(value));
        assert!(matches!(app.mode, Mode::Details { .. }));
        assert_eq!(app.board.issue("A-1").unwrap().description, "New bold text");
    }

    #[test]
    fn unchanged_text_does_not_save_and_errors_keep_the_editor_open() {
        let mut app = app();
        open_with_description(&mut app);
        press(&mut app, 'e');
        let none = app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(matches!(none, Action::None));
        assert!(matches!(app.mode, Mode::Details { .. }), "no changes closes without a request");

        press(&mut app, 'e');
        if let Mode::EditRich(editor) = &mut app.mode {
            editor.editor = new_text_area("changed");
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let mut err = JiraError::default();
        err.messages.push("Field is not editable".into());
        app.on_rich_updated("A-1", RichField::Description, Err(err));
        let Mode::EditRich(editor) = &app.mode else { panic!("editor stays open") };
        assert!(!editor.saving);
        assert_eq!(editor.editor.lines(), ["changed"]);
        assert!(editor.error.as_deref().unwrap().contains("not editable"));
    }

    #[test]
    fn typing_a_question_mark_in_the_editor_does_not_open_help() {
        let mut app = app();
        open_with_description(&mut app);
        press(&mut app, 'e');
        press(&mut app, '?');
        assert!(matches!(app.mode, Mode::EditRich(_)));
    }

    #[test]
    fn c_opens_the_create_dialog_and_a_created_card_lands_on_the_board() {
        let mut app = app();
        app.board.project_key = "DEMO".into();
        app.board.columns[0].status_ids = vec!["1".into()];
        let action = app.handle_key(KeyEvent::from(KeyCode::Char('c')));
        assert!(matches!(action, Action::FetchCreateMeta(ref p) if p == "DEMO"));
        assert!(matches!(app.mode, Mode::Create(_)));
        // `?` types into the dialog rather than opening the keymap.
        press(&mut app, '?');
        assert!(matches!(app.mode, Mode::Create(_)));

        let created = Issue { key: "DEMO-9".into(), status_id: "1".into(), ..Default::default() };
        app.on_created(Ok(created));
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(selected(&app).as_deref(), Some("DEMO-9"));
    }

    #[test]
    fn grouped_by_epic_the_dialog_defaults_to_the_current_section() {
        let mut app = app();
        app.board.project_key = "DEMO".into();
        press(&mut app, 'g'); // grouped by epic; cursor in lane A
        app.handle_key(KeyEvent::from(KeyCode::Char('c')));
        let Mode::Create(form) = &app.mode else { panic!("dialog should open") };
        assert_eq!(form.epic.chosen.key.as_deref(), Some("E-A"));
    }

    #[test]
    fn grouping_keeps_the_selected_card() {
        let mut app = app();
        press(&mut app, 'j'); // B-1
        press(&mut app, 'g');
        assert_eq!(app.grouping, Grouping::Epic);
        assert_eq!(selected(&app).as_deref(), Some("B-1"));
    }

    #[test]
    fn down_passes_over_the_next_lane_header_into_the_same_column() {
        let mut app = app();
        press(&mut app, 'g'); // epic lanes: A, B; cursor on A-1 (column 0)
        press(&mut app, 'j'); // past A's box: B's header, column kept
        assert_eq!(app.focus, LaneFocus::Header("epic:E-B".into()));
        assert!(!app.in_gutter);
        press(&mut app, 'j');
        assert_eq!(selected(&app).as_deref(), Some("B-1"));
        press(&mut app, 'j');
        assert_eq!(selected(&app).as_deref(), Some("B-2"));
        press(&mut app, 'k');
        press(&mut app, 'k'); // top of B's cell: B's header
        assert_eq!(app.focus, LaneFocus::Header("epic:E-B".into()));
        press(&mut app, 'k'); // into lane A's last card in column 0
        assert_eq!(selected(&app).as_deref(), Some("A-1"));
    }

    #[test]
    fn passing_through_keeps_the_column_even_into_an_empty_cell() {
        let mut app = app();
        press(&mut app, 'g');
        press(&mut app, 'l'); // A-2, column 1
        press(&mut app, 'j'); // B's header
        press(&mut app, 'j'); // B has nothing in column 1
        assert_eq!(app.focus, LaneFocus::Empty("epic:E-B".into()));
        assert_eq!(app.selected_column, 1);
        press(&mut app, 'k');
        press(&mut app, 'k');
        assert_eq!(selected(&app).as_deref(), Some("A-2"));
    }

    #[test]
    fn the_gutter_hops_between_lane_headers() {
        let mut app = app();
        press(&mut app, 'g');
        press(&mut app, 'h'); // left of the first column: lane A's header
        assert_eq!(app.focus, LaneFocus::Header("epic:E-A".into()));
        press(&mut app, 'j');
        assert_eq!(app.focus, LaneFocus::Header("epic:E-B".into()));
        press(&mut app, 'j'); // no lane below: stays
        assert_eq!(app.focus, LaneFocus::Header("epic:E-B".into()));
        press(&mut app, 'l'); // back into the first column of lane B
        assert_eq!(selected(&app).as_deref(), Some("B-1"));
    }

    #[test]
    fn horizontal_moves_stay_in_the_lane_even_into_empty_cells() {
        let mut app = app();
        press(&mut app, 'g');
        press(&mut app, 'l');
        assert_eq!(selected(&app).as_deref(), Some("A-2"));
        press(&mut app, 'h');
        press(&mut app, 'j'); // B's header
        press(&mut app, 'j'); // B-1
        press(&mut app, 'l'); // B has nothing in column 1
        assert_eq!(app.focus, LaneFocus::Empty("epic:E-B".into()));
        assert_eq!(selected(&app), None);
    }

    #[test]
    fn folded_lanes_are_skipped_and_fold_all_toggles() {
        let mut app = app();
        press(&mut app, 'g');
        press(&mut app, 'z'); // fold A from its card
        assert_eq!(app.focus, LaneFocus::Header("epic:E-A".into()));
        press(&mut app, 'j'); // folded A: straight on to B's header
        assert_eq!(app.focus, LaneFocus::Header("epic:E-B".into()));
        press(&mut app, 'j');
        press(&mut app, 'k');
        press(&mut app, 'k'); // up past B's header stops on folded A's header
        assert_eq!(app.focus, LaneFocus::Header("epic:E-A".into()));
        press(&mut app, 'Z'); // fold all
        assert_eq!(app.folded.len(), 2);
        press(&mut app, 'Z'); // unfold all
        assert!(app.folded.is_empty());
    }

    #[test]
    fn move_mode_cannot_start_from_a_lane_header() {
        let mut app = app();
        press(&mut app, 'g');
        press(&mut app, 'h'); // header A
        assert!(matches!(app.handle_key(KeyEvent::from(KeyCode::Char('m'))), Action::None));
        assert!(matches!(app.mode, Mode::Board));
    }

    fn click(app: &mut App, target: HitTarget) -> Action {
        app.hits = vec![Hit { area: Rect::new(0, 0, 10, 10), target }];
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn clicking_selects_then_opens_a_card_and_the_x_closes_it() {
        let mut app = app();
        click(&mut app, HitTarget::Card { column: 1, row: 0 });
        assert_eq!(selected(&app).as_deref(), Some("A-2"));
        assert!(matches!(app.mode, Mode::Board));
        assert!(matches!(click(&mut app, HitTarget::Card { column: 1, row: 0 }), Action::FetchDetails(_)));
        assert!(matches!(app.mode, Mode::Details { .. }));
        // Cards under the open modal aren't clickable.
        click(&mut app, HitTarget::Card { column: 0, row: 0 });
        assert_eq!(selected(&app).as_deref(), Some("A-2"));
        click(&mut app, HitTarget::CloseDetails);
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn clicking_a_footer_hint_presses_its_key() {
        let mut app = app();
        click(&mut app, HitTarget::Key(KeyEvent::from(KeyCode::Char('g'))));
        assert_eq!(app.grouping, Grouping::Epic);
        click(&mut app, HitTarget::Key(KeyEvent::from(KeyCode::Char('?'))));
        assert!(matches!(app.mode, Mode::Help { .. }));
    }

    #[test]
    fn clicking_a_lane_header_focuses_then_folds_it() {
        let mut app = app();
        press(&mut app, 'g');
        click(&mut app, HitTarget::LaneHeader("epic:E-B".into()));
        assert_eq!(app.focus, LaneFocus::Header("epic:E-B".into()));
        click(&mut app, HitTarget::LaneHeader("epic:E-B".into()));
        assert!(app.folded.contains("epic:E-B"));
    }

}
