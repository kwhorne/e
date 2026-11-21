# Terminal

`e` has an integrated, PTY-backed terminal with ANSI colour support.

## Opening & closing

- **`⌘T`** toggles the terminal panel (spawning your shell on first use).
- **`Ctrl+\``** also toggles it.
- **`⌘W`** (while the terminal is focused) closes the active terminal.

Your shell is taken from `$SHELL`.

## Pasting & copying

- **`⌘V`** pastes the clipboard (on Linux and Windows **`Ctrl+Shift+V`**, where
  `Ctrl+V` stays the shell's own; **`Shift+Insert`** everywhere). When the
  shell asks for *bracketed paste*, as zsh, bash and fish do, a pasted block
  arrives at the prompt as text, to edit or run with Enter, instead of each
  line running as it lands.
- **Right-click** the terminal for **Paste** and **Copy screen** (what it
  shows, as text).
- Other `⌘` chords go to the editor; they never type their letter into the
  shell.

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

## Git's editor

`git commit`, `git rebase -i` and anything else that opens `$EDITOR` open their
file in a tab of this window, not in vim at the bottom of the terminal. Close the
tab when you are done and the command carries on. See
[`e --wait`](getting-started.md#e-as-your-editor---wait).

## Notes

This is a pragmatic terminal that handles shells and ordinary command output
(printing, colour, cursor movement, common control sequences). It is not a full
terminal emulator, so complex full-screen TUIs may not render perfectly — for
those, the [AI agent panel](agents.md) runs agents in their own dedicated PTY.
