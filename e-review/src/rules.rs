//! The project's own rules: invariants a team writes down once, in
//! `.e/rules.toml`, and `e` holds every change to — what you type, what an
//! agent wrote, what is about to be committed.
//!
//! ```toml
//! [[rule]]
//! id = "no-unguarded-models"
//! says = "Models list their $fillable; never $guarded = []"
//! files = ["app/Models/**"]
//! forbid = '\$guarded\s*=\s*\[\s*\]'
//!
//! [[rule]]
//! id = "billing-authorizes"
//! says = "Every Billing controller action checks authorization"
//! files = ["app/Http/Controllers/Billing/**"]
//! require = '\$this->authorize\(|Gate::'
//! per = "method"
//! level = "error"
//!
//! [[rule]]
//! says = "Money is integer cents, never a float"
//! ```
//!
//! `forbid` is a pattern no file in `files` may contain; `require` one each
//! file — or, `per = "method"`, each public method — must. A rule with
//! neither is guidance: agents are told it, nothing checks it. An `error`
//! rule broken in a changeset blocks the ship gate.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::Deserialize;

use crate::flags::{added_lines, Flag, Severity};
use crate::{ChangeKind, Changeset};

/// Where the rules live, from the project root.
pub const FILE: &str = ".e/rules.toml";

/// Never checked, whatever `files` says: not the project's own code.
const NEVER: &[&str] = &["vendor/", "node_modules/", ".git/", "storage/"];

/// How much a broken rule matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Shown, counted, not blocking.
    Warning,
    /// Blocks the ship gate until fixed or deliberately overridden.
    Error,
}

/// One rule.
#[derive(Clone, Debug)]
pub struct Rule {
    /// `no-unguarded-models`; `rule-3` when the file gives none.
    pub id: String,
    /// What the rule says, as a person would.
    pub says: String,
    /// The globs it covers, as written; empty for every file.
    pub files: Vec<String>,
    pub level: Level,
    covers: Option<GlobSet>,
    except: Option<GlobSet>,
    forbid: Option<Regex>,
    require: Option<Regex>,
    per_method: bool,
}

impl Rule {
    /// Whether `e` checks it, or only tells agents.
    pub fn checked(&self) -> bool {
        self.forbid.is_some() || self.require.is_some()
    }

    /// Whether the file at `rel` (from the project root, `/`-separated) is
    /// one the rule covers.
    pub fn covers(&self, rel: &str) -> bool {
        if NEVER.iter().any(|n| rel.starts_with(n)) {
            return false;
        }
        self.covers.as_ref().is_none_or(|g| g.is_match(rel))
            && !self.except.as_ref().is_some_and(|g| g.is_match(rel))
    }
}

/// The rules of a project.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub rules: Vec<Rule>,
}

/// Why `.e/rules.toml` couldn't be read: on which line (0-based), and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulesError {
    pub line: usize,
    pub message: String,
}

/// Where a rule is broken in a file: 0-based lines, byte columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breach {
    /// Index into [`Rules::rules`].
    pub rule: usize,
    pub start: (usize, usize),
    pub end: (usize, usize),
    /// The lines the breach is about — a method's whole body for a `require`
    /// per method — so a review can tell whether a change touched it.
    pub span: (usize, usize),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    rule: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: Option<String>,
    says: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    except: Vec<String>,
    forbid: Option<String>,
    require: Option<String>,
    per: Option<String>,
    level: Option<String>,
}

/// The rules in `text`, the content of `.e/rules.toml`.
pub fn parse(text: &str) -> Result<Rules, RulesError> {
    let line_of = |off: usize| text[..off.min(text.len())].matches('\n').count();
    let raw: RawFile = toml::from_str(text).map_err(|e| RulesError {
        line: e.span().map_or(0, |s| line_of(s.start)),
        message: e.message().trim().to_string(),
    })?;
    // Where each `[[rule]]` starts, for errors no parser sees.
    let heads: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("[[rule]]"))
        .map(|(i, _)| i)
        .collect();
    let mut rules = Vec::new();
    for (i, r) in raw.rule.into_iter().enumerate() {
        let at = heads.get(i).copied().unwrap_or(0);
        let fail = |message: String| RulesError { line: at, message };
        let id = r.id.unwrap_or_else(|| format!("rule-{}", i + 1));
        let pattern = |p: Option<String>, what: &str| {
            p.map(|p| {
                Regex::new(&format!("(?m){p}"))
                    .map_err(|e| fail(format!("{id}: `{what}` isn't a pattern: {e}")))
            })
            .transpose()
        };
        let forbid = pattern(r.forbid, "forbid")?;
        let require = pattern(r.require, "require")?;
        let per_method = match r.per.as_deref() {
            None | Some("file") => false,
            Some("method") => true,
            Some(other) => {
                return Err(fail(format!(
                    "{id}: per = \"{other}\"; it is \"file\" or \"method\""
                )))
            }
        };
        if per_method && require.is_none() {
            return Err(fail(format!("{id}: per = \"method\" goes with `require`")));
        }
        let level = match r.level.as_deref() {
            None | Some("warning") => Level::Warning,
            Some("error") => Level::Error,
            Some(other) => {
                return Err(fail(format!(
                    "{id}: level = \"{other}\"; it is \"warning\" or \"error\""
                )))
            }
        };
        let globs = |list: &[String]| -> Result<Option<GlobSet>, RulesError> {
            if list.is_empty() {
                return Ok(None);
            }
            let mut set = GlobSetBuilder::new();
            for g in list {
                let g = g.trim_start_matches("./").trim_end_matches('/');
                // A directory covers what's in it; a bare name, that file
                // anywhere.
                let mut forms = vec![g.to_string()];
                if !g.contains(['*', '?', '[', '{']) {
                    forms.push(format!("{g}/**"));
                }
                if !g.contains('/') {
                    forms.push(format!("**/{g}"));
                }
                for f in forms {
                    let glob = GlobBuilder::new(&f)
                        .literal_separator(true)
                        .build()
                        .map_err(|e| fail(format!("{id}: {e}")))?;
                    set.add(glob);
                }
            }
            set.build()
                .map(Some)
                .map_err(|e| fail(format!("{id}: {e}")))
        };
        rules.push(Rule {
            covers: globs(&r.files)?,
            except: globs(&r.except)?,
            id,
            says: r.says.trim().to_string(),
            files: r.files,
            level,
            forbid,
            require,
            per_method,
        });
    }
    Ok(Rules { rules })
}

/// The rules of the project at `root`: none without a rules file.
pub fn load(root: &std::path::Path) -> Result<Rules, RulesError> {
    match std::fs::read_to_string(root.join(FILE)) {
        Ok(text) => parse(&text),
        Err(_) => Ok(Rules::default()),
    }
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Whether any checked rule covers the file at `rel`.
    pub fn applies(&self, rel: &str) -> bool {
        self.rules.iter().any(|r| r.checked() && r.covers(rel))
    }

    /// Where `text`, the file at `rel`, breaks the rules.
    pub fn check(&self, rel: &str, text: &str) -> Vec<Breach> {
        let mut out = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.checked() || !rule.covers(rel) {
                continue;
            }
            if let Some(re) = &rule.forbid {
                for m in re.find_iter(text) {
                    if m.start() == m.end() || in_comment(text, m.start()) {
                        continue;
                    }
                    let (start, end) = (position(text, m.start()), position(text, m.end()));
                    out.push(Breach {
                        rule: i,
                        start,
                        end,
                        span: (start.0, end.0),
                    });
                }
            }
            let Some(re) = &rule.require else {
                continue;
            };
            if rule.per_method {
                for m in public_methods(text) {
                    if !re.is_match(&text[m.body.clone()]) {
                        out.push(Breach {
                            rule: i,
                            start: position(text, m.name.start),
                            end: position(text, m.name.end),
                            span: (position(text, m.name.start).0, position(text, m.body.end).0),
                        });
                    }
                }
            } else if !re.is_match(text) {
                // On the class (or whatever the file declares), else the top.
                let at = text
                    .lines()
                    .position(|l| {
                        let l = l.trim_start();
                        [
                            "class ",
                            "final class ",
                            "abstract class ",
                            "export default",
                        ]
                        .iter()
                        .any(|k| l.starts_with(k))
                    })
                    .unwrap_or(0);
                let width = text.lines().nth(at).map_or(0, str::len);
                out.push(Breach {
                    rule: i,
                    start: (at, 0),
                    end: (at, width),
                    span: (0, usize::MAX),
                });
            }
        }
        out
    }

    /// The rules as a list to give an agent before it starts.
    pub fn for_agents(&self) -> String {
        let mut out = String::from(
            "This project's rules (from .e/rules.toml) — follow them; the editor checks them:\n",
        );
        for r in &self.rules {
            out.push_str(&format!("- {}", r.says));
            if !r.files.is_empty() {
                out.push_str(&format!(" (in {})", r.files.join(", ")));
            }
            out.push('\n');
        }
        out
    }
}

/// `(line, column)` of byte `off` in `text`, both 0-based.
fn position(text: &str, off: usize) -> (usize, usize) {
    let before = &text[..off];
    let line = before.matches('\n').count();
    (line, off - before.rfind('\n').map_or(0, |i| i + 1))
}

/// Whether `off` is on a line that is a comment: `//`, `#`, `/*`, a `*`
/// docblock line, a Blade `{{--`.
fn in_comment(text: &str, off: usize) -> bool {
    let start = text[..off].rfind('\n').map_or(0, |i| i + 1);
    let line = text[start..].trim_start();
    line.starts_with("//")
        || line.starts_with("/*")
        || line.starts_with("* ")
        || line == "*"
        || line.starts_with("{{--")
        || (line.starts_with('#') && !line.starts_with("#["))
}

/// A method: its name, and its body between the braces.
struct Method {
    name: std::ops::Range<usize>,
    body: std::ops::Range<usize>,
}

/// The public methods in `text` with a body, constructors and other magic
/// methods left out — but not `__invoke`.
fn public_methods(text: &str) -> Vec<Method> {
    static DECL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let decl = DECL.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t]*((?:(?:public|protected|private|static|final|abstract|readonly)\s+)*)function\s+&?\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(",
        )
        .unwrap()
    });
    let mut out = Vec::new();
    for c in decl.captures_iter(text) {
        let mods = c.get(1).map_or("", |m| m.as_str());
        let name = c.get(2).unwrap();
        // `__invoke` is a single-action controller's action.
        let magic = name.as_str().starts_with("__") && name.as_str() != "__invoke";
        if mods.contains("protected") || mods.contains("private") || magic {
            continue;
        }
        let Some(open) = body_open(text, c.get(0).unwrap().end()) else {
            continue;
        };
        let Some(close) = matching_brace(text, open) else {
            continue;
        };
        out.push(Method {
            name: name.range(),
            body: open + 1..close,
        });
    }
    out
}

/// The `{` opening the body of a function whose parameter list starts at
/// `from`, past the parameters and return type; `None` for one without a body.
fn body_open(text: &str, from: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (i, b) in text.as_bytes()[from..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'{' if depth == 0 => return Some(from + i),
            b';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// The `}` closing the `{` at `open`, past strings and comments.
fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            q @ (b'\'' | b'"') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'#' if b.get(i + 1) != Some(&b'[') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The rules a changeset breaks where it changed: a `forbid` match on a line
/// it added, a method it touched without the `require`d call, a file it
/// added or changed without it. `rel` turns a changeset path (from the
/// repository root) into one from the project root, `None` outside it;
/// `read` gives a file's text as it is now.
pub fn review_flags(
    rules: &Rules,
    cs: &Changeset,
    rel: impl Fn(&str) -> Option<String>,
    read: impl Fn(&str) -> Option<String>,
) -> Vec<Flag> {
    let mut out = Vec::new();
    for f in &cs.files {
        if f.kind == ChangeKind::Deleted || f.binary {
            continue;
        }
        let Some(rel) = rel(&f.path) else {
            continue;
        };
        if !rules.applies(&rel) {
            continue;
        }
        let Some(text) = read(&f.path) else {
            continue;
        };
        // 0-based lines this change added.
        let added: Vec<usize> = f
            .hunks
            .iter()
            .flat_map(|h| added_lines(h).into_iter().map(|(n, _)| n.saturating_sub(1)))
            .collect();
        if added.is_empty() {
            continue;
        }
        for b in rules.check(&rel, &text) {
            let touched = added.iter().any(|n| (b.span.0..=b.span.1).contains(n));
            if !touched {
                continue;
            }
            let rule = &rules.rules[b.rule];
            out.push(Flag {
                path: f.path.clone(),
                line: b.start.0 + 1,
                severity: match rule.level {
                    Level::Error => Severity::Danger,
                    Level::Warning => Severity::Warn,
                },
                code: "project-rule",
                message: format!("{} ({})", rule.says, rule.id),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changeset_from_diff;

    const RULES: &str = r#"
[[rule]]
id = "no-unguarded-models"
says = "Models list their $fillable; never $guarded = []"
files = ["app/Models"]
forbid = '\$guarded\s*=\s*\[\s*\]'

[[rule]]
id = "billing-authorizes"
says = "Every Billing controller action checks authorization"
files = ["app/Http/Controllers/Billing/*.php"]
require = '\$this->authorize\(|Gate::'
per = "method"
level = "error"

[[rule]]
says = "Money is integer cents, never a float"
"#;

    #[test]
    fn a_forbidden_pattern_is_found_where_it_is_written() {
        let rules = parse(RULES).unwrap();
        assert_eq!(rules.rules.len(), 3);
        assert_eq!(rules.rules[2].id, "rule-3");
        assert!(!rules.rules[2].checked());
        let text = "<?php\n\nclass Order extends Model\n{\n    // not $guarded = [] here\n    protected $guarded = [];\n}\n";
        let found = rules.check("app/Models/Order.php", text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, 0);
        assert_eq!(found[0].start, (5, 14));
        assert_eq!(found[0].end, (5, 27));
        // Only where the rule covers.
        assert!(rules.check("app/Services/Order.php", text).is_empty());
        assert!(rules
            .check("vendor/x/app/Models/Order.php", text)
            .is_empty());
    }

    #[test]
    fn each_public_method_must_have_what_is_required() {
        let rules = parse(RULES).unwrap();
        let text = r#"<?php

class InvoiceController extends Controller
{
    public function __construct(private Billing $billing) {}

    public function show(Invoice $invoice)
    {
        $this->authorize('view', $invoice);
        return view('invoice', ['url' => "{$invoice->id}}"]);
    }

    public function update(Request $request, Invoice $invoice): RedirectResponse
    {
        $invoice->update($request->all()); // no authorize() here
        return back();
    }

    private function helper(): void
    {
    }
}
"#;
        let found = rules.check("app/Http/Controllers/Billing/InvoiceController.php", text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, 1);
        assert_eq!(found[0].start, (12, 20));
        assert_eq!(found[0].span, (12, 16));
        // A single-action controller's action is checked too.
        let invokable = "<?php\nclass PayController\n{\n    public function __invoke(Invoice $invoice)\n    {\n        return 1;\n    }\n}\n";
        let found = rules.check("app/Http/Controllers/Billing/PayController.php", invokable);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].start, (3, 20));
        // Not in a subdirectory: `*` stops at `/`.
        assert!(rules
            .check("app/Http/Controllers/Billing/Admin/X.php", text)
            .is_empty());
    }

    #[test]
    fn a_broken_file_says_where_and_why() {
        let e = parse("[[rule]]\nsays = \"x\"\nforbid = '('\n").unwrap_err();
        assert_eq!(e.line, 0);
        assert!(
            e.message.starts_with("rule-1: `forbid` isn't a pattern"),
            "{e:?}"
        );
        let e = parse("\n[[rule]]\nsays = \"x\"\nlevel = \"fatal\"\n").unwrap_err();
        assert_eq!(e.line, 1);
        assert!(e.message.contains("\"warning\" or \"error\""));
        let e = parse("[[rule]]\nsays = \"x\"\nforbidden = 'y'\n").unwrap_err();
        assert_eq!(e.line, 2, "{e:?}");
        assert!(e.message.contains("forbidden"));
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn a_review_flags_only_what_the_change_touched() {
        let rules = parse(RULES).unwrap();
        let controller = "<?php\n\nclass InvoiceController\n{\n    public function show()\n    {\n        return 1;\n    }\n\n    public function update()\n    {\n        return 2;\n    }\n}\n";
        // The change added a line to update(), none to show().
        let diff = "diff --git a/app/Http/Controllers/Billing/InvoiceController.php b/app/Http/Controllers/Billing/InvoiceController.php\n--- a/app/Http/Controllers/Billing/InvoiceController.php\n+++ b/app/Http/Controllers/Billing/InvoiceController.php\n@@ -11,2 +11,3 @@\n     {\n+        return 2;\n     }\n";
        let cs = changeset_from_diff(diff);
        let flags = review_flags(
            &rules,
            &cs,
            |p| Some(p.to_string()),
            |_| Some(controller.to_string()),
        );
        assert_eq!(flags.len(), 1, "{flags:?}");
        assert_eq!(flags[0].line, 10);
        assert_eq!(flags[0].severity, Severity::Danger);
        assert_eq!(flags[0].code, "project-rule");
        assert_eq!(
            flags[0].message,
            "Every Billing controller action checks authorization (billing-authorizes)"
        );
        // Outside the project: nothing.
        assert!(review_flags(&rules, &cs, |_| None, |_| Some(controller.to_string())).is_empty());
    }

    #[test]
    fn agents_are_told_every_rule() {
        let rules = parse(RULES).unwrap();
        let told = rules.for_agents();
        assert!(
            told.contains("- Models list their $fillable; never $guarded = [] (in app/Models)\n")
        );
        assert!(told.contains("- Money is integer cents, never a float\n"));
    }
}
