//! jdo — a terminal UI client for Jira boards.
//!
//! P1: fetches a real board (Agile API) via a background worker and renders it.
//! `--dump` prints a text summary and exits (no TUI) — handy for testing auth
//! and the mapping.

// Some scaffolded items are unused until later phases; keep the build quiet.
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

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event as TermEvent};
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

    let mut app = App::new(Board::default());
    app.status = format!("Loading board {} …", cfg.board_id);

    let mut terminal = setup_terminal()?;
    let result = run_event_loop(&mut terminal, &mut app, &worker, cfg.board_id);
    restore_terminal(&mut terminal)?;

    let mut worker = worker;
    worker.shutdown();
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    worker: &Worker,
    board_id: u64,
) -> Result<()> {
    loop {
        // Drain completed fetches / write-acks before drawing.
        while let Ok(event) = worker.rx.try_recv() {
            match event {
                SyncEvent::Board(board) => {
                    app.replace_board(board);
                    app.status = format!(
                        "{} · {} issues · r: refresh · q: quit",
                        app.board.name,
                        app.total_issues()
                    );
                }
                SyncEvent::Error(message) => {
                    app.status = format!("error: {message}");
                }
                SyncEvent::IssueUpdated { .. } => {}
            }
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(TICK)? {
            if let TermEvent::Key(key) = event::read()? {
                match app.handle_key(key) {
                    Action::Quit => return Ok(()),
                    Action::Refresh => {
                        app.status = "Refreshing …".into();
                        let _ = worker.tx.send(Command::LoadBoard { board_id });
                    }
                    Action::None => {}
                }
            }
        }
    }
}

/// Headless fetch + print, for testing auth and mapping without the TUI.
fn dump_board(client: &JiraClient, cfg: &config::JiraConfig) -> Result<()> {
    let config = client.board_configuration(cfg.board_id)?;
    let list = sync::field_list(&cfg.fields);
    let raw = client.board_issues(cfg.board_id, &list)?;
    let board = jira::mapping::build_board(config, raw, &cfg.fields, client.browse_base());

    println!("Board: {} (id {})", board.name, cfg.board_id);
    for column in &board.columns {
        println!("\n== {} ({}) ==", column.name, column.issues.len());
        for issue in &column.issues {
            let points = issue
                .story_points
                .map(|p| format!(" · {p}pts"))
                .unwrap_or_default();
            println!("  {:<14} [{}]{}  {}", issue.key, issue.status, points, issue.summary);
        }
    }
    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
