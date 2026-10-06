//! jdo — a terminal UI client for Jira boards.
//!
//! P1: fetches a real board (Agile API) via a background worker and renders it.
//! `--dump` prints a text summary and exits (no TUI) — handy for testing auth
//! and the mapping.

// Some scaffolded items are unused until later phases; keep the build quiet.
#![allow(dead_code)]

mod app;
mod clipboard;
mod colors;
mod config;
mod form;
mod jira;
mod lanes;
mod model;
mod reload;
mod sync;
mod ui;

use std::io::{self, Stdout, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::{Action, App};
use jira::auth::ApiTokenAuth;
use jira::client::JiraClient;
use model::Board;
use sync::{Command, Event as SyncEvent, Worker};

/// Why the event loop ended.
enum Exit {
    Quit,
    /// A new jdo binary was installed; restart into it.
    Restart,
}

/// Input poll tick. The background worker (see [`sync`]) delivers fresh data
/// over a channel drained at the top of the loop, so network latency never
/// blocks rendering.
const TICK: Duration = Duration::from_millis(250);

#[derive(Parser)]
#[command(name = "jdo", about = "A terminal UI client for Jira boards")]
struct Cli {
    /// Board id (overrides config / JDO_BOARD).
    #[arg(long)]
    board: Option<u64>,
    /// Fetch the board and print a text summary, then exit (no TUI).
    #[arg(long)]
    dump: bool,
    /// Don't capture the mouse (keeps the terminal's own text selection).
    #[arg(long)]
    no_mouse: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut cfg = config::load()?;
    if let Some(board) = cli.board {
        cfg.board_id = board;
    }
    let creds = config::credentials()?;

    let auth = ApiTokenAuth::new(&cfg.base_url, &creds.email, &creds.token);
    let client = JiraClient::new(Box::new(auth))?;

    if cli.dump {
        return dump_board(&client, &cfg);
    }

    let worker = sync::spawn(client, cfg.fields.clone(), cfg.base_url.clone());
    worker
        .tx
        .send(Command::LoadBoard { board_id: cfg.board_id })
        .context("queue initial board load")?;

    let mut app = App::new(Board::default(), cfg.board_id);
    if let Ok(state) = std::env::var(reload::RESUME_VAR) {
        app.apply_resume(&state);
    }
    // Watch our own executable so a reinstall restarts this instance.
    let mut watch = reload::BinaryWatch::new();

    let mouse = !cli.no_mouse;
    let mut terminal = setup_terminal(mouse)?;
    let result = run_event_loop(&mut terminal, &mut app, &worker, cfg.board_id, watch.as_mut());
    restore_terminal(&mut terminal, mouse)?;

    let mut worker = worker;
    worker.shutdown();
    match result? {
        Exit::Quit => Ok(()),
        Exit::Restart => {
            let path = watch.expect("restart implies a watch").path().clone();
            // Only returns if the exec failed.
            let error = reload::exec(&path, &app.resume_state());
            Err(anyhow::anyhow!("couldn't restart into {}: {error}", path.display()))
        }
    }
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    worker: &Worker,
    board_id: u64,
    mut watch: Option<&mut reload::BinaryWatch>,
) -> Result<Exit> {
    loop {
        if app.can_restart() && watch.as_mut().is_some_and(|w| w.replaced()) {
            return Ok(Exit::Restart);
        }

        // Drain completed fetches / write results before drawing.
        while let Ok(event) = worker.rx.try_recv() {
            match event {
                SyncEvent::Board(board) => app.on_board(board),
                SyncEvent::BoardError(message) => app.on_board_error(message),
                SyncEvent::Myself { account_id, display_name } => {
                    app.me = Some((account_id, display_name));
                }
                SyncEvent::Transitions { key, result } => app.on_transitions(&key, result),
                SyncEvent::TransitionDone { key, result } => app.on_transition_done(&key, result),
                SyncEvent::IssueRefreshed(issue) => app.on_issue_refreshed(issue),
                SyncEvent::Comments { key, result } => app.on_comments(&key, result),
            }
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        // Redraw fast enough to animate the spinner while the board loads.
        let tick = if app.is_animating() { app::SPINNER_INTERVAL } else { TICK };
        if event::poll(tick)? {
            let action = match event::read()? {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                TermEvent::Mouse(mouse) => app.handle_mouse(mouse),
                _ => continue,
            };
            match action {
                Action::None => {}
                Action::Quit => return Ok(Exit::Quit),
                Action::Refresh => worker.send(Command::LoadBoard { board_id }),
                Action::FetchTransitions(key) => worker.send(Command::FetchTransitions { key }),
                Action::SubmitTransition { key, body } => worker.send(Command::DoTransition { key, body }),
                Action::FetchComments(key) => worker.send(Command::FetchComments { key }),
            }
        }
    }
}

/// Headless fetch + print, for testing auth and mapping without the TUI.
fn dump_board(client: &JiraClient, cfg: &config::JiraConfig) -> Result<()> {
    let board = sync::load_board(client, &cfg.fields, client.browse_base(), cfg.board_id)?;

    // Write errors (e.g. a closed pipe from `| head`) just end the dump.
    let mut out = io::stdout().lock();
    let _ = (|| -> io::Result<()> {
        writeln!(out, "Board: {} (id {}) — {}", board.name, cfg.board_id, board.scope)?;
        for column in &board.columns {
            writeln!(out, "\n== {} ({}) ==", column.name, column.issues.len())?;
            for issue in &column.issues {
                let points = issue
                    .story_points
                    .map(|p| format!(" · {p}pts"))
                    .unwrap_or_default();
                writeln!(out, "  {:<14} [{}]{}  {}", issue.key, issue.status, points, issue.summary)?;
            }
        }
        Ok(())
    })();
    Ok(())
}

fn setup_terminal(mouse: bool) -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    if mouse {
        execute!(stdout, EnableMouseCapture)?;
    }
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mouse: bool) -> Result<()> {
    disable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
