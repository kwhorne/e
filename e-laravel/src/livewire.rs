//! Livewire refactoring support: a component's class and Blade view pretend to
//! be one thing, so `e` treats them that way — `wire:model` completes from the
//! class's public properties, you can jump between the two files, and renaming a
//! property updates both the class and every `wire:` reference in the view.
//!
//! Three shapes of component are understood:
//! * class-based: `app/Livewire/Admin/UserTable.php` + `resources/views/livewire/admin/user-table.blade.php`;
//! * single-file (Livewire 4, and Volt on Livewire 3): one Blade file whose
//!   `<?php … ?>` block holds `new class extends Component { … }`, e.g.
//!   `resources/views/components/⚡counter.blade.php`;
//! * multi-file (Livewire 4): `⚡counter/counter.php` + `⚡counter/counter.blade.php`.

use std::path::{Path, PathBuf};

/// A resolved Livewire component: its class file, its Blade view, and name.
#[derive(Clone, Debug)]
pub struct Component {
    pub class_file: PathBuf,
    pub view_file: PathBuf,
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn kebab_to_pascal(seg: &str) -> String {
    seg.split('-')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn pascal_to_kebab(seg: &str) -> String {
    let mut out = String::new();
    for (i, c) in seg.chars().enumerate() {
        if c.is_uppercase() {
            if i != 0 {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The PHP of a single-file component — its `<?php … ?>` block holding
/// `new class` — or all of `src` for an ordinary class file. Scanning the
/// Blade half for `public` would find words in the markup.
pub fn class_part(src: &str) -> &str {
    if let Some(start) = src.find("<?php") {
        if let Some(len) = src[start..].find("?>") {
            let block = &src[start..start + len];
            if block.contains("new class") {
                return block;
            }
        }
    }
    src
}

/// Whether `src` is a single-file component (class and view in one file).
pub fn is_single_file(src: &str) -> bool {
    !std::ptr::eq(class_part(src), src)
}

/// A Livewire 4 multi-file component: `⚡counter/counter.php` beside
/// `⚡counter/counter.blade.php`, from either file.
fn multi_file(path: &Path) -> Option<Component> {
    let dir = path.parent()?;
    let dir_name = dir.file_name()?.to_string_lossy();
    let name = dir_name.trim_start_matches('⚡');
    let class_file = dir.join(format!("{name}.php"));
    let view_file = dir.join(format!("{name}.blade.php"));
    let is_part = path == class_file || path == view_file;
    (is_part && class_file.is_file() && view_file.is_file()).then_some(Component {
        class_file,
        view_file,
    })
}

/// Resolve the component that `path` belongs to (given either its view or class).
pub fn resolve(root: &Path, path: &Path) -> Option<Component> {
    let rel = |base: &Path| path.strip_prefix(base).ok().map(|p| p.to_path_buf());

    if let Some(c) = multi_file(path) {
        return Some(c);
    }
    // A single-file component is both halves at once.
    if path.to_string_lossy().ends_with(".blade.php") {
        if let Ok(src) = std::fs::read_to_string(path) {
            if is_single_file(&src) {
                return Some(Component {
                    class_file: path.to_path_buf(),
                    view_file: path.to_path_buf(),
                });
            }
        }
    }

    // From a Blade view: resources/views/livewire/<segs>.blade.php
    let views_base = root.join("resources/views/livewire");
    if let Some(r) = rel(&views_base) {
        let s = r.to_string_lossy();
        if let Some(stem) = s.strip_suffix(".blade.php") {
            let class_rel: PathBuf = stem.split('/').map(kebab_to_pascal).collect();
            for base in ["app/Livewire", "app/Http/Livewire"] {
                let cf = root.join(base).join(&class_rel).with_extension("php");
                if cf.is_file() {
                    return Some(Component {
                        class_file: cf,
                        view_file: path.to_path_buf(),
                    });
                }
            }
        }
    }

    // From a class: app/Livewire/<Segs>.php or app/Http/Livewire/<Segs>.php
    for base in ["app/Livewire", "app/Http/Livewire"] {
        if let Some(r) = rel(&root.join(base)) {
            let s = r.to_string_lossy();
            if let Some(stem) = s.strip_suffix(".php") {
                let view_rel = stem
                    .split('/')
                    .map(pascal_to_kebab)
                    .collect::<Vec<_>>()
                    .join("/");
                let vf = views_base.join(format!("{view_rel}.blade.php"));
                if vf.is_file() {
                    return Some(Component {
                        class_file: path.to_path_buf(),
                        view_file: vf,
                    });
                }
            }
        }
    }
    None
}

/// Public properties declared on a Livewire component class.
pub fn properties(class_src: &str) -> Vec<String> {
    let class_src = class_part(class_src);
    let mut props = Vec::new();
    let mut search = 0;
    while let Some(rel) = class_src[search..].find("public") {
        let at = search + rel;
        search = at + 6;
        // Whole-word "public".
        let before_ok = at == 0 || !is_ident(class_src[..at].chars().next_back().unwrap_or(' '));
        let after = &class_src[at + 6..];
        if !before_ok || after.chars().next().map(is_ident).unwrap_or(false) {
            continue;
        }
        let trimmed = after.trim_start();
        if trimmed.starts_with("function") || trimmed.starts_with("const") {
            continue;
        }
        // Find the first `$name` before a terminator on this declaration.
        let mut chars = after.char_indices();
        let mut name = None;
        for (i, c) in chars.by_ref() {
            if c == ';' || c == '{' || c == '(' {
                break;
            }
            if c == '$' {
                let rest: String = after[i + 1..]
                    .chars()
                    .take_while(|c| is_ident(*c))
                    .collect();
                if !rest.is_empty() {
                    name = Some(rest);
                }
                break;
            }
        }
        if let Some(n) = name {
            if !props.contains(&n) {
                props.push(n);
            }
        }
    }
    props
}

/// Methods an action can call — `wire:click="save"` — i.e. the class's public
/// methods minus Livewire's lifecycle hooks.
pub fn actions(class_src: &str) -> Vec<String> {
    const HOOKS: &[&str] = &[
        "mount",
        "render",
        "boot",
        "booted",
        "hydrate",
        "dehydrate",
        "rules",
        "messages",
        "validationAttributes",
        "exception",
        "rendering",
        "rendered",
    ];
    crate::laravel::public_methods(class_part(class_src))
        .into_iter()
        .filter(|m| {
            !HOOKS.contains(&m.as_str())
                && !m.starts_with("updated")
                && !m.starts_with("updating")
                && !m.starts_with("hydrate")
                && !m.starts_with("dehydrate")
        })
        .collect()
}

/// `wire:click="sa` (any `wire:<event>[.modifier]="…"` attribute) → `"sa"`.
pub fn wire_action_partial(line_before: &str) -> Option<String> {
    let at = line_before.rfind("wire:")?;
    let attr = &line_before[at + 5..];
    let (name, rest) = attr.split_once('=')?;
    let event = name.split('.').next().unwrap_or("");
    const EVENTS: &[&str] = &[
        "click",
        "submit",
        "change",
        "input",
        "keydown",
        "keyup",
        "blur",
        "focus",
        "mouseenter",
        "mouseleave",
        "scroll",
        "init",
        "load",
    ];
    if !EVENTS.contains(&event) {
        return None;
    }
    let value = rest.strip_prefix('"').or_else(|| rest.strip_prefix('\''))?;
    if value.contains(['"', '\'', '(', ' ']) {
        return None;
    }
    Some(value.to_string())
}

/// The major version of Livewire the project locks, if it can be read.
pub fn livewire_major(root: &Path) -> Option<u32> {
    let lock = std::fs::read_to_string(root.join("composer.lock")).ok()?;
    let at = lock.find("\"name\": \"livewire/livewire\"")?;
    let rest = &lock[at..];
    let v = rest.find("\"version\": \"")? + "\"version\": \"".len();
    rest[v..]
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Where single- and multi-file components live, with the prefix their names
/// take there. Livewire 4: `views/components` and `views/livewire` unprefixed,
/// `pages::` and `layouts::` namespaces. Volt on Livewire 3 mounts
/// `views/livewire` and `views/pages`, both unprefixed.
fn single_file_locations(root: &Path) -> Vec<(PathBuf, &'static str)> {
    let views = root.join("resources/views");
    if livewire_major(root).unwrap_or(4) >= 4 {
        vec![
            (views.join("components"), ""),
            (views.join("livewire"), ""),
            (views.join("pages"), "pages::"),
            (views.join("layouts"), "layouts::"),
        ]
    } else {
        vec![(views.join("livewire"), ""), (views.join("pages"), "")]
    }
}

/// Every component in the project, by the name Livewire knows it by, with the
/// file go-to-definition should open (the class, or the single file).
pub fn components(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    // Class-based components.
    for base in ["app/Livewire", "app/Http/Livewire"] {
        let b = root.join(base);
        for file in php_files(&b, 0) {
            if let Ok(rel) = file.strip_prefix(&b) {
                let name = rel
                    .with_extension("")
                    .components()
                    .map(|c| pascal_to_kebab(&c.as_os_str().to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join(".");
                out.push((name, file));
            }
        }
    }
    // Single- and multi-file components.
    for (base, prefix) in single_file_locations(root) {
        for file in php_files(&base, 0) {
            let s = file.to_string_lossy();
            let Some(stem) = s.strip_suffix(".blade.php") else {
                continue;
            };
            let Ok(rel) = std::path::Path::new(stem).strip_prefix(&base) else {
                continue;
            };
            let mut parts: Vec<String> = rel
                .components()
                .map(|c| {
                    c.as_os_str()
                        .to_string_lossy()
                        .trim_start_matches('⚡')
                        .to_string()
                })
                .collect();
            // `⚡counter/counter.blade.php` is the component `counter`.
            let multi = multi_file(&file).is_some();
            if multi && parts.len() >= 2 && parts[parts.len() - 1] == parts[parts.len() - 2] {
                parts.pop();
            }
            let single = !multi
                && std::fs::read_to_string(&file)
                    .map(|src| is_single_file(&src))
                    .unwrap_or(false);
            if multi || single {
                out.push((format!("{prefix}{}", parts.join(".")), file));
            }
        }
    }
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// The file behind a component name (`admin.user-table`, `pages::dashboard`).
pub fn resolve_name(root: &Path, name: &str) -> Option<PathBuf> {
    components(root)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p)
}

/// `.php` files under `dir`, a few levels deep.
fn php_files(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if depth > 6 {
        return out;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(php_files(&p, depth + 1));
        } else if p.extension().is_some_and(|x| x == "php") {
            out.push(p);
        }
    }
    out
}

/// The component named at `offset` in a `<livewire:name` tag, an
/// `@livewire('name'` directive or a `Route::livewire('uri', 'name')`.
pub fn component_at(text: &str, offset: usize) -> Option<String> {
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':');
    let offset = offset.min(text.len());
    let start = offset
        - text[..offset]
            .chars()
            .rev()
            .take_while(|c| is_name(*c))
            .map(char::len_utf8)
            .sum::<usize>();
    let end = offset
        + text[offset..]
            .chars()
            .take_while(|c| is_name(*c))
            .map(char::len_utf8)
            .sum::<usize>();
    let word = &text[start..end];
    let before = &text[..start];
    if let Some(name) = word.strip_prefix("livewire:") {
        return (before.ends_with('<') && !name.is_empty()).then(|| name.to_string());
    }
    let quoted = before.ends_with('\'') || before.ends_with('"');
    let line = &before[before.rfind('\n').map_or(0, |i| i + 1)..];
    let call = line.trim_end_matches(['\'', '"']).trim_end();
    let in_call =
        call.ends_with("@livewire(") || (line.contains("Route::livewire(") && call.ends_with(','));
    (quoted && in_call && !word.is_empty()).then(|| word.to_string())
}

/// Rename a property in a single-file component: the class half like a
/// class, the Blade half like a view.
pub fn single_file_rename(src: &str, old: &str, new: &str) -> String {
    let part = class_part(src);
    let start = part.as_ptr() as usize - src.as_ptr() as usize;
    let end = start + part.len();
    format!(
        "{}{}{}",
        &src[..start],
        class_rename(part, old, new),
        view_rename(&src[end..], old, new)
    )
}

/// Component names Livewire would resolve from the class tree:
/// `app/Livewire/Admin/UserTable.php` → `admin.user-table`.
pub fn component_names(root: &Path) -> Vec<String> {
    components(root).into_iter().map(|(n, _)| n).collect()
}

/// `@livewire('adm` or `<livewire:adm` → `"adm"`.
pub fn tag_partial(line_before: &str) -> Option<String> {
    if let Some(i) = line_before.rfind("<livewire:") {
        let p = &line_before[i + "<livewire:".len()..];
        if !p.contains([' ', '>', '/']) {
            return Some(p.to_string());
        }
    }
    if let Some(i) = line_before.rfind("@livewire(") {
        let p = line_before[i + "@livewire(".len()..].trim_start();
        let p = p.strip_prefix('\'').or_else(|| p.strip_prefix('"'))?;
        if !p.contains(['\'', '"', ',']) {
            return Some(p.to_string());
        }
    }
    None
}

/// Line (0-based) of the `public $prop` declaration in the class, if present.
pub fn property_line(class_src: &str, prop: &str) -> Option<usize> {
    let part = class_part(class_src);
    let skipped = class_src[..part.as_ptr() as usize - class_src.as_ptr() as usize]
        .matches('\n')
        .count();
    let needle = format!("${prop}");
    for (i, line) in part.lines().enumerate().map(|(i, l)| (i + skipped, l)) {
        if line.contains("public") && line.contains(&needle) {
            // Ensure it's the whole variable, not a prefix.
            if let Some(p) = line.find(&needle) {
                let after = line[p + needle.len()..].chars().next();
                if !after.map(is_ident).unwrap_or(false) {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// If the cursor line is inside an unclosed `wire:model…="` value, return the
/// partial typed so far.
pub fn wire_model_partial(line_before: &str) -> Option<String> {
    // Last quote that opens an attribute value.
    let dq = line_before.rfind("=\"");
    let sq = line_before.rfind("='");
    let (pos, quote) = match (dq, sq) {
        (Some(d), Some(s)) if d > s => (d, '"'),
        (Some(_), Some(s)) => (s, '\''),
        (Some(d), None) => (d, '"'),
        (None, Some(s)) => (s, '\''),
        (None, None) => return None,
    };
    let value = &line_before[pos + 2..];
    if value.contains(quote) {
        return None; // attribute already closed
    }
    // Attribute name immediately before `=`.
    let name: String = line_before[..pos]
        .chars()
        .rev()
        .take_while(|c| is_ident(*c) || *c == ':' || *c == '.' || *c == '-')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if name.starts_with("wire:model") || name == "wire:model" {
        Some(value.to_string())
    } else {
        None
    }
}

// ---- Cross-file rename ----------------------------------------------------

/// Rename a property inside the component class: `$old`→`$new` (declaration and
/// local `$old`) and `$this->old`→`$this->new` (member access, not methods).
pub fn class_rename(src: &str, old: &str, new: &str) -> String {
    let s = replace_var(src, old, new);
    replace_member(&s, old, new)
}

/// Rename a property inside the Blade view: `$old`→`$new` and any
/// `wire:…="old"` attribute value.
pub fn view_rename(src: &str, old: &str, new: &str) -> String {
    let s = replace_var(src, old, new);
    replace_wire_value(&s, old, new)
}

/// Replace `$old` with `$new` where `old` is a whole identifier.
fn replace_var(src: &str, old: &str, new: &str) -> String {
    let needle = format!("${old}");
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(&needle) {
            let after = bytes.get(i + needle.len()).map(|b| *b as char);
            if !after.map(is_ident).unwrap_or(false) {
                out.push('$');
                out.push_str(new);
                i += needle.len();
                continue;
            }
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Replace `->old` with `->new` (property access, not a method call).
fn replace_member(src: &str, old: &str, new: &str) -> String {
    let needle = format!("->{old}");
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(&needle) {
            let after = bytes.get(i + needle.len()).map(|b| *b as char);
            let is_word = after.map(is_ident).unwrap_or(false);
            let is_call = after == Some('(');
            if !is_word && !is_call {
                out.push_str("->");
                out.push_str(new);
                i += needle.len();
                continue;
            }
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Replace `wire:…="old"` / `wire:…='old'` attribute values equal to `old`.
fn replace_wire_value(src: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut search = 0;
    while let Some(rel) = src[search..].find("wire:") {
        let at = search + rel;
        out.push_str(&src[search..at]);
        // Copy the wire attribute name up to `=`.
        let rest = &src[at..];
        let eq = rest.find('=');
        match eq {
            Some(e) if matches!(rest.as_bytes().get(e + 1), Some(b'"') | Some(b'\'')) => {
                let quote = rest.as_bytes()[e + 1] as char;
                let val_start = e + 2;
                if let Some(close_rel) = rest[val_start..].find(quote) {
                    let value = &rest[val_start..val_start + close_rel];
                    // Rename only when the whole bound value is `old` (allow
                    // modifiers on the attribute name, not the value).
                    if value == old {
                        out.push_str(&rest[..val_start]);
                        out.push_str(new);
                        out.push(quote);
                        search = at + val_start + close_rel + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        // Not a match: emit "wire:" and continue past it.
        out.push_str("wire:");
        search = at + 5;
    }
    out.push_str(&src[search..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_properties() {
        let src = r#"<?php
        class Counter extends Component {
            public $count = 0;
            public string $name = '';
            #[Reactive] public array $items = [];
            protected $hidden = 1;
            public function render() { return view('livewire.counter'); }
        }"#;
        let p = properties(src);
        assert_eq!(p, vec!["count", "name", "items"]);
        assert_eq!(property_line(src, "name"), Some(3));
    }

    #[test]
    fn detects_wire_model_partial() {
        assert_eq!(
            wire_model_partial("<input wire:model=\"na").as_deref(),
            Some("na")
        );
        assert_eq!(
            wire_model_partial("<input wire:model.live=\"").as_deref(),
            Some("")
        );
        assert!(wire_model_partial("<input type=\"text").is_none());
        assert!(wire_model_partial("<input wire:model=\"name\"").is_none());
    }

    #[test]
    fn renames_across_class_and_view() {
        let class =
            "public $name = '';\n public function save() { $this->name = 'x'; $this->name(); }";
        let renamed = class_rename(class, "name", "title");
        assert!(renamed.contains("public $title"));
        assert!(renamed.contains("$this->title = 'x'"));
        // A method call `$this->name()` must be left alone.
        assert!(renamed.contains("$this->name()"));

        let view = r#"<input wire:model.live="name"> {{ $name }} <span wire:model="names">"#;
        let rv = view_rename(view, "name", "title");
        assert!(rv.contains(r#"wire:model.live="title""#));
        assert!(rv.contains("{{ $title }}"));
        // "names" must not be touched.
        assert!(rv.contains(r#"wire:model="names""#));
    }

    #[test]
    fn actions_skip_lifecycle_hooks_and_wire_contexts_are_found() {
        let cls = "class Counter extends Component\n{\n    public function mount() {}\n    public function increment() {}\n    public function updatedCount() {}\n    public function save() {}\n    public function render() {}\n}";
        assert_eq!(actions(cls), vec!["increment", "save"]);
        assert_eq!(
            wire_action_partial("<button wire:click=\"sa").as_deref(),
            Some("sa")
        );
        assert_eq!(
            wire_action_partial("<form wire:submit.prevent=\"").as_deref(),
            Some("")
        );
        assert_eq!(wire_action_partial("<input wire:model=\"na"), None);
        assert_eq!(wire_action_partial("<button wire:click=\"save("), None);
        assert_eq!(
            tag_partial("<livewire:admin.us").as_deref(),
            Some("admin.us")
        );
        assert_eq!(tag_partial("@livewire('cou").as_deref(), Some("cou"));
        assert_eq!(tag_partial("<livewire:counter />"), None);
    }

    const SFC: &str = "<?php\n\nuse Livewire\\Component;\n\nnew class extends Component\n{\n    public int $count = 0;\n    public string $label = '';\n\n    public function increment(): void { $this->count++; }\n};\n?>\n\n<div>\n    <p>A public notice for $visitors.</p>\n    <button wire:click=\"increment\">{{ $count }}</button>\n    <input wire:model=\"label\">\n</div>\n";

    #[test]
    fn a_single_file_component_reads_only_its_class_half() {
        assert!(is_single_file(SFC));
        assert_eq!(properties(SFC), vec!["count", "label"]);
        assert_eq!(actions(SFC), vec!["increment"]);
        assert_eq!(property_line(SFC, "label"), Some(7));
        assert!(!is_single_file(
            "<?php\nclass Counter extends Component {}\n"
        ));
    }

    #[test]
    fn renaming_in_a_single_file_touches_class_and_markup_once() {
        let out = single_file_rename(SFC, "count", "total");
        assert!(out.contains("public int $total = 0;"));
        assert!(out.contains("$this->total++"));
        assert!(out.contains("{{ $total }}"));
        assert!(out.contains("$visitors"));
        assert_eq!(out.matches("total").count(), 3);
    }

    fn project(lock_version: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("e-lw-{}-{lock_version}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let v = root.join("resources/views");
        for dir in [
            "components/billing",
            "livewire/settings",
            "pages",
            "components/⚡stats",
            "livewire",
        ] {
            std::fs::create_dir_all(v.join(dir)).unwrap();
        }
        std::fs::create_dir_all(root.join("app/Livewire/Admin")).unwrap();
        std::fs::write(
            root.join("app/Livewire/Admin/UserTable.php"),
            "<?php class UserTable {}",
        )
        .unwrap();
        std::fs::write(v.join("livewire/admin-table.blade.php"), "<div></div>").unwrap();
        std::fs::write(v.join("components/⚡counter.blade.php"), SFC).unwrap();
        std::fs::write(v.join("components/billing/⚡invoice.blade.php"), SFC).unwrap();
        std::fs::write(
            v.join("components/button.blade.php"),
            "<button>{{ $slot }}</button>",
        )
        .unwrap();
        std::fs::write(v.join("livewire/settings/profile.blade.php"), SFC).unwrap();
        std::fs::write(v.join("pages/dashboard.blade.php"), SFC).unwrap();
        std::fs::write(
            v.join("components/⚡stats/stats.php"),
            "<?php new class extends Component {};",
        )
        .unwrap();
        std::fs::write(v.join("components/⚡stats/stats.blade.php"), "<div></div>").unwrap();
        std::fs::write(
            root.join("composer.lock"),
            format!("{{\"packages\": [{{\"name\": \"livewire/livewire\", \"version\": \"{lock_version}\"}}]}}"),
        )
        .unwrap();
        root
    }

    #[test]
    fn livewire_4_components_are_found_by_their_names() {
        let root = project("v4.4.1");
        assert_eq!(livewire_major(&root), Some(4));
        let names = component_names(&root);
        assert_eq!(
            names,
            vec![
                "admin.user-table",
                "billing.invoice",
                "counter",
                "pages::dashboard",
                "settings.profile",
                "stats",
            ]
        );
        let stats = resolve_name(&root, "stats").unwrap();
        assert!(stats.ends_with("⚡stats/stats.blade.php"));
        let comp = resolve(
            &root,
            &root.join("resources/views/components/⚡stats/stats.php"),
        )
        .unwrap();
        assert!(comp.view_file.ends_with("stats.blade.php"));
        let sfc = root.join("resources/views/livewire/settings/profile.blade.php");
        let comp = resolve(&root, &sfc).unwrap();
        assert_eq!(comp.class_file, comp.view_file);
    }

    #[test]
    fn volt_pages_have_no_namespace_on_livewire_3() {
        let root = project("v3.6.3");
        assert!(component_names(&root).contains(&"dashboard".to_string()));
    }

    #[test]
    fn the_component_named_under_the_caret() {
        // `|` marks the caret.
        let at = |src: &str| component_at(&src.replace('|', ""), src.find('|').unwrap());
        assert_eq!(
            at("<livewire:billing.inv|oice />").as_deref(),
            Some("billing.invoice")
        );
        assert_eq!(
            at("<livewire:pages::dash|board />").as_deref(),
            Some("pages::dashboard")
        );
        assert_eq!(at("@livewire('coun|ter')").as_deref(), Some("counter"));
        assert_eq!(
            at("Route::livewire('settings/profile', 'settings.pro|file');").as_deref(),
            Some("settings.profile")
        );
        assert_eq!(at("<x-but|ton />"), None);
        assert_eq!(at("Route::livewire('settings/pro|file', 'x');"), None);
    }
}
