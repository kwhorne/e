# Source Control

`e` includes a git-powered Source Control panel, inline blame, and merge-conflict
resolution. All operations use the `git` command-line tool, so they behave
exactly like your terminal.

## The Source Control panel (`⌘2`)

Press **`⌘2`** to switch the sidebar to the Source Control panel (it opens the
sidebar if hidden). The panel shows:

- **Branch** — click the branch name to switch branches; the row also has:
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

## Suggested commit messages

Click the ✨ button next to the message box to generate a Conventional Commits
subject line from your staged (or otherwise changed) files — e.g.
`feat(app): add settings`, `docs: update readme`, `fix: update parser`. It is a
starting point you can edit before committing.

## Session review (`⌘⌥V`)

When an agent has changed a lot of files, you don't want to push and review the
diff on GitHub — you didn't write the code, so you need to **understand, verify
and be able to undo it**. *Review: Session Changes* opens the whole changeset in
one place:

- **Risk-ranked** — migrations, `.env`, `config/`, `routes/`, auth/middleware/
  policies, CI workflows and dependency manifests come first; lockfiles, tests and
  docs sink to the bottom. Each row shows why (`migration`, `auth`, `lockfile`, …),
  the change kind (`A`/`M`/`D`/`R`) and `+N −M`.
- **Sign-off flow** — a progress counter (`12/50 reviewed`); **Reviewed →** ticks
  the current file and jumps to the next one needing attention.
- **Per file** — **Open** jumps to the first changed line, **Ask why** asks the
  agent to explain that specific change, **Revert** undoes just that file (deleting
  it if the session created it, otherwise restoring it from `HEAD`).
- **Summarize** asks the agent to describe the whole changeset and flag anything
  risky — a second pass over its own work.

The session starts at a git checkpoint taken when the agent launches. If no
session was recorded, the panel reviews **everything uncommitted** — so it works
equally well when the agent ran in an external terminal.

When you're happy, commit from the [Source Control panel](#the-source-control-panel-2)
as usual.

## Commit history & stash

The panel lists recent commits (hash, summary, author, time), and the Stash /
 Pop buttons stash and restore your working changes.

## Status bar

The status bar also shows the current branch, so you always know where you are —
even with the Source Control panel closed.
