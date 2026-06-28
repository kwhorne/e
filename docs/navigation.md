# Navigation

## Fuzzy file finder (`⌘P`)

Type part of a file name or path to jump to any file in the workspace. Matching
is fuzzy (`wbp` finds `welcome.blade.php`) and ranked — file-name matches and
shorter paths rank highest. The index is built in the background, so the finder
opens instantly even on a large folder. Use `↑` / `↓` to select and `Enter` to
open.

## Recent files (`⌘E`)

A most-recently-used list of the files you've opened this session, newest first
(up to 10). The previous file is preselected, so `⌘E` then `Enter` quickly
toggles back to it. Type to filter the list.

## Command palette (`⌘⇧P`)

Run any command by name — every shortcut in `e` is listed here, including
zoom, theme, terminal, source control, "Open File…", "Check for Updates", and
more.

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
