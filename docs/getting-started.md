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

## The `e` command

To launch `e` from any directory, run **Install 'e' Command in PATH** from the
command palette (`⌘⇧P`). After that you can open any project from the shell:

```sh
e .            # open the current directory
e path/to/file # open a file
```

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

- Add more root folders with **Add Folder to Workspace** (multi-root); drag files
  from Finder into the window to open them.
- **Sidebar** (`⌘1`) — file explorer and document outline, or the Source Control
  panel (`⌘2`). Files show type-specific icons.
- **Editor** — tabs, breadcrumbs, and the code area. Split with `⌘\`.
- **Terminal** (`⌘T`) — an integrated terminal with tabs and split panes.
- **Agent panel** (`⌘L`) — run an AI coding agent beside your code.
- **Status bar** — cursor position, language, git branch, blame, diagnostics.

## Essential first steps

| Action | Shortcut |
| ------ | -------- |
| New file | `⌘N` |
| Open a file by name | `⌘P` |
| Run any command | `⌘⇧P` |
| Save (Save As… for new files) | `⌘S` |
| Find in file | `⌘F` |
| Search the whole project | `⌘⇧F` |
| Toggle the terminal | `⌘T` |
| Open settings | `⌘,` |

> The editor takes keyboard focus automatically when you create or open a file
> or switch tabs — just start typing, no click needed.

## Panels can be resized

Drag the edge between the sidebar/agent panel and the editor to resize them.

## Next steps

- Learn the [keyboard shortcuts](keyboard-shortcuts.md).
- Explore [editing features](editing.md) and [navigation](navigation.md).
- Set up [AI agents](agents.md) and [Source Control](source-control.md).
