//! Workspace trust: whether e may run a project's own code.
//!
//! Opening a folder must not run anything its author wrote. Until the project
//! is trusted, e reads it (highlighting, search, navigation, git, the
//! database panel) but doesn't run it: no `php artisan`, nothing from
//! `vendor/bin` or `node_modules`, no tests, tasks or debugger, no language
//! server that loads the project's code, and no `.e/rules.toml` handed to an
//! agent as instructions. One click on the banner trusts it for good.
//!
//! Trusted folders live in `~/.config/e/trusted.json`, apart from
//! `config.json`, as a list of absolute paths; a folder inside a trusted one
//! is trusted, and so is a git worktree of a trusted repository (agents and
//! Grove make those). The first time e runs with this, the projects in the
//! recent list are trusted: their code has already run.

use std::path::{Path, PathBuf};

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{label, stack, Decorators};
use floem::IntoView;
use serde_json::{json, Value};

use crate::state::AppState;
use crate::theme;

fn store_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("E_TRUST_FILE") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/e/trusted.json"))
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// The trusted folders, or `None` when the store doesn't exist yet.
fn read_store(path: &Path) -> Option<Vec<PathBuf>> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    Some(
        v.get("trusted")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn write_store(path: &Path, list: &[PathBuf]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let list: Vec<String> = list
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let text = serde_json::to_string_pretty(&json!({ "trusted": list }))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// The trusted folders, seeding the store from `seed` the first time.
fn trusted_list(path: &Path, seed: impl FnOnce() -> Vec<PathBuf>) -> Vec<PathBuf> {
    if let Some(list) = read_store(path) {
        return list;
    }
    let list: Vec<PathBuf> = seed().iter().map(|p| canonical(p)).collect();
    let _ = write_store(path, &list);
    list
}

/// The repository a git worktree at `root` belongs to: `.git` is then a file
/// saying `gitdir: <repo>/.git/worktrees/<name>`.
fn worktree_repo(root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_relative() {
        root.join(gitdir)
    } else {
        gitdir
    };
    let worktrees = gitdir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    // The repository must have registered this folder as its worktree; a
    // `.git` file alone is the folder's own word, and anyone can write one.
    let back = std::fs::read_to_string(gitdir.join("gitdir")).ok()?;
    if canonical(Path::new(back.trim())) != root.join(".git") {
        return None;
    }
    let dot_git = worktrees.parent()?;
    Some(canonical(dot_git.parent()?))
}

fn covered(list: &[PathBuf], root: &Path) -> bool {
    list.iter().any(|t| root.starts_with(t))
}

fn is_trusted_in(path: &Path, root: &Path, seed: impl FnOnce() -> Vec<PathBuf>) -> bool {
    let list = trusted_list(path, seed);
    let root = canonical(root);
    covered(&list, &root) || worktree_repo(&root).is_some_and(|repo| covered(&list, &repo))
}

fn set_trusted_in(path: &Path, root: &Path, trusted: bool) -> std::io::Result<()> {
    let mut list = read_store(path).unwrap_or_default();
    let root = canonical(root);
    list.retain(|p| p != &root);
    if trusted {
        list.push(root);
    } else {
        // Restricting a folder takes back any trust that covered it.
        list.retain(|p| !root.starts_with(p));
    }
    write_store(path, &list)
}

/// Whether the project at `root` may run its own code.
pub fn is_trusted(root: &Path) -> bool {
    match store_path() {
        Some(path) => is_trusted_in(&path, root, crate::config::recent_projects),
        None => false,
    }
}

/// Trust (or stop trusting) the project at `root`.
pub fn set_trusted(root: &Path, trusted: bool) -> std::io::Result<()> {
    let path = store_path().ok_or_else(|| std::io::Error::other("no home folder"))?;
    // Seed first, so restricting the first folder doesn't drop the others.
    let _ = trusted_list(&path, crate::config::recent_projects);
    set_trusted_in(&path, root, trusted)
}

/// What restricted mode holds back in the project at `root`, for the banner;
/// empty when there's nothing of the project's to run.
pub fn withheld(root: &Path) -> Vec<&'static str> {
    let has = |p: &str| root.join(p).exists();
    let mut out = Vec::new();
    if has("artisan") {
        out.push("artisan");
    }
    if has("composer.json") || has("vendor/bin") {
        out.push("PHP tools");
    }
    if has("package.json") || has("Cargo.toml") || has("go.mod") {
        out.push("language servers");
    }
    if has("phpunit.xml") || has("phpunit.xml.dist") || has("tests") {
        out.push("tests");
    }
    if has(".e/rules.toml") {
        out.push("project rules for agents");
    }
    out
}

/// Language servers that only read the project: safe in restricted mode.
/// The rest load the project's own code — TypeScript from `node_modules`
/// and its plugins, `build.rs` and proc macros, a Go toolchain named in
/// `go.mod`, the Laravel app booted by laravel-lsp.
pub fn server_reads_only(command: &str) -> bool {
    matches!(command, "intelephense" | "clangd" | "pyright-langserver")
}

/// The bar above the editor while the project is restricted and has code of
/// its own that e is holding back.
pub fn trust_bar(state: AppState) -> impl IntoView {
    let held = withheld(&state.root.get_untracked());
    let has_held = !held.is_empty();
    let text = format!(
        "Restricted mode: e doesn't run this project's code until you trust it ({}). \
         Reading, search and git work as usual.",
        held.join(", ")
    );
    let button = |text: &'static str| {
        label(move || text.to_string()).style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
    };
    stack((
        label(move || text.clone()).style(|s| {
            s.color(theme::fg())
                .font_size(12.0)
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .text_ellipsis()
        }),
        button("Trust Project").on_click_stop(move |_| state.trust_project()),
        button("×").on_click_stop(move |_| state.trust_banner_dismissed.set(true)),
    ))
    .style(move |s| {
        let s = s
            .items_center()
            .gap(8.0)
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .background(theme::accent().multiply_alpha(0.14))
            .border_bottom(1.0)
            .border_color(theme::border());
        if has_held && !state.trusted.get() && !state.trust_banner_dismissed.get() {
            s
        } else {
            s.hide()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("e-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        canonical(&dir)
    }

    #[test]
    fn a_folder_is_restricted_until_trusted_and_children_follow() {
        let dir = scratch("basic");
        let store = dir.join("trusted.json");
        let project = dir.join("shop");
        std::fs::create_dir_all(project.join("packages/x")).unwrap();

        assert!(!is_trusted_in(&store, &project, Vec::new));
        set_trusted_in(&store, &project, true).unwrap();
        assert!(is_trusted_in(&store, &project, Vec::new));
        assert!(is_trusted_in(&store, &project.join("packages/x"), Vec::new));
        // A sibling whose name starts the same is not inside it.
        std::fs::create_dir_all(dir.join("shop-evil")).unwrap();
        assert!(!is_trusted_in(&store, &dir.join("shop-evil"), Vec::new));

        // Restricting a child takes back the parent's trust.
        set_trusted_in(&store, &project.join("packages/x"), false).unwrap();
        assert!(!is_trusted_in(&store, &project, Vec::new));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_store_is_seeded_once_from_the_recent_projects() {
        let dir = scratch("seed");
        let store = dir.join("trusted.json");
        let old = dir.join("old");
        let new = dir.join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();

        assert!(is_trusted_in(&store, &old, || vec![old.clone()]));
        // Seeded already: a later seed isn't consulted.
        assert!(!is_trusted_in(&store, &new, || vec![new.clone()]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worktree_of_a_trusted_repo_is_trusted() {
        let dir = scratch("worktree");
        let store = dir.join("trusted.json");
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join(".git/worktrees/fix")).unwrap();
        let wt = dir.join("wt-fix");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/fix").display()),
        )
        .unwrap();

        assert!(!is_trusted_in(&store, &wt, Vec::new));
        set_trusted_in(&store, &repo, true).unwrap();
        // Claimed by the folder, not registered by the repository.
        assert!(!is_trusted_in(&store, &wt, Vec::new));
        std::fs::write(
            repo.join(".git/worktrees/fix/gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        assert!(is_trusted_in(&store, &wt, Vec::new));
        // A `.git` file pointing anywhere else proves nothing.
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", repo.display())).unwrap();
        assert!(!is_trusted_in(&store, &wt, Vec::new));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_read_only_servers_run_restricted() {
        assert!(server_reads_only("intelephense"));
        assert!(!server_reads_only("typescript-language-server"));
        assert!(!server_reads_only("rust-analyzer"));
        assert!(!server_reads_only("laravel-lsp"));
        assert!(!server_reads_only("gopls"));
    }
}
