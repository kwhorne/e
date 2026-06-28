//! Minimal multi-engine database access for the editor's Database panel.
//!
//! Supports MySQL/MariaDB, PostgreSQL and SQLite using blocking drivers. The
//! design mirrors the Conductor database panel: connections are described by a
//! [`DbConfig`], opened into a live [`Conn`], and queried into a [`QueryResult`]
//! whose cells are all stringified for display in a grid.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub const MAX_ROWS: usize = 1000;

/// A saved database connection (per project).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DbConfig {
    /// `mysql` | `postgres` | `sqlite`.
    pub engine: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// For sqlite: absolute path to the `.sqlite` file.
    #[serde(default)]
    pub path: String,
    /// Friendly label (e.g. project name).
    #[serde(default)]
    pub label: String,
    /// Optional group/folder name for organising connections in the panel.
    #[serde(default)]
    pub group: String,
}

impl DbConfig {
    /// A stable key for this connection within a project.
    pub fn key(&self) -> String {
        match self.engine.as_str() {
            "sqlite" => format!("sqlite:{}", self.path),
            _ => format!(
                "{}:{}@{}:{}/{}",
                self.engine, self.username, self.host, self.port, self.database
            ),
        }
    }

    /// Short display name for the tree.
    pub fn display_name(&self) -> String {
        match self.engine.as_str() {
            "sqlite" => Path::new(&self.path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "sqlite".to_string()),
            _ => {
                if !self.database.is_empty() {
                    self.database.clone()
                } else if !self.label.is_empty() {
                    self.label.clone()
                } else {
                    self.host.clone()
                }
            }
        }
    }

    /// Secondary line (host/user or file path).
    pub fn subtitle(&self) -> String {
        match self.engine.as_str() {
            "sqlite" => self.path.clone(),
            _ => format!("{}@{}:{}", self.username, self.host, self.port),
        }
    }
}

/// A live connection.
pub enum Conn {
    Mysql(mysql::Pool),
    Sqlite(String),
    Postgres(Mutex<postgres::Client>),
}

/// A query (or table) result, all cells stringified.
#[derive(Clone, Debug, Default, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub rows_affected: Option<u64>,
    pub elapsed_ms: u64,
    pub is_select: bool,
    pub truncated: bool,
}

// ── .env detection (Laravel conventions) ───────────────────────

fn parse_env(path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(txt) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let mut v = v.trim().to_string();
        if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            v = v[1..v.len() - 1].to_string();
        }
        out.insert(k.trim().to_string(), v);
    }
    out
}

/// Build a connection config from a project's `.env`.
pub fn from_env(project: &Path) -> Option<DbConfig> {
    let env = parse_env(&project.join(".env"));
    let engine = env
        .get("DB_CONNECTION")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let label = project
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let host = env
        .get("DB_HOST")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());

    match engine.as_str() {
        "mysql" | "mariadb" => Some(DbConfig {
            engine: "mysql".into(),
            host,
            port: env.get("DB_PORT").and_then(|p| p.parse().ok()).unwrap_or(3306),
            database: env.get("DB_DATABASE").cloned().unwrap_or_default(),
            username: env.get("DB_USERNAME").cloned().unwrap_or_default(),
            password: env.get("DB_PASSWORD").cloned().unwrap_or_default(),
            label,
            ..Default::default()
        }),
        "pgsql" | "postgres" | "postgresql" => Some(DbConfig {
            engine: "postgres".into(),
            host,
            port: env.get("DB_PORT").and_then(|p| p.parse().ok()).unwrap_or(5432),
            database: env.get("DB_DATABASE").cloned().unwrap_or_default(),
            username: env.get("DB_USERNAME").cloned().unwrap_or_default(),
            password: env.get("DB_PASSWORD").cloned().unwrap_or_default(),
            label,
            ..Default::default()
        }),
        "sqlite" => {
            let raw = env.get("DB_DATABASE").cloned().unwrap_or_default();
            let path = if raw.is_empty() {
                project.join("database/database.sqlite").to_string_lossy().into_owned()
            } else if Path::new(&raw).is_absolute() {
                raw
            } else {
                project.join(&raw).to_string_lossy().into_owned()
            };
            Some(DbConfig {
                engine: "sqlite".into(),
                path,
                label,
                ..Default::default()
            })
        }
        _ => None,
    }
}

// ── connect / test ─────────────────────────────────────────────

pub fn connect(config: &DbConfig) -> Result<Conn, String> {
    Ok(match config.engine.as_str() {
        "mysql" => {
            let opts = mysql::OptsBuilder::new()
                .ip_or_hostname(Some(config.host.clone()))
                .tcp_port(config.port)
                .user(Some(config.username.clone()))
                .pass(Some(config.password.clone()))
                .db_name(if config.database.is_empty() {
                    None
                } else {
                    Some(config.database.clone())
                });
            let pool = mysql::Pool::new(opts).map_err(|e| e.to_string())?;
            let _ = pool.get_conn().map_err(|e| e.to_string())?;
            Conn::Mysql(pool)
        }
        "sqlite" => {
            if !Path::new(&config.path).exists() {
                return Err(format!("SQLite file not found: {}", config.path));
            }
            rusqlite::Connection::open(&config.path).map_err(|e| e.to_string())?;
            Conn::Sqlite(config.path.clone())
        }
        "postgres" | "postgresql" | "pgsql" => {
            let mut pg = postgres::Config::new();
            pg.host(if config.host.is_empty() {
                "127.0.0.1"
            } else {
                &config.host
            })
            .port(if config.port == 0 { 5432 } else { config.port })
            .user(&config.username);
            if !config.password.is_empty() {
                pg.password(&config.password);
            }
            if !config.database.is_empty() {
                pg.dbname(&config.database);
            }
            let client = pg.connect(postgres::NoTls).map_err(|e| e.to_string())?;
            Conn::Postgres(Mutex::new(client))
        }
        other => return Err(format!("Unsupported engine: {other}")),
    })
}

/// Try a connection without keeping it (the "Test connection" button).
pub fn test(config: &DbConfig) -> Result<(), String> {
    connect(config).map(|_| ())
}

// ── schema ─────────────────────────────────────────────────────

pub fn tables(conn: &Conn) -> Result<Vec<String>, String> {
    match conn {
        Conn::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            c.query("SHOW TABLES").map_err(|e| e.to_string())
        }
        Conn::Sqlite(path) => {
            let c = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
            let mut stmt = c
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        }
        Conn::Postgres(m) => {
            let mut client = m.lock().unwrap();
            let res = pg_query(
                &mut client,
                "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY tablename",
                MAX_ROWS,
            )?;
            Ok(res
                .rows
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect())
        }
    }
}

/// Quote an identifier for the given engine.
fn quote_ident(engine: &str, ident: &str) -> String {
    match engine {
        "postgres" | "postgresql" | "pgsql" => format!("\"{}\"", ident.replace('"', "\"\"")),
        _ => format!("`{}`", ident.replace('`', "``")),
    }
}

/// `SELECT * FROM <table> LIMIT <max>` for browsing a table.
pub fn table_data(conn: &Conn, engine: &str, table: &str, max: usize) -> Result<QueryResult, String> {
    let sql = format!("SELECT * FROM {} LIMIT {}", quote_ident(engine, table), max);
    query(conn, &sql, max)
}

// ── query ──────────────────────────────────────────────────────

fn is_select(sql: &str) -> bool {
    let s = sql.trim_start().to_lowercase();
    s.starts_with("select")
        || s.starts_with("show")
        || s.starts_with("pragma")
        || s.starts_with("explain")
        || s.starts_with("describe")
        || s.starts_with("desc ")
        || s.starts_with("with")
}

pub fn query(conn: &Conn, sql: &str, max: usize) -> Result<QueryResult, String> {
    let select = is_select(sql);
    let start = Instant::now();
    match conn {
        Conn::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            if select {
                let result = c.query_iter(sql).map_err(|e| e.to_string())?;
                let columns: Vec<String> = result
                    .columns()
                    .as_ref()
                    .iter()
                    .map(|c| c.name_str().to_string())
                    .collect();
                let mut rows = Vec::new();
                let mut truncated = false;
                for row in result {
                    let row = row.map_err(|e| e.to_string())?;
                    if rows.len() >= max {
                        truncated = true;
                        break;
                    }
                    let vals: Vec<Option<String>> = (0..columns.len())
                        .map(|i| row.as_ref(i).and_then(mysql_value_to_string))
                        .collect();
                    rows.push(vals);
                }
                Ok(QueryResult {
                    columns,
                    rows,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    is_select: true,
                    truncated,
                    ..Default::default()
                })
            } else {
                c.query_drop(sql).map_err(|e| e.to_string())?;
                Ok(QueryResult {
                    rows_affected: Some(c.affected_rows()),
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    ..Default::default()
                })
            }
        }
        Conn::Sqlite(path) => {
            let c = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
            if select {
                let mut stmt = c.prepare(sql).map_err(|e| e.to_string())?;
                let columns: Vec<String> =
                    stmt.column_names().iter().map(|s| s.to_string()).collect();
                let ncols = columns.len();
                let mut rows = Vec::new();
                let mut truncated = false;
                let mut q = stmt.query([]).map_err(|e| e.to_string())?;
                while let Some(row) = q.next().map_err(|e| e.to_string())? {
                    if rows.len() >= max {
                        truncated = true;
                        break;
                    }
                    let mut vals = Vec::with_capacity(ncols);
                    for i in 0..ncols {
                        use rusqlite::types::ValueRef::*;
                        let v = match row.get_ref(i).map_err(|e| e.to_string())? {
                            Null => None,
                            Integer(n) => Some(n.to_string()),
                            Real(f) => Some(f.to_string()),
                            Text(t) => Some(String::from_utf8_lossy(t).to_string()),
                            Blob(b) => Some(format!("<{} bytes>", b.len())),
                        };
                        vals.push(v);
                    }
                    rows.push(vals);
                }
                Ok(QueryResult {
                    columns,
                    rows,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    is_select: true,
                    truncated,
                    ..Default::default()
                })
            } else {
                let affected = c.execute(sql, []).map_err(|e| e.to_string())?;
                Ok(QueryResult {
                    rows_affected: Some(affected as u64),
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    ..Default::default()
                })
            }
        }
        Conn::Postgres(m) => {
            let mut client = m.lock().unwrap();
            let res = pg_query(&mut client, sql, max)?;
            if select || !res.columns.is_empty() {
                Ok(QueryResult {
                    columns: res.columns,
                    rows: res.rows,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    is_select: true,
                    truncated: res.truncated,
                    ..Default::default()
                })
            } else {
                Ok(QueryResult {
                    rows_affected: res.affected,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    ..Default::default()
                })
            }
        }
    }
}

struct PgRows {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    affected: Option<u64>,
    truncated: bool,
}

fn pg_query(client: &mut postgres::Client, sql: &str, max: usize) -> Result<PgRows, String> {
    use postgres::SimpleQueryMessage::*;
    let msgs = client.simple_query(sql).map_err(|e| e.to_string())?;
    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut affected = None;
    let mut truncated = false;
    for m in msgs {
        match m {
            Row(row) => {
                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                if rows.len() >= max {
                    truncated = true;
                    continue;
                }
                let vals = (0..row.len())
                    .map(|i| row.get(i).map(|s| s.to_string()))
                    .collect();
                rows.push(vals);
            }
            CommandComplete(n) => affected = Some(n),
            _ => {}
        }
    }
    Ok(PgRows {
        columns,
        rows,
        affected,
        truncated,
    })
}

fn mysql_value_to_string(v: &mysql::Value) -> Option<String> {
    use mysql::Value::*;
    match v {
        NULL => None,
        Bytes(b) => Some(String::from_utf8_lossy(b).to_string()),
        Int(i) => Some(i.to_string()),
        UInt(u) => Some(u.to_string()),
        Float(f) => Some(f.to_string()),
        Double(d) => Some(d.to_string()),
        Date(y, mo, d, h, mi, s, _us) => Some(format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")),
        Time(neg, days, h, mi, s, _us) => Some(format!(
            "{}{}:{mi:02}:{s:02}",
            if *neg { "-" } else { "" },
            *h as u32 + days * 24
        )),
    }
}

// ── persistence (per project, in ~/.config/e/databases.json) ───

fn store_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("e")
            .join("databases.json"),
    )
}

fn read_store() -> serde_json::Value {
    store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Load the saved connections for a project (keyed by its root path).
pub fn load_connections(project: &Path) -> Vec<DbConfig> {
    let store = read_store();
    store
        .get(project.to_string_lossy().as_ref())
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Persist the connection list for a project.
pub fn save_connections(project: &Path, conns: &[DbConfig]) -> Result<(), String> {
    let mut store = read_store();
    let obj = store.as_object_mut().ok_or("invalid store")?;
    obj.insert(
        project.to_string_lossy().into_owned(),
        serde_json::to_value(conns).map_err(|e| e.to_string())?,
    );
    let path = store_path().ok_or("no HOME")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_roundtrip() {
        let dir = std::env::temp_dir().join("e_db_test");
        let _ = std::fs::create_dir_all(&dir);
        let dbfile = dir.join("t.sqlite");
        let _ = std::fs::remove_file(&dbfile);
        {
            let c = rusqlite::Connection::open(&dbfile).unwrap();
            c.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);\
                 INSERT INTO users (name) VALUES ('Alice'), ('Bob');",
            )
            .unwrap();
        }
        let cfg = DbConfig {
            engine: "sqlite".into(),
            path: dbfile.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let conn = connect(&cfg).unwrap();
        let mut tbls = tables(&conn).unwrap();
        tbls.sort();
        assert_eq!(tbls, vec!["users".to_string()]);
        let res = table_data(&conn, "sqlite", "users", 100).unwrap();
        assert_eq!(res.columns, vec!["id".to_string(), "name".to_string()]);
        assert_eq!(res.rows.len(), 2);
        assert_eq!(res.rows[0][1], Some("Alice".to_string()));
        let _ = std::fs::remove_file(&dbfile);
    }

    #[test]
    fn env_detection_sqlite() {
        let dir = std::env::temp_dir().join("e_db_env_test");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(".env"), "DB_CONNECTION=sqlite\n").unwrap();
        let cfg = from_env(&dir).unwrap();
        assert_eq!(cfg.engine, "sqlite");
        assert!(cfg.path.ends_with("database/database.sqlite"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
