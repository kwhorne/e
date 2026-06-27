//! Completion + hover popups (driven by the LSP layer).

use floem::kurbo::Point;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;
use lsp_types::{CompletionItem, CompletionItemKind};

use crate::state::AppState;
use crate::theme;

const MAX_ROWS: usize = 50;

/// Reactive state for the completion popup.
#[derive(Clone, Copy)]
pub struct Completion {
    pub open: RwSignal<bool>,
    pub items: RwSignal<Vec<CompletionItem>>,
    pub selected: RwSignal<usize>,
    pub anchor: RwSignal<Point>,
    pub buffer_id: RwSignal<Option<u64>>,
    /// Byte offset where the replaced word starts.
    pub start_offset: RwSignal<usize>,
}

impl Completion {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            items: RwSignal::new(Vec::new()),
            selected: RwSignal::new(0),
            anchor: RwSignal::new(Point::ZERO),
            buffer_id: RwSignal::new(None),
            start_offset: RwSignal::new(0),
        }
    }
}

/// Reactive state for the signature-help popup.
#[derive(Clone, Copy)]
pub struct SignatureState {
    pub open: RwSignal<bool>,
    pub label: RwSignal<String>,
    /// Char range of the active parameter within `label`.
    pub active: RwSignal<Option<(usize, usize)>>,
    pub anchor: RwSignal<Point>,
}

impl SignatureState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            label: RwSignal::new(String::new()),
            active: RwSignal::new(None),
            anchor: RwSignal::new(Point::ZERO),
        }
    }
}

/// Reactive state for the hover popup.
#[derive(Clone, Copy)]
pub struct HoverState {
    pub open: RwSignal<bool>,
    pub text: RwSignal<String>,
    pub anchor: RwSignal<Point>,
}

impl HoverState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            text: RwSignal::new(String::new()),
            anchor: RwSignal::new(Point::ZERO),
        }
    }
}

fn kind_icon(kind: Option<CompletionItemKind>) -> (&'static str, Color) {
    match kind {
        Some(CompletionItemKind::FUNCTION) | Some(CompletionItemKind::METHOD) => {
            ("ƒ", Color::from_rgb8(0x61, 0xaf, 0xef))
        }
        Some(CompletionItemKind::VARIABLE) | Some(CompletionItemKind::FIELD) => {
            ("$", Color::from_rgb8(0xe0, 0x6c, 0x75))
        }
        Some(CompletionItemKind::CLASS)
        | Some(CompletionItemKind::INTERFACE)
        | Some(CompletionItemKind::STRUCT)
        | Some(CompletionItemKind::ENUM) => ("C", Color::from_rgb8(0xe5, 0xc0, 0x7b)),
        Some(CompletionItemKind::CONSTANT) | Some(CompletionItemKind::ENUM_MEMBER) => {
            ("π", Color::from_rgb8(0xd1, 0x9a, 0x66))
        }
        Some(CompletionItemKind::KEYWORD) => ("k", Color::from_rgb8(0xc6, 0x78, 0xdd)),
        Some(CompletionItemKind::SNIPPET) => ("▢", Color::from_rgb8(0x98, 0xc3, 0x79)),
        _ => ("•", theme::fg_dim()),
    }
}

pub fn completion_popup(state: AppState) -> impl IntoView {
    let comp = state.completion;

    let rows = dyn_stack(
        move || {
            comp.items
                .get()
                .into_iter()
                .take(MAX_ROWS)
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(i, item)| {
            let (icon, color) = kind_icon(item.kind);
            let lbl = item.label.clone();
            let detail = item.detail.clone().unwrap_or_default();
            stack((
                label(move || icon.to_string()).style(move |s| s.width(16.0).color(color)),
                label(move || lbl.clone())
                    .style(|s| s.color(theme::fg()).flex_grow(1.0).text_ellipsis()),
                label(move || detail.clone())
                    .style(|s| s.color(theme::fg_dim()).text_ellipsis().max_width(140.0)),
            ))
            .style(move |s| {
                let s = s
                    .items_center()
                    .gap(8.0)
                    .height(22.0)
                    .width_full()
                    .padding_horiz(8.0);
                if comp.selected.get() == i {
                    s.background(theme::bg_active())
                } else {
                    s.hover(|s| s.background(theme::bg_hover()))
                }
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    scroll(rows).style(move |s| {
        let anchor = comp.anchor.get();
        let s = s
            .absolute()
            .inset_left(anchor.x)
            .inset_top(anchor.y + 4.0)
            .width(420.0)
            .max_height(240.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0);
        if comp.open.get() && !comp.items.get().is_empty() {
            s
        } else {
            s.hide()
        }
    })
}

pub fn signature_popup(state: AppState) -> impl IntoView {
    let sig = state.signature;

    let before = label(move || {
        let l = sig.label.get();
        match sig.active.get() {
            Some((s, _)) => l.chars().take(s).collect(),
            None => l,
        }
    })
    .style(|s| s.color(theme::fg_dim()));

    let active = label(move || {
        let l = sig.label.get();
        match sig.active.get() {
            Some((s, e)) => l.chars().skip(s).take(e.saturating_sub(s)).collect(),
            None => String::new(),
        }
    })
    .style(|s| s.color(theme::accent()));

    let after = label(move || {
        let l = sig.label.get();
        match sig.active.get() {
            Some((_, e)) => l.chars().skip(e).collect(),
            None => String::new(),
        }
    })
    .style(|s| s.color(theme::fg_dim()));

    stack((before, active, after)).style(move |s| {
        let anchor = sig.anchor.get();
        let s = s
            .absolute()
            .inset_left(anchor.x)
            .inset_top(anchor.y)
            .items_center()
            .padding_horiz(8.0)
            .height(24.0)
            .font_family("monospace".to_string())
            .font_size(13.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0);
        if sig.open.get() && !sig.label.get().is_empty() {
            s
        } else {
            s.hide()
        }
    })
}

pub fn hover_popup(state: AppState) -> impl IntoView {
    let hover = state.hover;

    label(move || hover.text.get())
        .style(move |s| {
            let anchor = hover.anchor.get();
            let s = s
                .absolute()
                .inset_left(anchor.x)
                .inset_top(anchor.y + 4.0)
                .max_width(520.0)
                .padding(8.0)
                .background(theme::bg_panel())
                .color(theme::fg())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(6.0);
            if hover.open.get() && !hover.text.get().is_empty() {
                s
            } else {
                s.hide()
            }
        })
        .on_event_stop(floem::event::EventListener::PointerDown, |_| {})
}
