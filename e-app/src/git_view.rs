//! The Source Control (git) panel, shown in the left sidebar (⌘2).

use std::path::PathBuf;

use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_stack, label, scroll, stack, text_input, Decorators};
use floem::IntoView;

use e_core::git::{self, StatusEntry};

use crate::state::AppState;
use crate::theme;

impl AppState {
    /// Toggle the Source Control panel (and ensure the sidebar is visible).
    pub fn toggle_git_panel(&self) {
        let open = self.git_panel_open.get_untracked();
        if open {
            self.git_panel_open.set(false);
        } else {
            self.git_panel_open.set(true);
            self.sidebar_open.set(true);
            self.refresh_git_status();
        }
    }

    /// Re-read the repository status and branch.
    ///
    /// Off the UI thread: it is four `git` processes, and it runs on every save
    /// while the panel is open. A refresh that finishes after a newer one has
    /// started is dropped, so the panel never steps back to older state.
    pub fn refresh_git_status(&self) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static GENERATION: AtomicU64 = AtomicU64::new(0);
        let gen = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        let root = self.root.get_untracked();
        let state = *self;
        self.spawn_bg(
            move || {
                let repo = git::repo_root(&root)?;
                Some((
                    git::current_branch(&repo),
                    git::status(&repo),
                    git::log(&repo, 20),
                    git::stash_count(&repo),
                    repo,
                ))
            },
            move |snapshot| {
                if GENERATION.load(Ordering::Relaxed) != gen {
                    return;
                }
                match snapshot {
                    Some((branch, status, log, stashes, repo)) => {
                        // A commit or checkout moved HEAD: the change marks
                        // compare against something else now.
                        let head =
                            |l: &[(String, String, String, String)]| l.first().map(|c| c.0.clone());
                        if state.git_log.with_untracked(|old| head(old)) != head(&log) {
                            state.refresh_head_texts();
                        }
                        state.git_root.set(Some(repo));
                        state.git_branch.set(branch);
                        state.git_status.set(status);
                        state.git_log.set(log);
                        state.git_stash_count.set(stashes);
                    }
                    None => {
                        state.git_root.set(None);
                        state.git_branch.set(None);
                        state.git_status.set(Vec::new());
                        state.git_log.set(Vec::new());
                        state.git_stash_count.set(0);
                    }
                }
            },
        );
    }

    pub fn git_stash(&self) {
        self.with_repo("stash", git::stash_push, |s| s.fs_rev.update(|r| *r += 1));
    }
    pub fn git_stash_pop(&self) {
        self.with_repo("stash pop", git::stash_pop, |s| {
            s.fs_rev.update(|r| *r += 1)
        });
    }

    /// Run a git operation off the UI thread — push, pull and a commit's hooks
    /// can take as long as the network or the hooks do — then refresh the
    /// panel. A failure is shown, with git's own message; `on_ok` runs only
    /// when it succeeded.
    fn with_repo(
        &self,
        what: &'static str,
        f: impl FnOnce(&std::path::Path) -> Result<(), String> + Send + 'static,
        on_ok: impl FnOnce(&AppState) + 'static,
    ) {
        let Some(repo) = self.git_root.get_untracked() else {
            return;
        };
        let state = *self;
        self.spawn_bg(
            move || f(&repo),
            move |result: Result<(), String>| {
                match result {
                    Ok(()) => on_ok(&state),
                    Err(e) => {
                        eprintln!("e: git {what}: {e}");
                        let first = e.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                        Self::notify(&format!("git {what} failed: {first}"));
                    }
                }
                state.refresh_git_status();
            },
        );
    }

    pub fn git_stage(&self, path: String) {
        self.with_repo("add", move |repo| git::stage(repo, &path), |_| {});
    }
    pub fn git_unstage(&self, path: String) {
        self.with_repo(
            "restore --staged",
            move |repo| git::unstage(repo, &path),
            |_| {},
        );
    }
    pub fn git_stage_all(&self) {
        self.with_repo("add", git::stage_all, |_| {});
    }

    /// Ask before discarding: it throws away uncommitted work for good, and the
    /// button sits right next to "stage".
    pub fn git_discard(&self, path: String) {
        let state = *self;
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        self.confirm.set(Some(crate::dialogs::Confirm {
            title: format!("Discard changes to “{name}”?"),
            detail: "Its uncommitted changes are thrown away. This can't be undone.".into(),
            action: "Discard",
            on_confirm: std::rc::Rc::new(move || {
                let path = path.clone();
                state.with_repo(
                    "checkout",
                    move |repo| git::discard(repo, &path),
                    |s| s.fs_rev.update(|r| *r += 1),
                );
            }),
        }));
    }
    pub fn git_checkout(&self, branch: String) {
        self.with_repo(
            "checkout",
            move |repo| git::checkout(repo, &branch),
            |s| s.fs_rev.update(|r| *r += 1),
        );
    }

    pub fn git_push(&self) {
        self.with_repo("push", git::push, |_| Self::notify("Pushed"));
    }
    pub fn git_pull(&self) {
        self.with_repo("pull", git::pull, |s| s.fs_rev.update(|r| *r += 1));
    }

    /// Commit, and clear the message box only once the commit exists — a
    /// rejected commit (a failing hook, nothing staged) keeps the message.
    pub fn git_commit(&self) {
        let msg = self.git_commit_msg.get_untracked();
        let msg = msg.trim().to_string();
        if msg.is_empty() {
            return;
        }
        let sent = msg.clone();
        self.with_repo(
            "commit",
            move |repo| git::commit(repo, &msg),
            move |s| {
                // Only if it wasn't edited while the commit ran.
                if s.git_commit_msg.get_untracked().trim() == sent {
                    s.git_commit_msg.set(String::new());
                }
            },
        );
    }

    /// Suggest a Conventional Commits message from the current changes and put
    /// it in the message box (off the UI thread).
    pub fn suggest_commit_message(&self) {
        let Some(repo) = self.git_root.get_untracked() else {
            return;
        };
        let target = self.git_commit_msg;
        let send = floem::ext_event::create_ext_action(self.cx, move |s: String| {
            if !s.is_empty() {
                target.set(s);
            }
        });
        std::thread::spawn(move || {
            send(git::suggest_commit(&repo));
        });
    }

    /// Open a status file in the editor.
    pub fn open_git_file(&self, rel: String) {
        if let Some(repo) = self.git_root.get_untracked() {
            self.open_path(repo.join(rel));
        }
    }
}

fn badge_color(entry: &StatusEntry) -> Color {
    if entry.is_untracked() {
        theme::fg_dim()
    } else {
        match entry.badge() {
            'M' | 'R' => theme::warning(), // yellow
            'A' => theme::success(),       // green
            'D' => theme::error(),         // red
            _ => theme::fg_dim(),
        }
    }
}

fn icon_btn(glyph: &'static str) -> impl IntoView {
    label(move || glyph.to_string()).style(|s| {
        s.width(22.0)
            .height(22.0)
            .items_center()
            .justify_center()
            .font_size(13.0)
            .border_radius(4.0)
            .color(theme::fg_dim())
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
    })
}

/// One file row: badge + name + actions (stage/unstage/discard).
fn file_row(state: AppState, entry: StatusEntry, staged: bool) -> impl IntoView {
    let path = entry.path.clone();
    let name = PathBuf::from(&entry.path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| entry.path.clone());
    let badge = entry.badge();
    let color = badge_color(&entry);
    let open_path = path.clone();

    let actions = if staged {
        let p = path.clone();
        stack((icon_btn("−").on_click_stop(move |_| state.git_unstage(p.clone())),))
            .style(|s| s.items_center().gap(2.0))
    } else {
        let (p1, p2) = (path.clone(), path.clone());
        stack((
            icon_btn("↺").on_click_stop(move |_| state.git_discard(p1.clone())),
            icon_btn("+").on_click_stop(move |_| state.git_stage(p2.clone())),
        ))
        .style(|s| s.items_center().gap(2.0))
    };

    stack((
        label(move || badge.to_string()).style(move |s| s.width(16.0).color(color).font_size(12.0)),
        label(move || name.clone()).style(|s| {
            s.flex_grow(1.0_f32)
                .color(theme::fg())
                .text_ellipsis()
                .min_width(0.0)
        }),
        actions,
    ))
    .style(|s| {
        s.items_center()
            .gap(6.0)
            .width_full()
            .padding_horiz(8.0)
            .height(24.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.background(theme::bg_hover()))
    })
    .on_click_stop(move |_| state.open_git_file(open_path.clone()))
}

fn group(state: AppState, title: &'static str, staged: bool) -> impl IntoView {
    let header = label(move || title.to_string()).style(|s| {
        s.color(theme::fg_dim())
            .font_size(11.0)
            .padding_horiz(8.0)
            .padding_vert(4.0)
    });

    let rows = dyn_stack(
        move || {
            state.git_status.with(|st| {
                st.iter()
                    .filter(|e| {
                        if staged {
                            e.is_staged()
                        } else {
                            e.is_unstaged()
                        }
                    })
                    .cloned()
                    .enumerate()
                    .collect::<Vec<_>>()
            })
        },
        |(i, e)| (*i, e.path.clone(), e.badge()),
        move |(_, e)| file_row(state, e, staged),
    )
    .style(|s| s.flex_col().width_full());

    let any = move || {
        state.git_status.with(|st| {
            st.iter().any(|e| {
                if staged {
                    e.is_staged()
                } else {
                    e.is_unstaged()
                }
            })
        })
    };

    stack((header, rows)).style(move |s| {
        let s = s.flex_col().width_full();
        if any() {
            s
        } else {
            s.hide()
        }
    })
}

pub fn git_panel(state: AppState) -> impl IntoView {
    // Header: branch (click to switch) + refresh / pull / push.
    let branch = label(move || {
        state
            .git_branch
            .get()
            .map(|b| format!("⎇ {b}  ▾"))
            .unwrap_or_else(|| "Not a git repository".to_string())
    })
    .style(|s| {
        s.flex_grow(1.0_f32)
            .color(theme::fg())
            .font_size(12.0)
            .text_ellipsis()
            .min_width(0.0)
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .popout_menu(move || {
        let current = state.git_branch.get_untracked().unwrap_or_default();
        let mut menu = floem::menu::Menu::new("Switch branch");
        let repo = state.git_root.get_untracked();
        let branches = repo.map(|r| git::branches(&r)).unwrap_or_default();
        for b in branches {
            let mark = if b == current { "● " } else { "   " };
            let target = b.clone();
            menu = menu.entry(
                floem::menu::MenuItem::new(format!("{mark}{b}"))
                    .action(move || state.git_checkout(target.clone())),
            );
        }
        menu
    });

    let header = stack((
        branch,
        icon_btn("⟳").on_click_stop(move |_| state.refresh_git_status()),
        icon_btn("↓").on_click_stop(move |_| state.git_pull()),
        icon_btn("↑").on_click_stop(move |_| state.git_push()),
    ))
    .style(|s| {
        s.items_center()
            .gap(2.0)
            .width_full()
            .height(30.0)
            .padding_horiz(8.0)
            .border_bottom(1.0)
            .border_color(theme::border())
    });

    // Commit message + buttons.
    let msg_input = text_input(state.git_commit_msg)
        .placeholder("Message (Enter to commit)")
        .on_enter(move || state.git_commit())
        .style(|s| {
            theme::input_colors(s)
                .flex_grow(1.0_f32)
                .height(30.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        });
    let suggest = label(|| "✨".to_string())
        .style(|s| {
            s.height(30.0)
                .padding_horiz(8.0)
                .items_center()
                .justify_center()
                .border(1.0)
                .border_color(theme::border())
                .border_radius(4.0)
                .color(theme::fg_dim())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()).color(theme::fg()))
        })
        .on_click_stop(move |_| state.suggest_commit_message());
    let msg =
        stack((msg_input, suggest)).style(|s| s.flex_row().gap(4.0).width_full().items_center());

    let commit_btn = label(|| "Commit".to_string())
        .style(|s| {
            s.height(28.0)
                .items_center()
                .justify_center()
                .width_full()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(theme::on_accent())
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| state.git_commit());

    let stage_all = label(|| "Stage All".to_string())
        .style(|s| {
            s.height(28.0)
                .items_center()
                .justify_center()
                .width_full()
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
        .on_click_stop(move |_| state.git_stage_all());

    let small = |text: String| {
        label(move || text.clone()).style(|s| {
            s.height(26.0)
                .padding_horiz(10.0)
                .items_center()
                .justify_center()
                .flex_grow(1.0_f32)
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
    };
    let stash_btn = small("Stash".to_string()).on_click_stop(move |_| state.git_stash());
    let pop_btn = label(move || format!("Pop ({})", state.git_stash_count.get()))
        .style(move |s| {
            let s = s
                .height(26.0)
                .padding_horiz(10.0)
                .items_center()
                .justify_center()
                .flex_grow(1.0_f32)
                .border_radius(5.0)
                .font_size(12.0)
                .border(1.0)
                .border_color(theme::border())
                .color(theme::fg())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()));
            if state.git_stash_count.get() == 0 {
                s.color(theme::fg_dim())
            } else {
                s
            }
        })
        .on_click_stop(move |_| state.git_stash_pop());

    let commit_area = stack((
        msg,
        stack((stage_all, commit_btn)).style(|s| s.gap(6.0).width_full()),
        stack((stash_btn, pop_btn)).style(|s| s.gap(6.0).width_full()),
    ))
    .style(|s| s.flex_col().gap(6.0).padding(8.0));

    // Recent commits.
    let commits_rows = dyn_stack(
        move || {
            state
                .git_log
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, c)| (*i, c.0.clone()),
        move |(_, (hash, author, when, summary))| {
            stack((
                stack((
                    label(move || hash.clone()).style(|s| {
                        s.color(theme::accent())
                            .font_size(11.0)
                            .font_family(theme::mono_family())
                    }),
                    label(move || summary.clone()).style(|s| {
                        s.color(theme::fg())
                            .font_size(12.0)
                            .flex_grow(1.0_f32)
                            .text_ellipsis()
                            .min_width(0.0)
                    }),
                ))
                .style(|s| s.items_center().gap(8.0).width_full()),
                label(move || format!("{author} · {when}"))
                    .style(|s| s.color(theme::fg_dim()).font_size(10.0)),
            ))
            .style(|s| {
                s.flex_col()
                    .gap(1.0)
                    .width_full()
                    .padding_horiz(8.0)
                    .padding_vert(4.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let commits = stack((
        label(|| "RECENT COMMITS".to_string()).style(|s| {
            s.color(theme::fg_dim())
                .font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .margin_top(6.0)
                .border_top(1.0)
                .border_color(theme::border())
        }),
        commits_rows,
    ))
    .style(|s| s.flex_col().width_full());

    let lists = scroll(
        stack((
            group(state, "STAGED CHANGES", true),
            group(state, "CHANGES", false),
            commits,
        ))
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.flex_grow(1.0_f32).width_full());

    stack((header, commit_area, lists)).style(|s| s.flex_col().width_full().height_full())
}
