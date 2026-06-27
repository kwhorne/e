# Editing

`e` provides the editing primitives you expect from a modern code editor.

## Line operations

| Action | Shortcut |
| ------ | -------- |
| Duplicate line | `⌘D` (or `⇧⌥↓`) |
| Move line up / down | `⌥↑` / `⌥↓` |
| Delete line | `⌘⇧K` |
| Indent / outdent | `⌘]` / `⌘[` |

Line operations act on the line containing the caret, or on every line touched
by the current selection.

## Comments

Toggle line comments for the current line or selection with **`⌘/`**. The comment
token is chosen from the file's language (`//` for Rust/JS/TS/PHP/C/Go,
`#` for Python/Shell/TOML, and so on). Toggling again removes them.

## Multiple cursors

- **`⌘⇧D`** adds a cursor at the next occurrence of the current word or selection.
  Repeat to keep adding cursors, then edit them all at once.

## Auto-closing brackets & quotes

As you type `(`, `[`, `{`, `"`, `'`, or `` ` ``, the matching closing character is
inserted automatically and the caret is placed between them. In addition:

- **Type-over:** typing a closing character when it already follows the caret
  steps over it instead of inserting a duplicate.
- **Wrap selection:** with text selected, typing an opening bracket or quote
  wraps the selection.
- **Smart backspace:** deleting the opening half of an empty pair removes both
  characters.
- Apostrophes after a word (e.g. `don't`) are not auto-closed.

Disable this behaviour with `"auto_close": false` in your
[configuration](configuration.md).

## Auto-indent

Pressing **Enter** keeps the indentation of the current line, so new lines line
up with the code above.

## Rename

Press **`F2`** to rename every whole-word occurrence of the identifier under the
caret within the current file. (Project-wide LSP rename depends on the language
server.)

## Saving

- **`⌘S`** saves the active file.
- With **format on save** and **trim on save** enabled (the defaults), the file
  is formatted via the language server and trailing whitespace is trimmed before
  writing.
- **Auto-save** writes dirty buffers after a short idle period.

See [Configuration](configuration.md) to adjust these.

## Unsaved changes & external edits

- Closing a tab with unsaved changes prompts you to **Save**, **Don't Save**, or
  **Cancel**.
- If a file changes on disk (e.g. after `git checkout`), clean buffers reload
  automatically. If you have unsaved edits, a bar offers to **Reload** (discard
  yours) or **Keep yours**.
