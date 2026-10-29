//! Reading the `failed_jobs` table: which job failed, with what, and on which
//! models. A queued job, listener, notification or mail that throws ends up
//! there — out of sight of the request that dispatched it, and of
//! `laravel.log` when the worker logs elsewhere.
//!
//! A row has the job's JSON payload — `displayName` is the class, and the
//! serialized command names the models it was dispatched with — and the
//! exception as PHP prints one: `Class: message in /path/file.php:12`, then
//! `Stack trace:` and a frame per line.

use std::path::PathBuf;

use crate::laravel_log::{frame, location, LoggedException};

/// The newest failed jobs, newest first. The `uuid` column came with Laravel
/// 8; [`QUERY_NO_UUID`] is for a table from before.
pub const QUERY: &str = "select id, uuid, queue, payload, exception, failed_at from failed_jobs order by id desc limit 20";
pub const QUERY_NO_UUID: &str = "select id, null as uuid, queue, payload, exception, failed_at from failed_jobs order by id desc limit 20";
/// What changes when a job fails, is retried or forgotten.
pub const FINGERPRINT: &str = "select count(*), max(id) from failed_jobs";

/// One failed job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedJob {
    pub id: String,
    /// What `queue:retry` takes; the id when the table has no uuid.
    pub uuid: Option<String>,
    pub queue: String,
    /// `2026-10-29 14:02:11`.
    pub failed_at: String,
    /// The job's class: `App\Jobs\SyncOrders`, `App\Listeners\SendInvoice`.
    pub class: String,
    /// The models it was dispatched with: `(App\Models\Order, 14)`.
    pub models: Vec<(String, String)>,
    /// The exception, with where it was thrown and its frames.
    pub exception: LoggedException,
}

impl FailedJob {
    /// What `php artisan queue:retry` takes for this job.
    pub fn retry_id(&self) -> &str {
        self.uuid.as_deref().unwrap_or(&self.id)
    }

    /// `App\Models\Order #14, App\Models\User #3`.
    pub fn models_text(&self) -> String {
        self.models
            .iter()
            .map(|(class, id)| format!("{class} #{id}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A row of [`QUERY`] as a failed job: `None` when the payload names no
/// class.
pub fn from_row(
    id: &str,
    uuid: Option<&str>,
    queue: &str,
    payload: &str,
    exception: &str,
    failed_at: &str,
) -> Option<FailedJob> {
    let json: serde_json::Value = serde_json::from_str(payload).ok()?;
    let class = json
        .get("displayName")
        .and_then(|v| v.as_str())
        .or_else(|| json.pointer("/data/commandName").and_then(|v| v.as_str()))?
        .to_string();
    let command = json
        .pointer("/data/command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Some(FailedJob {
        id: id.to_string(),
        uuid: uuid.filter(|u| !u.is_empty()).map(str::to_string),
        queue: queue.to_string(),
        failed_at: failed_at.to_string(),
        class,
        models: models(command),
        exception: thrown(exception, failed_at),
    })
}

/// The models a serialized command carries: each `ModelIdentifier`'s class
/// and id (`i:14;` or `s:36:"…";`).
fn models(command: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for part in command.split("ModelIdentifier").skip(1) {
        let Some(class) = serialized_after(part, "s:5:\"class\";") else {
            continue;
        };
        let Some(id) = serialized_after(part, "s:2:\"id\";") else {
            continue;
        };
        if !out.contains(&(class.clone(), id.clone())) {
            out.push((class, id));
        }
    }
    out
}

/// The scalar serialized right after `key` in `text`: `s:16:"App\Models\X";`
/// → `App\Models\X`, `i:14;` → `14`.
fn serialized_after(text: &str, key: &str) -> Option<String> {
    let rest = &text[text.find(key)? + key.len()..];
    if let Some(n) = rest.strip_prefix("i:") {
        return Some(n.split(';').next()?.to_string());
    }
    let rest = rest.strip_prefix("s:")?;
    let (len, rest) = rest.split_once(':')?;
    let len: usize = len.parse().ok()?;
    let value = rest.strip_prefix('"')?.get(..len)?;
    Some(value.to_string())
}

/// The exception as PHP prints it: `Class: message in /path:line`, then the
/// stack trace.
fn thrown(text: &str, failed_at: &str) -> LoggedException {
    let (head, trace) = text.split_once("\nStack trace:").unwrap_or((text, ""));
    let (class, rest) = head.split_once(": ").unwrap_or((head, ""));
    let (message, thrown_at) = match rest
        .rfind(" in ")
        .and_then(|i| Some((&rest[..i], location(&rest[i + 4..])?)))
    {
        Some((message, at)) => (message, Some(at)),
        None => (rest, None),
    };
    LoggedException {
        datetime: failed_at.to_string(),
        level: "ERROR".to_string(),
        class: class.trim().to_string(),
        message: message.trim().to_string(),
        thrown_at,
        frames: trace
            .lines()
            .filter_map(frame)
            .collect::<Vec<(PathBuf, usize)>>(),
        text: text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const PAYLOAD: &str = r#"{"uuid":"5f0c6e2a-1b7d-4c3e-9a51-0d2e8f6b7c11","displayName":"App\\Jobs\\SyncOrders","job":"Illuminate\\Queue\\CallQueuedHandler@call","maxTries":null,"data":{"commandName":"App\\Jobs\\SyncOrders","command":"O:19:\"App\\Jobs\\SyncOrders\":2:{s:5:\"order\";O:45:\"Illuminate\\Contracts\\Database\\ModelIdentifier\":5:{s:5:\"class\";s:16:\"App\\Models\\Order\";s:2:\"id\";i:14;s:9:\"relations\";a:0:{}s:10:\"connection\";s:6:\"sqlite\";s:15:\"collectionClass\";N;}s:4:\"user\";O:45:\"Illuminate\\Contracts\\Database\\ModelIdentifier\":5:{s:5:\"class\";s:15:\"App\\Models\\User\";s:2:\"id\";s:36:\"9a1b2c3d-0000-4000-8000-000000000001\";s:9:\"relations\";a:0:{}s:10:\"connection\";s:6:\"sqlite\";s:15:\"collectionClass\";N;}}"}}"#;

    const EXCEPTION: &str = "RuntimeException: The warehouse said no in /var/www/html/app/Services/Warehouse.php:31\nStack trace:\n#0 /var/www/html/app/Jobs/SyncOrders.php(24): App\\Services\\Warehouse->push()\n#1 /var/www/html/vendor/laravel/framework/src/Illuminate/Container/BoundMethod.php(36): App\\Jobs\\SyncOrders->handle()\n#2 {main}";

    #[test]
    fn a_row_reads_as_its_class_models_and_exception() {
        let job = from_row(
            "7",
            Some("5f0c6e2a-1b7d-4c3e-9a51-0d2e8f6b7c11"),
            "default",
            PAYLOAD,
            EXCEPTION,
            "2026-10-29 14:02:11",
        )
        .unwrap();
        assert_eq!(job.class, "App\\Jobs\\SyncOrders");
        assert_eq!(job.retry_id(), "5f0c6e2a-1b7d-4c3e-9a51-0d2e8f6b7c11");
        assert_eq!(
            job.models_text(),
            "App\\Models\\Order #14, App\\Models\\User #9a1b2c3d-0000-4000-8000-000000000001"
        );
        let e = &job.exception;
        assert_eq!(e.class, "RuntimeException");
        assert_eq!(e.message, "The warehouse said no");
        assert_eq!(
            e.thrown_at,
            Some((
                PathBuf::from("/var/www/html/app/Services/Warehouse.php"),
                31
            ))
        );
        assert_eq!(e.frames.len(), 2);
        // From a container, onto the project.
        let root = Path::new("/Users/me/shop");
        assert_eq!(
            e.origin(root, |p| p == root.join("app/Services/Warehouse.php")),
            Some((root.join("app/Services/Warehouse.php"), 31))
        );
    }

    #[test]
    fn an_old_table_retries_by_id_and_a_bare_payload_still_names_the_class() {
        let payload = r#"{"job":"Illuminate\\Queue\\CallQueuedHandler@call","data":{"commandName":"App\\Jobs\\Ping","command":"O:13:\"App\\Jobs\\Ping\":0:{}"}}"#;
        let job = from_row("3", None, "default", payload, "Exception: boom", "x").unwrap();
        assert_eq!(job.class, "App\\Jobs\\Ping");
        assert_eq!(job.retry_id(), "3");
        assert!(job.models.is_empty());
        assert_eq!(job.exception.message, "boom");
        assert_eq!(job.exception.thrown_at, None);
        assert_eq!(from_row("4", None, "default", "not json", "", "x"), None);
    }

    #[test]
    fn a_message_with_its_own_in_keeps_where_it_was_thrown() {
        let e = thrown(
            "ErrorException: Undefined array key \"sku\" in payload in /app/app/Jobs/Import.php:12\nStack trace:\n#0 {main}",
            "x",
        );
        assert_eq!(e.message, "Undefined array key \"sku\" in payload");
        assert_eq!(
            e.thrown_at,
            Some((PathBuf::from("/app/app/Jobs/Import.php"), 12))
        );
    }
}
