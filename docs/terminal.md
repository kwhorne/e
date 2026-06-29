# Terminal

`e` has an integrated, PTY-backed terminal with ANSI colour support.

## Opening & closing

- **`⌘T`** toggles the terminal panel (spawning your shell on first use).
- **`Ctrl+\``** also toggles it.
- **`⌘W`** (while the terminal is focused) closes the active terminal.

Your shell is taken from `$SHELL`.

## Multiple terminals

The tab strip at the top of the panel manages multiple sessions:

- **`+`** — new terminal.
- **`×`** on a tab — close that terminal.
- Click a tab to focus it.
- **Right-click** a tab for **Rename**, **Split**, and **Close**.

## Split panes

- The **`⊟`** button (or right-click → **Split**) splits the terminal into two
  side-by-side panes, each with its own session and focus.

## Resizing

Drag the handle along the **top edge** of the terminal panel to change its
height. The terminal grid reflows automatically to fit.

## Scrollback & colours

Scroll up with the mouse wheel to review earlier output (5000 lines of
scrollback). The view stays put while new output streams and snaps back to the
bottom as soon as you type. ANSI **background colours** are rendered too, so
`git diff`, coloured error output and search tools keep their highlighting.

## Notes

This is a pragmatic terminal that handles shells and ordinary command output
(printing, colour, cursor movement, common control sequences). It is not a full
terminal emulator, so complex full-screen TUIs may not render perfectly — for
those, the [AI agent panel](agents.md) runs agents in their own dedicated PTY.
