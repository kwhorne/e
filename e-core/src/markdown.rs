//! A small Markdown parser that turns text into a flat list of renderable
//! blocks. Used by the GUI's reading-mode preview.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// An inline run of text with style flags.
#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
}

/// A renderable block.
#[derive(Clone, Debug)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    Quote(Vec<Span>),
    ListItem(usize, Vec<Span>),
    Code(String),
    Rule,
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    bold: bool,
    italic: bool,
    link: bool,
    quote: bool,
    list_depth: usize,
    code_block: Option<String>,
    heading: Option<u8>,
    in_item: bool,
}

impl Builder {
    fn push_text(&mut self, text: &str, code: bool) {
        if let Some(cb) = self.code_block.as_mut() {
            cb.push_str(text);
            return;
        }
        if text.is_empty() {
            return;
        }
        self.spans.push(Span {
            text: text.to_string(),
            bold: self.bold,
            italic: self.italic,
            code,
            link: self.link,
        });
    }

    fn take_spans(&mut self) -> Vec<Span> {
        std::mem::take(&mut self.spans)
    }
}

pub fn parse(text: &str) -> Vec<Block> {
    let mut b = Builder::default();

    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => b.heading = Some(level as u8),
            Event::End(TagEnd::Heading(_)) => {
                let spans = b.take_spans();
                let level = b.heading.take().unwrap_or(1);
                b.blocks.push(Block::Heading(level, spans));
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                let spans = b.take_spans();
                if !spans.is_empty() {
                    if b.in_item {
                        b.blocks.push(Block::ListItem(b.list_depth.max(1), spans));
                    } else if b.quote {
                        b.blocks.push(Block::Quote(spans));
                    } else {
                        b.blocks.push(Block::Paragraph(spans));
                    }
                }
            }
            Event::Start(Tag::BlockQuote(_)) => b.quote = true,
            Event::End(TagEnd::BlockQuote(_)) => b.quote = false,
            Event::Start(Tag::List(_)) => b.list_depth += 1,
            Event::End(TagEnd::List(_)) => b.list_depth = b.list_depth.saturating_sub(1),
            Event::Start(Tag::Item) => b.in_item = true,
            Event::End(TagEnd::Item) => {
                // Items without an inner paragraph (tight lists) flush here.
                let spans = b.take_spans();
                if !spans.is_empty() {
                    b.blocks.push(Block::ListItem(b.list_depth.max(1), spans));
                }
                b.in_item = false;
            }
            Event::Start(Tag::CodeBlock(_)) => b.code_block = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = b.code_block.take() {
                    b.blocks.push(Block::Code(code.trim_end().to_string()));
                }
            }
            Event::Start(Tag::Emphasis) => b.italic = true,
            Event::End(TagEnd::Emphasis) => b.italic = false,
            Event::Start(Tag::Strong) => b.bold = true,
            Event::End(TagEnd::Strong) => b.bold = false,
            Event::Start(Tag::Link { .. }) => b.link = true,
            Event::End(TagEnd::Link) => b.link = false,
            Event::Text(t) => b.push_text(&t, false),
            Event::Code(t) => b.push_text(&t, true),
            Event::SoftBreak | Event::HardBreak => b.push_text(" ", false),
            Event::Rule => b.blocks.push(Block::Rule),
            _ => {}
        }
    }

    // Flush any trailing spans.
    if !b.spans.is_empty() {
        let spans = b.take_spans();
        b.blocks.push(Block::Paragraph(spans));
    }

    b.blocks
}

/// Heading level used by the renderer (1..=6).
pub fn heading_size(level: u8) -> f32 {
    match level {
        1 => 24.0,
        2 => 20.0,
        3 => 17.0,
        4 => 15.0,
        5 => 14.0,
        _ => 13.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, Block};

    #[test]
    fn parses_heading_and_inline() {
        let blocks = parse("# Title\n\nHi **b** and `c`.\n");
        assert!(matches!(blocks[0], Block::Heading(1, _)));
        if let Block::Paragraph(spans) = &blocks[1] {
            assert!(spans.iter().any(|s| s.bold && s.text == "b"));
            assert!(spans.iter().any(|s| s.code && s.text == "c"));
        } else {
            panic!("expected paragraph");
        }
    }
}
