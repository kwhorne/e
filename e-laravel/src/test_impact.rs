//! Which tests a change touches: from a saved PHP file to the test files
//! worth running, so a save can run those few instead of the whole suite.
//!
//! The rules are deliberately plain, and each one is a reason a developer
//! would recognise:
//!
//! - a test file is its own test;
//! - a test that `use`s the class, or names it in full;
//! - a test named after the class (`OrderTest.php` for `Order`);
//! - for a controller, a test that calls one of its routes by name
//!   (`route('orders.store')`) or by path (`->post('/orders')`).
//!
//! It finds tests that exercise a class directly. One reached only through
//! other classes is missed — that is what the full suite is for.

use std::path::{Path, PathBuf};

use crate::laravel::RouteInfo;

/// The fully qualified name of the class, interface, trait or enum `src`
/// declares.
pub fn declared_type(src: &str) -> Option<String> {
    let ns = src.lines().find_map(|l| {
        l.trim()
            .strip_prefix("namespace ")
            .map(|n| n.trim_end_matches(';').trim().to_string())
    });
    let name = src.lines().find_map(|l| {
        let t = l.trim();
        let t = ["final ", "abstract ", "readonly "]
            .iter()
            .fold(t, |t, m| t.strip_prefix(m).unwrap_or(t));
        ["class ", "interface ", "trait ", "enum "]
            .iter()
            .find_map(|k| t.strip_prefix(k))
            .map(|r| {
                r.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .filter(|n| !n.is_empty())
    })?;
    Some(match ns {
        Some(ns) => format!("{ns}\\{name}"),
        None => name,
    })
}

/// Whether `path` (relative to the project) is a test file.
pub fn is_test_file(rel: &Path) -> bool {
    rel.starts_with("tests")
        && rel
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("Test.php"))
}

/// The test files among `tests` — `(path relative to the project, text)` —
/// that the file at `rel`, now holding `src`, touches. Sorted, no repeats.
pub fn affected_tests(
    rel: &Path,
    src: &str,
    tests: &[(PathBuf, String)],
    routes: &[RouteInfo],
) -> Vec<PathBuf> {
    if is_test_file(rel) {
        return vec![rel.to_path_buf()];
    }
    let Some(fqcn) = declared_type(src) else {
        return Vec::new();
    };
    let short = fqcn.rsplit('\\').next().unwrap_or(&fqcn).to_string();
    let named = format!("{short}Test.php");

    // What a test would write to reach this controller's routes: the name,
    // and the path up to its first parameter (`/orders/{order}` is called as
    // `/orders/{$order->id}` or `/orders/1`).
    let mut names: Vec<String> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    for r in routes.iter().filter(|r| action_class(&r.action) == fqcn) {
        if !r.name.is_empty() {
            names.push(format!("route('{}'", r.name));
            names.push(format!("route(\"{}\"", r.name));
        }
        let path = r.uri.split('{').next().unwrap_or("").trim_matches('/');
        if !path.is_empty() {
            paths.push(format!("/{path}"));
        }
    }

    let mut out: Vec<PathBuf> = tests
        .iter()
        .filter(|(path, text)| {
            path.file_name().and_then(|n| n.to_str()) == Some(named.as_str())
                || uses(text, &fqcn)
                || names.iter().any(|n| text.contains(n.as_str()))
                || paths.iter().any(|p| calls_path(text, p))
        })
        .map(|(path, _)| path.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The class of a route action: `App\Http\Controllers\OrderController@store`
/// → `App\Http\Controllers\OrderController`.
fn action_class(action: &str) -> &str {
    action
        .trim_start_matches('\\')
        .split('@')
        .next()
        .unwrap_or("")
}

/// Whether `text` has `path` as a string of its own or the start of one that
/// goes on into the route (`'/orders'`, `"/orders/{$id}"`, `'/orders?page=2'`)
/// — not `'/orders-export'`.
fn calls_path(text: &str, path: &str) -> bool {
    ['\'', '"'].iter().any(|&quote| {
        let needle = format!("{quote}{path}");
        text.match_indices(&needle).any(|(i, _)| {
            matches!(
                text[i + needle.len()..].chars().next(),
                Some(c) if c == quote || matches!(c, '/' | '?' | '#')
            )
        })
    })
}

/// Whether `text` imports `fqcn` or names it in full.
fn uses(text: &str, fqcn: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        t.strip_prefix("use ")
            .map(|u| u.trim_end_matches(';').trim())
            .map(|u| u.split(" as ").next().unwrap_or(u).trim())
            .is_some_and(|u| u.trim_start_matches('\\') == fqcn)
    }) || text.contains(&format!("\\{fqcn}::"))
}

/// The test files under `root/tests`, with their text, paths relative to
/// `root`.
pub fn test_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("tests")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                if is_test_file(rel) {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        out.push((rel.to_path_buf(), text));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test(path: &str, text: &str) -> (PathBuf, String) {
        (PathBuf::from(path), text.to_string())
    }

    fn route(name: &str, uri: &str, action: &str) -> RouteInfo {
        RouteInfo {
            name: name.into(),
            uri: uri.into(),
            methods: "GET|HEAD".into(),
            action: action.into(),
            middleware: "web".into(),
        }
    }

    fn suite() -> Vec<(PathBuf, String)> {
        vec![
            test(
                "tests/Unit/OrderTest.php",
                "<?php\nit('totals', function () {});\n",
            ),
            test(
                "tests/Feature/CheckoutTest.php",
                "<?php\nuse App\\Models\\Order;\nuse App\\Models\\User;\n\nit('checks out', function () {\n    $this->post(route('orders.store'));\n});\n",
            ),
            test(
                "tests/Feature/OrderPagesTest.php",
                "<?php\nit('lists orders', function () {\n    $this->get('/orders')->assertOk();\n    $this->get(\"/orders/{$order->id}\")->assertOk();\n});\n",
            ),
            test(
                "tests/Feature/InvoiceTest.php",
                "<?php\nuse App\\Models\\Invoice;\nuse App\\Models\\OrderLine;\n\nit('bills', function () {\n    $this->get('/orders-export');\n});\n",
            ),
        ]
    }

    #[test]
    fn a_test_file_runs_itself() {
        let rel = Path::new("tests/Feature/InvoiceTest.php");
        assert_eq!(affected_tests(rel, "", &suite(), &[]), [rel]);
    }

    #[test]
    fn a_model_reaches_the_tests_that_use_it_or_are_named_for_it() {
        let src = "<?php\n\nnamespace App\\Models;\n\nclass Order extends Model\n{\n}\n";
        let found = affected_tests(Path::new("app/Models/Order.php"), src, &suite(), &[]);
        assert_eq!(
            found,
            [
                PathBuf::from("tests/Feature/CheckoutTest.php"),
                PathBuf::from("tests/Unit/OrderTest.php"),
            ]
        );
    }

    #[test]
    fn a_controller_reaches_the_tests_that_call_its_routes() {
        let src = "<?php\n\nnamespace App\\Http\\Controllers;\n\nfinal class OrderController extends Controller\n{\n}\n";
        let routes = [
            route(
                "orders.store",
                "orders",
                "App\\Http\\Controllers\\OrderController@store",
            ),
            route(
                "orders.show",
                "orders/{order}",
                "App\\Http\\Controllers\\OrderController@show",
            ),
            route(
                "invoices.index",
                "invoices",
                "App\\Http\\Controllers\\InvoiceController@index",
            ),
        ];
        let found = affected_tests(
            Path::new("app/Http/Controllers/OrderController.php"),
            src,
            &suite(),
            &routes,
        );
        // Not InvoiceTest: `/orders-export` is another route.
        assert_eq!(
            found,
            [
                PathBuf::from("tests/Feature/CheckoutTest.php"),
                PathBuf::from("tests/Feature/OrderPagesTest.php"),
            ]
        );
    }

    #[test]
    fn an_import_must_be_the_class_itself() {
        // `OrderLine` is not `Order`.
        let src = "<?php\nnamespace App\\Models;\nclass Order {}\n";
        let tests = [test(
            "tests/Feature/LinesTest.php",
            "<?php\nuse App\\Models\\OrderLine;\nuse App\\Models\\Order as Ord;\n",
        )];
        assert_eq!(
            affected_tests(Path::new("app/Models/Order.php"), src, &tests, &[]),
            [PathBuf::from("tests/Feature/LinesTest.php")]
        );
        let tests = [test(
            "tests/Feature/LinesTest.php",
            "<?php\nuse App\\Models\\OrderLine;\n",
        )];
        assert!(affected_tests(Path::new("app/Models/Order.php"), src, &tests, &[]).is_empty());
    }

    #[test]
    fn interfaces_traits_and_enums_are_types_too() {
        assert_eq!(
            declared_type("<?php\nnamespace App\\Enums;\nenum Status: string {}\n").as_deref(),
            Some("App\\Enums\\Status")
        );
        assert_eq!(
            declared_type("<?php\nnamespace App\\Concerns;\ntrait HasTeam {}\n").as_deref(),
            Some("App\\Concerns\\HasTeam")
        );
        assert_eq!(declared_type("<?php\nreturn [];\n"), None);
    }
}
