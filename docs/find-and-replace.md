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
and matching line; selecting a result opens the file at that location.

## Tips

- Use the regex toggle (`.*`) for patterns, e.g. `fn \w+\(`.
- Combine whole-word with case-sensitive to find exact identifiers.
- Workspace search respects your project root (the directory you opened).
