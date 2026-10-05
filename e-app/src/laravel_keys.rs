//! Find Usages and Rename for Laravel keys — route names, views, translation
//! keys — in the editor. The search itself is `e_laravel::keys`.

use std::path::PathBuf;

use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;
use lsp_types::{Position, Range, TextEdit};

use e_lsp::path_to_uri;

use crate::keys::{self, KeyUse};
use crate::laravel::Helper;
use crate::picker::{PickerItem, PickerMode};
use crate::state::AppState;

impl AppState {
    /// The Laravel key under the caret, or — in a Blade view with none — the
    /// view itself ("who renders this?").
    fn laravel_key_at_caret(&self) -> Option<(Helper, String)> {
        if let Some((helper, key, _)) = self.laravel_token() {
            if matches!(helper, Helper::Route | Helper::View | Helper::Trans) {
                return Some((helper, key));
            }
            return None;
        }
        let buf = self.active_buffer()?;
        let path = buf.file.path.as_ref()?;
        keys::view_name(&self.root.get_untracked(), path).map(|name| (Helper::View, name))
    }

    /// Find Usages on a Laravel key: every use and declaration, in the picker.
    /// Returns whether it took over from the language server.
    pub(crate) fn find_laravel_key_usages(&self) -> bool {
        if self.laravel.get_untracked().is_none() {
            return false;
        }
        let Some((helper, key)) = self.laravel_key_at_caret() else {
            return false;
        };
        let root = self.root.get_untracked();
        let p = self.picker;
        p.mode.set(PickerMode::References);
        p.query.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);
        let shown_root = root.clone();
        self.spawn_bg(
            move || keys::key_uses(&root, helper, &key),
            move |uses: Vec<KeyUse>| {
                let items = uses
                    .into_iter()
                    .map(|u| {
                        let rel = u.path.strip_prefix(&shown_root).unwrap_or(&u.path);
                        let mark = if u.definition { "  · declared" } else { "" };
                        PickerItem {
                            label: format!("{}:{}{mark}", rel.display(), u.line + 1),
                            detail: u.text.clone(),
                            uri: path_to_uri(&u.path),
                            line: u.line as u32,
                            char: u.col as u32,
                        }
                    })
                    .collect();
                p.items.set(items);
                p.selected.set(0);
            },
        );
        true
    }

    /// Start a rename of the Laravel key under the caret, if there is one.
    /// Returns whether it did (the rename box then holds the whole key).
    pub(crate) fn open_laravel_rename(&self) -> bool {
        let Some((helper, key, _)) = self.laravel_token() else {
            return false;
        };
        if !matches!(helper, Helper::Route | Helper::View | Helper::Trans) {
            return false;
        }
        let r = self.rename;
        r.laravel.set(Some(helper));
        r.word.set(key.clone());
        r.new_name.set(key);
        r.open.set(true);
        true
    }

    /// Plan renaming a Laravel key everywhere it appears, and show the plan —
    /// or say why it can't be done completely.
    pub(crate) fn plan_laravel_rename(&self, helper: Helper, old: String, new: String) {
        let root = self.root.get_untracked();
        let app = *self;
        let key = old.clone();
        self.spawn_bg(
            move || keys::key_uses(&root, helper, &key),
            move |uses: Vec<KeyUse>| {
                if let Some(why) = keys::rename_blocker(helper, &old, &new, &uses) {
                    Self::notify(&why);
                    return;
                }
                let root = app.root.get_untracked();
                let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
                if helper == Helper::View {
                    let to: PathBuf = new.split('.').collect();
                    let to = root
                        .join("resources/views")
                        .join(to)
                        .with_extension("blade.php");
                    if to.exists() {
                        Self::notify(&format!("A view named “{new}” already exists"));
                        return;
                    }
                    if let Some(from) = uses.iter().find(|u| u.definition).map(|u| u.path.clone()) {
                        moves.push((from, to));
                    }
                }
                let mut edits: Vec<(String, Vec<TextEdit>)> = Vec::new();
                for u in uses.iter().filter(|u| u.len > 0) {
                    let at = |c: usize| Position::new(u.line as u32, c as u32);
                    let edit = TextEdit {
                        range: Range::new(at(u.col), at(u.col + u.len)),
                        new_text: keys::replacement(helper, u, &new),
                    };
                    let uri = path_to_uri(&u.path);
                    match edits.iter_mut().find(|(x, _)| *x == uri) {
                        Some((_, es)) => es.push(edit),
                        None => edits.push((uri, vec![edit])),
                    }
                }
                let mut plan = crate::rename_preview::plan(&old, &new, &edits, |p| {
                    app.buffers
                        .with_untracked(|bs| {
                            bs.iter()
                                .find(|b| b.file.path.as_deref() == Some(p))
                                .map(|b| b.doc.text().to_string())
                        })
                        .or_else(|| std::fs::read_to_string(p).ok())
                });
                plan.moves = moves;
                if plan.is_empty() && plan.moves.is_empty() {
                    Self::notify(&format!("Nothing refers to “{old}”"));
                    return;
                }
                app.rename_plan.set(Some(plan));
            },
        );
    }
}
