# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/kwhorne/e/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/kwhorne/e/compare/v0.2.6...v0.3.0
[0.2.6]: https://github.com/kwhorne/e/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/kwhorne/e/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/kwhorne/e/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/kwhorne/e/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/kwhorne/e/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/kwhorne/e/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/kwhorne/e/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/kwhorne/e/releases/tag/v0.1.0
