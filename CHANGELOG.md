# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Delete rows & foreign-key hopping.** The cell edit dialog now has a **Delete
  row** action (honouring the read-only guard and requiring a primary key) and a
  **Follow FK →** action that jumps to the referenced table filtered to the
  linked value. Backed by new tested `e-db` primitives `insert_row`,
  `delete_row` (with an empty-predicate guard so it can never wipe a table),
  `fk_target` and `rows_where`.

## [0.7.1] - 2026-07-03

### Added

- **EXPLAIN + agent index suggestions.** With the cursor in a SQL string,
  **Explain SQL Under Cursor** (`⌥⌘⏎`) runs the engine's EXPLAIN, shows the plan
  in the result panel, and flags full table scans / missing indexes. **Suggest
  Index for SQL Under Cursor** then hands the query + findings to the AI agent,
  which proposes a Laravel migration adding the index — EXPLAIN → diagnosis →
  ready-made migration in one flow (MySQL, PostgreSQL, SQLite).
- **Index view.** The Structure tab now lists a table's indexes (name, unique vs
  plain, and the columns each covers) below its columns — for MySQL, PostgreSQL
  and SQLite. Useful for spotting missing indexes behind slow queries.
- **Database write protection.** Connections that look like production (SSH
  tunnels, non-local hosts, or names containing prod/production/live) default to
  **read-only**, and cell edits to a read-only connection are blocked with a
  warning — guarding against accidental writes to a real server over an SSH
  tunnel. A lock badge (🔒/🔓) on each connection shows and toggles the state.


- **Inline SQL intelligence.** Raw SQL inside PHP (`DB::select("…")`,
  `->whereRaw('…')`, migrations' `DB::statement("…")`, …) is no longer a dead
  string:
  - **Syntax highlighting** via the tree-sitter SQL grammar (detected from the
    PHP parse tree; double- and single-quoted; plain strings untouched).
  - **Schema-aware completion** — typing inside the SQL string suggests table
    names after `FROM`/`JOIN`/`UPDATE`/`INTO` and column names elsewhere, from
    the live database schema cache.
  - **Run the query under the cursor** with `⌘⏎` (or “Database: Run SQL Under
    Cursor”): executes it against a connected database and shows the results in
    the DB result panel.

  Highlighting + schema validation + one-key execution in the same editor —
  without a separate database tool.

### Fixed

- **Command palette (`⌘⇧P`) now actually filters as you type.** The real cause
  wasn't just a stale query (0.7.0) — its query/selection lived on `AppState`
  (a different reactive scope), so the `text_input` and the results list didn't
  stay in sync and the list stayed on the unfiltered set. The palette now uses
  view-local signals like the file finder, so typing filters live.

## [0.7.0] - 2026-07-03

### Fixed

- **Command palette (`⌘⇧P`) felt unresponsive.** It didn't clear the previous
  query on open, so new keystrokes appended to a stale search (e.g. `che`
  became `<old>che`) and matched nothing. It now starts fresh every time —
  typing filters immediately.

### Changed

- **Reopen the last project.** Launching `e` with no path (double-click, Dock,
  bare `e`) now reopens the project you last had open instead of the current
  directory.

### Added

- **Inline AI completion (“ghost text”).** After a short idle in a code file, `e`
  asks a local [Ollama](https://ollama.com) code model (fill-in-the-middle) for
  a one-line continuation and shows it as grey text at the cursor; `Tab` accepts
  it, typing or `Esc` dismisses it. Entirely local and **opt-in** — enable it in
  Settings → Editor (`ai_completion`), with the model set via `E_COMPLETION_MODEL`
  (default `qwen2.5-coder`). Requests are debounced and never block the editor;
  nothing runs unless it's enabled and Ollama is reachable.

## [0.6.9] - 2026-07-03

### Added

- **Linux releases.** Each release now publishes Linux binaries
  (`e-x86_64-unknown-linux-gnu.tar.gz` and `e-aarch64-unknown-linux-gnu.tar.gz`)
  alongside the macOS builds, so the in-app auto-updater works on Linux too. A
  Linux build+test job was added to CI to catch platform breakage early. See
  [installation](docs/installation.md#linux-build-dependencies) for the system
  libraries needed.

## [0.6.8] - 2026-07-03

### Fixed

- **Silent write failures are now surfaced.** A failed disk write during a
  Livewire property rename or an applied agent edit (full/read-only disk) used
  to report success anyway — risking a class and its view drifting out of sync.
  Writes now notify on failure and only report success when the change landed;
  agent edits reply `ok: false` instead of claiming success.
- **Database lock hardening.** A poisoned connection mutex in `e-db` (a thread
  panicking while holding the lock) no longer crashes the whole editor — the
  guard is recovered and the query returns normally or errors cleanly.

### Changed

- Modal overlay panels are registered as one `(open-signal, view)` list, so the
  "is anything open?" guard is derived automatically. Adding a panel is a single
  line and can no longer desync the two lists — the bug class behind the
  0.6.5/0.6.6 unclickable-window regressions.

### Internal

- Split the `state.rs` god-module: cohesive feature clusters now live in their
  own files (`runtime.rs`, `db_state.rs`, `terminal_state.rs`, `laravel_state.rs`,
  `completion_state.rs`, `navigation.rs`, `tdd_state.rs`), each extending
  `AppState` from its own module. Pure moves, no behaviour change; `state.rs` is
  down **~2,245 lines (6,599 → 4,354, −34%)**, leaving mostly irreducible core
  state (the constructor, buffers, LSP, save/format, diagnostics, cursor, tabs).
  A new `AppState::spawn_bg` helper centralises the background-work + UI-marshal
  boilerplate.
- A CI “Parser corpus” job runs the heuristic parsers (routes/views, Eloquent
  relationship + event graphs, Livewire/Inertia props) over real Laravel
  projects (laravel, pingcrm, livewire, laravel-permission), asserting “no
  panic, sane counts” — catching wild-PHP edge cases the happy-path unit tests
  miss.

- **macOS: "e cannot open files of this type".** Opening a file via Finder
  ("Open With → e", double-click, or the Dock) no longer fails — the app bundle
  now declares `e` as a text editor for *all* file types, so `.sql`, `.env`,
  `.log` and anything else are accepted. Files already opened fine by dragging
  onto the window, via `⌘O`, or `e <file>` from the CLI; this removes the OS
  rejection for the Finder path. (Rebuild the app / install the next release;
  macOS may take a moment to refresh its file associations.)

## [0.6.7] - 2026-07-03

### Added

- **`e-dap` crate** — a synchronous Debug Adapter Protocol client (sibling to
  `e-lsp`, same architecture: protocol client in its own crate, background
  reader thread, id-correlated blocking requests). Reuses the identical
  `Content-Length` stdio framing but dispatches on DAP's `request`/`response`/
  `event` shape, correlates responses by `request_seq`, delivers adapter events
  to a handler, and answers reverse requests. Typed helpers cover the full
  step-debugging flow: `initialize`, `launch`/`attach`, `setBreakpoints`,
  `configurationDone`, step controls, `threads`/`stackTrace`/`scopes`/
  `variables`, and `evaluate`. This is the editor half of native debugging;
  paired with Grove's `grove debug on`, PHP/Xdebug works, and JS (`js-debug`) /
  Rust (`codelldb`) come nearly free since the client is adapter-agnostic.
- **Debug panel + step-debugging in the UI.** A new Debug overlay shows session
  status, execution controls (start/continue, step over/into/out, stop), the
  live call stack (click a frame to jump to source), current-frame variables,
  and all breakpoints. Breakpoints show as a red dot in the editor margin, and
  the line execution is paused on is highlighted; stopping auto-jumps to it.
  Keybindings: F5 start/continue, F9 toggle breakpoint on
  the caret line, F10 step over, F11 step into, ⇧F11 step out; all also in the
  command palette (“Debug: …”). The adapter (`vscode-php-debug`) is launched over
  Grove's bundled Node automatically (discovered from Grove's `node-builds.json`,
  falling back to `node` on PATH); the adapter path is auto-detected from
  installed VS Code/Cursor extensions or `E_PHP_DEBUG_ADAPTER`. Pair with
  `grove debug on` and step-debugging PHP works end to end.
- **Multi-language debugging.** `e-dap` now speaks DAP over TCP as well as stdio,
  so beyond PHP (Xdebug) the debugger also drives JavaScript/TypeScript via
  `vscode-js-debug` and Rust/C/C++ via `codelldb` (both DAP servers). The
  adapter is chosen from the active file's language and auto-discovered from
  installed VS Code/Cursor extensions (overridable via `E_JS_DEBUG_ADAPTER`,
  `E_CODELLDB`, `E_DEBUG_PROGRAM`).
- Alt-click in the editor toggles a breakpoint on the clicked line (Floem's
  built-in gutter isn't clickable), and breakpoints set before a file is opened
  now appear when it opens.
- **Settings: Enable Xdebug** (Settings → Laravel, `xdebug` in `config.json`) —
  toggling it runs `grove debug on`/`off` so you can start step-debugging without
  the terminal. On startup the toggle is synced from Grove's real state
  (`grove debug status`). Degrades gracefully when Grove isn't installed.

### Fixed

- Debugging is fully opt-in and never affects the editor when Grove/Xdebug or a
  DAP adapter aren't installed: the adapter is now launched entirely off the UI
  thread, so a missing or slow adapter (including the TCP connect for JS/Rust)
  can't freeze the editor, and missing tools report a clear status instead.

## [0.6.6] - 2026-07-02

### Fixed

- The file explorer and editor were unclickable: an overlay group wrapper
  introduced in 0.6.5 covered the whole window and swallowed every click. It now
  only covers the window when one of its panels is open.
- `⌘P` now finds hidden files (`.env`, `.gitignore`, …); they were excluded from
  the file index.

## [0.6.5] - 2026-07-02

### Added

- **Query-builder completion**: column names inside `where()`, `orderBy()`,
  `select()`, `pluck()`, `value()`, `groupBy()`, … and relationship names inside
  `with()`, `load()`, `whereHas()`, resolved from the model/table and the live
  schema. Unknown columns are flagged inline (`Column emial not found in table
  users`) — something PhpStorm can't do without the database.
- **Related files** (`⌘⌥E`): jump between the model, migration(s), factory,
  seeder, controller, policy, request, resource, and test for the same resource.
- **Validation intelligence**: completion for rule names in `validate([…])` /
  FormRequest `rules()`, plus a command to generate rules from a table's live
  schema (nullability, string lengths, types).
- **Gates & policies**: completion and go-to-definition for abilities in
  `can()`, `authorize()`, `@can`, and `Gate::allows()` → the policy method.
- **Generate model from table**: builds an Eloquent model from the live schema —
  fillable, casts, and relationships inferred from the actual foreign keys.
- **Event dispatch graph** (`⌘⌥G`): events → listeners (from `$listen`,
  `Event::listen`, and auto-discovered `handle()` types), with `F12` on a
  dispatched event jumping to a listener.

### Changed

- The file explorer now shows hidden files (`.env`, `.gitignore`, `.github`, …).
  Only `.git`, `node_modules`, and `target` are hidden.

## [0.6.4] - 2026-07-02

### Added

- **Inertia awareness.** `Inertia::render('Users/Index')` now resolves like
  `view()`: go-to-definition and completion reach the page component under
  `resources/js/Pages`, and the architecture map goes route → controller → page
  component instead of stopping at the controller.
- **Props contract** (`⌘⌥C`): reconciles a page component with the controller
  that renders it — infers TypeScript types from the render call (`User::paginate()`
  → `User[]`, fields from the live schema), flags props sent but unused and props
  used but never sent, and generates TypeScript interfaces expanded from the
  real database schema. Also reconciles `useForm` fields against the matching
  FormRequest's validation rules.
- **Ziggy route intelligence on the JS side**: `route('name')` in
  JS/TS/Vue/Svelte gets completion, hover, and go-to-definition from the same
  Laravel route table the PHP side uses.
- **Shared props**: `HandleInertiaRequests::share()` is parsed so
  `$page.props.auth.user` and friends complete everywhere.
- **Inertia-aware request replay**: the replay renders an Inertia response as an
  explorable props tree (with the component name, click to open) instead of raw
  HTML.
- **Livewire refactoring**: `wire:model` completes from the component class's
  public properties, `⌘⌥J` switches between the view and class, `F12` on a
  property jumps to its declaration, and renaming a property (`F2`) updates both
  the class and every `wire:` reference in the view.
- **Runtime insight** (`⌘⌥I`): a continuous Telescope-style panel that captures
  every request against the dev app via Clockwork — queries with N+1 warnings,
  cache hits/misses, mails, and events — with "Explain with agent" one click
  away. No Telescope or Debugbar needed.

## [0.6.3] - 2026-07-02

### Added

- Eloquent relationship graph (`⌘⌥R`): parses `hasMany`/`belongsTo`/
  `belongsToMany`/`morph*` from your models and cross-checks them against the
  live database's foreign keys — flagging relations that exist in code but have
  no backing FK. Together with the schema diff it shows code, migrations, and the
  actual database in one picture.
- Security lens on the architecture map (`⌘⌥M`): each route shows its middleware
  and a 🔒 / ⚠ badge; state-changing routes with no authentication are flagged,
  and one click asks the agent to suggest protection.
- Generate a Pest test from a request replay: the 🧪 button writes a feature test
  with the path, status, and assertions derived from the actual response, ready
  for the `⌘⇧T` "fix to green" loop.

## [0.6.2] - 2026-07-02

### Changed

- Redesigned the settings dialog (`⌘,`) into a two-pane layout: a category
  sidebar plus a search box that filters settings across every category, with
  hairline row dividers, per-row "restart" badges, and an "Open config.json"
  footer link. Close with `Esc` or the ✕.
- The command palette (`⌘⇧P`) now uses fuzzy, ranked matching instead of a plain
  substring filter — typing `up` surfaces "Check for Updates" and "Move Line Up"
  first, and the selection resets to the best match as you type.

## [0.6.1] - 2026-07-02

### Added

- Semantic search (⌘⌥K): a "describe what you're looking for" mode that ranks
  project locations by meaning. Runs locally — uses a local Ollama embedding
  model when available, with a lexical fallback otherwise.
- Visual undo tree (⌘⌥U): a branching history that preserves edits a linear undo
  would discard, with click-to-jump time travel persisted across sessions.

### Changed

- Release builds are now signed with a Developer ID and notarized by Apple, so
  the DMG opens without Gatekeeper warnings. CI signs automatically when the
  signing secrets are configured (see docs/installation.md).
- Added the missing ⌘3 (Toggle database) shortcut to the welcome screen.

## [0.6.0] - 2026-07-02

### Added

- Schema diff (command palette): compares migrations against the live database
  and flags columns present in one but not the other.
- Eloquent completion from the live database schema: typing `$model->` suggests
  the real table columns (inferred model → table), merged with the language
  server.
- Live `laravel.log` panel (⌘⌥L): tails the log with coloured levels, clickable
  stack frames, and a "Fix with AI" action.
- Request-replay from the architecture map (⌘⌥M): a ▶ button on GET routes hits
  the running app (Grove `https://<folder>.test` by default, configurable via the
  App URL setting) and shows the response plus the SQL queries it ran (via
  Clockwork) with N+1 detection and "Explain with agent".
- Autonomous TDD panel (⌘⇧T): run the test suite with pass/fail status, and a
  "Fix to green" loop where failures are sent to the agent, its proposed edits
  are reviewed, and tests re-run automatically until green (with an iteration
  cap and Stop).
- Agent `propose_edit`: agents propose a new file version and you review it
  hunk-by-hunk (accept/reject each change) before applying — no blind writes.
- Agent timeline (⌘⌥A): an audit log of everything the agent does over the
  socket, and a 🤖 marker in the status bar showing where the agent is looking.

## [0.5.1] - 2026-07-01

### Changed

- The welcome screen shows a minimalist, transparent app glyph instead of the
  boxed icon.
- The Settings dialog (⌘,) has consistent row spacing and a darker backdrop.
- The macOS "About e" menu-bar panel now shows the app icon, version, tagline
  and links (matching the ⌘⇧P About box's content).

## [0.5.0] - 2026-07-01

### Added

- Agent co-op over the sync socket: `lsp_definition`/`lsp_references`/`lsp_hover`/
  `lsp_symbols` (reuse the running language server), `db_schema` (read the
  connected database's schema without exposing credentials), `db_query`
  (agent-proposed queries run only after you approve them in a consent dialog),
  `run` and `tinker` (execute commands/PHP and capture the output — the basis
  for autonomous TDD).
- "Explain with agent" on failed database queries, and a "Fix with AI agent"
  action on problems, both prompt the agent panel directly.
- Laravel Tinker scratchpad (⌘⌥T): run PHP against the app and see the output;
  "Tinker: Run Selection" evaluates the current selection.
- Laravel architecture map (⌘⌥M): an interactive route → controller → view flow
  with clickable cards that jump to the code.

### Changed

- The app icon now appears in the About dialog and the welcome screen, and the
  bundle icon was refreshed.

## [0.4.9] - 2026-06-29

### Added

- AI Agent Workspace Sync: the editor exposes a local socket (`$E_EDITOR_SOCK`)
  so a CLI agent can read editor context (current file, cursor, selection,
  diagnostics) and drive the editor (open a file at a line, focus a panel,
  notify). See docs/agent-sync.md.

## [0.4.8] - 2026-06-29

### Added

- Terminal scrollback (5000 lines): scroll up with the mouse wheel to review
  earlier output; the view stays anchored while output streams and snaps back to
  the bottom when you type.
- Terminal background colours (SGR 40–47/100–107 and 256/true-colour) — `git
  diff`, coloured errors and search tools keep their highlighting.

### Changed

- SSH passwords are no longer written to disk: the askpass helper reads the
  secret from an environment variable that only lives in memory.
- External-change file polling runs off the UI thread, so the editor never stalls
  on slow or network filesystems.
- External reloads honour the file's detected encoding (UTF-16/Windows-1252).

### Fixed

- Language servers now shut down gracefully (shutdown + exit) and their stderr is
  logged instead of discarded, making LSP issues diagnosable.

## [0.4.7] - 2026-06-29

### Added

- The terminal panel (⌘T) is now drag-resizable in height — drag the handle
  along its top edge.

### Fixed

- The agent panel (⌘L) could not find CLI agents installed via nvm
  (`command not found`) when the app was launched from Finder; agents now run
  through an interactive login shell so `.zshrc`/`.bashrc` PATH is available.

## [0.4.6] - 2026-06-29

### Added

- Source Control: a ✨ button suggests a Conventional Commits message
  (type, scope and changed files) generated from your staged changes.

### Changed

- Project links now point to elyracode.com (About dialog, README docs link and
  Cargo metadata).

## [0.4.5] - 2026-06-28

### Added

- Tailwind CSS highlighting inside `class="…"` attributes (Blade, HTML, Vue):
  utility classes, variant prefixes (`sm:`, `dark:`, `hover:`) and arbitrary
  values (`w-[680px]`) are coloured distinctly.

## [0.4.4] - 2026-06-28

### Changed

- Blade syntax highlighting now colours Blade directives, `{{ }}`/`{!! !!}`
  expressions, `{{-- comments --}}` and the embedded PHP inside `@php` blocks and
  echoes — in addition to HTML, attributes and Tailwind classes.

## [0.4.3] - 2026-06-28

### Changed

- The `⌘P` file finder now uses ranked fuzzy matching (file-name and short-path
  matches rank highest, e.g. `wbp` finds `welcome.blade.php`) and builds its
  index in the background, so it opens instantly even on very large folders.

## [0.4.2] - 2026-06-28

### Added

- Emmet abbreviation expansion (Tab) in HTML, Blade, Vue, Svelte and PHP:
  tags, classes, ids, attributes, text, nesting, grouping, multiplication and
  `$` numbering.

### Fixed

- `⌘W` / `Esc` now close the database results overlay (and the cell-edit popup).

## [0.4.1] - 2026-06-28

### Added

- Database: inline cell editing (double-click a cell in a table with a primary
  key), saved queries (per project), ClickHouse support (HTTP interface), and SSH
  tunnels for remote databases.

### Changed

- New application icon.

## [0.4.0] - 2026-06-28

### Added

- Database panel (⌘3): browse and query MySQL/MariaDB, PostgreSQL and SQLite
  databases. Connect from the project's `.env` or manually (with a Test button),
  browse tables with sortable columns, paging, a Data/Structure view and CSV
  export, and run SQL in a results grid (⌘↵ to run, horizontal scroll and arrow
  keys to pan). Right by default; configurable left.
- Laravel features on par with the official VS Code extension: completion, hover
  and go-to-definition for `route()`, `view()`, `config()`, `env()`, `__()`/`trans()`
  and `<x-...>` Blade components, sourced from your project via `php artisan`.
  Auto-enabled in Laravel projects; toggle under Settings → Laravel features.

## [0.3.3] - 2026-06-28

### Added

- Line-ending conversion: click LF/CRLF in the status bar to convert the buffer.
- Non-UTF-8 files now open (BOM detection + Windows-1252 fallback); the detected
  encoding is shown in the status bar and preserved on save.

### Changed

- Large files (>1MB) skip tree-sitter highlighting, git markers, blame, inlay
  hints and bracket matching to stay responsive.

## [0.3.2] - 2026-06-28

### Added

- Multi-root workspaces: "Add Folder to Workspace" adds more root folders; the
  explorer, file finder and search span them all.
- Drag & drop files from Finder into the window to open them (folders open in a
  new window).
- Select all occurrences of the word/selection (⌘⇧L).

## [0.3.1] - 2026-06-28

### Added

- Task runner (`⌘⇧B`): detects npm/yarn/pnpm/bun, Composer, Cargo, Go, Laravel
  artisan, Pest/PHPUnit and Makefile tasks and runs the chosen one in a named
  terminal. "Run Tests" runs the project's test command.
- Customizable keybindings: every action is a named command, rebindable in the
  `keybindings` section of `config.json`.
- Graphical settings page (`⌘,`): toggles and steppers for the common options,
  applied live and persisted to `config.json`. The raw JSON is still available
  via "Open Settings (config.json)".

## [0.3.0] - 2026-06-27

### Added

- Inlay hints: inline type and parameter-name hints from the language server,
  shown as dimmed phantom text. Configurable via `inlay_hints`.
- Sticky scroll: the enclosing scope lines stay pinned at the top of the editor
  as you scroll (indentation-based). Configurable via `sticky_scroll`.
- Workspace replace: the search panel (`⌘⇧F`) now has a Replace row and "Replace
  All".
- Source Control: branch switcher (click the branch name), recent-commit history,
  and stash (Stash / Pop).
- Editor tabs: drag to reorder, and right-click to pin (with Close Others).
- User-defined snippets in the `snippets` section of `config.json`.

## [0.2.6] - 2026-06-27

### Fixed

- After an in-place auto-update, the bundle Info.plist version is rewritten so the
  macOS About panel shows the correct version (previously stale).
- Dev/bundle scripts now stamp the real version from Cargo.toml into Info.plist.

## [0.2.5] - 2026-06-27

### Fixed

- Clicking a command/file in the `⌘P`, `⌘⇧P`, `⌘T` and `⌘E` palettes now runs the
  selection instead of just closing the palette (the close-on-blur fired before
  the click registered).
- The update notice's "What's new" changelog now wraps properly and strips
  markdown noise, instead of overflowing horizontally.

## [0.2.4] - 2026-06-27

### Added

- macOS DMG installer (`scripts/bundle-dmg.sh`, also built per release) — drag
  `e.app` into Applications. Supports universal (arm64 + x86_64) builds and
  optional Developer ID signing/notarization.
- "Install 'e' Command in PATH" command (⌘⇧P) — symlinks `e` into
  `/usr/local/bin` so you can launch the editor from any directory with `e .`.

## [0.2.3] - 2026-06-27

### Added

- Framework-aware completion: Flux UI components (`<flux:…>`), Livewire `wire:`
  directives, Tailwind utility classes (inside `class="…"`), and Vue/Svelte
  directives.
- File-type icons in the explorer, per language/extension, with open/closed
  folder icons.

### Fixed

- Accepting a completion now places the caret at the end of the inserted text
  instead of in the middle (affected framework and LSP completions alike).

## [0.2.2] - 2026-06-27

### Added

- Configurable panel layout: `sidebar_side` and `agent_side` in settings move the
  explorer/Git sidebar and agent panel to the left or right (default: sidebar
  left, agent right).

### Fixed

- The quick-open palettes (`⌘P`, `⌘⇧P`, `⌘T`, `⌘E`) no longer stretch to the full
  window height — they size to their contents.
- Typing in a palette now reliably reaches its input: the editor no longer steals
  keyboard focus while a palette or dialog is open (it re-focuses on close).

## [0.2.1] - 2026-06-27

### Added

- Built-in completion that works with or without a language server: language
  keywords, identifiers already in the file, and — for PHP/Blade — Laravel
  facades and Blade directives. Merged with LSP and snippet suggestions.
- New file (`⌘N`) creates an untitled buffer; Save As… (`⌘⇧S`) writes it to disk
  and reopens it with full language, LSP, and git support.
- Complete user documentation in `docs/` (installation, editing, navigation,
  languages, Laravel, source control, terminal, agents, configuration, updating,
  and troubleshooting).

### Changed

- The editor now takes keyboard focus automatically when a buffer becomes active
  (new file, opening a file, switching tabs), so you can type immediately
  without clicking into it first.

## [0.2.0] - 2026-06-27

### Added

- Git blame for the current line shown in the status bar.
- Merge-conflict resolution bar: accept current, incoming, or both sides when the
  caret is inside a conflict block.
- Open dialogs: ⌘O opens a native folder picker to open another project in a
  new window; an "Open File…" command opens any file in the current window.
- Source Control panel (⌘2): branch display, staged / unstaged / untracked file
  groups with stage, unstage, discard and stage-all; commit, push and pull.
- Editor zoom (`⌘=` / `⌘-` / `⌘0`) and a soft word-wrap toggle (`⌥Z`).
- Navigation history: go back (`⌃-`) and forward (`⌃⇧-`) across jumps.
- Richer status bar: git branch, line ending (LF/CRLF), indentation and encoding.
- Recent-files quick switcher (⌘E): a most-recently-used list of files opened
  this session, newest first, with arrow-key navigation.
- Built-in auto-updater: checks GitHub for newer releases on startup, shows the
  changelog in a notice, and installs the update in place on confirmation.
  Manual check available via the command palette ("Check for Updates").
- Release workflow that publishes per-platform binary assets for each tag.
- Find & Replace: replace and replace-all in the active file, with
  case-sensitive, whole-word and regex toggles (`⌥⌘F`).
- Editing essentials: toggle line comment (`⌘/`), go to line (`⌃G`), move line
  up/down (`⌥↑/↓`), duplicate line (`⇧⌥↓`), delete line (`⌘⇧K`), and
  indent/outdent (`⌘]` / `⌘[`).
- Auto-closing brackets and quotes (with type-over and pair-aware backspace) and
  auto-indent on newline. Configurable via `auto_close`.
- Unsaved-changes confirmation when closing a tab.
- External file-change detection: clean buffers reload automatically; buffers
  with unsaved edits show a reload/keep prompt.

## [0.1.0] - 2026-06-27

### Added

- Tree-sitter syntax highlighting for Rust, Python, JavaScript, TypeScript, Go,
  C/C++, JSON, PHP, HTML, CSS, Blade, Vue and Svelte.
- Language Server Protocol client with diagnostics, completion, hover,
  go-to-definition, find references, document & workspace symbols, formatting,
  rename, code actions and signature help; per-language servers auto-selected.
- Laravel-aware completion for `route()`, `view()`, `config()` and `env()`.
- Fuzzy file finder (`⌘P`) and command palette (`⌘⇧P`).
- Workspace search (`⌘⇧F`) and find-in-file (`⌘F`).
- Integrated PTY terminal with ANSI colour, multiple tabs, rename and split.
- AI agent panel (`⌘L`) running configurable CLI agents (Elyra, Claude Code,
  Codex), with an agent selector and global settings.
- Split editor, resizable panels (drag), multi-cursor (`⌘D`).
- Git change gutter and inline diff vs `HEAD`.
- Inline diagnostics, bracket matching, snippets, breadcrumbs.
- Markdown preview (`⌘⇧M`).
- Light/dark themes (`F8`), auto-save, format & trim on save.
- Session persistence per workspace and a workspace-wide problems panel.

[Unreleased]: https://github.com/kwhorne/e/compare/v0.6.6...HEAD
[0.6.6]: https://github.com/kwhorne/e/compare/v0.6.5...v0.6.6
[0.6.5]: https://github.com/kwhorne/e/compare/v0.6.4...v0.6.5
[0.6.4]: https://github.com/kwhorne/e/compare/v0.6.3...v0.6.4
[0.6.3]: https://github.com/kwhorne/e/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/kwhorne/e/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/kwhorne/e/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/kwhorne/e/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/kwhorne/e/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/kwhorne/e/compare/v0.4.9...v0.5.0
[0.4.9]: https://github.com/kwhorne/e/compare/v0.4.8...v0.4.9
[0.4.8]: https://github.com/kwhorne/e/compare/v0.4.7...v0.4.8
[0.4.7]: https://github.com/kwhorne/e/compare/v0.4.6...v0.4.7
[0.4.6]: https://github.com/kwhorne/e/compare/v0.4.5...v0.4.6
[0.4.5]: https://github.com/kwhorne/e/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/kwhorne/e/compare/v0.4.3...v0.4.4
[0.4.3]: https://github.com/kwhorne/e/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/kwhorne/e/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/kwhorne/e/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/kwhorne/e/compare/v0.3.3...v0.4.0
[0.3.3]: https://github.com/kwhorne/e/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/kwhorne/e/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/kwhorne/e/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/kwhorne/e/compare/v0.2.6...v0.3.0
[0.2.6]: https://github.com/kwhorne/e/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/kwhorne/e/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/kwhorne/e/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/kwhorne/e/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/kwhorne/e/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/kwhorne/e/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/kwhorne/e/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/kwhorne/e/releases/tag/v0.1.0
