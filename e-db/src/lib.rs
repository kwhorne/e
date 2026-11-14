//! Minimal multi-engine database access for the editor's Database panel.
//!
//! Supports MySQL/MariaDB, PostgreSQL and SQLite using blocking drivers. The
//! design mirrors the Conductor database panel: connections are described by a
//! [`DbConfig`], opened into a live [`Conn`], and queried into a [`QueryResult`]
//! whose cells are all stringified for display in a grid.

mod classify;
pub mod history;

pub use classify::{is_destructive, is_read_only};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const MAX_ROWS: usize = 1000;

/// A saved database connection (per project).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DbConfig {
    /// Stable identity, assigned on first save and never reused.
    ///
    /// Secrets are keyed by this rather than by `label`, so renaming a
    /// connection doesn't orphan its entry in the credential store.
    #[serde(default)]
    pub id: String,
    /// Whether this connection's secrets live in the OS credential store. When
    /// true the secret fields below are blank on disk and filled in on load.
    #[serde(default)]
    pub secrets_in_keychain: bool,
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
    /// TLS to the database server: `""` (auto), `disable`, `require` or
    /// `verify`. See [`TlsMode`].
    #[serde(default)]
    pub tls: String,
    /// A CA certificate (PEM file) to trust besides the system's, for a
    /// server whose certificate a private CA signed (RDS, Cloud SQL, …).
    #[serde(default)]
    pub tls_ca: String,
}

/// How a connection uses TLS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Encrypt a direct connection to a host that isn't local when the
    /// server offers it, without checking its certificate (libpq's `prefer`;
    /// on ClickHouse, HTTPS on its TLS ports 8443 and 443). Local hosts and
    /// SSH tunnels, which are encrypted already, go without.
    Auto,
    /// Never.
    Disable,
    /// Always, but trust whatever certificate the server shows.
    Require,
    /// Always, with the certificate checked against the system's CAs (and
    /// `tls_ca`) and the host name.
    Verify,
}

impl TlsMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "disable" | "off" | "false" => TlsMode::Disable,
            "require" => TlsMode::Require,
            "verify" | "verify-full" | "verify-ca" | "verify_identity" | "verify_ca" => {
                TlsMode::Verify
            }
            _ => TlsMode::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TlsMode::Auto => "",
            TlsMode::Disable => "disable",
            TlsMode::Require => "require",
            TlsMode::Verify => "verify",
        }
    }
}

/// Whether `host` is this machine (or a local development domain).
pub fn is_local_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    matches!(
        host.as_str(),
        "" | "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal"
    ) || host.ends_with(".test")
        || host.ends_with(".localhost")
}

impl DbConfig {
    /// Heuristic: does this look like a production database? Remote/SSH targets
    /// and names containing prod/production/live are treated as production, so
    /// the editor can default them to read-only and warn before writes.
    /// Classify the connection's environment for labelling and write guards.
    /// Production name hints win; then staging hints; then locality of the host.
    pub fn environment(&self) -> Environment {
        if name_hints_prod(&self.label)
            || name_hints_prod(&self.database)
            || name_hints_prod(&self.host)
            || name_hints_prod(&self.path)
        {
            return Environment::Production;
        }
        if name_hints_staging(&self.label)
            || name_hints_staging(&self.database)
            || name_hints_staging(&self.host)
            || name_hints_staging(&self.path)
        {
            return Environment::Staging;
        }
        if self.engine == "sqlite" {
            return Environment::Local;
        }
        if self.use_ssh && !self.ssh_host.is_empty() {
            return Environment::Staging; // remote via tunnel, no hint
        }
        if is_local_host(&self.host) {
            Environment::Local
        } else {
            Environment::Staging
        }
    }

    pub fn tls_mode(&self) -> TlsMode {
        TlsMode::parse(&self.tls)
    }

    /// Whether the auto mode should try TLS: a direct connection to a host
    /// that isn't this machine.
    fn tls_worth_trying(&self) -> bool {
        !self.use_ssh && !is_local_host(&self.host)
    }

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
        agent: ureq::Agent,
        base_url: String,
        user: String,
        password: String,
        database: String,
    },
}

/// A live connection, optionally keeping an SSH tunnel alive for its lifetime.
pub struct Conn {
    backend: Backend,
    /// Refuse anything but reads (see [`Conn::set_read_only`]).
    read_only: AtomicBool,
    /// Whether the server-side session may still be in read-only mode, so a
    /// connection switched back to read-write knows to undo it.
    session_ro: AtomicBool,
    /// Dropped (killing the `ssh` child) when the connection is dropped.
    _tunnel: Option<Mutex<SshTunnel>>,
    /// How to stop a PostgreSQL query that is running: taken when the
    /// connection opens, since the client itself is locked by the query.
    pg_cancel: Option<PgCancel>,
    /// The SQLite statements running now (each query opens the file
    /// afresh), by a number of their own, so Cancel can interrupt them.
    sqlite_running: Mutex<Vec<(u64, rusqlite::InterruptHandle)>>,
    /// The MySQL statements running now: the server's id of the pooled
    /// connection each runs on, so Cancel can `KILL QUERY` it.
    mysql_running: Mutex<Vec<(u64, u32)>>,
}

/// While alive, the MySQL connection it was made for can be stopped by
/// [`cancel`].
struct MysqlRunning<'a> {
    conn: &'a Conn,
    id: u64,
}

impl Drop for MysqlRunning<'_> {
    fn drop(&mut self) {
        if let Ok(mut r) = self.conn.mysql_running.lock() {
            r.retain(|(id, _)| *id != self.id);
        }
    }
}

/// While alive, the SQLite connection it was made for can be interrupted
/// by [`cancel`].
struct SqliteRunning<'a> {
    conn: &'a Conn,
    id: u64,
}

impl Drop for SqliteRunning<'_> {
    fn drop(&mut self) {
        if let Ok(mut r) = self.conn.sqlite_running.lock() {
            r.retain(|(id, _)| *id != self.id);
        }
    }
}

/// A PostgreSQL connection's cancel key, and the TLS it was opened with
/// (the cancel request is a connection of its own).
struct PgCancel {
    token: postgres::CancelToken,
    tls: Option<postgres_native_tls::MakeTlsConnector>,
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
            // Laravel's `mysql` connection trusts this CA when it is set.
            tls: env
                .get("MYSQL_ATTR_SSL_CA")
                .filter(|ca| !ca.trim().is_empty())
                .map(|_| "verify".to_string())
                .unwrap_or_default(),
            tls_ca: env
                .get("MYSQL_ATTR_SSL_CA")
                .filter(|ca| !ca.trim().is_empty())
                .map(|ca| project.join(ca.trim()).to_string_lossy().into_owned())
                .unwrap_or_default(),
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
            // libpq's sslmode, as Laravel's `pgsql` connection passes it.
            tls: env
                .get("DB_SSLMODE")
                .map(|m| match m.trim() {
                    "disable" => "disable",
                    "require" => "require",
                    "verify-ca" | "verify-full" => "verify",
                    _ => "",
                })
                .unwrap_or_default()
                .to_string(),
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
    let (backend, pg_cancel) = build_backend(&eff, config)?;
    Ok(Conn {
        backend,
        read_only: AtomicBool::new(false),
        session_ro: AtomicBool::new(false),
        _tunnel: tunnel.map(Mutex::new),
        pg_cancel,
        sqlite_running: Mutex::new(Vec::new()),
        mysql_running: Mutex::new(Vec::new()),
    })
}

/// Stop the query running on `conn`, on the server. `Ok(true)` when the
/// server was asked; `Ok(false)` for an engine `e` can't cancel yet (the
/// query then runs to its end in the background). The cancelled query fails
/// with PostgreSQL's "canceling statement due to user request", and the
/// connection is free again.
pub fn cancel(conn: &Conn) -> Result<bool, String> {
    if let Backend::Mysql(pool) = &conn.backend {
        // From another connection of the pool: `KILL QUERY` ends the
        // statement ("Query execution was interrupted"; a `SLEEP` returns 1)
        // and leaves its connection open.
        use mysql::prelude::Queryable;
        let ids: Vec<u32> = conn
            .mysql_running
            .lock()
            .map_err(|e| e.to_string())?
            .iter()
            .map(|(_, server_id)| *server_id)
            .collect();
        if !ids.is_empty() {
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            for id in ids {
                c.query_drop(format!("KILL QUERY {id}"))
                    .map_err(|e| e.to_string())?;
            }
        }
        return Ok(true);
    }
    if let Backend::Sqlite(_) = conn.backend {
        // Interrupted, a statement ends with SQLite's "interrupted".
        let running = conn.sqlite_running.lock().map_err(|e| e.to_string())?;
        for (_, handle) in running.iter() {
            handle.interrupt();
        }
        return Ok(true);
    }
    let Some(c) = &conn.pg_cancel else {
        return Ok(false);
    };
    match &c.tls {
        Some(tls) => c.token.cancel_query(tls.clone()),
        None => c.token.cancel_query(postgres::NoTls),
    }
    .map_err(pg_err)?;
    Ok(true)
}

impl Conn {
    /// Make this connection read-only (or writable again). While read-only,
    /// [`query`] and [`execute_transaction`] refuse any statement that
    /// [`is_read_only`] can't vouch for, and the server is asked to enforce it
    /// too where that's cheap: a read-only session on PostgreSQL and MySQL, a
    /// read-only open on SQLite and `readonly=1` on ClickHouse — so a write the
    /// classifier misses (a side-effecting function, say) still fails.
    pub fn set_read_only(&self, on: bool) {
        self.read_only.store(on, Ordering::SeqCst);
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    /// Refuse `statements` if this connection is read-only and any of them
    /// might write.
    fn guard(&self, statements: &[&str]) -> Result<(), String> {
        if self.is_read_only() {
            if let Some(s) = statements.iter().find(|s| !is_read_only(s)) {
                let head: String = s.trim().chars().take(60).collect();
                return Err(format!(
                    "Read-only connection: refusing a statement that may write ({head}). \
                     Toggle read-only off in the Database panel to run it."
                ));
            }
        }
        Ok(())
    }

    /// Open the SQLite file, read-only while the connection is.
    fn sqlite(&self, path: &str) -> Result<rusqlite::Connection, String> {
        sqlite_open(path, self.is_read_only())
    }

    /// Note the pooled MySQL connection `c` as running a statement, until
    /// the returned guard is dropped, so [`cancel`] can stop it.
    fn mysql_running(&self, c: &mysql::PooledConn) -> MysqlRunning<'_> {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut r) = self.mysql_running.lock() {
            r.push((id, c.connection_id()));
        }
        MysqlRunning { conn: self, id }
    }

    /// Note `c` as running a statement, until the returned guard is dropped,
    /// so [`cancel`] can interrupt it.
    fn sqlite_running(&self, c: &rusqlite::Connection) -> SqliteRunning<'_> {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut r) = self.sqlite_running.lock() {
            r.push((id, c.get_interrupt_handle()));
        }
        SqliteRunning { conn: self, id }
    }

    /// Bring a pooled MySQL session in line with the read-only flag.
    fn sync_mysql_session(&self, c: &mut mysql::PooledConn) -> Result<(), String> {
        use mysql::prelude::Queryable;
        if self.is_read_only() {
            self.session_ro.store(true, Ordering::SeqCst);
            c.query_drop("SET SESSION TRANSACTION READ ONLY")
                .map_err(|e| e.to_string())
        } else if self.session_ro.load(Ordering::SeqCst) {
            // Some pooled session may still be read-only; this one won't be.
            c.query_drop("SET SESSION TRANSACTION READ WRITE")
                .map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }

    /// Bring the PostgreSQL session in line with the read-only flag.
    fn sync_pg_session(&self, client: &mut postgres::Client) -> Result<(), String> {
        let want = self.is_read_only();
        if self.session_ro.load(Ordering::SeqCst) == want {
            return Ok(());
        }
        client
            .batch_execute(if want {
                "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY"
            } else {
                "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE"
            })
            .map_err(pg_err)?;
        self.session_ro.store(want, Ordering::SeqCst);
        Ok(())
    }
}

/// How long a statement waits for a lock another process holds on the file
/// (the app's queue worker writing, say) before giving up with "database is
/// locked". rusqlite sets the same five seconds when it opens a connection;
/// said here so `e` doesn't depend on that default.
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

fn sqlite_open(path: &str, read_only: bool) -> Result<rusqlite::Connection, String> {
    let c = if read_only {
        use rusqlite::OpenFlags;
        rusqlite::Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
    } else {
        rusqlite::Connection::open(path)
    }
    .map_err(|e| e.to_string())?;
    c.busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(|e| e.to_string())?;
    Ok(c)
}

/// A TLS connector for `mode`: checking certificates only for `Verify`, and
/// trusting `ca` (a PEM file) besides the system's CAs when one is given.
fn tls_connector(mode: TlsMode, ca: &str) -> Result<native_tls::TlsConnector, String> {
    let mut b = native_tls::TlsConnector::builder();
    if mode != TlsMode::Verify {
        b.danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }
    if !ca.trim().is_empty() {
        let pem = std::fs::read(ca.trim())
            .map_err(|e| format!("reading the CA certificate {}: {e}", ca.trim()))?;
        let cert = native_tls::Certificate::from_pem(&pem)
            .map_err(|e| format!("the CA certificate {} isn't PEM: {e}", ca.trim()))?;
        b.add_root_certificate(cert);
    }
    b.build().map_err(|e| e.to_string())
}

/// MySQL's TLS options for `config`, or `None` to go without.
fn mysql_ssl(config: &DbConfig) -> Option<mysql::SslOpts> {
    let mode = config.tls_mode();
    let ssl = mysql::SslOpts::default();
    let ssl = if config.tls_ca.trim().is_empty() {
        ssl
    } else {
        ssl.with_root_cert_path(Some(std::path::PathBuf::from(config.tls_ca.trim())))
    };
    match mode {
        TlsMode::Disable => None,
        TlsMode::Auto if !config.tls_worth_trying() => None,
        TlsMode::Auto | TlsMode::Require => Some(
            ssl.with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true),
        ),
        // Through a tunnel the server is reached as 127.0.0.1, which its
        // certificate doesn't name, and the driver can't be told the real
        // name: the chain is still checked.
        TlsMode::Verify => Some(ssl.with_danger_skip_domain_validation(config.use_ssh)),
    }
}

/// `eff` is what to connect to (through an SSH tunnel, the local end of
/// it); `config` is the connection as saved, whose host the server's
/// certificate names.
fn build_backend(eff: &DbConfig, config: &DbConfig) -> Result<(Backend, Option<PgCancel>), String> {
    let mut pg_cancel: Option<PgCancel> = None;
    let mut cancel_tls: Option<postgres_native_tls::MakeTlsConnector> = None;
    let tunnelled = eff.host != config.host || eff.port != config.port;
    let mode = config.tls_mode();
    let backend = match eff.engine.as_str() {
        "mysql" => {
            let config = eff;
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
            let ssl = mysql_ssl(config);
            let auto = mode == TlsMode::Auto && ssl.is_some();
            // On 127.0.0.1 the driver switches to the server's Unix socket,
            // where TLS doesn't apply; TLS asked for must not go missing that
            // way.
            let opts = opts.prefer_socket(ssl.is_none());
            let open = |opts: mysql::OptsBuilder| -> Result<mysql::Pool, mysql::Error> {
                let pool = mysql::Pool::new(opts)?;
                let _ = pool.get_conn()?;
                Ok(pool)
            };
            let pool = match open(opts.clone().ssl_opts(ssl)) {
                Ok(pool) => pool,
                // Auto: a server without TLS is reached without it.
                Err(mysql::Error::DriverError(mysql::DriverError::TlsNotSupported)) if auto => {
                    open(opts).map_err(|e| e.to_string())?
                }
                Err(e) => return Err(e.to_string()),
            };
            Backend::Mysql(pool)
        }
        "sqlite" => {
            if !Path::new(&config.path).exists() {
                return Err(format!("SQLite file not found: {}", config.path));
            }
            sqlite_open(&config.path, false)?;
            Backend::Sqlite(config.path.clone())
        }
        "postgres" | "postgresql" | "pgsql" => {
            let host_of = |c: &DbConfig| {
                if c.host.is_empty() {
                    "127.0.0.1".to_string()
                } else {
                    c.host.clone()
                }
            };
            let use_tls = match mode {
                TlsMode::Disable => false,
                TlsMode::Auto => config.tls_worth_trying(),
                TlsMode::Require | TlsMode::Verify => true,
            };
            let mut pg = postgres::Config::new();
            if use_tls && tunnelled {
                // Reach the tunnel, but check the certificate against the
                // name of the server behind it, not 127.0.0.1.
                pg.host(&host_of(config))
                    .hostaddr(std::net::IpAddr::from([127, 0, 0, 1]));
            } else {
                pg.host(&host_of(eff));
            }
            pg.port(if eff.port == 0 { 5432 } else { eff.port })
                .user(&eff.username);
            if !eff.password.is_empty() {
                pg.password(&eff.password);
            }
            if !eff.database.is_empty() {
                pg.dbname(&eff.database);
            }
            let client = if use_tls {
                pg.ssl_mode(if mode == TlsMode::Auto {
                    postgres::config::SslMode::Prefer
                } else {
                    postgres::config::SslMode::Require
                });
                let tls =
                    postgres_native_tls::MakeTlsConnector::new(tls_connector(mode, &eff.tls_ca)?);
                cancel_tls = Some(tls.clone());
                pg.connect(tls)
            } else {
                pg.ssl_mode(postgres::config::SslMode::Disable);
                pg.connect(postgres::NoTls)
            }
            .map_err(pg_err)?;
            pg_cancel = Some(PgCancel {
                token: client.cancel_token(),
                tls: cancel_tls,
            });
            Backend::Postgres(Mutex::new(client))
        }
        "clickhouse" | "ch" => {
            let host = if config.host.is_empty() {
                "127.0.0.1"
            } else {
                &config.host
            };
            let port = if config.port == 0 { 8123 } else { config.port };
            // ClickHouse serves HTTP and HTTPS on different ports (8123 and
            // 8443), so auto can't try one and fall back: it follows the port.
            let https = match mode {
                TlsMode::Disable => false,
                TlsMode::Require | TlsMode::Verify => true,
                TlsMode::Auto => matches!(port, 8443 | 443),
            };
            let mut agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10));
            if https {
                agent = agent.tls_connector(Arc::new(tls_connector(mode, &config.tls_ca)?));
            }
            if tunnelled {
                // The URL names the server (for its certificate and the Host
                // header); the bytes go to the tunnel.
                let local = std::net::SocketAddr::from(([127, 0, 0, 1], eff.port));
                agent = agent.resolver(move |_: &str| Ok(vec![local]));
            }
            let scheme = if https { "https" } else { "http" };
            let conn = Backend::Clickhouse {
                agent: agent.build(),
                base_url: format!("{scheme}://{host}:{port}/"),
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
    };
    Ok((backend, pg_cancel))
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

/// Refuse an SSH user or host that `ssh` could take for an option (a host of
/// `-oProxyCommand=…` runs a command), or that would change the destination
/// (whitespace, a second `@`).
#[cfg_attr(not(unix), allow(dead_code))]
fn ssh_target_ok(user: &str, host: &str) -> Result<(), String> {
    for (what, v) in [("user", user), ("host", host)] {
        if v.starts_with('-') || v.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!("Invalid SSH {what}: {v:?}"));
        }
    }
    if host.contains('@') {
        return Err(format!("Invalid SSH host: {host:?}"));
    }
    Ok(())
}

#[cfg(unix)]
fn start_tunnel(config: &DbConfig) -> Result<SshTunnel, String> {
    if config.ssh_host.trim().is_empty() {
        return Err("SSH host is required".to_string());
    }
    if config.ssh_user.trim().is_empty() {
        return Err("SSH user is required".to_string());
    }
    ssh_target_ok(&config.ssh_user, &config.ssh_host)?;
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

    // `--` ends option parsing, so the destination can never be read as one.
    cmd.arg("--")
        .arg(format!("{}@{}", config.ssh_user, config.ssh_host));
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
    if let Err(e) = res {
        // A tunnel that never came up must not live on, holding the port.
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    // Drain stderr for the tunnel's lifetime: unread, a long session's
    // "channel open failed" messages fill the pipe and stall ssh.
    if let Some(mut err) = child.stderr.take() {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut err, &mut std::io::sink());
        });
    }
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

/// List the views in the database (empty if the engine has none).
pub fn views(conn: &Conn) -> Result<Vec<String>, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            let rows: Vec<(String, String)> = c
                .query("SHOW FULL TABLES WHERE Table_type = 'VIEW'")
                .map_err(|e| e.to_string())?;
            Ok(rows.into_iter().map(|(name, _)| name).collect())
        }
        Backend::Sqlite(path) => {
            let c = conn.sqlite(path)?;
            let mut stmt = c
                .prepare("SELECT name FROM sqlite_master WHERE type='view' ORDER BY name")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        }
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner());
            let res = pg_query(
                &mut client,
                "SELECT table_name FROM information_schema.views \
                 WHERE table_schema NOT IN ('pg_catalog','information_schema') ORDER BY table_name",
                MAX_ROWS,
            )?;
            Ok(res
                .rows
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect())
        }
        Backend::Clickhouse { .. } => Ok(Vec::new()),
    }
}

/// The `CREATE TABLE` DDL for a table (read-only display / copy). PostgreSQL has
/// no single-call DDL, so a basic definition is reconstructed from its columns.
pub fn table_ddl(conn: &Conn, table: &str) -> Result<String, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            let q = format!("SHOW CREATE TABLE `{}`", table.replace('`', "``"));
            let row: Option<(String, String)> = c.query_first(q).map_err(|e| e.to_string())?;
            Ok(row.map(|(_, ddl)| ddl).unwrap_or_default())
        }
        Backend::Sqlite(path) => {
            let c = conn.sqlite(path)?;
            c.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name = ?1",
                [table],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())
        }
        Backend::Clickhouse { .. } => {
            let q = format!("SHOW CREATE TABLE `{}`", table.replace('`', "``"));
            let res = ch_query(&conn.backend, &q, 1)?;
            Ok(res
                .rows
                .into_iter()
                .next()
                .and_then(|r| r.into_iter().next().flatten())
                .unwrap_or_default())
        }
        Backend::Postgres(_) => {
            // Reconstruct a basic CREATE TABLE from the column list.
            let cols = columns(conn, table)?;
            let mut lines = Vec::new();
            for c in &cols {
                let null = if c.nullable { "" } else { " NOT NULL" };
                let pk = if c.key == "PRI" { " PRIMARY KEY" } else { "" };
                lines.push(format!(
                    "    {} {}{}{}",
                    quote_ident("postgres", &c.name),
                    c.data_type,
                    null,
                    pk
                ));
            }
            Ok(format!(
                "CREATE TABLE {} (\n{}\n);",
                quote_ident("postgres", table),
                lines.join(",\n")
            ))
        }
    }
}

/// The `CREATE VIEW`/definition SQL for a view (read-only display).
pub fn view_definition(conn: &Conn, view: &str) -> Result<String, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            let q = format!("SHOW CREATE VIEW `{}`", view.replace('`', "``"));
            let row: Option<(String, String, String, String)> =
                c.query_first(q).map_err(|e| e.to_string())?;
            Ok(row.map(|(_, def, _, _)| def).unwrap_or_default())
        }
        Backend::Sqlite(path) => {
            let c = conn.sqlite(path)?;
            c.query_row(
                "SELECT sql FROM sqlite_master WHERE type='view' AND name = ?1",
                [view],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())
        }
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner());
            let v = quote_literal("postgres", &quote_ident("postgres", view));
            let res = pg_query(
                &mut client,
                &format!("SELECT pg_get_viewdef({v}::regclass, true)"),
                1,
            )?;
            Ok(res
                .rows
                .into_iter()
                .next()
                .and_then(|r| r.into_iter().next().flatten())
                .unwrap_or_default())
        }
        Backend::Clickhouse { .. } => Ok(String::new()),
    }
}

pub fn tables(conn: &Conn) -> Result<Vec<String>, String> {
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            c.query("SHOW TABLES").map_err(|e| e.to_string())
        }
        Backend::Sqlite(path) => {
            let c = conn.sqlite(path)?;
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
    ch_query_as(backend, sql, max, false)
}

/// [`ch_query`], with ClickHouse itself refusing writes when `read_only`.
fn ch_query_as(
    backend: &Backend,
    sql: &str,
    max: usize,
    read_only: bool,
) -> Result<QueryResult, String> {
    let Backend::Clickhouse {
        agent,
        base_url,
        user,
        password,
        database,
    } = backend
    else {
        return Err("not a ClickHouse connection".into());
    };
    let select = classify::returns_rows(sql);
    let body = if select {
        format!("{sql}\nFORMAT TabSeparatedWithNames")
    } else {
        sql.to_string()
    };
    let mut url = base_url.clone();
    let mut params: Vec<String> = Vec::new();
    if !database.is_empty() {
        params.push(format!("database={}", url_encode(database)));
    }
    if read_only {
        params.push("readonly=1".into());
    }
    // The server stops making rows past the ones shown (it may finish the
    // block it is in; the reader below cuts there).
    if select && max != usize::MAX {
        params.push(format!("max_result_rows={}", max + 1));
        params.push("result_overflow_mode=break".into());
    }
    if !params.is_empty() {
        url.push('?');
        url.push_str(&params.join("&"));
    }
    let start = Instant::now();
    let resp = agent
        .post(&url)
        .set("X-ClickHouse-User", user)
        .set("X-ClickHouse-Key", password)
        .send_string(&body);
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => {
            return Err(r
                .into_string()
                .unwrap_or_else(|_| "ClickHouse error".into()))
        }
        Err(e) => return Err(e.to_string()),
    };
    if !select {
        let _ = resp.into_string();
        return Ok(QueryResult {
            rows_affected: Some(0),
            elapsed_ms: start.elapsed().as_millis() as u64,
            ..Default::default()
        });
    }
    // Read row by row and stop at the cut, rather than the whole answer into
    // one string first. (TSV escapes newlines inside values, so a line is a
    // row.)
    use std::io::BufRead;
    let mut lines = std::io::BufReader::new(resp.into_reader()).lines();
    let columns: Vec<String> = match lines.next() {
        Some(l) => l
            .map_err(|e| e.to_string())?
            .split('\t')
            .map(tsv_unescape)
            .collect(),
        None => Vec::new(),
    };
    let mut rows = Vec::new();
    let mut truncated = false;
    for line in lines {
        let line = line.map_err(|e| e.to_string())?;
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
        elapsed_ms: start.elapsed().as_millis() as u64,
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
            let c = conn.sqlite(path)?;
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
                                                                          // Metadata, not data: every foreign key, not the first MAX_ROWS.
            let res = pg_query(&mut client, PG_FOREIGN_KEYS, usize::MAX)?;
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
            let c = conn.sqlite(path)?;
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
            let t = quote_literal("postgres", table);
            let schema = pg_table_schema(table);
            let pk = pg_query(
                &mut client,
                &format!(
                    "SELECT kcu.column_name FROM information_schema.table_constraints tc \
                     JOIN information_schema.key_column_usage kcu \
                       ON kcu.constraint_name = tc.constraint_name AND kcu.table_schema = tc.table_schema \
                     WHERE tc.table_name = {t} AND tc.table_schema = {schema} \
                       AND tc.constraint_type = 'PRIMARY KEY'"
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
                     WHERE table_name = {t} AND table_schema = {schema} ORDER BY ordinal_position"
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

/// PostgreSQL's foreign keys between the tables the panel shows by bare name.
///
/// The catalog rather than `information_schema`: there, a constraint's columns
/// can only be matched up by constraint name, which repeats across tables, so a
/// composite key came back as every pairing of its columns, and
/// `constraint_column_usage` leaves out tables the user doesn't own. Here the
/// key's columns pair up by position (`conkey`/`confkey`).
///
/// A bare name means the table [`pg_table_schema`] picks: the one
/// `search_path` reaches, else the first schema with one by that name. Keys
/// from or to a namesake in another schema would send a hop to the wrong
/// table, so they're left out.
const PG_FOREIGN_KEYS: &str = "\
    WITH shown AS ( \
      SELECT c.oid FROM ( \
        SELECT c.oid, row_number() OVER ( \
                 PARTITION BY c.relname \
                 ORDER BY pg_table_is_visible(c.oid) DESC, n.nspname) AS pick \
          FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r', 'p') \
           AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
           AND n.nspname NOT LIKE 'pg_toast%' \
      ) c WHERE c.pick = 1 \
    ) \
    SELECT cl.relname, a.attname, rcl.relname, ra.attname \
      FROM pg_constraint con \
      JOIN pg_class cl ON cl.oid = con.conrelid \
      JOIN pg_class rcl ON rcl.oid = con.confrelid \
      CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(col, refcol, ord) \
      JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.col \
      JOIN pg_attribute ra ON ra.attrelid = con.confrelid AND ra.attnum = k.refcol \
     WHERE con.contype = 'f' \
       AND con.conrelid IN (SELECT oid FROM shown) \
       AND con.confrelid IN (SELECT oid FROM shown) \
     ORDER BY cl.relname, con.conname, k.ord";

/// SQL for the schema a bare PostgreSQL table name refers to.
///
/// The panel lists tables from every user schema by bare name and browses them
/// as `SELECT * FROM "name"`, which resolves through `search_path`. Metadata has
/// to come from that same table — matching on the name alone mixes in the
/// columns and indexes of a namesake in another schema. A table outside
/// `search_path` falls back to the first schema that has one by that name.
fn pg_table_schema(table: &str) -> String {
    let name = quote_literal("postgres", table);
    let regclass = quote_literal("postgres", &quote_ident("postgres", table));
    format!(
        "COALESCE(\
           (SELECT n.nspname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.oid = to_regclass({regclass})), \
           (SELECT table_schema FROM information_schema.tables WHERE table_name = {name} \
             ORDER BY table_schema LIMIT 1))"
    )
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
            let c = conn.sqlite(path)?;
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
            let t = quote_literal("postgres", table);
            let schema = pg_table_schema(table);
            let res = pg_query(
                &mut client,
                &format!(
                    "SELECT i.relname, ix.indisunique, a.attname \
                     FROM pg_class t \
                     JOIN pg_namespace n ON n.oid = t.relnamespace \
                     JOIN pg_index ix ON t.oid = ix.indrelid \
                     JOIN pg_class i ON i.oid = ix.indexrelid \
                     JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(ix.indkey) \
                     WHERE t.relname = {t} AND n.nspname = {schema} \
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

/// Run several write statements in a single transaction (all-or-nothing),
/// returning the total rows affected. On any error the transaction is rolled
/// back. ClickHouse has no transactions, so statements run sequentially
/// (best-effort) — a mid-way failure leaves earlier statements applied.
pub fn execute_transaction(conn: &Conn, statements: &[String]) -> Result<u64, String> {
    if statements.is_empty() {
        return Ok(0);
    }
    conn.guard(&statements.iter().map(String::as_str).collect::<Vec<_>>())?;
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            conn.sync_mysql_session(&mut c)?;
            let _running = conn.mysql_running(&c);
            let mut tx = c
                .start_transaction(mysql::TxOpts::default())
                .map_err(|e| e.to_string())?;
            let mut affected = 0u64;
            for s in statements {
                tx.query_drop(s.as_str()).map_err(|e| e.to_string())?;
                affected += tx.affected_rows();
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(affected)
        }
        Backend::Sqlite(path) => {
            let mut c = conn.sqlite(path)?;
            let _running = conn.sqlite_running(&c);
            let tx = c.transaction().map_err(|e| e.to_string())?;
            let mut affected = 0u64;
            for s in statements {
                affected += tx.execute(s.as_str(), []).map_err(|e| e.to_string())? as u64;
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(affected)
        }
        Backend::Postgres(m) => {
            let mut client = m.lock().unwrap_or_else(|e| e.into_inner());
            conn.sync_pg_session(&mut client)?;
            let mut tx = client.transaction().map_err(pg_err)?;
            let mut affected = 0u64;
            for s in statements {
                affected += tx.execute(s.as_str(), &[]).map_err(pg_err)?;
            }
            tx.commit().map_err(pg_err)?;
            Ok(affected)
        }
        Backend::Clickhouse { .. } => {
            for s in statements {
                ch_query_as(&conn.backend, s, 0, conn.is_read_only())?;
            }
            Ok(0)
        }
    }
}

/// Quote an identifier for the given engine.
pub fn quote_ident(engine: &str, ident: &str) -> String {
    match engine {
        "postgres" | "postgresql" | "pgsql" => format!("\"{}\"", ident.replace('"', "\"\"")),
        "clickhouse" | "ch" => format!("`{}`", ident.replace('`', "``")),
        _ => format!("`{}`", ident.replace('`', "``")),
    }
}

/// Quote `s` as a string literal for `engine`.
///
/// Doubling `'` is enough only where `\` is an ordinary character. MySQL and
/// ClickHouse treat it as an escape, so a value ending in `\` would otherwise
/// swallow the closing quote and let the rest of the statement out of the
/// string. PostgreSQL is safe with standard strings, but a server running
/// `standard_conforming_strings = off` behaves like MySQL — so a value with a
/// backslash goes in an `E'…'` string, whose escaping doesn't depend on that
/// setting.
pub fn quote_literal(engine: &str, s: &str) -> String {
    match engine {
        "sqlite" => format!("'{}'", s.replace('\'', "''")),
        "postgres" | "postgresql" | "pgsql" => {
            if s.contains('\\') {
                format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
            } else {
                format!("'{}'", s.replace('\'', "''"))
            }
        }
        // ClickHouse documents `\'`; MySQL needs `''`, which is also read right
        // in NO_BACKSLASH_ESCAPES mode, where `\'` would not be — hence the split.
        "clickhouse" | "ch" => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        // MySQL/MariaDB, and anything unrecognised: assume backslash escapes.
        _ => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''")),
    }
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
    let sql = update_sql(engine, table, set_col, set_val, pk);
    let r = query(conn, &sql, 0)?;
    Ok(r.rows_affected.unwrap_or(0))
}

/// A scalar SQL literal: quoted+escaped string, or NULL.
fn sql_literal(engine: &str, v: Option<&str>) -> String {
    match v {
        Some(v) => quote_literal(engine, v),
        None => "NULL".to_string(),
    }
}

/// `col = 'v'` / `col IS NULL` predicates joined by AND, for a primary key.
fn pk_conds(engine: &str, pk: &[(String, Option<String>)]) -> String {
    pk.iter()
        .map(|(c, v)| match v {
            Some(v) => format!("{} = {}", quote_ident(engine, c), quote_literal(engine, v)),
            None => format!("{} IS NULL", quote_ident(engine, c)),
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Build an `UPDATE … SET col = val WHERE pk…` statement (no execution).
pub fn update_sql(
    engine: &str,
    table: &str,
    set_col: &str,
    set_val: Option<&str>,
    pk: &[(String, Option<String>)],
) -> String {
    format!(
        "UPDATE {} SET {} = {} WHERE {}",
        quote_ident(engine, table),
        quote_ident(engine, set_col),
        sql_literal(engine, set_val),
        pk_conds(engine, pk)
    )
}

/// Build a `DELETE FROM table WHERE pk…` statement (no execution).
pub fn delete_sql(engine: &str, table: &str, pk: &[(String, Option<String>)]) -> String {
    format!(
        "DELETE FROM {} WHERE {}",
        quote_ident(engine, table),
        pk_conds(engine, pk)
    )
}

/// Build an `INSERT INTO table (…) VALUES (…)` statement (no execution).
pub fn insert_sql(engine: &str, table: &str, values: &[(String, Option<String>)]) -> String {
    let cols: Vec<String> = values.iter().map(|(c, _)| quote_ident(engine, c)).collect();
    let vals: Vec<String> = values
        .iter()
        .map(|(_, v)| sql_literal(engine, v.as_deref()))
        .collect();
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_ident(engine, table),
        cols.join(", "),
        vals.join(", ")
    )
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
    let sql = insert_sql(engine, table, values);
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
    let sql = delete_sql(engine, table, pk);
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

/// Find the `(child_table, child_column)` pairs whose foreign keys point at
/// `table`.`column` — i.e. rows that reference this row (reverse FK).
pub fn referencing(
    conn: &Conn,
    table: &str,
    column: &str,
) -> Result<Vec<(String, String)>, String> {
    Ok(foreign_keys(conn)?
        .into_iter()
        .filter(|fk| fk.ref_table == table && fk.ref_column == column)
        .map(|fk| (fk.table, fk.column))
        .collect())
}

/// Whether a column type holds text (searchable with LIKE).
fn is_texty(data_type: &str) -> bool {
    let t = data_type.to_ascii_lowercase();
    [
        "char", "text", "string", "varchar", "json", "enum", "clob", "uuid",
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// Search every table's text columns for `needle` (case-insensitive substring),
/// returning `(table, matching_rows)` for tables with at least one hit. Each
/// table is capped at `limit` rows. Skips tables with no text columns.
pub fn search_all(
    conn: &Conn,
    engine: &str,
    needle: &str,
    limit: usize,
) -> Result<Vec<(String, QueryResult)>, String> {
    let mut out = Vec::new();
    for t in tables(conn)? {
        let cols = match columns(conn, &t) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let text_cols: Vec<&ColumnInfo> = cols.iter().filter(|c| is_texty(&c.data_type)).collect();
        if text_cols.is_empty() {
            continue;
        }
        let pattern = quote_literal(engine, &format!("%{needle}%"));
        let conds: Vec<String> = text_cols
            .iter()
            .map(|c| format!("{} LIKE {}", quote_ident(engine, &c.name), pattern))
            .collect();
        let sql = format!(
            "SELECT * FROM {} WHERE {} LIMIT {}",
            quote_ident(engine, &t),
            conds.join(" OR "),
            limit
        );
        if let Ok(res) = query(conn, &sql, limit) {
            if !res.rows.is_empty() {
                out.push((t, res));
            }
        }
    }
    Ok(out)
}

/// Count rows in `table`, honouring an optional `col = value` / `IS NULL`
/// filter — for pagination totals.
pub fn count_rows(
    conn: &Conn,
    engine: &str,
    table: &str,
    filter: Option<(&str, Option<&str>)>,
) -> Result<i64, String> {
    let mut sql = format!("SELECT COUNT(*) FROM {}", quote_ident(engine, table));
    if let Some((col, val)) = filter {
        let cond = match val {
            Some(v) => format!(
                "{} = {}",
                quote_ident(engine, col),
                quote_literal(engine, v)
            ),
            None => format!("{} IS NULL", quote_ident(engine, col)),
        };
        sql.push_str(&format!(" WHERE {cond}"));
    }
    let res = query(conn, &sql, 1)?;
    let n = res
        .rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_ref())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    Ok(n)
}

/// How many rows a table has, for the table tree: what the database already
/// knows, not a count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowCount {
    pub rows: i64,
    /// Counted (SQLite, ClickHouse's MergeTree), rather than the server's
    /// estimate (MySQL's `TABLE_ROWS`, PostgreSQL's planner statistics).
    pub exact: bool,
}

/// Row counts for every table, cheaply: one catalog query where the server
/// keeps an estimate. `SELECT COUNT(*)` per table on connect scanned each
/// one in turn and held the connection for minutes on a large database.
/// SQLite keeps no estimate, but its file is local and its connections
/// aren't shared, so its tables are counted. A table the server has no
/// figure for (PostgreSQL before its first ANALYZE) is left out.
pub fn row_counts(conn: &Conn, tables: &[String]) -> Result<HashMap<String, RowCount>, String> {
    let figures = |sql: &str, exact: bool| -> Result<HashMap<String, RowCount>, String> {
        let res = query(conn, sql, usize::MAX)?;
        Ok(res
            .rows
            .into_iter()
            .filter_map(|r| {
                let name = r.first().cloned().flatten()?;
                let rows = r.get(1).cloned().flatten()?.parse::<f64>().ok()? as i64;
                (rows >= 0).then_some((name, RowCount { rows, exact }))
            })
            .collect())
    };
    match &conn.backend {
        Backend::Mysql(_) => figures(
            "SELECT TABLE_NAME, TABLE_ROWS FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE'",
            false,
        ),
        // The same table a bare name means in the panel (see
        // `pg_table_schema`); `reltuples` is -1 until the table is analyzed.
        Backend::Postgres(_) => figures(
            "SELECT relname, reltuples::bigint FROM ( \
               SELECT c.relname, c.reltuples, row_number() OVER ( \
                        PARTITION BY c.relname \
                        ORDER BY pg_table_is_visible(c.oid) DESC, n.nspname) AS pick \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                WHERE c.relkind IN ('r', 'p') \
                  AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
                  AND n.nspname NOT LIKE 'pg_toast%' \
             ) t WHERE pick = 1 AND reltuples >= 0",
            false,
        ),
        Backend::Clickhouse { .. } => figures(
            "SELECT name, total_rows FROM system.tables \
             WHERE database = currentDatabase() AND total_rows IS NOT NULL",
            true,
        ),
        Backend::Sqlite(_) => Ok(tables
            .iter()
            .filter_map(|t| {
                let rows = count_rows(conn, "sqlite", t, None).ok()?;
                Some((t.clone(), RowCount { rows, exact: true }))
            })
            .collect()),
    }
}

/// `n` for the table tree: `987`, `12.3k`, `4.1M`; an estimate with `≈`.
pub fn row_count_label(count: RowCount) -> String {
    let n = count.rows as f64;
    let short = if n < 1_000.0 {
        format!("{}", count.rows)
    } else if n < 1_000_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else if n < 1_000_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else {
        format!("{:.1}B", n / 1_000_000_000.0)
    };
    let short = short
        .replace(".0k", "k")
        .replace(".0M", "M")
        .replace(".0B", "B");
    if count.exact {
        short
    } else {
        format!("≈{short}")
    }
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
        Some(v) => format!(
            "{} = {}",
            quote_ident(engine, column),
            quote_literal(engine, v)
        ),
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
            Some(v) => format!(
                "{} = {}",
                quote_ident(engine, col),
                quote_literal(engine, v)
            ),
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

/// Find named `:param` placeholders in `sql`, in first-seen order, ignoring
/// occurrences inside strings, comments and backtick identifiers, and skipping
/// PostgreSQL `::type` casts. Returns the parameter names (without the colon).
pub fn find_params(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    // Reuse a tiny state machine for string/comment skipping.
    #[derive(PartialEq)]
    enum St {
        Normal,
        Single,
        Double,
        Backtick,
        Line,
        Block,
    }
    let mut st = St::Normal;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match st {
            St::Normal => match c {
                '\'' => st = St::Single,
                '"' => st = St::Double,
                '`' => st = St::Backtick,
                '-' if next == Some('-') => st = St::Line,
                '#' => st = St::Line,
                '/' if next == Some('*') => st = St::Block,
                ':' if next == Some(':') => {
                    i += 2; // PostgreSQL cast, not a param
                    continue;
                }
                ':' if next.is_some_and(|n| n.is_ascii_alphabetic() || n == '_') => {
                    let mut j = i + 1;
                    let mut name = String::new();
                    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                        name.push(chars[j]);
                        j += 1;
                    }
                    if !out.contains(&name) {
                        out.push(name);
                    }
                    i = j;
                    continue;
                }
                _ => {}
            },
            St::Single => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == '\'' {
                    st = St::Normal;
                }
            }
            St::Double => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == '"' {
                    st = St::Normal;
                }
            }
            St::Backtick => {
                if c == '`' {
                    st = St::Normal;
                }
            }
            St::Line => {
                if c == '\n' {
                    st = St::Normal;
                }
            }
            St::Block => {
                if c == '*' && next == Some('/') {
                    i += 2;
                    st = St::Normal;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Substitute `:param` placeholders with quoted SQL literals from `values`
/// (missing values become NULL). Ignores strings/comments and `::` casts, like
/// [`find_params`].
pub fn substitute_params(engine: &str, sql: &str, values: &HashMap<String, String>) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    #[derive(PartialEq)]
    enum St {
        Normal,
        Single,
        Double,
        Backtick,
        Line,
        Block,
    }
    let mut st = St::Normal;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match st {
            St::Normal => match c {
                '\'' => {
                    st = St::Single;
                    out.push(c);
                }
                '"' => {
                    st = St::Double;
                    out.push(c);
                }
                '`' => {
                    st = St::Backtick;
                    out.push(c);
                }
                '-' if next == Some('-') => {
                    st = St::Line;
                    out.push(c);
                }
                '#' => {
                    st = St::Line;
                    out.push(c);
                }
                '/' if next == Some('*') => {
                    st = St::Block;
                    out.push(c);
                }
                ':' if next == Some(':') => {
                    out.push(':');
                    out.push(':');
                    i += 2;
                    continue;
                }
                ':' if next.is_some_and(|n| n.is_ascii_alphabetic() || n == '_') => {
                    let mut j = i + 1;
                    let mut name = String::new();
                    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                        name.push(chars[j]);
                        j += 1;
                    }
                    match values.get(&name) {
                        Some(v) => out.push_str(&quote_literal(engine, v)),
                        None => out.push_str("NULL"),
                    }
                    i = j;
                    continue;
                }
                _ => out.push(c),
            },
            // A backslash escapes the next character inside a string, as in
            // `find_params` — otherwise `'it\'s :x'` would end the string early
            // and substitute a placeholder that sits inside it.
            St::Single | St::Double if c == '\\' => {
                out.push(c);
                if let Some(n) = next {
                    out.push(n);
                }
                i += 2;
                continue;
            }
            _ => {
                out.push(c);
                match st {
                    St::Single if c == '\'' => st = St::Normal,
                    St::Double if c == '"' => st = St::Normal,
                    St::Backtick if c == '`' => st = St::Normal,
                    St::Line if c == '\n' => st = St::Normal,
                    St::Block if c == '*' && next == Some('/') => {
                        out.push('/');
                        i += 2;
                        st = St::Normal;
                        continue;
                    }
                    _ => {}
                }
            }
        }
        i += 1;
    }
    out
}

/// Split a SQL script into individual statements on top-level `;`, ignoring
/// semicolons inside string literals (`'…'`, `"…"`), backtick identifiers,
/// line comments (`-- …`, `# …`) and block comments (`/* … */`). Handles
/// doubled-quote (`''`) and backslash escapes inside strings. Blank statements
/// are dropped.
pub fn split_statements(sql: &str) -> Vec<String> {
    split_statement_ranges(sql)
        .into_iter()
        .map(|(s, e)| sql[s..e].to_string())
        .collect()
}

/// Like [`split_statements`], but returns the byte range of each (trimmed,
/// non-empty) statement in `sql` — used to run the statement under the cursor.
pub fn split_statement_ranges(sql: &str) -> Vec<(usize, usize)> {
    #[derive(PartialEq)]
    enum St {
        Normal,
        Single,
        Double,
        Backtick,
        Line,
        Block,
    }
    let mut out = Vec::new();
    let mut st = St::Normal;
    let idx: Vec<(usize, char)> = sql.char_indices().collect();
    // Byte where the current segment starts (after the last top-level `;`).
    let mut seg_start = 0usize;
    let push = |out: &mut Vec<(usize, usize)>, from: usize, to: usize| {
        let seg = &sql[from..to];
        let trimmed = seg.trim();
        if !trimmed.is_empty() {
            let lead = seg.len() - seg.trim_start().len();
            let start = from + lead;
            out.push((start, start + trimmed.len()));
        }
    };
    let mut i = 0;
    while i < idx.len() {
        let (b, c) = idx[i];
        let next = idx.get(i + 1).map(|x| x.1);
        match st {
            St::Normal => match c {
                ';' => {
                    push(&mut out, seg_start, b);
                    seg_start = b + 1; // ';' is one byte
                }
                '\'' => st = St::Single,
                '"' => st = St::Double,
                '`' => st = St::Backtick,
                '-' if next == Some('-') => st = St::Line,
                '#' => st = St::Line,
                '/' if next == Some('*') => st = St::Block,
                _ => {}
            },
            St::Single | St::Double => {
                let quote = if st == St::Single { '\'' } else { '"' };
                if c == '\\' {
                    i += 2;
                    continue;
                } else if c == quote {
                    if next == Some(quote) {
                        i += 2;
                        continue;
                    }
                    st = St::Normal;
                }
            }
            St::Backtick => {
                if c == '`' {
                    st = St::Normal;
                }
            }
            St::Line => {
                if c == '\n' {
                    st = St::Normal;
                }
            }
            St::Block => {
                if c == '*' && next == Some('/') {
                    i += 2;
                    st = St::Normal;
                    continue;
                }
            }
        }
        i += 1;
    }
    push(&mut out, seg_start, sql.len());
    out
}

/// Whether a statement may write — anything [`is_read_only`] can't vouch for,
/// including writable CTEs and `EXPLAIN ANALYZE` of a write. Used to gate
/// writes to non-local databases.
pub fn is_write_statement(sql: &str) -> bool {
    !is_read_only(sql)
}

/// Percent-encode a URL query value.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn query(conn: &Conn, sql: &str, max: usize) -> Result<QueryResult, String> {
    conn.guard(&[sql])?;
    let select = classify::returns_rows(sql);
    let start = Instant::now();
    match &conn.backend {
        Backend::Mysql(pool) => {
            use mysql::prelude::Queryable;
            let mut c = pool.get_conn().map_err(|e| e.to_string())?;
            conn.sync_mysql_session(&mut c)?;
            let _running = conn.mysql_running(&c);
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
            let c = conn.sqlite(path)?;
            let _running = conn.sqlite_running(&c);
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
            conn.sync_pg_session(&mut client)?;
            let res = match pg_limited(sql, max) {
                Some(limited) => match pg_run(&mut client, &limited, max) {
                    Ok(res) => res,
                    // An error the wrapping may have caused (a syntax or name
                    // error) is better told by the statement as written —
                    // unless the first failure aborted the user's own
                    // transaction, which then refuses everything. Anything
                    // else (a cancel, a runtime error) is the answer.
                    Err(first) if pg_wrapping_may_cause(&first) => pg_run(&mut client, sql, max)
                        .map_err(|second| {
                            if second.code()
                                == Some(&postgres::error::SqlState::IN_FAILED_SQL_TRANSACTION)
                            {
                                pg_err(first)
                            } else {
                                pg_err(second)
                            }
                        })?,
                    Err(e) => return Err(pg_err(e)),
                },
                None => pg_query(&mut client, sql, max)?,
            };
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
        Backend::Clickhouse { .. } => ch_query_as(&conn.backend, sql, max, conn.is_read_only()),
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

/// What PostgreSQL said. The error's own `Display` is just "db error"; the
/// server's message (with its DETAIL and HINT) is in the `DbError` beneath.
fn pg_err(e: postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => db.to_string(),
        None => match std::error::Error::source(&e) {
            Some(cause) => format!("{e}: {cause}"),
            None => e.to_string(),
        },
    }
}

/// `sql` with the server told to stop after `max + 1` rows, when it is a read
/// that can be wrapped (see [`classify::limitable`]). `simple_query` hands
/// over every row the statement makes, held in memory before any is cut, so
/// `SELECT * FROM events` on a large table read the whole table into `e`;
/// with a limit the server stops where `e` would. One row past `max` says
/// there was more.
fn pg_limited(sql: &str, max: usize) -> Option<String> {
    if max == usize::MAX || !classify::limitable(sql) {
        return None;
    }
    let body = sql.trim().trim_end_matches(';').trim_end();
    // On its own line, so a `--` comment at the end can't swallow the `)`.
    Some(format!(
        "SELECT * FROM (\n{body}\n) AS e_rows LIMIT {}",
        max + 1
    ))
}

/// Whether `e` from a statement wrapped by [`pg_limited`] could be the
/// wrapping's doing: a syntax or name error (SQLSTATE class 42) or a
/// feature PostgreSQL won't do in a subquery (0A).
fn pg_wrapping_may_cause(e: &postgres::Error) -> bool {
    e.code()
        .is_some_and(|c| c.code().starts_with("42") || c.code().starts_with("0A"))
}

fn pg_query(client: &mut postgres::Client, sql: &str, max: usize) -> Result<PgRows, String> {
    pg_run(client, sql, max).map_err(pg_err)
}

fn pg_run(client: &mut postgres::Client, sql: &str, max: usize) -> Result<PgRows, postgres::Error> {
    use postgres::SimpleQueryMessage::*;
    let msgs = client.simple_query(sql)?;
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

// ── connection secrets (OS credential store) ───

/// Passwords and SSH secrets, kept in the OS credential store rather than in
/// `databases.json`.
///
/// They used to be serialised straight into that file, which was created with
/// the process umask — world-readable in practice. The askpass helper in this
/// same crate already treated SSH secrets as something never to put on disk;
/// the saved-connection path simply hadn't caught up.
///
/// When no credential store is reachable (a headless Linux box with no Secret
/// Service, say) the secrets stay in the file, which is written 0600 either way.
mod secrets {
    /// Service name under which every `e` connection secret is filed.
    const SERVICE: &str = "e-db";

    /// The three secret-bearing fields of a connection.
    pub const FIELDS: [&str; 3] = ["password", "ssh_password", "ssh_passphrase"];

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn entry(id: &str, field: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, &format!("{id}/{field}")).ok()
    }

    /// Store a secret. An empty secret removes the entry instead.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn set(id: &str, field: &str, secret: &str) -> Result<(), String> {
        let entry = entry(id, field).ok_or("no credential store")?;
        if secret.is_empty() {
            return match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        entry.set_password(secret).map_err(|e| e.to_string())
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn get(id: &str, field: &str) -> Option<String> {
        entry(id, field)?.get_password().ok()
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn delete(id: &str, field: &str) {
        if let Some(entry) = entry(id, field) {
            let _ = entry.delete_credential();
        }
    }

    // No backend on this platform: callers fall back to the 0600 file.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn set(_id: &str, _field: &str, _secret: &str) -> Result<(), String> {
        Err("no credential store on this platform".into())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn get(_id: &str, _field: &str) -> Option<String> {
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn delete(_id: &str, _field: &str) {}
}

impl DbConfig {
    /// Read one of the secret-bearing fields by name.
    fn secret(&self, field: &str) -> &str {
        match field {
            "password" => &self.password,
            "ssh_password" => &self.ssh_password,
            "ssh_passphrase" => &self.ssh_passphrase,
            _ => "",
        }
    }

    fn set_secret(&mut self, field: &str, value: String) {
        match field {
            "password" => self.password = value,
            "ssh_password" => self.ssh_password = value,
            "ssh_passphrase" => self.ssh_passphrase = value,
            _ => {}
        }
    }

    fn clear_secrets(&mut self) {
        for field in secrets::FIELDS {
            self.set_secret(field, String::new());
        }
    }
}

/// A fresh connection id. Local uniqueness is all this needs — it only has to
/// distinguish one saved connection from another on this machine.
fn new_connection_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
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
    let Some(path) = store_path() else {
        return serde_json::json!({});
    };
    // A store written by an earlier version is 0644 and may still hold
    // plaintext secrets. Its contents only move to the credential store on the
    // next save, which might never come if the user never edits a connection —
    // so tighten the mode the moment we touch the file, not just when we write it.
    tighten_permissions(&path);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Drop a file's mode to 0600 if it is looser. No-op when it doesn't exist.
fn tighten_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.permissions().mode() & 0o177 != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Load the saved connections for a project (keyed by its root path).
///
/// Connections whose secrets live in the credential store come back off disk
/// blank; their secrets are fetched here so the rest of the crate can keep
/// treating `DbConfig` as fully populated.
pub fn load_connections(project: &Path) -> Vec<DbConfig> {
    let store = read_store();
    let mut conns: Vec<DbConfig> = store
        .get(project.to_string_lossy().as_ref())
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    for conn in &mut conns {
        if !conn.secrets_in_keychain || conn.id.is_empty() {
            continue;
        }
        for field in secrets::FIELDS {
            if let Some(secret) = secrets::get(&conn.id, field) {
                conn.set_secret(field, secret);
            }
        }
    }
    conns
}

/// Persist the connection list for a project.
///
/// Secrets go to the OS credential store and are blanked before the list is
/// serialised. If the store can't be reached they stay in the file — which is
/// written 0600 regardless, so it is never readable by other users.
pub fn save_connections(project: &Path, conns: &[DbConfig]) -> Result<(), String> {
    let key = project.to_string_lossy().into_owned();
    let previous_ids: Vec<String> = read_store()
        .get(&key)
        .and_then(|v| serde_json::from_value::<Vec<DbConfig>>(v.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.id)
        .filter(|id| !id.is_empty())
        .collect();

    let mut on_disk = Vec::with_capacity(conns.len());
    for conn in conns {
        let mut conn = conn.clone();
        if conn.id.is_empty() {
            conn.id = new_connection_id();
        }
        // All-or-nothing per connection: a half-stored connection would come
        // back with some secrets missing and no way to tell which.
        let stored = secrets::FIELDS
            .iter()
            .try_for_each(|field| secrets::set(&conn.id, field, conn.secret(field)));
        match stored {
            Ok(()) => {
                conn.secrets_in_keychain = true;
                conn.clear_secrets();
            }
            Err(e) => {
                if conn.secrets_in_keychain {
                    // It used to be in the store and now isn't reachable. Say so
                    // rather than silently writing the secret back to disk.
                    eprintln!("e: credential store unavailable ({e}); keeping secrets in databases.json (0600)");
                }
                conn.secrets_in_keychain = false;
            }
        }
        on_disk.push(conn);
    }

    // Drop credential-store entries for connections that were removed.
    let live: Vec<&str> = on_disk.iter().map(|c| c.id.as_str()).collect();
    for id in previous_ids {
        if !live.contains(&id.as_str()) {
            for field in secrets::FIELDS {
                secrets::delete(&id, field);
            }
        }
    }

    let mut store = read_store();
    let obj = store.as_object_mut().ok_or("invalid store")?;
    obj.insert(
        key,
        serde_json::to_value(&on_disk).map_err(|e| e.to_string())?,
    );
    let path = store_path().ok_or("no HOME")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_private(
        &path,
        &serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?,
    )
}

/// Write a file readable only by its owner, tightening an existing file's mode
/// too — a `databases.json` created before this was enforced is still out there
/// at 0644.
fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| e.to_string())?;
        // `.mode()` only applies when creating, so fix up a pre-existing file.
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        file.write_all(contents.as_bytes())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents).map_err(|e| e.to_string())
    }
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

fn name_hints_staging(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    ["staging", "stage", "stg", "uat", "preprod", "pre-prod"]
        .iter()
        .any(|needle| s.contains(needle))
}

/// A connection's environment, used for coloured labels and write guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Environment {
    Local,
    Staging,
    Production,
}

impl Environment {
    pub fn label(self) -> &'static str {
        match self {
            Environment::Local => "local",
            Environment::Staging => "staging",
            Environment::Production => "production",
        }
    }
    pub fn is_local(self) -> bool {
        self == Environment::Local
    }
}

// ---- Snapshot restore (DB-704, the reversible half of the verify-fix loop) ----

/// `mysql` client args for replaying a dump into `config`'s database.
fn mysql_restore_args(config: &DbConfig) -> Vec<String> {
    vec![
        format!("--host={}", config.host),
        format!("--port={}", config.port),
        format!("--user={}", config.username),
        config.database.clone(),
    ]
}

/// `psql` client args for replaying a dump into `config`'s database.
fn pg_restore_args(config: &DbConfig) -> Vec<String> {
    vec![
        format!("--host={}", config.host),
        format!("--port={}", config.port),
        format!("--username={}", config.username),
        "--dbname".to_string(),
        config.database.clone(),
    ]
}

fn run_sql_file(
    program: &str,
    args: &[String],
    env: &[(&str, &str)],
    file: &Path,
) -> Result<(), String> {
    let input = std::fs::File::open(file).map_err(|e| format!("open snapshot: {e}"))?;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .stdin(std::process::Stdio::from(input))
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// Restore a snapshot previously written by the editor's snapshot feature: an
/// SQLite file copy, or a mysqldump / pg_dump `.sql` replayed through the
/// client. Mirrors the snapshot writer. Local connections only (callers gate on
/// [`DbConfig::environment`]); this overwrites the target database.
pub fn restore_snapshot(config: &DbConfig, snapshot: &Path) -> Result<(), String> {
    if !snapshot.exists() {
        return Err(format!("snapshot not found: {}", snapshot.display()));
    }
    match config.engine.as_str() {
        "sqlite" => {
            if config.path.trim().is_empty() {
                return Err("sqlite connection has no file path".to_string());
            }
            std::fs::copy(snapshot, &config.path)
                .map(|_| ())
                .map_err(|e| format!("restore copy failed: {e}"))
        }
        "mysql" | "mariadb" => run_sql_file(
            "mysql",
            &mysql_restore_args(config),
            &[("MYSQL_PWD", config.password.as_str())],
            snapshot,
        ),
        "postgres" | "postgresql" | "pgsql" => run_sql_file(
            "psql",
            &pg_restore_args(config),
            &[("PGPASSWORD", config.password.as_str())],
            snapshot,
        ),
        other => Err(format!("restore not supported for {other}")),
    }
}

/// Against a real PostgreSQL — `--ignored`, with `E_PG_PORT` (user
/// `postgres`, password `E_PG_PASSWORD`, a scratch database it may fill).
#[cfg(test)]
mod pg_live_tests {
    use super::*;

    /// `--ignored`, with `E_MYSQL_PORT` (user root, no password; reads only).
    #[test]
    #[ignore]
    fn cancel_kills_a_running_mysql_query() {
        let Some(port) = std::env::var("E_MYSQL_PORT").ok() else {
            return;
        };
        let conn = std::sync::Arc::new(
            connect(&DbConfig {
                engine: "mysql".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                username: "root".into(),
                ..Default::default()
            })
            .unwrap(),
        );
        let running = conn.clone();
        let t = Instant::now();
        let handle = std::thread::spawn(move || query(&running, "SELECT SLEEP(30)", 10));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(cancel(&conn), Ok(true));
        // An interrupted SLEEP returns 1 instead of an error.
        let r = handle.join().unwrap().unwrap();
        assert_eq!(r.rows[0][0].as_deref(), Some("1"));
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert!(conn.mysql_running.lock().unwrap().is_empty());
        assert!(query(&conn, "SELECT 1", 10).is_ok());
    }

    /// `--ignored`, with `E_PG_PORT` (+ `E_PG_PASSWORD`).
    #[test]
    #[ignore]
    fn cancel_stops_a_running_query_and_frees_the_connection() {
        let Some(port) = std::env::var("E_PG_PORT").ok() else {
            return;
        };
        let conn = std::sync::Arc::new(
            connect(&DbConfig {
                engine: "postgres".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                username: "postgres".into(),
                password: std::env::var("E_PG_PASSWORD").unwrap_or_default(),
                database: "postgres".into(),
                ..Default::default()
            })
            .unwrap(),
        );
        let running = conn.clone();
        let t = Instant::now();
        let handle = std::thread::spawn(move || query(&running, "SELECT pg_sleep(30)", 10));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(cancel(&conn), Ok(true));
        let err = handle.join().unwrap().unwrap_err();
        assert!(
            err.contains("canceling statement due to user request"),
            "{err}"
        );
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        // The connection is free for the next query at once.
        let t = Instant::now();
        let r = query(&conn, "SELECT 1", 10).unwrap();
        assert_eq!(r.rows[0][0].as_deref(), Some("1"));
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn reads_are_limited_on_the_server_and_nothing_else_is() {
        assert_eq!(
            pg_limited("select * from events order by id;  ", 1000).as_deref(),
            Some("SELECT * FROM (\nselect * from events order by id\n) AS e_rows LIMIT 1001")
        );
        // A trailing comment stays on its own line.
        assert!(pg_limited("SELECT 1 -- one", 10)
            .unwrap()
            .contains("-- one\n)"));
        assert!(pg_limited("WITH t AS (SELECT 1) SELECT * FROM t", 10).is_some());
        assert!(pg_limited("VALUES (1), (2)", 10).is_some());
        assert!(pg_limited("TABLE events", 10).is_some());
        for sql in [
            "SHOW search_path",
            "EXPLAIN SELECT 1",
            "UPDATE t SET a = 1",
            "SELECT * INTO copy FROM t",
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
        ] {
            assert_eq!(pg_limited(sql, 10), None, "{sql}");
        }
        assert_eq!(
            pg_limited("SELECT 1", usize::MAX),
            None,
            "metadata takes it all"
        );
    }

    /// `--ignored`, with `E_PG_PORT` (+ `E_PG_PASSWORD`) and/or `E_CH_PORT`.
    #[test]
    #[ignore]
    fn a_huge_result_stops_at_the_cut_on_the_server() {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        if let Some(port) = env("E_PG_PORT") {
            let conn = connect(&DbConfig {
                engine: "postgres".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                username: "postgres".into(),
                password: env("E_PG_PASSWORD").unwrap_or_default(),
                database: "postgres".into(),
                ..Default::default()
            })
            .unwrap();
            let t = Instant::now();
            let r = query(
                &conn,
                "SELECT g, md5(g::text) FROM generate_series(1, 50000000) g",
                1000,
            )
            .unwrap();
            let took = t.elapsed();
            assert_eq!(r.rows.len(), 1000);
            assert!(r.truncated);
            assert!(
                took < Duration::from_secs(5),
                "the server stopped early: {took:?}"
            );
            eprintln!("postgres: 50M-row query cut at 1000 in {took:?}");
            // The statement's own order is kept.
            let r = query(
                &conn,
                "SELECT g FROM generate_series(1, 5000) g ORDER BY g DESC",
                1000,
            )
            .unwrap();
            assert_eq!(r.rows[0][0].as_deref(), Some("5000"));
            assert_eq!(r.rows[999][0].as_deref(), Some("4001"));
            // A few rows: all of them, not truncated.
            let r = query(&conn, "SELECT 1 AS a UNION ALL SELECT 2", 1000).unwrap();
            assert_eq!((r.rows.len(), r.truncated), (2, false));
            // An error names the user's SQL, not the wrapper.
            let err = query(&conn, "SELECT nope FROM generate_series(1, 3) g", 1000).unwrap_err();
            assert!(err.contains("nope") && !err.contains("e_rows"), "{err}");
            // Inside the user's own transaction, the first error is the one shown.
            query(&conn, "BEGIN", 0).unwrap();
            let err = query(&conn, "SELECT nope FROM generate_series(1, 3) g", 1000).unwrap_err();
            assert!(err.contains("nope"), "{err}");
            query(&conn, "ROLLBACK", 0).unwrap();
        }
        if let Some(port) = env("E_CH_PORT") {
            let conn = connect(&DbConfig {
                engine: "clickhouse".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                ..Default::default()
            })
            .unwrap();
            let t = Instant::now();
            let r = query(
                &conn,
                "SELECT number, toString(number) FROM numbers(500000000)",
                1000,
            )
            .unwrap();
            let took = t.elapsed();
            assert_eq!(r.rows.len(), 1000);
            assert!(r.truncated);
            assert!(
                took < Duration::from_secs(5),
                "the server stopped early: {took:?}"
            );
            eprintln!("clickhouse: 500M-row query cut at 1000 in {took:?}");
            let r = query(&conn, "SELECT 1 UNION ALL SELECT 2", 1000).unwrap();
            assert_eq!((r.rows.len(), r.truncated), (2, false));
        }
    }

    #[test]
    fn row_counts_read_short_and_say_when_they_are_estimates() {
        let c = |rows, exact| row_count_label(RowCount { rows, exact });
        assert_eq!(c(0, true), "0");
        assert_eq!(c(987, true), "987");
        assert_eq!(c(1_000, true), "1k");
        assert_eq!(c(12_345, false), "≈12.3k");
        assert_eq!(c(4_100_000, false), "≈4.1M");
        assert_eq!(c(2_000_000_000, true), "2B");
    }

    /// Row counts from each server's catalog — `--ignored`, with whichever
    /// of `E_PG_PORT` (+ `E_PG_PASSWORD`), `E_CH_PORT` (HTTP) and
    /// `E_MYSQL_PORT` (user root, no password; reads the `mysql` schema
    /// only) are set.
    #[test]
    #[ignore]
    fn row_counts_come_from_the_catalog() {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        if let Some(port) = env("E_PG_PORT") {
            let conn = connect(&DbConfig {
                engine: "postgres".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                username: "postgres".into(),
                password: env("E_PG_PASSWORD").unwrap_or_default(),
                database: "postgres".into(),
                ..Default::default()
            })
            .unwrap();
            for sql in [
                "DROP TABLE IF EXISTS counted, unanalyzed",
                "CREATE TABLE counted AS SELECT g AS id FROM generate_series(1, 5000) g",
                "CREATE TABLE unanalyzed (id int)",
                "ANALYZE counted",
            ] {
                query(&conn, sql, 0).unwrap_or_else(|e| panic!("{sql}: {e}"));
            }
            let counts = row_counts(&conn, &[]).unwrap();
            assert_eq!(
                counts["counted"],
                RowCount {
                    rows: 5000,
                    exact: false
                }
            );
            assert!(!counts.contains_key("unanalyzed"), "{counts:?}");
            let _ = query(&conn, "DROP TABLE counted, unanalyzed", 0);
        }
        if let Some(port) = env("E_CH_PORT") {
            let conn = connect(&DbConfig {
                engine: "clickhouse".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                ..Default::default()
            })
            .unwrap();
            for sql in [
                "DROP TABLE IF EXISTS counted",
                "CREATE TABLE counted (id UInt32) ENGINE = MergeTree ORDER BY id",
                "INSERT INTO counted SELECT number FROM numbers(1234)",
            ] {
                query(&conn, sql, 0).unwrap_or_else(|e| panic!("{sql}: {e}"));
            }
            let counts = row_counts(&conn, &[]).unwrap();
            assert_eq!(
                counts["counted"],
                RowCount {
                    rows: 1234,
                    exact: true
                }
            );
            let _ = query(&conn, "DROP TABLE counted", 0);
        }
        if let Some(port) = env("E_MYSQL_PORT") {
            let conn = connect(&DbConfig {
                engine: "mysql".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                username: "root".into(),
                database: "mysql".into(),
                ..Default::default()
            })
            .unwrap();
            let counts = row_counts(&conn, &[]).unwrap();
            let user = counts.get("user").expect("mysql.user");
            assert!(!user.exact && user.rows >= 0, "{user:?}");
        }
    }

    #[test]
    #[ignore]
    fn foreign_keys_follow_the_tables_the_panel_shows() {
        let Some(port) = std::env::var("E_PG_PORT").ok() else {
            return;
        };
        let conn = connect(&DbConfig {
            engine: "postgres".into(),
            host: "127.0.0.1".into(),
            port: port.parse().unwrap(),
            username: "postgres".into(),
            password: std::env::var("E_PG_PASSWORD").unwrap_or_default(),
            database: "postgres".into(),
            ..Default::default()
        })
        .unwrap();
        for sql in [
            "DROP SCHEMA IF EXISTS other CASCADE",
            "DROP SCHEMA IF EXISTS app CASCADE",
            "DROP TABLE IF EXISTS line_items, posts, comments, orders, users CASCADE",
            "CREATE TABLE users (id int PRIMARY KEY)",
            "CREATE TABLE orders (id int PRIMARY KEY, no int, user_id int REFERENCES users (id), UNIQUE (id, no))",
            // A composite key: its two columns pair up, not four ways.
            "CREATE TABLE line_items (order_id int, order_no int, \
               FOREIGN KEY (order_id, order_no) REFERENCES orders (id, no))",
            // One constraint name on two tables of one schema.
            "CREATE TABLE posts (user_id int CONSTRAINT fk_owner REFERENCES users (id))",
            "CREATE TABLE comments (user_id int CONSTRAINT fk_owner REFERENCES users (id))",
            // Namesakes in a schema off the search path: not what `orders` means.
            "CREATE SCHEMA other",
            "CREATE TABLE other.users (id int PRIMARY KEY)",
            "CREATE TABLE other.orders (id int PRIMARY KEY, buyer_id int REFERENCES other.users (id))",
            // Only in a schema off the search path: the panel shows these.
            "CREATE SCHEMA app",
            "CREATE TABLE app.customers (id int PRIMARY KEY)",
            "CREATE TABLE app.invoices (customer_id int REFERENCES app.customers (id))",
        ] {
            query(&conn, sql, 0).unwrap_or_else(|e| panic!("{sql}: {e}"));
        }
        let mut got: Vec<String> = foreign_keys(&conn)
            .unwrap()
            .into_iter()
            .map(|f| {
                format!(
                    "{}.{} -> {}.{}",
                    f.table, f.column, f.ref_table, f.ref_column
                )
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                "comments.user_id -> users.id",
                "invoices.customer_id -> customers.id",
                "line_items.order_id -> orders.id",
                "line_items.order_no -> orders.no",
                "orders.user_id -> users.id",
                "posts.user_id -> users.id",
            ]
        );
        for sql in [
            "DROP SCHEMA other CASCADE",
            "DROP SCHEMA app CASCADE",
            "DROP TABLE line_items, posts, comments, orders, users CASCADE",
        ] {
            let _ = query(&conn, sql, 0);
        }
    }
}

#[cfg(test)]
mod tls_tests {
    use super::*;

    fn cfg(engine: &str, host: &str, tls: &str) -> DbConfig {
        DbConfig {
            engine: engine.into(),
            host: host.into(),
            tls: tls.into(),
            ..Default::default()
        }
    }

    #[test]
    fn tls_modes_read_libpq_and_mysql_names() {
        assert_eq!(TlsMode::parse(""), TlsMode::Auto);
        assert_eq!(TlsMode::parse("prefer"), TlsMode::Auto);
        assert_eq!(TlsMode::parse("disable"), TlsMode::Disable);
        assert_eq!(TlsMode::parse("require"), TlsMode::Require);
        assert_eq!(TlsMode::parse("verify-full"), TlsMode::Verify);
        assert_eq!(TlsMode::parse("VERIFY_IDENTITY"), TlsMode::Verify);
        for m in [
            TlsMode::Auto,
            TlsMode::Disable,
            TlsMode::Require,
            TlsMode::Verify,
        ] {
            assert_eq!(TlsMode::parse(m.as_str()), m);
        }
    }

    #[test]
    fn auto_encrypts_only_direct_connections_to_other_hosts() {
        assert!(mysql_ssl(&cfg("mysql", "127.0.0.1", "")).is_none());
        assert!(mysql_ssl(&cfg("mysql", "shop.test", "")).is_none());
        assert!(mysql_ssl(&cfg("mysql", "db.example.com", "")).is_some());
        let mut tunnelled = cfg("mysql", "db.example.com", "");
        tunnelled.use_ssh = true;
        assert!(mysql_ssl(&tunnelled).is_none());
        assert!(mysql_ssl(&cfg("mysql", "db.example.com", "disable")).is_none());
        // Asked for, it's used even locally.
        assert!(mysql_ssl(&cfg("mysql", "127.0.0.1", "require")).is_some());
    }

    #[test]
    fn a_ca_that_cant_be_read_is_said_plainly() {
        let err = tls_connector(TlsMode::Verify, "/no/such/ca.pem").unwrap_err();
        assert!(err.contains("/no/such/ca.pem"), "{err}");
        let dir = std::env::temp_dir().join(format!("e-db-tls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("junk.pem");
        std::fs::write(&junk, "not a certificate").unwrap();
        let err = tls_connector(TlsMode::Verify, junk.to_str().unwrap()).unwrap_err();
        assert!(err.contains("isn't PEM"), "{err}");
        assert!(tls_connector(TlsMode::Require, "").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_tls_settings_carry_over() {
        let dir = std::env::temp_dir().join(format!("e-db-tls-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".env"),
            "DB_CONNECTION=pgsql\nDB_HOST=db.example.com\nDB_SSLMODE=verify-full\n",
        )
        .unwrap();
        assert_eq!(from_env(&dir).unwrap().tls_mode(), TlsMode::Verify);
        std::fs::write(
            dir.join(".env"),
            "DB_CONNECTION=pgsql\nDB_HOST=db.example.com\nDB_SSLMODE=prefer\n",
        )
        .unwrap();
        assert_eq!(from_env(&dir).unwrap().tls_mode(), TlsMode::Auto);
        std::fs::write(
            dir.join(".env"),
            "DB_CONNECTION=mysql\nDB_HOST=db.example.com\nMYSQL_ATTR_SSL_CA=storage/ca.pem\n",
        )
        .unwrap();
        let c = from_env(&dir).unwrap();
        assert_eq!(c.tls_mode(), TlsMode::Verify);
        assert_eq!(Path::new(&c.tls_ca), dir.join("storage/ca.pem"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Against real servers — `--ignored`, with whichever of these are set:
    /// `E_TLS_PG=port`, `E_TLS_CH=https-port`, `E_TLS_MYSQL=port` (user root,
    /// no password), `E_TLS_CA=ca.pem` (the CA that signed the PostgreSQL and
    /// ClickHouse certificates, issued for `localhost`), `E_TLS_REMOTE=ip` (an
    /// address of this machine that isn't loopback, for auto), and
    /// `E_TLS_PASSWORD` for PostgreSQL's `postgres` user.
    #[test]
    #[ignore]
    fn live_tls_connections() {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let ca = env("E_TLS_CA").unwrap_or_default();
        let scalar = |conn: &Conn, sql: &str| -> String {
            let r = query(conn, sql, 1).unwrap();
            r.rows
                .first()
                .and_then(|row| row.first())
                .cloned()
                .flatten()
                .unwrap_or_default()
        };
        if let Some(port) = env("E_TLS_PG") {
            let pg = |host: &str, tls: &str, ca: &str| DbConfig {
                engine: "postgres".into(),
                host: host.into(),
                port: port.parse().unwrap(),
                username: "postgres".into(),
                password: env("E_TLS_PASSWORD").unwrap_or_default(),
                database: "postgres".into(),
                tls: tls.into(),
                tls_ca: ca.into(),
                ..Default::default()
            };
            let ssl = "SELECT ssl::text FROM pg_stat_ssl WHERE pid = pg_backend_pid()";
            let c = connect(&pg("localhost", "require", "")).unwrap();
            assert_eq!(scalar(&c, ssl), "true", "require");
            let c = connect(&pg("localhost", "verify", &ca)).unwrap();
            assert_eq!(scalar(&c, ssl), "true", "verify with the CA");
            let err = connect(&pg("localhost", "verify", ""))
                .err()
                .expect("untrusted CA");
            eprintln!("verify without the CA: {err}");
            let err = connect(&pg("127.0.0.1", "verify", &ca))
                .err()
                .expect("wrong name");
            eprintln!("verify against another name: {err}");
            let c = connect(&pg("localhost", "disable", "")).unwrap();
            assert_eq!(scalar(&c, ssl), "false", "disable");
            let c = connect(&pg("localhost", "", "")).unwrap();
            assert_eq!(scalar(&c, ssl), "false", "auto, local");
            if let Some(ip) = env("E_TLS_REMOTE") {
                let c = connect(&pg(&ip, "", "")).unwrap();
                assert_eq!(scalar(&c, ssl), "true", "auto, not local");
            }
            eprintln!("postgres: ok");
        }
        if let Some(port) = env("E_TLS_CH") {
            let ch = |host: &str, tls: &str, ca: &str| DbConfig {
                engine: "clickhouse".into(),
                host: host.into(),
                port: port.parse().unwrap(),
                tls: tls.into(),
                tls_ca: ca.into(),
                ..Default::default()
            };
            let c = connect(&ch("localhost", "require", "")).unwrap();
            assert_eq!(scalar(&c, "SELECT 1"), "1");
            let c = connect(&ch("localhost", "verify", &ca)).unwrap();
            assert_eq!(scalar(&c, "SELECT 1"), "1");
            assert!(
                connect(&ch("localhost", "verify", "")).is_err(),
                "untrusted CA"
            );
            // Auto follows the port: 8443 speaks HTTPS.
            if port == "8443" {
                assert!(connect(&ch("localhost", "", "")).is_ok(), "auto on 8443");
            }
            assert!(
                connect(&ch("localhost", "disable", "")).is_err(),
                "HTTP to HTTPS"
            );
            eprintln!("clickhouse: ok");
        }
        if let Some(port) = env("E_TLS_MYSQL") {
            let my = |tls: &str| DbConfig {
                engine: "mysql".into(),
                host: "127.0.0.1".into(),
                port: port.parse().unwrap(),
                username: "root".into(),
                tls: tls.into(),
                ..Default::default()
            };
            let cipher = "SHOW SESSION STATUS LIKE 'Ssl_cipher'";
            let row = |c: &Conn| {
                query(c, cipher, 1).unwrap().rows[0][1]
                    .clone()
                    .unwrap_or_default()
            };
            assert!(
                !row(&connect(&my("require")).unwrap()).is_empty(),
                "require"
            );
            assert!(row(&connect(&my("")).unwrap()).is_empty(), "auto, local");
            eprintln!("mysql: ok");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── connection secrets & store permissions ──────────────────────────
    //
    // These deliberately never touch the real credential store: hitting it
    // would write into the developer's login keychain on every `cargo test`,
    // and CI Linux has no Secret Service at all. What's covered here is the
    // deterministic half — field routing, id generation, file permissions, and
    // the on-disk fallback shape.

    use std::sync::Mutex;

    /// `HOME` is process-global; serialise the tests that repoint it.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    struct HomeGuard {
        previous: Option<std::ffi::OsString>,
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fake_home(tag: &str) -> HomeGuard {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("e-db-home-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".config").join("e")).unwrap();
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        HomeGuard {
            previous,
            dir,
            _lock: lock,
        }
    }

    /// Opt-in: exercises the *real* OS credential store, so it is not part of
    /// the normal suite. Run with `cargo test -p e-db keychain -- --ignored`.
    /// Cleans up after itself.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let id = new_connection_id();
        for (i, field) in secrets::FIELDS.iter().enumerate() {
            secrets::set(&id, field, &format!("secret-{i}"))
                .unwrap_or_else(|e| panic!("no credential store available: {e}"));
        }
        for (i, field) in secrets::FIELDS.iter().enumerate() {
            assert_eq!(
                secrets::get(&id, field).as_deref(),
                Some(&*format!("secret-{i}"))
            );
        }
        // An empty secret removes the entry rather than storing "".
        secrets::set(&id, "password", "").unwrap();
        assert_eq!(secrets::get(&id, "password"), None);

        for field in secrets::FIELDS {
            secrets::delete(&id, field);
        }
        for field in secrets::FIELDS {
            assert_eq!(secrets::get(&id, field), None, "{field} not cleaned up");
        }
    }

    #[test]
    fn every_secret_field_is_routed() {
        // A missing arm in `secret`/`set_secret` would silently drop a secret,
        // so assert all three round-trip rather than spot-checking one.
        let mut c = DbConfig::default();
        for (i, field) in secrets::FIELDS.iter().enumerate() {
            c.set_secret(field, format!("value-{i}"));
        }
        for (i, field) in secrets::FIELDS.iter().enumerate() {
            assert_eq!(c.secret(field), format!("value-{i}"), "field {field}");
        }
        assert_eq!(c.password, "value-0");
        assert_eq!(c.ssh_password, "value-1");
        assert_eq!(c.ssh_passphrase, "value-2");

        c.clear_secrets();
        for field in secrets::FIELDS {
            assert!(c.secret(field).is_empty(), "field {field} not cleared");
        }
    }

    #[test]
    fn connection_ids_are_distinct() {
        let ids: std::collections::HashSet<String> =
            (0..100).map(|_| new_connection_id()).collect();
        assert_eq!(ids.len(), 100);
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("e-db-perms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("databases.json");

        write_private(&path, "{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a new store must not be readable by anyone else"
        );

        // A store written before this was enforced is still out there at 0644.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, "{\"a\":1}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "an existing loose store must be tightened");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn merely_reading_the_store_tightens_a_loose_one() {
        use std::os::unix::fs::PermissionsExt;
        let home = fake_home("tighten");
        let store = home.dir.join(".config").join("e").join("databases.json");
        std::fs::write(&store, "{}").unwrap();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o644)).unwrap();

        // An upgrading user may never edit a connection, so the read path has
        // to be what fixes the mode.
        let _ = load_connections(&PathBuf::from("/tmp/anything"));

        let mode = std::fs::metadata(&store).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn connections_saved_before_the_keychain_still_load() {
        // Back-compat: files written by earlier versions have no `id`, no
        // `secrets_in_keychain`, and the password inline. Loading must not
        // consult the credential store for those (the flag gates it).
        let home = fake_home("legacy");
        let project = PathBuf::from("/tmp/some-project");
        let fixture = serde_json::json!({
            "/tmp/some-project": [{
                "engine": "mysql",
                "host": "127.0.0.1",
                "port": 3306,
                "database": "app",
                "username": "root",
                "password": "s3cret",
                "ssh_password": "ssh-s3cret"
            }]
        });
        std::fs::write(
            home.dir.join(".config").join("e").join("databases.json"),
            serde_json::to_string_pretty(&fixture).unwrap(),
        )
        .unwrap();

        let conns = load_connections(&project);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].password, "s3cret");
        assert_eq!(conns[0].ssh_password, "ssh-s3cret");
        assert!(conns[0].id.is_empty());
        assert!(!conns[0].secrets_in_keychain);
    }

    #[test]
    fn a_keychain_backed_connection_reads_blank_from_disk() {
        // The flag is set but the credential store holds nothing for this id,
        // so the secret stays empty rather than falling back to whatever the
        // file happens to contain.
        let home = fake_home("keychain");
        let project = PathBuf::from("/tmp/kc-project");
        let fixture = serde_json::json!({
            "/tmp/kc-project": [{
                "id": "no-such-id-in-any-keychain",
                "secrets_in_keychain": true,
                "engine": "mysql",
                "host": "127.0.0.1",
                "database": "app",
                "username": "root",
                "password": ""
            }]
        });
        std::fs::write(
            home.dir.join(".config").join("e").join("databases.json"),
            serde_json::to_string_pretty(&fixture).unwrap(),
        )
        .unwrap();

        let conns = load_connections(&project);
        assert_eq!(conns.len(), 1);
        assert!(conns[0].password.is_empty());
        assert!(conns[0].secrets_in_keychain);
    }

    fn cfg(engine: &str, host: &str) -> DbConfig {
        DbConfig {
            engine: engine.to_string(),
            host: host.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn literals_cannot_escape_on_backslash_engines() {
        // A trailing backslash used to turn `'x\'` into an escaped quote on
        // MySQL/ClickHouse, letting the rest of the statement out of the string.
        assert_eq!(quote_literal("mysql", r"x\"), r"'x\\'");
        assert_eq!(
            quote_literal("mysql", r"a\' OR 1=1 -- "),
            r"'a\\'' OR 1=1 -- '"
        );
        assert_eq!(quote_literal("clickhouse", r"x\'"), r"'x\\\''");
        // Standard-strings engines keep backslashes as they are…
        assert_eq!(quote_literal("sqlite", r"C:\x'"), r"'C:\x'''");
        assert_eq!(quote_literal("postgres", "it's"), "'it''s'");
        // …except Postgres uses an E-string so the server setting doesn't matter.
        assert_eq!(quote_literal("postgres", r"a\"), r"E'a\\'");
    }

    #[test]
    fn generated_statements_use_engine_escaping() {
        let pk = vec![("id".to_string(), Some(r"7\".to_string()))];
        let sql = update_sql("mysql", "t", "name", Some(r"v\"), &pk);
        assert_eq!(sql, r"UPDATE `t` SET `name` = 'v\\' WHERE `id` = '7\\'");
        let sql = delete_sql("sqlite", "t", &pk);
        assert_eq!(sql, r"DELETE FROM `t` WHERE `id` = '7\'");
        let sql = browse_sql("mysql", "t", Some(("c", Some(r"\"))), None, 10, 0);
        assert!(sql.contains(r"`c` = '\\'"), "{sql}");
    }

    #[test]
    fn substitute_params_respects_backslash_escapes_like_find_params() {
        // `\'` doesn't end the string, so `:x` inside it is not a parameter.
        let sql = r"SELECT 'it\'s :x' AS s, :y AS y";
        assert_eq!(find_params(sql), vec!["y".to_string()]);
        let mut vals = std::collections::HashMap::new();
        vals.insert("x".to_string(), "X".to_string());
        vals.insert("y".to_string(), r"a\".to_string());
        let out = substitute_params("mysql", sql, &vals);
        assert_eq!(out, r"SELECT 'it\'s :x' AS s, 'a\\' AS y");
    }

    #[test]
    fn ssh_targets_that_look_like_options_are_refused() {
        assert!(ssh_target_ok("deploy", "db.example.com").is_ok());
        assert!(ssh_target_ok("deploy", "-oProxyCommand=touch /tmp/x").is_err());
        assert!(ssh_target_ok("-oProxyCommand=x", "host").is_err());
        assert!(ssh_target_ok("deploy", "host extra").is_err());
        assert!(ssh_target_ok("deploy", "evil@host").is_err());
    }

    #[test]
    fn read_only_sqlite_connection_refuses_writes() {
        let dir = std::env::temp_dir().join(format!("e-db-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ro.sqlite");
        let _ = std::fs::remove_file(&file);
        rusqlite::Connection::open(&file)
            .unwrap()
            .execute_batch(
                "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'a');",
            )
            .unwrap();
        let path = file.to_string_lossy().into_owned();
        let conn = connect(&DbConfig {
            engine: "sqlite".into(),
            path: path.clone(),
            ..Default::default()
        })
        .unwrap();
        conn.set_read_only(true);
        assert!(query(&conn, "SELECT * FROM t", 10).is_ok());
        assert!(query(&conn, "-- c\nSELECT * FROM t", 10).unwrap().is_select);
        assert!(query(&conn, "DELETE FROM t", 0).is_err());
        assert!(query(&conn, "WITH d AS (SELECT 1) DELETE FROM t", 0).is_err());
        assert!(execute_transaction(&conn, &["UPDATE t SET v = 'b'".into()]).is_err());
        // A write the classifier would wave through still fails at the driver,
        // because the file is opened read-only.
        assert!(conn
            .sqlite(&path)
            .unwrap()
            .execute("DELETE FROM t", [])
            .is_err());
        conn.set_read_only(false);
        assert_eq!(
            query(&conn, "DELETE FROM t", 0).unwrap().rows_affected,
            Some(1)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_and_substitute_params() {
        let sql =
            "SELECT * FROM t WHERE id = :id AND name = ':notparam' -- :nope\n  AND x = :id::int";
        // :id found once; the one in the string/comment ignored; ::int cast skipped.
        assert_eq!(find_params(sql), vec!["id".to_string()]);
        let mut vals = std::collections::HashMap::new();
        vals.insert("id".to_string(), "O'x".to_string());
        let out = substitute_params("mysql", sql, &vals);
        assert!(out.contains("id = 'O''x'"), "{out}");
        assert!(out.contains("':notparam'"), "{out}"); // untouched in string
        assert!(
            out.contains(":id::int") || out.contains("'O''x'::int"),
            "{out}"
        );
    }

    #[test]
    fn split_statements_basic_and_edge_cases() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2"),
            vec!["SELECT 1", "SELECT 2"]
        );
        // trailing semicolon + blank fragments dropped
        assert_eq!(split_statements("SELECT 1;;  ;"), vec!["SELECT 1"]);
        // semicolon inside a single-quoted string is not a separator
        assert_eq!(
            split_statements("INSERT INTO t VALUES ('a;b'); SELECT 1"),
            vec!["INSERT INTO t VALUES ('a;b')", "SELECT 1"]
        );
        // doubled-quote escape keeps the string intact
        assert_eq!(
            split_statements("SELECT 'O''Brien; x'; SELECT 2"),
            vec!["SELECT 'O''Brien; x'", "SELECT 2"]
        );
        // backslash escape
        assert_eq!(
            split_statements("SELECT 'a\\'; b'; SELECT 2"),
            vec!["SELECT 'a\\'; b'", "SELECT 2"]
        );
        // line + block comments containing semicolons
        assert_eq!(
            split_statements("SELECT 1 -- c; c\n; SELECT 2"),
            vec!["SELECT 1 -- c; c", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT 1 /* a; b */; SELECT 2"),
            vec!["SELECT 1 /* a; b */", "SELECT 2"]
        );
        // backtick identifier with a semicolon
        assert_eq!(
            split_statements("SELECT `we;ird`; SELECT 2"),
            vec!["SELECT `we;ird`", "SELECT 2"]
        );
        // single statement, no trailing semicolon
        assert_eq!(split_statements("SELECT 1"), vec!["SELECT 1"]);
        assert!(split_statements("  ;; ").is_empty());
    }

    #[test]
    fn split_statement_ranges_map_back_to_text() {
        let sql = "SELECT 1;  SELECT 2 ; SELECT 'a;b'";
        let ranges = split_statement_ranges(sql);
        let slices: Vec<&str> = ranges.iter().map(|(s, e)| &sql[*s..*e]).collect();
        assert_eq!(slices, vec!["SELECT 1", "SELECT 2", "SELECT 'a;b'"]);
        // A cursor inside the second statement resolves to its range.
        let cursor = sql.find("SELECT 2").unwrap() + 3;
        let hit = ranges.iter().find(|(s, e)| *s <= cursor && cursor <= *e);
        assert_eq!(hit.map(|(s, e)| &sql[*s..*e]), Some("SELECT 2"));
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
    fn environment_classification() {
        assert_eq!(cfg("mysql", "127.0.0.1").environment(), Environment::Local);
        assert_eq!(cfg("mysql", "myapp.test").environment(), Environment::Local); // Grove
        assert_eq!(
            cfg("mysql", "db.example.com").environment(),
            Environment::Staging
        ); // remote, no hint
        assert_eq!(
            cfg("mysql", "staging-db.internal").environment(),
            Environment::Staging
        );
        assert_eq!(
            cfg("mysql", "prod-db.internal").environment(),
            Environment::Production
        );
        let mut sqlite = cfg("sqlite", "");
        sqlite.path = "/data/app.sqlite".into();
        assert_eq!(sqlite.environment(), Environment::Local);
        sqlite.path = "/data/production.sqlite".into();
        assert_eq!(sqlite.environment(), Environment::Production);
    }

    #[test]
    fn destructive_detection() {
        assert_eq!(is_destructive("DROP TABLE users"), Some("DROP"));
        assert_eq!(is_destructive("truncate table t"), Some("TRUNCATE"));
        assert_eq!(
            is_destructive("DELETE FROM orders"),
            Some("DELETE without WHERE")
        );
        assert_eq!(is_destructive("DELETE FROM orders WHERE id = 1"), None);
        assert_eq!(
            is_destructive("UPDATE orders SET total = 0"),
            Some("UPDATE without WHERE")
        );
        assert_eq!(is_destructive("UPDATE orders SET x=1 WHERE id=2"), None);
        assert_eq!(is_destructive("SELECT * FROM users"), None);
        assert_eq!(is_destructive("  insert into t values (1)"), None);
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
    fn sqlite_views() {
        let dir = std::env::temp_dir().join("e_db_views_test");
        let _ = std::fs::create_dir_all(&dir);
        let dbfile = dir.join("views.sqlite");
        let _ = std::fs::remove_file(&dbfile);
        {
            let c = rusqlite::Connection::open(&dbfile).unwrap();
            c.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, active INTEGER);\
                 CREATE VIEW active_users AS SELECT id FROM users WHERE active = 1;",
            )
            .unwrap();
        }
        let cfg = DbConfig {
            engine: "sqlite".into(),
            path: dbfile.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let conn = connect(&cfg).unwrap();
        assert_eq!(views(&conn).unwrap(), vec!["active_users".to_string()]);
        // Views are excluded from tables().
        assert_eq!(tables(&conn).unwrap(), vec!["users".to_string()]);
        let def = view_definition(&conn, "active_users").unwrap();
        assert!(def.to_uppercase().contains("SELECT"), "{def}");
        let ddl = table_ddl(&conn, "users").unwrap();
        assert!(ddl.to_uppercase().contains("CREATE TABLE"), "{ddl}");
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

        // referencing (reverse FK): who points at users.id?
        assert_eq!(
            referencing(&conn, "users", "id").unwrap(),
            vec![("posts".to_string(), "user_id".to_string())]
        );
        assert!(referencing(&conn, "posts", "id").unwrap().is_empty());

        // search_all: 'O'Brien' is in posts.title
        let hits = search_all(&conn, "sqlite", "Brien", 50).unwrap();
        assert!(
            hits.iter().any(|(t, _)| t == "posts"),
            "expected a posts hit, got {:?}",
            hits.iter().map(|(t, _)| t).collect::<Vec<_>>()
        );
        assert!(search_all(&conn, "sqlite", "nonexistent-zzz", 50)
            .unwrap()
            .is_empty());

        // A lock another process holds is waited for, not given up on.
        {
            let other = rusqlite::Connection::open(&dbfile).unwrap();
            other.execute_batch("BEGIN EXCLUSIVE").unwrap();
            let release = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                other.execute_batch("COMMIT").unwrap();
            });
            let t = Instant::now();
            let r = query(&conn, "SELECT COUNT(*) FROM users", 10);
            assert!(r.is_ok(), "waited for the lock: {r:?}");
            assert!(
                t.elapsed() >= Duration::from_millis(400),
                "{:?}",
                t.elapsed()
            );
            release.join().unwrap();
        }

        // A statement that would run for ever is interrupted by Cancel.
        {
            let conn = std::sync::Arc::new(connect(&cfg).unwrap());
            let running = conn.clone();
            let t = Instant::now();
            let endless = std::thread::spawn(move || {
                query(
                    &running,
                    "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) \
                     SELECT count(*) FROM c",
                    10,
                )
            });
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(cancel(&conn), Ok(true));
            let err = endless.join().unwrap().unwrap_err();
            assert!(err.contains("interrupted"), "{err}");
            assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
            // Nothing left to interrupt; the next query runs.
            assert!(conn.sqlite_running.lock().unwrap().is_empty());
            assert!(query(&conn, "SELECT 1", 10).is_ok());
        }

        // row_counts (the table tree): SQLite counts.
        let counts = row_counts(&conn, &["users".into(), "posts".into()]).unwrap();
        assert_eq!(
            counts["users"],
            RowCount {
                rows: 1,
                exact: true
            }
        );
        assert_eq!(
            counts["posts"],
            RowCount {
                rows: 1,
                exact: true
            }
        );

        // count_rows (pagination totals)
        assert_eq!(count_rows(&conn, "sqlite", "posts", None).unwrap(), 1);
        assert_eq!(
            count_rows(&conn, "sqlite", "posts", Some(("user_id", Some("1")))).unwrap(),
            1
        );
        assert_eq!(
            count_rows(&conn, "sqlite", "posts", Some(("user_id", Some("999")))).unwrap(),
            0
        );

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
    fn sqlite_transaction_commit_and_rollback() {
        let dir = std::env::temp_dir().join("e_db_tx_test");
        let _ = std::fs::create_dir_all(&dir);
        let dbfile = dir.join("tx.sqlite");
        let _ = std::fs::remove_file(&dbfile);
        {
            let c = rusqlite::Connection::open(&dbfile).unwrap();
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);")
                .unwrap();
        }
        let cfg = DbConfig {
            engine: "sqlite".into(),
            path: dbfile.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let conn = connect(&cfg).unwrap();

        // Commit: both rows land.
        let n = execute_transaction(
            &conn,
            &[
                "INSERT INTO t (id, v) VALUES (1, 'a')".into(),
                "INSERT INTO t (id, v) VALUES (2, 'b')".into(),
            ],
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(table_data(&conn, "sqlite", "t", 100).unwrap().rows.len(), 2);

        // Rollback: the second statement fails (duplicate PK), so neither applies.
        let err = execute_transaction(
            &conn,
            &[
                "INSERT INTO t (id, v) VALUES (3, 'c')".into(),
                "INSERT INTO t (id, v) VALUES (1, 'dup')".into(),
            ],
        );
        assert!(err.is_err());
        assert_eq!(
            table_data(&conn, "sqlite", "t", 100).unwrap().rows.len(),
            2,
            "failed transaction must not add rows"
        );
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
    fn restore_args_are_built_per_engine() {
        let mut c = DbConfig {
            host: "127.0.0.1".into(),
            port: 3306,
            username: "root".into(),
            database: "shop".into(),
            ..Default::default()
        };
        assert_eq!(
            mysql_restore_args(&c),
            vec!["--host=127.0.0.1", "--port=3306", "--user=root", "shop"]
        );
        c.port = 5432;
        assert_eq!(
            pg_restore_args(&c),
            vec![
                "--host=127.0.0.1",
                "--port=5432",
                "--username=root",
                "--dbname",
                "shop"
            ]
        );
    }

    #[test]
    fn restore_sqlite_copies_the_file_back() {
        let dir = std::env::temp_dir().join(format!("e-db-restore-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let live = dir.join("live.sqlite");
        let snap = dir.join("snap.sqlite");
        std::fs::write(&live, b"CURRENT").unwrap();
        std::fs::write(&snap, b"SNAPSHOT").unwrap();

        let c = DbConfig {
            engine: "sqlite".into(),
            path: live.to_string_lossy().into_owned(),
            ..Default::default()
        };

        restore_snapshot(&c, &snap).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"SNAPSHOT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_errors_on_missing_snapshot_and_unknown_engine() {
        let mut c = DbConfig {
            engine: "sqlite".into(),
            path: "/tmp/whatever.sqlite".into(),
            ..Default::default()
        };
        assert!(restore_snapshot(&c, Path::new("/no/such/snapshot.sqlite")).is_err());

        let dir = std::env::temp_dir().join(format!("e-db-restore2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let snap = dir.join("s.dump");
        std::fs::write(&snap, b"x").unwrap();
        c.engine = "clickhouse".into();
        assert!(restore_snapshot(&c, &snap).is_err());
        let _ = std::fs::remove_dir_all(&dir);
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
