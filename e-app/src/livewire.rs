//! Livewire refactoring support: a component's class and Blade view pretend to
//! be one thing, so `e` treats them that way — `wire:model` completes from the
//! class's public properties, you can jump between the two files, and renaming a
//! property updates both the class and every `wire:` reference in the view.

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

/// Resolve the component that `path` belongs to (given either its view or class).
pub fn resolve(root: &Path, path: &Path) -> Option<Component> {
    let rel = |base: &Path| path.strip_prefix(base).ok().map(|p| p.to_path_buf());

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

/// Line (0-based) of the `public $prop` declaration in the class, if present.
pub fn property_line(class_src: &str, prop: &str) -> Option<usize> {
    let needle = format!("${prop}");
    for (i, line) in class_src.lines().enumerate() {
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
}
