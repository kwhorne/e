//! Reading exceptions out of `storage/logs/laravel.log`: what was thrown, and
//! where in the project's own code it happened.
//!
//! Laravel logs an exception as one Monolog entry — a header line with the
//! time, level and message, a JSON context whose `exception` holds
//! `[object] (Class(code: N): message at /path/file.php:line)`, and the
//! stacktrace after `[stacktrace]`, one frame per line:
//! `#3 /path/file.php(48): Some\\Class->method()`. The interesting line is
//! the first one in the project's code: an exception thrown inside the
//! framework (a model not found, a validation failure) was caused by a call
//! from a controller, which is where the fix goes.

use std::path::{Path, PathBuf};

/// One exception from the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedException {
    /// `2026-10-28 20:15:03`.
    pub datetime: String,
    /// `ERROR`, `CRITICAL`, …
    pub level: String,
    /// `ErrorException`, `Illuminate\Database\QueryException`.
    pub class: String,
    pub message: String,
    /// Where it was thrown: `at /path:line`.
    pub thrown_at: Option<(PathBuf, usize)>,
    /// The stacktrace's frames, innermost first: file and 1-based line.
    pub frames: Vec<(PathBuf, usize)>,
    /// The entry as it stands in the log, for the agent.
    pub text: String,
}

impl LoggedException {
    /// `ErrorException` for `ErrorException`, `QueryException` for
    /// `Illuminate\Database\QueryException`.
    pub fn short_class(&self) -> &str {
        self.class.rsplit('\\').next().unwrap_or(&self.class)
    }

    /// The first place in the project's own code — under `root`, not in
    /// `vendor/` or `storage/` — that the exception passed through, as a file
    /// and a 1-based line. A path from inside a container (Sail, Docker) is
    /// mapped onto `root` when the same file exists there.
    pub fn origin(&self, root: &Path, exists: impl Fn(&Path) -> bool) -> Option<(PathBuf, usize)> {
        // A Blade view's error names the template; the compiled file it ran
        // as means nothing to the reader. No line comes with it — the caller,
        // which has the file, can look for an undefined variable's name.
        if let Some(view) = view_of(&self.message) {
            if let Some(view) = in_project(&view, root, &exists) {
                return Some((view, 1));
            }
        }
        self.thrown_at
            .iter()
            .chain(self.frames.iter())
            .find_map(|(path, line)| Some((in_project(path, root, &exists)?, *line)))
    }
}

/// `path` as a file of the project's own code under `root`, if it is one.
fn in_project(path: &Path, root: &Path, exists: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let own = |rel: &Path| !rel.starts_with("vendor") && !rel.starts_with("storage");
    if let Ok(rel) = path.strip_prefix(root) {
        return own(rel).then(|| path.to_path_buf());
    }
    // From a container: `/var/www/html/app/Models/Order.php` is the project's
    // `app/Models/Order.php` when there is one.
    let parts: Vec<_> = path.components().collect();
    (1..parts.len()).find_map(|i| {
        let rel: PathBuf = parts[i..].iter().collect();
        (own(&rel) && rel.components().count() > 1 && exists(&root.join(&rel)))
            .then(|| root.join(&rel))
    })
}

/// The template a view error names: `… (View: /path/resources/views/x.blade.php)`.
fn view_of(message: &str) -> Option<PathBuf> {
    let start = message.rfind("(View: ")? + "(View: ".len();
    let end = message[start..].find(')')? + start;
    Some(PathBuf::from(message[start..end].trim()))
}

/// The variable an "Undefined variable $x" message is about.
pub fn undefined_variable(message: &str) -> Option<&str> {
    let rest = message.split("Undefined variable $").nth(1)?;
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Whether `line` starts a log entry: `[2026-10-28 20:15:03] local.ERROR: …`.
fn header(line: &str) -> Option<(&str, &str, &str)> {
    let rest = line.strip_prefix('[')?;
    let (datetime, rest) = rest.split_once("] ")?;
    if datetime.len() < 19 || !datetime.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let (channel_level, message) = rest.split_once(": ")?;
    let level = channel_level.rsplit('.').next()?;
    level
        .chars()
        .all(|c| c.is_ascii_uppercase())
        .then_some((datetime, level, message))
}

/// The exceptions logged in `text` — a stretch of the log, from an entry's
/// start — at error level or worse, in the order they were written.
pub fn exceptions(text: &str) -> Vec<LoggedException> {
    let mut out = Vec::new();
    let mut entry: Vec<&str> = Vec::new();
    let mut flush = |entry: &mut Vec<&str>| {
        if let Some(e) = parse_entry(entry) {
            out.push(e);
        }
        entry.clear();
    };
    for line in text.lines() {
        if header(line).is_some() {
            flush(&mut entry);
        }
        entry.push(line);
    }
    flush(&mut entry);
    out
}

fn parse_entry(lines: &[&str]) -> Option<LoggedException> {
    let (datetime, level, _) = header(lines.first()?)?;
    if !matches!(level, "ERROR" | "CRITICAL" | "ALERT" | "EMERGENCY") {
        return None;
    }
    // `"exception":"[object] (Class(code: 0): message at /path:48)` — on
    // more lines than one when the message has line breaks (a Postgres
    // error's `LINE 1: …`), up to the stacktrace.
    let at = lines.iter().position(|l| l.contains("[object] ("))?;
    let first = lines[at];
    let start = first.find("[object] (")? + "[object] (".len();
    let trace = lines[at..]
        .iter()
        .position(|l| l.contains("[stacktrace]"))
        .map(|i| at + i);
    let object = match trace {
        Some(end) if end > at + 1 => std::iter::once(&first[start..])
            .chain(lines[at + 1..end].iter().copied())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => first[start..].to_string(),
    };
    let object = object.trim_end().trim_end_matches(')');
    let (class, rest) = object.split_once("(code: ")?;
    let (_, rest) = rest.split_once("): ")?;
    // The last ` at ` is where it was thrown; the message may have its own.
    let (message, thrown_at) = match rest
        .rfind(" at ")
        .and_then(|i| Some((&rest[..i], location(&rest[i + 4..])?)))
    {
        Some((message, at)) => (message, Some(at)),
        None => (rest, None),
    };
    let frames = lines
        .iter()
        .skip_while(|l| !l.contains("[stacktrace]"))
        .filter_map(|l| frame(l))
        .collect();
    Some(LoggedException {
        datetime: datetime.to_string(),
        level: level.to_string(),
        class: unescape(class),
        message: unescape(message),
        thrown_at,
        frames,
        text: lines.join("\n"),
    })
}

/// `/path/file.php:48` → the file and line.
pub(crate) fn location(s: &str) -> Option<(PathBuf, usize)> {
    let (path, line) = s.trim().rsplit_once(':')?;
    Some((PathBuf::from(path), line.trim().parse().ok()?))
}

/// `#3 /path/file.php(48): Some\\Class->method()` → the file and line.
pub(crate) fn frame(line: &str) -> Option<(PathBuf, usize)> {
    let rest = line.trim_start().strip_prefix('#')?;
    let (_, rest) = rest.split_once(' ')?;
    let open = rest.find("(")?;
    let close = rest[open..].find("):")? + open;
    let line_no = rest[open + 1..close].parse().ok()?;
    Some((PathBuf::from(&rest[..open]), line_no))
}

/// The JSON escaping the context carries: `\\` and `\"`.
fn unescape(s: &str) -> String {
    s.replace("\\\\", "\\").replace("\\\"", "\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/Users/kh/Code/shop";

    fn log() -> String {
        format!(
            "[2026-10-28 20:15:01] local.INFO: Order placed {{\"id\":7}}\n\
[2026-10-28 20:15:03] local.ERROR: Undefined variable $orders {{\"userId\":1,\"exception\":\"[object] (ErrorException(code: 0): Undefined variable $orders at {ROOT}/app/Http/Controllers/OrderController.php:48)\n\
[stacktrace]\n\
#0 {ROOT}/vendor/laravel/framework/src/Illuminate/Foundation/Bootstrap/HandleExceptions.php(256): Illuminate\\\\Foundation\\\\Bootstrap\\\\HandleExceptions->handleError(2, 'Undefined varia...', '/Users/kh/Code/...', 48)\n\
#1 {ROOT}/app/Http/Controllers/OrderController.php(48): Illuminate\\\\Foundation\\\\Bootstrap\\\\HandleExceptions->Illuminate\\\\Foundation\\\\Bootstrap\\\\{{closure}}(2, 'Undefined varia...', '/Users/kh/Code/...', 48)\n\
#2 {{main}}\n\
\"}} \n\
[2026-10-28 20:16:10] local.ERROR: No query results for model [App\\\\Models\\\\Order] 99 {{\"exception\":\"[object] (Illuminate\\\\Database\\\\Eloquent\\\\ModelNotFoundException(code: 0): No query results for model [App\\\\Models\\\\Order] 99 at {ROOT}/vendor/laravel/framework/src/Illuminate/Database/Eloquent/Builder.php:651)\n\
[stacktrace]\n\
#0 {ROOT}/vendor/laravel/framework/src/Illuminate/Database/Eloquent/Builder.php(700): Illuminate\\\\Database\\\\Eloquent\\\\Builder->findOrFail(99)\n\
#1 {ROOT}/app/Services/Checkout.php(31): Illuminate\\\\Database\\\\Eloquent\\\\Builder->findOrFail(99)\n\
#2 {ROOT}/app/Http/Controllers/CheckoutController.php(20): App\\\\Services\\\\Checkout->run()\n\
#3 {{main}}\n\
\"}} \n"
        )
    }

    #[test]
    fn exceptions_are_read_with_where_they_were_thrown() {
        let found = exceptions(&log());
        assert_eq!(found.len(), 2, "the INFO entry isn't one");
        let e = &found[0];
        assert_eq!(e.datetime, "2026-10-28 20:15:03");
        assert_eq!(e.class, "ErrorException");
        assert_eq!(e.message, "Undefined variable $orders");
        assert_eq!(
            e.thrown_at,
            Some((
                PathBuf::from(format!("{ROOT}/app/Http/Controllers/OrderController.php")),
                48
            ))
        );
        assert_eq!(e.frames.len(), 2);
        assert_eq!(
            found[1].class,
            "Illuminate\\Database\\Eloquent\\ModelNotFoundException"
        );
        assert_eq!(found[1].short_class(), "ModelNotFoundException");
        assert_eq!(
            found[1].message,
            "No query results for model [App\\Models\\Order] 99"
        );
    }

    #[test]
    fn the_origin_is_the_first_place_in_the_projects_code() {
        let found = exceptions(&log());
        let root = Path::new(ROOT);
        // Thrown in the controller itself.
        assert_eq!(
            found[0].origin(root, |_| false),
            Some((root.join("app/Http/Controllers/OrderController.php"), 48))
        );
        // Thrown in the framework: the service that called it, not the
        // controller further out.
        assert_eq!(
            found[1].origin(root, |_| false),
            Some((root.join("app/Services/Checkout.php"), 31))
        );
    }

    #[test]
    fn a_path_from_a_container_maps_onto_the_project() {
        let text = "[2026-10-28 20:15:03] local.ERROR: Boom {\"exception\":\"[object] (RuntimeException(code: 0): Boom at /var/www/html/app/Jobs/Sync.php:12)\n[stacktrace]\n#0 {main}\n\"} \n";
        let e = &exceptions(text)[0];
        let root = Path::new(ROOT);
        let exists = |p: &Path| p == root.join("app/Jobs/Sync.php");
        assert_eq!(
            e.origin(root, exists),
            Some((root.join("app/Jobs/Sync.php"), 12))
        );
        assert_eq!(e.origin(root, |_| false), None);
    }

    #[test]
    fn a_view_error_points_at_the_template() {
        let text = format!(
            "[2026-10-28 20:15:03] local.ERROR: Undefined variable $total (View: {ROOT}/resources/views/orders/show.blade.php) {{\"exception\":\"[object] (Illuminate\\\\View\\\\ViewException(code: 0): Undefined variable $total (View: {ROOT}/resources/views/orders/show.blade.php) at {ROOT}/storage/framework/views/0a1b2c.php:12)\n[stacktrace]\n#0 {{main}}\n\"}} \n"
        );
        let e = &exceptions(&text)[0];
        let root = Path::new(ROOT);
        assert_eq!(
            e.origin(root, |_| false),
            Some((root.join("resources/views/orders/show.blade.php"), 1))
        );
        assert_eq!(undefined_variable(&e.message), Some("total"));
    }

    #[test]
    fn a_message_over_several_lines_keeps_where_it_was_thrown() {
        // Postgres puts the query and a caret under the error, and the
        // message has an ` at ` of its own.
        let text = format!(
            "[2026-10-28 20:15:03] local.ERROR: SQLSTATE[42601]: Syntax error: 7 ERROR:  syntax error at or near \"fro\"\nLINE 1: select * fro orders\n                 ^ (Connection: pgsql, SQL: select * fro orders) {{\"exception\":\"[object] (Illuminate\\\\Database\\\\QueryException(code: 42601): SQLSTATE[42601]: Syntax error: 7 ERROR:  syntax error at or near \\\"fro\\\"\nLINE 1: select * fro orders\n                 ^ (Connection: pgsql, SQL: select * fro orders) at {ROOT}/vendor/laravel/framework/src/Illuminate/Database/Connection.php:825)\n[stacktrace]\n#0 {ROOT}/app/Http/Controllers/OrderController.php(20): Illuminate\\\\Database\\\\Connection->select()\n#1 {{main}}\n\"}} \n"
        );
        let e = &exceptions(&text)[0];
        assert_eq!(e.short_class(), "QueryException");
        assert!(
            e.message.starts_with("SQLSTATE[42601]: Syntax error")
                && e.message.contains("LINE 1: select * fro orders")
                && e.message
                    .ends_with("(Connection: pgsql, SQL: select * fro orders)"),
            "{}",
            e.message
        );
        assert_eq!(
            e.thrown_at,
            Some((
                PathBuf::from(format!(
                    "{ROOT}/vendor/laravel/framework/src/Illuminate/Database/Connection.php"
                )),
                825
            ))
        );
        let root = Path::new(ROOT);
        assert_eq!(
            e.origin(root, |_| false),
            Some((root.join("app/Http/Controllers/OrderController.php"), 20))
        );
    }

    #[test]
    fn a_partial_entry_and_noise_are_skipped() {
        assert!(exceptions("").is_empty());
        assert!(exceptions("#3 {main}\n\"} \n").is_empty());
        assert!(exceptions("[2026-10-28 20:15:03] local.ERROR: Plain error\n").is_empty());
    }
}
