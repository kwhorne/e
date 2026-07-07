# Keyboard shortcuts

The modifier is `⌘` (Command) on macOS; use `Ctrl` on Linux/Windows.

Every shortcut below is also available by name from the command palette (`⌘⇧P`).

## Files & navigation

| Shortcut | Action |
| -------- | ------ |
| `⌘N`     | New file |
| `⌘P`     | Find file (fuzzy) |
| `⌘E`     | Recent files (most recently used) |
| `⌘O`     | Open folder / project (new window) |
| `⌘⇧P`    | Command palette |
| `⌘⇧O`    | Go to symbol in workspace |
| `⌃G`     | Go to line (`line` or `line:col`) |
| `⌃-`     | Go back |
| `⌃⇧-`    | Go forward |
| `F12`    | Go to definition |
| `⇧F12`   | Find references |
| `F1`     | Hover info |

## Editing

| Shortcut | Action |
| -------- | ------ |
| `⌘S`     | Save file |
| `⌘⇧S`    | Save as… |
| `⌘W`     | Close tab / terminal / agent |
| `⌘D`     | Duplicate line |
| `⌘⇧D`    | Add cursor at next occurrence |
| `⌥⌘↑` / `⌥⌘↓` | Add cursor above / below (column editing) |
| `⌥↑` / `⌥↓` | Move line up / down |
| `⇧⌥↓`    | Duplicate line down |
| `⌘⇧K`    | Delete line |
| `⌘]` / `⌘[` | Indent / outdent |
| `⌘/`     | Toggle line comment |
| `⌘⇧L`    | Select all occurrences |
| `⌘⌥U`    | Toggle the visual undo tree |
| `F2`     | Rename symbol |
| `⌘.`     | Code actions / refactor (quick fixes, extract) |
| `⌘Space` | Trigger completion |
| `Tab`    | Expand Emmet abbreviation (HTML/Blade/Vue/Svelte) |

Brackets and quotes auto-close as you type; typing the closing character types
over it, and backspace deletes an empty pair.

## Find & replace

| Shortcut | Action |
| -------- | ------ |
| `⌘F`     | Find in file |
| `⌥⌘F`    | Replace in file |
| `⌘⇧F`    | Search in files (workspace) |
| `↑` / `↓` | Previous / next match (in the find bar) |

The find bar has toggles for **case-sensitive** (`Aa`), **whole-word** (`W`) and
**regular expressions** (`.*`).

## View & panels

| Shortcut | Action |
| -------- | ------ |
| `⌘1`     | Toggle sidebar |
| `⌘2`     | Toggle Source Control panel |
| `⌘3`     | Toggle database panel |
| `⌘↵`     | Run SQL under cursor (in a PHP query string) |
| `⌥⌘↵`    | Explain SQL under cursor (flags full scans / missing indexes) |
| `⌘T`     | Toggle terminal |
| `⌘⇧B`    | Run task |
| `⌘L`     | Toggle agent panel |
| `⌘\`     | Split editor |
| `⌘⇧M`    | Toggle markdown preview |
| `⌘=` / `⌘-` | Zoom in / out |
| `⌘0`     | Reset zoom |
| `⌥Z`     | Toggle word wrap |
| `F8`     | Toggle light / dark theme |
| `⌘,`     | Open settings |

## AI & Laravel

| Shortcut | Action |
| -------- | ------ |
| `⌘⌥K`    | Semantic search ("describe what you're looking for") |
| `⌘⌥M`    | Laravel architecture map (route → controller → view/page, request-replay) |
| `⌘⌥R`    | Eloquent relationship graph |
| `⌘⌥G`    | Event dispatch graph |
| `⌘⌥E`    | Related files (model / migration / …) |
| `⌘⌥C`    | Inertia props contract / generate TypeScript |
| `⌘⌥J`    | Livewire: switch between view and class |
| `⌘⌥I`    | Runtime insight (Telescope-style capture) |
| `⌘⌥L`    | Laravel log tail |
| `⌘⌥T`    | Tinker scratchpad |
| `⌘⇧T`    | Autonomous TDD panel |
| `⌘⌥A`    | Agent activity timeline |

**Schema diff** (migrations vs live DB) and **Laravel: refresh** are available
from the command palette (`⌘⇧P`). See [Laravel](laravel.md) and
[AI Agents](agents.md).

## Debugging

| Shortcut | Action |
| -------- | ------ |
| `F5`     | Start / continue |
| `F9`     | Toggle breakpoint on the caret line |
| `F10`    | Step over |
| `F11`    | Step into |
| `⇧F11`   | Step out |

`⌥`-click in the editor also toggles a breakpoint on that line. The Debug panel
and `Debug: Stop` are available from the command palette (`⌘⇧P`). See
[Debugging](debugging.md).

## Source Control

The Source Control panel (`⌘2`) provides stage, unstage, discard, stage-all,
commit, push, and pull. See [Source Control](source-control.md).

## Other

| Shortcut | Action |
| -------- | ------ |
| `Esc`    | Close palettes, dialogs, find/replace, prompts |
| `Ctrl+\`` | Toggle terminal (alternative) |
