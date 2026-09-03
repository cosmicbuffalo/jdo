# jdo

A terminal UI client for **Jira** boards — see and work your team's board without leaving the terminal.

`jdo` forks the TUI layer of [`tdo`](../tdo) (a git-backed kanban TUI) and drops the local storage backend entirely: all data comes from the Jira Cloud REST API. The goal is to replace day-to-day use of the Jira web GUI for the common flows — viewing the board, moving cards through statuses, editing key fields, and reading/writing comments.

> **Status: P0 scaffold.** Runs and renders a *mock* board to prove the shell. The Jira integration (real data, writes) lands in P1+.

## Try the scaffold

```sh
cargo run
```

Navigate with `h`/`j`/`k`/`l` (or arrows); `q` to quit. The board is fake data for now.

## Configuration (P1+)

`~/.config/jdo/config.toml`:

```toml
[jira]
base_url = "https://your-site.atlassian.net"
auth = "api_token"          # or "oauth"
board_id = 0                # from the board URL .../boards/<id>
refresh_seconds = 25
```

Secrets come from the environment, never the config file:

```sh
export JIRA_EMAIL="you@example.com"
export JIRA_API_TOKEN="…"   # Atlassian account → Security → API tokens
```

## License

MIT — see [LICENSE](LICENSE).
