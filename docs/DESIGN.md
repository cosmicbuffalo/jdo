# jdo design notes

`jdo` reuses the terminal UI of [`tdo`](https://github.com/cosmicbuffalo/tdo)
(a git-backed kanban TUI) and replaces its storage with the Jira Cloud REST API.
The aim is a terminal client good enough to replace day-to-day use of the Jira
web UI for one board.

## Shape

- **View model.** `model.rs` keeps tdo's `Board / Column / Issue` shape as an
  in-memory view model. `jira/mapping.rs` fills it from the board's own
  configuration (`/rest/agile/1.0/board/{id}/configuration`), so columns,
  status mapping and WIP limits mirror the real board. Scrum boards show the
  active sprint; kanban boards apply the board's sub-filter.
- **No async runtime.** The event loop stays synchronous. A background worker
  thread (`sync.rs`) runs a blocking HTTP client and talks to the UI over two
  `mpsc` channels (UI → worker `Command`s, worker → UI `Event`s), drained at the
  top of each loop iteration. Network calls happen in response to user
  actions, plus a timed board refresh (`AutoRefresh` in `app.rs`): every
  `active_refresh_seconds` (5) within a minute of the last input, every
  `refresh_seconds` (30) otherwise, one load at a time, doubling the wait
  after each failed load (capped at 5 minutes).
- **Typed writes.** Each mutation is its own command — workflow transition,
  field update, create — sent to the worker; Jira's validation errors come back
  structured and land on the fields they belong to.
- **Auth behind a trait.** `jira/auth.rs` defines `Auth` (`apply`, `api_base`,
  `browse_base`). `ApiTokenAuth` (Basic `email:token`, base
  `https://<site>.atlassian.net`) ships today. An OAuth 2.0 (3LO) backend would
  use a different base (`https://api.atlassian.com/ex/jira/{cloudId}`), which is
  why the client always builds URLs from `Auth::api_base()`.
- **Rich text.** Description and acceptance criteria are Atlassian Document
  Format. `jira/markdown.rs` converts ADF ↔ Markdown for editing; nodes with no
  Markdown equivalent (tables, mentions, …) become `⟦n:…⟧` placeholders that
  are restored on save.

## Field resolution

Custom-field ids differ between Jira sites, so none are built in.
`jira/fields.rs` resolves each logical field (story points, acceptance
criteria, sprint, epic link, team):

1. a `[jira.fields]` override in the config file wins;
2. otherwise the site's field catalog (`GET /rest/api/3/field`) is searched —
   first by Jira Software field type where one exists (sprint, epic link,
   team), then by name;
3. otherwise the field is left unresolved and the features that need it stay
   out of the way.

If every field is pinned, the catalog isn't fetched at all. `jdo --fields`
prints the outcome, including ambiguous matches worth pinning.

## Risks

- Some organizations restrict personal API tokens or OAuth apps; check your
  site's policy before using jdo there.
- Writes go to your real Jira. Try transitions and edits on a throwaway issue
  first when changing write paths.
- Auto-refresh must stay modest. The worker caches the board's shape (board
  configuration, board info, open sprints; `BoardShape` in `sync.rs`) for 24
  hours, so a timed refresh is just the issues, usually one request; `r`, the
  first load and any failed load fetch the shape again. Failed loads, HTTP 429
  included, back off.
- Open details reload their comments and rich text only when the card's
  `updated` timestamp moves forward (an open card off the board is re-fetched on
  the same timer). Our own writes are recorded with the time Jira confirmed
  them (`written` in `app.rs`): a board asked for before that is older than our
  copy, so our copy wins, and the first board after it shows our change, so its
  new timestamp doesn't count as an outside edit.
