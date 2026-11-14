//! The workspace: file operations in the tree, the open dialogs, trust, tasks and updates.

use super::*;

impl AppState {
    // ---- File explorer operations --------------------------------------

    /// Open the name prompt for a file operation rooted at `path`.
    pub fn start_file_op(&self, kind: FileOpKind, path: PathBuf) {
        let op = self.file_op;
        op.kind.set(kind);
        match kind {
            FileOpKind::NewFile | FileOpKind::NewFolder => {
                let base = if path.is_dir() {
                    path
                } else {
                    path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
                };
                op.base.set(base);
                op.input.set(String::new());
            }
            FileOpKind::Rename => {
                op.input.set(
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                op.base.set(path);
            }
            FileOpKind::Duplicate => {
                op.input.set(duplicate_name(&path));
                op.base.set(path);
            }
        }
        op.open.set(true);
    }

    /// Apply the pending file operation.
    pub fn confirm_file_op(&self) {
        let op = self.file_op;
        let kind = op.kind.get_untracked();
        let base = op.base.get_untracked();
        let name = op.input.get_untracked().trim().to_string();
        op.open.set(false);
        if name.is_empty() {
            return;
        }

        let mut open_after: Option<PathBuf> = None;
        let res: std::io::Result<()> = match kind {
            FileOpKind::NewFile => {
                let p = base.join(&name);
                let r = if p.exists() {
                    Ok(())
                } else {
                    std::fs::write(&p, "")
                };
                if r.is_ok() {
                    open_after = Some(p);
                }
                r
            }
            FileOpKind::NewFolder => std::fs::create_dir_all(base.join(&name)),
            FileOpKind::Rename => {
                let dst = base
                    .parent()
                    .map(|p| p.join(&name))
                    .unwrap_or_else(|| PathBuf::from(&name));
                std::fs::rename(&base, &dst)
            }
            FileOpKind::Duplicate => {
                let dst = base
                    .parent()
                    .map(|p| p.join(&name))
                    .unwrap_or_else(|| PathBuf::from(&name));
                copy_recursive(&base, &dst)
            }
        };
        if let Err(e) = res {
            eprintln!("e: file operation failed: {e}");
        }
        self.fs_rev.update(|r| *r += 1);
        if let Some(p) = open_after {
            self.open_path(p);
        }
    }

    /// Move a path to the Trash (recoverable) and close any open buffer for it.
    /// Move a file or folder to the trash — off the UI thread, since Finder
    /// can take its time — and close its tab once it is gone.
    pub fn delete_path(&self, path: PathBuf) {
        let state = *self;
        let target = path.clone();
        self.spawn_bg(
            move || crate::platform::trash(&target),
            move |result: Result<(), String>| {
                if let Err(e) = result {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
                    Self::notify(&format!("Could not move {name} to the trash: {e}"));
                    return;
                }
                if let Some(id) = state.editor.buffers.with_untracked(|bs| {
                    bs.iter()
                        .find(|b| b.file.path.as_deref() == Some(path.as_path()))
                        .map(|b| b.id)
                }) {
                    state.close(id);
                }
                state.fs_rev.update(|r| *r += 1);
            },
        );
    }

    pub fn copy_path_to_clipboard(&self, path: &std::path::Path) {
        let _ = floem::Clipboard::set_contents(path.display().to_string());
    }

    pub fn reveal_in_finder(&self, path: &std::path::Path) {
        crate::platform::reveal(path);
    }

    pub fn toggle_md_preview(&self) {
        let cur = self.editor.md_preview.get_untracked();
        self.editor.md_preview.set(!cur);
    }

    pub fn toggle_diff(&self) {
        let cur = self.editor.diff_open.get_untracked();
        self.editor.diff_open.set(!cur);
    }

    /// Compare the active file against another file picked from disk (DB-free
    /// diff), shown in the file-comparison panel.
    pub fn compare_with_file(&self) {
        let Some(buf) = self.active_buffer() else {
            Self::notify("Compare: open a file first");
            return;
        };
        let left_name = buf
            .file
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "current".into());
        let left_text = buf.doc.text().to_string();
        let sig = self.editor.file_diff;
        let opts = floem::file::FileDialogOptions::new().title("Compare active file with…");
        floem::action::open_file(opts, move |info| {
            let Some(path) = info.and_then(|i| i.path.into_iter().next()) else {
                return;
            };
            let Ok(right_text) = std::fs::read_to_string(&path) else {
                Self::notify("Compare: could not read the file");
                return;
            };
            let right_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // left = other file (old), right = active file (new).
            let lines = e_core::git::diff(&right_text, &left_text);
            sig.set(Some((right_name, left_name.clone(), lines)));
        });
    }

    pub fn close_file_diff(&self) {
        self.editor.file_diff.set(None);
    }

    // ---- Workspace trust ------------------------------------------------

    /// Take the project's trust as it stands: what may run, and the banner.
    pub fn apply_trust(&self, trusted: bool) {
        crate::phptools::allow_project_code(trusted);
        self.trusted.set(trusted);
    }

    /// Whether the project's code may run for `what` (a feature the user asked
    /// for); if not, say how to allow it.
    pub(crate) fn may_run_project_code(&self, what: &str) -> bool {
        if self.trusted.get_untracked() {
            return true;
        }
        crate::toast::post(&format!(
            "This project isn't trusted yet, so e doesn't run its code ({what}). \
             Trust it from the banner or with “Trust Project” (⌘⇧P)."
        ));
        false
    }

    /// Trust the project: remember it, then start what was held back.
    pub fn trust_project(&self) {
        let root = self.root.get_untracked();
        if let Err(e) = crate::trust::set_trusted(&root, true) {
            crate::toast::post(&format!("Couldn't save the project as trusted: {e}"));
        }
        self.apply_trust(true);
        self.trust_banner_dismissed.set(false);
        // Language servers that waited, for the files already open.
        let laravel = self.is_laravel_project();
        let mut ids: Vec<&'static str> = Vec::new();
        for b in self.editor.buffers.get_untracked() {
            for spec in lsp_registry::server_specs(b.file.language, laravel) {
                if !ids.contains(&spec.id) {
                    ids.push(spec.id);
                }
            }
        }
        for id in ids {
            if self.lsp.clients.with_untracked(|m| !m.contains_key(id)) {
                self.restart_lsp(id, 0);
            }
        }
        self.load_laravel();
        crate::toast::post("Project trusted");
    }

    /// Stop trusting the project. What already runs keeps running until e
    /// restarts; nothing new starts.
    pub fn restrict_project(&self) {
        let root = self.root.get_untracked();
        if let Err(e) = crate::trust::set_trusted(&root, false) {
            crate::toast::post(&format!("Couldn't save the project as restricted: {e}"));
        }
        self.apply_trust(false);
        self.trust_banner_dismissed.set(false);
        crate::toast::post("Project restricted. Restart e to stop what's already running.");
    }

    // ---- Task runner ---------------------------------------------------

    /// Run a shell command in a new, named terminal tab.
    pub fn run_task(&self, name: &str, command: &str) {
        if !self.may_run_project_code("tasks") {
            return;
        }
        let Some(id) = self.spawn_terminal() else {
            return;
        };
        let pane = self.term.focus_pane.get_untracked();
        self.pane_active(pane).set(Some(id));
        self.term.open.set(true);
        self.rename_terminal(id, name.to_string());
        // Give the shell a moment to start before sending the command.
        let app = *self;
        let cmd = format!("{command}\n");
        floem::action::exec_after(std::time::Duration::from_millis(300), move |_| {
            app.term_input_to(id, cmd.as_bytes());
        });
    }

    /// Run the project's test command, if one can be detected.
    pub fn run_test(&self) {
        if let Some(cmd) = crate::tasks::test_command(&self.root.get_untracked()) {
            self.run_task("test", &cmd);
        } else {
            eprintln!("e: no test command detected for this project");
        }
    }

    // ---- Open file / project (native dialogs) --------------------------

    /// Native dialog to open an arbitrary file in the current window.
    pub fn open_file_dialog(&self) {
        let state = *self;
        let opts = floem::file::FileDialogOptions::new()
            .title("Open File")
            .force_starting_directory(self.root.get_untracked());
        floem::action::open_file(opts, move |info| {
            if let Some(path) = info.and_then(|i| i.path.into_iter().next()) {
                state.open_path(path);
            }
        });
    }

    /// Native dialog to open a folder as another project (in a new window).
    pub fn open_project_dialog(&self) {
        let state = *self;
        let opts = floem::file::FileDialogOptions::new()
            .select_directories()
            .title("Open Folder")
            .force_starting_directory(self.root.get_untracked());
        floem::action::open_file(opts, move |info| {
            if let Some(path) = info.and_then(|i| i.path.into_iter().next()) {
                state.open_project(path);
            }
        });
    }

    /// Install the `e` command-line launcher into `/usr/local/bin` so the
    /// editor can be opened from any directory with `e .`.
    pub fn install_cli(&self) {
        let Ok(exe) = crate::platform::current_exe() else {
            Self::notify("Could not locate the e executable.");
            return;
        };
        let target = "/usr/local/bin/e";

        // Try a direct symlink first (works if /usr/local/bin is writable).
        let _ = std::fs::create_dir_all("/usr/local/bin");
        let _ = std::fs::remove_file(target);
        if std::os::unix::fs::symlink(&exe, target).is_ok() {
            Self::notify("Installed: run `e .` from any directory.");
            return;
        }

        // Otherwise ask for administrator privileges via osascript.
        let script = format!(
            "do shell script \"mkdir -p /usr/local/bin && ln -sf '{}' '{}'\" with administrator privileges",
            exe.display(),
            target
        );
        match std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status()
        {
            Ok(s) if s.success() => Self::notify("Installed: run `e .` from any directory."),
            _ => Self::notify("Could not install the `e` command (permission denied)."),
        }
    }

    /// Say `message` in the window, and on the desktop too while the window
    /// isn't in front (see [`crate::toast`]).
    pub(crate) fn notify(message: &str) {
        crate::agent_drive::remember_notice(message);
        crate::toast::post(message);
        if !crate::toast::window_focused() {
            crate::platform::notify(message);
        }
    }

    /// Add another root folder to the workspace (multi-root).
    pub fn add_workspace_folder(&self) {
        let state = *self;
        let opts = floem::file::FileDialogOptions::new()
            .select_directories()
            .title("Add Folder to Workspace")
            .force_starting_directory(self.root.get_untracked());
        floem::action::open_file(opts, move |info| {
            if let Some(path) = info.and_then(|i| i.path.into_iter().next()) {
                state.roots.update(|r| {
                    if !r.contains(&path) {
                        r.push(path);
                    }
                });
                state.fs_rev.update(|x| *x += 1);
            }
        });
    }

    /// Remove a root folder from the workspace (keeps at least the primary).
    pub fn remove_workspace_folder(&self, path: PathBuf) {
        self.roots.update(|r| r.retain(|p| p != &path));
        if self.roots.with_untracked(|r| r.is_empty()) {
            self.roots.set(vec![self.root.get_untracked()]);
        }
        self.fs_rev.update(|x| *x += 1);
    }

    /// Launch a new editor instance on `path` (a project folder or a file).
    pub fn open_project(&self, path: PathBuf) {
        let exe = crate::platform::current_exe().ok();
        if let Some(exe) = exe.as_ref() {
            let bundle = exe
                .ancestors()
                .find(|p| p.extension().map(|e| e == "app").unwrap_or(false));
            if let Some(bundle) = bundle {
                let _ = std::process::Command::new("open")
                    .arg("-n")
                    .arg(bundle)
                    .arg("--args")
                    .arg(&path)
                    .spawn();
                return;
            }
        }
        if let Some(exe) = exe {
            let _ = std::process::Command::new(exe).arg(&path).spawn();
        }
    }

    // ---- Auto-update ----------------------------------------------------

    /// Check GitHub for a newer release (non-blocking). `announce_up_to_date`
    /// controls whether an "already current" result is surfaced in the status.
    pub fn check_for_updates(&self, announce_up_to_date: bool) {
        use crate::updater::{self, UpdateStatus};
        if self.updater.status.get_untracked() == UpdateStatus::Downloading {
            return;
        }
        // The silent startup check is throttled so frequent restarts don't burn
        // the unauthenticated GitHub rate limit (60/hr); a manual check
        // (`announce_up_to_date`) always runs.
        let now = now_ms_epoch() / 1000;
        if !announce_up_to_date && now.saturating_sub(config::last_update_check()) < 6 * 3600 {
            return;
        }
        config::set_last_update_check(now);

        self.updater.status.set(UpdateStatus::Checking);
        let info_sig = self.updater.info;
        let status_sig = self.updater.status;
        let send = create_ext_action(
            self.cx,
            move |result: Result<Option<updater::UpdateInfo>, String>| match result {
                Ok(Some(info)) => {
                    info_sig.set(Some(info));
                    status_sig.set(UpdateStatus::Idle);
                }
                Ok(None) => {
                    status_sig.set(if announce_up_to_date {
                        UpdateStatus::UpToDate
                    } else {
                        UpdateStatus::Idle
                    });
                }
                // Never report a *failed* check as "up to date". Surface it on a
                // manual check; stay silent on the background startup check.
                Err(e) => {
                    status_sig.set(if announce_up_to_date {
                        UpdateStatus::CheckFailed(e)
                    } else {
                        UpdateStatus::Idle
                    });
                }
            },
        );
        std::thread::spawn(move || {
            let result = updater::check().map_err(|e| format!("{e:#}"));
            send(result);
        });
    }

    /// Download and install the available update in place (non-blocking).
    pub fn install_update(&self) {
        use crate::updater::{self, UpdateStatus};
        if self.updater.status.get_untracked() == UpdateStatus::Downloading {
            return;
        }
        self.updater.status.set(UpdateStatus::Downloading);
        let status_sig = self.updater.status;
        let send = create_ext_action(self.cx, move |result: Result<(), String>| match result {
            Ok(()) => status_sig.set(UpdateStatus::Installed),
            Err(e) => status_sig.set(UpdateStatus::Failed(e)),
        });
        let Some(version) = self.updater.info.get_untracked().map(|i| i.version) else {
            self.updater
                .status
                .set(UpdateStatus::Failed("no update to install".into()));
            return;
        };
        std::thread::spawn(move || {
            let result = updater::install(&version).map_err(|e| format!("{e:#}"));
            send(result);
        });
    }

    /// Dismiss the update notice (until the next check).
    pub fn dismiss_update(&self) {
        self.updater.info.set(None);
        self.updater.notes_open.set(false);
        self.updater.status.set(crate::updater::UpdateStatus::Idle);
    }

    /// Relaunch the application (used after an update is installed).
    pub fn restart_app(&self) {
        // Exiting skips the idle tick; take unsaved work along.
        self.save_session();
        let exe = crate::platform::current_exe().ok();
        // If we're running inside a macOS .app bundle, relaunch the bundle so
        // the window comes to the front; otherwise relaunch the bare binary.
        if let Some(exe) = exe.as_ref() {
            let bundle = exe
                .ancestors()
                .find(|p| p.extension().map(|e| e == "app").unwrap_or(false));
            if let Some(bundle) = bundle {
                let _ = std::process::Command::new("open")
                    .arg("-n")
                    .arg(bundle)
                    .spawn();
                std::process::exit(0);
            }
        }
        if let Some(exe) = exe {
            let _ = std::process::Command::new(exe)
                .arg(self.root.get_untracked())
                .spawn();
        }
        std::process::exit(0);
    }
}
