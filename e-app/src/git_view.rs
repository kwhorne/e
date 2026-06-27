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
    pub fn refresh_git_status(&self) {
        let root = self.root.get_untracked();
        let repo = git::repo_root(&root);
        self.git_root.set(repo.clone());
        match repo {
            Some(repo) => {
                self.git_branch.set(git::current_branch(&repo));
                self.git_status.set(git::status(&repo));
            }
            None => {
                self.git_branch.set(None);
                self.git_status.set(Vec::new());
            }
        }
    }

    fn with_repo(&self, f: impl FnOnce(&std::path::Path) -> Result<(), String>) {
        if let Some(repo) = self.git_root.get_untracked() {
            if let Err(e) = f(&repo) {
                eprintln!("e: git: {e}");
            }
            self.refresh_git_status();
        }
    }

    pub fn git_stage(&self, path: String) {
        self.with_repo(|repo| git::stage(repo, &path));
    }
    pub fn git_unstage(&self, path: String) {
        self.with_repo(|repo| git::unstage(repo, &path));
    }
    pub fn git_stage_all(&self) {
        self.with_repo(git::stage_all);
    }
    pub fn git_discard(&self, path: String) {
        self.with_repo(|repo| git::discard(repo, &path));
        self.fs_rev.update(|r| *r += 1);
    }
    pub fn git_checkout(&self, branch: String) {
        self.with_repo(|repo| git::checkout(repo, &branch));
        self.fs_rev.update(|r| *r += 1);
    }

    pub fn git_push(&self) {
        self.with_repo(git::push);
    }
    pub fn git_pull(&self) {
        self.with_repo(git::pull);
        self.fs_rev.update(|r| *r += 1);
    }

    pub fn git_commit(&self) {
        let msg = self.git_commit_msg.get_untracked();
        let msg = msg.trim().to_string();
        if msg.is_empty() {
            return;
        }
        self.with_repo(|repo| git::commit(repo, &msg));
        self.git_commit_msg.set(String::new());
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
            'M' | 'R' => Color::from_rgb8(0xe5, 0xc0, 0x7b), // yellow
            'A' => Color::from_rgb8(0x98, 0xc3, 0x79),       // green
            'D' => Color::from_rgb8(0xe0, 0x6c, 0x75),       // red
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
        label(move || badge.to_string())
            .style(move |s| s.width(16.0).color(color).font_size(12.0)),
        label(move || name.clone())
            .style(|s| s.flex_grow(1.0).color(theme::fg()).text_ellipsis().min_width(0.0)),
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
                    .filter(|e| if staged { e.is_staged() } else { e.is_unstaged() })
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
            st.iter()
                .any(|e| if staged { e.is_staged() } else { e.is_unstaged() })
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
        s.flex_grow(1.0)
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
    let msg = text_input(state.git_commit_msg)
        .placeholder("Message (Enter to commit)")
        .on_enter(move || state.git_commit())
        .style(|s| {
            theme::input_colors(s)
                .width_full()
                .height(30.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        });

    let commit_btn = label(|| "Commit".to_string())
        .style(|s| {
            s.height(28.0)
                .items_center()
                .justify_center()
                .width_full()
                .border_radius(5.0)
                .font_size(12.0)
                .background(theme::accent())
                .color(Color::from_rgb8(0x14, 0x16, 0x1b))
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

    let commit_area = stack((msg, stack((stage_all, commit_btn)).style(|s| s.gap(6.0).width_full())))
        .style(|s| s.flex_col().gap(6.0).padding(8.0));

    let lists = scroll(
        stack((group(state, "STAGED CHANGES", true), group(state, "CHANGES", false)))
            .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.flex_grow(1.0).width_full());

    stack((header, commit_area, lists)).style(|s| s.flex_col().width_full().height_full())
}
