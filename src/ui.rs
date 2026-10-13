//! Rendering.
//!
//! Follows tdo's look: accent-on-dark, a `▊` bar marking the selected card,
//! and centered modals with an accent border for floating views (keymap,
//! details, transition form). Draw functions take `&mut App` only to persist
//! scroll offsets that keep the selection on screen.

use ratatui::buffer::Buffer;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, CommentsState, CreateHit, Hit, HitTarget, LaneFocus, Mode, RichEditor, RichState, StatusLevel};
use crate::create::{CreateField, CreateFocus, CreateForm, PickOption};
use crate::jira::adf;
use crate::colors::{self, UserColors};
use crate::form::{FieldKind, TransitionForm};
use crate::lanes::{Grouping, Lane, LaneKind};
use crate::model::{EpicRef, Issue, Load, RichField, SyncState};

/// tdo's signature accent (TUI_ACCENT) and background.
const ACCENT: Color = Color::Rgb(255, 135, 0);
const BACKGROUND: Color = Color::Rgb(0, 0, 0);
const TEXT: Color = Color::Rgb(230, 230, 230);
const MUTED: Color = Color::Rgb(128, 128, 128);
/// Secondary text that still needs to read well: card ids, points, column titles.
const SUBTLE: Color = Color::Rgb(180, 180, 180);
const BORDER: Color = Color::Rgb(88, 88, 88);
const DIM: Color = Color::Rgb(50, 50, 50);
const REACHABLE: Color = Color::Rgb(95, 175, 95);
const DANGER: Color = Color::Rgb(215, 95, 95);
const CHANGE: Color = Color::Rgb(215, 175, 0);

const MIN_COLUMN_WIDTH: u16 = 24;
/// Left margin of a grouped board: the cursor sits here on a lane's header.
const GUTTER: u16 = 2;
const UNASSIGNED: Color = Color::Rgb(235, 52, 52);
/// Background of a hovered footer hint.
const HOVER: Color = Color::Rgb(70, 70, 70);
/// Background of the lane header the cursor is on.
const HEADER_FOCUS: Color = Color::Rgb(58, 58, 58);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [title, board, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    app.hits.clear();
    frame.render_widget(Block::default().style(Style::default().bg(BACKGROUND)), frame.area());
    draw_title(frame, title, app);
    draw_board(frame, board, app);
    draw_status(frame, status, app);
    draw_footer(frame, footer, app);

    if let Mode::Help { .. } = &app.mode {
        draw_help(frame);
    }
    if let Mode::Details { .. } = &app.mode {
        draw_details(frame, app);
    }
    if let Mode::TransitionForm(form, _) = &mut app.mode {
        draw_form(frame, form);
    }
    if let Mode::EditRich(editor) = &mut app.mode {
        draw_rich_editor(frame, editor);
    }
    if let Mode::Create(form) = &mut app.mode {
        let mut hits = Vec::new();
        draw_create(frame, form, &app.user_colors, &mut hits);
        app.hits.extend(hits);
    }
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(" jdo ", Style::default().fg(BACKGROUND).bg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(app.board.name.clone(), Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
    ];
    if !app.board.scope.is_empty() {
        spans.push(Span::styled(format!(" · {}", app.board.scope), Style::default().fg(MUTED)));
    }
    let mut view = Vec::new();
    if app.is_grouped() {
        view.push(app.grouping.label());
    }
    if app.mine_only {
        view.push("only mine");
    }
    if !view.is_empty() {
        spans.push(Span::styled(format!(" ({})", view.join(" · ")), Style::default().fg(ACCENT)));
    }
    let gutter = if app.is_grouped() { GUTTER } else { 0 };
    let visible = visible_lane_count(area.width.saturating_sub(gutter), app.board.columns.len());
    let hidden_left = app.lane_offset;
    let hidden_right = app.board.columns.len().saturating_sub(app.lane_offset + visible);
    if hidden_left + hidden_right > 0 {
        spans.push(Span::styled(
            format!("   ‹ {hidden_left} · {hidden_right} › columns"),
            Style::default().fg(MUTED),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ----- board ---------------------------------------------------------------

fn visible_lane_count(width: u16, columns: usize) -> usize {
    ((width / MIN_COLUMN_WIDTH) as usize).clamp(1, columns.max(1))
}

fn draw_board(frame: &mut Frame, area: Rect, app: &mut App) {
    if !app.board_loaded {
        draw_loading(frame, area, app);
        return;
    }
    if app.board.columns.is_empty() {
        frame.render_widget(
            Paragraph::new("This board has no columns.").style(Style::default().fg(MUTED)),
            area,
        );
        return;
    }

    let gutter = if app.is_grouped() { GUTTER } else { 0 };
    let columns_area = Rect::new(area.x + gutter, area.y, area.width.saturating_sub(gutter), area.height);
    let count = app.board.columns.len();
    let visible = visible_lane_count(columns_area.width, count);
    if app.selected_column < app.lane_offset {
        app.lane_offset = app.selected_column;
    } else if app.selected_column >= app.lane_offset + visible {
        app.lane_offset = app.selected_column + 1 - visible;
    }
    app.lane_offset = app.lane_offset.min(count - visible);
    app.column_scroll.resize(count, 0);

    if app.mine_only && !app.board.columns.iter().flat_map(|c| &c.issues).any(|i| app.visible(i)) {
        draw_none_of_mine(frame, area, app);
        return;
    }

    let slots = Layout::horizontal(vec![Constraint::Ratio(1, visible as u32); visible]).split(columns_area);
    let columns: Vec<usize> = (app.lane_offset..app.lane_offset + visible).collect();
    if app.is_grouped() {
        draw_grouped(frame, area, &columns, &slots, app);
        return;
    }
    for (slot, &column_index) in columns.iter().enumerate() {
        draw_column(frame, slots[slot], app, column_index);
    }
}

/// "Only mine" with nothing of mine on the board: say so, and how to get back.
fn draw_none_of_mine(frame: &mut Frame, area: Rect, app: &mut App) {
    let who = app.me.as_ref().map(|(_, name)| format!(" ({name})")).unwrap_or_default();
    let label = "  a  show everyone's cards  ";
    let lines = vec![
        Line::styled(
            format!("No cards on this board are assigned to you{who}."),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Line::styled("The board is filtered to only your cards.", Style::default().fg(MUTED)),
        Line::raw(""),
        Line::from(Span::styled(label, Style::default().fg(BACKGROUND).bg(ACCENT).add_modifier(Modifier::BOLD))),
    ];
    let y = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect::new(area.x, y, area.width, 4.min(area.height)),
    );
    let width = label.width() as u16;
    let button = Rect::new(area.x + area.width.saturating_sub(width) / 2, y + 3, width, 1);
    app.hits.push(Hit { area: button, target: HitTarget::Key(KeyEvent::from(KeyCode::Char('a'))) });
}

/// The single loading indicator: a spinner (or the failure) centered on the board.
fn draw_loading(frame: &mut Frame, area: Rect, app: &App) {
    let lines = match &app.load_error {
        None => vec![Line::from(vec![
            Span::styled(format!("{} ", app.spinner()), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("Loading board {}…", app.board_id), Style::default().fg(TEXT)),
        ])],
        Some(error) => {
            let mut lines = vec![
                Line::styled(format!("Couldn't load board {}", app.board_id), Style::default().fg(DANGER).add_modifier(Modifier::BOLD)),
                Line::raw(""),
            ];
            let width = area.width.saturating_sub(8).max(20) as usize;
            lines.extend(wrap(error, width).into_iter().map(|l| Line::styled(l, Style::default().fg(MUTED))));
            lines.push(Line::raw(""));
            lines.push(Line::styled("press r to retry · q to quit", Style::default().fg(TEXT)));
            lines
        }
    };
    let height = (lines.len() as u16).min(area.height);
    let y = area.y + area.height.saturating_sub(height) / 2;
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect::new(area.x, y, area.width, height),
    );
}

/// How a column looks during MOVE mode.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MoveRole {
    NotMoving,
    Here,
    Reachable,
    Unreachable,
    Loading,
}

fn move_role(app: &App, column: usize) -> MoveRole {
    let Mode::Moving(state) = &app.mode else { return MoveRole::NotMoving };
    let Some(targets) = &state.targets else {
        return if column == app.selected_column { MoveRole::Loading } else { MoveRole::NotMoving };
    };
    if column == app.selected_column {
        MoveRole::Here
    } else if column == state.pending.origin_column || targets.iter().any(|t| t.column == column) {
        MoveRole::Reachable
    } else {
        MoveRole::Unreachable
    }
}

/// A column's bordered box and title; returns the inner area for cards.
///
/// `active` is false for lanes the cursor isn't in, so selection and
/// move-mode colors only show where they apply. `section_count` labels a
/// lane's box with that lane's count. WIP limits always judge the whole
/// column; when broken, the title row between the corners fills red (over
/// max) or amber (under min) and the border takes the same color, as on the
/// web board — unless the cursor or a move is coloring the column.
fn column_frame(buf: &mut Buffer, area: Rect, app: &App, ci: usize, active: bool, section_count: Option<usize>) -> Rect {
    let role = if active { move_role(app, ci) } else { MoveRole::NotMoving };
    let selected_col = active && ci == app.selected_column;
    let column = &app.board.columns[ci];

    let (count, load) = column.load(app.board.constraint);
    let fill = match load {
        Load::OverMax => Some((Color::Rgb(255, 255, 255), DANGER)),
        Load::UnderMin => Some((BACKGROUND, CHANGE)),
        Load::Ok => None,
    };

    // The cursor's column and move-mode roles win; otherwise a broken WIP
    // limit colors the whole border.
    let border_color = match role {
        MoveRole::Here | MoveRole::Loading => ACCENT,
        MoveRole::Reachable => REACHABLE,
        MoveRole::Unreachable => DIM,
        MoveRole::NotMoving if selected_col => ACCENT,
        MoveRole::NotMoving => fill.map(|(_, bg)| bg).unwrap_or(BORDER),
    };
    // Titles stay readable even when the border is the quiet default gray.
    let title_color = match role {
        MoveRole::Unreachable => DIM,
        MoveRole::NotMoving if border_color == BORDER => SUBTLE,
        _ => border_color,
    };

    let limits = match (column.max, load) {
        (Some(max), _) => format!("{count}/{max}"),
        _ => count.to_string(),
    };
    let mut label = match section_count {
        // In a lane, show the lane's count; add the column's if a limit is broken.
        Some(n) if load != Load::Ok => format!(" {} ({n} · {limits}", column.name),
        Some(n) => format!(" {} ({n}", column.name),
        None => format!(" {} ({limits}", column.name),
    };
    if load == Load::UnderMin {
        label.push_str(&format!(", min {}", column.min.unwrap_or(0)));
    }
    label.push_str(") ");

    let mut extra = String::new();
    if role == MoveRole::Loading {
        extra.push_str("loading workflow… ");
    }
    if role == MoveRole::Here {
        match app.chosen_transition() {
            Some((t, n)) if n > 1 => extra.push_str(&format!("→ {} [tab: {} options] ", t.name, n)),
            Some((t, _)) => extra.push_str(&format!("→ {} ", t.name)),
            None => extra.push_str("· origin "),
        }
    }

    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    block.render(area, buf);

    // The title spans the top border between the corners; a broken limit
    // fills that span (the corner glyphs keep the plain background).
    let title_row = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1);
    match fill {
        Some((fg, bg)) => {
            Clear.render(title_row, buf);
            let style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);
            Paragraph::new(Line::from(vec![Span::styled(label, style), Span::styled(extra, style)]))
                .style(Style::default().bg(bg))
                .render(title_row, buf);
        }
        None => {
            let style = Style::default().fg(title_color).add_modifier(Modifier::BOLD);
            Paragraph::new(Line::from(vec![Span::styled(label, style), Span::styled(extra, style)])).render(title_row, buf);
        }
    }
    inner
}

fn draw_column(frame: &mut Frame, area: Rect, app: &mut App, ci: usize) {
    // Filtered to my cards, the title counts what's shown.
    let visible = app.visible_rows(ci);
    let shown = app.mine_only.then_some(visible.len());
    let inner = column_frame(frame.buffer_mut(), area, app, ci, true, shown);
    if inner.height == 0 || inner.width < 4 {
        return;
    }
    let role = move_role(app, ci);
    let selected_col = ci == app.selected_column;
    let column = &app.board.columns[ci];

    if visible.is_empty() {
        let text = if column.issues.is_empty() { "  (empty)" } else { "  (none of yours)" };
        frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), inner);
        return;
    }

    let moving = matches!(app.mode, Mode::Moving(_));
    let dim = role == MoveRole::Unreachable;
    let cards: Vec<Vec<Line<'static>>> = visible
        .iter()
        .map(|&ri| {
            let selected = selected_col && ri == app.selected_row;
            card_lines(&column.issues[ri], inner.width, selected, selected && moving, dim, true, true, &app.user_colors)
        })
        .collect();

    // Keep the selected card visible, reserving rows for the ↑/↓ hints.
    // Positions below are into `visible`/`cards`, not the column.
    let len = cards.len();
    let height = inner.height as usize;
    let mut offset = app.column_scroll[ci].min(len - 1);
    if selected_col {
        let sel = visible.iter().position(|&r| r == app.selected_row).unwrap_or(0);
        offset = offset.min(sel);
        while offset < sel {
            let needed: usize = cards[offset..=sel].iter().map(Vec::len).sum::<usize>()
                + usize::from(offset > 0)
                + usize::from(sel + 1 < len);
            if needed <= height {
                break;
            }
            offset += 1;
        }
    }
    app.column_scroll[ci] = offset;

    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    if offset > 0 {
        render_line(frame, inner, y, Line::styled(format!("  ↑ {offset} more"), Style::default().fg(MUTED)));
        y += 1;
    }
    for (i, card) in cards.iter().enumerate().skip(offset) {
        let reserve = u16::from(i + 1 < len);
        let card_height = card.len() as u16;
        if y + card_height + reserve > bottom && i > offset {
            render_line(
                frame,
                inner,
                bottom - 1,
                Line::styled(format!("  ↓ {} more", len - i), Style::default().fg(MUTED)),
            );
            break;
        }
        let rows = card_height.min(bottom.saturating_sub(y));
        let card_area = Rect::new(inner.x, y, inner.width, rows);
        frame.render_widget(Paragraph::new(card.clone()), card_area);
        app.hits.push(Hit { area: card_area, target: HitTarget::Card { column: ci, row: visible[i] } });
        y += rows;
        if y >= bottom {
            break;
        }
    }
}

fn render_line(frame: &mut Frame, area: Rect, y: u16, line: Line<'static>) {
    frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
}

/// A card: key (+ sync marker), wrapped summary, then points/assignee, then a gap.
/// `show_epic` / `show_assignee` are off when the board is grouped by that
/// attribute, since the lane header already says it.
fn card_lines(
    issue: &Issue,
    width: u16,
    selected: bool,
    moving: bool,
    dim: bool,
    show_epic: bool,
    show_assignee: bool,
    people: &UserColors,
) -> Vec<Line<'static>> {
    // A moving card is outlined: "│ " + content + " │".
    let text_width = width.saturating_sub(if moving { 4 } else { 2 }).max(1) as usize;
    let highlighted = selected || moving;
    let base = if dim { DIM } else { TEXT };
    let key_style = if highlighted {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(if dim { DIM } else { SUBTLE })
    };
    let summary_style = if highlighted {
        Style::default().fg(base).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(base)
    };

    // The card's content, before it is framed.
    let mut key = vec![Span::styled(issue.key.clone(), key_style)];
    match issue.sync_state {
        SyncState::Pending => key.push(Span::styled(" ⟳", Style::default().fg(CHANGE))),
        SyncState::Error => key.push(Span::styled(" ✗", Style::default().fg(DANGER))),
        SyncState::Synced => {}
    }

    let mut rows: Vec<Vec<Span<'static>>> = wrap(&issue.summary, text_width)
        .into_iter()
        .map(|chunk| vec![Span::styled(chunk, summary_style)])
        .collect();

    if let (true, Some(epic)) = (show_epic, &issue.epic) {
        // The epic's name in its Jira color.
        let name = truncate(&epic.name, text_width);
        let color = if dim { DIM } else { epic_color(epic) };
        rows.push(vec![Span::styled(name, Style::default().fg(color).add_modifier(Modifier::BOLD))]);
    }

    // Points (or "?" when unestimated), then who it's on.
    let points = issue.story_points.map(format_points).unwrap_or_else(|| "?".into());
    let points_text = format!("{points} pts ");
    let used = points_text.width();
    let mut meta = vec![Span::styled(points_text, Style::default().fg(if dim { DIM } else { SUBTLE }))];
    if show_assignee {
        // The pill's rounded caps take one column on each side.
        let room = text_width.saturating_sub(used + 2).max(1);
        match (&issue.assignee, dim) {
            (Some(name), false) => meta.extend(user_pill(&truncate(name, room), people.color(name))),
            (None, false) => meta.extend(unassigned_pill()),
            (name, true) => meta.push(Span::styled(
                format!(" {} ", truncate(name.as_deref().unwrap_or("Unassigned"), room)),
                Style::default().fg(DIM),
            )),
        }
    }
    rows.push(meta);

    if moving {
        outlined(key, rows, width as usize)
    } else {
        // The selected card is marked by a ▊ bar down its left edge.
        let bar = || Span::styled(if selected { "▊ " } else { "  " }, Style::default().fg(ACCENT));
        let mut lines = vec![Line::from([vec![bar()], key].concat())];
        lines.extend(rows.into_iter().map(|row| Line::from([vec![bar()], row].concat())));
        lines.push(Line::raw(""));
        lines
    }
}

/// Frame a card in an accent outline: the key sits in the top edge, the
/// content rows get side borders, and the bottom edge takes the place of the
/// card's usual spacer line so its height doesn't change.
fn outlined(key: Vec<Span<'static>>, rows: Vec<Vec<Span<'static>>>, width: usize) -> Vec<Line<'static>> {
    let border = Style::default().fg(ACCENT);
    let span_width = |spans: &[Span]| spans.iter().map(|s| s.content.width()).sum::<usize>();
    let width = width.max(6);

    let mut top = vec![Span::styled("┌ ", border)];
    let fill = width.saturating_sub(4 + span_width(&key));
    top.extend(key);
    top.push(Span::styled(format!(" {}┐", "─".repeat(fill)), border));
    let mut lines = vec![Line::from(top)];

    for row in rows {
        let pad = width.saturating_sub(4 + span_width(&row));
        let mut line = vec![Span::styled("│ ", border)];
        line.extend(row);
        line.push(Span::raw(" ".repeat(pad)));
        line.push(Span::styled(" │", border));
        lines.push(Line::from(line));
    }

    lines.push(Line::styled(format!("└{}┘", "─".repeat(width.saturating_sub(2))), border));
    lines
}

fn format_points(points: f64) -> String {
    if points.fract() == 0.0 { format!("{}", points as i64) } else { points.to_string() }
}

// ----- swimlanes -----------------------------------------------------------

/// One lane laid out on the grouped board's canvas.
struct Section {
    y: usize,
    folded: bool,
    /// Tallest cell, in lines (at least 1).
    body: usize,
    /// Per visible column: `(row, card lines)`.
    cards: Vec<Vec<(usize, Vec<Line<'static>>)>>,
}

/// The grouped board: per lane, a header row (fold icon + badge) and then,
/// unless folded, a complete board of its own — every column with its title
/// row and closing bottom border, holding only that lane's cards.
///
/// Sections can be partly scrolled off-screen, so everything is drawn into
/// an offscreen buffer as tall as all of them and the visible window copied.
fn draw_grouped(frame: &mut Frame, area: Rect, columns: &[usize], slots: &[Rect], app: &mut App) {
    let lanes = app.lanes();
    let focused_lane = app.focused_lane_key();
    let in_gutter = matches!(app.focus, LaneFocus::Header(_));
    let moving = matches!(app.mode, Mode::Moving(_));
    let show_epic = app.grouping != Grouping::Epic;
    let show_assignee = app.grouping != Grouping::Assignee;

    // Measure.
    let mut sections = Vec::new();
    let mut y = 0;
    for lane in &lanes {
        let folded = app.is_folded(lane);
        let active = focused_lane.as_deref() == Some(lane.key.as_str());
        let mut cards = Vec::new();
        let mut body = 0;
        if !folded {
            for (slot, &ci) in columns.iter().enumerate() {
                let dim = active && move_role(app, ci) == MoveRole::Unreachable;
                let width = slots[slot].width.saturating_sub(2);
                let cell: Vec<(usize, Vec<Line<'static>>)> = lane.cells[ci]
                    .iter()
                    .map(|&ri| {
                        let issue = &app.board.columns[ci].issues[ri];
                        let selected = app.focus == LaneFocus::Card && ci == app.selected_column && ri == app.selected_row;
                        (ri, card_lines(issue, width, selected, selected && moving, dim, show_epic, show_assignee, &app.user_colors))
                    })
                    .collect();
                body = body.max(cell.iter().map(|(_, lines)| lines.len()).sum());
                cards.push(cell);
            }
            body = body.max(1);
        }
        sections.push(Section { y, folded, body, cards });
        y += 1 + if folded { 0 } else { body + 2 };
    }
    let total = y.max(1);

    // Draw offscreen, collecting hit regions and the cursor's line range.
    let mut canvas = Buffer::empty(Rect::new(area.x, 0, area.width, total as u16));
    let mut hits: Vec<(Rect, HitTarget)> = Vec::new();
    let mut cursor = (0, 1);
    // The section the cursor is in: (index into `lanes`, its header's line).
    let mut anchor: Option<(usize, usize)> = None;
    for (li, (lane, section)) in lanes.iter().zip(&sections).enumerate() {
        let header_focused = app.focus == LaneFocus::Header(lane.key.clone());
        let header = Rect::new(area.x, section.y as u16, area.width, 1);
        let marker = header_focused && app.in_gutter;
        let base = if header_focused { Style::default().bg(HEADER_FOCUS) } else { Style::default() };
        Paragraph::new(lane_header(lane, section.folded, header_focused, marker, &app.user_colors))
            .style(base)
            .render(header, &mut canvas);
        hits.push((header, HitTarget::LaneHeader(lane.key.clone())));
        if header_focused {
            cursor = (section.y, section.y + 1);
            anchor = Some((li, section.y));
        }
        if section.folded {
            continue;
        }
        let active = !in_gutter && focused_lane.as_deref() == Some(lane.key.as_str());
        for (slot, &ci) in columns.iter().enumerate() {
            let boxed = Rect::new(slots[slot].x, (section.y + 1) as u16, slots[slot].width, (section.body + 2) as u16);
            let inner = column_frame(&mut canvas, boxed, app, ci, active, Some(lane.cells[ci].len()));
            let mut cy = inner.y;
            for (ri, lines) in &section.cards[slot] {
                let card = Rect::new(inner.x, cy, inner.width, lines.len() as u16);
                Paragraph::new(lines.clone()).render(card, &mut canvas);
                hits.push((card, HitTarget::Card { column: ci, row: *ri }));
                if app.focus == LaneFocus::Card && ci == app.selected_column && *ri == app.selected_row {
                    cursor = (cy as usize, cy as usize + lines.len());
                    anchor = Some((li, section.y));
                }
                cy += lines.len() as u16;
            }
            if app.focus == LaneFocus::Empty(lane.key.clone()) && ci == app.selected_column {
                Paragraph::new(Line::from(vec![
                    Span::styled("▊ ", Style::default().fg(ACCENT)),
                    Span::styled("no cards", Style::default().fg(MUTED)),
                ]))
                .render(Rect::new(inner.x, inner.y, inner.width, 1), &mut canvas);
                cursor = (inner.y as usize, inner.y as usize + 1);
                anchor = Some((li, section.y));
            }
        }
    }

    // Pin the cursor's section header to the top. If the cursor is further
    // down than the screen allows, scroll to it and keep the header stuck on
    // the top row instead.
    let height = area.height as usize;
    let (scroll, sticky) = match anchor {
        Some((li, header_y)) if cursor.1 > header_y + height => (cursor.1.saturating_sub(height), Some(li)),
        Some((_, header_y)) => (header_y, None),
        None => {
            let mut scroll = app.board_scroll.min(total.saturating_sub(height));
            if cursor.0 < scroll {
                scroll = cursor.0;
            } else if cursor.1 > scroll + height {
                scroll = cursor.1 - height;
            }
            (scroll, None)
        }
    };
    app.board_scroll = scroll;

    let screen = frame.buffer_mut();
    for row in 0..height.min(total - scroll) {
        for x in area.x..area.x + area.width {
            let src = canvas.cell((x, (scroll + row) as u16)).cloned();
            if let (Some(src), Some(dst)) = (src, screen.cell_mut((x, area.y + row as u16))) {
                *dst = src;
            }
        }
    }
    for (rect, target) in hits {
        let (top, bottom) = (rect.y as usize, rect.y as usize + rect.height as usize);
        let (start, end) = (top.max(scroll), bottom.min(scroll + height));
        if start < end {
            app.hits.push(Hit {
                area: Rect::new(rect.x, area.y + (start - scroll) as u16, rect.width, (end - start) as u16),
                target,
            });
        }
    }

    // The sticky header covers the top row (and wins its clicks: hits are
    // searched last-first).
    if let Some(li) = sticky {
        let lane = &lanes[li];
        let row = Rect::new(area.x, area.y, area.width, 1);
        Clear.render(row, screen);
        Paragraph::new(lane_header(lane, false, false, false, &app.user_colors))
            .style(Style::default().bg(BACKGROUND))
            .render(row, screen);
        app.hits.push(Hit { area: row, target: HitTarget::LaneHeader(lane.key.clone()) });
    }
}

/// `▊ ▼ [badge]  N issues` — the lane's fold row. A focused header gets a
/// highlighted row (drawn by the caller) and accent text; `▊` additionally
/// marks the cursor sitting in the gutter.
fn lane_header(lane: &Lane, folded: bool, focused: bool, gutter: bool, people: &UserColors) -> Line<'static> {
    let mut spans = vec![
        Span::styled(if gutter { "▊" } else { " " }, Style::default().fg(ACCENT)),
        Span::styled(if folded { " ▶ " } else { " ▼ " }, Style::default().fg(if focused { ACCENT } else { TEXT })),
    ];
    let plain = |text: &str| Span::styled(text.to_string(), Style::default().fg(TEXT).add_modifier(Modifier::BOLD));
    match &lane.kind {
        LaneKind::Epic(epic) => spans.push(Span::styled(
            format!("{}: {}", epic.key, epic.name),
            Style::default().fg(epic_color(epic)).add_modifier(Modifier::BOLD),
        )),
        LaneKind::Assignee(name) => spans.extend(user_pill(name, people.color(name))),
        LaneKind::Unassigned => spans.extend(unassigned_pill()),
        LaneKind::NoEpic => spans.push(plain("No epic")),
        LaneKind::All => {}
    }
    let count = lane.count();
    let noun = if count == 1 { "issue" } else { "issues" };
    spans.push(Span::styled(
        format!("  {count} {noun}"),
        Style::default().fg(if focused { ACCENT } else { MUTED }).add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

// ----- status + footer -----------------------------------------------------

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    if app.refreshing {
        let line = Line::from(vec![
            Span::styled(format!(" {} ", app.spinner()), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("refreshing board…", Style::default().fg(TEXT)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let Some((message, level)) = &app.status else { return };
    let color = match level {
        StatusLevel::Info => TEXT,
        StatusLevel::Error => DANGER,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {message}"), Style::default().fg(color))),
        area,
    );
}

/// A footer hint; `key` makes it a clickable button (navigation hints aren't).
struct Hint {
    label: &'static str,
    key: Option<KeyEvent>,
}

fn hint(label: &'static str, code: KeyCode) -> Hint {
    Hint { label, key: Some(KeyEvent::from(code)) }
}

fn hint_with(label: &'static str, code: KeyCode, modifiers: KeyModifiers) -> Hint {
    Hint { label, key: Some(KeyEvent::new(code, modifiers)) }
}

fn nav(label: &'static str) -> Hint {
    Hint { label, key: None }
}

fn footer_hints(app: &App) -> (&'static str, Vec<Hint>) {
    use KeyCode::{Char, Enter, Esc, Tab};
    match &app.mode {
        // Everything up to `g group` is identical grouped or not, so the `g`
        // button never shifts under the mouse when it's clicked.
        Mode::Board => {
            let mut hints = vec![
                nav("hjkl/arrows navigate"),
                hint("enter details", Enter),
                hint("m move", Char('m')),
                // Same width either way, so `g group` stays put.
                hint(if app.mine_only { "a show all " } else { "a only mine" }, Char('a')),
                hint("g group", Char('g')),
            ];
            if app.is_grouped() {
                hints.push(hint("z fold", Char('z')));
                hints.push(hint("Z fold all", Char('Z')));
            }
            hints.extend([hint("r refresh", Char('r')), hint("? keymap", Char('?')), hint("q quit", Char('q'))]);
            ("BOARD", hints)
        }
        Mode::Help { .. } => ("HELP", vec![hint("esc close", Esc)]),
        Mode::Details { .. } => (
            "DETAILS",
            vec![
                nav("j/k scroll"),
                hint("e edit description", Char('e')),
                hint("a edit acceptance criteria", Char('a')),
                hint("m move", Char('m')),
                hint("esc close", Esc),
                hint("? keymap", Char('?')),
            ],
        ),
        Mode::Create(_) => (
            "CREATE",
            vec![
                hint("tab next field", Tab),
                hint_with("^s submit", Char('s'), KeyModifiers::CONTROL),
                hint_with("^g $EDITOR", Char('g'), KeyModifiers::CONTROL),
                hint("esc cancel", Esc),
            ],
        ),
        Mode::EditRich(_) => (
            "EDIT",
            vec![
                hint_with("^s save", Char('s'), KeyModifiers::CONTROL),
                hint_with("^g $EDITOR", Char('g'), KeyModifiers::CONTROL),
                hint("esc cancel", Esc),
            ],
        ),
        Mode::Moving(_) => (
            "MOVE",
            vec![
                nav("h/l reachable columns"),
                hint("tab other transition", Tab),
                hint("enter confirm", Enter),
                hint("esc cancel", Esc),
            ],
        ),
        Mode::TransitionForm(form, _) if form.editor.is_some() => (
            "EDIT",
            vec![
                hint("enter save", Enter),
                hint_with("alt-enter newline", Enter, KeyModifiers::ALT),
                hint("esc discard changes", Esc),
            ],
        ),
        Mode::TransitionForm(form, _) if form.picker.is_some() => (
            "PICK",
            vec![
                nav("j/k select"),
                hint("space toggle (multi)", Char(' ')),
                hint("enter choose", Enter),
                hint("esc back", Esc),
            ],
        ),
        Mode::TransitionForm(..) => (
            "FORM",
            vec![
                nav("j/k field"),
                hint("enter edit", Enter),
                hint_with("^s submit", Char('s'), KeyModifiers::CONTROL),
                hint("esc cancel move", Esc),
                hint("? keymap", Char('?')),
            ],
        ),
    }
}

/// Mode hints as buttons: hovering one gives it a background, clicking it
/// presses its key.
fn draw_footer(frame: &mut Frame, area: Rect, app: &mut App) {
    let (mode, hints) = footer_hints(app);
    let mut spans = vec![Span::styled(format!(" {mode} "), Style::default().fg(MUTED).add_modifier(Modifier::BOLD))];
    let mut x = area.x + spans[0].content.width() as u16;
    for hint in hints {
        let text = format!(" {} ", hint.label);
        let width = text.width() as u16;
        if x + width > area.x + area.width {
            break;
        }
        let button = Rect::new(x, area.y, width, 1);
        let hovered = hint.key.is_some()
            && app.mouse.is_some_and(|(mx, my)| my == area.y && mx >= x && mx < x + width);
        let style = if hovered {
            Style::default().fg(TEXT).bg(HOVER).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        };
        spans.push(Span::styled(text, style));
        if let Some(key) = hint.key {
            app.hits.push(Hit { area: button, target: HitTarget::Key(key) });
        }
        x += width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ----- keymap --------------------------------------------------------------

fn draw_help(frame: &mut Frame) {
    let area = centered(frame.area(), 90, 24);
    let heading = |text: &'static str| Line::styled(text, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
    let key = |keys: &'static str, action: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {keys:<14}"), Style::default().fg(TEXT)),
            Span::styled(action, Style::default().fg(MUTED)),
        ])
    };
    let left = vec![
        heading("GLOBAL"),
        key("?", "open / close this keymap"),
        key("q", "close view / quit"),
        key("Ctrl-C", "quit jdo immediately"),
        Line::raw(""),
        heading("BOARD"),
        key("arrows / hjkl", "navigate columns and cards"),
        key("Home / End", "first / last card (or header)"),
        key("1-9", "jump to a column"),
        key("Enter", "open card details"),
        key("m", "enter MOVE mode for the card"),
        key("c", "create a new card"),
        key("a", "only my cards (toggle)"),
        key("g", "group: none → epic → assignee"),
        key("h at column 1", "gutter: j/k hop lane headers"),
        key("z / Enter", "fold / unfold a lane"),
        key("Z", "fold / unfold every lane"),
        key("r", "refresh the board from Jira"),
        Line::raw(""),
        heading("DETAILS"),
        key("j / k", "scroll"),
        key("^d / ^u", "scroll half a page"),
        key("g / G", "top / bottom"),
        key("e / a", "edit description / acceptance"),
        key("m", "move this card"),
        key("Esc / q", "close details"),
    ];
    let right = vec![
        heading("MOVE MODE"),
        key("h / l", "prev / next reachable column"),
        key("Tab / j / k", "other transition into column"),
        key("Enter / m", "confirm the move"),
        key("Esc", "cancel; card goes back"),
        Line::styled("  Reachable columns are lit green.", Style::default().fg(MUTED)),
        Line::raw(""),
        heading("TRANSITION FORM"),
        key("j / k / Tab", "select a field"),
        key("Enter / e", "edit field / open options"),
        key("Ctrl-S", "submit the move"),
        key("Esc", "cancel the move"),
        Line::raw(""),
        heading("EDITING DESCRIPTION / AC"),
        key("Ctrl-S", "save to Jira (Markdown)"),
        key("Ctrl-G", "edit in $EDITOR"),
        Line::raw(""),
        heading("EDITING A FIELD"),
        key("Enter", "save the field"),
        key("Alt-Enter", "new line (rich text)"),
        key("Esc", "discard changes"),
        key("Space", "toggle option (multi)"),
    ];

    frame.render_widget(Clear, area);
    let block = modal_block(" jdo · Jira board · keymap ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content, hint] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(content);
    frame.render_widget(Paragraph::new(left), columns[0]);
    frame.render_widget(Paragraph::new(right), columns[1]);
    frame.render_widget(
        Paragraph::new("Esc/q/? close · Ctrl-C quit").alignment(Alignment::Center).style(Style::default().fg(MUTED)),
        hint,
    );
}

// ----- details -------------------------------------------------------------

fn draw_details(frame: &mut Frame, app: &mut App) {
    let Mode::Details { key, .. } = &app.mode else { return };
    let area = centered(frame.area(), 92, frame.area().height.saturating_mul(9) / 10);
    let width = area.width.saturating_sub(4).max(10) as usize;

    let lines = match app.board.issue(key) {
        Some(issue) => detail_lines(issue, app.comments.get(key), app.rich.get(key), width, &app.user_colors),
        None => vec![Line::styled(format!("{key} is no longer on this board."), Style::default().fg(MUTED))],
    };
    let title = format!(" {key} · details ");

    frame.render_widget(Clear, area);
    let block = modal_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content, hint] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let content = content.inner(Margin::new(1, 0));

    // A clickable [×] in the top-right corner of the border.
    if area.width > 8 {
        let close = Rect::new(area.x + area.width - 5, area.y, 3, 1);
        let hovered = app.mouse.is_some_and(|(mx, my)| my == close.y && mx >= close.x && mx < close.x + close.width);
        let style = if hovered {
            Style::default().fg(TEXT).bg(DANGER).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ACCENT).bg(BACKGROUND).add_modifier(Modifier::BOLD)
        };
        frame.render_widget(Paragraph::new(Span::styled("[×]", style)), close);
        app.hits.push(Hit { area: close, target: HitTarget::CloseDetails });
    }

    let max_scroll = (lines.len() as u16).saturating_sub(content.height);
    let Mode::Details { scroll, .. } = &mut app.mode else { return };
    *scroll = (*scroll).min(max_scroll);
    let position = if max_scroll == 0 {
        String::new()
    } else {
        format!(" · {}%", (*scroll as u32 * 100 / max_scroll as u32))
    };

    frame.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), content);
    frame.render_widget(
        Paragraph::new(format!("j/k or wheel scroll · m move · Esc or [×] close{position}"))
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        hint,
    );
}

fn detail_lines(
    issue: &Issue,
    comments: Option<&CommentsState>,
    rich: Option<&RichState>,
    width: usize,
    people: &UserColors,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let heading = |text: String| Line::styled(text, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));

    for chunk in wrap(&issue.summary, width) {
        lines.push(Line::styled(chunk, Style::default().fg(TEXT).add_modifier(Modifier::BOLD)));
    }
    lines.push(Line::raw(""));

    let dash = || "—".to_string();
    let fields: Vec<(&str, String)> = vec![
        ("Type", issue.issue_type.clone().unwrap_or_else(dash)),
        ("Status", issue.status.clone()),
        ("Assignee", issue.assignee.clone().unwrap_or_else(|| "Unassigned".into())),
        ("Priority", issue.priority.clone().unwrap_or_else(dash)),
        ("Story Points", issue.story_points.map(format_points).unwrap_or_else(dash)),
        ("Sprint", issue.sprint.clone().unwrap_or_else(dash)),
        ("Epic", issue.epic.as_ref().map(|e| e.key.clone()).unwrap_or_else(dash)),
        ("Labels", if issue.labels.is_empty() { dash() } else { issue.labels.join(", ") }),
        ("URL", issue.url.clone()),
    ];
    let label_width = fields.iter().map(|(l, _)| l.len()).max().unwrap_or(0) + 2;
    for (label, value) in fields {
        if label == "Assignee" {
            let mut spans = vec![Span::styled(format!("{label:<label_width$}"), Style::default().fg(MUTED))];
            match &issue.assignee {
                Some(name) => spans.extend(user_pill(name, people.color(name))),
                None => spans.extend(unassigned_pill()),
            }
            lines.push(Line::from(spans));
            continue;
        }
        if let ("Epic", Some(epic)) = (label, &issue.epic) {
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<label_width$}"), Style::default().fg(MUTED)),
                epic_pill(epic, &epic.name),
                Span::styled(format!("  {}", epic.key), Style::default().fg(MUTED)),
            ]));
            continue;
        }
        let value_width = width.saturating_sub(label_width).max(10);
        for (i, chunk) in wrap(&value, value_width).into_iter().enumerate() {
            let label_text = if i == 0 { format!("{label:<label_width$}") } else { " ".repeat(label_width) };
            lines.push(Line::from(vec![
                Span::styled(label_text, Style::default().fg(MUTED)),
                Span::styled(chunk, Style::default().fg(TEXT)),
            ]));
        }
    }

    // Prefer the rich document (fetched on open) over the board API's plain text.
    for (field, fallback, key) in [
        (RichField::Description, &issue.description, "e"),
        (RichField::AcceptanceCriteria, &issue.acceptance_criteria, "a"),
    ] {
        let body = match rich.and_then(|r| r.get(field)) {
            Some(value) => adf::to_text(Some(value)),
            None => fallback.clone(),
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(field.label().to_string(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {key} to edit"), Style::default().fg(MUTED)),
        ]));
        push_body(&mut lines, &body, width, 0);
    }

    lines.push(Line::raw(""));
    match comments {
        None | Some(CommentsState::Loading) => {
            lines.push(heading("Comments".into()));
            lines.push(Line::styled("loading…", Style::default().fg(MUTED)));
        }
        Some(CommentsState::Failed(message)) => {
            lines.push(heading("Comments".into()));
            lines.push(Line::styled(format!("couldn't load comments: {message}"), Style::default().fg(DANGER)));
        }
        Some(CommentsState::Loaded(comments)) => {
            lines.push(heading(format!("Comments ({})", comments.len())));
            if comments.is_empty() {
                lines.push(Line::styled("—", Style::default().fg(MUTED)));
            }
            for comment in comments {
                lines.push(Line::raw(""));
                let mut header = user_pill(&comment.author, people.color(&comment.author));
                header.push(Span::styled(format!(" · {}", comment.created), Style::default().fg(MUTED)));
                lines.push(Line::from(header));
                push_body(&mut lines, &comment.body, width, 2);
            }
        }
    }
    lines
}

fn push_body(lines: &mut Vec<Line<'static>>, body: &str, width: usize, indent: usize) {
    if body.trim().is_empty() {
        lines.push(Line::styled(format!("{}—", " ".repeat(indent)), Style::default().fg(MUTED)));
        return;
    }
    for paragraph in body.split('\n') {
        if paragraph.trim().is_empty() {
            lines.push(Line::raw(""));
            continue;
        }
        for chunk in wrap(paragraph, width.saturating_sub(indent).max(10)) {
            lines.push(Line::styled(format!("{}{chunk}", " ".repeat(indent)), Style::default().fg(TEXT)));
        }
    }
}

// ----- transition form -----------------------------------------------------

fn draw_form(frame: &mut Frame, form: &mut TransitionForm) {
    let screen = frame.area();
    let width = (screen.width * 7 / 10).max(60).min(screen.width);
    let value_width = width.saturating_sub(8).max(10) as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_span = (0usize, 0usize);
    lines.push(Line::from(vec![
        Span::styled("Moving to ", Style::default().fg(MUTED)),
        Span::styled(form.to_status.clone(), Style::default().fg(REACHABLE).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" via “{}”", form.transition_name), Style::default().fg(MUTED)),
    ]));
    lines.push(Line::raw(""));

    for (i, field) in form.fields.iter().enumerate() {
        let start = lines.len();
        let selected = i == form.cursor;
        let marker = if selected { "▊ " } else { "  " };
        let mut label = vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(
                field.name.clone(),
                if selected {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
                },
            ),
        ];
        if field.required {
            label.push(Span::styled(" *", Style::default().fg(DANGER)));
        }
        if field.dirty {
            label.push(Span::styled(" (changed)", Style::default().fg(CHANGE)));
        }
        lines.push(Line::from(label));

        let unsupported = matches!(field.kind, FieldKind::Unsupported);
        let value = field.display_value();
        if value.trim().is_empty() {
            lines.push(Line::styled("    —", Style::default().fg(MUTED)));
        } else {
            let style = if unsupported {
                Style::default().fg(MUTED).add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(TEXT)
            };
            let all: Vec<String> = value.split('\n').flat_map(|p| wrap(p, value_width)).collect();
            let shown = if selected { all.len() } else { all.len().min(4) };
            for chunk in &all[..shown] {
                lines.push(Line::styled(format!("    {chunk}"), style));
            }
            if shown < all.len() {
                lines.push(Line::styled(format!("    … {} more lines", all.len() - shown), Style::default().fg(MUTED)));
            }
        }
        if let Some(error) = &field.error {
            for chunk in wrap(&format!("⚠ {error}"), value_width) {
                lines.push(Line::styled(format!("    {chunk}"), Style::default().fg(DANGER)));
            }
        }
        if selected {
            cursor_span = (start, lines.len());
        }
        lines.push(Line::raw(""));
    }

    for error in &form.errors {
        for chunk in wrap(&format!("⚠ {error}"), value_width + 4) {
            lines.push(Line::styled(chunk, Style::default().fg(DANGER)));
        }
    }
    if !form.errors.is_empty() {
        lines.push(Line::raw(""));
    }

    let button = |label: &str, selected: bool| {
        let style = if selected {
            Style::default().fg(BACKGROUND).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        Span::styled(format!("[ {label} ]"), style)
    };
    let buttons_line = lines.len();
    if form.submitting {
        lines.push(Line::styled("submitting to Jira…", Style::default().fg(CHANGE)));
    } else {
        lines.push(Line::from(vec![
            button("Submit", form.cursor == form.submit_index()),
            Span::raw("   "),
            button("Cancel", form.cursor == form.cancel_index()),
        ]));
    }
    if form.cursor >= form.submit_index() {
        cursor_span = (buttons_line, buttons_line + 1);
    }

    let height = (lines.len() as u16 + 3).min(screen.height.saturating_mul(9) / 10);
    let area = centered_fixed(screen, width, height);
    frame.render_widget(Clear, area);
    let title = format!(" {} · {} ", form.key, form.transition_name);
    let block = modal_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content, hint] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let content = content.inner(Margin::new(1, 0));

    // Scroll so the selected field (or the buttons) stays visible.
    let visible = content.height as usize;
    let scroll = if cursor_span.1 > visible { cursor_span.1 - visible } else { 0 }.min(cursor_span.0);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), content);
    frame.render_widget(
        Paragraph::new("Enter edit · ^S submit · Esc cancel move")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        hint,
    );

    if form.editor.is_some() {
        draw_field_editor(frame, form);
    } else if form.picker.is_some() {
        draw_picker(frame, form);
    }
}

fn draw_field_editor(frame: &mut Frame, form: &mut TransitionForm) {
    let field = &form.fields[form.cursor];
    let multiline = field.is_multiline();
    let title = format!(" {} ", field.name);
    let screen = frame.area();
    let width = (screen.width * 6 / 10).max(50).min(screen.width);
    let height = if multiline { 14 } else { 4 };
    let area = centered_fixed(screen, width, height);
    let hint = if multiline {
        "Enter save · Alt-Enter new line · Esc discard"
    } else {
        "Enter save · Esc discard"
    };

    frame.render_widget(Clear, area);
    let block = modal_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content, hint_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let editor = form.editor.as_mut().expect("editor open");
    editor.set_cursor_line_style(Style::default());
    editor.set_style(Style::default().fg(TEXT).bg(BACKGROUND));
    editor.set_cursor_style(Style::default().fg(BACKGROUND).bg(ACCENT));
    frame.render_widget(&*editor, content.inner(Margin::new(1, 0)));
    frame.render_widget(
        Paragraph::new(hint).alignment(Alignment::Center).style(Style::default().fg(MUTED)),
        hint_area,
    );
}

fn draw_picker(frame: &mut Frame, form: &TransitionForm) {
    let picker = form.picker.as_ref().expect("picker open");
    let field = &form.fields[picker.field];
    let (options, multi) = match &field.kind {
        FieldKind::Select { options } => (options, false),
        FieldKind::MultiSelect { options } => (options, true),
        _ => return,
    };
    let screen = frame.area();
    let width = (screen.width / 2).max(40).min(screen.width);
    let height = (options.len() as u16 + 3).min(screen.height * 7 / 10).max(5);
    let area = centered_fixed(screen, width, height);

    let lines: Vec<Line<'static>> = options
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let selected = i == picker.cursor;
            let mark = if multi {
                if picker.checked[i] { "[x] " } else { "[ ] " }
            } else if selected {
                "● "
            } else {
                "○ "
            };
            let style = if selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            };
            Line::from(vec![
                Span::styled(if selected { "▊ " } else { "  " }, Style::default().fg(ACCENT)),
                Span::styled(format!("{mark}{}", option.label), style),
            ])
        })
        .collect();

    frame.render_widget(Clear, area);
    let title = format!(" {} ", field.name);
    let block = modal_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content, hint] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let visible = content.height as usize;
    let scroll = (picker.cursor + 1).saturating_sub(visible);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), content);
    let hint_text = if multi { "Space toggle · Enter done · Esc back" } else { "Enter choose · Esc back" };
    frame.render_widget(
        Paragraph::new(hint_text).alignment(Alignment::Center).style(Style::default().fg(MUTED)),
        hint,
    );
}

/// The Markdown editor for a description or acceptance criteria.
fn draw_rich_editor(frame: &mut Frame, editor: &mut RichEditor) {
    let screen = frame.area();
    let area = centered(screen, 90, screen.height.saturating_mul(85) / 100);
    let title = format!(" {} · edit {} · Markdown ", editor.key, editor.field.label());

    // Notes under the text: placeholders, what saving loses, status.
    let mut notes: Vec<Line<'static>> = Vec::new();
    let kinds = editor.markdown.placeholder_kinds();
    if !kinds.is_empty() {
        notes.push(Line::styled(
            format!("⟦n:…⟧ placeholders keep this field's {} intact — leave them in place to keep them", kinds.join(", ")),
            Style::default().fg(MUTED),
        ));
    }
    if !editor.markdown.dropped.is_empty() {
        notes.push(Line::styled(
            format!("Saving will drop formatting Markdown can't express: {}", editor.markdown.dropped.join(", ")),
            Style::default().fg(CHANGE),
        ));
    }
    if let Some(error) = &editor.error {
        notes.push(Line::styled(format!("⚠ {error}"), Style::default().fg(DANGER)));
    }
    if editor.saving {
        notes.push(Line::styled("saving to Jira…", Style::default().fg(CHANGE)));
    }

    frame.render_widget(Clear, area);
    let block = modal_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [text_area, notes_area, hint] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(notes.len() as u16),
        Constraint::Length(1),
    ])
    .areas(inner);

    let text = &mut editor.editor;
    text.set_cursor_line_style(Style::default());
    text.set_style(Style::default().fg(TEXT).bg(BACKGROUND));
    text.set_cursor_style(Style::default().fg(BACKGROUND).bg(ACCENT));
    frame.render_widget(&*text, text_area.inner(Margin::new(1, 0)));
    frame.render_widget(Paragraph::new(notes).wrap(ratatui::widgets::Wrap { trim: false }), notes_area.inner(Margin::new(1, 0)));
    frame.render_widget(
        Paragraph::new("Ctrl-S save · Ctrl-G $EDITOR · Esc cancel")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        hint,
    );
}

// ----- create dialog ---------------------------------------------------------

/// Width of the label column in the create dialog.
const CREATE_LABEL: u16 = 23;

/// The "Create new <PROJECT> ticket" dialog. Records click targets in `hits`.
fn draw_create(frame: &mut Frame, form: &mut CreateForm, people: &UserColors, hits: &mut Vec<Hit>) {
    let screen = frame.area();
    let width = (screen.width * 8 / 10).clamp(60.min(screen.width), 130.min(screen.width));
    let area = centered_fixed(screen, width, screen.height.saturating_mul(9) / 10);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().fg(TEXT).bg(BACKGROUND));
    let inner = block.inner(area).inner(Margin::new(1, 0));
    frame.render_widget(block, area);

    let err_rows = |field| u16::from(form.field_errors.contains_key(&field));
    let rows = Layout::vertical([
        Constraint::Length(1),                                           // header
        Constraint::Length(1),                                           // rules status
        Constraint::Length(form.errors.len() as u16),                    // general errors
        Constraint::Length(1),                                           // gap
        Constraint::Length(1 + err_rows(CreateField::Team)),
        Constraint::Length(1 + err_rows(CreateField::Sprint)),
        Constraint::Length(1 + err_rows(CreateField::Epic)),
        Constraint::Length(1 + err_rows(CreateField::Assignee)),
        Constraint::Length(1),                                           // gap
        Constraint::Length(3 + err_rows(CreateField::Title)),
        Constraint::Min(4 + err_rows(CreateField::Description)),
        Constraint::Min(4 + err_rows(CreateField::Acceptance)),
        Constraint::Length(1),                                           // gap
        Constraint::Length(1),                                           // buttons
    ])
    .split(inner);

    // Header: title + [×].
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!("Create new {} ticket:", form.project_key),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        rows[0],
    );
    let close = Rect::new(area.x + area.width - 5, area.y, 3, 1);
    frame.render_widget(Paragraph::new(Span::styled("[×]", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))), close);
    hits.push(Hit { area: close, target: HitTarget::Create(CreateHit::Cancel) });

    let status = match (&form.meta, &form.meta_error) {
        (Some(meta), _) => Line::styled(format!("{} · fields marked * are required", meta.issue_type_name), Style::default().fg(MUTED)),
        (None, Some(error)) => Line::styled(format!("⚠ couldn't load Jira's create rules: {error}"), Style::default().fg(DANGER)),
        (None, None) => Line::styled("Story · loading Jira's create rules…", Style::default().fg(MUTED)),
    };
    frame.render_widget(Paragraph::new(status), rows[1]);
    let general: Vec<Line> = form.errors.iter().map(|e| Line::styled(format!("⚠ {e}"), Style::default().fg(DANGER))).collect();
    frame.render_widget(Paragraph::new(general), rows[2]);

    // Label on the left, control on the right, error underneath.
    let split = |row: Rect, field: CreateField, frame: &mut Frame, hits: &mut Vec<Hit>| -> Rect {
        let focused = form.focus == CreateFocus::Field(field);
        let mut label = vec![Span::styled(
            field.label(),
            if focused {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(SUBTLE)
            },
        )];
        if form.required(field) {
            label.push(Span::styled(" *", Style::default().fg(DANGER)));
        }
        let error = form.field_errors.get(&field);
        let control_height = row.height.saturating_sub(u16::from(error.is_some()));
        let label_y = if control_height >= 3 { row.y + 1 } else { row.y };
        frame.render_widget(Paragraph::new(Line::from(label)), Rect::new(row.x, label_y, CREATE_LABEL, 1));
        let control = Rect::new(row.x + CREATE_LABEL, row.y, row.width.saturating_sub(CREATE_LABEL), control_height);
        if let Some(error) = error {
            frame.render_widget(
                Paragraph::new(Span::styled(format!("⚠ {error}"), Style::default().fg(DANGER))),
                Rect::new(control.x, row.y + control_height, control.width, 1),
            );
        }
        hits.push(Hit { area: Rect::new(row.x, row.y, row.width, control_height), target: HitTarget::Create(CreateHit::Focus(field)) });
        control
    };

    // Pickers: one line each; the focused one shows its filter and opens a dropdown.
    let mut open_dropdown: Option<(CreateField, Rect)> = None;
    for (row, field) in [
        (rows[4], CreateField::Team),
        (rows[5], CreateField::Sprint),
        (rows[6], CreateField::Epic),
        (rows[7], CreateField::Assignee),
    ] {
        let control = split(row, field, frame, hits);
        let picker = form.picker(field).expect("picker field");
        let focused = form.focus == CreateFocus::Field(field);
        let mut spans = Vec::new();
        if focused {
            spans.push(Span::styled(picker.filter.clone(), Style::default().fg(TEXT)));
            spans.push(Span::styled(" ", Style::default().bg(ACCENT)));
            if picker.filter.is_empty() {
                spans.push(Span::styled("  type to filter · ", Style::default().fg(MUTED)));
                spans.extend(pick_spans(field, &picker.chosen, people));
            }
            open_dropdown = Some((field, Rect::new(control.x, row.y + row.height, control.width, 0)));
        } else {
            spans.extend(pick_spans(field, &picker.chosen, people));
            spans.push(Span::styled("  ▾", Style::default().fg(MUTED)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), control);
    }

    // Title / Description / Acceptance Criteria: boxed editors. Lay the rows
    // out first; the editors are borrowed mutably to draw them.
    let text_rows: Vec<(Rect, CreateField)> = [(rows[9], CreateField::Title), (rows[10], CreateField::Description), (rows[11], CreateField::Acceptance)]
        .into_iter()
        .map(|(row, field)| (split(row, field, frame, hits), field))
        .collect();
    for (control, field) in text_rows {
        let focused = form.focus == CreateFocus::Field(field);
        let area_widget = match field {
            CreateField::Title => &mut form.title,
            CreateField::Description => &mut form.description,
            _ => &mut form.acceptance,
        };
        let hint = match field {
            CreateField::Title => "",
            _ if focused => " Markdown · ^G $EDITOR ",
            _ => " Markdown ",
        };
        area_widget.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
                .title_bottom(Line::styled(hint, Style::default().fg(MUTED)).right_aligned()),
        );
        area_widget.set_style(Style::default().fg(TEXT).bg(BACKGROUND));
        area_widget.set_cursor_line_style(Style::default());
        area_widget.set_cursor_style(if focused { Style::default().fg(BACKGROUND).bg(ACCENT) } else { Style::default() });
        frame.render_widget(&*area_widget, control);
    }

    // Footer: centered Cancel, then Submit on the right.
    let footer = rows[13];
    if form.submitting {
        frame.render_widget(
            Paragraph::new(Span::styled("creating in Jira…", Style::default().fg(CHANGE))).alignment(Alignment::Center),
            footer,
        );
    } else {
        let cancel = button("Cancel", ButtonKind::Secondary, form.focus == CreateFocus::Cancel);
        let submit = button("Submit", ButtonKind::Primary, form.focus == CreateFocus::Submit);
        let (cancel_w, submit_w, gap) = (cancel.content.width() as u16, submit.content.width() as u16, 3);
        let total = cancel_w + gap + submit_w;
        let x = footer.x + footer.width.saturating_sub(total) / 2;
        let line = Line::from(vec![cancel, Span::raw(" ".repeat(gap as usize)), submit]);
        frame.render_widget(Paragraph::new(line), Rect::new(x, footer.y, total.min(footer.width), 1));
        hits.push(Hit { area: Rect::new(x, footer.y, cancel_w, 1), target: HitTarget::Create(CreateHit::Cancel) });
        hits.push(Hit {
            area: Rect::new(x + cancel_w + gap, footer.y, submit_w, 1),
            target: HitTarget::Create(CreateHit::Submit),
        });
    }

    // The focused picker's dropdown, drawn last so it overlays what's below.
    if let Some((field, anchor)) = open_dropdown {
        let picker = form.picker(field).expect("picker field");
        let options = picker.matches();
        let extra = u16::from(picker.loading);
        let visible = options.len().clamp(1, 8) as u16;
        let drop = Rect::new(anchor.x, anchor.y, anchor.width, visible + extra + 2).intersection(screen);
        frame.render_widget(Clear, drop);
        let start = (picker.highlight + 1).saturating_sub(visible as usize);
        let mut lines: Vec<Line> = options
            .iter()
            .enumerate()
            .skip(start)
            .take(visible as usize)
            .map(|(i, option)| {
                let mut spans = vec![Span::styled(if i == picker.highlight { "▊ " } else { "  " }, Style::default().fg(ACCENT))];
                spans.extend(pick_spans(field, option, people));
                Line::from(spans)
            })
            .collect();
        if lines.is_empty() {
            lines.push(Line::styled("  no matches", Style::default().fg(MUTED)));
        }
        if picker.loading {
            lines.push(Line::styled("  loading the team's members…", Style::default().fg(MUTED)));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(ACCENT)))
                .style(Style::default().bg(BACKGROUND)),
            drop,
        );
        for (row, i) in (start..options.len()).take(visible as usize).enumerate() {
            hits.push(Hit {
                area: Rect::new(drop.x + 1, drop.y + 1 + row as u16, drop.width.saturating_sub(2), 1),
                target: HitTarget::Create(CreateHit::Pick(field, i)),
            });
        }
    }
}

/// How a picker choice reads: epics in their color, people as pills.
fn pick_spans(field: CreateField, option: &PickOption, people: &UserColors) -> Vec<Span<'static>> {
    let mut spans = match (field, &option.key) {
        (CreateField::Epic, key) => epic_line(key, &option.label, option.color).spans,
        (CreateField::Assignee, Some(_)) => user_pill(&option.label, people.color(&option.label)),
        (CreateField::Assignee, None) => vec![Span::styled(option.label.clone(), Style::default().fg(MUTED))],
        (_, Some(_)) => vec![Span::styled(option.label.clone(), Style::default().fg(TEXT))],
        (_, None) => vec![Span::styled(option.label.clone(), Style::default().fg(MUTED))],
    };
    if let Some(note) = &option.note {
        spans.push(Span::styled(format!("  {note}"), Style::default().fg(MUTED)));
    }
    spans
}

#[derive(Clone, Copy)]
enum ButtonKind {
    /// The main action (Submit): accent fill.
    Primary,
    /// Everything else (Cancel): gray fill.
    Secondary,
}

/// A filled, square-edged button; brighter and bold when focused.
fn button(label: &str, kind: ButtonKind, focused: bool) -> Span<'static> {
    let (bg, fg) = match (kind, focused) {
        (ButtonKind::Primary, true) => (ACCENT, BACKGROUND),
        (ButtonKind::Primary, false) => (Color::Rgb(150, 82, 10), Color::Rgb(255, 230, 200)),
        (ButtonKind::Secondary, true) => (Color::Rgb(150, 150, 150), BACKGROUND),
        (ButtonKind::Secondary, false) => (Color::Rgb(70, 70, 70), TEXT),
    };
    let modifier = if focused { Modifier::BOLD } else { Modifier::empty() };
    Span::styled(format!("   {label}   "), Style::default().fg(fg).bg(bg).add_modifier(modifier))
}

/// `KEY: name` in the epic's color, or a muted "No epic".
fn epic_line(key: &Option<String>, name: &str, color: Option<usize>) -> Line<'static> {
    match key {
        Some(key) => {
            let epic = EpicRef { key: key.clone(), name: name.to_string(), color };
            let text = if name.is_empty() { key.clone() } else { format!("{key}: {name}") };
            Line::from(Span::styled(text, Style::default().fg(epic_color(&epic)).add_modifier(Modifier::BOLD)))
        }
        None => Line::styled(name.to_string(), Style::default().fg(MUTED)),
    }
}

// ----- helpers -------------------------------------------------------------

/// Nerd Font powerline caps that round off a pill's ends.
const PILL_LEFT: &str = "\u{e0b6}";
const PILL_RIGHT: &str = "\u{e0b4}";

/// A person's name as a rounded pill in their color (see [`UserColors`]).
fn user_pill(name: &str, bg: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(PILL_LEFT, Style::default().fg(bg)),
        Span::styled(name.to_string(), Style::default().fg(readable_on(bg)).bg(bg).add_modifier(Modifier::BOLD)),
        Span::styled(PILL_RIGHT, Style::default().fg(bg)),
    ]
}

/// The red rounded pill for cards nobody owns.
fn unassigned_pill() -> Vec<Span<'static>> {
    vec![
        Span::styled(PILL_LEFT, Style::default().fg(UNASSIGNED)),
        Span::styled("Unassigned", Style::default().fg(Color::Rgb(255, 255, 255)).bg(UNASSIGNED).add_modifier(Modifier::BOLD)),
        Span::styled(PILL_RIGHT, Style::default().fg(UNASSIGNED)),
    ]
}

/// An epic as a square-ended badge in the epic's own Jira color.
fn epic_pill(epic: &EpicRef, name: &str) -> Span<'static> {
    let bg = epic_color(epic);
    Span::styled(format!(" {name} "), Style::default().fg(readable_on(bg)).bg(bg))
}

/// Jira's epic color slots (`color_N` on the board API, `ghx-label-N` in the
/// epic's Epic Color field) in order, as the names its Issue color field uses.
/// Verified 1:1 against 100 epics on the team's instance.
const EPIC_COLOR_NAMES: [&str; 14] = [
    "dark_grey", "dark_yellow", "yellow", "dark_blue", "dark_teal", "green", "purple",
    "dark_purple", "orange", "blue", "teal", "grey", "dark_green", "dark_orange",
];

/// The epic's color: its Jira slot through Atlassian's palette, or a stable
/// hash of the key if Jira didn't say.
fn epic_color(epic: &EpicRef) -> Color {
    epic.color
        .and_then(|slot| EPIC_COLOR_NAMES.get(slot.checked_sub(1)?))
        .map(|name| jira_color(name))
        .unwrap_or_else(|| colors::hashed(&epic.key))
}

/// Atlassian's colors for Jira's named issue colors.
fn jira_color(name: &str) -> Color {
    match name {
        "purple" => Color::Rgb(0x82, 0x70, 0xDB),
        "dark_purple" => Color::Rgb(0x5E, 0x4D, 0xB2),
        "blue" => Color::Rgb(0x38, 0x8B, 0xFF),
        "dark_blue" => Color::Rgb(0x00, 0x55, 0xCC),
        "teal" => Color::Rgb(0x1D, 0x9A, 0xAA),
        "dark_teal" => Color::Rgb(0x20, 0x6A, 0x83),
        "green" => Color::Rgb(0x22, 0xA0, 0x6B),
        "dark_green" => Color::Rgb(0x21, 0x6E, 0x4E),
        "yellow" => Color::Rgb(0xE2, 0xB2, 0x03),
        "dark_yellow" => Color::Rgb(0x94, 0x6F, 0x00),
        "orange" => Color::Rgb(0xFA, 0xA5, 0x3D),
        "dark_orange" => Color::Rgb(0xC2, 0x51, 0x00),
        "dark_grey" => Color::Rgb(0x44, 0x54, 0x6F),
        _ => Color::Rgb(0x85, 0x90, 0xA2), // grey
    }
}

/// Black or white text, whichever reads better on `bg`.
fn readable_on(bg: Color) -> Color {
    let Color::Rgb(r, g, b) = bg else { return TEXT };
    let luma = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
    if luma > 140.0 { BACKGROUND } else { Color::Rgb(255, 255, 255) }
}

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .title_alignment(Alignment::Center)
        .title_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().fg(TEXT).bg(BACKGROUND))
}

fn centered(area: Rect, width_percent: u16, preferred_height: u16) -> Rect {
    let width = area.width.saturating_mul(width_percent).saturating_div(100).max(1);
    let height = preferred_height.min(area.height).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn centered_fixed(area: Rect, preferred_width: u16, preferred_height: u16) -> Rect {
    let width = preferred_width.min(area.width).max(1);
    let height = preferred_height.min(area.height).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Greedy word wrap by display width; words wider than `width` are split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for word in text.split_whitespace() {
        let word_width = word.width();
        let gap = usize::from(!current.is_empty());
        if current_width + gap + word_width <= width {
            if gap == 1 {
                current.push(' ');
            }
            current.push_str(word);
            current_width += gap + word_width;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if word_width <= width {
            current.push_str(word);
            current_width = word_width;
        } else {
            for ch in word.chars() {
                let w = ch.width().unwrap_or(0);
                if current_width + w > width {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                current.push(ch);
                current_width += w;
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_respects_width_and_splits_long_words() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 5), [""]);
    }

    #[test]
    fn pill_text_contrasts_with_background() {
        assert_eq!(readable_on(Color::Rgb(233, 196, 106)), BACKGROUND);
        assert_eq!(readable_on(Color::Rgb(69, 123, 157)), Color::Rgb(255, 255, 255));
    }

    #[test]
    fn a_moving_card_is_outlined_with_its_key_in_the_top_edge() {
        let issue = Issue {
            key: "DEMO-1".into(),
            summary: "Short summary".into(),
            story_points: Some(2.0),
            assignee: Some("Ann".into()),
            ..Default::default()
        };
        let text = |lines: &[Line]| -> Vec<String> {
            lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
        };
        let plain = card_lines(&issue, 24, true, false, false, true, true, &UserColors::default());
        let moving = card_lines(&issue, 24, true, true, false, true, true, &UserColors::default());
        let moving = text(&moving);
        assert_eq!(moving.len(), plain.len(), "outlining keeps the card's height");
        assert_eq!(moving[0], "┌ DEMO-1 ──────────────┐");
        assert_eq!(moving[1], "│ Short summary        │");
        assert_eq!(moving.last().unwrap(), "└──────────────────────┘");
        assert!(moving.iter().all(|l| l.width() == 24), "every row spans the card: {moving:?}");
        assert!(!moving.iter().any(|l| l.contains('▊')), "the outline replaces the cursor bar");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("hi", 6), "hi");
    }
}
