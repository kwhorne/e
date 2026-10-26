//! Contract Heal in the editor: `⌘.` on a form field offers to carry it
//! through the stack; the plan ([`e_laravel::heal`]) is shown in the same
//! preview a rename uses — every line as it reads now and after, the new
//! migration, and what was already done — and written only on confirming.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use e_core::language::Language;
use e_laravel::heal::{field_at, plan, FieldRef, Heal, Project};
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::text::Document;

use crate::rename_preview::{file_plan, RenamePlan};
use crate::state::AppState;

/// Whether a language's files hold forms the heal understands.
fn has_forms(language: Language) -> bool {
    matches!(
        language,
        Language::Vue
            | Language::Svelte
            | Language::JavaScript
            | Language::TypeScript
            | Language::Blade
    )
}

/// The form field at `offset`, in a file of `language`.
pub fn field_here(text: &str, language: Language, offset: usize) -> Option<FieldRef> {
    has_forms(language)
        .then(|| field_at(text, offset, language == Language::Blade))
        .flatten()
}

/// The preview of a heal.
fn preview(heal: Heal) -> RenamePlan {
    let mut files: Vec<_> = heal
        .changes
        .into_iter()
        .map(|(path, old, new)| file_plan(path, &old, &new))
        .filter(|f| !f.sites.is_empty())
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    RenamePlan {
        title: Some(format!("Carry “{}” through the stack?", heal.field)),
        verb: Some("Apply".into()),
        files,
        creates: heal.creates,
        notes: heal.notes,
        ..RenamePlan::default()
    }
}

impl AppState {
    /// Work out what carrying the field under the cursor through the stack
    /// takes, and show it.
    pub fn heal_field(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let (Some(path), Some(editor)) = (buf.file.path.clone(), buf.editor.get_untracked()) else {
            return;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let Some(field) = field_here(&text, buf.file.language, offset) else {
            Self::notify("No form field here");
            return;
        };
        let root = self.root.get_untracked();
        let routes = self
            .laravel
            .get_untracked()
            .map(|d| d.all_routes.clone())
            .unwrap_or_default();
        let schema = self.db.schema_cache.get_untracked();
        // Open files are planned from what they say now, not from disk.
        let open: HashMap<PathBuf, String> = self.buffers.with_untracked(|bs| {
            bs.iter()
                .filter_map(|b| Some((b.file.path.clone()?, b.doc.text().to_string())))
                .collect()
        });
        let out = self.rename_plan;
        let name = field.field.clone();
        self.spawn_bg(
            move || {
                let read = |p: &Path| {
                    open.get(p)
                        .cloned()
                        .or_else(|| std::fs::read_to_string(p).ok())
                };
                let timestamp = crate::codegen::migration_timestamp();
                let project = Project {
                    root: &root,
                    routes: &routes,
                    schema: &schema,
                    read: &read,
                    timestamp: &timestamp,
                };
                plan(&project, &path, &text, &field)
            },
            move |res: Result<Heal, String>| match res {
                Ok(heal) if heal.is_empty() => {
                    let mut msg = format!("“{name}” is carried through already");
                    for n in &heal.notes {
                        msg.push_str(&format!("\n• {n}"));
                    }
                    Self::notify(&msg);
                }
                Ok(heal) => out.set(Some(preview(heal))),
                Err(e) => Self::notify(&format!("Can't carry “{name}” through: {e}")),
            },
        );
    }

    /// Write the files a plan creates. Returns the ones that couldn't be.
    pub(crate) fn create_planned_files(creates: &[(PathBuf, String)]) -> Vec<String> {
        let mut failed = Vec::new();
        for (path, content) in creates {
            if path.exists() {
                failed.push(format!("{} (exists)", path.display()));
                continue;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if !Self::write_or_notify(path, content) {
                failed.push(path.display().to_string());
            }
        }
        failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_form_files_offer_it() {
        let vue = "const form = useForm({ name: '' })\nform.phone";
        let at = vue.find("phone").unwrap();
        assert_eq!(field_here(vue, Language::Vue, at).unwrap().field, "phone");
        assert!(field_here(vue, Language::Php, at).is_none());
    }

    #[test]
    fn the_preview_lists_lines_new_files_and_notes() {
        let heal = Heal {
            field: "phone".into(),
            changes: vec![(
                PathBuf::from("/app/Models/Customer.php"),
                "[\n    'name',\n];\n".into(),
                "[\n    'name',\n    'phone',\n];\n".into(),
            )],
            creates: vec![(PathBuf::from("/m.php"), "<?php".into())],
            notes: vec!["StoreCustomerRequest validates phone already".into()],
        };
        let p = preview(heal);
        assert_eq!(p.title.as_deref(), Some("Carry “phone” through the stack?"));
        assert_eq!(p.summary(), "1 site in 1 file, and a new file");
        assert_eq!(p.notes.len(), 1);
    }
}
