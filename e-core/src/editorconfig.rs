//! Minimal [EditorConfig](https://editorconfig.org) support: parse `.editorconfig`
//! files, walk up from a file resolving the properties that apply to it, and
//! expose the ones the editor acts on (indent, trailing whitespace, final
//! newline, charset).

use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentStyle {
    Space,
    Tab,
}

/// Resolved EditorConfig properties for a file (only the ones we use).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorConfig {
    pub indent_style: Option<IndentStyle>,
    pub indent_size: Option<usize>,
    pub tab_width: Option<usize>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub charset: Option<String>,
}

impl EditorConfig {
    /// The effective tab width: `tab_width`, else `indent_size`.
    pub fn effective_tab_width(&self) -> Option<usize> {
        self.tab_width.or(self.indent_size)
    }

    fn apply(&mut self, key: &str, val: &str) {
        let v = val.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "indent_style" => {
                self.indent_style = match v.to_ascii_lowercase().as_str() {
                    "space" => Some(IndentStyle::Space),
                    "tab" => Some(IndentStyle::Tab),
                    _ => self.indent_style,
                }
            }
            "indent_size" => {
                if let Ok(n) = v.parse::<usize>() {
                    self.indent_size = Some(n);
                }
            }
            "tab_width" => {
                if let Ok(n) = v.parse::<usize>() {
                    self.tab_width = Some(n);
                }
            }
            "trim_trailing_whitespace" => self.trim_trailing_whitespace = parse_bool(v),
            "insert_final_newline" => self.insert_final_newline = parse_bool(v),
            "charset" => self.charset = Some(v.to_string()),
            _ => {}
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// One parsed `.editorconfig`: whether it's a root, and its `(glob, props)`
/// sections in file order.
#[derive(Debug, Default, PartialEq)]
pub struct ParsedFile {
    pub root: bool,
    pub sections: Vec<(String, HashMap<String, String>)>,
}

/// Parse `.editorconfig` text into its preamble `root` flag and glob sections.
pub fn parse(content: &str) -> ParsedFile {
    let mut out = ParsedFile::default();
    let mut current: Option<(String, HashMap<String, String>)> = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(sec) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(c) = current.take() {
                out.sections.push(c);
            }
            current = Some((sec.to_string(), HashMap::new()));
        } else if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            match current.as_mut() {
                Some((_, props)) => {
                    props.insert(k, v);
                }
                None => {
                    // Preamble: only `root` is meaningful.
                    if k.eq_ignore_ascii_case("root") {
                        out.root = parse_bool(&v).unwrap_or(false);
                    }
                }
            }
        }
    }
    if let Some(c) = current.take() {
        out.sections.push(c);
    }
    out
}

/// Resolve the EditorConfig for `file_path` by walking up the directory tree,
/// reading `.editorconfig` files and applying matching sections (nearer files
/// and later sections win), stopping at a `root = true` file.
pub fn resolve(file_path: &Path) -> EditorConfig {
    // Collect (config_dir, parsed) from the file upward, until a root.
    let mut chain: Vec<(std::path::PathBuf, ParsedFile)> = Vec::new();
    let mut dir = file_path.parent().map(|p| p.to_path_buf());
    while let Some(d) = dir {
        let cfg = d.join(".editorconfig");
        if let Ok(content) = std::fs::read_to_string(&cfg) {
            let parsed = parse(&content);
            let is_root = parsed.root;
            chain.push((d.clone(), parsed));
            if is_root {
                break;
            }
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    // Apply from the top-most (root) down to the nearest so nearer wins.
    let mut result = EditorConfig::default();
    for (config_dir, parsed) in chain.iter().rev() {
        for (glob, props) in &parsed.sections {
            if matches_glob(glob, config_dir, file_path) {
                for (k, v) in props {
                    result.apply(k, v);
                }
            }
        }
    }
    result
}

/// Whether an EditorConfig `glob` (relative to `config_dir`) matches `file_path`.
fn matches_glob(glob: &str, config_dir: &Path, file_path: &Path) -> bool {
    let Ok(rel) = file_path.strip_prefix(config_dir) else {
        return false;
    };
    let rel = rel.to_string_lossy().replace('\\', "/");
    // A glob without a slash matches against the file name in any directory.
    let name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for pat in expand_braces(glob) {
        if pat.contains('/') {
            if glob_match(&pat, &rel) {
                return true;
            }
        } else if glob_match(&pat, &name) {
            return true;
        }
    }
    false
}

/// Expand `{a,b,c}` alternations into concrete patterns (one level; nested
/// braces are handled by repeated expansion).
fn expand_braces(pat: &str) -> Vec<String> {
    let Some(open) = pat.find('{') else {
        return vec![pat.to_string()];
    };
    // Find the matching close for this open.
    let mut depth = 0;
    let mut close = None;
    for (i, c) in pat[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![pat.to_string()];
    };
    let (pre, rest) = (&pat[..open], &pat[open + 1..close]);
    let post = &pat[close + 1..];
    let mut out = Vec::new();
    for alt in split_top_commas(rest) {
        for tail in expand_braces(post) {
            out.push(format!("{pre}{alt}{tail}"));
        }
    }
    out
}

fn split_top_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '{' => {
                depth += 1;
                cur.push(c);
            }
            '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

/// Match a wildcard `pattern` (`*`, `**`, `?`) against `text`. `*` does not
/// cross `/`; `**` does.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn m(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => {
                if p.get(1) == Some(&'*') {
                    // `**` matches anything (including `/`).
                    let rest = &p[2..];
                    (0..=t.len()).any(|i| m(rest, &t[i..]))
                } else {
                    // `*` matches anything except `/`.
                    let rest = &p[1..];
                    if m(rest, t) {
                        return true;
                    }
                    let mut i = 0;
                    while i < t.len() && t[i] != '/' {
                        i += 1;
                        if m(rest, &t[i..]) {
                            return true;
                        }
                    }
                    false
                }
            }
            '?' => !t.is_empty() && t[0] != '/' && m(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0] == c && m(&p[1..], &t[1..]),
        }
    }
    m(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_root_and_sections() {
        let p = parse(
            "root = true\n\n[*]\nindent_style = space\nindent_size = 2\n\n\
             [*.rs]\nindent_size = 4\ntrim_trailing_whitespace = true\n",
        );
        assert!(p.root);
        assert_eq!(p.sections.len(), 2);
        assert_eq!(p.sections[0].0, "*");
        assert_eq!(
            p.sections[1].1.get("indent_size").map(String::as_str),
            Some("4")
        );
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.py"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("*", "a/b")); // * doesn't cross /
        assert!(glob_match("**", "a/b/c"));
        assert!(glob_match("src/**", "src/a/b.rs"));
        assert!(glob_match("?.txt", "a.txt"));
    }

    #[test]
    fn brace_expansion() {
        let mut e = expand_braces("*.{js,ts}");
        e.sort();
        assert_eq!(e, vec!["*.js".to_string(), "*.ts".to_string()]);
    }

    #[test]
    fn config_apply_precedence() {
        // `[*]` then `[*.rs]`: rs overrides indent_size.
        let mut c = EditorConfig::default();
        c.apply("indent_size", "2");
        c.apply("indent_size", "4");
        assert_eq!(c.effective_tab_width(), Some(4));
        c.apply("trim_trailing_whitespace", "true");
        assert_eq!(c.trim_trailing_whitespace, Some(true));
    }
}
