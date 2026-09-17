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
cards** (tool, one-line summary, running / done / error, a result preview),
dimmed reasoning, and **Copy** on every code block and reply.

**The composer card** at the bottom:

- The input is multi-line and word-wrapped and **grows** as you type (then
  scrolls). **Enter** sends, **Shift+Enter** inserts a newline. Typing while
  Elyra runs **steers** it.
- **＋** attaches context: the **active file** (`@path`, which Elyra reads), the
  **selection** (file and line range), or **all open files**.
- **‹/› Code / ◇ Ask**: Code lets Elyra read, edit and run; Ask asks it to
  answer and plan without touching anything.
- **The model chip** (`Claude Opus 5 · high ▾`) lists every model Elyra offers,
  grouped by provider, and the **thinking level** for models that have one.
  The choice is remembered and Elyra starts on it next time.
- **↑** sends, **■** stops the current turn.

Under the card: `⌂ Local  ▭ <project>` — where the agent runs and on what.

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
