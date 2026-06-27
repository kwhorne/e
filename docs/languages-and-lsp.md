# Languages & LSP

## Syntax highlighting

`e` uses [tree-sitter](https://tree-sitter.github.io/) for fast, accurate syntax
highlighting. Supported languages include:

Rust · Python · JavaScript · TypeScript · Go · C / C++ · JSON · TOML · PHP ·
HTML · CSS · Blade · Vue · Svelte · Markdown · Shell

The language is detected from the file extension (and a few special names, like
`Cargo.lock` and `*.blade.php`).

## Language Server Protocol

When a language server is available on your `PATH`, `e` launches it automatically
and provides:

- **Diagnostics** — errors and warnings shown inline (squiggles) and in the
  [problems panel](#problems-panel).
- **Completion** — context-aware suggestions (`⌘Space` to trigger manually).
- **Hover** — type and documentation popups (`F1`).
- **Go to definition** (`F12`) and **find references** (`⇧F12`).
- **Document & workspace symbols** (`⌘⇧O`).
- **Formatting** — on save, or via the "Format Document" command.
- **Rename** and **code actions** (where the server supports them).
- **Signature help** — parameter hints while typing a call.

### Supported servers

| Language        | Server                        |
| --------------- | ----------------------------- |
| PHP             | Intelephense                  |
| Rust            | rust-analyzer                 |
| C / C++         | clangd                        |
| TypeScript / JS | typescript-language-server    |
| Go              | gopls                         |
| Python          | pyright                       |

See [Installation](installation.md#language-servers) for install commands.

A different server is launched per language, so a mixed project (e.g. PHP +
TypeScript) gets full support for each.

## Diagnostics

Errors and warnings appear as coloured squiggles under the code, with counts in
the status bar (`⨯ errors  ⚠ warnings`).

## Problems panel

The workspace problems panel collects every diagnostic across the project,
grouped by file. Click an entry to jump straight to the issue.

## Completion, snippets & signatures

- Even without a language server, built-in completion offers language keywords,
  identifiers from the open file, and (for PHP/Blade) Laravel facades and Blade
  directives.
- Framework-aware completion: Flux UI components (`<flux:…>`), Livewire `wire:`
  directives, Tailwind utility classes (inside `class="…"`), and Vue/Svelte
  directives.
- Completion combines LSP suggestions with built-in **snippets** (per-language
  templates) and, for Laravel projects, [helper completions](laravel.md).
- Accepting a snippet places the caret at the first placeholder.
- Signature help shows the active parameter as you type arguments.

## Bracket matching

The bracket matching the one next to the caret is highlighted, making it easy to
see scope boundaries.
