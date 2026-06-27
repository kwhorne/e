<div align="center">

# e

**The editor for the rest of us**

A fast, native code editor written in Rust — with first-class PHP/Laravel support,
a built-in terminal, and an integrated AI agent panel.

</div>

---

## Overview

`e` is a lightweight, GPU-accelerated code editor built from scratch in Rust. It
pairs a responsive native UI with the tooling developers expect day to day —
tree-sitter syntax highlighting, Language Server Protocol support, fuzzy file
navigation, an integrated terminal, and a right-hand panel that runs CLI coding
agents (Elyra, Claude Code, Codex, …) right next to your code. The UI is
GPU-accelerated and reactive, with a focus on staying fast and out of your way.

The editor targets the modern web stack out of the box — **PHP, Laravel, Blade,
Vue, Svelte, Tailwind/CSS** — alongside general-purpose languages.

## Features

- **Tree-sitter syntax highlighting** for 12+ languages
- **Language Server Protocol** — diagnostics, completion, hover, go-to-definition,
  find references, document & workspace symbols, formatting, rename, code actions
  and signature help, with per-language servers auto-selected
- **Laravel-aware completion** — `route()`, `view()`, `config()` and `env()` keys
- **Fuzzy file finder** (`⌘P`) and **command palette** (`⌘⇧P`)
- **Workspace search** across files (`⌘⇧F`) and **find-in-file** (`⌘F`)
- **Integrated terminal** — PTY-backed with ANSI colour, multiple tabs, rename and split
- **AI agent panel** — run Elyra, Claude Code, Codex or any CLI agent in a side panel (`⌘L`)
- **Split editor** (`⌘\`), **resizable panels** (drag the edges), **multi-cursor** (`⌘D`)
- **Git integration** — change gutter and inline diff vs `HEAD`
- **Inline diagnostics**, **bracket matching**, **snippets**, **breadcrumbs**
- **Markdown preview** (`⌘⇧M`)
- **Light / dark themes** (`F8`), **auto-save**, **format & trim on save**
- **Session persistence** — reopens your files, tabs and split layout per workspace
- **Workspace problems panel** — every diagnostic across the project, grouped and clickable
- **Built-in auto-updater** — detects new GitHub releases, shows the changelog, and installs in place

## Supported languages

Rust · Python · JavaScript · TypeScript · Go · C / C++ · JSON · PHP · HTML · CSS · Blade · Vue · Svelte

Language servers are launched automatically when available on your `PATH`:

| Language        | Server                |
| --------------- | --------------------- |
| PHP             | Intelephense          |
| Rust            | rust-analyzer         |
| C / C++         | clangd                |
| TypeScript / JS | typescript-language-server |
| Go              | gopls                 |
| Python          | pyright               |

## AI agents

The right-hand **Agent panel** (`⌘L`) runs a CLI coding agent in an embedded
terminal so it can work on your open project. Switch agents from the panel
header, and configure them in your global settings (`⌘,`):

```jsonc
{
  "agents": {
    "default": "elyra",
    "list": [
      { "id": "elyra",  "name": "Elyra",      "command": "elyra",  "cwd": "" },
      { "id": "claude", "name": "Claude Code", "command": "claude", "cwd": "" },
      { "id": "codex",  "name": "Codex",       "command": "codex",  "cwd": "" }
    ]
  }
}
```

- `command` is run through your login shell (`$SHELL -lc "<command>"`), so your
  full environment (PATH, nvm, …) is available.
- `cwd` defaults to the current workspace root when left empty.
- The default agent is **Elyra**; your selection is saved automatically.

## Keyboard shortcuts

> On macOS the modifier is `⌘`; on Linux/Windows use `Ctrl`.

| Shortcut   | Action                       |
| ---------- | ---------------------------- |
| `⌘P`       | Find file                    |
| `⌘⇧P`      | Command palette              |
| `⌘⇧F`      | Search in files              |
| `⌘⇧O`      | Go to symbol                 |
| `⌘F`       | Find in file                 |
| `⌘S`       | Save file                    |
| `⌘W`       | Close tab / terminal / agent |
| `⌘\`       | Split editor                 |
| `⌘D`       | Add cursor at next match     |
| `⌘T`       | Toggle terminal              |
| `⌘L`       | Toggle agent panel           |
| `⌘1`       | Toggle sidebar               |
| `⌘⇧M`      | Toggle markdown preview      |
| `⌘,`       | Open settings                |
| `⌘Space`   | Trigger completion           |
| `F1`       | Hover info                   |
| `F2`       | Rename                       |
| `F8`       | Toggle light / dark theme    |
| `F12`      | Go to definition             |
| `⇧F12`     | Find references              |

## Documentation

Full user documentation lives in [`docs/`](docs/README.md):

- [Installation](docs/installation.md) · [Getting started](docs/getting-started.md) · [Keyboard shortcuts](docs/keyboard-shortcuts.md)
- [Editing](docs/editing.md) · [Find & Replace](docs/find-and-replace.md) · [Navigation](docs/navigation.md)
- [Languages & LSP](docs/languages-and-lsp.md) · [Laravel](docs/laravel.md)
- [Source Control](docs/source-control.md) · [Terminal](docs/terminal.md) · [AI Agents](docs/agents.md)
- [Configuration](docs/configuration.md) · [Updating](docs/updating.md) · [Troubleshooting](docs/troubleshooting.md)

## Getting started

### Requirements

- [Rust](https://rustup.rs) 1.87 or newer
- A language server on your `PATH` for any language you want LSP features for
  (e.g. `intelephense`, `rust-analyzer`, `clangd`)

### Build & run

```sh
# Clone and build
git clone <repo-url> e
cd e
cargo build --release

# Run on a directory or file
cargo run --release -- path/to/project
```

On macOS, use the helper script to build, wrap the binary in a `.app` bundle and
bring the window to the front:

```sh
./scripts/run.sh path/to/project
```

To produce a distributable macOS app bundle:

```sh
./scripts/bundle-macos.sh
```

## Updating

`e` checks GitHub for a newer release on startup. When one is available, a
notice appears in the bottom-right corner with the changelog and an **Update
now** button — clicking it downloads the latest build for your platform and
replaces the running binary in place; restart `e` to finish.

You can also trigger a check manually from the command palette (`⌘⇧P` →
**Check for Updates**).

## Configuration

Global settings live in `~/.config/e/config.json` (open it with `⌘,`):

```jsonc
{
  "dark": true,
  "font_size": 14,
  "tab_width": 4,
  "format_on_save": true,
  "trim_on_save": true,
  "autosave": true,
  "indent_guides": true,
  "agents": { /* see "AI agents" above */ }
}
```

## Architecture

`e` is a Cargo workspace of focused crates:

| Crate     | Responsibility                                                       |
| --------- | -------------------------------------------------------------------- |
| `e-core`  | GUI-agnostic core: rope buffers, language detection, tree-sitter syntax, git diff, markdown |
| `e-lsp`   | Multi-server Language Server Protocol client                         |
| `e-term`  | PTY-backed terminal with a minimal VT100 screen model                |
| `e-app`   | The UI — editor, panels, palettes, theming, state                    |
| `e`       | The binary entry point                                               |

Run the test suite with:

```sh
cargo test --workspace
```

## Acknowledgements

Thanks to the maintainers of tree-sitter, the language servers, and the wider
Rust ecosystem that make `e` possible.

## License

Licensed under the [MIT License](LICENSE).

---

<div align="center">

**e** — The editor for the rest of us

Made with ♥ by [Knut W. Horne](https://kwhorne.com)

</div>
