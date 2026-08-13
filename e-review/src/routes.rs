//! Which routes does this changeset actually touch?
//!
//! The session review knows *what* changed and the runtime capture knows what a
//! request *costs*, but nothing joined the two. This is that join: given the
//! changed files and the project's route table, work out which routes a
//! reviewer should replay to see whether the change did what it claimed.
//!
//! Attribution is deliberately conservative. Every route reported comes with the
//! reason it was picked, because an attribution you cannot check is not
//! evidence — and a route list padded with guesses would make the measurement
//! that follows meaningless. What can't be traced is reported as unattributed
//! rather than hidden.

use crate::FileChange;

/// A route, reduced to what attribution needs.
///
/// Mirrors the editor's `RouteInfo` (from `php artisan route:list`) without
/// dragging the rest of the Laravel layer into this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub name: String,
    pub uri: String,
    /// `GET|HEAD`, `POST`, …
    pub methods: String,
    /// `App\Http\Controllers\OrderController@update`, or `Closure`.
    pub action: String,
}

impl Route {
    /// The controller's short class name, if this route has one.
    ///
    /// `App\Http\Controllers\OrderController@update` → `OrderController`.
    /// Invokable controllers (no `@method`) work too.
    pub fn controller_class(&self) -> Option<&str> {
        let class = self.action.split('@').next()?.trim();
        if class.is_empty() || class == "Closure" {
            return None;
        }
        let short = class.rsplit('\\').next()?.trim();
        (!short.is_empty()).then_some(short)
    }

    /// A GET route can be replayed safely; a POST or DELETE would change data.
    pub fn is_safe_to_replay(&self) -> bool {
        self.methods
            .split('|')
            .map(|m| m.trim().to_uppercase())
            .all(|m| matches!(m.as_str(), "GET" | "HEAD" | "OPTIONS"))
    }
}

/// Why a route was attributed to the changeset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// A controller the route dispatches to was changed.
    Controller { path: String, class: String },
    /// A routes file changed, and this route's uri or name appears in the diff.
    RouteFile { path: String },
}

impl Reason {
    /// One line, phrased for a reviewer reading the ship gate.
    pub fn describe(&self) -> String {
        match self {
            Reason::Controller { path, class } => format!("{class} changed ({path})"),
            Reason::RouteFile { path } => format!("declared in {path}, which changed"),
        }
    }
}

/// A route the changeset reaches, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Affected {
    pub route: Route,
    pub reason: Reason,
}

/// The result of attributing a changeset to routes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attribution {
    /// Routes worth replaying, in route-table order.
    pub affected: Vec<Affected>,
    /// Changed files that could not be traced to any route, in changeset order.
    ///
    /// Reported rather than dropped: "we measured 3 routes" means something
    /// different when 40 files went unattributed, and the reviewer should see
    /// which.
    pub unattributed: Vec<String>,
}

impl Attribution {
    pub fn is_empty(&self) -> bool {
        self.affected.is_empty()
    }

    /// The affected routes that can be replayed without changing data.
    pub fn replayable(&self) -> impl Iterator<Item = &Affected> {
        self.affected.iter().filter(|a| a.route.is_safe_to_replay())
    }
}

/// Does this path look like a PHP controller?
fn controller_class_of(path: &str) -> Option<&str> {
    let file = path.rsplit('/').next()?;
    let stem = file.strip_suffix(".php")?;
    let in_controllers_dir = path.contains("app/Http/Controllers/");
    (in_controllers_dir || stem.ends_with("Controller")).then_some(stem)
}

/// Does this path declare routes?
fn is_route_file(path: &str) -> bool {
    path.starts_with("routes/") && path.ends_with(".php")
}

/// The `+`/`-` lines of a change, without the markers.
fn changed_lines(change: &FileChange) -> Vec<&str> {
    change
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter_map(|l| {
            let rest = l.strip_prefix('+').or_else(|| l.strip_prefix('-'))?;
            // `+++`/`---` file headers are not content.
            (!rest.starts_with("++") && !rest.starts_with("--")).then_some(rest)
        })
        .collect()
}

/// Attribute `changes` to `routes`.
///
/// A route is reported at most once; the first reason found wins, so a
/// controller match (the stronger signal) is preferred over a routes-file
/// mention when both apply.
pub fn attribute(changes: &[FileChange], routes: &[Route]) -> Attribution {
    let mut out = Attribution::default();
    let mut claimed: Vec<bool> = vec![false; routes.len()];

    // Pass 1: controllers. The class name is a direct, checkable link.
    for change in changes {
        let Some(class) = controller_class_of(&change.path) else {
            continue;
        };
        for (i, route) in routes.iter().enumerate() {
            if claimed[i] || route.controller_class() != Some(class) {
                continue;
            }
            claimed[i] = true;
            out.affected.push(Affected {
                route: route.clone(),
                reason: Reason::Controller {
                    path: change.path.clone(),
                    class: class.to_string(),
                },
            });
        }
    }

    // Pass 2: routes files. Only routes whose uri or name is mentioned in the
    // diff — a changed `routes/web.php` does not make every route suspect.
    for change in changes {
        if !is_route_file(&change.path) {
            continue;
        }
        let lines = changed_lines(change);
        for (i, route) in routes.iter().enumerate() {
            if claimed[i] {
                continue;
            }
            let mentioned = lines.iter().any(|l| {
                (!route.uri.is_empty() && l.contains(&route.uri))
                    || (!route.name.is_empty() && l.contains(&route.name))
            });
            if !mentioned {
                continue;
            }
            claimed[i] = true;
            out.affected.push(Affected {
                route: route.clone(),
                reason: Reason::RouteFile {
                    path: change.path.clone(),
                },
            });
        }
    }

    // Keep route-table order so the list is stable between runs.
    out.affected.sort_by_key(|a| {
        routes
            .iter()
            .position(|r| r == &a.route)
            .unwrap_or(usize::MAX)
    });

    for change in changes {
        let traced = controller_class_of(&change.path)
            .map(|class| routes.iter().any(|r| r.controller_class() == Some(class)))
            .unwrap_or(false)
            || (is_route_file(&change.path)
                && out.affected.iter().any(
                    |a| matches!(&a.reason, Reason::RouteFile { path } if path == &change.path),
                ));
        if !traced {
            out.unattributed.push(change.path.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{changeset_from_diff, Changeset};

    fn route(name: &str, uri: &str, methods: &str, action: &str) -> Route {
        Route {
            name: name.into(),
            uri: uri.into(),
            methods: methods.into(),
            action: action.into(),
        }
    }

    fn routes() -> Vec<Route> {
        vec![
            route(
                "orders.index",
                "orders",
                "GET|HEAD",
                "App\\Http\\Controllers\\OrderController@index",
            ),
            route(
                "orders.update",
                "orders/{order}",
                "PATCH",
                "App\\Http\\Controllers\\OrderController@update",
            ),
            route(
                "users.index",
                "users",
                "GET|HEAD",
                "App\\Http\\Controllers\\UserController@index",
            ),
            route("health", "up", "GET|HEAD", "Closure"),
        ]
    }

    /// A changeset built from a real unified diff, so the parsing this depends
    /// on is exercised rather than hand-faked.
    fn changes(diff: &str) -> Changeset {
        changeset_from_diff(diff)
    }

    #[test]
    fn a_changed_controller_attributes_all_of_its_routes() {
        let cs = changes(
            "diff --git a/app/Http/Controllers/OrderController.php b/app/Http/Controllers/OrderController.php\n\
             --- a/app/Http/Controllers/OrderController.php\n\
             +++ b/app/Http/Controllers/OrderController.php\n\
             @@ -10,3 +10,3 @@\n\
             -        $orders = Order::all();\n\
             +        $orders = Order::with('customer')->get();\n",
        );
        let a = attribute(&cs.files, &routes());
        let names: Vec<_> = a.affected.iter().map(|x| x.route.name.as_str()).collect();
        assert_eq!(names, ["orders.index", "orders.update"]);
        assert!(a.unattributed.is_empty());
        assert_eq!(
            a.affected[0].reason,
            Reason::Controller {
                path: "app/Http/Controllers/OrderController.php".into(),
                class: "OrderController".into(),
            }
        );
    }

    #[test]
    fn only_get_routes_are_replayable() {
        let cs = changes(
            "diff --git a/app/Http/Controllers/OrderController.php b/app/Http/Controllers/OrderController.php\n\
             --- a/app/Http/Controllers/OrderController.php\n\
             +++ b/app/Http/Controllers/OrderController.php\n\
             @@ -1,1 +1,1 @@\n\
             -a\n+b\n",
        );
        let a = attribute(&cs.files, &routes());
        // Both order routes are affected, but replaying the PATCH would write.
        assert_eq!(a.affected.len(), 2);
        let replayable: Vec<_> = a.replayable().map(|x| x.route.name.as_str()).collect();
        assert_eq!(replayable, ["orders.index"]);
    }

    #[test]
    fn a_routes_file_only_claims_routes_its_diff_mentions() {
        // The whole point: touching routes/web.php must not make every route in
        // the app look affected.
        let cs = changes(
            "diff --git a/routes/web.php b/routes/web.php\n\
             --- a/routes/web.php\n\
             +++ b/routes/web.php\n\
             @@ -5,2 +5,3 @@\n\
             +Route::get('users', [UserController::class, 'index'])->name('users.index');\n",
        );
        let a = attribute(&cs.files, &routes());
        let names: Vec<_> = a.affected.iter().map(|x| x.route.name.as_str()).collect();
        assert_eq!(names, ["users.index"]);
        assert_eq!(
            a.affected[0].reason,
            Reason::RouteFile {
                path: "routes/web.php".into()
            }
        );
    }

    #[test]
    fn a_controller_match_wins_over_a_routes_file_mention() {
        let cs = changes(
            "diff --git a/app/Http/Controllers/UserController.php b/app/Http/Controllers/UserController.php\n\
             --- a/app/Http/Controllers/UserController.php\n\
             +++ b/app/Http/Controllers/UserController.php\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n\
             diff --git a/routes/web.php b/routes/web.php\n\
             --- a/routes/web.php\n\
             +++ b/routes/web.php\n\
             @@ -1,1 +1,2 @@\n\
             +Route::get('users', [UserController::class, 'index'])->name('users.index');\n",
        );
        let a = attribute(&cs.files, &routes());
        assert_eq!(a.affected.len(), 1, "the route must not be reported twice");
        assert!(matches!(a.affected[0].reason, Reason::Controller { .. },));
    }

    #[test]
    fn files_that_cannot_be_traced_are_reported_not_dropped() {
        let cs = changes(
            "diff --git a/app/Models/Order.php b/app/Models/Order.php\n\
             --- a/app/Models/Order.php\n\
             +++ b/app/Models/Order.php\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n\
             diff --git a/README.md b/README.md\n\
             --- a/README.md\n\
             +++ b/README.md\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n",
        );
        let a = attribute(&cs.files, &routes());
        assert!(a.is_empty());
        assert_eq!(a.unattributed, ["app/Models/Order.php", "README.md"]);
    }

    #[test]
    fn a_closure_route_has_no_controller_to_match() {
        assert_eq!(routes()[3].controller_class(), None);
        let cs = changes(
            "diff --git a/app/Http/Controllers/Closure.php b/app/Http/Controllers/Closure.php\n\
             --- a/app/Http/Controllers/Closure.php\n\
             +++ b/app/Http/Controllers/Closure.php\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n",
        );
        let a = attribute(&cs.files, &routes());
        assert!(
            a.is_empty(),
            "a file named Closure.php must not match the literal action"
        );
    }

    #[test]
    fn an_invokable_controller_is_matched() {
        let rs = vec![route(
            "report",
            "report",
            "GET|HEAD",
            "App\\Http\\Controllers\\ReportController",
        )];
        let cs = changes(
            "diff --git a/app/Http/Controllers/ReportController.php b/app/Http/Controllers/ReportController.php\n\
             --- a/app/Http/Controllers/ReportController.php\n\
             +++ b/app/Http/Controllers/ReportController.php\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n",
        );
        assert_eq!(attribute(&cs.files, &rs).affected.len(), 1);
    }

    #[test]
    fn a_controller_outside_the_conventional_directory_still_matches() {
        let rs = vec![route(
            "admin",
            "admin",
            "GET|HEAD",
            "Modules\\Admin\\AdminController@index",
        )];
        let cs = changes(
            "diff --git a/modules/Admin/AdminController.php b/modules/Admin/AdminController.php\n\
             --- a/modules/Admin/AdminController.php\n\
             +++ b/modules/Admin/AdminController.php\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n",
        );
        assert_eq!(attribute(&cs.files, &rs).affected.len(), 1);
    }

    #[test]
    fn no_routes_means_everything_is_unattributed() {
        let cs = changes(
            "diff --git a/app/Http/Controllers/OrderController.php b/app/Http/Controllers/OrderController.php\n\
             --- a/app/Http/Controllers/OrderController.php\n\
             +++ b/app/Http/Controllers/OrderController.php\n\
             @@ -1,1 +1,1 @@\n-a\n+b\n",
        );
        let a = attribute(&cs.files, &[]);
        assert!(a.is_empty());
        assert_eq!(a.unattributed, ["app/Http/Controllers/OrderController.php"]);
    }

    #[test]
    fn diff_headers_are_not_mistaken_for_content() {
        // `+++ b/routes/web.php` must not count as a line mentioning a route.
        let cs = changes(
            "diff --git a/routes/web.php b/routes/web.php\n\
             --- a/routes/web.php\n\
             +++ b/routes/web.php\n\
             @@ -1,1 +1,1 @@\n\
             -// nothing to see\n\
             +// still nothing\n",
        );
        let rs = vec![route("web", "web.php", "GET|HEAD", "Closure")];
        assert!(attribute(&cs.files, &rs).is_empty());
    }
}
