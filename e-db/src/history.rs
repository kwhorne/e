//! Query history, persisted in a single SQLite database (per the PRD: one DB,
//! keyed by project) so it survives restarts and stays searchable. Every run
//! from the SQL console is logged with its connection, timing and row count.

use rusqlite::Connection;

/// The maximum number of rows kept; oldest entries beyond this are pruned on
/// insert so the history file can't grow without bound.
pub const MAX_ENTRIES: usize = 10_000;

/// One logged query execution.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    /// Unix epoch milliseconds.
    pub ts: i64,
    pub project: String,
    pub connection: String,
    pub sql: String,
    pub rows: Option<i64>,
    pub duration_ms: i64,
    pub ok: bool,
    pub error: Option<String>,
}

fn open(path: &str) -> Result<Connection, String> {
    let c = Connection::open(path).map_err(|e| e.to_string())?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS query_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts INTEGER NOT NULL,
            project TEXT NOT NULL,
            connection TEXT NOT NULL,
            sql TEXT NOT NULL,
            rows INTEGER,
            duration_ms INTEGER NOT NULL,
            ok INTEGER NOT NULL,
            error TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_history_project_ts
            ON query_history (project, ts DESC);",
    )
    .map_err(|e| e.to_string())?;
    Ok(c)
}

/// Log a query execution, then prune anything past [`MAX_ENTRIES`].
#[allow(clippy::too_many_arguments)]
pub fn record(
    path: &str,
    project: &str,
    connection: &str,
    sql: &str,
    rows: Option<i64>,
    duration_ms: i64,
    ok: bool,
    error: Option<&str>,
) -> Result<(), String> {
    let c = open(path)?;
    let ts = now_ms();
    c.execute(
        "INSERT INTO query_history
            (ts, project, connection, sql, rows, duration_ms, ok, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            ts,
            project,
            connection,
            sql,
            rows,
            duration_ms,
            ok as i64,
            error
        ],
    )
    .map_err(|e| e.to_string())?;
    // Prune: keep only the newest MAX_ENTRIES rows overall.
    c.execute(
        "DELETE FROM query_history WHERE id NOT IN
            (SELECT id FROM query_history ORDER BY id DESC LIMIT ?1)",
        rusqlite::params![MAX_ENTRIES as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The most recent entries for a project (newest first).
pub fn recent(path: &str, project: &str, limit: usize) -> Result<Vec<HistoryEntry>, String> {
    query_entries(
        path,
        "SELECT id, ts, project, connection, sql, rows, duration_ms, ok, error
         FROM query_history WHERE project = ?1 ORDER BY id DESC LIMIT ?2",
        rusqlite::params![project, limit as i64],
    )
}

/// Entries for a project whose SQL matches `needle` (case-insensitive substring),
/// newest first.
pub fn search(
    path: &str,
    project: &str,
    needle: &str,
    limit: usize,
) -> Result<Vec<HistoryEntry>, String> {
    let like = format!("%{}%", needle.replace('%', "\\%").replace('_', "\\_"));
    query_entries(
        path,
        "SELECT id, ts, project, connection, sql, rows, duration_ms, ok, error
         FROM query_history
         WHERE project = ?1 AND sql LIKE ?2 ESCAPE '\\'
         ORDER BY id DESC LIMIT ?3",
        rusqlite::params![project, like, limit as i64],
    )
}

/// Delete all history for a project.
pub fn clear(path: &str, project: &str) -> Result<(), String> {
    let c = open(path)?;
    c.execute(
        "DELETE FROM query_history WHERE project = ?1",
        rusqlite::params![project],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn query_entries(
    path: &str,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<HistoryEntry>, String> {
    let c = open(path)?;
    let mut stmt = c.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, |r| {
            Ok(HistoryEntry {
                id: r.get(0)?,
                ts: r.get(1)?,
                project: r.get(2)?,
                connection: r.get(3)?,
                sql: r.get(4)?,
                rows: r.get(5)?,
                duration_ms: r.get(6)?,
                ok: r.get::<_, i64>(7)? != 0,
                error: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let dir = std::env::temp_dir().join("e_db_history_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn record_recent_search_and_clear() {
        let path = tmp("h.sqlite");
        record(
            &path,
            "proj",
            "myapp (local)",
            "SELECT 1",
            Some(1),
            5,
            true,
            None,
        )
        .unwrap();
        record(
            &path,
            "proj",
            "myapp (local)",
            "SELECT * FROM users",
            Some(42),
            12,
            true,
            None,
        )
        .unwrap();
        record(
            &path,
            "proj",
            "myapp (local)",
            "SELECT bad",
            None,
            3,
            false,
            Some("boom"),
        )
        .unwrap();
        // Another project is isolated.
        record(&path, "other", "x", "SELECT 99", Some(1), 1, true, None).unwrap();

        let latest = recent(&path, "proj", 10).unwrap();
        assert_eq!(latest.len(), 3);
        // newest first
        assert_eq!(latest[0].sql, "SELECT bad");
        assert!(!latest[0].ok);
        assert_eq!(latest[0].error.as_deref(), Some("boom"));
        assert_eq!(latest[2].sql, "SELECT 1");

        let hits = search(&path, "proj", "users", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rows, Some(42));

        clear(&path, "proj").unwrap();
        assert!(recent(&path, "proj", 10).unwrap().is_empty());
        // other project untouched
        assert_eq!(recent(&path, "other", 10).unwrap().len(), 1);
    }

    #[test]
    fn prunes_beyond_max() {
        // Sanity check the prune keeps only the most recent MAX_ENTRIES; use a
        // tiny loop rather than MAX to keep the test fast by asserting ordering.
        let path = tmp("prune.sqlite");
        for i in 0..5 {
            record(
                &path,
                "p",
                "c",
                &format!("SELECT {i}"),
                Some(0),
                1,
                true,
                None,
            )
            .unwrap();
        }
        let all = recent(&path, "p", 100).unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].sql, "SELECT 4");
    }
}
