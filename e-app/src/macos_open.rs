//! Files opened from the Finder, the Dock or `open -a e` on macOS.
//!
//! LaunchServices doesn't pass those as arguments: it sends the running (or
//! just-launched) app an "open documents" Apple Event, which AppKit turns into
//! `application:openFiles:` on the application delegate. winit's delegate
//! doesn't implement it, so AppKit fell through to its document machinery and
//! put up "e cannot open files in the “Markdown” format". We add the method to
//! winit's delegate class at runtime; it queues the paths, and the UI thread
//! opens them (a file in the editor, a folder as the project).

use std::ffi::c_char;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationDelegateReply};
use objc2_foundation::{NSArray, NSString};

static QUEUE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WAKE_TX: OnceLock<Sender<()>> = OnceLock::new();
static WAKE_RX: Mutex<Option<Receiver<()>>> = Mutex::new(None);

/// The paths handed to us since the last call.
pub fn take() -> Vec<PathBuf> {
    QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// The wake channel's receiving end, once, for the UI thread's bridge.
pub fn take_wake_rx() -> Option<Receiver<()>> {
    WAKE_RX.lock().ok().and_then(|mut r| r.take())
}

unsafe extern "C-unwind" fn open_files(
    _this: *mut AnyObject,
    _cmd: Sel,
    app: *mut NSApplication,
    files: *mut NSArray<NSString>,
) {
    // SAFETY: AppKit passes a live NSArray<NSString> of absolute paths and the
    // shared NSApplication.
    let files = unsafe { &*files };
    let paths: Vec<PathBuf> = files.iter().map(|s| PathBuf::from(s.to_string())).collect();
    if let Ok(mut q) = QUEUE.lock() {
        q.extend(paths);
    }
    if let Some(tx) = WAKE_TX.get() {
        let _ = tx.send(());
    }
    unsafe { (*app).replyToOpenOrPrint(NSApplicationDelegateReply::Success) };
}

/// Teach the application delegate `application:openFiles:`. Call on the main
/// thread after the event loop (and so the delegate) exists and before it
/// runs, so a document that launched the app is caught too.
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (tx, rx) = channel();
    let _ = WAKE_TX.set(tx);
    if let Ok(mut slot) = WAKE_RX.lock() {
        *slot = Some(rx);
    }
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else {
        eprintln!("e: no application delegate; Finder opens won't reach the editor");
        return;
    };
    // The delegate's class (winit's), asked the Objective-C way: the
    // protocol object doesn't expose `class()` directly.
    let class: *const AnyClass = unsafe { msg_send![&*delegate, class] };
    let imp: unsafe extern "C-unwind" fn() = unsafe {
        std::mem::transmute(
            open_files
                as unsafe extern "C-unwind" fn(
                    *mut AnyObject,
                    Sel,
                    *mut NSApplication,
                    *mut NSArray<NSString>,
                ),
        )
    };
    // "v@:@@": returns void; receiver, selector, the app, the file list.
    let types = c"v@:@@";
    let added = unsafe {
        objc2::ffi::class_addMethod(
            class as *mut AnyClass,
            sel!(application:openFiles:),
            imp,
            types.as_ptr() as *const c_char,
        )
    };
    if !added.as_bool() {
        eprintln!("e: couldn't add application:openFiles: to the delegate");
    }
}
