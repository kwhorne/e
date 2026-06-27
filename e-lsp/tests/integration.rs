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
        Box::new(move |p| {
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
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!items.is_empty(), "expected completions from clangd");
}
