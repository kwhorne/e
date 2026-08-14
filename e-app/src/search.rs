//! One search dialog for everything (`⌘P`).
//!
//! Finding a file, a symbol, a command and a line of text used to be four
//! separate overlays with four separate shortcuts, which meant knowing which one
//! you wanted before you started typing. They are one dialog with tabs now, and
//! the backends behind them are the same ones as before.
//!
//! The list is heterogeneous — a file, a class, a command and a text hit are
//! different things — so everything becomes a [`Hit`] with a label, a detail and
//! what pressing Enter should do.

use std::path::PathBuf;

/// Which source the dialog is searching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    /// The default, and by far the most common thing to want.
    #[default]
    Files,
    Symbols,
    Actions,
    Text,
}

impl Tab {
    /// In the order they are shown. Files first because it is the default and
    /// by far the most common thing to want.
    pub const ALL: [Tab; 4] = [Tab::Files, Tab::Symbols, Tab::Actions, Tab::Text];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Files => "Files",
            Tab::Symbols => "Symbols",
            Tab::Actions => "Actions",
            Tab::Text => "Text",
        }
    }

    /// Whether results come from a background request rather than a list the
    /// dialog already holds. The two async tabs need a query before they can
    /// show anything.
    pub fn is_async(self) -> bool {
        matches!(self, Tab::Symbols | Tab::Text)
    }

    /// What an empty query should say, when the tab can't show anything yet.
    pub fn empty_hint(self) -> &'static str {
        match self {
            Tab::Symbols => "Type to search symbols",
            Tab::Text => "Type to search the project",
            _ => "",
        }
    }

    /// The next tab, for cycling with Tab.
    pub fn next(self) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + 1) % Tab::ALL.len()]
    }

    pub fn prev(self) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// What pressing Enter on a row does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Open(PathBuf),
    Goto {
        uri: String,
        /// 0-based.
        line: u32,
        col: u32,
    },
    Command(&'static str),
}

/// One row in the list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hit {
    /// The bit you read first — a filename, a symbol name, a command.
    pub label: String,
    /// Where it is, in dimmer text.
    pub detail: String,
    pub action: Action,
}

/// Rank the file list for `query`.
///
/// An empty query lists what is there, so opening the dialog and pressing Enter
/// still does something sensible.
pub fn file_hits(query: &str, files: &[PathBuf], root: &std::path::Path, limit: usize) -> Vec<Hit> {
    let hit_of = |p: &PathBuf| {
        let rel = p.strip_prefix(root).unwrap_or(p);
        let rel_s = rel.to_string_lossy().into_owned();
        let (name, dir) = match rel_s.rfind('/') {
            Some(i) => (rel_s[i + 1..].to_string(), rel_s[..i].to_string()),
            None => (rel_s.clone(), String::new()),
        };
        Hit {
            label: name,
            detail: dir,
            action: Action::Open(p.clone()),
        }
    };

    if query.is_empty() {
        return files.iter().take(limit).map(hit_of).collect();
    }
    let mut scored: Vec<(i32, String, &PathBuf)> = files
        .iter()
        .filter_map(|p| {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned();
            crate::fuzzy::match_path(query, &rel).map(|m| (m.score, rel, p))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.iter().map(|(_, _, p)| hit_of(p)).collect()
}

/// Rank the command list for `query`.
pub fn action_hits(
    query: &str,
    commands: &[(&'static str, &'static str)],
    limit: usize,
) -> Vec<Hit> {
    let hit_of = |(id, label): &(&'static str, &'static str)| Hit {
        label: (*label).to_string(),
        detail: String::new(),
        action: Action::Command(id),
    };
    if query.is_empty() {
        return commands.iter().take(limit).map(hit_of).collect();
    }
    let mut scored: Vec<(i32, usize, &(&'static str, &'static str))> = commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| crate::fuzzy::match_score(query, c.1).map(|m| (m.score, i, c)))
        .collect();
    // Score, then original order, so equal matches don't shuffle.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.iter().map(|(_, _, c)| hit_of(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn files(paths: &[&str]) -> Vec<PathBuf> {
        paths
            .iter()
            .map(|p| PathBuf::from("/proj").join(p))
            .collect()
    }

    #[test]
    fn tabs_cycle_both_ways_and_wrap() {
        assert_eq!(Tab::default(), Tab::Files);
        assert_eq!(Tab::Files.next(), Tab::Symbols);
        assert_eq!(Tab::Text.next(), Tab::Files, "the last wraps to the first");
        assert_eq!(Tab::Files.prev(), Tab::Text);
        // Round trip, so the two agree.
        for t in Tab::ALL {
            assert_eq!(t.next().prev(), t);
        }
    }

    #[test]
    fn only_the_async_tabs_need_a_query_first() {
        assert!(!Tab::Files.is_async());
        assert!(!Tab::Actions.is_async());
        assert!(Tab::Symbols.is_async());
        assert!(Tab::Text.is_async());
        // And those two say why they're empty rather than looking broken.
        assert!(!Tab::Symbols.empty_hint().is_empty());
        assert!(Tab::Files.empty_hint().is_empty());
    }

    #[test]
    fn a_file_hit_splits_the_name_from_its_directory() {
        let hits = file_hits(
            "order",
            &files(&["app/Models/Order.php"]),
            Path::new("/proj"),
            10,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "Order.php");
        assert_eq!(hits[0].detail, "app/Models");
        assert_eq!(
            hits[0].action,
            Action::Open(PathBuf::from("/proj/app/Models/Order.php"))
        );
    }

    #[test]
    fn file_hits_are_ranked_by_the_matcher() {
        let hits = file_hits(
            "oc",
            &files(&[
                "resources/views/documentation.blade.php",
                "app/Http/Controllers/OrderController.php",
            ]),
            Path::new("/proj"),
            10,
        );
        assert_eq!(hits[0].label, "OrderController.php");
    }

    #[test]
    fn an_empty_query_still_lists_files() {
        // Opening the dialog and pressing Enter should do something.
        let hits = file_hits("", &files(&["a.php", "b.php"]), Path::new("/proj"), 10);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn the_limit_is_respected_on_both_paths() {
        let many = files(&["a.php", "b.php", "c.php", "d.php"]);
        assert_eq!(file_hits("", &many, Path::new("/proj"), 2).len(), 2);
        assert_eq!(file_hits("php", &many, Path::new("/proj"), 2).len(), 2);
    }

    #[test]
    fn a_file_at_the_root_has_no_directory_detail() {
        let hits = file_hits("comp", &files(&["composer.json"]), Path::new("/proj"), 10);
        assert_eq!(hits[0].label, "composer.json");
        assert_eq!(hits[0].detail, "");
    }

    #[test]
    fn actions_rank_by_initials_like_everything_else() {
        let cmds = [
            ("move-class", "Refactor: Move Class…"),
            ("session-review", "Review: Session Changes"),
            ("toggle-terminal", "Toggle Terminal"),
        ];
        let hits = action_hits("rmc", &cmds, 10);
        assert_eq!(hits[0].label, "Refactor: Move Class…");
        assert_eq!(hits[0].action, Action::Command("move-class"));
    }

    #[test]
    fn an_action_that_does_not_match_is_dropped() {
        let cmds = [("a", "Alpha"), ("b", "Beta")];
        assert!(action_hits("zzz", &cmds, 10).is_empty());
        assert_eq!(action_hits("", &cmds, 10).len(), 2);
    }
}
