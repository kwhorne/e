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
  risky — a second pass over its own work. The agent can send that write-up back
  over the [sync socket](agent-sync.md) (`review_summary`), and it becomes the
  pull-request description.

### Automated flags

Alongside your own reading, `e` inspects **the diff itself** and flags what an
agent tends to leave behind — shown per file (a coloured dot in the list) and
above the diff, each with an **Ask** button that sends the finding to the agent:

| Flag | Severity | Example |
| ---- | -------- | ------- |
| `debug-leftover` | warn | `dd(`, `dump(`, `console.log(`, `dbg!(` |
| `secret` | danger | a long quoted literal assigned to `token`/`password`/`api_key` |
| `sql-injection` | danger | `DB::raw("… $id")`, `whereRaw` with interpolation |
| `destructive-migration` | danger | `dropColumn`, `dropIfExists`, `truncate(` in a migration |
| `env-changed` | danger | a value changed in `.env` |
| `auth-removed` | danger | an `authorize(`/`Gate::`/`can:` line **deleted** |
| `unsafe` | danger | `shell_exec(`, `eval(`, `unsafe {` |
| `test-skipped` | warn | `it.only(`, `->skip(`, `#[ignore]` |
| `verification-disabled` | warn | `rejectUnauthorized: false`, `--no-verify`, `chmod 777` |
| `tests-removed` / `large-deletion` | warn | net test lines or a big one-sided deletion |
| `todo-added` / `sleep` | info | `TODO`/`FIXME`, a blocking `sleep(` |

The checks are call-aware, so `add(` isn't mistaken for `dd(` and `array(` isn't
mistaken for `ray(`.

### Ship it

The bar along the bottom is the **ship gate** — a verdict plus the reasons
behind it:

- **Ready** — every file reviewed, tests green, no flags.
- **Notes** — shippable, but something is loose (files unreviewed, tests not run,
  warnings outstanding).
- **Needs attention** — tests are failing or there are danger flags.

**Run tests** runs the project's suite (the same one the [TDD panel](laravel.md)
uses). **Commit & PR** then ships the changeset without leaving the editor:

1. creates a branch (e.g. `agent/app-models`, derived from what changed),
2. commits in **logical groups, in dependency order** — `chore(deps)`, then
   `feat(db)`, `chore(config)`, `feat(routes)`, `feat(auth)`, `feat`, `test`,
   `docs`, `ci` — each with a Conventional Commits subject,
3. pushes and sets the upstream,
4. opens a pull request via the [GitHub CLI](https://cli.github.com) (`gh`) with a
   description containing the summary, the grouped file list, and the review
   evidence (files reviewed, test result, flag counts).

So GitHub still gets the PR as the record for your team — but the review happened
here, where it could be run, verified and undone.

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
