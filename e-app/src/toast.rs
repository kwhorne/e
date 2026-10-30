//! Messages in the window instead of the desktop's notification centre: a
//! short stack at the bottom right that goes by itself, and stays while the
//! pointer is on it. A click dismisses one. While the window isn't in front,
//! a message goes to the desktop as well, so a test run or an agent that
//! finishes while you're elsewhere still reaches you.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use floem::ext_event::create_signal_from_channel;
use floem::reactive::{create_effect, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

/// A message on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    /// How many times it was said while on screen.
    pub count: u32,
    /// Bumped when it is said again, so the older timer lets it be.
    generation: u64,
    hovered: bool,
}

/// At most this many on screen; the oldest goes first.
const SHOWN: usize = 4;

/// Messages said since the stack last took them, from any thread.
static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WAKE: OnceLock<Mutex<Sender<()>>> = OnceLock::new();
static FOCUSED: AtomicBool = AtomicBool::new(true);
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Show `message` in the window.
pub fn post(message: &str) {
    if message.trim().is_empty() {
        return;
    }
    if let Ok(mut q) = QUEUE.lock() {
        q.push(message.to_string());
    }
    if let Some(tx) = WAKE.get().and_then(|t| t.lock().ok()) {
        let _ = tx.send(());
    }
}

/// Whether the window is in front.
pub fn window_focused() -> bool {
    FOCUSED.load(Ordering::Relaxed)
}

pub fn set_window_focused(focused: bool) {
    FOCUSED.store(focused, Ordering::Relaxed);
}

/// How long `text` stays: long enough to read it.
fn life(text: &str) -> Duration {
    let ms = 3500 + 45 * text.chars().count() as u64;
    Duration::from_millis(ms.min(10_000))
}

/// Add `text` to `shown`: a message already there is counted again rather
/// than shown twice. Returns the toast's id and generation for its timer.
fn add(shown: &mut Vec<Toast>, text: &str) -> (u64, u64) {
    // Said again: counted, and moved down to the newest.
    if let Some(at) = shown.iter().position(|t| t.text == text) {
        let mut t = shown.remove(at);
        t.count += 1;
        t.generation += 1;
        let timer = (t.id, t.generation);
        shown.push(t);
        return timer;
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    shown.push(Toast {
        id,
        text: text.to_string(),
        count: 1,
        generation: 0,
        hovered: false,
    });
    if shown.len() > SHOWN {
        shown.remove(0);
    }
    (id, 0)
}

/// Let toast `id` go after `after`, unless it was said again since or the
/// pointer is on it.
fn expire(state: AppState, id: u64, generation: u64, after: Duration) {
    floem::action::exec_after(after, move |_| {
        let held = state.toasts.with_untracked(|v| {
            v.iter()
                .find(|t| t.id == id)
                .map(|t| (t.generation, t.hovered))
        });
        match held {
            Some((g, true)) if g == generation => {
                expire(state, id, generation, Duration::from_millis(1500))
            }
            Some((g, false)) if g == generation => dismiss(state, id),
            _ => {}
        }
    });
}

fn dismiss(state: AppState, id: u64) {
    state.toasts.update(|v| v.retain(|t| t.id != id));
}

/// Take what was said since last time onto the stack.
fn take(state: AppState) {
    let said: Vec<String> = QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default();
    for text in said {
        let mut timer = (0, 0);
        state.toasts.update(|v| timer = add(v, &text));
        expire(state, timer.0, timer.1, life(&text));
    }
}

/// The stack, bottom right above the status bar (and above the update
/// notice while it shows).
pub fn toasts(state: AppState) -> impl IntoView {
    let (tx, rx) = channel();
    let _ = WAKE.set(Mutex::new(tx));
    let woken = create_signal_from_channel(rx);
    create_effect(move |_| {
        woken.get();
        take(state);
    });

    let rows = dyn_stack(
        move || state.toasts.get(),
        |t| (t.id, t.count),
        move |t| {
            let id = t.id;
            let count = (t.count > 1).then(|| format!("×{}", t.count));
            stack((
                label(move || t.text.clone()).style(|s| {
                    s.color(theme::fg())
                        .font_size(13.0)
                        .line_height(1.35)
                        .max_width(360.0)
                }),
                label(move || count.clone().unwrap_or_default()).style(move |s| {
                    let s = s.color(theme::fg_dim()).font_size(11.0).margin_left(8.0);
                    if t.count > 1 {
                        s
                    } else {
                        s.hide()
                    }
                }),
            ))
            .style(|s| {
                s.flex_row()
                    .items_start()
                    .padding_vert(9.0)
                    .padding_horiz(14.0)
                    .background(theme::bg_panel())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(8.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_event_cont(floem::event::EventListener::PointerEnter, move |_| {
                state.toasts.update(|v| {
                    if let Some(t) = v.iter_mut().find(|t| t.id == id) {
                        t.hovered = true;
                    }
                })
            })
            .on_event_cont(floem::event::EventListener::PointerLeave, move |_| {
                state.toasts.update(|v| {
                    if let Some(t) = v.iter_mut().find(|t| t.id == id) {
                        t.hovered = false;
                    }
                })
            })
            .on_click_stop(move |_| dismiss(state, id))
        },
    )
    .style(|s| s.flex_col().items_end().gap(8.0));

    stack((rows,)).style(move |s| {
        let lifted = state.update_info.with(Option::is_some)
            || !matches!(
                state.update_status.get(),
                crate::updater::UpdateStatus::Idle | crate::updater::UpdateStatus::Checking
            );
        let s = s
            .absolute()
            .inset_right(20.0)
            .inset_bottom(if lifted { 220.0 } else { 46.0 })
            .z_index(60);
        if state.toasts.with(Vec::is_empty) {
            s.hide()
        } else {
            s
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_said_again_is_counted_not_repeated() {
        let mut shown = Vec::new();
        let (a, g0) = add(&mut shown, "Saved");
        let (b, g1) = add(&mut shown, "Saved");
        assert_eq!((a, shown.len(), shown[0].count), (b, 1, 2));
        assert!(g1 > g0, "the older timer must not take it");
        add(&mut shown, "Pint failed");
        add(&mut shown, "Saved");
        let order: Vec<_> = shown.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(order, ["Pint failed", "Saved"], "the repeat is the newest");
    }

    #[test]
    fn only_the_newest_few_stay() {
        let mut shown = Vec::new();
        for i in 0..SHOWN + 2 {
            add(&mut shown, &format!("m{i}"));
        }
        assert_eq!(shown.len(), SHOWN);
        assert_eq!(shown[0].text, "m2");
    }

    #[test]
    fn a_long_message_stays_longer_but_not_forever() {
        assert!(life(&"x".repeat(120)) > life("Saved"));
        assert_eq!(life(&"x".repeat(5000)), Duration::from_secs(10));
    }
}
