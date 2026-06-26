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
    let (lang_fn, highlights, injections, locals) = match language {
        Language::Rust => (
            tree_sitter_rust::LANGUAGE,
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        ),
        Language::Python => (
            tree_sitter_python::LANGUAGE,
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        Language::JavaScript => (
            tree_sitter_javascript::LANGUAGE,
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        Language::Json => (
            tree_sitter_json::LANGUAGE,
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        Language::Go => (
            tree_sitter_go::LANGUAGE,
            tree_sitter_go::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        Language::C => (
            tree_sitter_c::LANGUAGE,
            tree_sitter_c::HIGHLIGHT_QUERY,
            "",
            "",
        ),
        _ => return None,
    };

    let mut config = HighlightConfiguration::new(
        lang_fn.into(),
        language.name(),
        highlights,
        injections,
        locals,
    )
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

    with_config(language, |config| {
        let Some(config) = config else {
            return;
        };
        let mut highlighter = Highlighter::new();
        let events = match highlighter.highlight(config, text.as_bytes(), None, |_| None) {
            Ok(ev) => ev,
            Err(_) => return,
        };

        let mut stack: Vec<Highlight> = Vec::new();
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(h),
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    let Some(h) = stack.last() else { continue };
                    let Some(name) = NAMES.get(h.0) else { continue };
                    let kind = name_to_kind(name);
                    push_span(&line_bounds, &mut lines, start, end, kind);
                }
                Err(_) => return,
            }
        }
    });

    lines
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
