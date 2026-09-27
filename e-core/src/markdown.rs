//! A small Markdown parser that turns text into a flat list of renderable
//! blocks. Used by the GUI's preview and by panels that show Markdown.
//!
//! Tables, task lists and strikethrough are on (GitHub's dialect, which is what
//! READMEs are written in); ordered lists carry their numbers, links their
//! targets, and images their source, so the renderer can draw all of it.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// An inline run of text with style flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
    pub strike: bool,
    /// The link's target, for spans inside a link.
    pub href: Option<String>,
}

/// A renderable block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    Quote(Vec<Span>),
    ListItem {
        depth: usize,
        /// The item's number in an ordered list.
        number: Option<u64>,
        /// `Some(checked)` for a task item.
        task: Option<bool>,
        spans: Vec<Span>,
    },
    Code(String),
    Rule,
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Image {
        src: String,
        alt: String,
    },
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    bold: bool,
    italic: bool,
    strike: bool,
    href: Option<String>,
    quote: bool,
    /// One entry per open list: the next number for an ordered list.
    lists: Vec<Option<u64>>,
    code_block: Option<String>,
    heading: Option<u8>,
    /// How many list items are open (an item can hold a nested list).
    items_open: usize,
    item_number: Option<u64>,
    task: Option<bool>,
    /// Alt text collected while inside an image.
    image: Option<(String, String)>,
    in_table: bool,
    in_head: bool,
    header: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
    row: Vec<Vec<Span>>,
}

impl Builder {
    fn push_text(&mut self, text: &str, code: bool) {
        if let Some(cb) = self.code_block.as_mut() {
            cb.push_str(text);
            return;
        }
        if let Some((_, alt)) = self.image.as_mut() {
            alt.push_str(text);
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
            link: self.href.is_some(),
            strike: self.strike,
            href: self.href.clone(),
        });
    }

    fn take_spans(&mut self) -> Vec<Span> {
        std::mem::take(&mut self.spans)
    }

    /// Flush pending inline text as the block it belongs to.
    fn flush_paragraph(&mut self) {
        let spans = self.take_spans();
        if spans.is_empty() {
            return;
        }
        if self.items_open > 0 {
            let depth = self.lists.len().max(1);
            self.blocks.push(Block::ListItem {
                depth,
                number: self.item_number,
                task: self.task,
                spans,
            });
            // A second paragraph in the same item continues it unnumbered.
            self.item_number = None;
            self.task = None;
        } else if self.quote {
            self.blocks.push(Block::Quote(spans));
        } else {
            self.blocks.push(Block::Paragraph(spans));
        }
    }
}

pub fn parse(text: &str) -> Vec<Block> {
    let mut b = Builder::default();
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;

    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => b.heading = Some(level as u8),
            Event::End(TagEnd::Heading(_)) => {
                let spans = b.take_spans();
                let level = b.heading.take().unwrap_or(1);
                b.blocks.push(Block::Heading(level, spans));
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => b.flush_paragraph(),
            Event::Start(Tag::BlockQuote(_)) => b.quote = true,
            Event::End(TagEnd::BlockQuote(_)) => b.quote = false,
            Event::Start(Tag::List(start)) => {
                // A nested list starts inside an item: the item's own text is
                // complete, and must not run into the nested item's.
                if b.items_open > 0 {
                    b.flush_paragraph();
                }
                b.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                b.lists.pop();
            }
            Event::Start(Tag::Item) => {
                b.items_open += 1;
                b.task = None;
                b.item_number = match b.lists.last_mut() {
                    Some(Some(n)) => {
                        let cur = *n;
                        *n += 1;
                        Some(cur)
                    }
                    _ => None,
                };
            }
            Event::TaskListMarker(checked) => b.task = Some(checked),
            Event::End(TagEnd::Item) => {
                // Items without an inner paragraph (tight lists) flush here.
                b.flush_paragraph();
                b.items_open = b.items_open.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(_)) => {
                b.flush_paragraph();
                b.code_block = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = b.code_block.take() {
                    b.blocks.push(Block::Code(code.trim_end().to_string()));
                }
            }
            Event::Start(Tag::Emphasis) => b.italic = true,
            Event::End(TagEnd::Emphasis) => b.italic = false,
            Event::Start(Tag::Strong) => b.bold = true,
            Event::End(TagEnd::Strong) => b.bold = false,
            Event::Start(Tag::Strikethrough) => b.strike = true,
            Event::End(TagEnd::Strikethrough) => b.strike = false,
            Event::Start(Tag::Link { dest_url, .. }) => b.href = Some(dest_url.to_string()),
            Event::End(TagEnd::Link) => b.href = None,
            Event::Start(Tag::Image { dest_url, .. }) => {
                b.image = Some((dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((src, alt)) = b.image.take() {
                    // An image stands on its own line; text around it becomes
                    // its own paragraph.
                    b.flush_paragraph();
                    b.blocks.push(Block::Image { src, alt });
                }
            }
            Event::Start(Tag::Table(_)) => {
                b.in_table = true;
                b.header.clear();
                b.rows.clear();
            }
            Event::End(TagEnd::Table) => {
                b.in_table = false;
                b.blocks.push(Block::Table {
                    header: std::mem::take(&mut b.header),
                    rows: std::mem::take(&mut b.rows),
                });
            }
            Event::Start(Tag::TableHead) => {
                b.in_head = true;
                b.row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                b.in_head = false;
                b.header = std::mem::take(&mut b.row);
            }
            Event::Start(Tag::TableRow) => b.row.clear(),
            Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut b.row);
                b.rows.push(row);
            }
            Event::Start(Tag::TableCell) => {}
            Event::End(TagEnd::TableCell) => {
                let cell = b.take_spans();
                b.row.push(cell);
            }
            Event::Text(t) => b.push_text(&t, false),
            Event::Code(t) => b.push_text(&t, true),
            Event::SoftBreak | Event::HardBreak => b.push_text(" ", false),
            Event::Rule => b.blocks.push(Block::Rule),
            _ => {}
        }
    }

    // Flush any trailing spans.
    b.flush_paragraph();

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
        let blocks = parse("# Title\n\nHi **b** and `c` and ~~gone~~ [e](https://e.dev).\n");
        assert!(matches!(blocks[0], Block::Heading(1, _)));
        if let Block::Paragraph(spans) = &blocks[1] {
            assert!(spans.iter().any(|s| s.bold && s.text == "b"));
            assert!(spans.iter().any(|s| s.code && s.text == "c"));
            assert!(spans.iter().any(|s| s.strike && s.text == "gone"));
            let link = spans.iter().find(|s| s.text == "e").unwrap();
            assert!(link.link);
            assert_eq!(link.href.as_deref(), Some("https://e.dev"));
        } else {
            panic!("expected paragraph");
        }
    }

    #[test]
    fn lists_carry_numbers_tasks_and_depth() {
        let blocks = parse("3. three\n4. four\n   - nested\n\n- [ ] todo\n- [x] done\n");
        let items: Vec<(usize, Option<u64>, Option<bool>, String)> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem {
                    depth,
                    number,
                    task,
                    spans,
                } => Some((*depth, *number, *task, spans[0].text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                (1, Some(3), None, "three".into()),
                (1, Some(4), None, "four".into()),
                (2, None, None, "nested".into()),
                (1, None, Some(false), "todo".into()),
                (1, None, Some(true), "done".into()),
            ]
        );
    }

    #[test]
    fn tables_and_images() {
        let blocks = parse("| a | b |\n|---|---|\n| 1 | **2** |\n\n![logo](img/logo.png)\n");
        match &blocks[0] {
            Block::Table { header, rows } => {
                assert_eq!(header.len(), 2);
                assert_eq!(header[0][0].text, "a");
                assert_eq!(rows.len(), 1);
                assert!(rows[0][1][0].bold);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            blocks[1],
            Block::Image {
                src: "img/logo.png".into(),
                alt: "logo".into()
            }
        );
    }
}
