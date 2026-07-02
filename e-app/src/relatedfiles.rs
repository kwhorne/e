//! "Related files": jump between the model, migration, factory, seeder,
//! controller, policy, request, resource, and test that belong to the same
//! Laravel resource. The naming conventions make the mapping deterministic.

use std::path::{Path, PathBuf};

use crate::eloquent::{pluralize, snake_case};

/// Infer the resource name (`User`) from any related file's path.
pub fn resource_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;

    // Migration: `…_create_users_table` / `…_add_x_to_users_table`.
    if let Some(name) = stem
        .split("create_")
        .nth(1)
        .and_then(|s| s.strip_suffix("_table"))
    {
        return Some(pascal(&singularize(name)));
    }
    if let Some(table) = stem.rsplit("_table").nth(1).map(|_| stem) {
        if let Some(t) = table
            .rsplit("_to_")
            .next()
            .and_then(|s| s.strip_suffix("_table"))
        {
            return Some(pascal(&singularize(t)));
        }
    }

    // Strip a known type suffix.
    for suffix in [
        "Controller",
        "Factory",
        "Seeder",
        "Policy",
        "Resource",
        "Test",
    ] {
        if let Some(base) = stem.strip_suffix(suffix) {
            if !base.is_empty() {
                return Some(base.to_string());
            }
        }
    }
    // FormRequest: `StoreUserRequest` → strip verb + `Request`.
    if let Some(base) = stem.strip_suffix("Request") {
        for verb in ["Store", "Update", "Create", "Delete", "Edit"] {
            if let Some(b) = base.strip_prefix(verb) {
                if !b.is_empty() {
                    return Some(b.to_string());
                }
            }
        }
        if !base.is_empty() {
            return Some(base.to_string());
        }
    }
    // Otherwise treat it as the model name itself.
    Some(stem.to_string())
}

/// The related files that actually exist for `name`, as `(kind, path)`.
pub fn related(root: &Path, name: &str) -> Vec<(String, PathBuf)> {
    let table = pluralize(&snake_case(name));
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut add = |kind: &str, p: PathBuf| {
        if p.is_file() && !out.iter().any(|(_, x)| *x == p) {
            out.push((kind.to_string(), p));
        }
    };

    add("Model", root.join(format!("app/Models/{name}.php")));
    add("Model", root.join(format!("app/{name}.php")));
    add(
        "Controller",
        root.join(format!("app/Http/Controllers/{name}Controller.php")),
    );
    add(
        "Factory",
        root.join(format!("database/factories/{name}Factory.php")),
    );
    add(
        "Seeder",
        root.join(format!("database/seeders/{name}Seeder.php")),
    );
    add(
        "Policy",
        root.join(format!("app/Policies/{name}Policy.php")),
    );
    add(
        "Resource",
        root.join(format!("app/Http/Resources/{name}Resource.php")),
    );
    add("Test", root.join(format!("tests/Feature/{name}Test.php")));
    add("Test", root.join(format!("tests/Unit/{name}Test.php")));
    add(
        "Test",
        root.join(format!("tests/Feature/{name}ControllerTest.php")),
    );

    // Migrations: files whose name mentions the table.
    let mig = format!("_{table}_table");
    scan(&root.join("database/migrations"), &mut |p| {
        if p.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.contains(&mig))
            .unwrap_or(false)
        {
            out.push(("Migration".to_string(), p.to_path_buf()));
        }
    });
    // FormRequests mentioning the name.
    scan(&root.join("app/Http/Requests"), &mut |p| {
        if p.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.contains(name))
            .unwrap_or(false)
        {
            out.push(("Request".to_string(), p.to_path_buf()));
        }
    });
    out
}

fn scan(dir: &Path, f: &mut dyn FnMut(&Path)) {
    if let Ok(read) = std::fs::read_dir(dir) {
        let mut paths: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_file() {
                f(&p);
            }
        }
    }
}

fn pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn singularize(s: &str) -> String {
    if let Some(base) = s.strip_suffix("ies") {
        format!("{base}y")
    } else if let Some(base) = s.strip_suffix("es") {
        if base.ends_with('s')
            || base.ends_with('x')
            || base.ends_with("ch")
            || base.ends_with("sh")
        {
            base.to_string()
        } else {
            format!("{base}e")
        }
    } else if let Some(base) = s.strip_suffix('s') {
        base.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_resource_name() {
        assert_eq!(
            resource_name(Path::new("app/Models/User.php")).as_deref(),
            Some("User")
        );
        assert_eq!(
            resource_name(Path::new("app/Http/Controllers/UserController.php")).as_deref(),
            Some("User")
        );
        assert_eq!(
            resource_name(Path::new("database/factories/UserFactory.php")).as_deref(),
            Some("User")
        );
        assert_eq!(
            resource_name(Path::new("2024_01_01_000000_create_users_table.php")).as_deref(),
            Some("User")
        );
        assert_eq!(
            resource_name(Path::new("app/Http/Requests/StoreUserRequest.php")).as_deref(),
            Some("User")
        );
    }

    #[test]
    fn singularizes() {
        assert_eq!(singularize("users"), "user");
        assert_eq!(singularize("categories"), "category");
        assert_eq!(singularize("classes"), "class");
    }
}
