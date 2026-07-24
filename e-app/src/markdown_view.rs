//! Markdown reading-mode preview (⌘⇧M) for `.md` files.

use std::ops::Range;

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::text::{
    Attrs, AttrsList, FamilyOwned, LineHeightValue, Style as TextStyle, TextLayout, Weight,
};
use floem::views::{dyn_stack, empty, label, rich_text, scroll, stack, Decorators};
use floem::IntoView;

use e_core::language::Language;
use e_core::markdown::{self, heading_size, Block, Span};

use crate::state::AppState;
use crate::theme;

const CODE_COLOR: Color = Color::from_rgb8(0xd1, 0x9a, 0x66);

fn layout(spans: &[Span], size: f32, base_bold: bool) -> TextLayout {
    let sans: Vec<FamilyOwned> = FamilyOwned::parse_list("sans-serif").collect();
    let mono: Vec<FamilyOwned> = FamilyOwned::parse_list("monospace").collect();

    let mut base = Attrs::new()
        .family(&sans)
        .font_size(size)
        .line_height(LineHeightValue::Normal(1.45))
        .color(theme::fg());
    if base_bold {
        base = base.weight(Weight::BOLD);
    }

    let mut text = String::new();
    let mut styled: Vec<(Range<usize>, Attrs)> = Vec::new();
    for sp in spans {
        let start = text.len();
        text.push_str(&sp.text);
        let mut a = Attrs::new()
            .font_size(size)
            .line_height(LineHeightValue::Normal(1.45));
        if sp.code {
            a = a.family(&mono).color(CODE_COLOR);
        } else {
            a = a.family(&sans).color(if sp.link {
                theme::accent()
            } else {
                theme::fg()
            });
        }
        if sp.bold || base_bold {
            a = a.weight(Weight::BOLD);
        }
        if sp.italic {
            a = a.style(TextStyle::Italic);
        }
        styled.push((start..text.len(), a));
    }

    let mut list = AttrsList::new(base);
    for (range, attrs) in styled {
        list.add_span(range, attrs);
    }
    let mut tl = TextLayout::new();
    tl.set_text(&text, list, None);
    tl
}

fn block_view(block: Block) -> impl IntoView {
    match block {
        Block::Heading(level, spans) => {
            let size = heading_size(level);
            rich_text(move || layout(&spans, size, true))
                .selectable()
                .style(|s| s.width_full().padding_top(8.0))
                .into_any()
        }
        Block::Paragraph(spans) => rich_text(move || layout(&spans, 14.0, false))
            .selectable()
            .style(|s| s.width_full())
            .into_any(),
        Block::Quote(spans) => rich_text(move || layout(&spans, 14.0, false))
            .selectable()
            .style(|s| {
                s.width_full()
                    .padding_left(14.0)
                    .border_left(3.0)
                    .border_color(theme::border())
                    .color(theme::fg_dim())
            })
            .into_any(),
        Block::ListItem(depth, mut spans) => {
            spans.insert(
                0,
                Span {
                    text: "•  ".to_string(),
                    bold: false,
                    italic: false,
                    code: false,
                    link: false,
                },
            );
            let indent = depth as f64 * 16.0;
            rich_text(move || layout(&spans, 14.0, false))
                .selectable()
                .style(move |s| s.width_full().padding_left(indent))
                .into_any()
        }
        Block::Code(code) => {
            let body = code.clone();
            let code_text = rich_text(move || {
                let mono: Vec<FamilyOwned> = FamilyOwned::parse_list("monospace").collect();
                let attrs = Attrs::new()
                    .family(&mono)
                    .font_size(13.0)
                    .line_height(LineHeightValue::Normal(1.4))
                    .color(theme::fg());
                let mut tl = TextLayout::new();
                tl.set_text(&body, AttrsList::new(attrs), None);
                tl
            })
            .selectable()
            .style(|s| s.width_full().padding(12.0));

            // Copy-to-clipboard button, top-right of the code block.
            let copied = RwSignal::new(false);
            let copy = label(move || {
                if copied.get() {
                    "Copied".to_string()
                } else {
                    "Copy".to_string()
                }
            })
            .style(|s| {
                s.absolute()
                    .inset_right(8.0)
                    .inset_top(8.0)
                    .padding_horiz(8.0)
                    .padding_vert(2.0)
                    .font_size(11.0)
                    .color(theme::fg_dim())
                    .background(theme::bg())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.color(theme::fg()).background(theme::bg_hover()))
            })
            .on_click_stop(move |_| {
                let _ = floem::Clipboard::set_contents(code.clone());
                copied.set(true);
                floem::action::exec_after(std::time::Duration::from_millis(1200), move |_| {
                    copied.set(false)
                });
            });

            stack((code_text, copy))
                .style(|s| {
                    s.width_full()
                        .background(theme::bg_panel())
                        .border(1.0)
                        .border_color(theme::border())
                        .border_radius(6.0)
                })
                .into_any()
        }
        Block::Rule => empty()
            .style(|s| {
                s.width_full()
                    .height(1.0)
                    .margin_top(8.0)
                    .background(theme::border())
            })
            .into_any(),
    }
}

/// Render an arbitrary markdown string as a *selectable* column of rich block
/// views, so the reader can drag-select and copy (`⌘C`) any text while keeping
/// inline formatting (bold, inline code, colors). Backed by our vendored Floem
/// fork, which added text selection to `rich_text`.
pub fn markdown_body_selectable(text: &str) -> impl IntoView {
    let blocks = markdown::parse(text);
    dyn_stack(
        move || blocks.clone().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(_, block)| block_view(block),
    )
    .style(|s| s.flex_col().width_full().gap(8.0))
}

fn is_markdown(state: AppState) -> bool {
    state
        .active_buffer()
        .map(|b| b.file.language == Language::Markdown)
        .unwrap_or(false)
}

pub fn markdown_preview(state: AppState) -> impl IntoView {
    floem::views::dyn_container(
        move || {
            let visible = state.md_preview.get() && is_markdown(state);
            let rev = state
                .active_buffer()
                .map(|b| {
                    use floem::views::editor::text::Document;
                    b.doc.cache_rev().get()
                })
                .unwrap_or(0);
            (visible, rev)
        },
        move |(visible, _rev)| {
            if !visible {
                return empty().into_any();
            }
            let text = state
                .active_buffer()
                .map(|b| {
                    use floem::views::editor::text::Document;
                    b.doc.text().to_string()
                })
                .unwrap_or_default();
            let blocks = markdown::parse(&text);

            let list = dyn_stack(
                move || blocks.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(_, block)| block_view(block),
            )
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .max_width(820.0)
                    .gap(10.0)
                    .padding(28.0)
            });

            scroll(list)
                .style(|s| s.size_full().background(theme::bg()))
                .into_any()
        },
    )
    .style(move |s| {
        let s = s.absolute().inset(0.0).size_full();
        if state.md_preview.get() && is_markdown(state) {
            s
        } else {
            s.hide()
        }
    })
}
