# jdo

A terminal UI client for **Jira** boards — see and work your team's board without leaving the terminal.

`jdo` forks the TUI layer of [`tdo`](../tdo) (a git-backed kanban TUI) and drops the local storage backend entirely: all data comes from the Jira Cloud REST API. The goal is to replace day-to-day use of the Jira web GUI for the common flows — viewing the board, moving cards through statuses, editing key fields, and reading/writing comments.

> **Status:** reads the live board and moves cards through the workflow. Field edits outside transitions, writing comments, clipboard and OAuth are still to come.

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
- **Cards** — key, summary, epic (in the epic's Jira color), points (`? pts` when unestimated) and a colored assignee pill (red `Unassigned` when nobody owns it).
- **Grouping** — `g` cycles none → epic → assignee. Each group is a foldable section with a complete board of its own.
- **Details** — `Enter` opens a card: fields, description, acceptance criteria, comments.
- **Move mode** — `m` loads the card's workflow; only columns it can legally move to light up. Transitions with screens open a form pre-filled with the issue's values; Jira's validator errors land on the offending fields.
- **Mouse** — click to select, click again to open, `[×]` to close; footer hints are buttons. `--no-mouse` keeps the terminal's own text selection.

## Keys

| Key | Action |
|---|---|
| `h` `j` `k` `l` / arrows | navigate (grouped: `h` from the first column reaches the section-header gutter) |
| `Enter` | open card details (on a section header: fold) |
| `m` | move mode: `h`/`l` reachable columns, `Tab` other transition, `Enter` confirm, `Esc` cancel |
| `g` | group: none → epic → assignee |
| `z` / `Z` | fold the section / all sections |
| `Home` / `End` | first / last card |
| `r` | refresh |
| `?` | keymap |
| `q` | quit |

## Configuration

Optional `~/.config/jdo/config.toml`:

```toml
[jira]
base_url = "https://your-site.atlassian.net"
board_id = 123            # from the board URL .../boards/<id>; or JDO_BOARD / --board

[jira.fields]               # custom-field ids, if your project differs
story_points = "customfield_10016"
acceptance_criteria = "customfield_10035"
sprint = "customfield_10020"
epic_link = "customfield_10014"
```

Secrets come from the environment only, never the config file.

## License

MIT — see [LICENSE](LICENSE).
