//! Exceptions on the line that threw them. `e` follows the project's
//! `storage/logs/laravel.log` (or today's `laravel-YYYY-MM-DD.log`), and an
//! exception the running app logs lands in the editor on the first line of
//! the project's own code it passed through: red at the end of that line, as
//! a diagnostic in the Problems list, and in the status bar until it's dealt
//! with. `⌘.` on the line hands it to the agent with its stacktrace.
//!
//! It works for whatever serves the app — Grove, Herd, `artisan serve`, Sail
//! — as it reads the log, not a proxy. Only what is logged after `e` opens the
//! project counts. An exception stays with its line as the file is edited;
//! saving the file clears its exceptions, as that is usually the fix, and the
//! next failing request brings them back.

use std::path::{Path, PathBuf};

use e_laravel::laravel_log::{self, LoggedException};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::text::Document;
use floem::views::{label, Decorators};
use floem::IntoView;
use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::state::AppState;
use crate::theme;

/// The most exceptions kept; older ones drop off.
const KEEP: usize = 20;

/// An exception, placed in the project's code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caught {
    pub path: PathBuf,
    /// 0-based.
    pub line: usize,
    pub class: String,
    pub message: String,
    /// `20:15:03`.
    pub time: String,
    /// The log entry, stacktrace and all.
    pub text: String,
}

impl Caught {
    fn short_class(&self) -> &str {
        self.class.rsplit('\\').next().unwrap_or(&self.class)
    }

    /// `ErrorException: Undefined variable $orders`.
    pub fn summary(&self) -> String {
        let mut message: String = self.message.chars().take(140).collect();
        if message.len() < self.message.len() {
            message.push('…');
        }
        format!("{}: {message}", self.short_class())
    }
}

/// The log Laravel writes to now: `laravel.log`, or the newest daily one.
fn current_log(root: &Path) -> Option<PathBuf> {
    let dir = root.join("storage/logs");
    std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("laravel") && name.ends_with(".log")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// Place `e` in the project: its file and 0-based line. A view's error comes
/// without a line; an undefined variable's first use in the template is a
/// good one.
fn place(e: &LoggedException, root: &Path) -> Option<Caught> {
    let (path, line) = e.origin(root, |p| p.is_file())?;
    let mut line = line.saturating_sub(1);
    if path.to_string_lossy().ends_with(".blade.php") {
        if let Some(var) = laravel_log::undefined_variable(&e.message) {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let needle = format!("${var}");
            if let Some(i) = text.lines().position(|l| {
                l.match_indices(&needle).any(|(at, _)| {
                    !l[at + needle.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                })
            }) {
                line = i;
            }
        }
    }
    Some(Caught {
        path,
        line,
        class: e.class.clone(),
        message: e.message.clone(),
        time: e
            .datetime
            .rsplit(' ')
            .next()
            .unwrap_or(&e.datetime)
            .to_string(),
        text: e.text.clone(),
    })
}

impl AppState {
    /// Read what the log gained since last time. Called on the idle tick; a
    /// `stat` when nothing was written.
    pub fn poll_laravel_log(&self) {
        let root = self.root.get_untracked();
        if !root.join("artisan").is_file() {
            return;
        }
        let Some(log) = current_log(&root) else {
            return;
        };
        let len = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
        let from = match self.log_read.get_untracked() {
            // The first look: what is there already happened before `e`.
            None => {
                self.log_read.set(Some((log, len)));
                return;
            }
            Some((seen, at)) if seen == log && at == len => return,
            // Truncated, or cleared: start over.
            Some((seen, at)) if seen == log && at < len => at,
            // Today's new daily file, or a cleared one.
            _ => 0,
        };
        self.log_read.set(Some((log.clone(), len)));
        let state = *self;
        self.spawn_bg(
            move || {
                use std::io::{Read, Seek, SeekFrom};
                let mut file = std::fs::File::open(&log).ok()?;
                file.seek(SeekFrom::Start(from)).ok()?;
                let mut bytes = Vec::new();
                file.take(len - from).read_to_end(&mut bytes).ok()?;
                let text = String::from_utf8_lossy(&bytes);
                Some(
                    laravel_log::exceptions(&text)
                        .iter()
                        .filter_map(|e| place(e, &root))
                        .collect::<Vec<Caught>>(),
                )
            },
            move |found: Option<Vec<Caught>>| {
                if let Some(found) = found.filter(|f| !f.is_empty()) {
                    state.catch(found);
                }
            },
        );
    }

    /// Keep `found`, newest last; one per line, the latest.
    fn catch(&self, found: Vec<Caught>) {
        self.caught.update(|kept| {
            for c in found {
                kept.retain(|k| !(k.path == c.path && k.line == c.line));
                kept.push(c);
            }
            let over = kept.len().saturating_sub(KEEP);
            kept.drain(..over);
        });
        self.sync_caught_marks();
    }

    /// The file at `path` was saved: its exceptions were probably the reason.
    pub(crate) fn clear_caught_in(&self, path: &Path) {
        if self
            .caught
            .with_untracked(|c| c.iter().any(|c| c.path == path))
        {
            self.caught.update(|c| c.retain(|c| c.path != path));
            self.sync_caught_marks();
        }
    }

    /// Forget the exception on `line` of `path`.
    pub fn dismiss_caught(&self, path: &Path, line: usize) {
        self.caught
            .update(|c| c.retain(|c| !(c.path == path && c.line == line)));
        self.sync_caught_marks();
    }

    /// Move the exceptions in `path` along with an edit from `old` to `new`:
    /// lines added above push one down, and removing its line removes it.
    pub(crate) fn follow_caught(&self, path: &Path, old: &Rope, new: &str) {
        let lines: Vec<usize> = self.caught.with_untracked(|c| {
            c.iter()
                .filter(|c| c.path == path)
                .map(|c| c.line)
                .collect()
        });
        if lines.is_empty() {
            return;
        }
        let old = old.to_string();
        // Each line on its own, so a removed one can be told from one that
        // moved onto another's place.
        let moved: Vec<(usize, Option<usize>)> = lines
            .iter()
            .map(|&l| {
                (
                    l,
                    e_core::diff::follow_lines(&old, new, &[l]).first().copied(),
                )
            })
            .collect();
        if moved.iter().all(|(from, to)| Some(*from) == *to) {
            return;
        }
        self.caught.update(|c| {
            c.retain_mut(|c| {
                if c.path != path {
                    return true;
                }
                match moved.iter().find(|(from, _)| *from == c.line) {
                    Some((_, Some(to))) => {
                        c.line = *to;
                        true
                    }
                    Some((_, None)) => false,
                    None => true,
                }
            });
        });
        self.sync_caught_marks();
    }

    /// The exception on `line` (0-based) of `path`, if one is kept.
    pub fn caught_at(&self, path: &Path, line: usize) -> Option<Caught> {
        self.caught.with_untracked(|c| {
            c.iter()
                .rev()
                .find(|c| c.path == path && c.line == line)
                .cloned()
        })
    }

    /// Hand the exception on `line` of `path` to the agent, stacktrace and all.
    pub fn caught_to_agent(&self, path: &Path, line: usize) {
        let Some(c) = self.caught_at(path, line) else {
            return;
        };
        let root = self.root.get_untracked();
        let rel = c.path.strip_prefix(&root).unwrap_or(&c.path);
        let entry: String = c.text.chars().take(6000).collect();
        self.send_to_agent(&format!(
            "The running app threw {} at {}:{}: {}\n\nFind the cause and fix it. \
             The entry from storage/logs/laravel.log:\n\n{entry}",
            c.class,
            rel.display(),
            c.line + 1,
            c.message,
        ));
    }

    /// Bring every open file's exception marks up to date.
    pub(crate) fn sync_caught_marks(&self) {
        let caught = self.caught.get_untracked();
        let buffers = self.buffers.get_untracked();
        for b in buffers {
            let Some(path) = &b.file.path else {
                continue;
            };
            let here: Vec<&Caught> = caught.iter().filter(|c| &c.path == path).collect();
            let alerts: Vec<(usize, String)> = here
                .iter()
                .map(|c| (c.line, format!("✖ {} · {}", c.summary(), c.time)))
                .collect();
            if b.alerts.get_untracked() == alerts {
                continue;
            }
            b.alerts.set(alerts);
            let text = b.doc.text().to_string();
            *b.caught.borrow_mut() = here
                .iter()
                .map(|c| {
                    let content = text.lines().nth(c.line).unwrap_or("");
                    let start = content.len() - content.trim_start().len();
                    Diagnostic {
                        range: Range::new(
                            Position::new(c.line as u32, start as u32),
                            Position::new(c.line as u32, content.len() as u32),
                        ),
                        severity: Some(DiagnosticSeverity::ERROR),
                        source: Some("laravel.log".into()),
                        message: c.summary(),
                        ..Default::default()
                    }
                })
                .collect();
            if let Some(uri) = b.uri.clone() {
                let lsp = self
                    .diagnostics
                    .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
                self.apply_diagnostics_to_buffer(&uri, &lsp);
            }
            b.doc.cache_rev().update(|r| *r += 1);
        }
    }
}

/// The newest exception, in the status bar; click to go to its line.
pub fn caught_status(state: AppState) -> impl IntoView {
    let root = state.root;
    label(move || {
        state.caught.with(|c| match c.last() {
            Some(c) => {
                let name = c
                    .path
                    .strip_prefix(root.get())
                    .unwrap_or(&c.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("✖ {}:{} {}", name, c.line + 1, c.short_class())
            }
            None => String::new(),
        })
    })
    .style(move |s| {
        let s = s
            .color(theme::error())
            .text_ellipsis()
            .max_width(320.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .hover(|s| s.color(theme::fg()));
        if state.caught.with(Vec::is_empty) {
            s.hide()
        } else {
            s
        }
    })
    .on_click_stop(move |_| {
        if let Some(c) = state.caught.with_untracked(|c| c.last().cloned()) {
            state.goto_location(c.path, c.line, 0);
        }
    })
}
