//! Evaluating a PHP expression where it is written. Put the caret on
//! `Order::whereBelongsTo($user)->withSum('items', 'price')->first()` — or
//! select it — press `⌘⏎`, and the value shows at the end of the line, with
//! the queries it ran: no Tinker session, no `dd()` left behind.
//!
//! The expression runs in the booted app, as the file would see it: in its
//! namespace, with its `use` imports. Writes are rolled back — it runs in a
//! transaction on the default connection — and mail, notifications and
//! queued jobs are faked, so evaluating `$order->ship()` sends nothing.

use std::ops::Range;
use std::path::Path;

/// What evaluating an expression gave.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluated {
    /// The value, on one line: `App\Models\Order #14 {id: 14, total: 990}`.
    pub value: String,
    /// The value in full, for the panel: pretty JSON, or the error's trace.
    pub full: String,
    /// `None` when it ran; the exception when it threw.
    pub error: Option<String>,
    /// The queries it ran, with their bindings in place, and their time.
    pub queries: Vec<(String, f64)>,
    /// How long it took.
    pub ms: f64,
    /// What it printed, if anything.
    pub printed: String,
}

impl Evaluated {
    /// What the end of the line says.
    pub fn label(&self) -> String {
        let mut out = match &self.error {
            Some(e) => format!("✖ {e}"),
            None => format!("⇒ {}", self.value),
        };
        if out.chars().count() > 160 {
            out = out.chars().take(159).collect::<String>() + "…";
        }
        match self.queries.len() {
            0 => {}
            1 => out.push_str(" · 1 query"),
            n => out.push_str(&format!(" · {n} queries")),
        }
        out.push_str(&format!(" · {}", ms_text(self.ms)));
        out
    }

    /// Everything, for the Tinker panel: the value, then the queries.
    pub fn report(&self, code: &str) -> String {
        let mut out = format!(">>> {}\n", code.trim());
        if !self.printed.is_empty() {
            out.push_str(&self.printed);
            if !self.printed.ends_with('\n') {
                out.push('\n');
            }
        }
        match &self.error {
            Some(e) => out.push_str(&format!("✖ {e}\n{}\n", self.full)),
            None => out.push_str(&format!("{}\n", self.full)),
        }
        if !self.queries.is_empty() {
            out.push_str(&format!("\n{} queries:\n", self.queries.len()));
            for (sql, ms) in &self.queries {
                out.push_str(&format!("  {sql}   ({})\n", ms_text(*ms)));
            }
        }
        out.push_str(&format!(
            "\n{} · writes rolled back, mail, notifications and queued jobs faked\n",
            ms_text(self.ms)
        ));
        out
    }
}

fn ms_text(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

/// The statement around byte `offset` of `text`, a PHP file: from after the
/// `;`, `{` or `}` before it to the `;` that ends it — across lines, past
/// closures in its arguments. `None` outside one.
pub fn statement_at(text: &str, offset: usize) -> Option<Range<usize>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Frame {
        Paren,
        Brace,
    }
    let b = text.as_bytes();
    let offset = offset.min(b.len());
    // Each frame, and where the statement in it began.
    let mut stack: Vec<(Frame, usize)> = vec![(Frame::Brace, 0)];
    let mut i = 0;
    let mut found: Option<(usize, usize)> = None; // (depth, start)
    while i < b.len() {
        if i >= offset && found.is_none() {
            let depth = stack.iter().rposition(|(f, _)| *f == Frame::Brace)?;
            found = Some((depth, stack[depth].1));
        }
        match b[i] {
            q @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'#' if b.get(i + 1) != Some(&b'[') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'(' | b'[' => stack.push((Frame::Paren, 0)),
            b')' | b']' => {
                if stack.len() > 1 && stack.last().is_some_and(|(f, _)| *f == Frame::Paren) {
                    stack.pop();
                }
            }
            b'{' => stack.push((Frame::Brace, i + 1)),
            b'}' => {
                if let Some((depth, start)) = found {
                    // The block the statement is in ends: it had no `;`.
                    if stack.len() - 1 == depth {
                        return trimmed(text, start..i);
                    }
                }
                if stack.len() > 1 {
                    stack.pop();
                }
                if let Some(top) = stack.last_mut().filter(|(f, _)| *f == Frame::Brace) {
                    top.1 = i + 1;
                }
            }
            b';' => {
                let top = stack.len() - 1;
                if let Some((depth, start)) = found {
                    if top == depth {
                        return trimmed(text, start..i);
                    }
                }
                if stack[top].0 == Frame::Brace {
                    stack[top].1 = i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let (_, start) = found.or_else(|| {
        let depth = stack.iter().rposition(|(f, _)| *f == Frame::Brace)?;
        Some((depth, stack[depth].1))
    })?;
    trimmed(text, start..b.len())
}

/// `range` of `text` without the whitespace and comments around it; `None`
/// when nothing is left.
fn trimmed(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    let mut start = range.start;
    let end = range.end;
    loop {
        let rest = &text[start..end];
        let skipped = rest.len() - rest.trim_start().len();
        start += skipped;
        let rest = &text[start..end];
        if rest.starts_with("//") || (rest.starts_with('#') && !rest.starts_with("#[")) {
            start += rest.find('\n').map_or(rest.len(), |n| n + 1);
        } else if rest.starts_with("/*") {
            start += rest.find("*/").map_or(rest.len(), |n| n + 2);
        } else if rest.starts_with("<?php") {
            start += 5;
        } else {
            break;
        }
    }
    let end = start + text[start..end].trim_end().len();
    (start < end).then_some(start..end)
}

/// Why `code` can't be evaluated as it is, or the code to evaluate: a
/// statement's value is its expression's — `return`, `echo` and a trailing
/// `;` are dropped, an assignment is kept and gives what it assigns.
pub fn expression(code: &str) -> Result<String, String> {
    let mut code = code.trim().trim_end_matches(';').trim();
    for keyword in ["return ", "echo ", "yield "] {
        if let Some(rest) = code.strip_prefix(keyword) {
            code = rest.trim();
        }
    }
    let first = code
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("");
    const DECLARATIONS: &[&str] = &[
        "namespace",
        "use",
        "class",
        "interface",
        "trait",
        "enum",
        "function",
        "public",
        "protected",
        "private",
        "abstract",
        "final",
        "readonly",
        "if",
        "else",
        "elseif",
        "foreach",
        "for",
        "while",
        "do",
        "switch",
        "match",
        "try",
        "catch",
        "finally",
    ];
    if code.is_empty() {
        return Err("Nothing to evaluate here".to_string());
    }
    // `match (…) {…}` and `function () {…}` are expressions; a bare `if` or
    // a method declaration isn't.
    let callable = code.starts_with("function") && code[8..].trim_start().starts_with('(');
    if DECLARATIONS.contains(&first) && first != "match" && !callable {
        return Err(format!(
            "`{first}` starts a declaration or a block, not a value — select the expression to evaluate"
        ));
    }
    Ok(code.to_string())
}

/// The `namespace` and top-level `use` lines of `file`, the PHP file the
/// code comes from, so its names resolve as they do there.
pub fn context_of(file: &str) -> String {
    let mut out = String::new();
    for line in file.lines() {
        // Imports end where the first declaration starts.
        let declares = [
            "class ",
            "final ",
            "abstract ",
            "readonly ",
            "interface ",
            "trait ",
            "enum ",
            "function ",
        ]
        .iter()
        .any(|k| line.starts_with(k));
        if declares {
            break;
        }
        if line.starts_with("namespace ") || line.starts_with("use ") {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    // A group `use` over several lines: take it whole.
    if out.lines().any(|l| l.contains('{') && !l.contains('}')) {
        out.clear();
        let mut in_group = false;
        for line in file.lines() {
            let declares = [
                "class ",
                "final ",
                "abstract ",
                "interface ",
                "trait ",
                "enum ",
            ]
            .iter()
            .any(|k| line.starts_with(k));
            if declares {
                break;
            }
            if line.starts_with("namespace ") || line.starts_with("use ") || in_group {
                out.push_str(line.trim_end());
                out.push('\n');
                in_group = (in_group || line.contains('{')) && !line.contains('}');
            }
        }
    }
    out
}

/// The marker the runner's answer line starts with.
const MARK: &str = "__E_EVAL__";

/// The PHP that boots the app at `root` and evaluates `code` in `context`
/// (a file's namespace and imports), answering with one JSON line.
pub fn runner(root: &Path, context: &str, code: &str) -> String {
    let quoted = |p: &Path| {
        format!(
            "'{}'",
            p.to_string_lossy()
                .replace('\\', "\\\\")
                .replace('\'', "\\'")
        )
    };
    // The code goes in a nowdoc, so it is never parsed as part of the runner:
    // a typo in it is a ParseError the runner catches.
    let mut tag = String::from("E_EVAL_CODE");
    while code.contains(&tag) || context.contains(&tag) {
        tag.push('_');
    }
    let source = format!("{context}\nreturn ({code});");
    RUNNER
        .replace("{autoload}", &quoted(&root.join("vendor/autoload.php")))
        .replace("{bootstrap}", &quoted(&root.join("bootstrap/app.php")))
        .replace("{mark}", MARK)
        .replace("{source}", &format!("<<<'{tag}'\n{source}\n{tag}"))
}

const RUNNER: &str = r#"<?php
require {autoload};
$__eApp = require {bootstrap};
$__eApp->make(\Illuminate\Contracts\Console\Kernel::class)->bootstrap();
error_reporting(E_ALL & ~E_DEPRECATED & ~E_USER_DEPRECATED);
set_error_handler(function ($no, $message, $file, $line) {
    if (!(error_reporting() & $no)) {
        return false;
    }
    throw new \ErrorException($message, 0, $no, $file, $line);
});

// What the app booted with, not what `.env` says: a cached config ignores
// it. Fails closed — a guard that can't tell says no.
$__eRefused = (function () use ($__eApp) {
    try {
        if ($__eApp->environment('production')) {
            return 'the app runs as production';
        }
        $connection = config('database.connections.' . config('database.default')) ?? [];
        $database = (string) ($connection['database'] ?? '');
        $named = preg_match('/(^|[^a-z])(prod|production|live)([^a-z]|$)/i', basename($database));
        if (($connection['driver'] ?? '') === 'sqlite') {
            return $named ? "its database $database looks like production" : null;
        }
        $host = $connection['host'] ?? '';
        $host = strtolower(trim((string) (is_array($host) ? reset($host) : $host)));
        if (!in_array($host, ['', 'localhost', '127.0.0.1', '::1', '0.0.0.0', 'host.docker.internal'], true)) {
            return "its database is on $host, not this machine";
        }
        return $named ? "its database $database looks like production" : null;
    } catch (\Throwable $e) {
        return 'its environment could not be read (' . $e->getMessage() . ')';
    }
})();
if ($__eRefused !== null) {
    echo "\n{mark}" . json_encode(['value' => '', 'full' => '', 'error' => "Not evaluated: $__eRefused", 'printed' => '', 'ms' => 0, 'queries' => []]) . "\n";
    exit(0);
}

function __e_base($v): string {
    $class = is_object($v) ? get_class($v) : (string) $v;
    $at = strrpos($class, '\\');
    return $at === false ? $class : substr($class, $at + 1);
}

function __e_one($v, int $depth = 0): string {
    if ($v === null) {
        return 'null';
    }
    if (is_bool($v) || is_int($v) || is_float($v)) {
        return var_export($v, true);
    }
    if (is_string($v)) {
        return json_encode($v, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);
    }
    if (is_array($v)) {
        if ($depth > 0) {
            return '[…' . count($v) . ']';
        }
        $parts = [];
        $list = array_is_list($v);
        foreach (array_slice($v, 0, 6, true) as $k => $item) {
            $parts[] = ($list ? '' : $k . ': ') . __e_one($item, $depth + 1);
        }
        return '[' . implode(', ', $parts) . (count($v) > 6 ? ', …' : '') . ']';
    }
    if ($v instanceof \Illuminate\Database\Eloquent\Model) {
        $key = $v->getKey();
        $head = get_class($v) . ($key !== null ? ' #' . $key : '');
        if ($depth > 0) {
            return $head;
        }
        $parts = [];
        foreach (array_slice($v->attributesToArray(), 0, 6, true) as $k => $item) {
            $parts[] = $k . ': ' . __e_one($item, $depth + 1);
        }
        return $head . ' {' . implode(', ', $parts) . (count($v->getAttributes()) > 6 ? ', …' : '') . '}';
    }
    if ($v instanceof \Illuminate\Support\Collection) {
        $head = __e_base($v) . '(' . $v->count() . ')';
        if ($depth > 0) {
            return $head;
        }
        $items = $v->take(3)->map(fn ($item) => __e_one($item, 1))->all();
        return $head . ' [' . implode(', ', $items) . ($v->count() > 3 ? ', …' : '') . ']';
    }
    if ($v instanceof \Illuminate\Contracts\Pagination\LengthAwarePaginator) {
        return __e_base($v) . '(' . $v->count() . ' of ' . $v->total() . ')';
    }
    if ($v instanceof \Illuminate\Database\Eloquent\Builder || $v instanceof \Illuminate\Database\Query\Builder || $v instanceof \Illuminate\Database\Eloquent\Relations\Relation) {
        return __e_base($v) . ': ' . __e_sql($v);
    }
    if ($v instanceof \Closure) {
        return 'Closure';
    }
    if ($v instanceof \UnitEnum) {
        return get_class($v) . '::' . $v->name;
    }
    if ($v instanceof \DateTimeInterface) {
        return get_class($v) . ' ' . $v->format('Y-m-d H:i:s');
    }
    if ($v instanceof \Stringable) {
        return get_class($v) . ' ' . json_encode((string) $v, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);
    }
    return get_class($v);
}

function __e_sql($builder): string {
    $base = $builder;
    if ($base instanceof \Illuminate\Database\Eloquent\Relations\Relation) {
        $base = $base->getQuery();
    }
    if ($base instanceof \Illuminate\Database\Eloquent\Builder) {
        $base = $base->toBase();
    }
    if (method_exists($base, 'toRawSql')) {
        return $base->toRawSql();
    }
    $sql = $base->toSql();
    foreach ($base->getBindings() as $binding) {
        $value = is_numeric($binding) ? (string) $binding : "'" . str_replace("'", "''", (string) $binding) . "'";
        $sql = preg_replace('/\\?/', $value, $sql, 1);
    }
    return $sql;
}

function __e_full($v): string {
    if ($v instanceof \Illuminate\Database\Eloquent\Builder || $v instanceof \Illuminate\Database\Query\Builder || $v instanceof \Illuminate\Database\Eloquent\Relations\Relation) {
        return __e_one($v);
    }
    if ($v instanceof \Illuminate\Contracts\Support\Arrayable) {
        $v = $v->toArray();
    } elseif ($v instanceof \JsonSerializable) {
        $v = $v->jsonSerialize();
    } elseif (is_object($v) && !($v instanceof \Closure)) {
        $v = [get_class($v) => get_object_vars($v)];
    }
    $json = json_encode($v, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_PARTIAL_OUTPUT_ON_ERROR);
    return $json === false ? __e_one($v) : (strlen($json) > 20000 ? substr($json, 0, 20000) . "\n…" : $json);
}

$__eQueries = [];
$__eConnection = null;
try {
    \Illuminate\Support\Facades\Mail::fake();
    \Illuminate\Support\Facades\Notification::fake();
    \Illuminate\Support\Facades\Queue::fake();
    \Illuminate\Support\Facades\DB::listen(function ($q) use (&$__eQueries) {
        $sql = $q->sql;
        try {
            if (method_exists($q, 'toRawSql')) {
                $sql = $q->toRawSql();
            }
        } catch (\Throwable $e) {
        }
        $__eQueries[] = [$sql, (float) $q->time];
    });
    $__eConnection = \Illuminate\Support\Facades\DB::connection();
    $__eConnection->beginTransaction();
} catch (\Throwable $e) {
    // No database: the expression may not need one.
}

$__eStart = hrtime(true);
ob_start();
try {
    $__eValue = eval({source}
    );
    $__eOut = ['value' => __e_one($__eValue), 'full' => __e_full($__eValue), 'error' => null];
} catch (\Throwable $__eError) {
    // Where it was thrown, when that's the project's own code.
    $__eFile = $__eError->getFile();
    $__eFrom = $__eFile === __FILE__ || str_contains($__eFile, "eval()'d code") || str_contains($__eFile, '/vendor/')
        ? '' : ' (' . $__eFile . ':' . $__eError->getLine() . ')';
    $__eOut = [
        'value' => '',
        'full' => $__eError->getTraceAsString(),
        'error' => __e_base($__eError) . ': ' . $__eError->getMessage() . $__eFrom,
    ];
}
$__eOut['printed'] = (string) ob_get_clean();
$__eOut['ms'] = (hrtime(true) - $__eStart) / 1e6;
$__eOut['queries'] = $__eQueries;
try {
    while ($__eConnection && $__eConnection->transactionLevel() > 0) {
        $__eConnection->rollBack();
    }
} catch (\Throwable $e) {
}
echo "\n{mark}" . json_encode($__eOut, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_PARTIAL_OUTPUT_ON_ERROR) . "\n";
"#;

/// The runner's answer in `stdout`, or why there is none — the app didn't
/// boot, PHP died — from what it printed.
pub fn read_answer(stdout: &str, stderr: &str) -> Result<Evaluated, String> {
    let Some(line) = stdout.lines().rev().find_map(|l| l.strip_prefix(MARK)) else {
        let said = format!("{stderr}\n{stdout}");
        let first = said
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("PHP gave no answer");
        return Err(first.chars().take(300).collect());
    };
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok(Evaluated {
        value: s("value"),
        full: s("full"),
        error: v.get("error").and_then(|e| e.as_str()).map(str::to_string),
        queries: v
            .get("queries")
            .and_then(|q| q.as_array())
            .map(|qs| {
                qs.iter()
                    .filter_map(|q| Some((q.get(0)?.as_str()?.to_string(), q.get(1)?.as_f64()?)))
                    .collect()
            })
            .unwrap_or_default(),
        ms: v.get("ms").and_then(|m| m.as_f64()).unwrap_or(0.0),
        printed: s("printed"),
    })
}

/// Evaluate `code` in the app at `root`, as `file` (the text of the file it
/// comes from) would see it. Blocking: off the UI thread.
pub fn run(root: &Path, file: &str, code: &str) -> Result<Evaluated, String> {
    static RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let script = std::env::temp_dir().join(format!("e-eval-{}-{n}.php", std::process::id()));
    std::fs::write(&script, runner(root, &context_of(file), code)).map_err(|e| e.to_string())?;
    let out = crate::phptools::php(
        root,
        &[&script.to_string_lossy()],
        std::time::Duration::from_secs(30),
    );
    let _ = std::fs::remove_file(&script);
    let out = out?;
    read_answer(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'a>(text: &'a str, marker: &str) -> &'a str {
        let off = text.find(marker).unwrap();
        &text[statement_at(text, off).unwrap()]
    }

    const CONTROLLER: &str = r#"<?php

namespace App\Http\Controllers;

use App\Models\Order;
use Illuminate\Support\Facades\{DB, Cache};

class OrderController extends Controller
{
    public function index(User $user)
    {
        $orders = Order::query()
            ->whereBelongsTo($user)
            ->withSum('items', 'price') // the total; ; in a comment
            ->each(function ($o) { $o->touch(); });
        if ($orders) {
            return view('orders.index', ['n' => "a;b{c}"]);
        }
        $x = 1
    }
}
"#;

    #[test]
    fn the_statement_spans_its_lines_and_skips_what_is_inside_it() {
        assert_eq!(
            at(CONTROLLER, "whereBelongsTo"),
            "$orders = Order::query()\n            ->whereBelongsTo($user)\n            ->withSum('items', 'price') // the total; ; in a comment\n            ->each(function ($o) { $o->touch(); })"
        );
        assert_eq!(
            at(CONTROLLER, "return view"),
            "return view('orders.index', ['n' => \"a;b{c}\"])"
        );
        // Inside the closure: the statement there.
        assert_eq!(at(CONTROLLER, "touch"), "$o->touch()");
        // No `;` before the block ends.
        assert_eq!(at(CONTROLLER, "$x = 1"), "$x = 1");
    }

    #[test]
    fn a_statement_is_evaluated_for_its_value() {
        assert_eq!(expression("return view('x');").unwrap(), "view('x')");
        assert_eq!(
            expression("$a = Order::first();").unwrap(),
            "$a = Order::first()"
        );
        assert_eq!(expression("fn () => 1").unwrap(), "fn () => 1");
        assert_eq!(
            expression("match ($a) { 1 => 2 }").unwrap(),
            "match ($a) { 1 => 2 }"
        );
        assert!(expression("if ($orders) {").is_err());
        assert!(expression("public function index()").is_err());
        assert!(expression("  ").is_err());
    }

    #[test]
    fn the_file_s_namespace_and_imports_come_along() {
        assert_eq!(
            context_of(CONTROLLER),
            "namespace App\\Http\\Controllers;\nuse App\\Models\\Order;\nuse Illuminate\\Support\\Facades\\{DB, Cache};\n"
        );
        let grouped = "<?php\nnamespace A;\nuse B\\{\n    C,\n    D,\n};\nuse E;\n\nclass F\n{\n    use G;\n}\n";
        assert_eq!(
            context_of(grouped),
            "namespace A;\nuse B\\{\n    C,\n    D,\n};\nuse E;\n"
        );
    }

    #[test]
    fn the_answer_reads_as_a_label_and_a_report() {
        let out = "noise\n__E_EVAL__{\"value\":\"App\\\\Models\\\\Order #14 {id: 14}\",\"full\":\"{}\",\"error\":null,\"printed\":\"\",\"ms\":3.25,\"queries\":[[\"select * from orders limit 1\",0.8],[\"select 1\",0.1]]}\n";
        let e = read_answer(out, "").unwrap();
        assert_eq!(
            e.label(),
            "⇒ App\\Models\\Order #14 {id: 14} · 2 queries · 3.2 ms"
        );
        assert!(e
            .report("Order::first()")
            .contains("  select * from orders limit 1   (0.8 ms)\n"));
        assert_eq!(
            read_answer(
                "",
                "PHP Fatal error:  Uncaught Error: Class \"X\" not found\n"
            ),
            Err("PHP Fatal error:  Uncaught Error: Class \"X\" not found".to_string())
        );
    }

    /// The runner, end to end, against a stand-in app: PHP must be there.
    #[test]
    fn an_expression_runs_in_its_file_s_namespace() {
        if std::process::Command::new("php")
            .arg("-v")
            .output()
            .is_err()
        {
            return;
        }
        let root = std::env::temp_dir().join(format!("e-eval-app-{}", std::process::id()));
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        std::fs::create_dir_all(root.join("bootstrap")).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(
            root.join("vendor/autoload.php"),
            "<?php\nnamespace App\\Support { class Money { public static function cents(int $kr): int { return $kr * 100; } } }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("bootstrap/app.php"),
            "<?php\nfunction config($key) { return $key === 'database.default' ? 'sqlite' : ['driver' => 'sqlite', 'database' => getenv('E_STUB_DB') ?: '/tmp/app.sqlite']; }\nreturn new class { public function make($c) { return new class { public function bootstrap() {} }; } public function environment($env) { return getenv('E_STUB_ENV') === $env; } };\n",
        )
        .unwrap();
        let file = "<?php\n\nnamespace App\\Http;\n\nuse App\\Support\\Money;\n\nclass X {}\n";
        let e = run(&root, file, "Money::cents(12)").unwrap();
        assert_eq!(e.error, None, "{e:?}");
        assert_eq!(e.value, "1200");
        let e = run(&root, file, "[1, 'a' => null]").unwrap();
        assert_eq!(e.value, "[0: 1, a: null]");
        // A typo and an undefined variable come back as errors, not a crash.
        let e = run(&root, file, "Money::cents(").unwrap();
        assert!(
            e.error.as_deref().unwrap().starts_with("ParseError: "),
            "{e:?}"
        );
        let e = run(&root, file, "$user->id").unwrap();
        assert_eq!(
            e.error.as_deref(),
            Some("ErrorException: Undefined variable $user"),
            "{e:?}"
        );
        // Never against what looks like production.
        std::env::set_var("E_STUB_DB", "/srv/shop_production.sqlite");
        let e = run(&root, file, "Money::cents(1)").unwrap();
        std::env::remove_var("E_STUB_DB");
        assert_eq!(
            e.error.as_deref(),
            Some("Not evaluated: its database /srv/shop_production.sqlite looks like production")
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
