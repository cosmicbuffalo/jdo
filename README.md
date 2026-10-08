# jdo

A terminal UI client for **Jira** boards — see and work your team's board without leaving the terminal.

`jdo` forks the TUI layer of [`tdo`](../tdo) (a git-backed kanban TUI) and drops the local storage backend entirely: all data comes from the Jira Cloud REST API. The goal is to replace day-to-day use of the Jira web GUI for the common flows — viewing the board, moving cards through statuses, editing key fields, creating cards, and reading comments.

> **Status:** reads the live board, moves cards through the workflow, edits description / acceptance criteria, and creates cards. Writing comments, clipboard, auto-refresh and OAuth are still to come.

## Install

```sh
cargo install --path .
export JIRA_EMAIL="you@example.com"
export JIRA_API_TOKEN="…"   # Atlassian account → Security → API tokens
jdo                          # or: jdo --board <id>
```

A running `jdo` notices when it is reinstalled and restarts itself into the new binary, keeping your grouping, folds and cursor.

## What it does

- **Board** — scrum boards show the active sprint (kanban boards apply the board's sub-filter), with the board's own columns and WIP limits: a column over its max (or under its min) gets a red (amber) title row and border.
- **Cards** — key, summary, epic (in the epic's Jira color), points (`? pts` when unestimated) and a distinct colored assignee pill (red `Unassigned` when nobody owns it).
- **Grouping** — `g` cycles none → epic → assignee. Each group is a foldable section with a complete board of its own.
- **Details** — `Enter` opens a card: fields, description, acceptance criteria, comments.
- **Editing** — in details, `e` / `a` edit the description / acceptance criteria as Markdown (`Ctrl-G` for `$EDITOR`). Headings, lists, code, quotes, links and bold/italic round-trip; tables, mentions and the like are kept as `⟦n:…⟧` placeholders and restored on save.
- **Move mode** — `m` loads the card's workflow; only columns it can legally move to light up. Transitions with screens open a form pre-filled with the issue's values; Jira's validator errors land on the offending fields.
- **Create** — `c` opens a "Create new <PROJECT> ticket" dialog: Team and Sprint default to the board's, Epic (the section you're in, when grouped by epic) and Assignee (the team's members) are type-to-filter pickers, plus Title, Description and Acceptance Criteria. Jira's create rules mark required fields; its errors show under the fields they belong to.
- **Mouse** — click to select, click again to open, `[×]` to close; footer hints and dialog buttons are clickable. `--no-mouse` keeps the terminal's own text selection.

## Keys

| Key | Action |
|---|---|
| `h` `j` `k` `l` / arrows | navigate (grouped: `h` from the first column reaches the section-header gutter) |
| `Enter` | open card details (on a section header: fold) |
| `m` | move mode: `h`/`l` reachable columns, `Tab` other transition, `Enter` confirm, `Esc` cancel |
| `c` | create a card (`Tab` between fields, `Ctrl-S` submit, `Esc` cancel) |
| `e` / `a` | in details: edit description / acceptance criteria (`Ctrl-S` save) |
| `g` | group: none → epic → assignee |
| `z` / `Z` | fold the section / all sections |
| `Home` / `End` | first / last card |
| `r` | refresh |
| `?` | keymap |
| `q` | quit |

## Configuration

**Board** — the only thing you normally choose. Highest priority first: `jdo --board <id>`, then `JDO_BOARD=<id>`, then `board_id` in the config file. The id is the number in the board's URL (`.../boards/<id>`).

**Team, project and sprint** aren't configured — they come from the board: new cards go to the board's project, the create dialog defaults to the board's most common Team and its active sprint, and the Assignee list is that team's recent assignees. Switch boards and they all follow.

**Credentials** come from the environment only (`JIRA_EMAIL`, `JIRA_API_TOKEN`), never the config file.

**Config file** — optional, not created for you: `~/.config/jdo/config.toml` (or `$XDG_CONFIG_HOME/jdo/config.toml`).

```toml
[jira]
base_url = "https://your-site.atlassian.net"   # or JIRA_BASE_URL
board_id = 123

[jira.fields]               # custom-field ids, only if your project differs
story_points = "customfield_10016"
acceptance_criteria = "customfield_10035"
sprint = "customfield_10020"
epic_link = "customfield_10014"
team = "customfield_10001"
```

`refresh_seconds` and `auth = "oauth"` are accepted but not implemented yet (press `r` to refresh; only API-token auth works).

## License

MIT — see [LICENSE](LICENSE).
