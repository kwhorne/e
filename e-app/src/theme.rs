//! Reactive colour palette with a light/dark toggle.
//!
//! Colours are functions, not constants: each reads a thread-local `dark`
//! signal, so any Floem style closure that uses them re-runs (and re-themes)
//! when the theme is toggled.

use std::cell::RefCell;

use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};
use floem::style::Style;
use floem::views::editor::text::{default_dark_color, default_light_theme};
use floem::views::EditorCustomStyle;

thread_local! {
    static DARK: RefCell<Option<RwSignal<bool>>> = const { RefCell::new(None) };
    /// The chosen code font (a family name; empty = the system monospace).
    static MONO: RefCell<Option<RwSignal<String>>> = const { RefCell::new(None) };
}

fn mono_signal() -> RwSignal<String> {
    MONO.with(|c| {
        *c.borrow_mut()
            .get_or_insert_with(|| RwSignal::new(crate::fonts::DEFAULT_FONT.to_string()))
    })
}

/// Change the code font. Every style closure that read [`mono_family`]
/// re-runs; editors are re-laid-out by the caller (`repaint_all_buffers`).
pub fn set_mono_family(name: &str) {
    mono_signal().set(name.trim().to_string());
}

/// The chosen code font's name (empty = system monospace). Tracks the signal.
pub fn mono_family_name() -> String {
    mono_signal().get()
}

/// The family list for code — the chosen font, then the system monospace —
/// for `.font_family(...)` and `FamilyOwned::parse_list`. Tracks the signal,
/// so a style closure re-themes when the font changes.
pub fn mono_family() -> String {
    crate::fonts::family_list(&mono_signal().get())
}

/// [`mono_family`] without tracking, for layout code outside effects.
pub fn mono_family_untracked() -> String {
    use floem::reactive::SignalWith;
    mono_signal().with_untracked(|n| crate::fonts::family_list(n))
}

fn dark_signal() -> RwSignal<bool> {
    DARK.with(|c| *c.borrow_mut().get_or_insert_with(|| RwSignal::new(true)))
}

pub fn is_dark() -> bool {
    dark_signal().get()
}

pub fn set_dark(dark: bool) {
    dark_signal().set(dark);
}

pub fn toggle() {
    let s = dark_signal();
    let cur = s.get_untracked();
    s.set(!cur);
}

/// Pick the dark or light variant for the current theme (tracks the signal).
fn pick(dark: Color, light: Color) -> Color {
    if is_dark() {
        dark
    } else {
        light
    }
}

pub fn bg() -> Color {
    pick(
        Color::from_rgb8(0x1b, 0x1e, 0x24),
        Color::from_rgb8(0xf6, 0xf7, 0xf9),
    )
}
pub fn bg_panel() -> Color {
    pick(
        Color::from_rgb8(0x21, 0x25, 0x2b),
        Color::from_rgb8(0xec, 0xee, 0xf1),
    )
}
pub fn bg_active() -> Color {
    pick(
        Color::from_rgb8(0x2c, 0x31, 0x39),
        Color::from_rgb8(0xff, 0xff, 0xff),
    )
}
pub fn bg_hover() -> Color {
    pick(
        Color::from_rgb8(0x33, 0x39, 0x42),
        Color::from_rgb8(0xe1, 0xe5, 0xea),
    )
}
pub fn border() -> Color {
    pick(
        Color::from_rgb8(0x33, 0x39, 0x42),
        Color::from_rgb8(0xd4, 0xd8, 0xde),
    )
}
pub fn fg() -> Color {
    pick(
        Color::from_rgb8(0xc5, 0xcb, 0xd6),
        Color::from_rgb8(0x2b, 0x2f, 0x36),
    )
}
pub fn fg_dim() -> Color {
    pick(
        Color::from_rgb8(0x7a, 0x82, 0x90),
        Color::from_rgb8(0x6a, 0x72, 0x80),
    )
}
pub fn accent() -> Color {
    pick(
        Color::from_rgb8(0x5c, 0x9c, 0xf5),
        Color::from_rgb8(0x2f, 0x6e, 0xf5),
    )
}

/// Themed colours for a `text_input`, overriding Floem's default white
/// focus/hover backgrounds. Apply this, then add layout (size/padding/border).
pub fn input_colors(s: Style) -> Style {
    s.background(bg())
        .color(fg())
        .border_color(border())
        .hover(|s| s.background(bg()))
        .focus(|s| {
            s.background(bg())
                .border_color(accent())
                .hover(|s| s.background(bg()))
        })
}

/// Editor (text area) colours, switched with the theme. Reactive.
pub fn editor_style(style: EditorCustomStyle) -> EditorCustomStyle {
    if is_dark() {
        default_dark_color(style)
    } else {
        default_light_theme(style)
    }
}
