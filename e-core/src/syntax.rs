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

/// A language's grammar and its (highlights, injections, locals) queries —
/// owned so they can be concatenated (TypeScript = JavaScript + TypeScript).
type Grammar = (tree_sitter::Language, String, String, String);

fn grammar(language: Language) -> Option<Grammar> {
    let g: Grammar = match language {
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
    Some(g)
}

fn build_config(language: Language) -> Option<HighlightConfiguration> {
    let (lang_fn, highlights, injections, locals) = grammar(language)?;
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
        Language::Html | Language::Vue => {
            let mut over = embedded_spans(text, language == Language::Vue);
            over.extend(class_spans(text));
            merge_spans(ts_spans(language, text), over)
        }
        // Overlay SQL highlighting on raw-SQL strings (DB::select("…"), …).
        Language::Php => merge_spans(ts_spans(Language::Php, text), php_sql_spans(text)),
        // Standalone SQL (the database console) uses the SQL grammar directly.
        Language::Sql => sql_spans(text),
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
        if pos > 0 && bytes[pos - 1] == b':' {
            continue; // `:class` / `x-bind:class`: an expression, not classes
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
    with_config(language, |config| match config {
        Some(config) => extract_spans(config, text),
        None => Vec::new(),
    })
}

/// Run a highlight config over `text` and flatten the events into byte spans.
fn extract_spans(config: &HighlightConfiguration, text: &str) -> Vec<Span> {
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
}

// ---- Inline SQL (SQL strings inside PHP: DB::select("…"), ->whereRaw, …) -----

/// `DB::<method>(...)` calls whose first string argument is raw SQL.
const DB_SQL_METHODS: &[&str] = &[
    "select",
    "selectOne",
    "scalar",
    "statement",
    "insert",
    "update",
    "delete",
    "raw",
    "unprepared",
];

/// Query-builder / connection `->method(...)` calls carrying raw SQL.
const RAW_SQL_METHODS: &[&str] = &[
    "selectRaw",
    "whereRaw",
    "orWhereRaw",
    "havingRaw",
    "orHavingRaw",
    "orderByRaw",
    "groupByRaw",
    "fromRaw",
    "statement",
    "raw",
    "unprepared",
];

/// The SQL grammar config for highlighting SQL fragments (built once per thread).
fn build_sql_config() -> Option<HighlightConfiguration> {
    let mut config = HighlightConfiguration::new(
        tree_sitter_sequel::LANGUAGE.into(),
        "sql",
        tree_sitter_sequel::HIGHLIGHTS_QUERY,
        "",
        "",
    )
    .ok()?;
    config.configure(NAMES);
    Some(config)
}

thread_local! {
    static SQL_CONFIG: RefCell<Option<Option<Rc<HighlightConfiguration>>>> =
        const { RefCell::new(None) };
}

/// Highlight a bare SQL fragment into byte spans (relative to the fragment).
fn sql_spans(sql: &str) -> Vec<Span> {
    SQL_CONFIG.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| build_sql_config().map(Rc::new))
            .as_deref()
            .map(|cfg| extract_spans(cfg, sql))
            .unwrap_or_default()
    })
}

/// Highlight the SQL strings inside PHP `DB::`/`->rawMethod` calls. Returns
/// absolute byte spans, produced by highlighting each SQL string with the SQL
/// grammar directly (which — unlike tree-sitter-highlight's injection layering
/// against PHP's own string highlight — is exact). Merged over the PHP spans.
fn php_sql_spans(text: &str) -> Vec<Span> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    let src = text.as_bytes();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    collect_sql_ranges(tree.root_node(), src, &mut ranges);

    let mut out = Vec::new();
    for (s, e) in ranges {
        if e <= s || e > text.len() {
            continue;
        }
        for (rs, re, kind) in sql_spans(&text[s..e]) {
            out.push((s + rs, s + re, kind));
        }
    }
    out
}

/// The byte range of the raw-SQL string containing `offset`, if the cursor sits
/// inside a recognised PHP DB call (`DB::select("…")`, `->whereRaw('…')`, …).
/// Used by the editor to run / complete the SQL under the cursor.
pub fn php_sql_range_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
        .ok()?;
    let tree = parser.parse(text, None)?;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    collect_sql_ranges(tree.root_node(), text.as_bytes(), &mut ranges);
    ranges
        .into_iter()
        .find(|&(s, e)| offset >= s && offset <= e)
}

/// Walk the PHP tree, collecting the byte range of the SQL string argument of
/// each recognised DB call.
fn collect_sql_ranges(node: tree_sitter::Node, src: &[u8], out: &mut Vec<(usize, usize)>) {
    collect_sql_ranges_in(node, src, 0..usize::MAX, out);
}

/// [`collect_sql_ranges`] for the calls overlapping `within` only, so an
/// incremental update doesn't walk the whole tree.
fn collect_sql_ranges_in(
    node: tree_sitter::Node,
    src: &[u8],
    within: std::ops::Range<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    if node.end_byte() < within.start || node.start_byte() > within.end {
        return;
    }
    let kind = node.kind();
    if kind == "scoped_call_expression" || kind == "member_call_expression" {
        let method = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(src).ok())
            .unwrap_or("");
        let matched = if kind == "scoped_call_expression" {
            let scope = node
                .child_by_field_name("scope")
                .and_then(|n| n.utf8_text(src).ok())
                .unwrap_or("");
            scope == "DB" && DB_SQL_METHODS.contains(&method)
        } else {
            RAW_SQL_METHODS.contains(&method)
        };
        if matched {
            if let Some(args) = node.child_by_field_name("arguments") {
                if let Some(range) = first_string_range(args) {
                    out.push(range);
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_sql_ranges_in(child, src, within.clone(), out);
    }
}

/// The inner content byte range of the first string argument in `arguments`
/// (quotes excluded).
fn first_string_range(arguments: tree_sitter::Node) -> Option<(usize, usize)> {
    let mut cursor = arguments.walk();
    for arg in arguments.children(&mut cursor) {
        if arg.kind() != "argument" {
            continue;
        }
        let mut ac = arg.walk();
        for child in arg.children(&mut ac) {
            match child.kind() {
                // Double-quoted: use the inner `string_content` (no quotes,
                // skips interpolations — those keep PHP highlighting).
                "encapsed_string" => {
                    let mut ec = child.walk();
                    for part in child.children(&mut ec) {
                        if part.kind() == "string_content" {
                            return Some((part.start_byte(), part.end_byte()));
                        }
                    }
                }
                // Single-quoted: strip the surrounding quote bytes.
                "string" => {
                    let (s, e) = (child.start_byte(), child.end_byte());
                    if e >= s + 2 {
                        return Some((s + 1, e - 1));
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Highlight a fragment of PHP (no `<?php` tag) by wrapping it, mapping the
/// resulting spans back into the original document at `base`.
///
/// Each fragment is its own PHP parse, so a view with hundreds of `{{ }}`
/// cost tens of milliseconds a keystroke (36 ms for 600 echoes in 16 KB).
/// Between keystrokes all but the edited fragment are unchanged — and
/// `{{ $user->name }}` recurs — so results are cached by fragment text.
fn php_fragment_spans(inner: &str, base: usize, out: &mut Vec<Span>) {
    fragment_spans(Language::Php, "<?php ", "", inner, base, out);
}

/// Highlight `inner` as `language`, wrapped in `prefix` and `suffix` to make
/// it parse (`<?php `; a JavaScript expression in parentheses), and map the
/// spans back into the document at `base`. Cached by fragment, for the
/// reason [`php_fragment_spans`] gives.
fn fragment_spans(
    language: Language,
    prefix: &str,
    suffix: &str,
    inner: &str,
    base: usize,
    out: &mut Vec<Span>,
) {
    /// A fragment: its language, its wrapping, its text.
    type Key = (Language, String, String);
    thread_local! {
        static FRAGMENTS: RefCell<HashMap<Key, Rc<Vec<Span>>>> = RefCell::new(HashMap::new());
    }
    let key = (
        language,
        format!("{prefix}\u{0}{suffix}"),
        inner.to_string(),
    );
    let spans = FRAGMENTS.with(|cell| {
        if let Some(hit) = cell.borrow().get(&key) {
            return hit.clone();
        }
        let wrapped = format!("{prefix}{inner}{suffix}");
        let end = prefix.len() + inner.len();
        let spans: Rc<Vec<Span>> = Rc::new(
            ts_spans(language, &wrapped)
                .into_iter()
                .filter(|(s, _, _)| *s >= prefix.len() && *s < end)
                .map(|(s, e, k)| (s - prefix.len(), e.min(end) - prefix.len(), k))
                .collect(),
        );
        let mut cache = cell.borrow_mut();
        // Bounded: a big edit session shouldn't grow it without limit.
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(key, spans.clone());
        spans
    });
    out.extend(spans.iter().map(|&(s, e, k)| (base + s, base + e, k)));
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
    merge_spans(merge_spans(html, embedded_spans(text, false)), over)
}

/// What a `<script>` block is written in: TypeScript with `lang="ts"`, JSON
/// for a JSON `type`, nothing for a template or other non-script `type`.
fn script_language(start_tag: tree_sitter::Node, src: &[u8]) -> Option<Language> {
    let mut lang = Language::JavaScript;
    let mut cursor = start_tag.walk();
    for attr in start_tag
        .children(&mut cursor)
        .filter(|n| n.kind() == "attribute")
    {
        let mut c = attr.walk();
        let mut name = "";
        let mut value = String::new();
        for part in attr.children(&mut c) {
            match part.kind() {
                "attribute_name" => name = part.utf8_text(src).unwrap_or(""),
                "attribute_value" => value = part.utf8_text(src).unwrap_or("").to_string(),
                "quoted_attribute_value" => {
                    value = part
                        .utf8_text(src)
                        .unwrap_or("")
                        .trim_matches(['"', '\''])
                        .to_string()
                }
                _ => {}
            }
        }
        let value = value.to_ascii_lowercase();
        match name {
            "lang" if value == "ts" || value == "tsx" || value == "typescript" => {
                lang = Language::TypeScript
            }
            "type" if value.contains("json") => lang = Language::Json,
            "type" if !(value.contains("javascript") || value == "module" || value.is_empty()) => {
                return None
            }
            _ => {}
        }
    }
    Some(lang)
}

/// A Vue attribute that holds an expression: `v-if`, `:prop`, `@click`,
/// `#slot`, `v-bind:…`.
fn vue_directive(name: &str) -> bool {
    name.starts_with("v-")
        || name.starts_with(':')
        || name.starts_with('@')
        || name.starts_with('#')
}

/// Code inside markup: `<script>` blocks as JavaScript (TypeScript with
/// `lang="ts"`), `<style>` blocks as CSS — and in a Vue component, the
/// template's `{{ }}` interpolations and directive values as expressions in
/// the component's script language, the directive names as keywords.
fn embedded_spans(text: &str, vue: bool) -> Vec<Span> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_html::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    let src = text.as_bytes();
    let root = tree.root_node();
    // The template's expressions are in the language of the component's
    // script, which usually comes after it.
    let mut expr_lang = Language::JavaScript;
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "script_element" {
            if let Some(Language::TypeScript) =
                child.child(0).and_then(|tag| script_language(tag, src))
            {
                expr_lang = Language::TypeScript;
            }
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "script_element" | "style_element" => {
                let lang = if node.kind() == "style_element" {
                    Some(Language::Css)
                } else {
                    node.child(0).and_then(|tag| script_language(tag, src))
                };
                let mut c = node.walk();
                let raw = node.children(&mut c).find(|n| n.kind() == "raw_text");
                if let (Some(lang), Some(raw)) = (lang, raw) {
                    let (start, end) = (raw.start_byte(), raw.end_byte());
                    out.extend(
                        ts_spans(lang, &text[start..end])
                            .into_iter()
                            .map(|(s, e, k)| (start + s, start + e, k)),
                    );
                }
                continue;
            }
            "attribute" if vue => {
                let mut c = node.walk();
                let parts: Vec<_> = node.children(&mut c).collect();
                let name = parts.iter().find(|n| n.kind() == "attribute_name");
                let value = parts.iter().find_map(|n| match n.kind() {
                    "attribute_value" => Some(*n),
                    "quoted_attribute_value" => {
                        let mut c = n.walk();
                        let inner = n.children(&mut c).find(|v| v.kind() == "attribute_value");
                        inner
                    }
                    _ => None,
                });
                if let Some(name) = name {
                    let n = name.utf8_text(src).unwrap_or("");
                    if vue_directive(n) {
                        // The directive itself; an argument after it (`v-on:click`,
                        // `:class`) keeps the attribute colour.
                        let head = if n.starts_with("v-") {
                            n.find([':', '.']).unwrap_or(n.len())
                        } else {
                            1
                        };
                        out.push((
                            name.start_byte(),
                            name.start_byte() + head,
                            HighlightKind::Keyword,
                        ));
                        if let Some(v) = value {
                            vue_expression(
                                expr_lang,
                                n,
                                &text[v.start_byte()..v.end_byte()],
                                v.start_byte(),
                                &mut out,
                            );
                        }
                    }
                }
                continue;
            }
            "text" if vue => {
                let t = &text[node.start_byte()..node.end_byte()];
                let mut from = 0;
                while let Some(open) = t[from..].find("{{") {
                    let open = from + open;
                    let Some(close) = t[open + 2..].find("}}").map(|c| open + 2 + c) else {
                        break;
                    };
                    let base = node.start_byte();
                    out.push((base + open, base + open + 2, HighlightKind::Operator));
                    fragment_spans(
                        expr_lang,
                        "(",
                        ")",
                        &t[open + 2..close],
                        base + open + 2,
                        &mut out,
                    );
                    out.push((base + close, base + close + 2, HighlightKind::Operator));
                    from = close + 2;
                }
                continue;
            }
            _ => {}
        }
        let mut c = node.walk();
        stack.extend(node.children(&mut c));
    }
    out
}

/// A directive's value as code: a handler (`@click`) as statements, the
/// `v-for` loop's `in`/`of` as a keyword between two expressions, anything
/// else as an expression.
fn vue_expression(lang: Language, directive: &str, value: &str, base: usize, out: &mut Vec<Span>) {
    if directive.starts_with('@') || directive.starts_with("v-on") {
        fragment_spans(lang, "", "", value, base, out);
        return;
    }
    if directive == "v-for" {
        if let Some(at) = value.find(" in ").or_else(|| value.find(" of ")) {
            fragment_spans(lang, "(", ")", &value[..at], base, out);
            out.push((base + at + 1, base + at + 3, HighlightKind::Keyword));
            fragment_spans(lang, "(", ")", &value[at + 4..], base + at + 4, out);
            return;
        }
    }
    fragment_spans(lang, "(", ")", value, base, out);
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
        // `merged` is sorted and disjoint, so jump straight to the first
        // interval that can overlap this span instead of rescanning from the
        // front. Rescanning made this O(base × merged): in a PHP file every
        // `DB::select("…")` adds an interval and every token adds a base span,
        // so cost grew with the *product* — 312 KB of PHP cost 2.4 seconds per
        // keystroke, four times the size of a 78 KB file but eight times the time.
        let first = merged.partition_point(|&(_, ce)| ce <= cur);
        for &(cs, ce) in &merged[first..] {
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

// ---- Incremental highlighting ------------------------------------------------
//
// `highlight_lines` re-parses and re-highlights the whole file, which is what a
// keystroke cost: ~10 ms for 60 KB of PHP (two full parses, then every capture
// in the file). A `SyntaxTree` keeps its parse tree between edits instead, so
// tree-sitter re-parses only what changed, and re-highlights only the lines
// whose text or syntax changed, splicing them into the existing per-line spans.
//
// It reads the same highlight queries and must colour exactly as
// `tree-sitter-highlight` does — the tests hold it to that, edit by edit:
//   * a node captured by several patterns takes the *last* pattern's capture;
//   * where captured nodes nest, the innermost wins;
//   * injections are ignored (the full path passes no injection callback).
// Languages with a locals query (JavaScript, TypeScript, Svelte) resolve
// variable references through scopes, which this doesn't model, and the
// HTML-family languages layer extra scanning on top; both keep the full path.

/// A highlight query compiled for direct use, with each capture's colour.
struct TreeQuery {
    language: tree_sitter::Language,
    query: tree_sitter::Query,
    kinds: Vec<Option<HighlightKind>>,
    /// How many levels above a node a pattern that involves it can be rooted:
    /// the deepest pattern's nesting, less one.
    reach: usize,
}

thread_local! {
    static TREE_QUERIES: RefCell<HashMap<Language, Option<Rc<TreeQuery>>>> =
        RefCell::new(HashMap::new());
}

fn tree_query(language: Language) -> Option<Rc<TreeQuery>> {
    if !SyntaxTree::supports(language) {
        return None;
    }
    TREE_QUERIES.with(|cell| {
        cell.borrow_mut()
            .entry(language)
            .or_insert_with(|| {
                let (lang, highlights, _, _) = grammar(language)?;
                let query = tree_sitter::Query::new(&lang, &highlights).ok()?;
                let kinds = query
                    .capture_names()
                    .iter()
                    .map(|name| recognized_name(name).map(name_to_kind))
                    .collect();
                Some(Rc::new(TreeQuery {
                    language: lang,
                    query,
                    kinds,
                    reach: pattern_depth(&highlights).saturating_sub(1),
                }))
            })
            .clone()
    })
}

/// The deepest nesting of node patterns in a query: `(a (b (c)))` is 3.
/// Predicates, strings and comments don't count.
fn pattern_depth(query: &str) -> usize {
    let b = query.as_bytes();
    let (mut depth, mut max, mut parens) = (0usize, 0usize, Vec::<bool>::new());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b';' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' => {
                let node = b.get(i + 1) != Some(&b'#');
                parens.push(node);
                if node {
                    depth += 1;
                    max = max.max(depth);
                }
            }
            b')' if parens.pop() == Some(true) => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    max
}

/// The entry of [`NAMES`] a capture name maps to: the one with the most
/// dot-separated parts that all occur in the capture name — exactly
/// `HighlightConfiguration::configure`'s rule.
fn recognized_name(capture: &str) -> Option<&'static str> {
    let parts: Vec<&str> = capture.split('.').collect();
    let mut best: Option<(&'static str, usize)> = None;
    for &name in NAMES {
        let len = name.split('.').count();
        if name.split('.').all(|p| parts.contains(&p)) && best.is_none_or(|(_, l)| len > l) {
            best = Some((name, len));
        }
    }
    best.map(|(n, _)| n)
}

/// Highlight spans for the nodes overlapping `range`, the way
/// `tree-sitter-highlight` would produce them for the whole file.
fn tree_spans(
    q: &TreeQuery,
    tree: &tree_sitter::Tree,
    text: &str,
    range: std::ops::Range<usize>,
) -> Vec<Span> {
    use tree_sitter::StreamingIterator;
    let mut cursor = tree_sitter::QueryCursor::new();
    cursor.set_byte_range(range);
    let mut captures = cursor.captures(&q.query, tree.root_node(), text.as_bytes());

    let mut spans = SpanStack::default();
    // Captures of one node arrive together, ordered by pattern; the last wins.
    let mut pending: Option<(tree_sitter::Node, u32)> = None;
    while let Some((m, i)) = captures.next() {
        let c = m.captures[*i];
        match pending {
            Some((node, _)) if node == c.node => pending = Some((node, c.index)),
            _ => {
                if let Some((node, capture)) = pending.take() {
                    spans.open(node.byte_range(), q.kinds[capture as usize]);
                }
                pending = Some((c.node, c.index));
            }
        }
    }
    if let Some((node, capture)) = pending {
        spans.open(node.byte_range(), q.kinds[capture as usize]);
    }
    spans.finish()
}

/// Turns nested highlighted ranges, opened in document order, into flat
/// spans where the innermost colour wins — `HighlightEvent`s, flattened.
#[derive(Default)]
struct SpanStack {
    /// Open highlights: (end byte, colour), innermost last.
    open: Vec<(usize, HighlightKind)>,
    /// Everything before this has been emitted.
    offset: usize,
    out: Vec<Span>,
}

impl SpanStack {
    /// Close the highlights that end at or before `pos`.
    fn close_until(&mut self, pos: usize) {
        while let Some(&(end, kind)) = self.open.last() {
            if end > pos {
                break;
            }
            if self.offset < end {
                self.out.push((self.offset, end, kind));
                self.offset = end;
            }
            self.open.pop();
        }
    }

    /// Open a highlight over `range` (a capture without a colour opens none,
    /// but still closes what ended before it).
    fn open(&mut self, range: std::ops::Range<usize>, kind: Option<HighlightKind>) {
        self.close_until(range.start);
        let Some(kind) = kind else {
            return;
        };
        if let Some(&(_, outer)) = self.open.last() {
            if self.offset < range.start {
                self.out.push((self.offset, range.start, outer));
            }
        }
        self.offset = self.offset.max(range.start);
        self.open.push((range.end, kind));
    }

    fn finish(mut self) -> Vec<Span> {
        self.close_until(usize::MAX);
        self.out
    }
}

/// A document's parse tree, kept up to date edit by edit, for highlighting
/// that costs what changed rather than the whole file.
pub struct SyntaxTree {
    language: Language,
    query: Rc<TreeQuery>,
    parser: tree_sitter::Parser,
    tree: tree_sitter::Tree,
    /// The text `tree` describes.
    text: String,
    /// Lines the last `update` re-highlighted (for tests and measurements).
    redone: usize,
}

impl SyntaxTree {
    /// Whether `language` is highlighted incrementally (see the section note).
    pub fn supports(language: Language) -> bool {
        matches!(
            language,
            Language::Php
                | Language::Rust
                | Language::Python
                | Language::Go
                | Language::C
                | Language::Json
                | Language::Css
        )
    }

    /// Parse `text`, returning the tree and the per-line spans for all of it
    /// (the same as [`highlight_lines`] would). `None` for an unsupported
    /// language.
    pub fn new(language: Language, text: &str) -> Option<(Self, Vec<Vec<LineSpan>>)> {
        let query = tree_query(language)?;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&query.language).ok()?;
        let tree = parser.parse(text, None)?;
        let st = SyntaxTree {
            language,
            query,
            parser,
            tree,
            text: text.to_string(),
            redone: 0,
        };
        let bounds = line_bounds(text);
        let mut lines = vec![Vec::new(); bounds.len()];
        st.highlight_into(&bounds, 0, bounds.len() - 1, &mut lines);
        Some((st, lines))
    }

    /// Bring the tree up to `new_text` and update `lines` (the spans this tree
    /// last produced) to match it, re-highlighting only what changed.
    pub fn update(&mut self, new_text: &str, lines: &mut Vec<Vec<LineSpan>>) {
        let (at, old_end, new_end) = edit_between(&self.text, new_text);
        if at == old_end && at == new_end {
            return;
        }
        let edit = tree_sitter::InputEdit {
            start_byte: at,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position: point_at(&self.text, at),
            old_end_position: point_at(&self.text, old_end),
            new_end_position: point_at(new_text, new_end),
        };
        let mut old = self.tree.clone();
        old.edit(&edit);
        // Around broken code, error recovery can settle on a different tree
        // incrementally than a fresh parse would, so colours there may differ
        // from `highlight_lines` until the code parses again — as in every
        // editor that parses incrementally. They are always the colours of
        // this tree, and identical once the code is valid.
        let Some(tree) = self.parser.parse(new_text, Some(&old)) else {
            return;
        };
        // `lines` must be what this tree last produced; if anyone replaced
        // them, splicing would misplace colours, so start over instead.
        let foreign = lines.len() != line_bounds(&self.text).len();

        // What to redo: the edited bytes; wherever the two trees' structure
        // differs, widened to every pattern that could see the difference
        // (see `differing_ranges`); what tree-sitter itself reports as
        // changed; and — for PHP — any DB call around all that, since
        // renaming `select` changes the colour of a SQL string lines away.
        let mut changed = Vec::new();
        changed.push(at..new_end);
        let mut budget = DIFF_BUDGET;
        differing_ranges(
            old.root_node(),
            tree.root_node(),
            self.query.reach,
            &mut budget,
            &mut changed,
        );
        changed.extend(old.changed_ranges(&tree).map(|r| r.start_byte..r.end_byte));
        let whole = foreign || budget == 0;
        let mut lo = changed.iter().map(|r| r.start).min().unwrap_or(at);
        let mut hi = changed.iter().map(|r| r.end).max().unwrap_or(new_end);
        self.tree = tree;
        self.text = new_text.to_string();
        let bounds = line_bounds(new_text);
        if whole {
            self.redone = bounds.len();
            let mut all = vec![Vec::new(); bounds.len()];
            self.highlight_into(&bounds, 0, bounds.len() - 1, &mut all);
            *lines = all;
            return;
        }
        let tree = &self.tree;
        if self.language == Language::Php {
            for end in [lo, hi] {
                let mut node = tree.root_node().descendant_for_byte_range(end, end);
                while let Some(n) = node {
                    if matches!(
                        n.kind(),
                        "scoped_call_expression" | "member_call_expression"
                    ) {
                        lo = lo.min(n.start_byte());
                        hi = hi.max(n.end_byte());
                    }
                    node = n.parent();
                }
            }
        }

        let old_line_count = lines.len();
        // Lines before `first` are untouched. Lines after `last` are the old
        // ones, shifted: `last` reaches the line holding `new_end` itself, so
        // the next line starts inside the unchanged tail — an insertion ending
        // in "\n" leaves the rest of its old line on a new one, which must be
        // redone, not reused.
        let first = line_of(&bounds, lo);
        let last = line_of(&bounds, hi.saturating_sub(1).max(new_end)).min(bounds.len() - 1);
        let tail = bounds.len() - (last + 1);
        let old_end = old_line_count.saturating_sub(tail).max(first);
        self.redone = last + 1 - first;
        let mut fresh = vec![Vec::new(); bounds.len()];
        self.highlight_into(&bounds, first, last, &mut fresh);
        lines.splice(first..old_end, fresh.drain(first..=last));
        debug_assert_eq!(lines.len(), bounds.len());
    }

    /// How many lines the last [`Self::update`] re-highlighted.
    pub fn lines_redone(&self) -> usize {
        self.redone
    }

    /// Whether the current text has syntax errors.
    pub fn has_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }

    /// Every line highlighted from the current tree — what `update` keeps in
    /// step incrementally. For tests.
    #[doc(hidden)]
    pub fn highlight_all(&self) -> Vec<Vec<LineSpan>> {
        let bounds = line_bounds(&self.text);
        let mut lines = vec![Vec::new(); bounds.len()];
        self.highlight_into(&bounds, 0, bounds.len() - 1, &mut lines);
        lines
    }

    /// Highlight lines `first..=last` of the current text into `lines`.
    fn highlight_into(
        &self,
        bounds: &[(usize, usize)],
        first: usize,
        last: usize,
        lines: &mut [Vec<LineSpan>],
    ) {
        let text = self.text.as_str();
        let start = bounds[first].0;
        let end = bounds.get(last + 1).map_or(text.len(), |b| b.0);
        let mut spans = tree_spans(&self.query, &self.tree, text, start..end);
        if self.language == Language::Php {
            let mut ranges = Vec::new();
            collect_sql_ranges_in(
                self.tree.root_node(),
                text.as_bytes(),
                start..end,
                &mut ranges,
            );
            let mut over = Vec::new();
            for (s, e) in ranges {
                if e <= s || e > text.len() {
                    continue;
                }
                for (rs, re, kind) in sql_spans(&text[s..e]) {
                    over.push((s + rs, s + re, kind));
                }
            }
            spans = merge_spans(spans, over);
        }
        for (s, e, kind) in spans {
            let s = s.max(start);
            let e = e.min(end);
            if e > s {
                push_span(bounds, lines, s, e, kind);
            }
        }
    }
}

/// Nodes [`differing_ranges`] may compare before giving up and re-highlighting
/// the whole file; a typical edit visits a few dozen.
const DIFF_BUDGET: usize = 20_000;

/// Collect the byte ranges (in new-text coordinates — `old` has been edited)
/// where highlighting may differ between two trees of the same text.
///
/// A subtree the new parse reused is the same node in both, and skipped.
/// Elsewhere the trees are walked in step; where they part ways — a node's
/// kind, flags, children or fields differ — the ranges covered are those of
/// the ancestors `reach` levels up, in both trees: any pattern that involves
/// the differing node is rooted no higher, and captures only inside its root.
/// This is what `Tree::changed_ranges` misses: error recovery re-parenting
/// code that didn't change, which changes its colour all the same.
fn differing_ranges(
    old: tree_sitter::Node,
    new: tree_sitter::Node,
    reach: usize,
    budget: &mut usize,
    out: &mut Vec<std::ops::Range<usize>>,
) {
    if old.id() == new.id() || *budget == 0 {
        return;
    }
    *budget -= 1;
    let same = old.kind_id() == new.kind_id()
        && old.child_count() == new.child_count()
        && old.is_missing() == new.is_missing()
        && old.is_extra() == new.is_extra()
        && old.is_error() == new.is_error();
    if !same {
        out.push(ancestor_range(old, reach));
        out.push(ancestor_range(new, reach));
        return;
    }
    // Same shape: a token is the same token (its text, if edited, lies in the
    // edited range already); anything else, compare child by child.
    let (mut oc, mut nc) = (old.walk(), new.walk());
    let (olds, news): (Vec<_>, Vec<_>) = (
        old.children(&mut oc).collect(),
        new.children(&mut nc).collect(),
    );
    for (i, (o, n)) in olds.into_iter().zip(news).enumerate() {
        if old.field_name_for_child(i as u32) != new.field_name_for_child(i as u32) {
            out.push(ancestor_range(o, reach));
            out.push(ancestor_range(n, reach));
            continue;
        }
        differing_ranges(o, n, reach, budget, out);
    }
}

/// The byte range of `node`'s ancestor `levels` up (or the root's).
fn ancestor_range(node: tree_sitter::Node, levels: usize) -> std::ops::Range<usize> {
    let mut top = node;
    for _ in 0..levels {
        match top.parent() {
            Some(p) => top = p,
            None => break,
        }
    }
    top.byte_range()
}

/// The single edit turning `old` into `new`: `(start, old end, new end)`,
/// trimmed to whole characters.
fn edit_between(old: &str, new: &str) -> (usize, usize, usize) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(pre) || !new.is_char_boundary(pre) {
        pre -= 1;
    }
    let max_suf = a.len().min(b.len()) - pre;
    let mut suf = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suf)
        .take_while(|(x, y)| x == y)
        .count();
    while !old.is_char_boundary(a.len() - suf) || !new.is_char_boundary(b.len() - suf) {
        suf -= 1;
    }
    (pre, a.len() - suf, b.len() - suf)
}

/// Row and byte column of `byte` in `text`, as tree-sitter counts them.
fn point_at(text: &str, byte: usize) -> tree_sitter::Point {
    let before = &text.as_bytes()[..byte];
    let row = before.iter().filter(|&&b| b == b'\n').count();
    let column = byte
        - before
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
    tree_sitter::Point { row, column }
}

#[cfg(test)]
mod tests {
    use super::{merge_spans, Span};

    /// The straightforward O(base × over) merge the fast path replaced. Kept as
    /// the oracle: the optimisation is only allowed to change *how long* the
    /// merge takes, never what it produces.
    fn merge_spans_reference(base: Vec<Span>, over: Vec<Span>) -> Vec<Span> {
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

    fn normalise(mut v: Vec<Span>) -> Vec<Span> {
        v.sort_by_key(|&(s, e, _)| (s, e));
        v
    }

    /// Deterministic pseudo-randomness — no dev-dependency, same cases every run.
    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    #[test]
    fn merge_spans_matches_the_reference_across_many_layouts() {
        use HighlightKind::*;
        let kinds = [Keyword, String, Comment, Number, Variable];
        let mut seed = 0x5EED_1234u64;

        for case in 0..300 {
            let n_base = (lcg(&mut seed) % 40) as usize;
            let n_over = (lcg(&mut seed) % 12) as usize;
            let span = |seed: &mut u64| -> Span {
                let start = (lcg(seed) % 200) as usize;
                let len = 1 + (lcg(seed) % 15) as usize;
                (start, start + len, kinds[(lcg(seed) % 5) as usize])
            };
            // Base spans arrive in document order from tree-sitter.
            let mut base: Vec<Span> = (0..n_base).map(|_| span(&mut seed)).collect();
            base.sort_by_key(|&(s, e, _)| (s, e));
            let over: Vec<Span> = (0..n_over).map(|_| span(&mut seed)).collect();

            assert_eq!(
                normalise(merge_spans(base.clone(), over.clone())),
                normalise(merge_spans_reference(base.clone(), over.clone())),
                "case {case}: base={base:?} over={over:?}"
            );
        }
    }

    #[test]
    fn merge_spans_edge_cases() {
        use HighlightKind::*;
        let cases: Vec<(Vec<Span>, Vec<Span>)> = vec![
            // Nothing to override.
            (vec![(0, 10, Keyword)], vec![]),
            // Nothing underneath.
            (vec![], vec![(2, 4, String)]),
            // Override exactly covers the base span.
            (vec![(0, 10, Keyword)], vec![(0, 10, String)]),
            // Override splits a base span in two.
            (vec![(0, 10, Keyword)], vec![(4, 6, String)]),
            // Several overrides inside one base span.
            (
                vec![(0, 30, Keyword)],
                vec![(2, 4, String), (10, 12, String), (20, 22, String)],
            ),
            // Overlapping overrides collapse into one interval.
            (
                vec![(0, 20, Keyword)],
                vec![(2, 8, String), (5, 12, String)],
            ),
            // Override entirely before / after the base span.
            (vec![(10, 20, Keyword)], vec![(0, 5, String)]),
            (vec![(0, 5, Keyword)], vec![(10, 20, String)]),
            // Touching but not overlapping.
            (vec![(0, 10, Keyword)], vec![(10, 20, String)]),
            // Zero-length base span.
            (vec![(5, 5, Keyword)], vec![(0, 10, String)]),
        ];
        for (i, (base, over)) in cases.into_iter().enumerate() {
            assert_eq!(
                normalise(merge_spans(base.clone(), over.clone())),
                normalise(merge_spans_reference(base.clone(), over.clone())),
                "edge case {i}"
            );
        }
    }

    use super::{highlight_lines, HighlightKind};
    use crate::language::Language;

    #[test]
    fn rust_keyword_highlighted() {
        let lines = highlight_lines(Language::Rust, "fn main() {}\n");
        let kinds: Vec<_> = lines[0].iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&HighlightKind::Keyword));
        assert!(kinds.contains(&HighlightKind::Function));
    }

    /// A standalone SQL document (the database console) highlights clause
    /// keywords directly via the SQL grammar.
    #[test]
    fn standalone_sql_document_highlighted() {
        let src = "SELECT id FROM users WHERE id = 1";
        let spans = &highlight_lines(Language::Sql, src)[0];
        let kind_at = |needle: &str| -> Option<HighlightKind> {
            let at = src.find(needle)?;
            spans
                .iter()
                .find(|s| s.start <= at && at < s.end)
                .map(|s| s.kind)
        };
        assert_eq!(kind_at("SELECT"), Some(HighlightKind::Keyword), "{spans:?}");
        assert_eq!(kind_at("FROM"), Some(HighlightKind::Keyword));
        assert_eq!(kind_at("WHERE"), Some(HighlightKind::Keyword));
    }

    /// SQL inside a PHP `DB::select("…")` string is highlighted as SQL: every
    /// clause keyword (incl. the leading SELECT) is a keyword, not a string.
    #[test]
    fn sql_highlighted_in_php_db_call() {
        let src = "<?php\n$r = DB::select(\"SELECT id FROM users WHERE id = 1\");\n";
        let line = src.lines().nth(1).unwrap();
        let spans = &highlight_lines(Language::Php, src)[1];
        let kind_at = |needle: &str| -> Option<HighlightKind> {
            let at = line.find(needle)?;
            spans
                .iter()
                .find(|s| s.start <= at && at < s.end)
                .map(|s| s.kind)
        };
        assert_eq!(kind_at("SELECT"), Some(HighlightKind::Keyword), "{spans:?}");
        assert_eq!(kind_at("FROM"), Some(HighlightKind::Keyword));
        assert_eq!(kind_at("WHERE"), Some(HighlightKind::Keyword));
    }

    #[test]
    fn sql_highlighted_in_single_quoted_where_raw() {
        let src = "<?php\n$q->whereRaw('price > 100 AND active = 1');\n";
        let line = src.lines().nth(1).unwrap();
        let spans = &highlight_lines(Language::Php, src)[1];
        let at = line.find("AND").unwrap();
        let kind = spans
            .iter()
            .find(|s| s.start <= at && at < s.end)
            .map(|s| s.kind);
        assert_eq!(kind, Some(HighlightKind::Keyword), "{spans:?}");
    }

    #[test]
    fn plain_php_string_stays_string_and_keywords_work() {
        // A non-DB string isn't touched, and PHP keywords still highlight.
        let src = "<?php\nfunction f() { $s = \"SELECT nope\"; return $s; }\n";
        let spans = &highlight_lines(Language::Php, src)[1];
        let line = src.lines().nth(1).unwrap();
        let at = line.find("SELECT").unwrap();
        let kind = spans
            .iter()
            .find(|s| s.start <= at && at < s.end)
            .map(|s| s.kind);
        assert_eq!(
            kind,
            Some(HighlightKind::String),
            "plain string must stay String: {spans:?}"
        );
        assert!(
            spans.iter().any(|s| s.kind == HighlightKind::Keyword),
            "php keywords"
        );
    }
}

#[cfg(test)]
mod embedded_tests {
    use super::{highlight_lines, HighlightKind};
    use crate::language::Language;

    /// The kind at the first `needle` on line `line` (0-based).
    fn kind_at(lang: Language, src: &str, line: usize, needle: &str) -> Option<HighlightKind> {
        let lines = highlight_lines(lang, src);
        let at = src.lines().nth(line)?.find(needle)?;
        lines[line]
            .iter()
            .find(|s| s.start <= at && at < s.end)
            .map(|s| s.kind)
    }

    const SFC: &str = r#"<template>
  <li v-for="item in items" :key="item.id" @click="select(item)">
    {{ item.name.toUpperCase() }}
  </li>
</template>

<script setup lang="ts">
const items = defineProps<{ items: Item[] }>()
interface Item { id: number }
</script>

<style scoped>
.active { color: red; }
</style>
"#;

    #[test]
    fn a_vue_component_colours_its_script_style_and_template_expressions() {
        let k = |line, needle| kind_at(Language::Vue, SFC, line, needle);
        // <script setup lang="ts">: TypeScript.
        assert_eq!(k(7, "const"), Some(HighlightKind::Keyword));
        assert_eq!(k(8, "interface"), Some(HighlightKind::Keyword));
        assert_eq!(k(8, "number"), Some(HighlightKind::Type));
        // <style>: CSS.
        assert_eq!(k(12, "color"), Some(HighlightKind::Property));
        // Directives, and their values as code.
        assert_eq!(k(1, "v-for"), Some(HighlightKind::Keyword));
        assert_eq!(k(1, "in items").unwrap(), HighlightKind::Keyword);
        assert_eq!(k(1, "select"), Some(HighlightKind::Function));
        assert_eq!(k(1, "id\""), Some(HighlightKind::Property));
        // A bound class is an expression, not Tailwind classes.
        let src = "<template>\n<li :class=\"{ active: on }\">x</li>\n</template>\n";
        assert_eq!(
            kind_at(Language::Vue, src, 1, "active"),
            Some(HighlightKind::Property)
        );
        // {{ }}: braces and the expression inside.
        assert_eq!(k(2, "{{"), Some(HighlightKind::Operator));
        assert_eq!(k(2, "toUpperCase"), Some(HighlightKind::Function));
        // The markup itself is still HTML.
        assert_eq!(k(1, "li"), Some(HighlightKind::Tag));
    }

    #[test]
    fn script_blocks_in_html_and_blade_are_javascript() {
        let src = "<div>{{ $x }}</div>\n<script>\nconst total = sum(1, 2);\n</script>\n<script type=\"text/template\">\nconst no = 1;\n</script>\n";
        assert_eq!(
            kind_at(Language::Blade, src, 2, "const"),
            Some(HighlightKind::Keyword)
        );
        assert_eq!(
            kind_at(Language::Html, src, 2, "sum"),
            Some(HighlightKind::Function)
        );
        // A template isn't script.
        assert_eq!(kind_at(Language::Html, src, 5, "const"), None);
        // Outside Vue, `{{ }}` is Blade's (or text), not JavaScript.
        assert_eq!(kind_at(Language::Html, src, 0, "{{"), None);
        assert_eq!(
            kind_at(Language::Blade, src, 0, "{{"),
            Some(HighlightKind::Operator)
        );
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
