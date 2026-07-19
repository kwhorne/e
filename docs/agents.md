# AI Agents

`e` can run command-line coding agents — **Elyra**, **Claude Code**, **Codex**,
or any CLI agent — in a dedicated side panel, so they work on your project right
next to your code.

## Opening the agent panel

- **`⌘L`** toggles the agent panel on the right.
- For **Elyra**, the panel is a [native chat](#native-chat-panel-elyra); other
  agents launch in an embedded terminal.
- **`⌘W`** (while the panel is focused) closes it; the agent keeps running, so
  `⌘L` reopens the same session.

## Native chat panel (Elyra)

Elyra runs headless over its structured RPC protocol (`elyra --mode rpc`) and the
conversation is drawn with native views instead of a terminal, so it stays fast
and readable no matter how long the transcript gets:

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

Other agents (Claude Code, Codex) use the terminal panel. To force the terminal
panel for Elyra too, set `"native_agent": false` in
[`config.json`](configuration.md).

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

## Tips

- Leave `cwd` empty so the agent operates on whatever project you have open.
- Add your own entries to `list` to run any CLI agent or script.
- The default agent is **Elyra**.
