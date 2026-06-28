//! A pragmatic Emmet abbreviation expander for the HTML family (HTML, Blade,
//! Vue, Svelte, PHP, XML). Covers the abbreviations people actually type:
//! tags, `.class`, `#id`, `[attr=val]`, `{text}`, child `>`, sibling `+`,
//! grouping `()`, multiplication `*N` and `$` numbering.
//!
//! `expand` returns the markup with a single `\0` marking where the caret should
//! land; the caller removes it and positions the cursor.

const CURSOR: char = '\0';

/// HTML void (self-closing) elements.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// A small set of well-known tags so we can tell an abbreviation (`section`)
/// from a normal word (`the`) when deciding whether Tab should expand.
const KNOWN_TAGS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "article",
    "aside",
    "audio",
    "b",
    "blockquote",
    "body",
    "button",
    "canvas",
    "caption",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "label",
    "legend",
    "li",
    "link",
    "main",
    "map",
    "mark",
    "menu",
    "meta",
    "nav",
    "ol",
    "option",
    "p",
    "picture",
    "pre",
    "progress",
    "q",
    "section",
    "select",
    "small",
    "source",
    "span",
    "strong",
    "style",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "u",
    "ul",
    "video",
];

#[derive(Clone, Default)]
struct Node {
    tag: String,
    is_group: bool,
    id: String,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
    text: String,
    multiply: usize,
    children: Vec<Node>,
}

struct Parser {
    c: Vec<char>,
    i: usize,
}

impl Parser {
    fn new(s: &str) -> Self {
        Parser {
            c: s.chars().collect(),
            i: 0,
        }
    }
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }
    fn bump(&mut self) -> Option<char> {
        let x = self.peek();
        if x.is_some() {
            self.i += 1;
        }
        x
    }

    fn siblings(&mut self) -> Vec<Node> {
        let mut out = vec![self.term()];
        while self.peek() == Some('+') {
            self.bump();
            out.push(self.term());
        }
        out
    }

    fn term(&mut self) -> Node {
        let mut node = self.atom();
        if self.peek() == Some('*') {
            self.bump();
            node.multiply = self.read_number().unwrap_or(1);
        }
        if self.peek() == Some('>') {
            self.bump();
            node.children = self.siblings();
        }
        node
    }

    fn atom(&mut self) -> Node {
        if self.peek() == Some('(') {
            self.bump();
            let inner = self.siblings();
            if self.peek() == Some(')') {
                self.bump();
            }
            return Node {
                is_group: true,
                children: inner,
                ..Default::default()
            };
        }
        self.element()
    }

    fn element(&mut self) -> Node {
        let mut n = Node {
            tag: self.read_ident(),
            ..Default::default()
        };
        loop {
            match self.peek() {
                Some('.') => {
                    self.bump();
                    let name = self.read_name();
                    if !name.is_empty() {
                        n.classes.push(name);
                    }
                }
                Some('#') => {
                    self.bump();
                    n.id = self.read_name();
                }
                Some('[') => {
                    self.bump();
                    n.attrs.extend(self.read_attrs());
                }
                Some('{') => {
                    self.bump();
                    n.text = self.read_text();
                }
                _ => break,
            }
        }
        n
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '-' || c == ':' {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        s
    }

    fn read_name(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '$' {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        s
    }

    fn read_number(&mut self) -> Option<usize> {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        s.parse().ok()
    }

    /// Parse `name=value name2="value 2" boolean` until `]`.
    fn read_attrs(&mut self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        loop {
            // skip spaces
            while self.peek() == Some(' ') {
                self.bump();
            }
            match self.peek() {
                Some(']') | None => {
                    self.bump();
                    break;
                }
                _ => {}
            }
            let mut name = String::new();
            while let Some(c) = self.peek() {
                if c == '=' || c == ' ' || c == ']' {
                    break;
                }
                name.push(c);
                self.bump();
            }
            let mut value = String::new();
            if self.peek() == Some('=') {
                self.bump();
                match self.peek() {
                    Some(q @ '"') | Some(q @ '\'') => {
                        self.bump();
                        while let Some(c) = self.peek() {
                            self.bump();
                            if c == q {
                                break;
                            }
                            value.push(c);
                        }
                    }
                    _ => {
                        while let Some(c) = self.peek() {
                            if c == ' ' || c == ']' {
                                break;
                            }
                            value.push(c);
                            self.bump();
                        }
                    }
                }
            }
            if !name.is_empty() {
                out.push((name, value));
            }
        }
        out
    }

    fn read_text(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            self.bump();
            if c == '}' {
                break;
            }
            s.push(c);
        }
        s
    }
}

/// Replace `$` runs with the (1-based) index, zero-padded to the run length.
fn number(s: &str, idx: usize) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let mut run = 0;
            while i < chars.len() && chars[i] == '$' {
                run += 1;
                i += 1;
            }
            out.push_str(&format!("{idx:0width$}", width = run));
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn default_attrs(tag: &str) -> Vec<(String, String)> {
    match tag {
        "a" => vec![("href".into(), String::new())],
        "img" => vec![("src".into(), String::new()), ("alt".into(), String::new())],
        "input" => vec![("type".into(), "text".into())],
        "link" => vec![
            ("rel".into(), "stylesheet".into()),
            ("href".into(), String::new()),
        ],
        "script" => vec![("src".into(), String::new())],
        "form" => vec![("action".into(), String::new())],
        _ => Vec::new(),
    }
}

/// Pick an implicit tag for an empty tag name, based on the parent.
fn implicit_tag(parent: &str) -> &'static str {
    match parent {
        "ul" | "ol" => "li",
        "table" | "tbody" | "thead" | "tfoot" => "tr",
        "tr" => "td",
        "select" | "optgroup" => "option",
        "dl" => "dt",
        _ => "div",
    }
}

#[allow(clippy::too_many_arguments)]
fn render(
    nodes: &[Node],
    depth: usize,
    unit: &str,
    parent: &str,
    idx: usize,
    out: &mut String,
    placed: &mut bool,
    first: &mut bool,
) {
    for node in nodes {
        let times = node.multiply.max(1);
        for k in 1..=times {
            // A multiplied node's own index drives `$`; otherwise inherit.
            let n = if times > 1 { k } else { idx };
            if node.is_group {
                render(&node.children, depth, unit, parent, n, out, placed, first);
                continue;
            }
            if !*first {
                out.push('\n');
            }
            *first = false;
            render_element(node, depth, unit, parent, n, out, placed);
        }
    }
}

fn render_element(
    node: &Node,
    depth: usize,
    unit: &str,
    parent: &str,
    idx: usize,
    out: &mut String,
    placed: &mut bool,
) {
    let pad = unit.repeat(depth);
    let tag = if node.tag.is_empty() {
        implicit_tag(parent).to_string()
    } else {
        number(&node.tag, idx)
    };
    let void = VOID.contains(&tag.as_str());

    out.push_str(&pad);
    out.push('<');
    out.push_str(&tag);

    if !node.id.is_empty() {
        out.push_str(&format!(" id=\"{}\"", number(&node.id, idx)));
    }
    if !node.classes.is_empty() {
        let cls: Vec<String> = node.classes.iter().map(|c| number(c, idx)).collect();
        out.push_str(&format!(" class=\"{}\"", cls.join(" ")));
    }
    // User attributes, then defaults that weren't overridden. For void elements,
    // the caret lands in the first empty attribute value.
    let mut seen: Vec<String> = node.attrs.iter().map(|(k, _)| k.clone()).collect();
    let write_attr = |out: &mut String, placed: &mut bool, k: &str, v: &str| {
        if v.is_empty() && void && !*placed {
            out.push_str(&format!(" {k}=\"{CURSOR}\""));
            *placed = true;
        } else {
            out.push_str(&format!(" {k}=\"{v}\""));
        }
    };
    for (k, v) in &node.attrs {
        write_attr(out, placed, k, &number(v, idx));
    }
    for (k, v) in default_attrs(&tag) {
        if !seen.iter().any(|s| s == &k) {
            write_attr(out, placed, &k, &v);
            seen.push(k);
        }
    }

    if void {
        out.push_str(" />");
        return;
    }
    out.push('>');

    if !node.children.is_empty() {
        out.push('\n');
        let mut inner_first = true;
        render(
            &node.children,
            depth + 1,
            unit,
            &tag,
            idx,
            out,
            placed,
            &mut inner_first,
        );
        out.push('\n');
        out.push_str(&pad);
    } else if !node.text.is_empty() {
        out.push_str(&number(&node.text, idx));
        if !*placed {
            out.push(CURSOR);
            *placed = true;
        }
    } else if !*placed {
        out.push(CURSOR);
        *placed = true;
    }

    out.push_str(&format!("</{tag}>"));
}

/// Whether `abbr` should be treated as an Emmet abbreviation (so Tab expands it)
/// rather than a normal word.
pub fn is_expandable(abbr: &str) -> bool {
    if abbr.is_empty() {
        return false;
    }
    if abbr.contains(['.', '#', '>', '+', '*', '[', '{', '(']) {
        return true;
    }
    // A bare word only expands if it's a known tag.
    KNOWN_TAGS.contains(&abbr)
}

/// Expand an Emmet abbreviation. The result contains a single `\0` at the caret.
pub fn expand(abbr: &str, unit: &str) -> Option<String> {
    let abbr = abbr.trim();
    if abbr.is_empty() {
        return None;
    }
    let mut parser = Parser::new(abbr);
    let nodes = parser.siblings();
    // Reject if we didn't consume the whole thing (malformed).
    if parser.i != parser.c.len() {
        return None;
    }
    if nodes.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut placed = false;
    let mut first = true;
    render(&nodes, 0, unit, "", 1, &mut out, &mut placed, &mut first);
    if !placed {
        out.push(CURSOR);
    }
    Some(out)
}

/// Find the Emmet abbreviation immediately before the cursor in `line_before`.
/// Returns the byte index where it starts and the abbreviation itself.
pub fn abbreviation_at(line_before: &str) -> Option<(usize, String)> {
    let chars: Vec<(usize, char)> = line_before.char_indices().collect();
    let mut i = chars.len();
    let mut brace = 0i32;
    let mut bracket = 0i32;
    while i > 0 {
        let c = chars[i - 1].1;
        match c {
            '}' => brace += 1,
            '{' => {
                brace -= 1;
                if brace < 0 {
                    break;
                }
            }
            ']' => bracket += 1,
            '[' => {
                bracket -= 1;
                if bracket < 0 {
                    break;
                }
            }
            c if c.is_whitespace() && brace == 0 && bracket == 0 => break,
            '<' | '>' if brace == 0 && bracket == 0 => {
                // Allow '>' (child operator) but not a literal tag bracket pair.
                if c == '<' {
                    break;
                }
            }
            _ => {}
        }
        i -= 1;
    }
    let start = chars.get(i).map(|(b, _)| *b).unwrap_or(line_before.len());
    let abbr = &line_before[start..];
    if abbr.is_empty() {
        return None;
    }
    Some((start, abbr.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(abbr: &str) -> String {
        expand(abbr, "  ").unwrap().replace('\0', "|")
    }

    #[test]
    fn basic() {
        assert_eq!(ex("div"), "<div>|</div>");
        assert_eq!(ex(".foo"), "<div class=\"foo\">|</div>");
        assert_eq!(ex("#bar"), "<div id=\"bar\">|</div>");
        assert_eq!(ex("p.a.b"), "<p class=\"a b\">|</p>");
        assert_eq!(ex("a"), "<a href=\"\">|</a>");
        assert_eq!(ex("img"), "<img src=\"|\" alt=\"\" />");
        assert_eq!(ex("p{Hi}"), "<p>Hi|</p>");
    }

    #[test]
    fn attrs() {
        assert_eq!(
            ex("input[type=email name=q]"),
            "<input type=\"email\" name=\"q\" />|"
        );
    }

    #[test]
    fn nesting_and_mul() {
        assert_eq!(ex("ul>li*2"), "<ul>\n  <li>|</li>\n  <li></li>\n</ul>");
        assert_eq!(
            ex("li.item$*2"),
            "<li class=\"item1\">|</li>\n<li class=\"item2\"></li>"
        );
    }

    #[test]
    fn siblings_and_groups() {
        assert_eq!(ex("h1+p"), "<h1>|</h1>\n<p></p>");
        assert_eq!(ex("(span)*2"), "<span>|</span>\n<span></span>");
    }

    #[test]
    fn detects_abbreviation() {
        assert_eq!(abbreviation_at("    ul>li").unwrap().1, "ul>li");
        assert_eq!(abbreviation_at("foo .a.b").unwrap().1, ".a.b");
        assert_eq!(abbreviation_at("p{a b c}").unwrap().1, "p{a b c}");
        assert!(!is_expandable("the"));
        assert!(is_expandable("div"));
        assert!(is_expandable(".foo"));
    }
}
