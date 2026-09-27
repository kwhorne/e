//! Markdown preview (⌘⇧M): live, beside the editor, following its scroll.
//!
//! Blocks come from `e_core::markdown`; this draws them with native views —
//! selectable rich text, code blocks with Copy, tables, task boxes, local
//! images — and makes links clickable: a URL opens in the browser, a relative
//! `.md` in the editor. `markdown_body_selectable` renders Markdown anywhere
//! else (agent replies, panels).

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::text::{
    Attrs, AttrsList, FamilyOwned, LineHeightValue, Style as TextStyle, TextLayout, Weight,
};
use floem::unit::Pct;
use floem::views::{
    dyn_container, dyn_stack, empty, img, label, rich_text, scroll, stack, stack_from_iter,
    Decorators,
};
use floem::{AnyView, IntoView};

use e_core::language::Language;
use e_core::markdown::{self, heading_size, Block, Span};

use crate::state::AppState;
use crate::theme;

/// Where relative links and images resolve, and what a click on a link does.
#[derive(Clone)]
struct Ctx {
    base: Option<PathBuf>,
    on_link: Rc<dyn Fn(&str)>,
}

fn code_color() -> Color {
    theme::number()
}

fn layout(spans: &[Span], size: f32, base_bold: bool) -> TextLayout {
    let sans: Vec<FamilyOwned> = FamilyOwned::parse_list("sans-serif").collect();
    let mono: Vec<FamilyOwned> = FamilyOwned::parse_list(&theme::mono_family()).collect();

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
        let color = if sp.link {
            theme::accent()
        } else if sp.strike {
            theme::fg_dim()
        } else {
            theme::fg()
        };
        if sp.code {
            a = a.family(&mono).color(code_color());
        } else {
            a = a.family(&sans).color(color);
        }
        if sp.bold || base_bold {
            a = a.weight(Weight::BOLD);
        }
        if sp.italic || sp.strike {
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

/// The link under byte `offset` of the laid-out text, if any.
fn href_at(spans: &[Span], offset: usize) -> Option<String> {
    let mut pos = 0;
    for sp in spans {
        let end = pos + sp.text.len();
        if offset >= pos && offset < end {
            return sp.href.clone();
        }
        pos = end;
    }
    None
}

/// Selectable rich text for one block; a click on a link follows it.
fn inline(spans: Vec<Span>, size: f32, bold: bool, ctx: &Ctx) -> floem::views::RichText {
    let for_layout = spans.clone();
    let on_link = ctx.on_link.clone();
    rich_text(move || layout(&for_layout, size, bold))
        .selectable()
        .on_click_offset(move |off| {
            if let Some(href) = href_at(&spans, off) {
                on_link(&href);
            }
        })
}

fn block_view(block: Block, ctx: Ctx) -> AnyView {
    match block {
        Block::Heading(level, spans) => inline(spans, heading_size(level), true, &ctx)
            .style(|s| s.width_full().padding_top(8.0))
            .into_any(),
        Block::Paragraph(spans) => inline(spans, 14.0, false, &ctx)
            .style(|s| s.width_full())
            .into_any(),
        Block::Quote(spans) => inline(spans, 14.0, false, &ctx)
            .style(|s| {
                s.width_full()
                    .padding_left(14.0)
                    .border_left(3.0)
                    .border_color(theme::border())
                    .color(theme::fg_dim())
            })
            .into_any(),
        Block::ListItem {
            depth,
            number,
            task,
            mut spans,
        } => {
            let marker = match (task, number) {
                (Some(true), _) => "☑  ".to_string(),
                (Some(false), _) => "☐  ".to_string(),
                (None, Some(n)) => format!("{n}.  "),
                (None, None) => "•  ".to_string(),
            };
            spans.insert(
                0,
                Span {
                    text: marker,
                    bold: false,
                    italic: false,
                    code: false,
                    link: false,
                    strike: false,
                    href: None,
                },
            );
            let indent = depth.saturating_sub(1) as f64 * 18.0;
            inline(spans, 14.0, false, &ctx)
                .style(move |s| s.width_full().padding_left(indent))
                .into_any()
        }
        Block::Code(code) => code_block(code),
        Block::Rule => empty()
            .style(|s| {
                s.width_full()
                    .height(1.0)
                    .margin_top(8.0)
                    .background(theme::border())
            })
            .into_any(),
        Block::Table { header, rows } => table(header, rows, &ctx),
        Block::Image { src, alt } => image(&src, &alt, &ctx),
    }
}

/// A fenced code block in the code font, with a Copy button.
fn code_block(code: String) -> AnyView {
    let body = code.clone();
    let code_text = rich_text(move || {
        let mono: Vec<FamilyOwned> = FamilyOwned::parse_list(&theme::mono_family()).collect();
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

/// A table: the header row bold on the panel colour, cells sharing the width.
fn table(header: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>>, ctx: &Ctx) -> AnyView {
    let row_view = |cells: Vec<Vec<Span>>, bold: bool, ctx: &Ctx| -> AnyView {
        let cells: Vec<AnyView> = cells
            .into_iter()
            .map(|spans| {
                inline(spans, 13.0, bold, ctx)
                    .style(|s| {
                        s.flex_basis(0.0)
                            .flex_grow(1.0_f32)
                            .min_width(0.0)
                            .padding_horiz(10.0)
                            .padding_vert(6.0)
                    })
                    .into_any()
            })
            .collect();
        stack_from_iter(cells)
            .style(move |s| {
                let s = s
                    .flex_row()
                    .width_full()
                    .border_bottom(1.0)
                    .border_color(theme::border());
                if bold {
                    s.background(theme::bg_panel())
                } else {
                    s
                }
            })
            .into_any()
    };
    let mut all: Vec<AnyView> = vec![row_view(header, true, ctx)];
    for row in rows {
        all.push(row_view(row, false, ctx));
    }
    stack_from_iter(all)
        .style(|s| {
            s.flex_col()
                .width_full()
                .border(1.0)
                .border_color(theme::border())
                .border_radius(6.0)
        })
        .into_any()
}

/// A local image, scaled to the column; a remote one or a missing file shows
/// its alt text and where it was meant to come from.
fn image(src: &str, alt: &str, ctx: &Ctx) -> AnyView {
    let placeholder = |why: &str| -> AnyView {
        let text = if alt.is_empty() {
            format!("🖼 {src}  ({why})")
        } else {
            format!("🖼 {alt}  —  {src}  ({why})")
        };
        label(move || text.clone())
            .style(|s| {
                s.font_size(12.0)
                    .color(theme::fg_dim())
                    .padding(10.0)
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(6.0)
            })
            .into_any()
    };
    if src.starts_with("http://") || src.starts_with("https://") {
        return placeholder("remote image");
    }
    let path = if Path::new(src).is_absolute() {
        PathBuf::from(src)
    } else {
        match &ctx.base {
            Some(base) => base.join(src),
            None => return placeholder("no base folder"),
        }
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return placeholder("not found");
    };
    let caption = alt.to_string();
    let has_caption = !caption.is_empty();
    let picture = img(move || bytes.clone()).style(|s| s.max_width(Pct(100.0)));
    let cap = label(move || caption.clone()).style(move |s| {
        let s = s.font_size(11.5).color(theme::fg_dim()).margin_top(4.0);
        if has_caption {
            s
        } else {
            s.hide()
        }
    });
    stack((picture, cap))
        .style(|s| s.flex_col().width_full().items_start())
        .into_any()
}

/// Render an arbitrary Markdown string as a *selectable* column of rich block
/// views, so the reader can drag-select and copy (`⌘C`) any text while keeping
/// inline formatting. Links open in the browser.
pub fn markdown_body_selectable(text: &str) -> impl IntoView {
    let blocks = markdown::parse(text);
    let ctx = Ctx {
        base: None,
        on_link: Rc::new(open_external_link),
    };
    dyn_stack(
        move || blocks.clone().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(_, block)| block_view(block, ctx.clone()),
    )
    .style(|s| s.flex_col().width_full().gap(8.0))
}

/// A URL goes to the browser (or mail client); anything else is left alone.
fn open_external_link(href: &str) {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:") {
        let _ = std::process::Command::new("open").arg(href).spawn();
    }
}

fn is_markdown(state: AppState) -> bool {
    state
        .active_buffer()
        .map(|b| b.file.language == Language::Markdown)
        .unwrap_or(false)
}

/// How far down the active editor is scrolled, 0..=1, so the preview can
/// follow.
fn editor_scroll_fraction(state: AppState) -> f64 {
    let Some(buf) = state.active_buffer() else {
        return 0.0;
    };
    let Some(ed) = buf.editor.get() else {
        return 0.0;
    };
    let vp = ed.viewport.get();
    let line_h = (ed.line_height(0) as f64).max(1.0);
    let total = (ed.last_vline().get() + 1) as f64 * line_h;
    let max = (total - vp.height()).max(1.0);
    (vp.y0 / max).clamp(0.0, 1.0)
}

impl AppState {
    /// A link clicked in the preview: a URL opens outside, a relative file (a
    /// sibling `.md`, say) opens in the editor.
    pub fn follow_markdown_link(&self, href: &str, base: Option<&Path>) {
        if href.starts_with("http://")
            || href.starts_with("https://")
            || href.starts_with("mailto:")
        {
            open_external_link(href);
            return;
        }
        let target = href.split('#').next().unwrap_or("");
        if target.is_empty() {
            return; // an in-page anchor
        }
        let path = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            match base {
                Some(b) => b.join(target),
                None => return,
            }
        };
        if path.is_file() {
            self.open_path(path);
        } else if path.is_dir() {
            self.open_project(path);
        } else {
            Self::notify(&format!("{} doesn't exist", path.display()));
        }
    }
}

/// The preview pane: beside the editor when `⌘⇧M` is on and the active file
/// is Markdown, re-rendered as the text changes, scrolled with the editor.
pub fn markdown_preview(state: AppState) -> impl IntoView {
    let visible = move || state.md_preview.get() && is_markdown(state);
    dyn_container(
        move || {
            let on = visible();
            let (rev, id) = state
                .active_buffer()
                .map(|b| {
                    use floem::views::editor::text::Document;
                    (b.doc.cache_rev().get(), b.id)
                })
                .unwrap_or((0, 0));
            (on, rev, id)
        },
        move |(on, _rev, _id)| {
            if !on {
                return empty().into_any();
            }
            let Some(buf) = state.active_buffer() else {
                return empty().into_any();
            };
            let text = {
                use floem::views::editor::text::Document;
                buf.doc.text().to_string()
            };
            let base: Option<PathBuf> = buf
                .file
                .path
                .as_ref()
                .and_then(|p| p.parent().map(Path::to_path_buf));
            let link_base = base.clone();
            let ctx = Ctx {
                base,
                on_link: Rc::new(move |href| {
                    state.follow_markdown_link(href, link_base.as_deref())
                }),
            };
            let blocks = markdown::parse(&text);

            let list = dyn_stack(
                move || blocks.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(_, block)| block_view(block, ctx.clone()),
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
                .scroll_to_percent(move || editor_scroll_fraction(state) as f32)
                .into_any()
        },
    )
    .style(move |s| {
        let s = s
            .height_full()
            .width(Pct(50.0))
            .min_width(0.0)
            .flex_shrink(0.0_f32)
            .border_left(1.0)
            .border_color(theme::border())
            .background(theme::bg());
        if visible() {
            s
        } else {
            s.hide()
        }
    })
}
