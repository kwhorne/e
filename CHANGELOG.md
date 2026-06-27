# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/kwhorne/e/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/kwhorne/e/releases/tag/v0.1.0
