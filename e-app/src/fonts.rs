//! The code font: which one is in use, which are installed, and the one that
//! ships with `e`.
//!
//! JetBrains Mono (OFL, see `fonts/JetBrainsMono/OFL.txt`) is compiled in and
//! registered with the font system at startup, so the default looks the same
//! on every machine. Settings offers the usual programming fonts; the ones
//! not installed are listed but greyed, so the choice never silently falls
//! back to something else.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use floem::text::FONT_SYSTEM;

/// The font `e` ships and starts with.
pub const DEFAULT_FONT: &str = "JetBrains Mono";

/// The fonts programmers reach for, in the order the menu shows them. The
/// installed ones come first at runtime; this is the canonical order within
/// each group.
pub const PROGRAMMING_FONTS: &[&str] = &[
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Code",
    "Source Code Pro",
    "IBM Plex Mono",
    "Hack",
    "Inconsolata",
    "Monaco",
    "Menlo",
    "SF Mono",
    "Consolas",
    "Ubuntu Mono",
    "Roboto Mono",
    "DejaVu Sans Mono",
    "Courier New",
    "Andale Mono",
    "PT Mono",
    "Iosevka",
    "Victor Mono",
    "Geist Mono",
    "Monaspace Neon",
    "Berkeley Mono",
    "Input Mono",
    "Noto Sans Mono",
    "Liberation Mono",
];

/// Register the bundled faces with the shared font system. Call once, before
/// anything is laid out.
pub fn install_bundled() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let mut fs = FONT_SYSTEM.lock();
        let db = fs.db_mut();
        for data in [
            &include_bytes!("../../fonts/JetBrainsMono/JetBrainsMono-Regular.ttf")[..],
            &include_bytes!("../../fonts/JetBrainsMono/JetBrainsMono-Bold.ttf")[..],
            &include_bytes!("../../fonts/JetBrainsMono/JetBrainsMono-Italic.ttf")[..],
            &include_bytes!("../../fonts/JetBrainsMono/JetBrainsMono-BoldItalic.ttf")[..],
        ] {
            db.load_font_data(data.to_vec());
        }
    });
}

/// Every family the font system knows, sorted. Read once: the system font
/// database doesn't change while `e` runs, and the bundled faces are
/// registered before the first call.
pub fn installed_families() -> &'static BTreeSet<String> {
    static FAMILIES: OnceLock<BTreeSet<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        install_bundled();
        let fs = FONT_SYSTEM.lock();
        fs.db()
            .faces()
            .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
            .collect()
    })
}

pub fn is_installed(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && installed_families().contains(name)
}

/// The family list to hand to a style: the chosen font first, the system
/// monospace as the fallback. Empty means the system monospace alone.
pub fn family_list(chosen: &str) -> String {
    let chosen = chosen.trim();
    if chosen.is_empty() || chosen.eq_ignore_ascii_case("monospace") {
        "monospace".to_string()
    } else {
        format!("{chosen}, monospace")
    }
}

/// What to call a choice in the UI.
pub fn display_name(chosen: &str) -> String {
    if chosen.trim().is_empty() {
        "System monospace".to_string()
    } else {
        chosen.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_list_always_falls_back_to_monospace() {
        assert_eq!(family_list(""), "monospace");
        assert_eq!(family_list("  "), "monospace");
        assert_eq!(family_list("monospace"), "monospace");
        assert_eq!(family_list("Fira Code"), "Fira Code, monospace");
        assert_eq!(family_list(" Menlo "), "Menlo, monospace");
    }

    #[test]
    fn the_bundled_font_is_installed_and_is_the_default() {
        assert!(is_installed(DEFAULT_FONT));
        assert!(PROGRAMMING_FONTS.contains(&DEFAULT_FONT));
        assert!(!is_installed(""));
        assert!(!is_installed("No Such Font 1234"));
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name(""), "System monospace");
        assert_eq!(display_name("Hack"), "Hack");
    }
}
