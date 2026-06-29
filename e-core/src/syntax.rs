//! Tree-sitter syntax highlighting.
//!
//! Produces, for a given language and source text, a per-line list of coloured
//! spans (line-local byte ranges). The GUI layer maps [`HighlightKind`] to
//! actual colours, so this crate stays free of any UI dependency.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

use crate::language::Language;

/// Semantic token classes we colour. Capture names from a grammar's
/// `highlights.scm` are mapped onto these by longest-prefix matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
    Constant,
    Variable,
    Property,
    Operator,
    Punctuation,
    Namespace,
    Attribute,
    Constructor,
    Label,
    Tag,
    Escape,
}

/// The capture names we recognise, in priority order. The index of each name
/// here is what tree-sitter hands back as a [`Highlight`].
const NAMES: &[&str] = &[
    "keyword",
    "function",
    "function.macro",
    "function.method",
    "type",
    "type.builtin",
    "constructor",
    "string",
    "string.special",
    "escape",
    "number",
    "comment",
    "constant",
    "constant.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "property",
    "operator",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "namespace",
    "module",
    "attribute",
    "label",
    "tag",
];

fn name_to_kind(name: &str) -> HighlightKind {
    // Match on the most specific prefix first.
    let base = name.split('.').next().unwrap_or(name);
    match base {
        "keyword" => HighlightKind::Keyword,
        "function" => HighlightKind::Function,
        "constructor" => HighlightKind::Constructor,
        "type" => HighlightKind::Type,
        "string" => HighlightKind::String,
        "escape" => HighlightKind::Escape,
        "number" | "float" => HighlightKind::Number,
        "comment" => HighlightKind::Comment,
        "constant" => HighlightKind::Constant,
        "variable" => HighlightKind::Variable,
        "property" | "field" => HighlightKind::Property,
        "operator" => HighlightKind::Operator,
        "punctuation" => HighlightKind::Punctuation,
        "namespace" | "module" => HighlightKind::Namespace,
        "attribute" => HighlightKind::Attribute,
        "label" => HighlightKind::Label,
        "tag" => HighlightKind::Tag,
        _ => HighlightKind::Variable,
    }
}

/// A coloured span within a single line (line-local byte offsets).
#[derive(Debug, Clone, Copy)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
    pub kind: HighlightKind,
}

fn build_config(language: Language) -> Option<HighlightConfiguration> {
    // (grammar, highlights, injections, locals) — queries owned so we can
    // concatenate (e.g. TypeScript = JavaScript + TypeScript highlights).
    let (lang_fn, highlights, injections, locals): (tree_sitter::Language, String, String, String) =
        match language {
            Language::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
                tree_sitter_rust::INJECTIONS_QUERY.into(),
                String::new(),
            ),
            Language::Python => (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::HIGHLIGHTS_QUERY.into(),
                String::new(),
                String::new(),
            ),
            Language::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                tree_sitter_javascript::HIGHLIGHT_QUERY.into(),
                tree_sitter_javascript::INJECTIONS_QUERY.into(),
                tree_sitter_javascript::LOCALS_QUERY.into(),
            ),
            Language::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
                tree_sitter_javascript::INJECTIONS_QUERY.into(),
                tree_sitter_typescript::LOCALS_QUERY.into(),
            ),
            Language::Json => (
                tree_sitter_json::LANGUAGE.into(),
                tree_sitter_json::HIGHLIGHTS_QUERY.into(),
                String::new(),
                String::new(),
            ),
            Language::Go => (
                tree_sitter_go::LANGUAGE.into(),
                tree_sitter_go::HIGHLIGHTS_QUERY.into(),
                String::new(),
                String::new(),
            ),
            Language::C => (
                tree_sitter_c::LANGUAGE.into(),
                tree_sitter_c::HIGHLIGHT_QUERY.into(),
                String::new(),
                String::new(),
            ),
            Language::Php => (
                tree_sitter_php::LANGUAGE_PHP.into(),
                tree_sitter_php::HIGHLIGHTS_QUERY.into(),
                tree_sitter_php::INJECTIONS_QUERY.into(),
                String::new(),
            ),
            Language::Css => (
                tree_sitter_css::LANGUAGE.into(),
                tree_sitter_css::HIGHLIGHTS_QUERY.into(),
                String::new(),
                String::new(),
            ),
            // Blade (Laravel/Livewire) and Vue SFCs are HTML-dominant; use the
            // HTML grammar so tags, attributes and Tailwind classes highlight.
            Language::Html | Language::Blade | Language::Vue => (
                tree_sitter_html::LANGUAGE.into(),
                tree_sitter_html::HIGHLIGHTS_QUERY.into(),
                tree_sitter_html::INJECTIONS_QUERY.into(),
                String::new(),
            ),
            Language::Svelte => (
                tree_sitter_svelte_ng::LANGUAGE.into(),
                tree_sitter_svelte_ng::HIGHLIGHTS_QUERY.into(),
                tree_sitter_svelte_ng::INJECTIONS_QUERY.into(),
                tree_sitter_svelte_ng::LOCALS_QUERY.into(),
            ),
            _ => return None,
        };

    let mut config =
        HighlightConfiguration::new(lang_fn, language.name(), &highlights, &injections, &locals)
            .ok()?;
    config.configure(NAMES);
    Some(config)
}

thread_local! {
    static CONFIGS: RefCell<HashMap<Language, Option<Rc<HighlightConfiguration>>>> =
        RefCell::new(HashMap::new());
}

fn with_config<R>(language: Language, f: impl FnOnce(Option<&HighlightConfiguration>) -> R) -> R {
    CONFIGS.with(|cell| {
        let mut map = cell.borrow_mut();
        let entry = map
            .entry(language)
            .or_insert_with(|| build_config(language).map(Rc::new));
        f(entry.as_deref())
    })
}

/// Compute per-line highlight spans for `text`.
///
/// The returned vector has one entry per line. If the language is unsupported
/// or parsing fails, every line is empty (the editor falls back to plain text).
pub fn highlight_lines(language: Language, text: &str) -> Vec<Vec<LineSpan>> {
    let line_bounds = line_bounds(text);
    let mut lines: Vec<Vec<LineSpan>> = vec![Vec::new(); line_bounds.len()];

    let spans = match language {
        Language::Blade => blade_spans(text),
        Language::Html | Language::Vue => merge_spans(ts_spans(language, text), class_spans(text)),
        _ => ts_spans(language, text),
    };
    for (start, end, kind) in spans {
        push_span(&line_bounds, &mut lines, start, end, kind);
    }
    lines
}

/// Highlight Tailwind utility classes inside `class="…"` / `class='…'`
/// attributes: variant prefixes (`sm:`, `dark:`, `hover:`) as keywords, the
/// utility as a function colour, and arbitrary values `[…]` as numbers. Scanned
/// textually so it survives even when the surrounding markup fails to parse.
fn class_spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let len = text.len();
    let mut search = 0;
    while let Some(rel) = text[search..].find("class") {
        let pos = search + rel;
        search = pos + 5;
        if pos > 0 && is_word(bytes[pos - 1]) {
            continue; // part of a longer word
        }
        let mut j = pos + 5;
        while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        if j >= len || bytes[j] != b'=' {
            continue;
        }
        j += 1;
        while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        if j >= len || (bytes[j] != b'"' && bytes[j] != b'\'') {
            continue;
        }
        let quote = bytes[j] as char;
        let vstart = j + 1;
        let Some(qrel) = text[vstart..].find(quote) else {
            continue;
        };
        let vend = vstart + qrel;
        color_classes(&text[vstart..vend], vstart, &mut out);
        search = vend + 1;
    }
    out
}

fn color_classes(val: &str, base: usize, out: &mut Vec<Span>) {
    let b = val.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        while i < n && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        let ts = i;
        let mut bracket = 0i32;
        while i < n && (!b[i].is_ascii_whitespace() || bracket > 0) {
            match b[i] {
                b'[' => bracket += 1,
                b']' => bracket -= 1,
                _ => {}
            }
            i += 1;
        }
        let te = i;
        let token = &val[ts..te];
        // Leave Blade/dynamic expressions to the PHP/JS highlighter.
        if token.contains(['{', '}', '$']) {
            continue;
        }
        // Split the variant prefix (everything up to the last top-level ':').
        let mut colon = None;
        let mut br = 0i32;
        for (k, c) in token.char_indices() {
            match c {
                '[' => br += 1,
                ']' => br -= 1,
                ':' if br == 0 => colon = Some(k),
                _ => {}
            }
        }
        let util = colon.map(|k| k + 1).unwrap_or(0);
        if util > 0 {
            out.push((base + ts, base + ts + util, HighlightKind::Keyword));
        }
        if let Some(bs) = token[util..].find('[') {
            let abs = util + bs;
            if abs > util {
                out.push((base + ts + util, base + ts + abs, HighlightKind::Function));
            }
            out.push((base + ts + abs, base + te, HighlightKind::Number));
        } else {
            out.push((base + ts + util, base + te, HighlightKind::Function));
        }
    }
}

type Span = (usize, usize, HighlightKind);

/// Run a tree-sitter grammar and return flat (start, end, kind) byte spans.
fn ts_spans(language: Language, text: &str) -> Vec<Span> {
    with_config(language, |config| {
        let Some(config) = config else {
            return Vec::new();
        };
        let mut highlighter = Highlighter::new();
        let events = match highlighter.highlight(config, text.as_bytes(), None, |_| None) {
            Ok(ev) => ev,
            Err(_) => return Vec::new(),
        };
        let mut stack: Vec<Highlight> = Vec::new();
        let mut out = Vec::new();
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(h),
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Some(h) = stack.last() {
                        if let Some(name) = NAMES.get(h.0) {
                            out.push((start, end, name_to_kind(name)));
                        }
                    }
                }
                Err(_) => return Vec::new(),
            }
        }
        out
    })
}

/// Highlight a fragment of PHP (no `<?php` tag) by wrapping it, mapping the
/// resulting spans back into the original document at `base`.
fn php_fragment_spans(inner: &str, base: usize, out: &mut Vec<Span>) {
    const PREFIX: &str = "<?php ";
    let wrapped = format!("{PREFIX}{inner}");
    for (s, e, k) in ts_spans(Language::Php, &wrapped) {
        if s >= PREFIX.len() {
            out.push((base + s - PREFIX.len(), base + e - PREFIX.len(), k));
        }
    }
}

/// Blade = HTML + embedded PHP (`@php…@endphp`, `{{ }}`, `{!! !!}`) + directives.
fn blade_spans(text: &str) -> Vec<Span> {
    let html = ts_spans(Language::Html, text);
    let mut over: Vec<Span> = Vec::new();
    let len = text.len();
    let mut i = 0;
    while i < len {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        // Blade comment: {{-- … --}}
        if let Some(tail) = rest.strip_prefix("{{--") {
            let end = tail.find("--}}").map(|p| i + 4 + p + 4).unwrap_or(len);
            over.push((i, end, HighlightKind::Comment));
            i = end;
            continue;
        }
        // Raw echo: {!! … !!}
        if let Some(tail) = rest.strip_prefix("{!!") {
            let close = tail.find("!!}").map(|p| i + 3 + p).unwrap_or(len);
            over.push((i, (i + 3).min(len), HighlightKind::Operator));
            php_fragment_spans(
                &text[(i + 3).min(close)..close],
                (i + 3).min(close),
                &mut over,
            );
            let after = (close + 3).min(len);
            if close < len {
                over.push((close, after, HighlightKind::Operator));
            }
            i = after;
            continue;
        }
        // Echo: {{ … }}
        if let Some(tail) = rest.strip_prefix("{{") {
            let close = tail.find("}}").map(|p| i + 2 + p).unwrap_or(len);
            over.push((i, (i + 2).min(len), HighlightKind::Operator));
            php_fragment_spans(
                &text[(i + 2).min(close)..close],
                (i + 2).min(close),
                &mut over,
            );
            let after = (close + 2).min(len);
            if close < len {
                over.push((close, after, HighlightKind::Operator));
            }
            i = after;
            continue;
        }
        // @php … @endphp block.
        let rb = rest.as_bytes();
        if rest.starts_with("@php") && rb.get(4).is_none_or(|b| !is_word(*b)) {
            over.push((i, i + 4, HighlightKind::Keyword));
            if let Some(p) = rest.find("@endphp") {
                let inner_start = i + 4;
                let endphp = i + p;
                php_fragment_spans(&text[inner_start..endphp], inner_start, &mut over);
                over.push((endphp, endphp + 7, HighlightKind::Keyword));
                i = endphp + 7;
            } else {
                i += 4;
            }
            continue;
        }
        // A Blade directive: @word
        if rest.starts_with('@') && rb.get(1).is_some_and(|b| b.is_ascii_alphabetic()) {
            let mut j = 1;
            while j < rb.len() && is_word(rb[j]) {
                j += 1;
            }
            over.push((i, i + j, HighlightKind::Keyword));
            i += j;
            continue;
        }
        i += 1;
    }
    over.extend(class_spans(text));
    merge_spans(html, over)
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Combine base spans with override spans: where they overlap, the override
/// wins. Returns a flat (possibly unsorted) list of non-overlapping spans.
fn merge_spans(base: Vec<Span>, over: Vec<Span>) -> Vec<Span> {
    // Merge the override byte ranges into disjoint intervals.
    let mut iv: Vec<(usize, usize)> = over.iter().map(|(s, e, _)| (*s, *e)).collect();
    iv.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in iv {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }

    let mut out = over;
    for (hs, he, hk) in base {
        let mut cur = hs;
        for &(cs, ce) in &merged {
            if ce <= cur {
                continue;
            }
            if cs >= he {
                break;
            }
            if cs > cur {
                out.push((cur, cs.min(he), hk));
            }
            cur = cur.max(ce);
            if cur >= he {
                break;
            }
        }
        if cur < he {
            out.push((cur, he, hk));
        }
    }
    out
}

/// `(line_start_byte, content_end_byte)` per line, excluding the newline.
fn line_bounds(text: &str) -> Vec<(usize, usize)> {
    let mut bounds = Vec::new();
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        let start = off;
        let end = off + line.len();
        let content_end = if line.ends_with("\r\n") {
            end - 2
        } else if line.ends_with('\n') {
            end - 1
        } else {
            end
        };
        bounds.push((start, content_end));
        off = end;
    }
    if bounds.is_empty() {
        bounds.push((0, 0));
    }
    bounds
}

fn line_of(bounds: &[(usize, usize)], byte: usize) -> usize {
    match bounds.binary_search_by(|&(start, _)| start.cmp(&byte)) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

fn push_span(
    bounds: &[(usize, usize)],
    lines: &mut [Vec<LineSpan>],
    start: usize,
    end: usize,
    kind: HighlightKind,
) {
    let first = line_of(bounds, start);
    let last = line_of(bounds, end.saturating_sub(1)).min(lines.len().saturating_sub(1));
    for line in first..=last {
        let (lstart, lend) = bounds[line];
        let s = start.max(lstart);
        let e = end.min(lend);
        if e > s {
            lines[line].push(LineSpan {
                start: s - lstart,
                end: e - lstart,
                kind,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{highlight_lines, HighlightKind};
    use crate::language::Language;

    #[test]
    fn rust_keyword_highlighted() {
        let lines = highlight_lines(Language::Rust, "fn main() {}\n");
        let kinds: Vec<_> = lines[0].iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&HighlightKind::Keyword));
        assert!(kinds.contains(&HighlightKind::Function));
    }
}

#[cfg(test)]
mod blade_tests {
    use super::{highlight_lines, HighlightKind};
    use crate::language::Language;

    #[test]
    fn blade_directives_and_php() {
        let src =
            "@php\n$x = route('home');\n@endphp\n{{ $user->name }}\n{{-- c --}}\n<div>hi</div>\n";
        let lines = highlight_lines(Language::Blade, src);
        // @php is a keyword
        assert!(
            lines[0].iter().any(|s| s.kind == HighlightKind::Keyword),
            "line0={:?}",
            lines[0]
        );
        // embedded PHP: variable + function/string somewhere on line 1
        let k1: Vec<_> = lines[1].iter().map(|s| s.kind).collect();
        assert!(
            k1.contains(&HighlightKind::Variable) || k1.contains(&HighlightKind::String),
            "line1={:?}",
            lines[1]
        );
        // {{ }} echo has operator braces + variable inside (line 3)
        assert!(
            lines[3].iter().any(|s| s.kind == HighlightKind::Operator),
            "line3={:?}",
            lines[3]
        );
        // comment line 4
        assert!(
            lines[4].iter().any(|s| s.kind == HighlightKind::Comment),
            "line4={:?}",
            lines[4]
        );
        // html tag line 5
        assert!(
            lines[5].iter().any(|s| s.kind == HighlightKind::Tag),
            "line5={:?}",
            lines[5]
        );
    }
}
