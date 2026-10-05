//! Moving a PHP class, and everything that has to move with it.
//!
//! Renaming a class file by hand means three separate edits you have to
//! remember: the file's own `namespace`, every `use` that imports it, and every
//! fully-qualified mention. Miss one and the project only tells you at runtime.
//!
//! PSR-4 makes the mechanical half derivable — a namespace maps to a directory,
//! so the new path follows from the new namespace — and the references are
//! findable. What is left is showing the result before doing it, which reuses
//! the rename preview.

use std::path::{Path, PathBuf};

/// One `psr-4` entry: a namespace prefix and the directory it maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Psr4Root {
    /// `App\`, with the trailing separator, as composer writes it.
    pub prefix: String,
    /// `app/`, relative to the project root.
    pub dir: String,
}

/// Read the `psr-4` maps out of a `composer.json`.
///
/// Both `autoload` and `autoload-dev` count: a test class lives under `Tests\`
/// and moving one is the same operation.
pub fn psr4_roots(composer_json: &str) -> Vec<Psr4Root> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(composer_json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for section in ["autoload", "autoload-dev"] {
        let Some(map) = v
            .get(section)
            .and_then(|a| a.get("psr-4"))
            .and_then(|p| p.as_object())
        else {
            continue;
        };
        for (prefix, dir) in map {
            // A prefix can map to several directories; the first is where new
            // files go, which is what matters here.
            let dir = match dir {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(a) => match a.first().and_then(|d| d.as_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                },
                _ => continue,
            };
            out.push(Psr4Root {
                prefix: prefix.clone(),
                dir,
            });
        }
    }
    // Longest prefix first, so `Database\Factories\` wins over `Database\`.
    out.sort_by(|a, b| {
        b.prefix
            .len()
            .cmp(&a.prefix.len())
            .then(a.prefix.cmp(&b.prefix))
    });
    out
}

/// The file a fully-qualified class name should live in.
///
/// `App\Models\Order` with `App\ => app/` gives `app/Models/Order.php`.
pub fn path_for(roots: &[Psr4Root], fqn: &str) -> Option<PathBuf> {
    let fqn = fqn.trim_start_matches('\\');
    let root = roots.iter().find(|r| fqn.starts_with(&r.prefix))?;
    let rest = &fqn[root.prefix.len()..];
    if rest.is_empty() {
        return None;
    }
    let dir = root.dir.trim_end_matches('/');
    Some(PathBuf::from(format!(
        "{dir}/{}.php",
        rest.replace('\\', "/")
    )))
}

/// The fully-qualified name a file should declare, from its path.
///
/// The inverse of [`path_for`], used to work out what a class is currently
/// called from where it sits.
pub fn fqn_for(roots: &[Psr4Root], rel_path: &Path) -> Option<String> {
    let p = rel_path.to_string_lossy().replace('\\', "/");
    let p = p.strip_suffix(".php")?;
    let root = roots
        .iter()
        .find(|r| p.starts_with(r.dir.trim_end_matches('/')))?;
    let rest = p
        .strip_prefix(root.dir.trim_end_matches('/'))?
        .trim_start_matches('/');
    if rest.is_empty() {
        return None;
    }
    Some(format!("{}{}", root.prefix, rest.replace('/', "\\")))
}

/// The namespace part of a fully-qualified name — everything before the class.
pub fn namespace_of(fqn: &str) -> &str {
    fqn.rfind('\\').map(|i| &fqn[..i]).unwrap_or("")
}

/// The class name — everything after the last separator.
pub fn class_of(fqn: &str) -> &str {
    fqn.rfind('\\').map(|i| &fqn[i + 1..]).unwrap_or(fqn)
}

/// Is this byte part of a PHP identifier or namespace separator?
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\\'
}

/// Byte ranges where `fqn` appears as a whole name.
///
/// Whole-name matching is what keeps this from rewriting `App\Models\OrderItem`
/// while moving `App\Models\Order`, and from touching the string
/// `"App\Models\Order"` in a config array — the latter is a judgement call, but
/// rewriting a string silently is the worse mistake.
pub fn references_in(text: &str, fqn: &str) -> Vec<(usize, usize)> {
    let needle = fqn.trim_start_matches('\\');
    if needle.is_empty() {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        from = end;

        // A leading `\` is part of the reference and should be replaced with it.
        let before_ok = start == 0 || !is_name_byte(bytes[start - 1]) || bytes[start - 1] == b'\\';
        let after_ok = end >= bytes.len() || !is_name_byte(bytes[end]);
        if !before_ok || !after_ok {
            continue;
        }
        // Don't match a longer name that merely ends with ours.
        if start > 0 && bytes[start - 1] == b'\\' {
            // Preceded by a separator: only a leading `\` counts, not
            // `Other\App\Models\Order`.
            let before = start - 1;
            if before > 0 && is_name_byte(bytes[before - 1]) {
                continue;
            }
            out.push((before, end));
            continue;
        }
        out.push((start, end));
    }
    out
}

/// One file that has to change, with the text it will become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    pub path: PathBuf,
    pub updated: String,
    /// How many references were rewritten in it.
    pub hits: usize,
}

/// Rewrite every reference to `old_fqn` in `text` as `new_fqn`.
///
/// Returns `None` when nothing matched, so a caller can skip the file rather
/// than write it back unchanged.
pub fn rewrite_references(text: &str, old_fqn: &str, new_fqn: &str) -> Option<(String, usize)> {
    let hits = references_in(text, old_fqn);
    if hits.is_empty() {
        return None;
    }
    let new = new_fqn.trim_start_matches('\\');
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e) in &hits {
        out.push_str(&text[last..*s]);
        // Preserve a leading `\` if the original had one.
        if text.as_bytes()[*s] == b'\\' {
            out.push('\\');
        }
        out.push_str(new);
        last = *e;
    }
    out.push_str(&text[last..]);
    Some((out, hits.len()))
}

/// Rewrite the moved file itself: its `namespace` line, and any self-references.
pub fn rewrite_moved_file(text: &str, old_fqn: &str, new_fqn: &str) -> String {
    let (old_ns, new_ns) = (namespace_of(old_fqn), namespace_of(new_fqn));
    let (old_cls, new_cls) = (class_of(old_fqn), class_of(new_fqn));

    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let trimmed = line.trim_start();
        // Only the declaration, and only if it says what we expect — a file
        // whose namespace disagrees with its path is already broken, and
        // rewriting it blind would hide that.
        if trimmed.starts_with("namespace ") && line.contains(old_ns) && !old_ns.is_empty() {
            out.push_str(&line.replacen(old_ns, new_ns, 1));
            continue;
        }
        let _ = i;
        // The class name itself, when it changed.
        if old_cls != new_cls {
            out.push_str(&rename_class_token(line, old_cls, new_cls));
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Replace the class name where it appears as a whole token on one line.
fn rename_class_token(line: &str, old: &str, new: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(rel) = line[from..].find(old) {
        let s = from + rel;
        let e = s + old.len();
        from = e;
        let before_ok = s == 0 || !is_name_byte(bytes[s - 1]);
        let after_ok = e >= bytes.len() || !is_name_byte(bytes[e]);
        if before_ok && after_ok {
            out.push_str(&line[last..s]);
            out.push_str(new);
            last = e;
        }
    }
    out.push_str(&line[last..]);
    out
}

// ---- planning a move ------------------------------------------------------

/// Everything a move would do, ready to preview.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MovePlan {
    pub old_fqn: String,
    pub new_fqn: String,
    /// Where the file is now, and where it goes. Relative to the project root.
    pub from: PathBuf,
    pub to: PathBuf,
    /// The moved file itself: its namespace line, and its own class name when
    /// the move renames it too.
    pub moved: Option<Rewrite>,
    /// Other files that refer to it. Kept apart from `moved` so a count of
    /// "references" never quietly includes the subject of the move.
    pub referrers: Vec<Rewrite>,
}

impl MovePlan {
    /// References in *other* files. The moved file's own rewrite is not one.
    pub fn reference_count(&self) -> usize {
        self.referrers.iter().map(|r| r.hits).sum()
    }

    pub fn referrer_count(&self) -> usize {
        self.referrers.len()
    }

    /// Every file that changes, the moved one first.
    #[cfg(test)]
    pub fn rewrites(&self) -> impl Iterator<Item = &Rewrite> {
        self.moved.iter().chain(self.referrers.iter())
    }

    pub fn summary(&self) -> String {
        let (n, f) = (self.reference_count(), self.referrer_count());
        if f == 0 {
            return "no other file refers to it".into();
        }
        format!(
            "{n} {} in {f} other {}",
            if n == 1 { "reference" } else { "references" },
            if f == 1 { "file" } else { "files" },
        )
    }
}

/// Work out the whole move.
///
/// `files` is every candidate the caller wants scanned — the walker already
/// honours ignore rules, so `vendor/` never reaches here. `read` resolves each
/// to its current text, so open buffers can be served from memory rather than
/// disk.
pub fn plan_move<F>(
    roots: &[Psr4Root],
    old_fqn: &str,
    new_fqn: &str,
    files: &[PathBuf],
    mut read: F,
) -> Option<MovePlan>
where
    F: FnMut(&Path) -> Option<String>,
{
    let from = path_for(roots, old_fqn)?;
    let to = path_for(roots, new_fqn)?;
    if from == to {
        return None;
    }
    let mut plan = MovePlan {
        old_fqn: old_fqn.to_string(),
        new_fqn: new_fqn.to_string(),
        from: from.clone(),
        to,
        ..Default::default()
    };

    if let Some(text) = read(&from) {
        let updated = rewrite_moved_file(&text, old_fqn, new_fqn);
        plan.moved = Some(Rewrite {
            path: from.clone(),
            hits: usize::from(updated != text),
            updated,
        });
    }

    for path in files {
        if *path == from {
            continue;
        }
        let Some(text) = read(path) else { continue };
        if let Some((updated, hits)) = rewrite_references(&text, old_fqn, new_fqn) {
            plan.referrers.push(Rewrite {
                path: path.clone(),
                updated,
                hits,
            });
        }
    }
    Some(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSER: &str = r#"{
      "autoload": {
        "psr-4": {
          "App\\": "app/",
          "Database\\Factories\\": "database/factories/"
        }
      },
      "autoload-dev": { "psr-4": { "Tests\\": "tests/" } }
    }"#;

    fn roots() -> Vec<Psr4Root> {
        psr4_roots(COMPOSER)
    }

    fn reader(files: Vec<(&str, &str)>) -> impl FnMut(&Path) -> Option<String> {
        let map: std::collections::HashMap<PathBuf, String> = files
            .into_iter()
            .map(|(p, t)| (PathBuf::from(p), t.to_string()))
            .collect();
        move |p: &Path| map.get(p).cloned()
    }

    #[test]
    fn a_plan_covers_the_file_and_everything_that_imports_it() {
        let files = vec![
            (
                "app/Models/Order.php",
                "<?php\nnamespace App\\Models;\n\nclass Order {}\n",
            ),
            (
                "app/Http/Controllers/OrderController.php",
                "<?php\nuse App\\Models\\Order;\n\nclass OrderController { public function i() { return Order::all(); } }\n",
            ),
            (
                "app/Support/Unrelated.php",
                "<?php\nclass Unrelated {}\n",
            ),
        ];
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(*p)).collect();
        let plan = plan_move(
            &roots(),
            "App\\Models\\Order",
            "App\\Domain\\Order",
            &paths,
            reader(files.clone()),
        )
        .unwrap();

        assert_eq!(plan.from, PathBuf::from("app/Models/Order.php"));
        assert_eq!(plan.to, PathBuf::from("app/Domain/Order.php"));
        // The moved file, plus the one controller. Not the unrelated file.
        assert_eq!(plan.rewrites().count(), 2);
        assert_eq!(plan.referrer_count(), 1);
        assert_eq!(plan.summary(), "1 reference in 1 other file");
        assert!(plan
            .moved
            .as_ref()
            .unwrap()
            .updated
            .contains("namespace App\\Domain;"));
        assert!(plan.referrers[0]
            .updated
            .contains("use App\\Domain\\Order;"));
    }

    #[test]
    fn a_move_to_the_same_place_is_not_a_move() {
        let paths = vec![PathBuf::from("app/Models/Order.php")];
        assert!(plan_move(
            &roots(),
            "App\\Models\\Order",
            "App\\Models\\Order",
            &paths,
            reader(vec![]),
        )
        .is_none());
    }

    #[test]
    fn a_target_namespace_with_no_psr4_map_cannot_be_planned() {
        // Better to refuse than to invent a path composer will never autoload.
        let paths = vec![PathBuf::from("app/Models/Order.php")];
        assert!(plan_move(
            &roots(),
            "App\\Models\\Order",
            "Nowhere\\Order",
            &paths,
            reader(vec![]),
        )
        .is_none());
    }

    #[test]
    fn a_class_nothing_imports_still_moves() {
        let files = vec![(
            "app/Models/Order.php",
            "<?php\nnamespace App\\Models;\n\nclass Order {}\n",
        )];
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(*p)).collect();
        let plan = plan_move(
            &roots(),
            "App\\Models\\Order",
            "App\\Domain\\Order",
            &paths,
            reader(files.clone()),
        )
        .unwrap();
        assert_eq!(plan.rewrites().count(), 1);
        assert_eq!(plan.summary(), "no other file refers to it");
    }

    #[test]
    fn psr4_maps_are_read_from_both_autoload_sections() {
        let r = roots();
        assert_eq!(r.len(), 3);
        // Longest prefix first, so the more specific map wins a lookup.
        assert_eq!(r[0].prefix, "Database\\Factories\\");
        assert!(r.iter().any(|x| x.prefix == "Tests\\" && x.dir == "tests/"));
    }

    #[test]
    fn a_namespace_maps_to_a_path_and_back() {
        let r = roots();
        assert_eq!(
            path_for(&r, "App\\Models\\Order"),
            Some(PathBuf::from("app/Models/Order.php"))
        );
        assert_eq!(
            fqn_for(&r, Path::new("app/Models/Order.php")).as_deref(),
            Some("App\\Models\\Order")
        );
        // The round trip has to survive the more specific prefix too.
        assert_eq!(
            path_for(&r, "Database\\Factories\\UserFactory"),
            Some(PathBuf::from("database/factories/UserFactory.php"))
        );
        assert_eq!(
            fqn_for(&r, Path::new("database/factories/UserFactory.php")).as_deref(),
            Some("Database\\Factories\\UserFactory")
        );
    }

    #[test]
    fn a_leading_separator_is_tolerated() {
        assert_eq!(
            path_for(&roots(), "\\App\\Models\\Order"),
            Some(PathBuf::from("app/Models/Order.php"))
        );
    }

    #[test]
    fn a_namespace_outside_every_map_has_no_path() {
        assert_eq!(path_for(&roots(), "Vendor\\Thing"), None);
        assert_eq!(fqn_for(&roots(), Path::new("vendor/thing.php")), None);
    }

    #[test]
    fn a_use_statement_is_a_reference() {
        let text = "<?php\nuse App\\Models\\Order;\nuse App\\Models\\OrderItem;\n";
        let hits = references_in(text, "App\\Models\\Order");
        assert_eq!(hits.len(), 1, "OrderItem must not match Order");
        let (out, n) =
            rewrite_references(text, "App\\Models\\Order", "App\\Domain\\Order").unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("use App\\Domain\\Order;"));
        assert!(out.contains("use App\\Models\\OrderItem;"), "{out}");
    }

    #[test]
    fn a_longer_name_that_ends_with_ours_is_not_a_match() {
        // `Legacy\App\Models\Order` is a different class entirely.
        let text = "use Legacy\\App\\Models\\Order;\n";
        assert!(references_in(text, "App\\Models\\Order").is_empty());
    }

    #[test]
    fn a_leading_backslash_is_replaced_with_the_reference() {
        let text = "$o = \\App\\Models\\Order::create();\n";
        let (out, n) =
            rewrite_references(text, "App\\Models\\Order", "App\\Domain\\Order").unwrap();
        assert_eq!(n, 1);
        assert_eq!(out, "$o = \\App\\Domain\\Order::create();\n");
    }

    #[test]
    fn an_aliased_import_keeps_its_alias() {
        let text = "use App\\Models\\Order as OrderModel;\n";
        let (out, _) =
            rewrite_references(text, "App\\Models\\Order", "App\\Domain\\Order").unwrap();
        assert_eq!(out, "use App\\Domain\\Order as OrderModel;\n");
    }

    #[test]
    fn a_file_with_no_reference_is_left_alone() {
        assert_eq!(
            rewrite_references("<?php\nclass Other {}\n", "App\\Models\\Order", "X\\Order"),
            None,
            "returning Some would rewrite an unchanged file"
        );
    }

    #[test]
    fn the_moved_file_gets_the_new_namespace() {
        let text =
            "<?php\n\nnamespace App\\Models;\n\nuse App\\Support\\Money;\n\nclass Order\n{\n}\n";
        let out = rewrite_moved_file(text, "App\\Models\\Order", "App\\Domain\\Order");
        assert!(out.contains("namespace App\\Domain;"), "{out}");
        assert!(
            out.contains("use App\\Support\\Money;"),
            "an unrelated import must survive: {out}"
        );
        assert!(out.contains("class Order"), "{out}");
    }

    #[test]
    fn moving_and_renaming_at_once_updates_the_class_too() {
        let text = "<?php\nnamespace App\\Models;\n\nclass Order\n{\n    public function order(): Order { return new Order(); }\n}\n";
        let out = rewrite_moved_file(text, "App\\Models\\Order", "App\\Domain\\Purchase");
        assert!(out.contains("namespace App\\Domain;"), "{out}");
        assert!(out.contains("class Purchase"), "{out}");
        assert!(out.contains("new Purchase()"), "{out}");
        assert!(
            out.contains("public function order()"),
            "a method that happens to share the name must not be renamed: {out}"
        );
    }

    #[test]
    fn a_namespace_line_that_disagrees_with_the_path_is_left_alone() {
        // Rewriting it blind would paper over a file that is already wrong.
        let text = "<?php\nnamespace Something\\Else;\n\nclass Order {}\n";
        let out = rewrite_moved_file(text, "App\\Models\\Order", "App\\Domain\\Order");
        assert!(out.contains("namespace Something\\Else;"), "{out}");
    }

    #[test]
    fn namespace_and_class_split_correctly() {
        assert_eq!(namespace_of("App\\Models\\Order"), "App\\Models");
        assert_eq!(class_of("App\\Models\\Order"), "Order");
        assert_eq!(namespace_of("Order"), "");
        assert_eq!(class_of("Order"), "Order");
    }

    #[test]
    fn a_prefix_mapped_to_several_directories_uses_the_first() {
        let json = r#"{"autoload":{"psr-4":{"App\\":["app/","src/"]}}}"#;
        let r = psr4_roots(json);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].dir, "app/");
    }

    #[test]
    fn a_broken_composer_file_yields_no_roots_rather_than_a_panic() {
        assert!(psr4_roots("").is_empty());
        assert!(psr4_roots("{not json").is_empty());
        assert!(psr4_roots("{}").is_empty());
    }
}

/// Against a real project tree. Opt-in.
///
/// ```sh
/// E_MOVE_PROJECT=/path/to/app cargo test -p e-app live_move -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_move {
    use super::*;

    #[test]
    #[ignore]
    fn moving_a_model_updates_every_referrer() {
        let Ok(root) = std::env::var("E_MOVE_PROJECT") else {
            eprintln!("set E_MOVE_PROJECT — skipping");
            return;
        };
        let root = PathBuf::from(root);
        let composer = std::fs::read_to_string(root.join("composer.json")).expect("composer.json");
        let roots = psr4_roots(&composer);
        println!("psr-4: {roots:?}");

        // Every .php file under the project, relative — what the editor's
        // ignore-aware walker would hand over.
        let mut files: Vec<PathBuf> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "php") {
                    files.push(p.strip_prefix(&root).unwrap().to_path_buf());
                }
            }
        }
        files.sort();
        println!("files: {files:?}");

        let plan = plan_move(
            &roots,
            "App\\Models\\Order",
            "App\\Domain\\Order",
            &files,
            |p| std::fs::read_to_string(root.join(p)).ok(),
        )
        .expect("a plan");

        println!("{} -> {}", plan.from.display(), plan.to.display());
        println!("{}", plan.summary());
        for r in plan.rewrites() {
            println!("  {} ({} hits)", r.path.display(), r.hits);
        }

        assert_eq!(plan.to, PathBuf::from("app/Domain/Order.php"));
        // The controller (two references) and the aliased test import.
        assert_eq!(plan.referrer_count(), 2, "controller and test");
        assert_eq!(plan.reference_count(), 3, "use + \\FQN + aliased use");

        let ctrl = plan
            .referrers
            .iter()
            .find(|r| r.path.ends_with("OrderController.php"))
            .unwrap();
        assert!(
            ctrl.updated.contains("use App\\Domain\\Order;"),
            "{}",
            ctrl.updated
        );
        assert!(
            ctrl.updated.contains("use App\\Models\\OrderItem;"),
            "OrderItem must be untouched: {}",
            ctrl.updated
        );
        assert!(
            ctrl.updated.contains("\\App\\Domain\\Order::query()"),
            "{}",
            ctrl.updated
        );

        let test = plan
            .referrers
            .iter()
            .find(|r| r.path.ends_with("OrderTest.php"))
            .unwrap();
        assert!(
            test.updated
                .contains("use App\\Domain\\Order as OrderModel;"),
            "{}",
            test.updated
        );

        let moved = plan.moved.as_ref().unwrap();
        assert!(
            moved.updated.contains("namespace App\\Domain;"),
            "{}",
            moved.updated
        );
        assert!(
            moved.updated.contains("use App\\Support\\Money;"),
            "{}",
            moved.updated
        );
    }
}
