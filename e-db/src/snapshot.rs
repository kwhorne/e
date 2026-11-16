//! Database snapshots (DB-704): a copy of a local database to go back to —
//! before a migration, an import, an agent's change.
//!
//! - SQLite: `VACUUM INTO` a new file, which takes what's in the
//!   write-ahead log too (a plain file copy misses it), and back through
//!   SQLite's backup API, which writes the whole database consistently.
//! - MySQL: `mysqldump`, replayed through `mysql`.
//! - PostgreSQL: `pg_dump --clean --if-exists`, so replaying it drops what
//!   it then makes, replayed through `psql` in one transaction that stops
//!   at the first error.
//!
//! Each connection's snapshots have a folder of their own, named by the
//! database and a hash of where it is, so two projects' `app` databases
//! never share one — restoring one into the other would be a disaster.

use std::path::{Path, PathBuf};

use crate::DbConfig;

/// A snapshot on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub path: PathBuf,
    /// When it was taken, in seconds since the epoch.
    pub taken: u64,
    pub bytes: u64,
}

fn engine_family(engine: &str) -> &str {
    match engine {
        "mysql" | "mariadb" => "mysql",
        "postgres" | "postgresql" | "pgsql" => "postgres",
        other => other,
    }
}

/// FNV-1a: a stable hash (`DefaultHasher` may change between releases, and
/// the folder must stay the same).
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The folder `config`'s snapshots go in, under `root`: named by the
/// database, not the connection's label, so the same database found from
/// `.env` (before a migration) and from the panel share it.
pub fn snapshot_folder(root: &Path, config: &DbConfig) -> PathBuf {
    let name = if !config.database.trim().is_empty() {
        config.database.clone()
    } else {
        Path::new(&config.path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "database".into())
    };
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    // A file by its real path: `/tmp/x` and `/private/tmp/x` are one.
    let path = std::fs::canonicalize(&config.path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| config.path.clone());
    let identity = format!(
        "{}|{}|{}|{}|{path}",
        engine_family(&config.engine),
        config.host,
        config.port,
        config.database,
    );
    root.join(format!("{name}-{:08x}", (fnv1a(&identity) >> 32) as u32))
}

/// `mysqldump`'s arguments for `config`'s database.
fn mysqldump_args(config: &DbConfig) -> Vec<String> {
    vec![
        format!("--host={}", config.host),
        format!("--port={}", config.port),
        format!("--user={}", config.username),
        // One consistent read of InnoDB tables, without locking them.
        "--single-transaction".into(),
        "--routines".into(),
        "--triggers".into(),
        config.database.clone(),
    ]
}

/// `pg_dump`'s arguments: the dump drops each object before making it, so
/// it can be replayed over the database it came from.
fn pg_dump_args(config: &DbConfig) -> Vec<String> {
    vec![
        format!("--host={}", config.host),
        format!("--port={}", config.port),
        format!("--username={}", config.username),
        "--clean".into(),
        "--if-exists".into(),
        config.database.clone(),
    ]
}

/// `mysql`'s arguments for replaying a dump (it stops at the first error).
fn mysql_restore_args(config: &DbConfig) -> Vec<String> {
    vec![
        format!("--host={}", config.host),
        format!("--port={}", config.port),
        format!("--user={}", config.username),
        config.database.clone(),
    ]
}

/// `psql`'s arguments for replaying a dump: in one transaction, stopping
/// at the first error, so a dump that doesn't fit changes nothing.
fn pg_restore_args(config: &DbConfig) -> Vec<String> {
    vec![
        format!("--host={}", config.host),
        format!("--port={}", config.port),
        format!("--username={}", config.username),
        "--set=ON_ERROR_STOP=1".into(),
        "--single-transaction".into(),
        "--quiet".into(),
        "--dbname".into(),
        config.database.clone(),
    ]
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Take a snapshot of `config`'s database into its folder under `root`.
pub fn take_snapshot(config: &DbConfig, root: &Path) -> Result<PathBuf, String> {
    let dir = snapshot_folder(root, config);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ext = if engine_family(&config.engine) == "sqlite" {
        "sqlite"
    } else {
        "sql"
    };
    // Named by when it was taken; a second in the same second gets a suffix.
    let stamp = now_secs();
    let mut dest = dir.join(format!("{stamp}.{ext}"));
    let mut n = 1;
    while dest.exists() {
        dest = dir.join(format!("{stamp}-{n}.{ext}"));
        n += 1;
    }
    let dump = |program: &str, args: Vec<String>, env: (&str, &str)| {
        let out = std::process::Command::new(program)
            .env(env.0, env.1)
            .args(args)
            .output()
            .map_err(|e| format!("{program}: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        std::fs::write(&dest, out.stdout).map_err(|e| e.to_string())
    };
    match engine_family(&config.engine) {
        "sqlite" => {
            if config.path.trim().is_empty() {
                return Err("the SQLite connection has no file".into());
            }
            let c = rusqlite::Connection::open_with_flags(
                &config.path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| e.to_string())?;
            c.execute("VACUUM INTO ?1", [dest.to_string_lossy()])
                .map_err(|e| e.to_string())?;
        }
        "mysql" => dump(
            "mysqldump",
            mysqldump_args(config),
            ("MYSQL_PWD", &config.password),
        )?,
        "postgres" => dump(
            "pg_dump",
            pg_dump_args(config),
            ("PGPASSWORD", &config.password),
        )?,
        other => return Err(format!("snapshots aren't supported for {other}")),
    }
    Ok(dest)
}

/// `config`'s snapshots under `root`, newest first.
pub fn list_snapshots(root: &Path, config: &DbConfig) -> Vec<Snapshot> {
    let Ok(entries) = std::fs::read_dir(snapshot_folder(root, config)) else {
        return Vec::new();
    };
    let mut out: Vec<Snapshot> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            let taken = stem.split('-').next()?.parse().ok()?;
            Some(Snapshot {
                path,
                taken,
                bytes: meta.len(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.taken.cmp(&a.taken).then(b.path.cmp(&a.path)));
    out
}

fn run_sql_file(
    program: &str,
    args: &[String],
    env: (&str, &str),
    file: &Path,
) -> Result<(), String> {
    let input = std::fs::File::open(file).map_err(|e| format!("open snapshot: {e}"))?;
    let out = std::process::Command::new(program)
        .args(args)
        .env(env.0, env.1)
        .stdin(std::process::Stdio::from(input))
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// Put `config`'s database back as `snapshot` has it. This overwrites the
/// database: callers allow it on local connections only, and confirm.
pub fn restore_snapshot(config: &DbConfig, snapshot: &Path) -> Result<(), String> {
    if !snapshot.is_file() {
        return Err(format!("snapshot not found: {}", snapshot.display()));
    }
    match engine_family(&config.engine) {
        "sqlite" => {
            if config.path.trim().is_empty() {
                return Err("the SQLite connection has no file".into());
            }
            let mut c = rusqlite::Connection::open(&config.path).map_err(|e| e.to_string())?;
            c.restore(
                rusqlite::DatabaseName::Main,
                snapshot,
                None::<fn(rusqlite::backup::Progress)>,
            )
            .map_err(|e| e.to_string())
        }
        "mysql" => run_sql_file(
            "mysql",
            &mysql_restore_args(config),
            ("MYSQL_PWD", &config.password),
            snapshot,
        ),
        "postgres" => run_sql_file(
            "psql",
            &pg_restore_args(config),
            ("PGPASSWORD", &config.password),
            snapshot,
        ),
        other => Err(format!("restore isn't supported for {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("e-db-snap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn each_database_has_a_folder_of_its_own() {
        let root = Path::new("/snaps");
        let a = DbConfig {
            engine: "mysql".into(),
            host: "127.0.0.1".into(),
            port: 3306,
            database: "app".into(),
            ..Default::default()
        };
        let b = DbConfig {
            port: 3307,
            ..a.clone()
        };
        let fa = snapshot_folder(root, &a);
        assert!(fa.to_string_lossy().starts_with("/snaps/app-"), "{fa:?}");
        assert_ne!(fa, snapshot_folder(root, &b), "same name, another server");
        // A connection's label doesn't move it: `.env` and the panel agree.
        let labelled = DbConfig {
            label: "Shop (local)".into(),
            ..a.clone()
        };
        assert_eq!(snapshot_folder(root, &labelled), fa);
        let sqlite = DbConfig {
            engine: "sqlite".into(),
            path: "/data/my app.sqlite".into(),
            ..Default::default()
        };
        assert!(snapshot_folder(root, &sqlite)
            .to_string_lossy()
            .starts_with("/snaps/my_app-"));
        // And it stays the same from one run to the next.
        assert_eq!(fa, snapshot_folder(root, &a.clone()));
    }

    #[test]
    fn dumps_and_replays_are_made_to_fit_each_other() {
        let c = DbConfig {
            host: "127.0.0.1".into(),
            port: 5432,
            username: "root".into(),
            database: "shop".into(),
            ..Default::default()
        };
        assert!(pg_dump_args(&c).contains(&"--clean".to_string()));
        assert!(pg_dump_args(&c).contains(&"--if-exists".to_string()));
        let restore = pg_restore_args(&c);
        assert!(restore.contains(&"--single-transaction".to_string()));
        assert!(restore.contains(&"--set=ON_ERROR_STOP=1".to_string()));
        assert_eq!(restore.last().map(String::as_str), Some("shop"));
        assert_eq!(
            mysql_restore_args(&c),
            ["--host=127.0.0.1", "--port=5432", "--user=root", "shop"]
        );
        assert!(mysqldump_args(&c).contains(&"--single-transaction".to_string()));
    }

    #[test]
    fn a_sqlite_snapshot_takes_the_log_too_and_restores() {
        let dir = scratch("sqlite");
        let live = dir.join("live.sqlite");
        let config = DbConfig {
            engine: "sqlite".into(),
            path: live.to_string_lossy().into_owned(),
            ..Default::default()
        };
        // A writer in WAL mode that hasn't checkpointed: the row is only
        // in the log, which a plain copy of the file would miss.
        let writer = rusqlite::Connection::open(&live).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('kept');",
            )
            .unwrap();
        let snap = take_snapshot(&config, &dir.join("snaps")).unwrap();
        let in_snapshot: String = rusqlite::Connection::open(&snap)
            .unwrap()
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(in_snapshot, "kept");

        writer
            .execute_batch("DELETE FROM t; CREATE TABLE later (x);")
            .unwrap();
        drop(writer);
        restore_snapshot(&config, &snap).unwrap();
        let c = rusqlite::Connection::open(&live).unwrap();
        let rows: Vec<String> = c
            .prepare("SELECT v FROM t")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows, ["kept"]);
        let later: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'later'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(later, 0, "the restore puts back the whole database");

        // Listed newest first, two in the same second kept apart.
        let second = take_snapshot(&config, &dir.join("snaps")).unwrap();
        assert_ne!(second, snap);
        let listed: Vec<PathBuf> = list_snapshots(&dir.join("snaps"), &config)
            .into_iter()
            .map(|s| s.path)
            .collect();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&snap) && listed.contains(&second));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_refuses_a_missing_file_and_clickhouse() {
        let dir = scratch("refuse");
        let mut c = DbConfig {
            engine: "sqlite".into(),
            path: dir.join("x.sqlite").to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert!(restore_snapshot(&c, &dir.join("missing.sqlite")).is_err());
        let snap = dir.join("s.sql");
        std::fs::write(&snap, b"x").unwrap();
        c.engine = "clickhouse".into();
        assert!(restore_snapshot(&c, &snap).is_err());
        assert!(take_snapshot(&c, &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
