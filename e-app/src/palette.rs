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

pub fn palette(state: AppState) -> impl IntoView {
    use crate::search::{Action, Hit, Tab};

    let query: RwSignal<String> = RwSignal::new(String::new());
    let files: RwSignal<Vec<PathBuf>> = RwSignal::new(Vec::new());
    let selected: RwSignal<usize> = RwSignal::new(0);
    let tab: RwSignal<Tab> = RwSignal::new(Tab::default());
    // Symbols and Text are answered by a background request, so their results
    // live here rather than being derived from a list we already hold.
    let async_hits: RwSignal<Vec<Hit>> = RwSignal::new(Vec::new());
    let gen: RwSignal<u64> = RwSignal::new(0);

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
            async_hits.set(Vec::new());
            tab.set(Tab::default());
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

    // Ask the backend behind the current tab. Only the async ones go out to the
    // language server or the file walker; the rest are ranked in place.
    let request_async = move || {
        let t = tab.get_untracked();
        if !t.is_async() {
            return;
        }
        let q = query.get_untracked();
        if q.trim().len() < 2 {
            async_hits.set(Vec::new());
            return;
        }
        let g = gen.get_untracked() + 1;
        gen.set(g);
        let roots = state.roots.get_untracked();
        let root = state.root.get_untracked();
        let send =
            floem::ext_event::create_ext_action(state.cx, move |(got, hits): (u64, Vec<Hit>)| {
                if got == gen.get_untracked() {
                    async_hits.set(hits);
                    selected.set(0);
                }
            });
        match t {
            Tab::Text => {
                std::thread::spawn(move || {
                    let hits = crate::workspace_search::search(&roots, &q, Default::default(), 200)
                        .into_iter()
                        .map(|h| {
                            let rel = h
                                .path
                                .strip_prefix(&root)
                                .unwrap_or(&h.path)
                                .to_string_lossy()
                                .into_owned();
                            Hit {
                                label: h.text,
                                detail: format!("{rel}:{}", h.line + 1),
                                action: Action::Goto {
                                    uri: format!("file://{}", h.path.display()),
                                    line: h.line,
                                    col: h.col,
                                },
                            }
                        })
                        .collect();
                    send((g, hits));
                });
            }
            Tab::Symbols => {
                let Some(client) = state.lsp_for_active() else {
                    async_hits.set(Vec::new());
                    return;
                };
                std::thread::spawn(move || {
                    let hits = client
                        .workspace_symbol(&q)
                        .unwrap_or_default()
                        .into_iter()
                        .take(200)
                        .map(|(name, uri, line, ch)| {
                            let path = e_lsp::uri_to_path(&uri);
                            let rel = path
                                .strip_prefix(&root)
                                .unwrap_or(&path)
                                .to_string_lossy()
                                .into_owned();
                            Hit {
                                label: name,
                                detail: format!("{rel}:{}", line + 1),
                                action: Action::Goto { uri, line, col: ch },
                            }
                        })
                        .collect();
                    send((g, hits));
                });
            }
            _ => {}
        }
    };

    // Re-query when either the text or the tab changes.
    create_effect(move |_| {
        let _ = (query.get(), tab.get());
        if state.palette_open.get_untracked() {
            request_async();
        }
    });

    let filtered = move || -> Vec<Hit> {
        let q = query.get();
        match tab.get() {
            Tab::Files => {
                crate::search::file_hits(&q, &files.get(), &state.root.get(), MAX_RESULTS)
            }
            Tab::Actions => {
                crate::search::action_hits(&q, crate::cmd_palette::COMMANDS, MAX_RESULTS)
            }
            Tab::Symbols | Tab::Text => async_hits.get(),
        }
    };

    let run = move |hit: Hit| {
        state.palette_open.set(false);
        match hit.action {
            Action::Open(p) => state.open_path(p),
            Action::Goto { uri, line, col } => state.jump_to(&uri, line as usize, col as usize),
            Action::Command(id) => crate::cmd_palette::run_command(state, id),
        }
    };
    let open_selected = move || {
        let results = filtered();
        if results.is_empty() {
            return;
        }
        let idx = selected.get().min(results.len() - 1);
        run(results[idx].clone());
    };

    let input = text_input(query)
        // Static: `placeholder` is not reactive, and the tab bar above already
        // says which source you are in.
        .placeholder("Search…")
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
        )
        // Tab cycles the source, shift-Tab goes back — so switching never means
        // reaching for the mouse mid-query.
        .on_key_down(
            Key::Named(NamedKey::Tab),
            |_| true,
            move |e| {
                let back = matches!(e, floem::event::Event::KeyDown(k) if k.modifiers.shift());
                tab.update(|t| *t = if back { t.prev() } else { t.next() });
                selected.set(0);
            },
        );

    let results = dyn_stack(
        move || filtered().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, h): &(usize, Hit)| (*i, h.label.clone(), h.detail.clone()),
        move |(i, hit): (usize, Hit)| {
            let (name, detail) = (hit.label.clone(), hit.detail.clone());
            stack((
                label(move || name.clone())
                    .style(|s| s.text_ellipsis().color(theme::fg()).font_size(13.0)),
                label(move || detail.clone()).style(|s| {
                    s.text_ellipsis()
                        .flex_grow(1.0_f32)
                        .margin_left(10.0)
                        .color(theme::fg_dim())
                        .font_size(11.0)
                }),
            ))
            .style(move |s| {
                let s = s
                    .height(26.0)
                    .width_full()
                    .items_center()
                    .padding_horiz(10.0)
                    .cursor(floem::style::CursorStyle::Pointer);
                if selected.get() == i {
                    s.background(theme::bg_active())
                } else {
                    s.hover(|s| s.background(theme::bg_hover()))
                }
            })
            .on_click_stop(move |_| {
                selected.set(i);
                run(hit.clone());
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    // The tab bar. Clicking switches source; Tab cycles.
    let tabs = dyn_stack(
        || Tab::ALL.into_iter().collect::<Vec<_>>(),
        |t: &Tab| *t,
        move |t: Tab| {
            label(move || t.label().to_string())
                .style(move |s| {
                    let s = s
                        .padding_horiz(12.0)
                        .height(30.0)
                        .items_center()
                        .font_size(12.0)
                        .cursor(floem::style::CursorStyle::Pointer);
                    if tab.get() == t {
                        s.color(theme::accent())
                            .border_bottom(2.0)
                            .border_color(theme::accent())
                    } else {
                        s.color(theme::fg_dim()).hover(|s| s.color(theme::fg()))
                    }
                })
                .on_click_stop(move |_| {
                    tab.set(t);
                    selected.set(0);
                    focus_pulse.update(|x| *x += 1);
                })
        },
    )
    .style(|s| {
        s.flex_row()
            .width_full()
            .padding_horiz(4.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // What to say when a tab has nothing to show yet.
    let hint = label(move || {
        let t = tab.get();
        if filtered().is_empty() && query.get().trim().len() < 2 {
            t.empty_hint().to_string()
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let s = s.padding(10.0).font_size(12.0).color(theme::fg_dim());
        if tab.get().is_async() && filtered().is_empty() {
            s
        } else {
            s.hide()
        }
    });

    let results_scroll = scroll(results)
        .scroll_to_percent(move || {
            let n = filtered().len().max(1) as f32;
            selected.get() as f32 / n
        })
        .style(|s| s.max_height(320.0).width_full());

    let box_ = stack((tabs, input, hint, results_scroll))
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
