# Source Control

`e` includes a git-powered Source Control panel, inline blame, and merge-conflict
resolution. All operations use the `git` command-line tool, so they behave
exactly like your terminal.

## The Source Control panel (`⌘2`)

Press **`⌘2`** to switch the sidebar to the Source Control panel (it opens the
sidebar if hidden). The panel shows:

- **Branch** — the current branch (`⎇ main`), with actions:
  - **⟳** refresh status
  - **↓** pull (`git pull --ff-only`)
  - **↑** push (`git push`)
- **Commit message** field — type a message and press `Enter` (or **Commit**).
- **Stage All** — stage every change.
- **STAGED CHANGES** — files staged for commit. Each row:
  - shows a coloured status badge (`M` modified, `A` added, `D` deleted),
  - opens the file when clicked,
  - **−** unstages it.
- **CHANGES** — unstaged and untracked files. Each row:
  - **+** stages it,
  - **↺** discards work-tree changes (`git checkout -- <file>`).

The panel refreshes automatically after saves, file operations, and git actions.

## Change gutter

Lines changed relative to `HEAD` are marked in the editor gutter (added vs
modified), so you can see edits at a glance.

## Diff vs HEAD

Run **Show Git Diff vs HEAD** from the command palette to view a unified diff of
the active file against the committed version.

## Inline blame

The status bar shows git blame for the line under the caret —
`author, 3 days ago • commit summary`. Uncommitted lines show
`You • Uncommitted changes`. Blame updates when you save.

## Merge conflicts

When the caret is inside a conflict block (`<<<<<<<` / `=======` / `>>>>>>>`), a
bar appears above the editor with one-click resolution:

- **Accept Current** — keep your side.
- **Accept Incoming** — keep the other side.
- **Accept Both** — keep both, removing the markers.

## Status bar

The status bar also shows the current branch, so you always know where you are —
even with the Source Control panel closed.
