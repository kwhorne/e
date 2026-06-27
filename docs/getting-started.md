# Getting started

This guide walks you through your first session in `e`.

## Opening a project

Launch `e` with a directory to open it as a **workspace**:

```sh
e ~/code/my-project
```

The directory becomes the root for the file tree, fuzzy file finder, workspace
search, and Source Control. You can also:

- Press **⌘O** to open another project in a new window.
- Run **Open File…** from the command palette to open any file in the current window.

## The interface

```
┌───────────┬─────────────────────────────────────┬───────────────┐
│           │  tabs                               │               │
│  sidebar  │  breadcrumbs                        │  agent panel  │
│  (⌘1)     │                                     │  (⌘L)         │
│           │            editor                   │               │
│  explorer │                                     │               │
│  or git   │─────────────────────────────────────│               │
│  (⌘2)     │  terminal (⌘T)                      │               │
│           │─────────────────────────────────────│               │
│           │  status bar                         │               │
└───────────┴─────────────────────────────────────┴───────────────┘
```

- **Sidebar** (`⌘1`) — file explorer and document outline, or the Source Control
  panel (`⌘2`).
- **Editor** — tabs, breadcrumbs, and the code area. Split with `⌘\`.
- **Terminal** (`⌘T`) — an integrated terminal with tabs and split panes.
- **Agent panel** (`⌘L`) — run an AI coding agent beside your code.
- **Status bar** — cursor position, language, git branch, blame, diagnostics.

## Essential first steps

| Action | Shortcut |
| ------ | -------- |
| Open a file by name | `⌘P` |
| Run any command | `⌘⇧P` |
| Save | `⌘S` |
| Find in file | `⌘F` |
| Search the whole project | `⌘⇧F` |
| Toggle the terminal | `⌘T` |
| Open settings | `⌘,` |

## Panels can be resized

Drag the edge between the sidebar/agent panel and the editor to resize them.

## Next steps

- Learn the [keyboard shortcuts](keyboard-shortcuts.md).
- Explore [editing features](editing.md) and [navigation](navigation.md).
- Set up [AI agents](agents.md) and [Source Control](source-control.md).
