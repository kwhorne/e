# AI Agents

`e` can run command-line coding agents — **Elyra**, **Claude Code**, **Codex**,
or any CLI agent — in a dedicated side panel, so they work on your project right
next to your code.

## Opening the agent panel

- **`⌘L`** toggles the agent panel on the right (it opens at 600px, resizable).
- Every agent runs in an **embedded terminal** by default. Elyra can optionally
  use an experimental [native chat panel](#native-chat-panel-elyra-experimental).
- **`⌘W`** (while the panel is focused) closes it; the agent keeps running, so
  `⌘L` reopens the same session.

## Native chat panel (Elyra, experimental)

By default the agent panel is a terminal for **every** agent, including Elyra.
Elyra can *optionally* be rendered as a native chat panel instead: turn on
**Settings → Agents → “Native Elyra chat”** (or set `"native_agent": true` in
[`config.json`](configuration.md)). It is **off by default** while the underlying
text/input views mature.

When enabled, Elyra runs headless over its structured RPC protocol
(`elyra --mode rpc`) and the conversation is drawn with native views instead of a
terminal, so it stays fast and readable no matter how long the transcript gets:

- **Streaming replies** render as formatted **markdown** — headings, lists,
  inline code, fenced code blocks and links.
- **Tool-call cards** show each tool, a one-line summary of its arguments, its
  status (running / done / error) and a compact result preview.
- The **composer** is multi-line and word-wrapped, and **grows** as you type
  (then scrolls). **Enter** sends, **Shift+Enter** inserts a newline. It is
  focused automatically when the panel opens.
- **Stop** aborts the current turn; typing while it runs **steers** it. **New
  Chat** (in the header menu) starts a fresh session.
- **Copy** buttons sit on every code block and under each reply.

Other agents (Claude Code, Codex) always use the terminal panel. Turn the toggle
back off to run Elyra in the terminal too (the default).

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
  failures (proposing edits you review) until the tests pass.
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

## Tips

- Leave `cwd` empty so the agent operates on whatever project you have open.
- Add your own entries to `list` to run any CLI agent or script.
- The default agent is **Elyra**.
