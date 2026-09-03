//! Rendering.
//!
//! P0 is a minimal board render to prove the shell. The heavier fork of tdo's
//! `ui.rs` — the detail modal, pickers, text editor, ADF rendering — lands in
//! P2+. Colors follow tdo's accent-on-dark look.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;

/// tdo's signature accent (TUI_ACCENT).
const ACCENT: Color = Color::Rgb(255, 135, 0);

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_title(frame, chunks[0], app);
    draw_columns(frame, chunks[1], app);
    draw_status(frame, chunks[2], app);
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App) {
    let line = Line::from(vec![
        Span::styled(
            " jdo ",
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(app.board.name.clone(), Style::default().add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_columns(frame: &mut Frame, area: Rect, app: &App) {
    if app.board.columns.is_empty() {
        frame.render_widget(Paragraph::new("No columns."), area);
        return;
    }

    let n = app.board.columns.len() as u32;
    let constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Ratio(1, n)).collect();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    for (ci, column) in app.board.columns.iter().enumerate() {
        let selected_col = ci == app.selected_column;
        let border_color = if selected_col { ACCENT } else { Color::DarkGray };
        let title = format!(" {} ({}) ", column.name, column.issues.len());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(
                title,
                Style::default().fg(border_color).add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(cols[ci]);
        frame.render_widget(block, cols[ci]);
        frame.render_widget(column_cards(app, ci, column), inner);
    }
}

fn column_cards<'a>(app: &App, ci: usize, column: &'a crate::model::Column) -> Paragraph<'a> {
    let selected_col = ci == app.selected_column;
    let mut lines: Vec<Line> = Vec::new();

    for (ri, issue) in column.issues.iter().enumerate() {
        let selected = selected_col && ri == app.selected_row;
        let bar = if selected { "▊ " } else { "  " };
        let key_style = if selected {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let summary_style = if selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::styled(bar, Style::default().fg(ACCENT)),
            Span::styled(issue.key.clone(), key_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(issue.summary.clone(), summary_style),
        ]));

        let mut meta = String::new();
        if let Some(sp) = issue.story_points {
            meta.push_str(&format!("{sp} pts  "));
        }
        if let Some(assignee) = &issue.assignee {
            meta.push_str(assignee);
        }
        if !meta.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {meta}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(""));
    }

    Paragraph::new(lines).wrap(Wrap { trim: true })
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(Span::styled(
            app.status.clone(),
            Style::default().fg(Color::DarkGray),
        )),
        area,
    );
}
