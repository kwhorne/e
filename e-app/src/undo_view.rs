//! Visual undo-tree panel: a branching history you can jump around in,
//! including branches that a linear undo would have thrown away.

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{dyn_stack, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

struct Row {
    id: usize,
    depth: usize,
    current: bool,
    label: String,
}

fn rel_time(now: u64, ts: u64) -> String {
    if ts == 0 {
        return "base".to_string();
    }
    let secs = now.saturating_sub(ts) / 1000;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

fn rows(state: AppState) -> Vec<Row> {
    let Some(buf) = state.active_buffer() else {
        return Vec::new();
    };
    let t = buf.undo.borrow();
    let now = crate::state::now_ms_epoch();
    let mut out = Vec::new();
    // Pre-order DFS; children pushed in reverse so the oldest renders first.
    let mut stack = vec![(0usize, 0usize)];
    while let Some((id, depth)) = stack.pop() {
        let node = &t.nodes[id];
        let delta = match node.parent {
            Some(p) => {
                let d = node.text.len() as i64 - t.nodes[p].text.len() as i64;
                if d >= 0 {
                    format!("+{d}")
                } else {
                    format!("{d}")
                }
            }
            None => String::new(),
        };
        let branch = if node.children.len() > 1 { " ⑂" } else { "" };
        let label = format!("{}  {}{}", rel_time(now, node.ts), delta, branch);
        out.push(Row {
            id,
            depth,
            current: id == t.current,
            label,
        });
        for &c in node.children.iter().rev() {
            stack.push((c, depth + 1));
        }
    }
    out
}

fn tool(text: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    label(move || text.to_string())
        .style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .font_size(11.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| on())
}

pub fn undo_tree_panel(state: AppState) -> impl IntoView {
    let title = label(|| "Undo Tree".to_string()).style(|s| {
        s.flex_grow(1.0)
            .font_size(13.0)
            .font_bold()
            .color(theme::fg())
    });
    let undo = tool("↺ Undo", move || state.undo_tree_undo());
    let redo = tool("↻ Redo", move || state.undo_tree_redo());
    let close = label(|| "✕".to_string())
        .style(|s| {
            s.padding_horiz(8.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.color(theme::fg()))
        })
        .on_click_stop(move |_| state.undo_open.set(false));
    let header = stack((title, undo, redo, close)).style(|s| {
        s.flex_row()
            .items_center()
            .gap(8.0)
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .width_full()
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    let list = dyn_stack(
        move || {
            let _ = state.undo_rev.get();
            let _ = state.active.get();
            rows(state)
                .into_iter()
                .map(|r| (r.id, r))
                .collect::<Vec<_>>()
        },
        |(id, _)| *id,
        move |(_, r)| {
            let indent = 8.0 + r.depth as f64 * 16.0;
            let dot = if r.current { "●" } else { "○" };
            let cur = r.current;
            let id = r.id;
            let text = format!("{dot}  {}", r.label);
            label(move || text.clone())
                .style(move |s| {
                    let s = s
                        .padding_left(indent)
                        .padding_right(12.0)
                        .padding_vert(3.0)
                        .width_full()
                        .font_size(12.0)
                        .font_family("monospace".to_string())
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(theme::bg_hover()));
                    if cur {
                        s.color(Color::from_rgb8(0x61, 0xaf, 0xef)).font_bold()
                    } else {
                        s.color(theme::fg())
                    }
                })
                .on_click_stop(move |_| state.undo_tree_goto(id))
        },
    )
    .style(|s| s.flex_col().width_full());

    let card = stack((
        header,
        scroll(list).style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| {
        s.flex_col()
            .width(360.0)
            .height(520.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(10.0)
            .background(theme::bg())
    });

    floem::views::container(card).style(move |s| {
        let s = s
            .absolute()
            .inset(0.0)
            .size_full()
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 0xCC));
        if state.undo_open.get() {
            s
        } else {
            s.hide()
        }
    })
}
