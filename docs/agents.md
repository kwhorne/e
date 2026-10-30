# AI Agents

`e` can run command-line coding agents — **Elyra**, **Claude Code**, **Codex**,
or any CLI agent — in a dedicated side panel, so they work on your project right
next to your code.

## Opening the agent panel

- **`⌘L`** toggles the agent panel on the right (it opens at 600px, resizable).
- Every agent runs in an **embedded terminal** by default. Elyra can optionally
  use [Elyra Cascade](#elyra-cascade), a chat panel.
- **`⌘W`** (while the panel is focused) closes it; the agent keeps running, so
  `⌘L` reopens the same session.

## Elyra Cascade

By default the agent panel is a terminal for **every** agent, including Elyra.
**Elyra Cascade** is the alternative: Elyra as a chat panel, drawn the way Devin
Local and Windsurf's Cascade draw theirs. Turn it on with **Settings → Agents →
Elyra Cascade**, the palette command **Agent: Elyra Cascade on/off**, or
`"cascade": true` in [`config.json`](configuration.md); the panel's **⋯** menu
has *Use the terminal panel instead* to go back. Other agents (Claude Code,
Codex) always use the terminal.

Underneath, Elyra runs headless over its RPC protocol (`elyra --mode rpc`), so
the conversation is drawn with native views and stays fast however long it
gets — and every provider and model Elyra knows is available.

**The header** is a session tab with **＋** (new session), **◷** (this
project's earlier sessions — pick one to resume it, with the conversation
redrawn), **⋯** (new session, compact context, restart, terminal panel,
settings) and **✕**.

**The empty state** shows the Elyra mark and *Describe your task to Elyra*;
before any API key is stored it says where to add one.

**The transcript**: streaming replies rendered as **markdown**, **tool-call
cards** (tool, one-line summary, running / done / error, a result preview —
while a tool runs, its latest output as it arrives, so a test run or a build
shows where it is), dimmed reasoning, and **Copy** on every code block and
reply.

**The composer card** at the bottom:

- The input is multi-line and word-wrapped and **grows** as you type (then
  scrolls). **Enter** sends, **Shift+Enter** inserts a newline.
- While Elyra works, **Enter** sends a message that **steers** it — it is
  read once the current tool calls finish — and **⌥Enter** queues one to run
  **next**, when it is done. What waits is listed above the input (*Steer* or
  *Next*), and joins the transcript where Elyra takes it up rather than where
  you typed it.

**A session in a worktree.** **⋯ → New session in a worktree** gives Elyra a
branch of its own (`e/agent-<date>-<time>`) beside your checkout: its own git
worktree, its own `.env`, its own copy of the database, migrated, and its own
site at `<site>--e-agent-….test`. Its migrations, test runs and half-done edits
stay off the files you are working in and the database you are using. With the
project served by [Grove](laravel.md#runtime-insight), Grove does the work
(`grove try`); `vendor/` and `node_modules/` are cloned from your checkout, so
it takes seconds.

Any other git project — a Rust or Go one, or a Laravel app not linked in Grove
— gets a plain git worktree under `~/.config/e/worktrees/`: its own branch and
files, with `.env` and `database/database.sqlite` copied over, but no site.
A Laravel app on MySQL or Postgres then shares your database, and Cascade
says so in red when the session starts; `grove link` the project to give its
worktrees a copy of their own.

While Elyra works in one, a line under the tab names the branch, with **Open
site** (for a Grove worktree), **Open in e** (the worktree's code in a new window) and **Back to the
project**. Turn receipts and the session review measure the worktree, not your
checkout.

The ⋯ menu also lists this project's earlier worktrees — *(merged)* once their
branch is merged into yours — to **Continue in** one or **Take down** one: its
site, database and worktree go; the branch stays. Neither Grove nor git will
take down a worktree with uncommitted work in it. If a worktree is taken down while Elyra
is working in it, Cascade says so and stops showing it.

**What each turn changed.** When Elyra finishes, a card under its reply says
what that turn changed — *This turn changed 3 files · +42 −7*, with ⚠ and
the number of [review flags](source-control.md#session-review-v) when there
are any — and lists the files:

- **Review** opens the session review on just that turn (*Review: one turn*,
  with **Whole session** to widen it again).
- **Undo** puts the turn's files back as they were before it, and removes the
  ones it created. A file changed again since — by you, or by a later turn —
  or open with unsaved edits is left alone, and the card says which.

Before a prompt goes to an idle Elyra, `e` takes a snapshot of the working
tree (tracked files and untracked ones that aren't ignored) as a git tree
object, and another when Elyra finishes; the card is the difference. An
untracked file over 10 MB — a database dump, a build artifact — is left out,
so it isn't copied into `.git/objects` on every turn. The
snapshot uses a copy of the index, so your staging area, HEAD and stash are
never touched. It needs the project to be a git repository.
- **＋** attaches context: the **active file** (`@path`, which Elyra reads), the
  **selection** (file and line range), or **all open files**.
- **‹/› Code / ◇ Ask**: Code lets Elyra read, edit and run; Ask asks it to
  answer and plan without touching anything.
- **The model chip** (`Claude Opus 5 · high ▾`) lists every model Elyra offers,
  grouped by provider, and the **thinking level** for models that have one.
  The choice is remembered and Elyra starts on it next time.
- **↑** sends, **■** stops the current turn.
- **Context and cost** — `42% context · $0.14` beside the model, once there
  is something to say: how full the model's context window is, and what the
  session has cost so far. It turns amber past 70% and red past 90%, when
  replies start to suffer; click it to **compact** the context now, before
  starting the next task. Elyra reports both after each turn.

Under the card: `⌂ Local  ▭ <project>` — where the agent runs and on what.

### Other agents in Cascade

Cascade runs other agents too, through the
[Agent Client Protocol](https://agentclientprotocol.com) (ACP). Pick one in the
`⋯` menu under **Agent**; the choice is saved as `cascade_agent` in
`config.json` and starts a new session.

| Agent          | Runs                                                   | Needs        |
| -------------- | ------------------------------------------------------ | ------------ |
| Elyra          | `elyra --mode rpc` (the default)                       | `elyra`      |
| Claude Code    | `npx -y @agentclientprotocol/claude-agent-acp@0.98.0`  | `claude`     |
| Codex          | `npx -y @agentclientprotocol/codex-acp@2.3.0`          | `codex`      |
| Gemini CLI     | `gemini --experimental-acp`                            | `gemini`     |
| GitHub Copilot | `copilot --acp`                                        | `copilot`    |
| Cursor         | `cursor-agent acp`                                     | `cursor-agent` |
| OpenCode       | `opencode acp`                                         | `opencode`   |
| Goose          | `goose acp`                                            | `goose`      |
| Qwen Code      | `qwen --experimental-acp`                              | `qwen`       |
| Kimi CLI       | `kimi acp`                                             | `kimi`       |

An agent whose program isn't on your `PATH` is marked *(not installed)*; sign in
to it once in a terminal (`claude`, `codex login`, …) before picking it. The
adapters run through `npx` are pinned to a version, so an update doesn't change
a session under you. Any other ACP agent works too: set `cascade_agent` to
`"custom"` and `cascade_custom_command` to its command line, run through your
login shell.

They get the same panel as Elyra — the transcript, tool cards, turn receipts
with **Review** and **Undo**, worktree sessions and the context meter (when the
agent reports usage). When one asks before acting, the question is a card in
the transcript with the agent's own choices (*Allow*, *Always allow*,
*Reject*…); **Stop** answers an open one as cancelled. What ACP doesn't have:

- **No steering.** A message sent while the agent works runs next, when it is
  done; `↵` queues it.
- **No model menu.** The agent's own settings choose the model.
- **No compaction from `e`**, and Elyra's API keys aren't passed on — each
  agent uses its own sign-in.

### API keys

**Settings → Agents** has a key field for **Anthropic**, **OpenAI**, **Gemini**
and **Grok (xAI)**. A key is stored in the macOS Keychain (elsewhere in
`~/.config/e/secrets.json`, mode 0600), never in `config.json`, and is shown
masked once saved (`••••1234`, with *Replace* and *Remove*). Cascade hands the
stored keys to Elyra as its environment variables (`ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`) when it starts, so a key
entered once is all it takes; a variable already exported in your shell still
wins. Elyra's own credentials (`/login` for Claude Pro/Max, ChatGPT or Copilot
subscriptions, and `~/.elyra/agent/auth.json`) keep working alongside.

## Switching agents

Click the agent name in the panel header (`Elyra ▾`) for a menu to:

- switch to another configured agent (the active one is marked `●`),
- **Restart Agent**,
- open **Settings…** (`⌘,`).

The header also has **⟳** (restart) and **×** (close) buttons. Drag the panel's
left edge to resize it.

## Configuration

Agents are configured in your global [`config.json`](configuration.md) under the
`agents` key:

```jsonc
{
  "agents": {
    "default": "elyra",
    "list": [
      { "id": "elyra",  "name": "Elyra",       "command": "elyra",  "cwd": "" },
      { "id": "claude", "name": "Claude Code",  "command": "claude", "cwd": "" },
      { "id": "codex",  "name": "Codex",        "command": "codex",  "cwd": "" }
    ]
  }
}
```

| Field     | Meaning |
| --------- | ------- |
| `id`      | Stable identifier used as the default-agent key |
| `name`    | Display name shown in the header |
| `command` | Command line, run through your login shell (`$SHELL -lc "<command>"`) |
| `cwd`     | Working directory — empty means the current workspace root |

Because the command runs through your login shell, your full environment
(`PATH`, nvm, etc.) is available. Your selection is saved automatically when you
switch agents from the menu.

## Editor co-op

The agent isn't just a terminal — it gets a local Unix socket
(`$E_EDITOR_SOCK`) to collaborate with the editor directly:

- **Read context** — the current file, cursor, selection, open files, and
  diagnostics.
- **Reuse the language server** — definitions, references, hover and symbols
  from the same server the editor runs.
- **Know the Laravel project** — routes, models with their live columns,
  Livewire components and Inertia pages in one answer, instead of grepping
  for them.
- **Follow the project's rules** — the rules in `.e/rules.toml` start a
  new Cascade session's first message, and any agent can read them, and
  where the project breaks them, over the socket. See
  [Project Rules](project-rules.md).
- **Measure its own work** — replay a route before and after a change and get
  the verdict: faster or slower, fewer queries, N+1 gone.
- **Query the database** through the editor's connection (consent-gated — you
  approve each query; the agent never sees your credentials).
- **Propose edits you review** — the agent sends a new version of a file and you
  accept or reject each hunk before anything is written. No blind writes.
- **Autonomous TDD** (`⌘⇧T`) — run the test suite and let the agent iterate on
  failures (proposing edits you review) until the tests pass. The panel lists
  each failing test with its first assertion line; click one to open the file at
  the failing line. See [Test results](laravel.md#test-results).
- **Timeline** (`⌘⌥A`) — an audit log of everything the agent did over the
  socket, with a 🤖 marker showing where it's looking.

See [Agent Workspace Sync](agent-sync.md) for the full protocol.

## Editor integration

Two conveniences tie the panel to the editor (toggle **Settings → Agents →
“Editor integration”**, on by default):

- **Click file paths in the output** — a `path`, `path:line` or `path:line:col`
  reference in the agent (or terminal) output is clickable and opens that file at
  the line in the editor.
- **Send selection to agent** — the *Agent: Send Selection to Agent* command
  (command palette) types a one-line reference to your current file and selected
  lines into the agent, so you can add your question and let it read the exact
  spot.

You can also **drag-select** any text in the panel and copy it with `⌘C` (click
to resume typing).

## Reviewing what the agent changed

When a session has touched a lot of files, press **`⌘⌥V`** (*Review: Session
Changes*) for a risk-ranked review of the whole changeset — sign off file by file,
ask the agent why it made a change, or revert a single file — without pushing a PR
first. See [Session review](source-control.md#session-review-v).

## Tips

- Leave `cwd` empty so the agent operates on whatever project you have open.
- Add your own entries to `list` to run any CLI agent or script.
- The default agent is **Elyra**.
