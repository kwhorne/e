//! The bottom status bar.

use floem::{
    peniko::Color,
    reactive::{RwSignal, SignalGet},
    views::{label, stack, Decorators},
    IntoView,
};

use e_core::language::Language;

/// A thin bar at the bottom of the window: file name + dirty marker on the
/// left, detected language on the right.
pub fn status_bar(name: String, language: Language, dirty: RwSignal<bool>) -> impl IntoView {
    let left = label(move || {
        let mark = if dirty.get() { " ●" } else { "" };
        format!("{name}{mark}")
    });

    let right = label(move || language.name().to_string());

    stack((left, right)).style(|s| {
        s.height(24.0)
            .width_full()
            .items_center()
            .justify_between()
            .padding_horiz(10.0)
            .background(Color::from_rgb8(0x24, 0x28, 0x30))
            .color(Color::from_rgb8(0xc0, 0xc5, 0xce))
    })
}
