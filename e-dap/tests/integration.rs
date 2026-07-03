//! End-to-end DAP flow against the bundled `e-dap-mock` adapter. Deterministic
//! and dependency-free, so it always runs in CI.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use e_dap::DapClient;

#[test]
fn full_initialize_inspect_disconnect_flow() {
    let adapter = env!("CARGO_BIN_EXE_e-dap-mock");

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let client = DapClient::start(
        adapter,
        &[],
        Box::new(move |ev| sink.lock().unwrap().push(ev.event)),
    )
    .expect("mock adapter should start");

    // initialize → capabilities come back, `initialized` event fires.
    let caps = client.initialize("mock").expect("initialize");
    assert_eq!(caps["supportsConfigurationDoneRequest"], true);

    let mut saw_initialized = false;
    for _ in 0..50 {
        if events.lock().unwrap().iter().any(|e| e == "initialized") {
            saw_initialized = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(saw_initialized, "expected an `initialized` event");

    // Breakpoints are verified by the adapter.
    let bps = client
        .set_breakpoints("/tmp/app.php", &[3, 7])
        .expect("setBreakpoints");
    assert_eq!(bps.len(), 2);
    assert!(bps.iter().all(|b| b.verified));
    assert_eq!(bps[0].line, Some(3));

    // Threads + stack trace decode into typed structs.
    let threads = client.threads().expect("threads");
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].name, "main");

    let frames = client.stack_trace(threads[0].id).expect("stackTrace");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].source_path.as_deref(), Some("/tmp/app.php"));
    assert_eq!(frames[0].line, 3);

    client.disconnect(true);
}
