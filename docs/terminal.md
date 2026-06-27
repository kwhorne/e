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

The terminal grid resizes automatically to fit the panel; drag the panel border
or the window to change its size.

## Notes

This is a pragmatic terminal that handles shells and ordinary command output
(printing, colour, cursor movement, common control sequences). It is not a full
terminal emulator, so complex full-screen TUIs may not render perfectly — for
those, the [AI agent panel](agents.md) runs agents in their own dedicated PTY.
