# Navigation

## Search (`⌘P`)

One dialog for four things, with tabs across the top:

| Tab | Searches |
| --- | --- |
| **Files** | every file in the workspace (the default) |
| **Symbols** | workspace symbols, from the language server |
| **Actions** | every command, the same list as `⌘⇧P` |
| **Text** | the contents of the project |

`Tab` moves to the next source, `⇧Tab` back, or click one. `↑` / `↓` select and
`Enter` acts — open the file, jump to the symbol or line, run the command.

### Matching

Matching is fuzzy and, more usefully, it can see where words begin. Typing the
capitals of a name finds it:

```
oc    → app/Http/Controllers/OrderController.php
prlc  → app/Http/Controllers/PasswordResetLinkController.php
cot   → database/migrations/…_create_orders_table.php
```

Boundaries count at `/`, `_`, `-`, `.` and at a lower→upper transition inside a
name, so CamelCase, snake_case and kebab-case all work the same way. Typing in
capitals is a hint, not a filter: `OC` and `oc` both match, but an exact-case hit
ranks higher.

The file name outranks its directory — searching `order` finds `Order.php`
before everything under `orders/` — and shorter, shallower paths win ties.

Files and Actions list everything when the box is empty, so `⌘P` then `Enter`
still does something. Symbols and Text need something to search for, and say so.
The file index is built in the background, so the dialog opens instantly even on
a large folder.

## Recent files (`⌘E`)

A most-recently-used list of the files you've opened this session, newest first
(up to 10). The previous file is preselected, so `⌘E` then `Enter` quickly
toggles back to it. Type to filter the list.

## Command palette (`⌘⇧P`)

Run any command by name — every shortcut in `e` is listed here, including
zoom, theme, terminal, source control, "Open File…", "Check for Updates", and
more. Matching is fuzzy and ranked: type `up` and "Check for Updates" and "Move
Line Up" rise to the top. `↑`/`↓` move the selection and `Enter` runs it.

## Go to line (`⌃G`)

Enter a line number, or `line:col`, to move the caret there.

## Go to symbol (`⌘⇧O`)

Search symbols (functions, classes, methods) across the workspace using the
language server, and jump to their definitions.

## Go to definition & references

| Action | Shortcut |
| ------ | -------- |
| Go to definition | `F12` |
| Find references | `⇧F12` |
| Hover info | `F1` |

## Navigation history

After jumping (go-to-definition, find references, symbol search), retrace your
steps:

| Action | Shortcut |
| ------ | -------- |
| Go back | `⌃-` |
| Go forward | `⌃⇧-` |

## Breadcrumbs

The breadcrumb bar above the editor shows the path to the file and the symbol
at the caret, giving you context within large files.

## Outline

The document outline in the sidebar lists the symbols in the current file
(from the language server). Click an entry to jump to it.

## Semantic search

`⌘⌥K` opens **"describe what you're looking for"** search. Type a question like
*"where is the invoice email sent"* and `e` ranks project locations by meaning.

It runs **entirely locally**. If a local [Ollama](https://ollama.com) server is
running it embeds your code and query with a real embedding model
(`nomic-embed-text` by default, override with `E_EMBED_MODEL`) for genuine
semantic matches. Otherwise it falls back to a fast lexical index — no cloud, no
data leaves your machine either way.
