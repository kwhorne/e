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
    /// Tunnel the database connection through SSH (remote databases).
    #[serde(default)]
    pub use_ssh: bool,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default)]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: String,
    /// `key` | `password`.
    #[serde(default)]
    pub ssh_auth: String,
    #[serde(default)]
    pub ssh_password: String,
    #[serde(default)]
    pub ssh_key_path: String,
    #[serde(default)]
    pub ssh_passphrase: String,
}

impl DbConfig {
    /// Heuristic: does this look like a production database? Remote/SSH targets
    /// and names containing prod/production/live are treated as production, so
    /// the editor can default them to read-only and warn before writes.
    pub fn looks_like_prod(&self) -> bool {
        if self.engine == "sqlite" {
            return name_hints_prod(&self.path) || name_hints_prod(&self.label);
        }
        // SSH tunnels almost always target a real (remote) server.
        if self.use_ssh && !self.ssh_host.is_empty() {
            return true;
        }
        let host = self.host.trim().to_ascii_lowercase();
        let local = matches!(
            host.as_str(),
            "" | "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal"
        );
        if !local {
            return true;
        }
        name_hints_prod(&self.host)
            || name_hints_prod(&self.database)
            || name_hints_prod(&self.label)
    }

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

/// The engine-specific transport behind a connection.
#[allow(clippy::large_enum_variant)]
enum Backend {
    Mysql(mysql::Pool),
    Sqlite(String),
    Postgres(Mutex<postgres::Client>),
    /// ClickHouse over its HTTP interface.
    Clickhouse {
        base_url: String,
        user: String,
        password: String,
        database: String,
    },
}

/// A live connection, optionally keeping an SSH tunnel alive for its lifetime.
pub struct Conn {
    backend: Backend,
    /// Dropped (killing the `ssh` child) when the connection is dropped.
    _tunnel: Option<Mutex<SshTunnel>>,
}

/// An SSH local port-forward run via the system `ssh` binary.
struct SshTunnel {
    child: std::process::Child,
    #[allow(dead_code)]
    local_port: u16,
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
            port: env
                .get("DB_PORT")
                .and_then(|p| p.parse().ok())
                .unwrap_or(3306),
            database: env.get("DB_DATABASE").cloned().unwrap_or_default(),
            username: env.get("DB_USERNAME").cloned().unwrap_or_default(),
            password: env.get("DB_PASSWORD").cloned().unwrap_or_default(),
            label,
            ..Default::default()
        }),
        "pgsql" | "postgres" | "postgresql" => Some(DbConfig {
            engine: "postgres".into(),
            host,
            port: env
                .get("DB_PORT")
                .and_then(|p| p.parse().ok())
                .unwrap_or(5432),
            database: env.get("DB_DATABASE").cloned().unwrap_or_default(),
            username: env.get("DB_USERNAME").cloned().unwrap_or_default(),
            password: env.get("DB_PASSWORD").cloned().unwrap_or_default(),
            label,
            ..Default::default()
        }),
        "clickhouse" => Some(DbConfig {
            engine: "clickhouse".into(),
            host,
            // HTTP interface (native protocol is 9000; HTTP is 8123).
            port: env
                .get("DB_PORT")
                .and_then(|p| p.parse().ok())
                .unwrap_or(8123),
            database: env.get("DB_DATABASE").cloned().unwrap_or_default(),
            username: env
                .get("DB_USERNAME")
                .cloned()
                .unwrap_or_else(|| "default".into()),
            password: env.get("DB_PASSWORD").cloned().unwrap_or_default(),
            label,
            ..Default::default()
        }),
        "sqlite" => {
            let raw = env.get("DB_DATABASE").cloned().unwrap_or_default();
            let path = if raw.is_empty() {
                project
                    .join("database/database.sqlite")
                    .to_string_lossy()
                    .into_owned()
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
    let (eff, tunnel) = prepare(config)?;
    let backend = build_backend(&eff)?;
    Ok(Conn {
        backend,
        _tunnel: tunnel.map(Mutex::new),
    })
}

fn build_backend(config: &DbConfig) -> Result<Backend, String> {
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
            Backend::Mysql(pool)
        }
        "sqlite" => {
            if !Path::new(&config.path).exists() {
                return Err(format!("SQLite file not found: {}", config.path));
            }
            rusqlite::Connection::open(&config.path).map_err(|e| e.to_string())?;
            Backend::Sqlite(config.path.clone())
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
            Backend::Postgres(Mutex::new(client))
        }
        "clickhouse" | "ch" => {
            let host = if config.host.is_empty() {
                "127.0.0.1"
            } else {
                &config.host
            };
            let port = if config.port == 0 { 8123 } else { config.port };
            let conn = Backend::Clickhouse {
                base_url: format!("http://{host}:{port}/"),
                user: if config.username.is_empty() {
                    "default".to_string()
                } else {
                    config.username.clone()
                },
                password: config.password.clone(),
                database: config.database.clone(),
            };
            // Validate eagerly.
            ch_query(&conn, "SELECT 1", 1)?;
            conn
        }
        other => return Err(format!("Unsupported engine: {other}")),
    })
}

/// Try a connection without keeping it (the "Test connection" button).
pub fn test(config: &DbConfig) -> Result<(), String> {
    connect(config).map(|_| ())
}

// ── SSH tunnel (remote databases) ──────────────────────────────
// Shell out to the system `ssh` with a local port-forward; it handles key and
// password auth and is available everywhere. Secrets are fed via SSH_ASKPASS.

fn engine_default_port(engine: &str) -> u16 {
    match engine {
        "mysql" | "mariadb" => 3306,
        "postgres" | "postgresql" | "pgsql" => 5432,
        "clickhouse" | "ch" => 8123,
        _ => 0,
    }
}

fn free_local_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

/// Environment variable carrying the SSH secret to the askpass helper. The
/// secret only ever lives in process memory — never written to disk.
const ASKPASS_ENV: &str = "E_SSH_ASKPASS_SECRET";

/// Write a temporary SSH_ASKPASS helper (mode 0700) that echoes the secret read
/// from an environment variable, so the secret itself is never written to disk.
#[cfg(unix)]
fn write_askpass() -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut path = std::env::temp_dir();
    path.push(format!("e-askpass-{}-{}.sh", std::process::id(), nanos));
    let script = format!("#!/bin/sh\nprintf '%s\\n' \"${ASKPASS_ENV}\"\n");
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    f.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(path)
}

fn wait_port(
    port: u16,
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> Result<(), String> {
    use std::io::Read;
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            let mut err = String::new();
            if let Some(mut s) = child.stderr.take() {
                let _ = s.read_to_string(&mut err);
            }
            let msg = err.trim();
            return Err(if msg.is_empty() {
                "SSH tunnel failed (ssh exited)".to_string()
            } else {
                format!("SSH tunnel failed: {msg}")
            });
        }
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err("SSH tunnel timed out (port did not open)".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

#[cfg(unix)]
fn start_tunnel(config: &DbConfig) -> Result<SshTunnel, String> {
    if config.ssh_host.trim().is_empty() {
        return Err("SSH host is required".to_string());
    }
    if config.ssh_user.trim().is_empty() {
        return Err("SSH user is required".to_string());
    }
    let local_port = free_local_port()?;
    let db_host = if config.host.is_empty() {
        "127.0.0.1".to_string()
    } else {
        config.host.clone()
    };
    let db_port = if config.port == 0 {
        engine_default_port(&config.engine)
    } else {
        config.port
    };
    let ssh_port = if config.ssh_port == 0 {
        22
    } else {
        config.ssh_port
    };

    let mut cmd = std::process::Command::new("ssh");
    cmd.arg("-N")
        .args(["-o", "ExitOnForwardFailure=yes"])
        .args(["-o", "StrictHostKeyChecking=accept-new"])
        .args(["-o", "ConnectTimeout=10"])
        .args(["-o", "ServerAliveInterval=30"])
        .args(["-o", "NumberOfPasswordPrompts=1"])
        .args(["-p", &ssh_port.to_string()])
        .args(["-L", &format!("127.0.0.1:{local_port}:{db_host}:{db_port}")]);

    let password_auth = config.ssh_auth == "password";
    let secret = if password_auth {
        config.ssh_password.clone()
    } else {
        config.ssh_passphrase.clone()
    };
    if password_auth {
        cmd.args(["-o", "PubkeyAuthentication=no"]);
        cmd.args([
            "-o",
            "PreferredAuthentications=password,keyboard-interactive",
        ]);
    } else if !config.ssh_key_path.trim().is_empty() {
        cmd.args(["-i", &expand_tilde(config.ssh_key_path.trim())]);
        cmd.args(["-o", "IdentitiesOnly=yes"]);
    }

    let mut askpass: Option<PathBuf> = None;
    if secret.is_empty() {
        cmd.args(["-o", "BatchMode=yes"]);
    } else {
        let p = write_askpass()?;
        cmd.env("SSH_ASKPASS", &p);
        cmd.env("SSH_ASKPASS_REQUIRE", "force");
        cmd.env("DISPLAY", "localhost:0");
        // The secret travels via the environment, not the on-disk helper.
        cmd.env(ASKPASS_ENV, &secret);
        askpass = Some(p);
    }

    cmd.arg(format!("{}@{}", config.ssh_user, config.ssh_host));
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start ssh: {e}"))?;
    let res = wait_port(local_port, &mut child, std::time::Duration::from_secs(15));
    if let Some(p) = &askpass {
        let _ = std::fs::remove_file(p);
    }
    res?;
    Ok(SshTunnel { child, local_port })
}

#[cfg(not(unix))]
fn start_tunnel(_config: &DbConfig) -> Result<SshTunnel, String> {
    Err("SSH tunnels are only supported on Unix".to_string())
}

/// Resolve a config to what we should actually connect to. When SSH is enabled,
/// start a tunnel and point the connection at the local forwarded port.
fn prepare(config: &DbConfig) -> Result<(DbConfig, Option<SshTunnel>), String> {
    if config.use_ssh && config.engine != "sqlite" {
        let tunnel = start_tunnel(config)?;
        let mut eff = config.clone();
        eff.host = "127.0.0.1".to_string();
        eff.port = tunnel.local_port;
        Ok((eff, Some(tunnel)))
    } else {
        Ok((config.clone(), None))
    }
}

// ── schema ─────────────────────────────────────────────────────

pub fn tables(conn: &Conn) -> Result<Vec<String>, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            c.query("SHOW TABLES").map_err(|e| e.to_string())
        }
        Backend::Sqlite(path) => {
            let c = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
            let mut stmt = c
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        }
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner()); // recover a poisoned lock instead of crashing the app
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
        Backend::Clickhouse { .. } => {
            let res = ch_query(&conn.backend, "SHOW TABLES", MAX_ROWS)?;
            Ok(res
                .rows
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect())
        }
    }
}

// ── ClickHouse over HTTP ───────────────────────────────────────

fn tsv_unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('0') => out.push('\0'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn ch_query(backend: &Backend, sql: &str, max: usize) -> Result<QueryResult, String> {
    let Backend::Clickhouse {
        base_url,
        user,
        password,
        database,
    } = backend
    else {
        return Err("not a ClickHouse connection".into());
    };
    let select = is_select(sql);
    let body = if select {
        format!("{sql}\nFORMAT TabSeparatedWithNames")
    } else {
        sql.to_string()
    };
    let mut url = base_url.clone();
    if !database.is_empty() {
        url.push_str("?database=");
        url.push_str(database);
    }
    let start = Instant::now();
    let resp = ureq::post(&url)
        .set("X-ClickHouse-User", user)
        .set("X-ClickHouse-Key", password)
        .send_string(&body);
    let text = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, r)) => {
            return Err(r
                .into_string()
                .unwrap_or_else(|_| "ClickHouse error".into()))
        }
        Err(e) => return Err(e.to_string()),
    };
    let elapsed_ms = start.elapsed().as_millis() as u64;
    if !select {
        return Ok(QueryResult {
            rows_affected: Some(0),
            elapsed_ms,
            ..Default::default()
        });
    }
    let mut lines = text.lines();
    let columns: Vec<String> = lines
        .next()
        .map(|l| l.split('\t').map(tsv_unescape).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut truncated = false;
    for line in lines {
        if rows.len() >= max {
            truncated = true;
            break;
        }
        let cells: Vec<Option<String>> = line
            .split('\t')
            .map(|c| {
                if c == "\\N" {
                    None
                } else {
                    Some(tsv_unescape(c))
                }
            })
            .collect();
        rows.push(cells);
    }
    Ok(QueryResult {
        columns,
        rows,
        elapsed_ms,
        is_select: true,
        truncated,
        ..Default::default()
    })
}

/// Column metadata for the structure view.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub key: String,
}

/// A foreign-key relationship discovered in the live schema.
#[derive(Clone, Debug, PartialEq)]
pub struct ForeignKey {
    pub table: String,
    pub column: String,
    pub ref_table: String,
    pub ref_column: String,
}

/// All foreign keys in the database (empty for engines without them).
pub fn foreign_keys(conn: &Conn) -> Result<Vec<ForeignKey>, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            let q =
                "SELECT TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
                     FROM information_schema.KEY_COLUMN_USAGE \
                     WHERE REFERENCED_TABLE_NAME IS NOT NULL AND TABLE_SCHEMA = DATABASE()";
            let rows: Vec<(String, String, String, String)> =
                c.query(q).map_err(|e| e.to_string())?;
            Ok(rows
                .into_iter()
                .map(|(table, column, ref_table, ref_column)| ForeignKey {
                    table,
                    column,
                    ref_table,
                    ref_column,
                })
                .collect())
        }
        Backend::Sqlite(path) => {
            let c = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
            let tables: Vec<String> = {
                let mut stmt = c
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?;
                rows.filter_map(|r| r.ok()).collect()
            };
            let mut out = Vec::new();
            for t in tables {
                let q = format!("PRAGMA foreign_key_list(\"{}\")", t.replace('"', "\"\""));
                let Ok(mut stmt) = c.prepare(&q) else {
                    continue;
                };
                // Columns: id, seq, table(ref), from(col), to(ref col), …
                let rows = stmt.query_map([], |r| {
                    Ok(ForeignKey {
                        table: t.clone(),
                        ref_table: r.get::<_, String>(2)?,
                        column: r.get::<_, String>(3)?,
                        ref_column: r.get::<_, String>(4).unwrap_or_default(),
                    })
                });
                if let Ok(rows) = rows {
                    out.extend(rows.filter_map(|r| r.ok()));
                }
            }
            Ok(out)
        }
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner()); // recover a poisoned lock instead of crashing the app
            let res = pg_query(
                &mut client,
                "SELECT tc.table_name, kcu.column_name, ccu.table_name, ccu.column_name \
                 FROM information_schema.table_constraints tc \
                 JOIN information_schema.key_column_usage kcu \
                   ON kcu.constraint_name = tc.constraint_name AND kcu.table_schema = tc.table_schema \
                 JOIN information_schema.constraint_column_usage ccu \
                   ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema \
                 WHERE tc.constraint_type = 'FOREIGN KEY'",
                MAX_ROWS,
            )?;
            Ok(res
                .rows
                .into_iter()
                .map(|r| ForeignKey {
                    table: r.first().cloned().flatten().unwrap_or_default(),
                    column: r.get(1).cloned().flatten().unwrap_or_default(),
                    ref_table: r.get(2).cloned().flatten().unwrap_or_default(),
                    ref_column: r.get(3).cloned().flatten().unwrap_or_default(),
                })
                .collect())
        }
        Backend::Clickhouse { .. } => Ok(Vec::new()),
    }
}

pub fn columns(conn: &Conn, table: &str) -> Result<Vec<ColumnInfo>, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            let q = format!("SHOW COLUMNS FROM `{}`", table.replace('`', "``"));
            let rows: Vec<(String, String, String, String, Option<String>, String)> =
                c.query(q).map_err(|e| e.to_string())?;
            Ok(rows
                .into_iter()
                .map(|(field, ty, null, key, _d, _e)| ColumnInfo {
                    name: field,
                    data_type: ty,
                    nullable: null.eq_ignore_ascii_case("YES"),
                    key,
                })
                .collect())
        }
        Backend::Sqlite(path) => {
            let c = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
            let q = format!("PRAGMA table_info(\"{}\")", table.replace('"', "\"\""));
            let mut stmt = c.prepare(&q).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(ColumnInfo {
                        name: r.get::<_, String>(1)?,
                        data_type: r.get::<_, String>(2).unwrap_or_default(),
                        nullable: r.get::<_, i64>(3).unwrap_or(0) == 0,
                        key: if r.get::<_, i64>(5).unwrap_or(0) > 0 {
                            "PRI".into()
                        } else {
                            String::new()
                        },
                    })
                })
                .map_err(|e| e.to_string())?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        }
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner()); // recover a poisoned lock instead of crashing the app
            let t = table.replace('\'', "''");
            let pk = pg_query(
                &mut client,
                &format!(
                    "SELECT kcu.column_name FROM information_schema.table_constraints tc \
                     JOIN information_schema.key_column_usage kcu \
                       ON kcu.constraint_name = tc.constraint_name AND kcu.table_schema = tc.table_schema \
                     WHERE tc.table_name = '{t}' AND tc.constraint_type = 'PRIMARY KEY'"
                ),
                MAX_ROWS,
            )?;
            let pks: std::collections::HashSet<String> = pk
                .rows
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect();
            let res = pg_query(
                &mut client,
                &format!(
                    "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
                     WHERE table_name = '{t}' ORDER BY ordinal_position"
                ),
                MAX_ROWS,
            )?;
            Ok(res
                .rows
                .into_iter()
                .map(|r| {
                    let name = r.first().cloned().flatten().unwrap_or_default();
                    let data_type = r.get(1).cloned().flatten().unwrap_or_default();
                    let nullable = r
                        .get(2)
                        .cloned()
                        .flatten()
                        .unwrap_or_default()
                        .eq_ignore_ascii_case("YES");
                    let key = if pks.contains(&name) {
                        "PRI".to_string()
                    } else {
                        String::new()
                    };
                    ColumnInfo {
                        name,
                        data_type,
                        nullable,
                        key,
                    }
                })
                .collect())
        }
        Backend::Clickhouse { .. } => {
            let q = format!("DESCRIBE TABLE `{}`", table.replace('`', "``"));
            let res = ch_query(&conn.backend, &q, MAX_ROWS)?;
            // DESCRIBE columns: name, type, default_type, default_expression, …
            Ok(res
                .rows
                .into_iter()
                .map(|r| {
                    let name = r.first().cloned().flatten().unwrap_or_default();
                    let data_type = r.get(1).cloned().flatten().unwrap_or_default();
                    let nullable = data_type.starts_with("Nullable(");
                    ColumnInfo {
                        name,
                        data_type,
                        nullable,
                        key: String::new(),
                    }
                })
                .collect())
        }
    }
}

/// A table index (name, uniqueness, and the columns it covers, in order).
#[derive(Clone, Debug, PartialEq)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub columns: Vec<String>,
}

/// List the indexes on `table`. ClickHouse has no secondary indexes, so it
/// returns an empty list.
pub fn indexes(conn: &Conn, table: &str) -> Result<Vec<IndexInfo>, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            let q = format!("SHOW INDEX FROM `{}`", table.replace('`', "``"));
            let rows: Vec<mysql::Row> = c.query(q).map_err(|e| e.to_string())?;
            Ok(group_indexes(rows.into_iter().filter_map(|mut r| {
                let name: String = r.take("Key_name")?;
                let non_unique: i64 = r.take("Non_unique").unwrap_or(1);
                let col: String = r.take("Column_name").unwrap_or_default();
                Some((name, non_unique == 0, col))
            })))
        }
        Backend::Sqlite(path) => {
            let c = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
            let list_q = format!("PRAGMA index_list(\"{}\")", table.replace('"', "\"\""));
            // index_list columns: seq, name, unique, origin, partial.
            let listed: Vec<(String, bool)> = {
                let mut stmt = c.prepare(&list_q).map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2).unwrap_or(0) != 0))
                    })
                    .map_err(|e| e.to_string())?;
                rows.filter_map(|r| r.ok()).collect()
            };
            let mut out = Vec::new();
            for (name, unique) in listed {
                let info_q = format!("PRAGMA index_info(\"{}\")", name.replace('"', "\"\""));
                let mut stmt = c.prepare(&info_q).map_err(|e| e.to_string())?;
                // index_info columns: seqno, cid, name.
                let cols: Vec<String> = stmt
                    .query_map([], |r| r.get::<_, Option<String>>(2))
                    .map_err(|e| e.to_string())?
                    .filter_map(|r| r.ok().flatten())
                    .collect();
                out.push(IndexInfo {
                    name,
                    unique,
                    columns: cols,
                });
            }
            Ok(out)
        }
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner());
            let t = table.replace('\'', "''");
            let res = pg_query(
                &mut client,
                &format!(
                    "SELECT i.relname, ix.indisunique, a.attname \
                     FROM pg_class t \
                     JOIN pg_index ix ON t.oid = ix.indrelid \
                     JOIN pg_class i ON i.oid = ix.indexrelid \
                     JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(ix.indkey) \
                     WHERE t.relname = '{t}' \
                     ORDER BY i.relname, array_position(ix.indkey, a.attnum)"
                ),
                MAX_ROWS,
            )?;
            Ok(group_indexes(res.rows.into_iter().map(|row| {
                let name = row.first().cloned().flatten().unwrap_or_default();
                let uniq = row.get(1).cloned().flatten().unwrap_or_default();
                let unique = matches!(uniq.as_str(), "t" | "true" | "TRUE");
                let col = row.get(2).cloned().flatten().unwrap_or_default();
                (name, unique, col)
            })))
        }
        Backend::Clickhouse { .. } => Ok(Vec::new()),
    }
}

/// Fold `(index_name, unique, column)` rows (in column order) into per-index
/// [`IndexInfo`], preserving first-seen index order.
fn group_indexes(rows: impl Iterator<Item = (String, bool, String)>) -> Vec<IndexInfo> {
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, IndexInfo> = std::collections::HashMap::new();
    for (name, unique, col) in rows {
        if name.is_empty() {
            continue;
        }
        let entry = map.entry(name.clone()).or_insert_with(|| {
            order.push(name.clone());
            IndexInfo {
                name: name.clone(),
                unique,
                columns: Vec::new(),
            }
        });
        if !col.is_empty() {
            entry.columns.push(col);
        }
    }
    order.into_iter().filter_map(|n| map.remove(&n)).collect()
}

/// Quote an identifier for the given engine.
fn quote_ident(engine: &str, ident: &str) -> String {
    match engine {
        "postgres" | "postgresql" | "pgsql" => format!("\"{}\"", ident.replace('"', "\"\"")),
        "clickhouse" | "ch" => format!("`{}`", ident.replace('`', "``")),
        _ => format!("`{}`", ident.replace('`', "``")),
    }
}

/// Escape a string literal (double single-quotes).
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// Update a single cell, identified by the table's primary-key columns.
/// `pk` is a list of `(column, value)` for the row's primary key.
pub fn update_cell(
    conn: &Conn,
    engine: &str,
    table: &str,
    set_col: &str,
    set_val: Option<&str>,
    pk: &[(String, Option<String>)],
) -> Result<u64, String> {
    if pk.is_empty() {
        return Err("Table has no primary key — cannot edit safely".into());
    }
    let set_expr = match set_val {
        Some(v) => format!("'{}'", esc(v)),
        None => "NULL".to_string(),
    };
    let conds: Vec<String> = pk
        .iter()
        .map(|(c, v)| match v {
            Some(v) => format!("{} = '{}'", quote_ident(engine, c), esc(v)),
            None => format!("{} IS NULL", quote_ident(engine, c)),
        })
        .collect();
    let sql = format!(
        "UPDATE {} SET {} = {} WHERE {}",
        quote_ident(engine, table),
        quote_ident(engine, set_col),
        set_expr,
        conds.join(" AND ")
    );
    let r = query(conn, &sql, 0)?;
    Ok(r.rows_affected.unwrap_or(0))
}

/// Insert a row. `values` maps column name to optional value (`None` = NULL).
/// Returns the number of rows inserted.
pub fn insert_row(
    conn: &Conn,
    engine: &str,
    table: &str,
    values: &[(String, Option<String>)],
) -> Result<u64, String> {
    if values.is_empty() {
        return Err("No columns to insert".into());
    }
    let cols: Vec<String> = values.iter().map(|(c, _)| quote_ident(engine, c)).collect();
    let vals: Vec<String> = values
        .iter()
        .map(|(_, v)| match v {
            Some(v) => format!("'{}'", esc(v)),
            None => "NULL".to_string(),
        })
        .collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_ident(engine, table),
        cols.join(", "),
        vals.join(", ")
    );
    let r = query(conn, &sql, 0)?;
    Ok(r.rows_affected.unwrap_or(0))
}

/// Delete the row(s) matching the given primary-key columns. Refuses an empty
/// predicate so it can never wipe a whole table.
pub fn delete_row(
    conn: &Conn,
    engine: &str,
    table: &str,
    pk: &[(String, Option<String>)],
) -> Result<u64, String> {
    if pk.is_empty() {
        return Err("Table has no primary key — cannot delete safely".into());
    }
    let conds: Vec<String> = pk
        .iter()
        .map(|(c, v)| match v {
            Some(v) => format!("{} = '{}'", quote_ident(engine, c), esc(v)),
            None => format!("{} IS NULL", quote_ident(engine, c)),
        })
        .collect();
    let sql = format!(
        "DELETE FROM {} WHERE {}",
        quote_ident(engine, table),
        conds.join(" AND ")
    );
    let r = query(conn, &sql, 0)?;
    Ok(r.rows_affected.unwrap_or(0))
}

/// Find the foreign-key target `(ref_table, ref_column)` that `table`.`column`
/// points at, if any — for FK-hopping in the data view.
pub fn fk_target(
    conn: &Conn,
    table: &str,
    column: &str,
) -> Result<Option<(String, String)>, String> {
    Ok(foreign_keys(conn)?
        .into_iter()
        .find(|fk| fk.table == table && fk.column == column)
        .map(|fk| (fk.ref_table, fk.ref_column)))
}

/// Browse rows of `table` where `column` equals `value` (or IS NULL). Used for
/// FK-hopping and column filters.
pub fn rows_where(
    conn: &Conn,
    engine: &str,
    table: &str,
    column: &str,
    value: Option<&str>,
    max: usize,
) -> Result<QueryResult, String> {
    let cond = match value {
        Some(v) => format!("{} = '{}'", quote_ident(engine, column), esc(v)),
        None => format!("{} IS NULL", quote_ident(engine, column)),
    };
    let sql = format!(
        "SELECT * FROM {} WHERE {} LIMIT {}",
        quote_ident(engine, table),
        cond,
        max
    );
    query(conn, &sql, max)
}

/// `SELECT * FROM <table> LIMIT <max>` for browsing a table.
pub fn table_data(
    conn: &Conn,
    engine: &str,
    table: &str,
    max: usize,
) -> Result<QueryResult, String> {
    let sql = browse_sql(engine, table, None, None, max, 0);
    query(conn, &sql, max)
}

/// Build a browsing query with an optional `col = value` (or `IS NULL`) filter,
/// sort and pagination.
pub fn browse_sql(
    engine: &str,
    table: &str,
    filter: Option<(&str, Option<&str>)>,
    order_by: Option<(&str, bool)>,
    limit: usize,
    offset: usize,
) -> String {
    let mut sql = format!("SELECT * FROM {}", quote_ident(engine, table));
    if let Some((col, val)) = filter {
        let cond = match val {
            Some(v) => format!("{} = '{}'", quote_ident(engine, col), esc(v)),
            None => format!("{} IS NULL", quote_ident(engine, col)),
        };
        sql.push_str(&format!(" WHERE {cond}"));
    }
    if let Some((col, asc)) = order_by {
        sql.push_str(&format!(
            " ORDER BY {} {}",
            quote_ident(engine, col),
            if asc { "ASC" } else { "DESC" }
        ));
    }
    sql.push_str(&format!(" LIMIT {limit} OFFSET {offset}"));
    sql
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
    match &conn.backend {
        Backend::Mysql(pool) => {
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
        Backend::Sqlite(path) => {
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
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner()); // recover a poisoned lock instead of crashing the app
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
        Backend::Clickhouse { .. } => ch_query(&conn.backend, sql, max),
    }
}

/// The engine identifier for a live connection.
fn backend_engine(conn: &Conn) -> &'static str {
    match &conn.backend {
        Backend::Mysql(_) => "mysql",
        Backend::Sqlite(_) => "sqlite",
        Backend::Postgres(_) => "postgres",
        Backend::Clickhouse { .. } => "clickhouse",
    }
}

/// Run the engine's EXPLAIN on `sql` and return the plan as a result grid.
pub fn explain(conn: &Conn, sql: &str) -> Result<QueryResult, String> {
    let sql = sql.trim().trim_end_matches(';');
    let prefixed = match backend_engine(conn) {
        "sqlite" => format!("EXPLAIN QUERY PLAN {sql}"),
        _ => format!("EXPLAIN {sql}"),
    };
    query(conn, &prefixed, MAX_ROWS)
}

/// Inspect an EXPLAIN result for performance red flags (full table scans /
/// unused indexes), returning human-readable issues. Pure — unit-tested.
pub fn analyze_explain(engine: &str, plan: &QueryResult) -> Vec<String> {
    let col = |name: &str| {
        plan.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
    };
    let mut issues = Vec::new();
    match engine {
        "sqlite" => {
            // EXPLAIN QUERY PLAN: the `detail` column starts with "SCAN" for a
            // full scan, "SEARCH … USING INDEX" when an index is used.
            for row in &plan.rows {
                if let Some(detail) = row.last().and_then(|c| c.as_deref()) {
                    let d = detail.trim();
                    if d.starts_with("SCAN") && !d.contains("USING") {
                        issues.push(format!("Full scan: {d}"));
                    }
                }
            }
        }
        "mysql" => {
            let (ti, tyi, ki) = (col("table"), col("type"), col("key"));
            for row in &plan.rows {
                let get = |i: Option<usize>| i.and_then(|i| row.get(i)).and_then(|c| c.as_deref());
                let table = get(ti).unwrap_or("?");
                let scan_type = get(tyi).unwrap_or("");
                let key = get(ki);
                if scan_type.eq_ignore_ascii_case("ALL") {
                    issues.push(format!("Full table scan on `{table}` (type=ALL, no index)"));
                } else if key.is_none() || key == Some("") {
                    issues.push(format!("No index used on `{table}`"));
                }
            }
        }
        "postgres" => {
            for row in &plan.rows {
                if let Some(line) = row.first().and_then(|c| c.as_deref()) {
                    if let Some(rest) = line.trim_start().strip_prefix("Seq Scan on ") {
                        let table = rest.split_whitespace().next().unwrap_or("?");
                        issues.push(format!("Sequential scan on {table} (no index)"));
                    }
                }
            }
        }
        _ => {}
    }
    issues
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
        Date(y, mo, d, h, mi, s, _us) => {
            Some(format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"))
        }
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

// ── saved queries (per project, in ~/.config/e/queries.json) ───

fn queries_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("e")
            .join("queries.json"),
    )
}

fn read_queries_store() -> serde_json::Value {
    queries_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// A named, saved SQL query.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
}

/// Load the saved queries for a project.
pub fn load_queries(project: &Path) -> Vec<SavedQuery> {
    read_queries_store()
        .get(project.to_string_lossy().as_ref())
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Persist the saved queries for a project.
pub fn save_queries(project: &Path, queries: &[SavedQuery]) -> Result<(), String> {
    let mut store = read_queries_store();
    let obj = store.as_object_mut().ok_or("invalid store")?;
    obj.insert(
        project.to_string_lossy().into_owned(),
        serde_json::to_value(queries).map_err(|e| e.to_string())?,
    );
    let path = queries_path().ok_or("no HOME")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Whether a host/name/path string hints at production.
fn name_hints_prod(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    ["prod", "production", "live"]
        .iter()
        .any(|needle| s.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(engine: &str, host: &str) -> DbConfig {
        let mut c = DbConfig::default();
        c.engine = engine.to_string();
        c.host = host.to_string();
        c
    }

    #[test]
    fn browse_sql_filter_and_sort() {
        assert_eq!(
            browse_sql("sqlite", "posts", None, None, 50, 0),
            "SELECT * FROM `posts` LIMIT 50 OFFSET 0"
        );
        assert_eq!(
            browse_sql("sqlite", "posts", Some(("user_id", Some("1"))), None, 50, 0),
            "SELECT * FROM `posts` WHERE `user_id` = '1' LIMIT 50 OFFSET 0"
        );
        assert_eq!(
            browse_sql("sqlite", "posts", Some(("deleted_at", None)), None, 50, 0),
            "SELECT * FROM `posts` WHERE `deleted_at` IS NULL LIMIT 50 OFFSET 0"
        );
        // filter + sort compose, and values are escaped.
        assert_eq!(
            browse_sql(
                "mysql",
                "t",
                Some(("name", Some("O'x"))),
                Some(("id", false)),
                10,
                20
            ),
            "SELECT * FROM `t` WHERE `name` = 'O''x' ORDER BY `id` DESC LIMIT 10 OFFSET 20"
        );
    }

    #[test]
    fn prod_detection() {
        assert!(!cfg("mysql", "127.0.0.1").looks_like_prod());
        assert!(!cfg("mysql", "localhost").looks_like_prod());
        assert!(cfg("mysql", "db.example.com").looks_like_prod()); // remote
        assert!(cfg("mysql", "prod-db.internal").looks_like_prod()); // name hint
        let mut ssh = cfg("mysql", "127.0.0.1");
        ssh.use_ssh = true;
        ssh.ssh_host = "bastion.example.com".into();
        assert!(ssh.looks_like_prod()); // SSH tunnel
        let mut named = cfg("sqlite", "");
        named.path = "/data/production.sqlite".into();
        assert!(named.looks_like_prod());
    }

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
    fn sqlite_indexes() {
        let dir = std::env::temp_dir().join("e_db_idx_test");
        let _ = std::fs::create_dir_all(&dir);
        let dbfile = dir.join("idx.sqlite");
        let _ = std::fs::remove_file(&dbfile);
        {
            let c = rusqlite::Connection::open(&dbfile).unwrap();
            c.execute_batch(
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, slug TEXT);\
                 CREATE UNIQUE INDEX posts_slug_unique ON posts (slug);\
                 CREATE INDEX posts_user_id_index ON posts (user_id);",
            )
            .unwrap();
        }
        let cfg = DbConfig {
            engine: "sqlite".into(),
            path: dbfile.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let conn = connect(&cfg).unwrap();
        let mut idx = indexes(&conn, "posts").unwrap();
        idx.sort_by(|a, b| a.name.cmp(&b.name));
        let unique = idx.iter().find(|i| i.name == "posts_slug_unique").unwrap();
        assert!(unique.unique);
        assert_eq!(unique.columns, vec!["slug".to_string()]);
        let plain = idx
            .iter()
            .find(|i| i.name == "posts_user_id_index")
            .unwrap();
        assert!(!plain.unique);
        assert_eq!(plain.columns, vec!["user_id".to_string()]);
        let _ = std::fs::remove_file(&dbfile);
    }

    #[test]
    fn sqlite_insert_delete_and_fk() {
        let dir = std::env::temp_dir().join("e_db_rows_test");
        let _ = std::fs::create_dir_all(&dir);
        let dbfile = dir.join("rows.sqlite");
        let _ = std::fs::remove_file(&dbfile);
        {
            let c = rusqlite::Connection::open(&dbfile).unwrap();
            c.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);\
                 CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER \
                     REFERENCES users(id), title TEXT);\
                 INSERT INTO users (id, name) VALUES (1, 'Alice');",
            )
            .unwrap();
        }
        let cfg = DbConfig {
            engine: "sqlite".into(),
            path: dbfile.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let conn = connect(&cfg).unwrap();

        // insert
        let n = insert_row(
            &conn,
            "sqlite",
            "posts",
            &[
                ("id".into(), Some("10".into())),
                ("user_id".into(), Some("1".into())),
                ("title".into(), Some("O'Brien".into())), // exercises escaping
            ],
        )
        .unwrap();
        assert_eq!(n, 1);
        let data = table_data(&conn, "sqlite", "posts", 100).unwrap();
        assert_eq!(data.rows.len(), 1);
        assert_eq!(data.rows[0][2], Some("O'Brien".to_string()));

        // fk_target
        let fk = fk_target(&conn, "posts", "user_id").unwrap();
        assert_eq!(fk, Some(("users".to_string(), "id".to_string())));
        assert_eq!(fk_target(&conn, "posts", "title").unwrap(), None);

        // rows_where (FK-hop target)
        let hop = rows_where(&conn, "sqlite", "users", "id", Some("1"), 100).unwrap();
        assert_eq!(hop.rows.len(), 1);
        assert_eq!(hop.rows[0][1], Some("Alice".to_string()));
        assert_eq!(
            rows_where(&conn, "sqlite", "users", "id", Some("999"), 100)
                .unwrap()
                .rows
                .len(),
            0
        );

        // delete
        let d = delete_row(
            &conn,
            "sqlite",
            "posts",
            &[("id".into(), Some("10".into()))],
        )
        .unwrap();
        assert_eq!(d, 1);
        assert_eq!(
            table_data(&conn, "sqlite", "posts", 100)
                .unwrap()
                .rows
                .len(),
            0
        );

        // delete guard: empty predicate is refused
        assert!(delete_row(&conn, "sqlite", "posts", &[]).is_err());
        let _ = std::fs::remove_file(&dbfile);
    }

    #[test]
    fn sqlite_explain_flags_full_scan() {
        let dir = std::env::temp_dir().join("e_db_explain_test");
        let _ = std::fs::create_dir_all(&dir);
        let dbfile = dir.join("explain.sqlite");
        let _ = std::fs::remove_file(&dbfile);
        {
            let c = rusqlite::Connection::open(&dbfile).unwrap();
            c.execute_batch(
                "CREATE TABLE t (id INTEGER PRIMARY KEY, email TEXT);\
                 INSERT INTO t (email) VALUES ('a@x'), ('b@x');",
            )
            .unwrap();
        }
        let cfg = DbConfig {
            engine: "sqlite".into(),
            path: dbfile.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let conn = connect(&cfg).unwrap();
        // No index on email → full scan.
        let plan = explain(&conn, "SELECT * FROM t WHERE email = 'a@x'").unwrap();
        let issues = analyze_explain("sqlite", &plan);
        assert!(
            issues.iter().any(|i| i.contains("Full scan")),
            "expected a full-scan issue, got {issues:?}"
        );
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
