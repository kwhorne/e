# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/kwhorne/e/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/kwhorne/e/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/kwhorne/e/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/kwhorne/e/releases/tag/v0.1.0
