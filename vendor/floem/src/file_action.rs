use std::path::PathBuf;

use floem_reactive::Scope;

use crate::{
    ext_event::create_ext_action,
    file::{FileDialogOptions, FileInfo},
};

static ON_UNAVAILABLE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Call `f` when a file dialog can't be opened (the action gets `None`, as
/// for a cancel), so the app can say so.
pub fn on_dialog_unavailable(f: fn()) {
    let _ = ON_UNAVAILABLE.set(f);
}

fn unavailable() {
    if let Some(f) = ON_UNAVAILABLE.get() {
        f();
    }
}

/// Whether AppKit can make the panel now. `rfd` panics when `openPanel` or
/// `savePanel` returns nil — which it does when the system's panel service
/// fails to launch — and that panic, inside a main-queue callback, aborts the
/// app. Asked here, on the main thread, first.
#[cfg(target_os = "macos")]
fn panel_available(save: bool) -> bool {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    if objc2::MainThreadMarker::new().is_none() {
        return true;
    }
    let Some(class) = AnyClass::get(if save { c"NSSavePanel" } else { c"NSOpenPanel" }) else {
        return false;
    };
    let panel: Option<Retained<AnyObject>> = unsafe {
        if save {
            objc2::msg_send![class, savePanel]
        } else {
            objc2::msg_send![class, openPanel]
        }
    };
    if panel.is_none() {
        eprintln!("floem: the system file dialog could not be opened");
    }
    panel.is_some()
}

#[cfg(not(target_os = "macos"))]
fn panel_available(_save: bool) -> bool {
    true
}

/// Open a file using the system file dialog
pub fn open_file(
    options: FileDialogOptions,
    file_info_action: impl Fn(Option<FileInfo>) + 'static,
) {
    if !panel_available(false) {
        unavailable();
        file_info_action(None);
        return;
    }
    let send = create_ext_action(
        Scope::new(),
        move |(path, paths): (Option<PathBuf>, Option<Vec<PathBuf>>)| {
            if paths.is_some() {
                file_info_action(paths.map(|paths| FileInfo {
                    path: paths,
                    format: None,
                }))
            } else {
                file_info_action(path.map(|path| FileInfo {
                    path: vec![path],
                    format: None,
                }))
            }
        },
    );
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(path) = options.starting_directory.as_ref() {
            dialog = dialog.set_directory(path);
        }
        if let Some(title) = options.title.as_ref() {
            dialog = dialog.set_title(title);
        }
        if let Some(allowed_types) = options.allowed_types.as_ref() {
            dialog = allowed_types.iter().fold(dialog, |dialog, filter| {
                dialog.add_filter(filter.name, filter.extensions)
            });
        }

        if options.select_directories && options.multi_selection {
            send((None, dialog.pick_folders()));
        } else if options.select_directories && !options.multi_selection {
            send((dialog.pick_folder(), None));
        } else if !options.select_directories && options.multi_selection {
            send((None, dialog.pick_files()));
        } else {
            send((dialog.pick_file(), None));
        }
    });
}

/// Open a system file save dialog
pub fn save_as(options: FileDialogOptions, file_info_action: impl Fn(Option<FileInfo>) + 'static) {
    if !panel_available(true) {
        unavailable();
        file_info_action(None);
        return;
    }
    let send = create_ext_action(Scope::new(), move |path: Option<PathBuf>| {
        file_info_action(path.map(|path| FileInfo {
            path: vec![path],
            format: None,
        }))
    });
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(path) = options.starting_directory.as_ref() {
            dialog = dialog.set_directory(path);
        }
        if let Some(name) = options.default_name.as_ref() {
            dialog = dialog.set_file_name(name);
        }
        if let Some(title) = options.title.as_ref() {
            dialog = dialog.set_title(title);
        }
        let path = dialog.save_file();
        send(path);
    });
}
