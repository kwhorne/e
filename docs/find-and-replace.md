# Find & Replace

## Find in file (`⌘F`)

Opens the find bar in the top-right of the editor. Type to search the active
file; matches are highlighted and the count is shown (e.g. `3 / 12`).

- **`↓` / `↑`** — jump to the next / previous match.
- **`Enter`** — go to the next match.
- **`Esc`** — close the find bar.

### Search options

The find bar has three toggles:

| Toggle | Meaning |
| ------ | ------- |
| `Aa`   | Match case |
| `W`    | Whole word |
| `.*`   | Regular expression |

Results update live as you type or change options.

## Replace in file (`⌥⌘F`)

Opens the find bar with the replace row expanded:

- **Replace** — replace the current match and advance.
- **All** — replace every match in one step.

The replacement text is inserted literally.

## Search in files (`⌘⇧F`)

Searches the entire workspace. Results are shown in a picker with the file path
and matching line; selecting a result opens the file at that location. Every
occurrence is listed, including several on the same line.

**Aa** toggles case sensitivity. The query is matched literally, so
`$user->name()` finds that text rather than being read as a pattern.

### Replace All

Type into the Replace row and press **Replace All**. Before anything is
written, `e` shows what it is about to do — how many matches, in how many
files, which files, and under which case setting. Nothing touches disk until
you confirm, and the confirmation reports exactly the set of matches shown in
the results list: the search and the replace use the same matcher.

Replace All rewrites files on disk directly. Files that are open in the editor
reload afterwards, but the change is **not** undoable from the editor — commit
or stash first if you want a way back.

### What gets searched

Search and replace skip anything your ignore rules exclude — `.gitignore`, the
global gitignore, and `.git/info/exclude` — so a Laravel `vendor/`, `storage/`
or `public/build/` is never searched and never rewritten. A `vendor/` directory
that you actually track stays searchable. `.git`, `target` and `node_modules`
are always skipped, even in a project with no ignore rules, as are hidden
files, binaries, and files over 2 MB.

## Tips

- Use the regex toggle (`.*`) in the **in-file** find bar for patterns, e.g. `fn \w+\(`.
- Combine whole-word with case-sensitive to find exact identifiers.
- Workspace search covers every root folder of a multi-root workspace.
