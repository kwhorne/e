# AI Agents

`e` can run command-line coding agents — **Elyra**, **Claude Code**, **Codex**,
or any CLI agent — in a dedicated side panel, so they work on your project right
next to your code.

## Opening the agent panel

- **`⌘L`** toggles the agent panel on the right.
- On first open, the selected agent launches in an embedded terminal.
- **`⌘W`** (while the panel is focused) closes it; the agent process keeps
  running, so `⌘L` reopens the same session.

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

## Tips

- Leave `cwd` empty so the agent operates on whatever project you have open.
- Add your own entries to `list` to run any CLI agent or script.
- The default agent is **Elyra**.
