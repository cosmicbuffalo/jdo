# jdo

A terminal UI client for **Jira** boards — see and work your team's board without leaving the terminal.

`jdo` forks the TUI layer of [`tdo`](https://github.com/cosmicbuffalo/tdo) (a git-backed kanban TUI) and drops the local storage backend entirely: all data comes from the Jira Cloud REST API. The goal is to replace day-to-day use of the Jira web GUI for the common flows — viewing the board, moving cards through statuses, editing key fields, creating cards, and reading comments.

> **Status:** reads the live board, moves cards through the workflow, edits description / acceptance criteria, and creates cards. Writing comments, clipboard, auto-refresh and OAuth are still to come. Design notes are in [`docs/DESIGN.md`](docs/DESIGN.md).

## Install

```sh
cargo install --git https://github.com/cosmicbuffalo/jdo
# or, from a clone:
cargo install --path .
```

Then follow [Setup](#setup) — jdo needs to know your Jira site, your board and your credentials before it can start.

A running `jdo` notices when it is reinstalled and restarts itself into the new binary, keeping your grouping, folds and cursor.

## What it does

- **Board** — scrum boards show the active sprint (kanban boards apply the board's sub-filter), with the board's own columns and WIP limits: a column over its max (or under its min) gets a red (amber) title row and border.
- **Cards** — key, summary, epic (in the epic's Jira color), points (`? pts` when unestimated) and a distinct colored assignee pill (red `Unassigned` when nobody owns it).
- **Grouping** — `g` cycles none → epic → assignee. Each group is a foldable section with a complete board of its own.
- **Only mine** — `a` on the board shows only cards assigned to you (press again for everyone's); works with grouping and survives a reinstall restart.
- **Find** — `f` finds any ticket in the board's project, on the board or not: type a key (`1234` or `ABC-1234`) or words from its title, description or comments. Board cards match instantly (fuzzy, on key + title) while Jira's text search fills in as you pause; `↑`/`↓` pick, `Enter` opens it just like a board card, and closing the details returns to the results. An empty query lists the tickets you created, newest first; `●` marks cards already on the board.
- **Details** — `Enter` opens a card: fields, description, acceptance criteria, comments.
- **Editing** — in details, `e` enters edit mode: `j`/`k` walk the editable fields (title, status, assignee, story points, sprint, epic, description, acceptance criteria) and `Enter` changes the one under the cursor — a one-line input, or a type-to-filter list for assignee, sprint and epic. Status lists the statuses the card's workflow can move it to from where it is, like move mode; a transition with a screen opens its form. This works for cards that aren't on the board too. Each change saves to Jira on its own. Putting a ticket in the active sprint puts it on the board; moving a board card out of it takes it off. `a` still jumps straight to the acceptance criteria. Description and acceptance criteria are edited as Markdown (`Ctrl-G` for `$EDITOR`). Headings, lists, code, quotes, links and bold/italic round-trip; tables, mentions and the like are kept as `⟦n:…⟧` placeholders and restored on save.
- **Move mode** — `m` loads the card's workflow; only columns it can legally move to light up. Transitions with screens open a form pre-filled with the issue's values; Jira's validator errors land on the offending fields.
- **Create** — `c` opens a "Create new <PROJECT> ticket" dialog: Team (if your site has a Team field) and Sprint default to the board's, Epic (the section you're in, when grouped by epic) and Assignee (the team's members) are type-to-filter pickers, plus Title, Description and Acceptance Criteria. Jira's create rules mark required fields; its errors show under the fields they belong to.
- **Mouse** — click to select, click again to open, `[×]` to close; footer hints and dialog buttons are clickable. `--no-mouse` keeps the terminal's own text selection.

## Keys

| Key | Action |
|---|---|
| `h` `j` `k` `l` / arrows | navigate (grouped: `h` from the first column reaches the section-header gutter) |
| `Enter` | open card details (on a section header: fold) |
| `m` | move mode: `h`/`l` reachable columns, `Tab` other transition, `Enter` confirm, `Esc` cancel |
| `c` | create a card (`Tab` between fields, `Ctrl-S` submit, `Esc` cancel) |
| `f` | find a ticket: type to search, `↑`/`↓` select, `Enter` open, `Esc` close |
| `e` | in details: edit mode — `j`/`k` field, `Enter` change it, `Esc` done |
| `a` | in details: edit acceptance criteria (`Ctrl-S` save) |
| `g` | group: none → epic → assignee |
| `a` | on the board: show only your cards / everyone's |
| `z` / `Z` | fold the section / all sections |
| `Home` / `End` | first / last card |
| `r` | refresh |
| `?` | keymap |
| `q` | quit |

## Setup

jdo works with **Jira Cloud** (`https://<your-site>.atlassian.net`). It needs three things.

### 1. Credentials

Create an API token at <https://id.atlassian.com/manage-profile/security/api-tokens>, then export it with the email of your Atlassian account (e.g. in your shell profile):

```sh
export JIRA_EMAIL="you@example.com"
export JIRA_API_TOKEN="…"
```

Credentials are read from the environment only — never from the config file. The token acts as you, with your Jira permissions.

### 2. Site and board

Open your board in the browser. Its URL looks like

```
https://your-site.atlassian.net/jira/software/c/projects/ABC/boards/123
```

The part before `/jira` is your **site**; the number after `/boards/` is the **board id**. Put both in `~/.config/jdo/config.toml` (or `$XDG_CONFIG_HOME/jdo/config.toml`; jdo doesn't create it for you):

```toml
[jira]
base_url = "https://your-site.atlassian.net"
board_id = 123
```

Either can come from elsewhere instead, highest priority first:

| Setting | Command line | Environment | Config file |
|---|---|---|---|
| Site | — | `JIRA_BASE_URL` | `base_url` |
| Board | `--board <id>` | `JDO_BOARD` | `board_id` |

Project, team and sprint aren't configured — they come from the board: new cards go to the board's project, the create dialog defaults to the board's most common Team and its active sprint, and the Assignee list is that team's recent assignees. Switch boards and they all follow.

### 3. Custom fields (usually automatic)

Story points, acceptance criteria, sprint, epic link and team are *custom fields* whose ids differ from site to site. At startup jdo looks them up in your site's field catalog by type and name, so on most sites there's nothing to do. Check what it found with:

```sh
jdo --fields
```

```
story_points         customfield_10028    auto-detected     Story Points
                       also matches customfield_10016 (Story point estimate)
acceptance_criteria  -                    not on this site
sprint               customfield_10020    auto-detected     Sprint
epic_link            customfield_10014    auto-detected     Epic Link
team                 customfield_10001    auto-detected     Team
```

A field that's `not on this site` is simply left blank in jdo. If jdo picked the wrong one — common when a site has several fields with the same name — pin the right id:

```toml
[jira.fields]
story_points = "customfield_10016"
```

`jdo --fields` lists every candidate it saw; for the full list, open `https://your-site.atlassian.net/rest/api/3/field` in a browser where you're signed in to Jira — each entry has an `id` and `name`. When every field is pinned, jdo skips the catalog lookup entirely.

### Check it

```sh
jdo --dump    # print the board as text and exit — a quick check of auth, board and fields
jdo           # open the TUI
```

### All options

```toml
[jira]
base_url = "https://your-site.atlassian.net"   # required (or JIRA_BASE_URL)
board_id = 123                                 # required (or JDO_BOARD / --board)

[jira.fields]          # optional; each one left out is found automatically
story_points = "customfield_NNNNN"
acceptance_criteria = "customfield_NNNNN"
sprint = "customfield_NNNNN"
epic_link = "customfield_NNNNN"
team = "customfield_NNNNN"
```

Unknown keys are reported rather than ignored, so a typo won't silently fall back. `refresh_seconds` and `auth = "oauth"` are accepted but not implemented yet (press `r` to refresh; only API-token auth works).

## License

MIT — see [LICENSE](LICENSE).
