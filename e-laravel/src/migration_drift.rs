//! Whether the database is where the checked-out code expects it. Switch to a
//! branch with two new migrations and the app fails on a missing column until
//! `migrate` runs; switch back and the database has a migration this branch
//! knows nothing about — a column it may not expect, which `migrate:rollback`
//! can't undo from here, the file being on the other branch.
//!
//! Compares the migration files (the project's, and those packages load from
//! `vendor/`) with the `migrations` table's rows.

use std::path::Path;

/// What the database and the checkout disagree on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drift {
    /// Migrations in the checkout that haven't run, oldest first.
    pub pending: Vec<String>,
    /// Migrations that ran but aren't in the checkout: from another branch.
    pub foreign: Vec<String>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.foreign.is_empty()
    }

    /// What the banner says.
    pub fn summary(&self) -> String {
        let n = |count: usize, one: &str, many: &str| {
            if count == 1 {
                format!("1 {one}")
            } else {
                format!("{count} {many}")
            }
        };
        let pending = (!self.pending.is_empty()).then(|| {
            format!(
                "{} on this branch {} run",
                n(self.pending.len(), "migration", "migrations"),
                if self.pending.len() == 1 {
                    "hasn't"
                } else {
                    "haven't"
                }
            )
        });
        let foreign = (!self.foreign.is_empty()).then(|| {
            let first = &self.foreign[0];
            let more = match self.foreign.len() {
                1 => String::new(),
                k => format!(" and {} more", k - 1),
            };
            format!("the database has {first}{more}, which this branch doesn't")
        });
        match (pending, foreign) {
            (Some(p), Some(f)) => format!("{p}, and {f}"),
            (Some(p), None) => p,
            (None, Some(f)) => {
                let mut f = f;
                f.replace_range(..1, &f[..1].to_uppercase());
                f
            }
            (None, None) => String::new(),
        }
    }
}

/// The migrations `root` has: `database/migrations/*.php` and the packages'
/// `vendor/<vendor>/<package>/database/migrations/*.php`, by name.
pub fn migration_names(root: &Path) -> Vec<String> {
    let mut dirs = vec![root.join("database/migrations")];
    for vendor in read_dirs(&root.join("vendor")) {
        for package in read_dirs(&vendor) {
            dirs.push(package.join("database/migrations"));
        }
    }
    let mut out: Vec<String> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".php").map(str::to_string)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn read_dirs(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Where `files` (the checkout's migrations) and `ran` (the table's rows)
/// part ways. Only the project's own files can be pending: a package's
/// migration that hasn't run may be one it doesn't load.
pub fn drift(files: &[String], own: &[String], ran: &[String]) -> Drift {
    let mut pending: Vec<String> = own.iter().filter(|f| !ran.contains(f)).cloned().collect();
    pending.sort();
    let mut foreign: Vec<String> = ran.iter().filter(|r| !files.contains(r)).cloned().collect();
    foreign.sort();
    Drift { pending, foreign }
}

/// The project's own migrations: `database/migrations/*.php`, by name.
pub fn own_migration_names(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join("database/migrations"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".php").map(str::to_string)
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pending_and_foreign_are_told_apart() {
        let own = names(&[
            "2024_01_01_000000_create_users",
            "2026_10_01_000000_add_sku",
        ]);
        let mut files = own.clone();
        files.push("2016_06_01_000001_create_oauth_auth_codes".into());
        let ran = names(&[
            "2024_01_01_000000_create_users",
            "2016_06_01_000001_create_oauth_auth_codes",
            "2026_09_01_000000_add_discount",
        ]);
        let d = drift(&files, &own, &ran);
        assert_eq!(d.pending, names(&["2026_10_01_000000_add_sku"]));
        assert_eq!(d.foreign, names(&["2026_09_01_000000_add_discount"]));
        assert_eq!(
            d.summary(),
            "1 migration on this branch hasn't run, and the database has 2026_09_01_000000_add_discount, which this branch doesn't"
        );
        // A package's migration not run isn't pending: it may not load it.
        let d = drift(
            &files,
            &own,
            &names(&[
                "2024_01_01_000000_create_users",
                "2026_10_01_000000_add_sku",
            ]),
        );
        assert!(d.is_empty());
    }

    #[test]
    fn the_summary_reads_for_each_side() {
        let d = Drift {
            pending: names(&["a", "b"]),
            foreign: Vec::new(),
        };
        assert_eq!(d.summary(), "2 migrations on this branch haven't run");
        let d = Drift {
            pending: Vec::new(),
            foreign: names(&["x", "y", "z"]),
        };
        assert_eq!(
            d.summary(),
            "The database has x and 2 more, which this branch doesn't"
        );
    }

    #[test]
    fn vendor_migrations_count_as_files() {
        let root = std::env::temp_dir().join(format!("e-drift-{}", std::process::id()));
        std::fs::create_dir_all(root.join("database/migrations")).unwrap();
        std::fs::create_dir_all(root.join("vendor/laravel/cashier/database/migrations")).unwrap();
        std::fs::write(
            root.join("database/migrations/2024_01_01_000000_create_users.php"),
            "",
        )
        .unwrap();
        std::fs::write(
            root.join("vendor/laravel/cashier/database/migrations/2019_05_03_000001_create_customer_columns.php"),
            "",
        )
        .unwrap();
        assert_eq!(
            migration_names(&root),
            names(&[
                "2019_05_03_000001_create_customer_columns",
                "2024_01_01_000000_create_users"
            ])
        );
        assert_eq!(
            own_migration_names(&root),
            names(&["2024_01_01_000000_create_users"])
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
