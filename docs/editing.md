# Editing

`e` provides the editing primitives you expect from a modern code editor.

## Creating & saving files

- **`⌘N`** creates a new, empty **untitled** buffer — start typing right away.
- **`⌘S`** saves; for an untitled buffer it opens a **Save As** dialog, then
  reopens the file with the right language, LSP, and git support.
- **`⌘⇧S`** is Save As… for any buffer.

## Fonts

**Settings → Editor → Font** picks the code font for the editor, the terminal
and code shown in panels, with a preview line underneath. The menu lists the
fonts programmers use — JetBrains Mono, Fira Code, Cascadia Code, Source Code
Pro, IBM Plex Mono, Hack, Inconsolata, Monaco, Menlo, SF Mono, Consolas,
Ubuntu Mono, Roboto Mono, DejaVu Sans Mono, Courier New, Andale Mono, PT Mono,
Iosevka, Victor Mono, Geist Mono, Monaspace Neon, Berkeley Mono, Input Mono,
Noto Sans Mono, Liberation Mono — with the ones installed on your machine
selectable and the rest listed greyed, so a choice never silently falls back
to another font. **System monospace** uses the platform's default (Menlo on
macOS), and **Other…** takes any installed family by name.

**JetBrains Mono ships with `e`** (SIL Open Font License, see
`fonts/JetBrainsMono/OFL.txt`) and is the default, so code looks the same on
every machine. The setting is `font_family` in
[`config.json`](configuration.md); the size is `⌘=` / `⌘-` or **Font size** in
the same section.

## Markdown

A Markdown file wraps at the editor width, and `⌘⇧M` opens a live **preview
beside the editor** that follows your scrolling and updates as you type:
headings, paragraphs, quotes, bulleted, numbered and task lists (`- [ ]`,
`- [x]`), fenced code with a Copy button, tables, horizontal rules,
strikethrough, links (click to open — a URL in the browser, a relative `.md`
in the editor) and local images. Text in the preview is selectable.

Writing: **Enter** at the end of a list item, task or quote starts the next
one (numbered lists count on); Enter on an empty item ends the list. **`⌘B`**
and **`⌘I`** wrap the selection in `**`/`_` (or unwrap it), **`⌘K`** makes a
link from the selection, using the clipboard when it holds a URL, and
**`⌘⇧X`** toggles the task box on the line. Typing `` ` `` with a selection
wraps it in a code span.

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
- **`⌘⇧L`** selects *all* occurrences at once.
- **`⌥⌘↑` / `⌥⌘↓`** add a cursor on the line above / below at the same column
  (column editing).
- **`⌥`-click** adds a cursor with the mouse; **`⌥`-drag** adds a selection.

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

## Undo tree

Beyond the usual undo/redo (`⌘Z` / `⌘⇧Z`), `e` records every edit into a
**branching undo tree**. When you undo and then type something new, the old
branch isn't thrown away — it becomes a sibling you can return to.

Press **`⌘⌥U`** to open the tree. Each node shows how long ago it was and how
many characters changed; a `⑂` marks a branch point. Click any node to jump the
buffer to that state ("time travel"). The tree is saved per file under
`~/.config/e/undo/`, so history survives across sessions.

## Rename

Press **`F2`** to rename every whole-word occurrence of the identifier under the
caret within the current file. (Project-wide LSP rename depends on the language
server.)

## Code actions & refactors

Press **`⌘.`** to ask the language server for **code actions** at the cursor or
selection — quick fixes for diagnostics and refactors like *extract variable* /
*extract method* — then pick one to apply. Availability depends on the server.

## Compare files

Run **Compare Active File With…** from the command palette (`⌘⇧P`) to pick another
file and see a line-by-line diff of it against the current file. (For changes vs
git `HEAD`, use **Show Git Diff vs HEAD**.)

## Saving

- **`⌘S`** saves the active file.
- With **format on save** and **trim on save** enabled (the defaults), the file
  is formatted via the language server and trailing whitespace is trimmed before
  writing.
- **Auto-save** writes dirty buffers after a short idle period.
- **EditorConfig:** if the project has an `.editorconfig`, its `indent_size` /
  `tab_width` set the buffer's tab width, and `trim_trailing_whitespace` /
  `insert_final_newline` override the on-save settings for matching files.

See [Configuration](configuration.md) to adjust these.

## Emmet

In HTML, Blade, Vue, Svelte and PHP files, type an Emmet abbreviation and press
**Tab** to expand it (or run *Emmet: Expand Abbreviation* from the palette):

| Abbreviation | Expands to |
| ------------ | ---------- |
| `.card>h2{Title}+p` | a `div.card` containing an `<h2>` and a `<p>` |
| `ul>li.item$*3` | a `<ul>` with three `<li class="item1…3">` |
| `a` / `img` / `input` | tags with their default attributes |
| `nav>(a+a+a)` | grouping with `()` |

Supported: tags, `.class`, `#id`, `[attr=value]`, `{text}`, child `>`, sibling
`+`, grouping `()`, multiplication `*N` and `$` numbering. The caret lands at the
first sensible insertion point.

## Encoding & line endings

The status bar shows the file encoding and line-ending style. Click the **LF /
 CRLF** indicator to convert the buffer. Non-UTF-8 files are detected on open
(BOM or Windows-1252) and saved back in their original encoding.

## Unsaved changes & external edits

- Closing a tab with unsaved changes prompts you to **Save**, **Don't Save**, or
  **Cancel**.
- If a file changes on disk (e.g. after `git checkout`), clean buffers reload
  automatically. If you have unsaved edits, a bar offers to **Reload** (discard
  yours) or **Keep yours**.

## Rename (`F2`)

Put the caret on a symbol and press `F2`. `e` asks the language server what the
rename would change and shows you every site — the file, the line, and the line
as it will read — before anything is written:

```
app/Models/Order.php
  14: public $total;
     → public $amount;
  27: return $this->total * 1.25;
     → return $this->amount * 1.25;
```

**Rename** applies it; **Cancel** leaves everything alone. Files you have open
change through the buffer, so a rename is undoable like any other edit; the rest
are written to disk.

The language server is what makes this safe: it knows which occurrences are the
symbol and which are a word that happens to match inside a string or a comment,
and it sees the whole workspace rather than the file you're in.

Without a language server for the language — or when the server declines the
rename — `e` falls back to replacing whole-word matches in the current buffer.
That is textual, so check the preview.

## Move Class

*Refactor: Move Class…* in the command palette. It reads the active file's
fully-qualified name from its path, asks for the new one, and previews the move:

```
App\Models\Order → App\Domain\Order
app/Models/Order.php → app/Domain/Order.php · 3 references in 2 other files

app/Http/Controllers/OrderController.php  (2 references)
tests/Feature/OrderTest.php  (1 reference)
```

**Move** rewrites the referrers, writes the file at its new path, and removes the
old one. Referrers are written first: if one of those fails, the class is still
where every reference expects it.

PSR-4 from your `composer.json` decides where the file goes, including
`autoload-dev`, so moving a test class works the same way. A namespace no
PSR-4 entry covers is refused rather than written somewhere Composer will never
autoload it.

Matching is by whole name. Moving `App\Models\Order` leaves
`App\Models\OrderItem` alone, and `Legacy\App\Models\Order` is a different
class. Aliased imports keep their alias; a leading `\` is preserved. Change the
class name in the same move and its declaration and self-references follow —
without renaming a method that happens to share the name.

> References inside strings are not rewritten. A class name in a config array is
> indistinguishable from prose, and silently editing a string is the worse
> mistake. Search for the old name afterwards if you use them.
