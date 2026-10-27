//! The native chat transcript and composer input behind Elyra Cascade.
//!
//! Renders the conversation folded from elyra's RPC event stream by the
//! `e-agent` crate with native floem views — streaming assistant text, tool-call
//! cards, a multi-line composer — instead of running the agent's terminal UI in
//! a PTY. This is what makes ⌘L feel responsive: we append only the changed text
//! rather than re-parsing a whole ANSI grid every frame. The panel around it
//! (header, empty state, composer card, footer) lives in [`crate::cascade`].

use std::rc::Rc;

use e_agent::{ChatItem, ToolStatus};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{create_effect, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::keypress::default_key_handler;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::{Document, WrapMethod};
use floem::views::editor::text_document::TextDocument;
use floem::views::{dyn_stack, empty, label, scroll, stack, text_editor_keys, Decorators};
use floem::IntoView;

use crate::app::handle_shortcut;
use crate::state::AppState;
use crate::theme;

/// Total characters currently in the transcript — used to drive auto-scroll so
/// the view follows streaming text (not just newly-added items).
fn transcript_size(state: AppState) -> usize {
    state.agent.chat.with(|c| {
        c.items
            .iter()
            .map(|it| match it {
                ChatItem::User { text }
                | ChatItem::Assistant { text, .. }
                | ChatItem::Reasoning { text, .. }
                | ChatItem::Notice { text, .. } => text.len(),
                ChatItem::Tool(tc) => {
                    tc.summary.len() + tc.result.as_ref().map(String::len).unwrap_or(0)
                }
                ChatItem::Receipt(r) => r.files.len(),
            })
            .sum()
    })
}

/// A reactive getter for the text of item `i`, given its variant is stable.
fn item_text(state: AppState, i: usize) -> String {
    state.agent.chat.with(|c| match c.items.get(i) {
        Some(ChatItem::User { text })
        | Some(ChatItem::Assistant { text, .. })
        | Some(ChatItem::Reasoning { text, .. })
        | Some(ChatItem::Notice { text, .. }) => text.clone(),
        _ => String::new(),
    })
}

fn is_streaming(state: AppState, i: usize) -> bool {
    state.agent.chat.with(|c| {
        matches!(
            c.items.get(i),
            Some(ChatItem::Assistant {
                streaming: true,
                ..
            }) | Some(ChatItem::Reasoning {
                streaming: true,
                ..
            })
        )
    })
}

/// Render one transcript row. The item's *variant* never changes once created
/// (the reducer only appends items or mutates their fields), so we pick the
/// layout once and read the mutable text/status reactively.
fn render_item(state: AppState, i: usize) -> impl IntoView {
    let variant = state.agent.chat.with_untracked(|c| c.items.get(i).cloned());

    match variant {
        Some(ChatItem::User { .. }) => label(move || item_text(state, i))
            .label_style(|s| s.selectable(true))
            .style(|s| {
                s.padding(10.0)
                    .margin_vert(4.0)
                    .margin_horiz(8.0)
                    .border_radius(8.0)
                    .background(theme::bg_active())
                    .color(theme::fg())
                    .width_full()
            })
            .into_any(),

        Some(ChatItem::Assistant { .. }) => {
            let content = floem::views::dyn_container(
                // While streaming, show plain text (cheap per-delta rebuild); once
                // the message is complete, render it as formatted markdown.
                move || (is_streaming(state, i), item_text(state, i)),
                move |(streaming, text)| {
                    if streaming {
                        label(move || format!("{text}\u{258d}"))
                            .label_style(|s| s.selectable(true))
                            .style(|s| s.color(theme::fg()).width_full())
                            .into_any()
                    } else {
                        crate::markdown_view::markdown_body_selectable(&text)
                            .style(|s| s.width_full())
                            .into_any()
                    }
                },
            )
            .style(|s| s.width_full());

            // A subtle "Copy" action for the whole message (rich_text isn't
            // selectable, so this is how you grab the answer text).
            let copied = RwSignal::new(false);
            let copy = label(move || {
                if copied.get() {
                    "Copied".to_string()
                } else {
                    "Copy".to_string()
                }
            })
            .style(|s| {
                s.padding_horiz(6.0)
                    .padding_vert(1.0)
                    .font_size(11.0)
                    .color(theme::fg_dim())
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.color(theme::fg()).background(theme::bg_hover()))
            })
            .on_click_stop(move |_| {
                let _ = floem::Clipboard::set_contents(item_text(state, i));
                copied.set(true);
                floem::action::exec_after(std::time::Duration::from_millis(1200), move |_| {
                    copied.set(false)
                });
            });
            let copy_row = stack((copy,)).style(|s| s.width_full().justify_end());

            stack((content, copy_row))
                .style(|s| {
                    s.flex_col()
                        .padding_horiz(14.0)
                        .padding_vert(6.0)
                        .width_full()
                })
                .into_any()
        }

        Some(ChatItem::Reasoning { .. }) => label(move || item_text(state, i))
            .label_style(|s| s.selectable(true))
            .style(|s| {
                s.padding_horiz(12.0)
                    .padding_vert(4.0)
                    .font_size(12.0)
                    .font_style(floem::text::Style::Italic)
                    .color(theme::fg_dim())
                    .width_full()
            })
            .into_any(),

        Some(ChatItem::Tool(_)) => tool_card(state, i).into_any(),

        Some(ChatItem::Receipt(_)) => receipt_card(state, i).into_any(),

        Some(ChatItem::Notice { error, .. }) => label(move || item_text(state, i))
            .style(move |s| {
                let c = if error {
                    theme::error()
                } else {
                    theme::fg_dim()
                };
                s.padding_horiz(12.0)
                    .padding_vert(3.0)
                    .font_size(12.0)
                    .color(c)
                    .width_full()
            })
            .into_any(),

        None => empty().into_any(),
    }
}

/// What a turn changed, under it: the files with their line counts, how many
/// review flags they raise, and **Review** / **Undo** for just this turn.
fn receipt_card(state: AppState, i: usize) -> impl IntoView {
    let receipt = move || {
        state.agent.chat.with(|c| match c.items.get(i) {
            Some(ChatItem::Receipt(r)) => Some(r.clone()),
            _ => None,
        })
    };
    let undone = move || receipt().is_some_and(|r| r.undone.is_some());

    let headline = label(move || {
        let Some(r) = receipt() else {
            return String::new();
        };
        if let Some(said) = &r.undone {
            return said.clone();
        }
        let flags = match r.flags {
            0 => String::new(),
            1 => " · ⚠ 1 flag".to_string(),
            n => format!(" · ⚠ {n} flags"),
        };
        format!("This turn changed {}{flags}", r.summary())
    })
    .style(move |s| {
        let flagged = receipt().is_some_and(|r| r.flags > 0 && r.undone.is_none());
        s.flex_grow(1.0_f32)
            .min_width(0.0)
            .font_size(12.0)
            .color(if flagged {
                theme::warning()
            } else {
                theme::fg()
            })
    });

    let button = |text: &'static str| {
        label(move || text.to_string()).style(move |s| {
            let s = s
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .font_size(11.0)
                .border(1.0)
                .border_color(theme::border())
                .border_radius(4.0)
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if undone() {
                s.hide()
            } else {
                s
            }
        })
    };
    let review = button("Review").on_click_stop(move |_| state.turn_review(i));
    let undo = button("Undo").on_click_stop(move |_| state.turn_undo(i));

    let head = stack((headline, review, undo)).style(|s| s.items_center().width_full().gap(6.0));

    // The files, a line each: path, then +added −removed.
    let files = label(move || {
        let Some(r) = receipt() else {
            return String::new();
        };
        let mut lines: Vec<String> = r
            .files
            .iter()
            .take(8)
            .map(|(path, added, removed)| format!("{path}  +{added} −{removed}"))
            .collect();
        if r.files.len() > 8 {
            lines.push(format!("… and {} more", r.files.len() - 8));
        }
        lines.join("\n")
    })
    .style(move |s| {
        let s = s
            .font_family(theme::mono_family())
            .font_size(11.0)
            .color(theme::fg_dim())
            .margin_top(4.0);
        if undone() {
            s.hide()
        } else {
            s
        }
    });

    // Stretched by the column to its width less the margins: `width_full()`
    // plus margins would overflow the panel and cut off the buttons.
    stack((head, files)).style(|s| {
        s.flex_col()
            .margin_vert(4.0)
            .margin_horiz(8.0)
            .padding(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
            .min_width(0.0)
    })
}

/// A collapsible-looking card for one tool call: status glyph + name + summary,
/// and a dimmed monospace preview of the result.
fn tool_card(state: AppState, i: usize) -> impl IntoView {
    let header = label(move || {
        state.agent.chat.with(|c| match c.items.get(i) {
            Some(ChatItem::Tool(tc)) => {
                let glyph = match tc.status {
                    ToolStatus::Running => "\u{25cf}", // ●
                    ToolStatus::Done => "\u{2713}",    // ✓
                    ToolStatus::Error => "\u{2717}",   // ✗
                };
                if tc.summary.is_empty() {
                    format!("{glyph}  {}", tc.name)
                } else {
                    format!("{glyph}  {}  {}", tc.name, tc.summary)
                }
            }
            _ => String::new(),
        })
    })
    .style(move |s| {
        let color = state.agent.chat.with(|c| match c.items.get(i) {
            Some(ChatItem::Tool(tc)) => match tc.status {
                ToolStatus::Running => theme::accent(),
                ToolStatus::Done => theme::fg_dim(),
                ToolStatus::Error => theme::error(),
            },
            _ => theme::fg_dim(),
        });
        s.font_family(theme::mono_family())
            .font_size(12.0)
            .color(color)
    });

    let result = label(move || {
        state.agent.chat.with(|c| match c.items.get(i) {
            Some(ChatItem::Tool(tc)) => {
                let r = tc.result.clone().unwrap_or_default();
                // A few lines keep cards compact: the first of a result, the
                // latest of output still arriving.
                let all: Vec<&str> = r.lines().collect();
                if all.len() <= 8 {
                    all.join("\n")
                } else if tc.status == ToolStatus::Running {
                    format!("…\n{}", all[all.len() - 8..].join("\n"))
                } else {
                    format!("{}\n…", all[..8].join("\n"))
                }
            }
            _ => String::new(),
        })
    })
    .style(|s| {
        s.font_family(theme::mono_family())
            .font_size(11.0)
            .color(theme::fg_dim())
            .margin_top(4.0)
    });

    stack((header, result)).style(|s| {
        s.flex_col()
            .margin_vert(4.0)
            .margin_horiz(8.0)
            .padding(8.0)
            .border(1.0)
            .border_color(theme::border())
            .border_radius(6.0)
            .background(theme::bg_panel())
            .min_width(0.0)
    })
}

/// What waits for the agent, above the composer: messages that will steer the
/// running work and ones that run next, and — while it works — how to send
/// either. Empty and out of the way when the agent is idle with nothing queued.
pub(crate) fn queue_rows(state: AppState) -> impl IntoView {
    let rows = dyn_stack(
        move || {
            state.agent.chat.with(|c| {
                c.steering
                    .iter()
                    .map(|t| ("Steer", t.clone()))
                    .chain(c.follow_up.iter().map(|t| ("Next", t.clone())))
                    .enumerate()
                    .collect::<Vec<_>>()
            })
        },
        |(i, (kind, text))| (*i, *kind, text.clone()),
        |(_, (kind, text))| {
            let first = text.lines().next().unwrap_or("").to_string();
            stack((
                label(move || kind.to_string())
                    .style(|s| s.font_size(11.0).color(theme::accent()).min_width(40.0)),
                label(move || first.clone()).style(|s| {
                    s.font_size(12.0)
                        .color(theme::fg_dim())
                        .text_ellipsis()
                        .flex_grow(1.0_f32)
                        .min_width(0.0)
                }),
            ))
            .style(|s| s.items_center().width_full().gap(6.0).padding_vert(2.0))
        },
    )
    .style(|s| s.flex_col().width_full());

    let hint =
        label(|| "↵ steers the running work · ⌥↵ runs it next".to_string()).style(move |s| {
            let s = s.font_size(11.0).color(theme::fg_dim()).padding_vert(2.0);
            if state.agent.chat.with(|c| c.running) {
                s
            } else {
                s.hide()
            }
        });

    stack((rows, hint)).style(move |s| {
        let busy = state
            .agent
            .chat
            .with(|c| c.running || !c.steering.is_empty() || !c.follow_up.is_empty());
        let s = s
            .flex_col()
            .width_full()
            .padding_horiz(10.0)
            .padding_bottom(4.0);
        if busy {
            s
        } else {
            s.hide()
        }
    })
}

/// The scrollable transcript.
pub(crate) fn transcript(state: AppState) -> impl IntoView {
    let rows = dyn_stack(
        move || 0..state.agent.chat.with(|c| c.items.len()),
        |i| *i,
        move |i| render_item(state, i),
    )
    .style(|s| s.flex_col().width_full().padding_vert(10.0).gap(2.0));

    scroll(rows)
        // min_height(0) lets the scroll area shrink inside the flex column and
        // scroll internally, instead of growing to fit content and pushing the
        // composer off the bottom of the panel.
        .style(|s| {
            s.width_full()
                .flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_height(0.0)
                .background(theme::bg())
        })
        // Follow the tail as content streams in. Floem takes a percentage
        // (0–100): `1.0` was 1%, which put the view back near the top each
        // time a reply or a tool's output grew.
        .scroll_to_percent(move || {
            let _ = transcript_size(state);
            100.0
        })
}

/// The multi-line composer input, shared by the classic native panel and
/// Cascade: word-wrapped, auto-growing (one to ~7 lines, then scrolling), Enter
/// sends and Shift+Enter breaks the line, focused when the panel opens.
/// `framed` draws its own border and background; Cascade's card supplies those.
pub(crate) fn composer_input(state: AppState, placeholder: &str, framed: bool) -> impl IntoView {
    // Kept deliberately minimal (no reactive mirror) and all mutation is
    // deferred out of the key handler (see send_composer), which is what
    // previously aborted on Enter.
    let doc = Rc::new(TextDocument::new(state.cx, String::new()));
    state.agent.composer_doc.set(Some(doc.clone()));

    let te = text_editor_keys("", move |editor_sig, kp, mods| {
        if let KeyInput::Keyboard(key, physical) = &kp.key {
            if matches!(key, Key::Named(NamedKey::Enter))
                && !mods.shift()
                && !mods.meta()
                && !mods.control()
                && !mods.alt()
            {
                state.send_composer();
                return CommandExecuted::Yes;
            }
            // ⌥↵: run after the current work instead of steering it.
            if matches!(key, Key::Named(NamedKey::Enter))
                && mods.alt()
                && !mods.shift()
                && !mods.meta()
                && !mods.control()
            {
                state.send_composer_next();
                return CommandExecuted::Yes;
            }
            if handle_shortcut(state, key, Some(physical), mods) {
                return CommandExecuted::Yes;
            }
        }
        default_key_handler(editor_sig)(kp, mods)
    })
    .use_doc(doc.clone() as Rc<dyn Document>)
    .editor_style(|s| {
        theme::editor_style(s)
            .wrap_method(WrapMethod::EditorWidth)
            .hide_gutter(true)
    });

    // Grab the editor handle before styling so the height closure can read the
    // current visual-line count.
    let ed = te.editor().clone();
    doc.add_placeholder(ed.id(), placeholder.to_string());

    // Auto-grow: the box height follows the number of *visual* (soft-wrapped)
    // lines, clamped between one line and ~7, then it scrolls internally.
    let ed_h = ed.clone();
    let doc_h = doc.clone();
    let te = te.style(move |s| {
        use floem::views::editor::text::Document;
        doc_h.cache_rev().get(); // re-evaluate whenever the text changes
        ed_h.screen_lines.with(|_| {}); // and after each (re)layout / wrap
        let line_h = (ed_h.line_height(0) as f64).max(16.0);
        let lines = (ed_h.last_vline().get() + 1).clamp(1, 7);
        let height = lines as f64 * line_h + 16.0; // + vertical padding
        let s = s
            .flex_grow(1.0_f32)
            .min_width(0.0)
            .width_full()
            .height(height)
            .font_size(13.0)
            .padding_horiz(10.0)
            .padding_vert(8.0);
        if framed {
            s.border(1.0)
                .border_color(theme::border())
                .border_radius(10.0)
                .background(theme::bg_panel())
        } else {
            s
        }
    });

    // Focus the field when the panel opens / the agent (re)starts. The focusable
    // is the editor's *inner* content view, so request focus on its view id
    // rather than the wrapper (a plain .request_focus() focuses the wrong view).
    state.agent.composer_editor.set(Some(ed.clone()));
    create_effect(move |_| {
        state.agent.focus_pulse.get();
        if let Some(vid) = ed.editor_view_id.get_untracked() {
            vid.request_focus();
        }
    });
    te
}
