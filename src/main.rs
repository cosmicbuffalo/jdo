//! jdo — a terminal UI client for Jira boards.
//!
//! P1: fetches a real board (Agile API) via a background worker and renders it.
//! `--dump` prints a text summary and exits (no TUI) — handy for testing auth
//! and the mapping. See `docs/DESIGN.md` for the overall shape.

// Some scaffolded items are unused until later phases; keep the build quiet.
#![allow(dead_code)]

mod app;
mod clipboard;
mod colors;
mod config;
mod create;
mod edit;
mod find;
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
    /// Show which custom-field ids jdo uses and where each came from, then exit.
    #[arg(long)]
    fields: bool,
    /// Don't capture the mouse (keeps the terminal's own text selection).
    #[arg(long)]
    no_mouse: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let cfg = config::load(cli.board)?;
    let creds = config::credentials()?;

    let auth = ApiTokenAuth::new(&cfg.base_url, &creds.email, &creds.token);
    let client = JiraClient::new(Box::new(auth))?;

    // Pinned fields need no lookup; otherwise ask the site's catalog.
    let catalog = if cfg.fields.complete() && !cli.fields {
        Vec::new()
    } else {
        client.fields().context("look up custom-field ids in Jira's field catalog")?
    };
    let (fields, resolutions) = jira::fields::resolve(&cfg.fields, &catalog);

    if cli.fields {
        print_fields(&resolutions);
        return Ok(());
    }
    if cli.dump {
        return dump_board(&client, &fields, cfg.board_id);
    }

    let worker = sync::spawn(client, fields, cfg.base_url.clone());
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
    let result = run_event_loop(&mut terminal, &mut app, &worker, cfg.board_id, watch.as_mut(), mouse);
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
    mouse: bool,
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
                SyncEvent::Myself { account_id, display_name } => app.on_myself(account_id, display_name),
                SyncEvent::MyselfFailed(message) => app.on_myself_failed(message),
                SyncEvent::Transitions { key, result } => app.on_transitions(&key, result),
                SyncEvent::TransitionDone { key, result } => app.on_transition_done(&key, result),
                SyncEvent::IssueRefreshed(issue) => app.on_issue_refreshed(issue),
                SyncEvent::Comments { key, result } => app.on_comments(&key, result),
                SyncEvent::Rich { key, result } => app.on_rich(&key, result),
                SyncEvent::RichUpdated { key, field, result } => app.on_rich_updated(&key, field, result),
                SyncEvent::CreateMeta(result) => app.on_create_meta(result),
                SyncEvent::TeamMembers(result) => app.on_team_members(result),
                SyncEvent::Created(result) => app.on_created(result),
                SyncEvent::SearchResults { seq, query, result } => app.on_search_results(seq, query, result),
                SyncEvent::FieldUpdated { key, field, result } => app.on_field_updated(&key, field, result),
            }
        }
        let action = app.tick();
        if dispatch(action, app, worker, board_id, terminal, mouse) {
            return Ok(Exit::Quit);
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
            if dispatch(action, app, worker, board_id, terminal, mouse) {
                return Ok(Exit::Quit);
            }
        }
    }
}

/// Carry out an [`Action`]; returns true to quit.
fn dispatch(
    action: Action,
    app: &mut App,
    worker: &Worker,
    board_id: u64,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mouse: bool,
) -> bool {
    match action {
        Action::None => {}
        Action::Many(actions) => {
            for action in actions {
                if dispatch(action, app, worker, board_id, terminal, mouse) {
                    return true;
                }
            }
        }
        Action::Quit => return true,
        Action::Refresh => worker.send(Command::LoadBoard { board_id }),
        Action::FetchMyself => worker.send(Command::FetchMyself),
        Action::FetchTransitions(key) => worker.send(Command::FetchTransitions { key }),
        Action::SubmitTransition { key, body } => worker.send(Command::DoTransition { key, body }),
        Action::FetchDetails(key) => {
            worker.send(Command::FetchComments { key: key.clone() });
            worker.send(Command::FetchRich { key });
        }
        Action::SaveRich { key, field, value } => worker.send(Command::UpdateRich { key, field, value }),
        Action::FetchCreateMeta(project) => worker.send(Command::FetchCreateMeta { project }),
        Action::FetchTeamMembers(team_id) => worker.send(Command::FetchTeamMembers { team_id }),
        Action::CreateIssue(draft) => worker.send(Command::CreateIssue { draft }),
        Action::Search { seq, project, query } => worker.send(Command::Search { seq, project, query }),
        Action::UpdateField { key, field, value } => worker.send(Command::UpdateField { key, field, value }),
        Action::EditExternally => {
            if let Some(text) = app.rich_editor_text() {
                match edit_externally(terminal, mouse, &text) {
                    Ok(edited) => app.set_rich_editor_text(&edited),
                    Err(error) => app.error(format!("$EDITOR failed: {error:#}")),
                }
            }
        }
    }
    false
}

/// Headless fetch + print, for testing auth and mapping without the TUI.
/// `jdo --fields`: the id behind each custom field, for checking or pinning
/// in `[jira.fields]`.
fn print_fields(resolutions: &[jira::fields::Resolution]) {
    use jira::fields::Source;
    let mut out = io::stdout().lock();
    let _ = (|| -> io::Result<()> {
        for r in resolutions {
            let source = match r.source {
                Source::Config => "pinned in config",
                Source::Catalog => "auto-detected",
                Source::Missing => "not on this site",
            };
            let name = r.candidates.iter().find(|(id, _)| *id == r.id).map(|(_, n)| n.as_str());
            let shown = if r.id.is_empty() { "-".to_string() } else { r.id.clone() };
            let line = format!("{:<20} {:<20} {:<17} {}", r.key, shown, source, name.unwrap_or(""));
            writeln!(out, "{}", line.trim_end())?;
            for (id, name) in r.candidates.iter().filter(|(id, _)| *id != r.id) {
                writeln!(out, "{:<20}   also matches {id} ({name})", "")?;
            }
        }
        Ok(())
    })();
}

fn dump_board(client: &JiraClient, fields: &config::FieldMap, board_id: u64) -> Result<()> {
    let board = sync::load_board(client, fields, client.browse_base(), board_id)?;

    // Write errors (e.g. a closed pipe from `| head`) just end the dump.
    let mut out = io::stdout().lock();
    let _ = (|| -> io::Result<()> {
        writeln!(out, "Board: {} (id {}) — {}", board.name, board_id, board.scope)?;
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

/// Suspend the TUI, edit `text` in `$VISUAL`/`$EDITOR` (default `vi`) as a
/// Markdown file, then resume. Returns the edited text.
fn edit_externally(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mouse: bool, text: &str) -> Result<String> {
    let path = std::env::temp_dir().join(format!("jdo-{}.md", std::process::id()));
    std::fs::write(&path, text).context("write temp file")?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    let argv = shlex::split(&editor).filter(|a| !a.is_empty()).unwrap_or_else(|| vec![editor.clone()]);

    disable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    let status = std::process::Command::new(&argv[0]).args(&argv[1..]).arg(&path).status();
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    if mouse {
        execute!(terminal.backend_mut(), EnableMouseCapture)?;
    }
    terminal.clear()?;

    let edited = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    match status {
        Ok(status) if status.success() => Ok(edited.context("read temp file")?.trim_end_matches('\n').to_string()),
        Ok(status) => anyhow::bail!("{} exited with {status}", argv[0]),
        Err(error) => anyhow::bail!("couldn't run {}: {error}", argv[0]),
    }
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
