//! Reactive colour themes.
//!
//! A theme is one [`Palette`]: the UI chrome, the editor chrome and the code
//! colours together. Colours are functions, not constants: each reads a
//! thread-local signal holding the current theme, so any Floem style closure
//! that uses them re-runs (and re-themes) when the theme changes. Editors
//! re-lay out their lines through `AppState::set_theme`, which is why the code
//! colours change at once too.
//!
//! Seven ship: `e`'s own Dark and Light, Tokyo Night, Darcula, Material
//! (Oceanic), Nord and Palenight. The code font lives here as well, so one
//! module answers every "how should code look" question.

use std::cell::RefCell;

use e_core::syntax::HighlightKind;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Style;
use floem::views::EditorCustomStyle;

/// Every colour a theme decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    // UI chrome
    pub bg: Color,
    pub bg_panel: Color,
    pub bg_active: Color,
    pub bg_hover: Color,
    pub border: Color,
    pub fg: Color,
    pub fg_dim: Color,
    pub accent: Color,
    /// Text drawn on the accent (buttons).
    pub on_accent: Color,
    /// The terminal and other "deeper than the panels" surfaces.
    pub bg_deep: Color,
    // Editor chrome
    pub editor_bg: Color,
    pub gutter_fg: Color,
    pub cursor: Color,
    pub selection: Color,
    pub current_line: Color,
    pub indent_guide: Color,
    // Meaning
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,
    // Code
    pub keyword: Color,
    pub function: Color,
    pub type_: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub property: Color,
    pub operator: Color,
    pub namespace: Color,
    pub attribute: Color,
    pub tag: Color,
    // Marks in the editor
    pub find_current: Color,
    pub find_other: Color,
    pub git_added: Color,
    pub git_modified: Color,
}

pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub palette: Palette,
}

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

/// The themes, in the order the picker shows them. The first is the default.
pub const THEMES: &[Theme] = &[
    Theme {
        id: "dark",
        name: "Dark",
        dark: true,
        palette: Palette {
            bg: rgb(0x1b1e24),
            bg_panel: rgb(0x21252b),
            bg_active: rgb(0x2c3139),
            bg_hover: rgb(0x333942),
            border: rgb(0x333942),
            fg: rgb(0xc5cbd6),
            fg_dim: rgb(0x7a8290),
            accent: rgb(0x5c9cf5),
            on_accent: rgb(0x14161b),
            bg_deep: rgb(0x14161b),
            editor_bg: rgb(0x282c34),
            gutter_fg: rgb(0x5c6370),
            cursor: rgb(0x528bff),
            selection: rgb(0x3e4451),
            current_line: rgb(0x2c313c),
            indent_guide: rgb(0x3e4451),
            error: rgb(0xe06c75),
            warning: rgb(0xe5c07b),
            success: rgb(0x98c379),
            info: rgb(0x61afef),
            keyword: rgb(0xc678dd),
            function: rgb(0x61afef),
            type_: rgb(0xe5c07b),
            string: rgb(0x98c379),
            number: rgb(0xd19a66),
            comment: rgb(0x5c6370),
            property: rgb(0xe06c75),
            operator: rgb(0x56b6c2),
            namespace: rgb(0xe5c07b),
            attribute: rgb(0x61afef),
            tag: rgb(0xe06c75),
            find_current: rgb(0xc88a3a),
            find_other: rgb(0x5a532a),
            git_added: rgb(0x6ab04a),
            git_modified: rgb(0x4a7cc0),
        },
    },
    Theme {
        id: "light",
        name: "Light",
        dark: false,
        palette: Palette {
            bg: rgb(0xf6f7f9),
            bg_panel: rgb(0xeceef1),
            bg_active: rgb(0xffffff),
            bg_hover: rgb(0xe1e5ea),
            border: rgb(0xd4d8de),
            fg: rgb(0x2b2f36),
            fg_dim: rgb(0x6a7280),
            accent: rgb(0x2f6ef5),
            on_accent: rgb(0xffffff),
            bg_deep: rgb(0xeef0f3),
            editor_bg: rgb(0xfafafa),
            gutter_fg: rgb(0xa0a1a7),
            cursor: rgb(0x526fff),
            selection: rgb(0xe5e5e6),
            current_line: rgb(0xf2f2f2),
            indent_guide: rgb(0xe5e5e6),
            error: rgb(0xe45649),
            warning: rgb(0xc18401),
            success: rgb(0x50a14f),
            info: rgb(0x4078f2),
            keyword: rgb(0xa626a4),
            function: rgb(0x4078f2),
            type_: rgb(0xc18401),
            string: rgb(0x50a14f),
            number: rgb(0x986801),
            comment: rgb(0xa0a1a7),
            property: rgb(0xe45649),
            operator: rgb(0x0184bc),
            namespace: rgb(0xc18401),
            attribute: rgb(0x4078f2),
            tag: rgb(0xe45649),
            find_current: rgb(0xf2c37a),
            find_other: rgb(0xf6e3b0),
            git_added: rgb(0x6ab04a),
            git_modified: rgb(0x4a7cc0),
        },
    },
    Theme {
        id: "tokyo-night",
        name: "Tokyo Night",
        dark: true,
        palette: Palette {
            bg: rgb(0x1a1b26),
            bg_panel: rgb(0x16161e),
            bg_active: rgb(0x24283b),
            bg_hover: rgb(0x292e42),
            border: rgb(0x292e42),
            fg: rgb(0xa9b1d6),
            fg_dim: rgb(0x565f89),
            accent: rgb(0x7aa2f7),
            on_accent: rgb(0x1a1b26),
            bg_deep: rgb(0x16161e),
            editor_bg: rgb(0x1a1b26),
            gutter_fg: rgb(0x3b4261),
            cursor: rgb(0xc0caf5),
            selection: rgb(0x283457),
            current_line: rgb(0x24283b),
            indent_guide: rgb(0x292e42),
            error: rgb(0xf7768e),
            warning: rgb(0xe0af68),
            success: rgb(0x9ece6a),
            info: rgb(0x7dcfff),
            keyword: rgb(0xbb9af7),
            function: rgb(0x7aa2f7),
            type_: rgb(0x2ac3de),
            string: rgb(0x9ece6a),
            number: rgb(0xff9e64),
            comment: rgb(0x565f89),
            property: rgb(0x7dcfff),
            operator: rgb(0x89ddff),
            namespace: rgb(0x2ac3de),
            attribute: rgb(0xbb9af7),
            tag: rgb(0xf7768e),
            find_current: rgb(0x8a6a2a),
            find_other: rgb(0x3d3a2a),
            git_added: rgb(0x9ece6a),
            git_modified: rgb(0x7aa2f7),
        },
    },
    Theme {
        id: "darcula",
        name: "Darcula",
        dark: true,
        palette: Palette {
            bg: rgb(0x2b2b2b),
            bg_panel: rgb(0x3c3f41),
            bg_active: rgb(0x45494a),
            bg_hover: rgb(0x4e5254),
            border: rgb(0x4a4a4a),
            fg: rgb(0xbbbbbb),
            fg_dim: rgb(0x787878),
            accent: rgb(0x589df6),
            on_accent: rgb(0xffffff),
            bg_deep: rgb(0x232323),
            editor_bg: rgb(0x2b2b2b),
            gutter_fg: rgb(0x606366),
            cursor: rgb(0xbbbbbb),
            selection: rgb(0x214283),
            current_line: rgb(0x323232),
            indent_guide: rgb(0x3a3a3a),
            error: rgb(0xff6b68),
            warning: rgb(0xbbb529),
            success: rgb(0x629755),
            info: rgb(0x6897bb),
            keyword: rgb(0xcc7832),
            function: rgb(0xffc66d),
            type_: rgb(0xa9b7c6),
            string: rgb(0x6a8759),
            number: rgb(0x6897bb),
            comment: rgb(0x808080),
            property: rgb(0x9876aa),
            operator: rgb(0xa9b7c6),
            namespace: rgb(0xa9b7c6),
            attribute: rgb(0xbababa),
            tag: rgb(0xe8bf6a),
            find_current: rgb(0x32593d),
            find_other: rgb(0x2d3e33),
            git_added: rgb(0x629755),
            git_modified: rgb(0x6897bb),
        },
    },
    Theme {
        id: "material",
        name: "Material",
        dark: true,
        palette: Palette {
            bg: rgb(0x263238),
            bg_panel: rgb(0x1e272c),
            bg_active: rgb(0x314549),
            bg_hover: rgb(0x2e3c43),
            border: rgb(0x37474f),
            fg: rgb(0xb0bec5),
            fg_dim: rgb(0x546e7a),
            accent: rgb(0x80cbc4),
            on_accent: rgb(0x263238),
            bg_deep: rgb(0x1e272c),
            editor_bg: rgb(0x263238),
            gutter_fg: rgb(0x4a5c66),
            cursor: rgb(0xffcc00),
            selection: rgb(0x2f4a4c),
            current_line: rgb(0x2b3a40),
            indent_guide: rgb(0x37474f),
            error: rgb(0xff5370),
            warning: rgb(0xffcb6b),
            success: rgb(0xc3e88d),
            info: rgb(0x82aaff),
            keyword: rgb(0xc792ea),
            function: rgb(0x82aaff),
            type_: rgb(0xffcb6b),
            string: rgb(0xc3e88d),
            number: rgb(0xf78c6c),
            comment: rgb(0x546e7a),
            property: rgb(0xf07178),
            operator: rgb(0x89ddff),
            namespace: rgb(0xffcb6b),
            attribute: rgb(0xffcb6b),
            tag: rgb(0xf07178),
            find_current: rgb(0x6b5a24),
            find_other: rgb(0x3a4046),
            git_added: rgb(0xc3e88d),
            git_modified: rgb(0x82aaff),
        },
    },
    Theme {
        id: "nord",
        name: "Nord",
        dark: true,
        palette: Palette {
            bg: rgb(0x2e3440),
            bg_panel: rgb(0x3b4252),
            bg_active: rgb(0x434c5e),
            bg_hover: rgb(0x4c566a),
            border: rgb(0x434c5e),
            fg: rgb(0xd8dee9),
            fg_dim: rgb(0x7b88a1),
            accent: rgb(0x88c0d0),
            on_accent: rgb(0x2e3440),
            bg_deep: rgb(0x272c36),
            editor_bg: rgb(0x2e3440),
            gutter_fg: rgb(0x4c566a),
            cursor: rgb(0xd8dee9),
            selection: rgb(0x434c5e),
            current_line: rgb(0x3b4252),
            indent_guide: rgb(0x434c5e),
            error: rgb(0xbf616a),
            warning: rgb(0xebcb8b),
            success: rgb(0xa3be8c),
            info: rgb(0x81a1c1),
            keyword: rgb(0x81a1c1),
            function: rgb(0x88c0d0),
            type_: rgb(0x8fbcbb),
            string: rgb(0xa3be8c),
            number: rgb(0xb48ead),
            comment: rgb(0x616e88),
            property: rgb(0xd8dee9),
            operator: rgb(0x81a1c1),
            namespace: rgb(0x8fbcbb),
            attribute: rgb(0xd08770),
            tag: rgb(0x81a1c1),
            find_current: rgb(0x5d5233),
            find_other: rgb(0x3f4657),
            git_added: rgb(0xa3be8c),
            git_modified: rgb(0x81a1c1),
        },
    },
    Theme {
        id: "palenight",
        name: "Palenight",
        dark: true,
        palette: Palette {
            bg: rgb(0x292d3e),
            bg_panel: rgb(0x242838),
            bg_active: rgb(0x32374d),
            bg_hover: rgb(0x3a3f58),
            border: rgb(0x3a3f58),
            fg: rgb(0xa6accd),
            fg_dim: rgb(0x676e95),
            accent: rgb(0x82aaff),
            on_accent: rgb(0x292d3e),
            bg_deep: rgb(0x202331),
            editor_bg: rgb(0x292d3e),
            gutter_fg: rgb(0x4e5579),
            cursor: rgb(0xffcc00),
            selection: rgb(0x3c4363),
            current_line: rgb(0x2f3348),
            indent_guide: rgb(0x3a3f58),
            error: rgb(0xff5370),
            warning: rgb(0xffcb6b),
            success: rgb(0xc3e88d),
            info: rgb(0x82aaff),
            keyword: rgb(0xc792ea),
            function: rgb(0x82aaff),
            type_: rgb(0xffcb6b),
            string: rgb(0xc3e88d),
            number: rgb(0xf78c6c),
            comment: rgb(0x676e95),
            property: rgb(0xf07178),
            operator: rgb(0x89ddff),
            namespace: rgb(0xffcb6b),
            attribute: rgb(0xffcb6b),
            tag: rgb(0xf07178),
            find_current: rgb(0x6b5a24),
            find_other: rgb(0x3d3e50),
            git_added: rgb(0xc3e88d),
            git_modified: rgb(0x82aaff),
        },
    },
];

/// The theme with `id`, if there is one.
pub fn by_id(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.id == id)
}

thread_local! {
    /// Index into [`THEMES`] of the theme in use.
    static CURRENT: RefCell<Option<RwSignal<usize>>> = const { RefCell::new(None) };
    /// The dark theme to come back to when `F8` leaves Light.
    static LAST_DARK: RefCell<&'static str> = const { RefCell::new("dark") };
    /// The chosen code font (a family name; empty = the system monospace).
    static MONO: RefCell<Option<RwSignal<String>>> = const { RefCell::new(None) };
}

fn current_signal() -> RwSignal<usize> {
    CURRENT.with(|c| *c.borrow_mut().get_or_insert_with(|| RwSignal::new(0)))
}

/// Switch themes. Unknown ids are ignored.
pub fn set_theme(id: &str) {
    let Some(idx) = THEMES.iter().position(|t| t.id == id) else {
        return;
    };
    if THEMES[idx].dark {
        LAST_DARK.with(|l| *l.borrow_mut() = THEMES[idx].id);
    }
    current_signal().set(idx);
}

/// The id of the theme in use. Tracks the signal.
pub fn current_id() -> &'static str {
    THEMES[current_signal().get()].id
}

pub fn current() -> &'static Theme {
    &THEMES[current_signal().get()]
}

/// The palette in use. Tracks the signal, so a style closure re-themes.
pub fn palette() -> Palette {
    current().palette
}

/// [`palette`] without tracking, for layout code outside effects.
pub fn palette_untracked() -> Palette {
    THEMES[current_signal().with_untracked(|i| *i)].palette
}

pub fn is_dark() -> bool {
    current().dark
}

/// Where `F8` goes next: Light from any dark theme, and back to the dark
/// theme that was in use before.
pub fn toggle_target() -> &'static str {
    if current_signal().with_untracked(|i| THEMES[*i].dark) {
        "light"
    } else {
        LAST_DARK.with(|l| *l.borrow())
    }
}

// ---- UI chrome ------------------------------------------------------------------

pub fn bg() -> Color {
    palette().bg
}
pub fn bg_panel() -> Color {
    palette().bg_panel
}
pub fn bg_active() -> Color {
    palette().bg_active
}
pub fn bg_hover() -> Color {
    palette().bg_hover
}
pub fn border() -> Color {
    palette().border
}
pub fn fg() -> Color {
    palette().fg
}
pub fn fg_dim() -> Color {
    palette().fg_dim
}
pub fn accent() -> Color {
    palette().accent
}
/// Text on an accent-coloured control.
pub fn on_accent() -> Color {
    palette().on_accent
}
/// The terminal's and the agent terminal's background.
pub fn bg_deep() -> Color {
    palette().bg_deep
}

// ---- Meaning -------------------------------------------------------------------

pub fn error() -> Color {
    palette().error
}
pub fn warning() -> Color {
    palette().warning
}
pub fn success() -> Color {
    palette().success
}
pub fn info() -> Color {
    palette().info
}
pub fn keyword() -> Color {
    palette().keyword
}
pub fn number() -> Color {
    palette().number
}

// ---- Code ----------------------------------------------------------------------

/// The colour for a syntax token class in `p`; `None` keeps the foreground.
pub fn syntax_color_in(p: &Palette, kind: HighlightKind) -> Option<Color> {
    use HighlightKind::*;
    Some(match kind {
        Keyword => p.keyword,
        Function | Constructor => p.function,
        Type => p.type_,
        String => p.string,
        Number | Constant => p.number,
        Comment => p.comment,
        Property => p.property,
        Operator | Escape => p.operator,
        Namespace => p.namespace,
        Attribute => p.attribute,
        Label | Tag => p.tag,
        Punctuation => p.fg_dim,
        Variable => return None,
    })
}

/// [`syntax_color_in`] for the theme in use, without tracking (layout code).
pub fn syntax_color_untracked(kind: HighlightKind) -> Option<Color> {
    syntax_color_in(&palette_untracked(), kind)
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

/// Editor (text area) colours from the theme in use. Reactive.
pub fn editor_style(style: EditorCustomStyle) -> EditorCustomStyle {
    let p = palette();
    style
        .text_colors(p.fg, p.editor_bg)
        .gutter_dim_color(p.gutter_fg)
        .cursor_color(p.cursor)
        .selection_color(p.selection)
        .current_line_color(p.current_line)
        .visible_whitespace(p.indent_guide)
        .preedit_underline_color(p.fg)
        .indent_guide_color(p.indent_guide)
        .gutter_current_color(p.current_line)
}

// ---- The code font ----------------------------------------------------------------

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
    mono_signal().with_untracked(|n| crate::fonts::family_list(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_has_a_unique_id_and_the_default_is_first() {
        let mut ids: Vec<&str> = THEMES.iter().map(|t| t.id).collect();
        assert_eq!(ids[0], "dark");
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), THEMES.len());
        for want in [
            "dark",
            "light",
            "tokyo-night",
            "darcula",
            "material",
            "nord",
            "palenight",
        ] {
            assert!(by_id(want).is_some(), "{want}");
        }
        assert!(by_id("solarized").is_none());
    }

    #[test]
    fn code_colours_differ_from_the_background_in_every_theme() {
        for t in THEMES {
            let p = &t.palette;
            for (name, c) in [
                ("keyword", p.keyword),
                ("string", p.string),
                ("function", p.function),
                ("comment", p.comment),
                ("fg", p.fg),
                ("error", p.error),
            ] {
                assert_ne!(
                    c, p.editor_bg,
                    "{}: {name} equals the editor background",
                    t.id
                );
            }
            assert_eq!(syntax_color_in(p, HighlightKind::Variable), None);
            assert_eq!(syntax_color_in(p, HighlightKind::Keyword), Some(p.keyword));
        }
    }
}
