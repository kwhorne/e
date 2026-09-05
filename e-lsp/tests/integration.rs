//! End-to-end LSP tests against a real language server (clangd).
//! These skip gracefully if clangd isn't installed, so they're safe in CI.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use e_lsp::{path_to_uri, LspClient};

fn clangd_available() -> bool {
    std::process::Command::new("clangd")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn tmp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("e_lsp_it_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn clangd_reports_diagnostics() {
    if !clangd_available() {
        eprintln!("skipping clangd_reports_diagnostics: clangd not installed");
        return;
    }
    let dir = tmp_dir("diag");
    let file = dir.join("main.c");
    // Two errors: bad initializer + missing semicolon.
    let src = "int main() {\n    int x = \"oops\";\n    return x\n}\n";
    std::fs::write(&file, src).unwrap();

    let got: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let g2 = got.clone();
    let client = LspClient::start(
        "clangd",
        &[],
        &dir,
        Box::new(move |ev| {
            let e_lsp::ServerEvent::Diagnostics(p) = ev else {
                return;
            };
            if p.uri.as_str().ends_with("main.c") && !p.diagnostics.is_empty() {
                *g2.lock().unwrap() = p.diagnostics.iter().map(|d| d.message.clone()).collect();
            }
        }),
    )
    .expect("clangd should start");

    client.did_open(&path_to_uri(&file), "c", 1, src);

    // Wait up to ~10s for diagnostics.
    for _ in 0..100 {
        if !got.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let diags = got.lock().unwrap().clone();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!diags.is_empty(), "expected diagnostics from clangd");
    assert!(
        diags
            .iter()
            .any(|m| m.to_lowercase().contains("expected ';'")
                || m.to_lowercase().contains("expected ';' after")),
        "expected a missing-semicolon diagnostic, got: {diags:?}"
    );
}

#[test]
fn clangd_completes_after_member_access() {
    if !clangd_available() {
        eprintln!("skipping clangd_completes_after_member_access: clangd not installed");
        return;
    }
    let dir = tmp_dir("comp");
    let file = dir.join("a.c");
    let src = "#include <string.h>\nint main() {\n    str\n}\n";
    std::fs::write(&file, src).unwrap();

    let client = LspClient::start("clangd", &[], &dir, Box::new(|_| {})).expect("clangd starts");
    let uri = path_to_uri(&file);
    client.did_open(&uri, "c", 1, src);

    // Poll completion until clangd's preamble is ready (indexing takes a moment).
    let mut items = Vec::new();
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(300));
        items = client.completion(&uri, 2, 7).unwrap_or_default();
        if !items.is_empty() {
            break;
        }
    }
    assert!(client.is_ready());
    assert!(client.incremental_sync(), "clangd syncs incrementally");

    // Type one more letter. Only the changed byte goes over the wire; if the
    // range were wrong clangd would see garbage and have no `strlen` to offer.
    let src2 = "#include <string.h>\nint main() {\n    strl\n}\n";
    client.did_change(&uri, 2, src2);
    let mut after = Vec::new();
    for _ in 0..20 {
        after = client.completion(&uri, 2, 8).unwrap_or_default();
        if after.iter().any(|i| i.label.contains("strlen")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        after.iter().any(|i| i.label.contains("strlen")),
        "completion after an incremental change offers strlen: {:?}",
        after.iter().map(|i| &i.label).take(5).collect::<Vec<_>>()
    );

    assert!(!items.is_empty(), "expected completions from clangd");
}

/// The handshake offers UTF-8 columns; clangd takes the offer, so positions in
/// non-ASCII text need no conversion — and a diagnostic on a line with `ø` lands
/// on the right bytes.
#[test]
fn clangd_negotiates_utf8_and_reports_byte_columns() {
    if !clangd_available() {
        eprintln!("skipping clangd_negotiates_utf8_and_reports_byte_columns: clangd not installed");
        return;
    }
    let dir = tmp_dir("enc");
    let file = dir.join("b.c");
    // `ø` is two bytes: the undeclared `x` starts at byte 22, UTF-16 unit 21.
    let src = "int main() { /* Bjørn */ x = 1; }\n";
    std::fs::write(&file, src).unwrap();

    let got = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
    let g2 = got.clone();
    let client = LspClient::start(
        "clangd",
        &[],
        &dir,
        Box::new(move |ev| {
            let e_lsp::ServerEvent::Diagnostics(p) = ev else {
                return;
            };
            if let Some(d) = p.diagnostics.iter().find(|d| d.message.contains('x')) {
                *g2.lock().unwrap() = Some(d.range.start.character);
            }
        }),
    )
    .expect("clangd starts");
    assert!(
        client.wait_ready(Duration::from_secs(15)),
        "clangd initialises"
    );
    assert_eq!(client.position_encoding(), e_lsp::PositionEncoding::Utf8);

    let uri = path_to_uri(&file);
    client.did_open(&uri, "c", 1, src);
    let mut col = None;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(250));
        col = *got.lock().unwrap();
        if col.is_some() {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(col, Some(src.find("x = 1").unwrap() as u32));
}

// ---- Intelephense: the server `e` lives on, and a UTF-16 one --------------------

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

/// Intelephense counts columns in UTF-16; on a line with `ø` before the symbol
/// its column is one less than the byte column the editor draws at. The client
/// must hand us bytes.
#[test]
fn intelephense_reports_diagnostics_in_byte_columns() {
    if !on_path("intelephense") {
        eprintln!("skipping intelephense_reports_diagnostics_in_byte_columns: not installed");
        return;
    }
    let dir = tmp_dir("intelephense-diag");
    let file = dir.join("a.php");
    // `$ukjent` is never assigned; `Bjørn` puts a 2-byte character before it.
    let src = "<?php\n$navn = 'Bjørn'; echo $ukjent;\n";
    std::fs::write(&file, src).unwrap();

    let got = std::sync::Arc::new(std::sync::Mutex::new(None::<(u32, u32)>));
    let g2 = got.clone();
    let client = LspClient::start(
        "intelephense",
        &["--stdio"],
        &dir,
        Box::new(move |ev| {
            let e_lsp::ServerEvent::Diagnostics(p) = ev else {
                return;
            };
            if let Some(d) = p.diagnostics.iter().find(|d| d.message.contains("ukjent")) {
                *g2.lock().unwrap() = Some((d.range.start.line, d.range.start.character));
            }
        }),
    )
    .expect("intelephense starts");
    assert!(
        client.wait_ready(Duration::from_secs(30)),
        "intelephense initialises"
    );
    assert_eq!(client.position_encoding(), e_lsp::PositionEncoding::Utf16);

    let uri = path_to_uri(&file);
    client.did_open(&uri, "php", 1, src);
    let mut at = None;
    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(250));
        at = *got.lock().unwrap();
        if at.is_some() {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let line = src.lines().nth(1).unwrap();
    assert_eq!(at, Some((1, line.find("$ukjent").unwrap() as u32)));
}

/// Completing a class from another namespace: the item arrives without its
/// `use` import, `completionItem/resolve` supplies it — and an incremental
/// `didChange` in between must leave the server with the same text we have.
#[test]
fn intelephense_resolves_the_use_import_after_an_incremental_change() {
    if !on_path("intelephense") {
        eprintln!("skipping intelephense_resolves_the_use_import: not installed");
        return;
    }
    let dir = tmp_dir("intelephense-resolve");
    std::fs::create_dir_all(dir.join("app/Models")).unwrap();
    std::fs::write(
        dir.join("app/Models/Bestilling.php"),
        "<?php\n\nnamespace App\\Models;\n\nclass Bestilling\n{\n}\n",
    )
    .unwrap();
    let file = dir.join("app/Http/Handler.php");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let src = "<?php\n\nnamespace App\\Http;\n\nclass Handler\n{\n    public function run()\n    {\n        $b = new Besti\n    }\n}\n";
    std::fs::write(&file, src).unwrap();

    // Intelephense caches a request's answer until a document event invalidates
    // it, and answers anything asked *during* workspace indexing with nothing —
    // so wait for its indexing progress to finish before asking anything.
    let indexed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ix = indexed.clone();
    let client = LspClient::start(
        "intelephense",
        &["--stdio"],
        &dir,
        Box::new(move |ev| {
            if let e_lsp::ServerEvent::Progress { done: true, .. } = ev {
                ix.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }),
    )
    .expect("intelephense starts");
    assert!(client.wait_ready(Duration::from_secs(30)));
    assert!(
        client.incremental_sync(),
        "intelephense syncs incrementally"
    );
    assert!(client.supports_completion_resolve());
    for _ in 0..120 {
        if indexed.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(
        indexed.load(std::sync::atomic::Ordering::SeqCst),
        "intelephense reports its indexing finished via $/progress"
    );

    let uri = path_to_uri(&file);
    client.did_open(&uri, "php", 1, src);

    // Type one more letter through the incremental path, then complete after it.
    let src2 = src.replace("new Besti", "new Bestil");
    client.did_change(&uri, 2, &src2);
    let line = 8u32;
    let col = src2.lines().nth(8).unwrap().len() as u32;
    let mut item = None;
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..20 {
        let items = client.completion(&uri, line, col).unwrap_or_default();
        seen = items.iter().map(|i| i.label.clone()).take(10).collect();
        item = items.into_iter().find(|i| i.label == "Bestilling");
        if item.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let Some(item) = item else {
        let _ = std::fs::remove_dir_all(&dir);
        panic!("intelephense never offered Bestilling at {line}:{col}; saw {seen:?}");
    };
    // The import comes either attached to the item or from resolving it;
    // `e` handles both, and resolve must work either way.
    let attached = item.additional_text_edits.clone().unwrap_or_default();
    let resolved = client
        .resolve_completion(&uri, &item)
        .expect("resolve succeeds");
    let _ = std::fs::remove_dir_all(&dir);
    let from_resolve = resolved.additional_text_edits.unwrap_or_default();
    let has_import = |edits: &[lsp_types::TextEdit]| {
        edits
            .iter()
            .any(|e| e.new_text.contains("use App\\Models\\Bestilling"))
    };
    assert!(
        has_import(&attached) || has_import(&from_resolve),
        "the completion brings its import: attached {attached:?}, resolved {from_resolve:?}"
    );
}

/// laravel-lsp needs a real Laravel application to boot. Opt in with
/// `E_LARAVEL_PROJECT=/path/to/app`; skipped otherwise.
#[test]
fn laravel_lsp_initialises_on_a_real_project() {
    let Some(root) = std::env::var_os("E_LARAVEL_PROJECT").map(std::path::PathBuf::from) else {
        eprintln!("skipping laravel_lsp_initialises_on_a_real_project: E_LARAVEL_PROJECT not set");
        return;
    };
    if !on_path("laravel-lsp") {
        eprintln!("skipping laravel_lsp_initialises_on_a_real_project: not installed");
        return;
    }
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let ev2 = events.clone();
    let client = LspClient::start(
        "laravel-lsp",
        &[],
        &root,
        Box::new(move |ev| {
            ev2.lock()
                .unwrap()
                .push(format!("{ev:?}").chars().take(120).collect());
        }),
    )
    .expect("laravel-lsp starts");
    let ready = client.wait_ready(Duration::from_secs(60));
    assert!(
        ready,
        "laravel-lsp initialises: {:?}",
        events.lock().unwrap()
    );

    let routes = root.join("routes/web.php");
    let src = std::fs::read_to_string(&routes).expect("routes/web.php");
    let uri = path_to_uri(&routes);
    client.did_open(&uri, "php", 1, &src);
    // Whatever it answers, it must answer: a request that errors means the
    // handshake or document sync is off.
    let last_line = src.lines().count().saturating_sub(1) as u32;
    client
        .completion(&uri, last_line, 0)
        .expect("laravel-lsp answers a completion request");
    eprintln!(
        "laravel-lsp: {:?} columns, incremental={}",
        client.position_encoding(),
        client.incremental_sync()
    );
}
