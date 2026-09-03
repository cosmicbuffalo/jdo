//! jdo — a terminal UI client for Jira boards.
//!
//! P0 scaffold: renders a mock board so the shell is visible before the Jira
//! layer lands.
//! Modules under `jira/`, plus `sync` and `clipboard`, are stubs filled in
//! from P1 onward.

// Many scaffold items are unused until later phases; keep P0 warning-free.
#![allow(dead_code)]

mod app;
mod clipboard;
mod config;
mod jira;
mod model;
mod sync;
mod ui;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;
use model::Board;

/// Input poll tick. Matches tdo's cadence; the future background Jira worker
/// (see `sync`) will deliver fresh data over a channel drained at the top of
/// this loop, so network latency never blocks rendering.
const TICK: Duration = Duration::from_millis(250);

fn main() -> Result<()> {
    // P0: static mock data. P1 replaces this with a board fetched from Jira.
    let board = Board::mock();
    let mut app = App::new(board);

    let mut terminal = setup_terminal()?;
    let result = run_event_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        // P1 seam: drain completed Jira fetches / write-acks here before drawing.
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(TICK)? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
