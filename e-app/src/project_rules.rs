//! The project's own rules (`.e/rules.toml`, see [`e_review::rules`]) in the
//! editor: a squiggle where a file breaks one as you type, a flag in Session
//! Review and at the ship gate, the whole project checked from the palette —
//! and every agent told them with its first message, and over the socket.

use std::path::Path;
use std::sync::Arc;

use e_lsp::path_to_uri;
use e_review::rules::{self, Breach, Level, Rules, RulesError};
use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

use crate::picker::{PickerItem, PickerMode};
use crate::state::AppState;

/// What **Project Rules: Open** writes when the project has none yet: the
/// shape of a rule, nothing enforced.
const STARTER: &str = r#"# This project's rules. `e` checks them as you type, flags a change that
# breaks one in Session Review (an `error` rule blocks the ship gate), and
# tells every agent them before it starts.
#
# says     what the rule says, as you'd tell a colleague (required)
# files    globs from the project root it covers; a directory covers what's
#          in it; leave out for every file
# except   globs it doesn't cover
# forbid   a pattern (regex) no covered file may contain
# require  a pattern each covered file must contain — or, with
#          per = "method", each public method
# level    "warning" (default) or "error"
#
# A rule with neither forbid nor require isn't checked; agents are told it.

# [[rule]]
# id = "no-unguarded-models"
# says = "Models list their $fillable; never $guarded = []"
# files = ["app/Models"]
# forbid = '\$guarded\s*=\s*\[\s*\]'

# [[rule]]
# id = "billing-authorizes"
# says = "Every Billing controller action checks authorization"
# files = ["app/Http/Controllers/Billing"]
# require = '\$this->authorize\(|Gate::'
# per = "method"
# level = "error"

# [[rule]]
# says = "Money is stored as integer cents, never a float"
"#;

/// `rel` with `/` between its parts, as the rules' globs are written.
fn slashed(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The rules as an agent reads them over the socket.
pub fn rules_json(rules: &Rules) -> serde_json::Value {
    rules
        .rules
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "says": r.says,
                "files": r.files,
                "level": match r.level {
                    Level::Error => "error",
                    Level::Warning => "warning",
                },
                "checked": r.checked(),
            })
        })
        .collect()
}

/// A squiggle for each rule `text`, the file at `rel`, breaks.
pub fn lint(rules: &Rules, rel: &str, text: &str) -> Vec<Diagnostic> {
    rules
        .check(rel, text)
        .into_iter()
        .map(|b| diagnostic(rules, &b))
        .collect()
}

fn diagnostic(rules: &Rules, b: &Breach) -> Diagnostic {
    let rule = &rules.rules[b.rule];
    let at = |(line, col): (usize, usize)| Position::new(line as u32, col as u32);
    Diagnostic {
        range: Range::new(at(b.start), at(b.end)),
        severity: Some(match rule.level {
            Level::Error => DiagnosticSeverity::ERROR,
            Level::Warning => DiagnosticSeverity::WARNING,
        }),
        source: Some("rules".to_string()),
        code: Some(NumberOrString::String(rule.id.clone())),
        message: rule.says.clone(),
        ..Default::default()
    }
}

/// The rules file itself: where it doesn't read.
pub fn lint_rules_file(text: &str) -> Vec<Diagnostic> {
    match rules::parse(text) {
        Ok(_) => Vec::new(),
        Err(RulesError { line, message }) => {
            let width = text.lines().nth(line).map_or(0, str::len) as u32;
            vec![Diagnostic {
                range: Range::new(
                    Position::new(line as u32, 0),
                    Position::new(line as u32, width),
                ),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("rules".to_string()),
                message,
                ..Default::default()
            }]
        }
    }
}

/// Every place in the project at `root` that breaks a rule, by file:
/// `(relative path, line, column, rule index)`. Walks what git doesn't
/// ignore.
pub fn check_project(root: &Path, rules: &Rules) -> Vec<(String, usize, usize, usize)> {
    let mut out = Vec::new();
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for entry in walk.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = slashed(rel);
        if !rules.applies(&rel) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for b in rules.check(&rel, &text) {
            out.push((rel.clone(), b.start.0, b.start.1, b.rule));
        }
    }
    out.sort();
    out
}

impl AppState {
    /// Read `.e/rules.toml` again, and re-lint what's open. At startup, when
    /// it is saved, and after a branch switch.
    pub fn load_project_rules(&self) {
        let root = self.root.get_untracked();
        let app = *self;
        self.spawn_bg(
            move || rules::load(&root),
            move |loaded: Result<Rules, RulesError>| {
                match loaded {
                    Ok(r) => {
                        if !r.is_empty() {
                            eprintln!("e: {} project rules", r.rules.len());
                        }
                        app.project_rules.set(Arc::new(r));
                    }
                    // The file shows why; keep the rules that last read.
                    Err(e) => eprintln!("e: {} line {}: {}", rules::FILE, e.line + 1, e.message),
                }
                app.relint_all();
            },
        );
    }

    /// A file was saved: if it is the rules file, the rules changed.
    pub(crate) fn rules_touch(&self, path: &Path) {
        if path == self.root.get_untracked().join(rules::FILE) {
            self.load_project_rules();
        }
    }

    /// The project-relative path `path` is checked as, when a rule covers it.
    pub(crate) fn rules_rel(&self, path: &Path) -> Option<String> {
        let rules = self.project_rules.get_untracked();
        let rel = slashed(path.strip_prefix(self.root.get_untracked()).ok()?);
        rules.applies(&rel).then_some(rel)
    }

    /// **Project Rules: Open** — the rules file, started with the shape of a
    /// rule when there is none.
    pub fn open_project_rules(&self) {
        let path = self.root.get_untracked().join(rules::FILE);
        if !path.exists() {
            let made = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::write(&path, STARTER));
            if let Err(e) = made {
                Self::notify(&format!("Couldn't create {}: {e}", rules::FILE));
                return;
            }
        }
        self.open_path(path);
    }

    /// **Project Rules: Check Project** — every place that breaks a rule, in
    /// the picker.
    pub fn check_project_rules(&self) {
        let rules = self.project_rules.get_untracked();
        if !rules.rules.iter().any(|r| r.checked()) {
            Self::notify(&format!(
                "No rules to check — write them in {} (Project Rules: Open)",
                rules::FILE
            ));
            return;
        }
        let root = self.root.get_untracked();
        let p = self.picker;
        p.mode.set(PickerMode::References);
        p.query.set(String::new());
        p.items.set(vec![PickerItem {
            label: "Checking…".into(),
            detail: String::new(),
            uri: String::new(),
            line: 0,
            char: 0,
        }]);
        p.selected.set(0);
        p.open.set(true);
        let send = create_ext_action(self.cx, move |items: Vec<PickerItem>| {
            if items.is_empty() {
                p.open.set(false);
                Self::notify("The project keeps every rule");
                return;
            }
            p.items.set(items);
            p.selected.set(0);
        });
        std::thread::spawn(move || {
            let items = check_project(&root, &rules)
                .into_iter()
                .map(|(rel, line, col, rule)| PickerItem {
                    label: format!("{rel}:{}", line + 1),
                    detail: rules.rules[rule].says.clone(),
                    uri: path_to_uri(&root.join(&rel)),
                    line: line as u32,
                    char: col as u32,
                })
                .collect();
            send(items);
        });
    }

    /// What an agent's first message starts with: the rules, when there are
    /// any.
    pub(crate) fn rules_preamble(&self) -> Option<String> {
        let rules = self.project_rules.get_untracked();
        (!rules.is_empty()).then(|| rules.for_agents())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_file_reads_and_enforces_nothing() {
        let r = rules::parse(STARTER).unwrap();
        assert!(r.is_empty());
        // Uncommented, its examples read too.
        let examples = &STARTER[STARTER.find("# [[rule]]").unwrap()..];
        let live: String = examples
            .lines()
            .map(|l| format!("{}\n", l.trim_start_matches("# ")))
            .collect();
        let r = rules::parse(&live).unwrap();
        assert_eq!(r.rules.len(), 3);
        assert!(lint_rules_file(&live).is_empty());
    }

    #[test]
    fn a_rules_file_that_doesnt_read_is_marked_on_its_line() {
        let d = lint_rules_file("[[rule]]\nsays = \"x\"\nper = \"class\"\nrequire = 'y'\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].range.start.line, 0);
        assert!(d[0].message.contains("\"file\" or \"method\""));
    }

    #[test]
    fn the_whole_project_is_checked_past_what_git_ignores() {
        let root = std::env::temp_dir().join(format!("e-rules-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app/Models")).unwrap();
        std::fs::create_dir_all(root.join("vendor/x/app/Models")).unwrap();
        let bad = "<?php\nclass A {\n    protected $guarded = [];\n}\n";
        std::fs::write(root.join("app/Models/A.php"), bad).unwrap();
        std::fs::write(root.join("app/Models/B.php"), "<?php\nclass B {}\n").unwrap();
        std::fs::write(root.join("vendor/x/app/Models/C.php"), bad).unwrap();
        let rules = rules::parse(
            "[[rule]]\nsays = \"no\"\nfiles = [\"app/Models\"]\nforbid = '\\$guarded\\s*=\\s*\\[\\]'\n",
        )
        .unwrap();
        assert_eq!(
            check_project(&root, &rules),
            vec![("app/Models/A.php".to_string(), 2, 14, 0)]
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
