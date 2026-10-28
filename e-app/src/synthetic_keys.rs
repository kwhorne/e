//! Keystrokes and clicks for the agent socket's `key` and `click` methods, so
//! an agent — or a test — can use the editor as a person would: press
//! `cmd+shift+p`, `down` or `enter`, type text, click a button or the gutter.
//!
//! On macOS each is a real `NSEvent` (key down then up; mouse moved, down,
//! up) sent to the editor's window, so it takes the same path as a key on the
//! keyboard or a click of the mouse:
//! winit's view, the keyboard layout, IME, Floem's dispatch and `e`'s keymap.
//! Unlike System Events it needs no Accessibility permission, and it reaches
//! the window whether or not it is in front. Keys are named by their place on
//! a US keyboard (`cmd+.`, `cmd+shift+/`); text is typed as given.

/// One key press: the virtual key code, the text it types (with and without
/// its modifiers) and the modifier flags, as AppKit describes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stroke {
    pub code: u16,
    pub chars: String,
    pub unmodified: String,
    pub flags: usize,
}

const SHIFT: usize = 1 << 17;
const CONTROL: usize = 1 << 18;
const OPTION: usize = 1 << 19;
const COMMAND: usize = 1 << 20;

/// Named keys: their virtual key code and the character AppKit gives them.
const NAMED: &[(&str, u16, char)] = &[
    ("enter", 36, '\r'),
    ("return", 36, '\r'),
    ("tab", 48, '\t'),
    ("space", 49, ' '),
    ("backspace", 51, '\u{7f}'),
    ("escape", 53, '\u{1b}'),
    ("esc", 53, '\u{1b}'),
    ("delete", 117, '\u{f728}'),
    ("home", 115, '\u{f729}'),
    ("end", 119, '\u{f72b}'),
    ("pageup", 116, '\u{f72c}'),
    ("pagedown", 121, '\u{f72d}'),
    ("left", 123, '\u{f702}'),
    ("right", 124, '\u{f703}'),
    ("down", 125, '\u{f701}'),
    ("up", 126, '\u{f700}'),
    ("f1", 122, '\u{f704}'),
    ("f2", 120, '\u{f705}'),
    ("f3", 99, '\u{f706}'),
    ("f4", 118, '\u{f707}'),
    ("f5", 96, '\u{f708}'),
    ("f6", 97, '\u{f709}'),
    ("f7", 98, '\u{f70a}'),
    ("f8", 100, '\u{f70b}'),
    ("f9", 101, '\u{f70c}'),
    ("f10", 109, '\u{f70d}'),
    ("f11", 103, '\u{f70e}'),
    ("f12", 111, '\u{f70f}'),
];

/// The US-keyboard key that types `c`, and whether it takes shift.
fn key_for(c: char) -> Option<(u16, bool)> {
    const PLAIN: &str = "asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.\0\0`";
    const SHIFTED: &str = "ASDFHGZXCV\0BQWERYT!@#$^%+(&_*)}OU{IP\0LJ\"K:|<?NM>\0\0~";
    if let Some(i) = PLAIN.chars().position(|p| p == c && p != '\0') {
        return Some((i as u16, false));
    }
    SHIFTED
        .chars()
        .position(|p| p == c && p != '\0')
        .map(|i| (i as u16, true))
}

/// A chord such as `cmd+shift+p`, `down`, `ctrl+tab` or `cmd++`.
pub fn parse_chord(chord: &str) -> Result<Stroke, String> {
    let chord = chord.trim();
    // `cmd++`: the key is `+` itself.
    let (mods, key) = match chord.strip_suffix("++") {
        Some(rest) => (rest, "+"),
        None => match chord.rsplit_once('+') {
            Some((mods, key)) if !key.is_empty() => (mods, key),
            _ => ("", chord),
        },
    };
    let flags = modifier_flags(mods).map_err(|e| format!("{e} in {chord:?}"))?;
    let lower = key.to_ascii_lowercase();
    if let Some((_, code, c)) = NAMED.iter().find(|(n, ..)| *n == lower) {
        return Ok(Stroke {
            code: *code,
            chars: c.to_string(),
            unmodified: c.to_string(),
            flags,
        });
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Err(format!("unknown key {key:?} in {chord:?}"));
    };
    let (code, shifted) = key_for(c).ok_or_else(|| format!("no key types {c:?}"))?;
    // A shifted character names its key with shift held: `cmd+?` is
    // `cmd+shift+/`.
    let flags = if shifted { flags | SHIFT } else { flags };
    let typed = if flags & SHIFT != 0 && c.is_ascii_lowercase() {
        c.to_ascii_uppercase().to_string()
    } else {
        c.to_string()
    };
    // AppKit's "ignoring modifiers" keeps shift.
    Ok(Stroke {
        code,
        unmodified: typed.clone(),
        chars: typed,
        flags,
    })
}

/// The strokes that type `text`. A character no US key types (`æ`, `→`) is
/// still typed — its text is what the editor receives.
pub fn text_strokes(text: &str) -> Vec<Stroke> {
    text.chars()
        .map(|c| {
            let (code, shifted) = match c {
                '\n' => (36, false),
                '\t' => (48, false),
                ' ' => (49, false),
                _ => key_for(c).unwrap_or((0, false)),
            };
            let chars = if c == '\n' {
                "\r".into()
            } else {
                c.to_string()
            };
            Stroke {
                code,
                unmodified: chars.clone(),
                chars,
                flags: if shifted { SHIFT } else { 0 },
            }
        })
        .collect()
}

/// The window to send events to: the one in front, else the first showing;
/// and its number.
#[cfg(target_os = "macos")]
unsafe fn target_window(
    mtm: objc2::MainThreadMarker,
) -> Result<(*mut objc2::runtime::AnyObject, isize), String> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSApplication;

    let app = NSApplication::sharedApplication(mtm);
    let mut window: *mut AnyObject = msg_send![&*app, keyWindow];
    if window.is_null() {
        window = msg_send![&*app, mainWindow];
    }
    if window.is_null() {
        let windows: *mut AnyObject = msg_send![&*app, windows];
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let visible: bool = msg_send![w, isVisible];
            if visible {
                window = w;
                break;
            }
        }
    }
    if window.is_null() {
        return Err("no window to send events to".into());
    }
    let number: isize = msg_send![window, windowNumber];
    Ok((window, number))
}

/// Seconds since boot, the clock AppKit stamps events with.
#[cfg(target_os = "macos")]
unsafe fn uptime() -> f64 {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    let Some(process) = AnyClass::get(c"NSProcessInfo") else {
        return 0.0;
    };
    let info: *mut AnyObject = msg_send![process, processInfo];
    msg_send![info, systemUptime]
}

/// Queue `event` for `window`. Queued on the run loop rather than sent now:
/// this runs inside winit's own event handling, which must return first.
#[cfg(target_os = "macos")]
unsafe fn queue(window: *mut objc2::runtime::AnyObject, event: *mut objc2::runtime::AnyObject) {
    use objc2::{msg_send, sel};
    let _: () = msg_send![window,
        performSelector: sel!(sendEvent:),
        withObject: event,
        afterDelay: 0.0f64];
}

/// Press `strokes` in the editor's window, in order. They arrive once the
/// current event has been handled.
#[cfg(target_os = "macos")]
pub fn press(strokes: &[Stroke]) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSPoint, NSString};

    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let class = AnyClass::get(c"NSEvent").ok_or("no NSEvent")?;
    unsafe {
        let (window, number) = target_window(mtm)?;
        let now = uptime();
        let context: *mut AnyObject = std::ptr::null_mut();
        for s in strokes {
            let chars = NSString::from_str(&s.chars);
            let unmodified = NSString::from_str(&s.unmodified);
            // NSEventTypeKeyDown, then NSEventTypeKeyUp.
            for kind in [10usize, 11] {
                let event: *mut AnyObject = msg_send![class,
                    keyEventWithType: kind,
                    location: NSPoint::new(0.0, 0.0),
                    modifierFlags: s.flags,
                    timestamp: now,
                    windowNumber: number,
                    context: context,
                    characters: &*chars,
                    charactersIgnoringModifiers: &*unmodified,
                    isARepeat: false,
                    keyCode: s.code];
                if event.is_null() {
                    return Err(format!("AppKit made no event for key {}", s.code));
                }
                queue(window, event);
            }
        }
    }
    Ok(())
}

/// Click the left mouse button at `(x, y)` in the editor's window, in points
/// from the top left of its content — where a screenshot of the window,
/// halved on a Retina display, puts it. `modifiers` are held, as in a chord
/// (`"alt"`, `"cmd+shift"`).
#[cfg(target_os = "macos")]
pub fn click(x: f64, y: f64, modifiers: &str) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSPoint, NSRect};

    let flags = modifier_flags(modifiers)?;
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let class = AnyClass::get(c"NSEvent").ok_or("no NSEvent")?;
    unsafe {
        let (window, number) = target_window(mtm)?;
        let view: *mut AnyObject = msg_send![window, contentView];
        let frame: NSRect = msg_send![view, frame];
        // AppKit counts up from the bottom.
        let at = NSPoint::new(x, frame.size.height - y);
        let now = uptime();
        let context: *mut AnyObject = std::ptr::null_mut();
        // Move there, press, release.
        for kind in [5usize, 1, 2] {
            let event: *mut AnyObject = msg_send![class,
                mouseEventWithType: kind,
                location: at,
                modifierFlags: flags,
                timestamp: now,
                windowNumber: number,
                context: context,
                eventNumber: 0isize,
                clickCount: if kind == 5 { 0isize } else { 1isize },
                pressure: if kind == 1 { 1.0f32 } else { 0.0f32 }];
            if event.is_null() {
                return Err("AppKit made no mouse event".into());
            }
            queue(window, event);
        }
    }
    Ok(())
}

/// The flags for modifiers named as in a chord: `"alt"`, `"cmd+shift"`.
fn modifier_flags(modifiers: &str) -> Result<usize, String> {
    let mut flags = 0;
    for m in modifiers
        .split('+')
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        flags |= match m.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" | "⌘" => COMMAND,
            "shift" | "⇧" => SHIFT,
            "alt" | "opt" | "option" | "⌥" => OPTION,
            "ctrl" | "control" | "⌃" => CONTROL,
            other => return Err(format!("unknown modifier {other:?}")),
        };
    }
    Ok(flags)
}

#[cfg(not(target_os = "macos"))]
pub fn press(_strokes: &[Stroke]) -> Result<(), String> {
    Err("pressing keys is only supported on macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn click(_x: f64, _y: f64, _modifiers: &str) -> Result<(), String> {
    Err("clicking is only supported on macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_names_its_modifiers_and_key() {
        let s = parse_chord("cmd+shift+p").unwrap();
        assert_eq!((s.code, s.flags), (35, COMMAND | SHIFT));
        assert_eq!((s.chars.as_str(), s.unmodified.as_str()), ("P", "P"));

        let s = parse_chord("Down").unwrap();
        assert_eq!((s.code, s.flags, s.chars.as_str()), (125, 0, "\u{f701}"));

        assert_eq!(parse_chord("cmd+.").unwrap().code, 47);
        assert_eq!(parse_chord("ctrl+tab").unwrap().flags, CONTROL);
        assert_eq!(parse_chord("⌘+⌥+n").unwrap().flags, COMMAND | OPTION);
    }

    #[test]
    fn a_shifted_character_holds_shift_on_its_key() {
        let s = parse_chord("cmd+?").unwrap();
        assert_eq!((s.code, s.flags), (44, COMMAND | SHIFT));
        let s = parse_chord("cmd++").unwrap();
        assert_eq!(
            (s.code, s.flags, s.chars.as_str()),
            (24, COMMAND | SHIFT, "+")
        );
    }

    #[test]
    fn a_wrong_chord_says_what_is_wrong() {
        assert!(parse_chord("hyper+a").unwrap_err().contains("modifier"));
        assert!(parse_chord("cmd+banana")
            .unwrap_err()
            .contains("unknown key"));
    }

    #[test]
    fn text_is_typed_as_given() {
        let s = text_strokes("Hi æ\n");
        let typed: Vec<&str> = s.iter().map(|s| s.chars.as_str()).collect();
        assert_eq!(typed, ["H", "i", " ", "æ", "\r"]);
        assert_eq!((s[0].code, s[0].flags), (4, SHIFT));
        assert_eq!(s[4].code, 36);
    }
}
