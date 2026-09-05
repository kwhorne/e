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
- **Inlay hints** — inline type and parameter-name hints from the language server.

### Supported servers

| Language        | Server                                    |
| --------------- | ----------------------------------------- |
| PHP             | Intelephense **+ laravel/lsp** (Laravel)  |
| Blade           | laravel/lsp (Laravel)                     |
| Rust            | rust-analyzer                             |
| C / C++         | clangd                                    |
| TypeScript / JS | typescript-language-server                |
| Go              | gopls                                     |
| Python          | pyright                                   |

See [Installation](installation.md#language-servers) for install commands — or
just open a file: when a server's binary isn't on `PATH`, `e` names it and prints
the command that installs it instead of reporting a spawn failure.

Servers are launched per language, so a mixed project (e.g. PHP + TypeScript)
gets full support for each. A language can also run **several** servers at once —
see below.

### The Laravel language server

In a Laravel project, `e` runs the official
[`laravel/lsp`](https://github.com/laravel/lsp) **alongside** Intelephense, and
merges their answers: Intelephense for general PHP intelligence, `laravel/lsp`
for framework awareness. It also gives **Blade files a language server**, which
they otherwise wouldn't have.

Install it once:

```sh
composer global require laravel/lsp
```

Make sure Composer's global `vendor/bin` is on your `PATH`. If it isn't
installed, nothing breaks — you simply keep `e`'s built-in Laravel intelligence,
and `e` tells you the command that would install it (once per session, not once
per file). Install it while `e` is running and the next PHP or Blade file you
open picks it up; no restart needed.

Or let `e` do it: **Laravel: Install Intelephense** and **Laravel: Install Laravel
language server** in the command palette (`⌘⇧P`) run the install command in a
terminal tab and start the server the moment it appears. In a Laravel project
the status bar shows a missing PHP server whatever file is open, and clicking it
does the same.

It adds routes, views/Blade, translations, config, environment variables,
assets/Mix, middleware, Inertia, Livewire, auth/policies, container bindings and
validation rules — with completions, hovers, **diagnostics** (an unknown route or
missing view is now a squiggle) and **quick fixes**.

When the server is running it owns the `route()` / `view()` / `config()` / `env()`
contexts. Turn it off in **Settings → Laravel → “Laravel language server”** to fall
back to `e`'s built-in helpers instead (restart to apply).

#### How several servers share one file

| Request | Behaviour |
| ------- | --------- |
| Document sync (`didOpen`/`didChange`/`didSave`/`didClose`) | sent to **every** server |
| Completion, code actions | **merged** from all servers |
| Hover, go to definition | the **first** server with an answer |
| Formatting, rename | the **primary** (general-purpose) server only |
| Diagnostics | kept **per server** and merged, so one can't erase the other's |

### Server settings

`e` hands each server the settings it asks for over `workspace/configuration`
(and pushes them once after the handshake). Intelephense gets
`files.maxSize = 5 MB` instead of its 1 MB default, so a Laravel project's
Composer class map and — more usefully — the model helper Laravel Idea writes to
`vendor/_laravel_ide/` are indexed rather than skipped with an
"over the maximum file size" line.

### Server health

The status bar shows each server for the active file: `intelephense ✓`, what it
is busy with while it indexes (`intelephense: Indexing 42%`), or why it isn't
there (`laravel-lsp ↓ click to install`, `intelephense ✗ crashed`). Click it to
bring back whatever is down: a server that isn't installed has its install
command run in a terminal tab and starts the moment the binary appears on
`PATH`; a crashed one is started again.

A server that dies is restarted and handed every open document again, up to
three times per session; after that it is left down and you are told. A server
that can't start at all (a broken install, say) is left down until you click.

Messages a server sends about itself — an invalid license, a project too large
to index — appear as notifications. Edits a server asks for
(`workspace/applyEdit`, including from code actions that run a server command)
are applied to open buffers through the editor, and to other files on disk.

Columns are converted between the editor's UTF-8 bytes and the server's UTF-16
units, so squiggles, completions and renames land where they should on lines
with `æøå`, emoji or any other non-ASCII text. Servers that speak UTF-8
(rust-analyzer, clangd) are asked to, and skip the conversion.

### Nothing waits for a server

Servers start in the background: opening a file never waits for a handshake,
and in a Laravel project the PHP and Blade servers are started as soon as the
project opens, so they are usually ready before the first file is. Edits made
while a server starts are folded into the document it is handed when it comes
up. The status bar shows `intelephense …` until then.

Every keystroke sends the server only the bytes that changed (for servers that
support incremental sync — all the usual ones do), through a writer thread, so a
server that is slow to read never stalls typing. When several servers serve one
file they are asked in parallel, and a request the user has typed past is
cancelled (`$/cancelRequest`) rather than left to finish. Formatting, including
format-on-save, runs off the UI thread; if the file changes while the server is
formatting, the stale result is dropped rather than applied. The Laravel
query-builder lint (unknown columns against the live schema) runs 300 ms after
the last edit, on a background thread.

## Diagnostics

Errors and warnings appear as coloured squiggles under the code, with counts in
the status bar (`⨯ errors  ⚠ warnings`).

## Problems panel

The workspace problems panel collects every diagnostic across the project,
grouped by file. Click an entry to jump straight to the issue.

## Code actions & refactors

Press **`⌘.`** to request code actions from the language server at the cursor or
selection: quick fixes for diagnostics and refactors such as *extract variable*
and *extract method*. Pick one from the list to apply its edit. What's offered
depends on the server (e.g. rust-analyzer is rich here; some servers offer
little). Symbol rename is **`F2`**; a document **outline** of the active file
sits in the sidebar.

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
- Define your own snippets in `config.json` under `snippets` (per language):
  `"snippets": { "php": [ { "prefix": "dd", "body": "dd($0);" } ] }`.
- Signature help shows the active parameter as you type arguments.

### Inline AI completion

Optionally, `e` can show whole-line **AI suggestions** as grey “ghost text” at
the cursor, generated by a local [Ollama](https://ollama.com) code model:

- Enable it in **Settings → Editor** (`ai_completion` in `config.json`). It's off
  by default and **fully local** — nothing runs unless it's on *and* Ollama is
  reachable.
- After a short idle it requests a fill-in-the-middle completion and shows the
  next line inline; press **`Tab`** to accept, or keep typing / `Esc` to dismiss.
- The model is set via `E_COMPLETION_MODEL` (default `qwen2.5-coder`); pull one
  first, e.g. `ollama pull qwen2.5-coder`.
- Requests are debounced and run off the UI thread, so they never block editing,
  and they defer to the LSP completion popup when it's open.

## Bracket matching

The bracket matching the one next to the caret is highlighted, making it easy to
see scope boundaries.
