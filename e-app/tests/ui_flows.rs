//! End-to-end flows through the real app: each test starts `e` on a small
//! project of its own, drives it over its agent socket — keys, commands, the
//! `⌘.` menu — and reads what's on screen back from `ui_state`, the way a
//! person would use it and check it.
//!
//! They open windows and press keys (macOS only, as the socket's `key` method
//! is), so they don't run with the unit tests:
//!
//! ```sh
//! cargo test -p e-app --test ui_flows -- --ignored --test-threads=1
//! ```

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// How long a flow waits for the screen to show what it expects.
const PATIENCE: Duration = Duration::from_secs(15);

/// A running `e` on a project of its own, and the home it lives in.
struct App {
    child: Child,
    home: PathBuf,
    project: PathBuf,
}

impl App {
    /// Start `e` on a fresh copy of the fixture project, with `config` as its
    /// settings, and wait until its socket answers.
    fn launch(config: Value) -> App {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        // Short: the socket's path has to fit a Unix socket address.
        let home = PathBuf::from(format!(
            "/tmp/e-ui-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".config/e")).unwrap();
        std::fs::write(home.join(".config/e/config.json"), config.to_string()).unwrap();
        let project = home.join("shop");
        fixture(&project);
        let project = project.canonicalize().unwrap();
        let log = std::fs::File::create(home.join("e.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_e"))
            .arg(&project)
            .env("HOME", &home)
            .env("E_DETACHED", "1")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start e");
        let app = App {
            child,
            home,
            project,
        };
        let start = Instant::now();
        while app.socket().is_none() {
            assert!(start.elapsed() < PATIENCE, "e never opened its socket");
            std::thread::sleep(Duration::from_millis(100));
        }
        // The socket answers before the window is up; a key needs the window.
        while app.call(json!({"method": "key", "keys": ["escape"]}))["ok"] != true {
            assert!(start.elapsed() < PATIENCE, "e never showed its window");
            std::thread::sleep(Duration::from_millis(100));
        }
        app
    }

    fn socket(&self) -> Option<PathBuf> {
        std::fs::read_dir(self.home.join(".config/e"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("agent-"))
                    && p.extension().is_some_and(|x| x == "sock")
            })
    }

    /// One request over the socket, and its answer.
    fn call(&self, request: Value) -> Value {
        let path = self.socket().expect("socket");
        let mut stream = UnixStream::connect(path).expect("connect");
        stream.set_read_timeout(Some(PATIENCE)).unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line}"))
    }

    fn state(&self) -> Value {
        self.call(json!({"method": "ui_state"}))
    }

    fn keys(&self, keys: &[&str]) {
        let r = self.call(json!({"method": "key", "keys": keys}));
        assert_eq!(r["ok"], true, "keys {keys:?}: {r}");
    }

    fn type_text(&self, text: &str) {
        let r = self.call(json!({"method": "key", "text": text}));
        assert_eq!(r["ok"], true, "typing {text:?}: {r}");
    }

    fn command(&self, id: &str) {
        let r = self.call(json!({"method": "command", "id": id}));
        assert_eq!(r["ok"], true, "command {id}: {r}");
    }

    /// Open `name` with ⌘P.
    fn open(&self, name: &str) {
        self.keys(&["cmd+p"]);
        self.type_text(name);
        self.wait_for("the file in the palette", |_| true);
        std::thread::sleep(Duration::from_millis(300));
        self.keys(&["enter"]);
        let file = name.to_string();
        self.wait_for(&format!("{name} open"), move |s| {
            s["editor"]["path"]
                .as_str()
                .is_some_and(|p| p.ends_with(&file))
        });
    }

    /// Put the caret on `line` (1-based) of the open file, at its start.
    fn caret_to(&self, line: usize) {
        let mut keys = vec!["cmd+up"];
        keys.extend(std::iter::repeat_n("down", line - 1));
        self.keys(&keys);
    }

    /// Poll `ui_state` until `ready` says yes; fail with what was on screen.
    fn wait_for(&self, what: &str, ready: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            let state = self.state();
            if ready(&state) {
                return state;
            }
            if start.elapsed() > PATIENCE {
                panic!("waited for {what}; the screen said:\n{state:#}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn file(&self, rel: &str) -> PathBuf {
        self.project.join(rel)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// A small Laravel project: a model, a controller, two tests that use them,
/// a stand-in for PHPUnit that logs how it was called and reports one
/// failure, and an empty log. Committed, so the gutter has a base.
fn fixture(root: &Path) {
    std::fs::create_dir_all(root.join("database")).unwrap();
    let files: &[(&str, &str)] = &[
        // `php artisan test` is what `e` runs here: hand it to the stand-in.
        // `queue:retry` takes the jobs out of `failed_jobs`, as the real
        // one does once they're back on the queue.
        (
            "artisan",
            "<?php\nif (($argv[1] ?? '') === 'queue:retry') {\n    foreach (array_slice($argv, 2) as $id) {\n        if ($id[0] !== '-') {\n            exec('sqlite3 ' . __DIR__ . '/database/database.sqlite ' . escapeshellarg(\"delete from failed_jobs where uuid = '$id'\"));\n        }\n    }\n    exit(0);\n}\n$args = array_map('escapeshellarg', array_slice($argv, 2));\npassthru(__DIR__ . '/vendor/bin/phpunit ' . implode(' ', $args), $code);\nexit($code);\n",
        ),
        (".env", "APP_NAME=Laravel\nDB_CONNECTION=sqlite\n"),
        (
            "app/Jobs/SyncOrders.php",
            "<?php\n\nnamespace App\\Jobs;\n\nclass SyncOrders implements ShouldQueue\n{\n    public function __construct(public Order $order) {}\n\n    public function handle(): void\n    {\n    }\n}\n",
        ),
        ("composer.json", r#"{"autoload": {"psr-4": {"App\\": "app/"}}}"#),
        ("storage/logs/laravel.log", ""),
        (
            "app/Models/Order.php",
            "<?php\n\nnamespace App\\Models;\n\nclass Order\n{\n    public function total(): int\n    {\n        return 1;\n    }\n}\n",
        ),
        (
            "app/Http/Controllers/OrderController.php",
            "<?php\n\nnamespace App\\Http\\Controllers;\n\nclass OrderController extends Controller\n{\n    public function index()\n    {\n        return view('orders.index', ['orders' => $orders]);\n    }\n}\n",
        ),
        (
            "app/Services/Pay.php",
            "<?php\n\nnamespace App\\Services;\n\nclass Pay\n{\n    public function secret(): string\n    {\n        return env('STRIPE_WEBHOOK_SECRET');\n    }\n}\n",
        ),
        (
            "config/services.php",
            "<?php\n\nreturn [\n\n    'postmark' => [\n        'token' => env('POSTMARK_TOKEN'),\n    ],\n\n];\n",
        ),
        (".env.example", "APP_NAME=Laravel\nPOSTMARK_TOKEN=\n"),
        (
            "resources/views/orders/index.blade.php",
            "<ul>\n@foreach ($orders as $order)\n    <li>{{ $order->id }}</li>\n@endforeach\n</ul>\n",
        ),
        (
            "tests/Feature/OrderControllerTest.php",
            "<?php\n\nnamespace Tests\\Feature;\n\nuse App\\Http\\Controllers\\OrderController;\n\nclass OrderControllerTest extends TestCase\n{\n}\n",
        ),
        (
            "tests/Unit/OrderTest.php",
            "<?php\n\nnamespace Tests\\Unit;\n\nclass OrderTest extends TestCase\n{\n    public function test_totals(): void\n    {\n    }\n}\n",
        ),
        (
            "tests/Feature/CheckoutTest.php",
            "<?php\n\nnamespace Tests\\Feature;\n\nuse App\\Models\\Order;\n\nclass CheckoutTest extends TestCase\n{\n    public function test_checks_out(): void\n    {\n    }\n}\n",
        ),
        (
            "vendor/bin/phpunit",
            r#"#!/bin/sh
echo "$@" >> "$(dirname "$0")/../../phpunit.log"
while [ $# -gt 0 ]; do case "$1" in --log-junit) out="$2"; shift;; esac; shift; done
cat > "$out" <<X
<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
<testsuite name="Tests\Unit\OrderTest" file="$PWD/tests/Unit/OrderTest.php">
<testcase name="test_totals" class="Tests\Unit\OrderTest" file="$PWD/tests/Unit/OrderTest.php" line="7" time="0.01"/>
</testsuite>
<testsuite name="Tests\Feature\CheckoutTest" file="$PWD/tests/Feature/CheckoutTest.php">
<testcase name="test_checks_out" class="Tests\Feature\CheckoutTest" file="$PWD/tests/Feature/CheckoutTest.php" line="9" time="0.02"><failure>Failed asserting that 1 is 2.</failure></testcase>
</testsuite>
</testsuites>
X
exit 1
"#,
        ),
    ];
    for (rel, text) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let made = Command::new("sqlite3")
        .arg(root.join("database/database.sqlite"))
        .arg("create table failed_jobs (id integer primary key autoincrement, uuid varchar unique, connection text, queue text, payload text, exception text, failed_at datetime default current_timestamp);")
        .status()
        .is_ok_and(|s| s.success());
    assert!(made, "sqlite3");
    use std::os::unix::fs::PermissionsExt;
    let phpunit = root.join("vendor/bin/phpunit");
    std::fs::set_permissions(&phpunit, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join(".gitignore"), "vendor/\nstorage/\nphpunit.log\n").unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "-A"],
        &[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=Test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "init",
        ],
    ] {
        let ok = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "git {args:?}");
    }
}

fn lines_of(state: &Value, key: &str) -> Vec<u64> {
    state["editor"][key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_u64().or_else(|| v[0].as_u64()))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn typing_marks_the_file_and_undo_unmarks_it() {
    let app = App::launch(json!({}));
    app.open("Order.php");
    app.caret_to(9);
    app.keys(&["cmd+right"]);
    app.type_text(" // one");
    let s = app.wait_for("the edit", |s| s["editor"]["dirty"] == true);
    assert_eq!(lines_of(&s, "changes"), [9], "the changed line is marked");
    app.keys(&["cmd+z"]);
    let s = app.wait_for("undo back to the saved text", |s| {
        s["editor"]["dirty"] == false
    });
    assert!(lines_of(&s, "changes").is_empty());
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn saving_writes_the_file() {
    let app = App::launch(json!({"format_on_save": false}));
    app.open("Order.php");
    app.caret_to(9);
    app.keys(&["cmd+right"]);
    app.type_text(" // saved");
    app.wait_for("the edit", |s| s["editor"]["dirty"] == true);
    app.keys(&["cmd+s"]);
    app.wait_for("the save", |s| s["editor"]["dirty"] == false);
    let text = std::fs::read_to_string(app.file("app/Models/Order.php")).unwrap();
    assert!(text.contains("return 1; // saved"), "{text}");
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn a_breakpoint_stays_with_its_line() {
    let app = App::launch(json!({}));
    app.open("Order.php");
    app.caret_to(9);
    app.command("debug-toggle-breakpoint");
    app.wait_for("the breakpoint", |s| lines_of(s, "breakpoints") == [9]);
    app.caret_to(2);
    app.type_text("\n\n");
    app.wait_for("the breakpoint two lines down", |s| {
        lines_of(s, "breakpoints") == [11]
    });
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn a_change_is_peeked_staged_and_left_on_disk() {
    let app = App::launch(json!({"format_on_save": false}));
    app.open("Order.php");
    app.caret_to(9);
    app.keys(&["cmd+right"]);
    app.type_text(" // staged");
    app.keys(&["cmd+s"]);
    app.wait_for("the save", |s| s["editor"]["dirty"] == false);
    app.command("git-peek-change");
    let s = app.wait_for("the peek", |s| !s["hunk_peek"].is_null());
    assert_eq!(s["hunk_peek"]["old_lines"], json!(["        return 1;"]));
    app.command("git-stage-change");
    app.wait_for("the change staged", |s| s["hunk_peek"]["staged"] == true);
    let staged = Command::new("git")
        .arg("-C")
        .arg(&app.project)
        .args(["diff", "--cached", "--name-only"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&staged.stdout).trim(),
        "app/Models/Order.php"
    );

    // Peeked again, it knows; and Unstage takes it back out.
    app.keys(&["escape"]);
    app.wait_for("the peek closed", |s| s["hunk_peek"].is_null());
    app.command("git-peek-change");
    app.wait_for("the peek seeing it staged", |s| {
        s["hunk_peek"]["staged"] == true
    });
    app.command("git-unstage-change");
    app.wait_for("the change unstaged", |s| s["hunk_peek"]["staged"] == false);
    let staged = Command::new("git")
        .arg("-C")
        .arg(&app.project)
        .args(["diff", "--cached", "--name-only"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&staged.stdout).trim(), "");
    assert!(std::fs::read_to_string(app.file("app/Models/Order.php"))
        .unwrap()
        .contains("return 1; // staged"));
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn a_logged_exception_lands_on_the_line_that_caused_it() {
    let app = App::launch(json!({}));
    app.open("OrderController.php");
    // Read once, so the entry below is new to `e`.
    std::thread::sleep(Duration::from_secs(3));
    let controller = app.file("app/Http/Controllers/OrderController.php");
    let entry = format!(
        "[2026-10-28 20:15:03] local.ERROR: Undefined variable $orders {{\"exception\":\"[object] (ErrorException(code: 0): Undefined variable $orders at {}:9)\n[stacktrace]\n#0 {{main}}\n\"}} \n",
        controller.display()
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(app.file("storage/logs/laravel.log"))
        .unwrap()
        .write_all(entry.as_bytes())
        .unwrap();
    let s = app.wait_for("the exception on line 9", |s| {
        lines_of(s, "exceptions") == [9]
    });
    assert!(
        s["editor"]["exceptions"][0][1]
            .as_str()
            .unwrap()
            .contains("ErrorException: Undefined variable $orders"),
        "{s:#}"
    );
    app.caret_to(9);
    let actions = app.call(json!({"method": "code_actions"}));
    let titles = actions.to_string();
    assert!(
        titles.contains("Exception: ask the agent to fix"),
        "{titles}"
    );
    app.keys(&["escape"]);
    app.keys(&["cmd+s"]);
    app.wait_for("the exception cleared by the save", |s| {
        lines_of(s, "exceptions").is_empty()
    });
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn saving_a_model_runs_the_tests_that_use_it() {
    let app = App::launch(json!({"test_on_save": true, "format_on_save": false}));
    app.open("Order.php");
    app.caret_to(9);
    app.keys(&["cmd+right"]);
    app.type_text(" // run");
    app.keys(&["cmd+s"]);
    let s = app.wait_for("the tests run on save", |s| {
        s["tests_on_save"]["failed"].is_u64()
    });
    assert_eq!(s["tests_on_save"], json!({"passed": 1, "failed": 1}));
    let calls = std::fs::read_to_string(app.file("phpunit.log")).unwrap();
    assert!(
        calls.contains("--filter=CheckoutTest|OrderTest"),
        "only the tests that use Order: {calls}"
    );
    app.open("CheckoutTest.php");
    app.wait_for("the failed test marked", |s| {
        s["editor"]["tests"] == json!([[9, "failed"]])
    });
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn saving_a_view_runs_the_tests_of_its_controller() {
    let app = App::launch(json!({"test_on_save": true, "format_on_save": false}));
    app.open("index.blade.php");
    app.caret_to(1);
    app.type_text("Orders");
    app.keys(&["cmd+s"]);
    app.wait_for("the tests run on save", |s| {
        s["tests_on_save"]["failed"].is_u64()
    });
    let calls = std::fs::read_to_string(app.file("phpunit.log")).unwrap();
    assert!(
        calls.contains("tests/Feature/OrderControllerTest.php"),
        "the test of the controller that renders it: {calls}"
    );
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn env_outside_config_is_moved_into_it() {
    let app = App::launch(json!({"format_on_save": false}));
    app.open("Pay.php");
    let flagged = |app: &App| {
        app.call(json!({"method": "diagnostics"}))
            .to_string()
            .contains("env('STRIPE_WEBHOOK_SECRET') is null once the config is cached")
    };
    let start = Instant::now();
    while !flagged(&app) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no env() warning: {}",
            app.call(json!({"method": "diagnostics"}))
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    // On the call: before the `;` that ends the line.
    app.caret_to(9);
    app.keys(&["cmd+right", "left"]);
    let actions = app.call(json!({"method": "code_actions"}));
    assert!(
        actions.to_string().contains(
            "Move env('STRIPE_WEBHOOK_SECRET') to config/services.php and read config('services.stripe.webhook_secret')"
        ),
        "{actions}"
    );
    let applied = app
        .call(json!({"method": "apply_code_action", "title": "Move env('STRIPE_WEBHOOK_SECRET')"}));
    assert_eq!(applied["ok"], true, "{applied}");
    app.keys(&["cmd+s"]);
    app.wait_for("the save", |s| s["editor"]["dirty"] == false);

    let read = |rel: &str| std::fs::read_to_string(app.file(rel)).unwrap();
    let start = Instant::now();
    while !read("config/services.php").contains("'stripe'") {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{}",
            read("config/services.php")
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        read("app/Services/Pay.php").contains("return config('services.stripe.webhook_secret');"),
        "{}",
        read("app/Services/Pay.php")
    );
    assert!(
        read("config/services.php").contains(
            "    'stripe' => [\n        'webhook_secret' => env('STRIPE_WEBHOOK_SECRET'),\n    ],"
        ),
        "{}",
        read("config/services.php")
    );
    assert!(
        read(".env.example").ends_with("POSTMARK_TOKEN=\nSTRIPE_WEBHOOK_SECRET=\n"),
        "{:?}",
        read(".env.example")
    );
    let start = Instant::now();
    while flagged(&app) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the warning stays"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "opens a window; run with --ignored"]
fn a_failed_job_shows_on_its_handle_and_is_retried() {
    let app = App::launch(json!({}));
    app.open("SyncOrders.php");
    let payload = r#"{"uuid":"u-1","displayName":"App\\Jobs\\SyncOrders","data":{"commandName":"App\\Jobs\\SyncOrders","command":"O:19:\"App\\Jobs\\SyncOrders\":1:{s:5:\"order\";O:45:\"Illuminate\\Contracts\\Database\\ModelIdentifier\":2:{s:5:\"class\";s:16:\"App\\Models\\Order\";s:2:\"id\";i:14;}}"}}"#;
    let exception = "RuntimeException: The warehouse said no in /var/www/html/app/Jobs/SyncOrders.php:11\nStack trace:\n#0 {main}";
    let sql = format!(
        "insert into failed_jobs (uuid, connection, queue, payload, exception, failed_at) values ('u-1', 'database', 'default', '{}', '{}', '2026-10-29 14:02:11');",
        payload.replace('\'', "''"),
        exception.replace('\'', "''"),
    );
    let ok = Command::new("sqlite3")
        .arg(app.file("database/database.sqlite"))
        .arg(&sql)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "insert");

    let s = app.wait_for("the failed job on handle()", |s| {
        lines_of(s, "exceptions") == [9]
    });
    assert_eq!(
        s["editor"]["exceptions"][0][1],
        "✖ Failed job SyncOrders: RuntimeException: The warehouse said no · 14:02:11"
    );
    app.caret_to(9);
    let actions = app.call(json!({"method": "code_actions"})).to_string();
    assert!(
        actions.contains("Failed job: retry SyncOrders (queue:retry)"),
        "{actions}"
    );
    assert!(
        actions.contains("Failed job: ask the agent to fix"),
        "{actions}"
    );
    let applied = app.call(json!({"method": "apply_code_action", "title": "Failed job: retry"}));
    assert_eq!(applied["ok"], true, "{applied}");
    app.wait_for("the job gone once retried", |s| {
        lines_of(s, "exceptions").is_empty()
    });
}
