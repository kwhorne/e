//! Command palette / fuzzy file finder (⌘P or Ctrl+P).

use std::path::{Path, PathBuf};

use floem::keyboard::{Key, NamedKey};
use floem::reactive::{create_effect, RwSignal, SignalGet, SignalUpdate};
use floem::views::{container, dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

const MAX_FILES: usize = 40_000;
const MAX_RESULTS: usize = 200;

/// Directories that are almost never the target of a quick-open and would
/// otherwise blow up the index (especially when a high-level folder is opened).
fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "target"
                | "node_modules"
                | "vendor"
                | "dist"
                | "build"
                | "Pods"
                | "DerivedData"
                | "System"
                | "Library"
                | "Applications"
        )
}

/// Collect files under `root`, breadth-first so shallow (more relevant) files
/// are indexed first, skipping noise. Capped at `MAX_FILES`.
fn collect_files(root: &Path) -> Vec<PathBuf> {
    use std::collections::VecDeque;
    let mut out = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());
    let is_root = root == Path::new("/");
    while let Some(dir) = queue.pop_front() {
        if out.len() >= MAX_FILES {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    // Skip heavy/VCS directories, and system dirs near the root.
                    // Other dot-directories (e.g. `.github`) are still searched.
                    if matches!(name.as_ref(), ".git" | "target" | "node_modules" | "vendor")
                        || (is_root && skip_dir(&name))
                    {
                        continue;
                    }
                    queue.push_back(path);
                }
                // Files, including dotfiles like `.env` and `.gitignore`.
                Ok(_) => out.push(path),
                Err(_) => {}
            }
        }
    }
    out
}

/// Subsequence fuzzy score; higher is better. `None` if `q` is not a
/// subsequence of `text`. Rewards consecutive matches and word boundaries.
pub(crate) fn fuzzy_score(q: &str, text: &str) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    let tb = text.as_bytes();
    let qb = q.as_bytes();
    let (mut ti, mut qi) = (0usize, 0usize);
    let mut score = 0i64;
    let mut last: i64 = -2;
    while ti < tb.len() && qi < qb.len() {
        if tb[ti] == qb[qi] {
            let mut s = 1i64;
            if ti as i64 == last + 1 {
                s += 6; // consecutive
            }
            if ti == 0 || matches!(tb[ti - 1], b'/' | b'_' | b'-' | b'.' | b' ') {
                s += 10; // start of a path/word segment
            }
            score += s;
            last = ti as i64;
            qi += 1;
        }
        ti += 1;
    }
    if qi == qb.len() {
        Some(score)
    } else {
        None
    }
}

/// Rank a file (relative path) against a lowercase query. Filename matches are
/// weighted far above path matches, and shorter paths win ties.
fn rank(q: &str, rel: &str) -> Option<i64> {
    let rel_l = rel.to_lowercase();
    let name = rel_l.rsplit('/').next().unwrap_or(&rel_l);

    let mut score = if let Some(ns) = fuzzy_score(q, name) {
        let mut s = ns * 8 + 200;
        if name == q {
            s += 2000;
        } else if name.starts_with(q) {
            s += 600;
        } else if name.contains(q) {
            s += 250;
        }
        s + fuzzy_score(q, &rel_l).unwrap_or(0)
    } else {
        fuzzy_score(q, &rel_l)?
    };
    // Prefer shorter, shallower paths.
    score -= (rel_l.len() as i64) / 6;
    score -= rel_l.matches('/').count() as i64 * 2;
    Some(score)
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::{rank, MAX_RESULTS};

    fn best<'a>(q: &str, paths: &[&'a str]) -> &'a str {
        let mut scored: Vec<(i64, &str)> = paths
            .iter()
            .filter_map(|p| rank(q, p).map(|s| (s, *p)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
        scored.first().map(|(_, p)| *p).unwrap_or("")
    }

    #[test]
    fn filename_beats_deep_path() {
        let paths = [
            "Applications/Devin.app/Contents/Resources/app/out/vs/workbench/contrib/welcomeGettingStarted.js",
            "resources/views/welcome.blade.php",
        ];
        assert_eq!(best("welcome", &paths), "resources/views/welcome.blade.php");
        assert_eq!(best("welc", &paths), "resources/views/welcome.blade.php");
    }

    #[test]
    fn fuzzy_subsequence_matches() {
        assert!(rank("wbp", "resources/views/welcome.blade.php").is_some());
        assert!(rank("xyz", "resources/views/welcome.blade.php").is_none());
        assert!(MAX_RESULTS > 0);
    }
}

pub fn palette(state: AppState) -> impl IntoView {
    let query: RwSignal<String> = RwSignal::new(String::new());
    let files: RwSignal<Vec<PathBuf>> = RwSignal::new(Vec::new());
    let selected: RwSignal<usize> = RwSignal::new(0);

    // Pulsed on open so the input grabs focus without the request_focus
    // tracking `open` (which would re-grab on close and steal/loop focus).
    let focus_pulse: RwSignal<u64> = RwSignal::new(0);

    // (Re)load the file list whenever the palette opens — off the UI thread so
    // it stays instant even when a huge folder (or `/`) is open.
    create_effect(move |_| {
        if state.palette_open.get() {
            query.set(String::new());
            selected.set(0);
            files.set(Vec::new());
            focus_pulse.update(|x| *x += 1);
            let roots = state.roots.get_untracked();
            let send = floem::ext_event::create_ext_action(state.cx, move |all: Vec<PathBuf>| {
                files.set(all);
            });
            std::thread::spawn(move || {
                let all: Vec<PathBuf> = roots.iter().flat_map(|r| collect_files(r)).collect();
                send(all);
            });
        }
    });

    // Filtered results, shared by the list view and the keyboard handlers.
    let filtered = move || -> Vec<PathBuf> {
        let q = query.get().to_lowercase();
        let root = state.root.get();
        let all = files.get();
        if q.is_empty() {
            return all.into_iter().take(MAX_RESULTS).collect();
        }
        let mut scored: Vec<(i64, PathBuf)> = all
            .into_iter()
            .filter_map(|p| {
                let r = rel(&p, &root);
                rank(&q, &r).map(|s| (s, p))
            })
            .collect();
        // Highest score first; break ties by shorter path.
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| rel(&a.1, &root).len().cmp(&rel(&b.1, &root).len()))
        });
        scored
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(_, p)| p)
            .collect()
    };

    let open_selected = move || {
        let results = filtered();
        if results.is_empty() {
            return;
        }
        let idx = selected.get().min(results.len() - 1);
        state.open_path(results[idx].clone());
        state.palette_open.set(false);
    };

    let input = text_input(query)
        .placeholder("Go to file…")
        .on_enter(open_selected)
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(36.0)
                .padding_horiz(10.0)
                .border(0.0)
                .border_bottom(1.0)
        })
        .request_focus(move || {
            focus_pulse.get();
        })
        .on_event_stop(floem::event::EventListener::FocusLost, move |_| {
            floem::action::exec_after(std::time::Duration::from_millis(150), move |_| {
                if state.palette_open.get_untracked() {
                    state.palette_open.set(false);
                }
            });
        })
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| state.palette_open.set(false),
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowDown),
            |_| true,
            move |_| {
                let len = filtered().len();
                if len > 0 {
                    selected.update(|i| *i = (*i + 1).min(len - 1));
                }
            },
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |_| true,
            move |_| {
                selected.update(|i| *i = i.saturating_sub(1));
            },
        );

    let results = dyn_stack(
        move || filtered().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, p)| (*i, p.clone()),
        move |(i, path)| {
            let root = state.root.get();
            let text = rel(&path, &root);
            label(move || text.clone())
                .style(move |s| {
                    let s = s
                        .height(26.0)
                        .width_full()
                        .items_center()
                        .padding_horiz(10.0)
                        .text_ellipsis()
                        .cursor(floem::style::CursorStyle::Pointer)
                        .color(theme::fg());
                    if selected.get() == i {
                        s.background(theme::bg_active()).color(theme::accent())
                    } else {
                        s.hover(|s| s.background(theme::bg_hover()))
                    }
                })
                .on_click_stop(move |_| {
                    selected.set(i);
                    state.open_path(path.clone());
                    state.palette_open.set(false);
                })
        },
    )
    .style(|s| s.flex_col().width_full());

    let results_scroll = scroll(results)
        .scroll_to_percent(move || {
            let n = filtered().len().max(1) as f32;
            selected.get() as f32 / n
        })
        .style(|s| s.max_height(320.0).width_full());

    let box_ = stack((input, results_scroll))
        .style(|s| {
            s.flex_col()
                .width(560.0)
                .background(theme::bg_panel())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(8.0)
        })
        .on_click_stop(|_| {});

    // Backdrop fills the window; clicking it closes the palette.
    container(box_)
        .style(move |s| {
            let s = s
                .absolute()
                .inset(0.0)
                .width_full()
                .height_full()
                .justify_center()
                .items_start()
                .padding_top(90.0);
            if state.palette_open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.palette_open.set(false))
}
