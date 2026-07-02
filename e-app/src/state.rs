//! Shared, reactive application state.
//!
//! `AppState` is `Copy` (every field is a Floem signal or `Scope`), so it can
//! be handed to as many view closures as needed without cloning ceremony.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use std::sync::mpsc::{channel, Receiver, Sender};

use floem::ext_event::create_ext_action;
use floem::kurbo::Point;
use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::{SelRegion, Selection};
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::Editor;
use lsp_types::{Diagnostic, PublishDiagnosticsParams};

use e_core::buffer::{self, FileInfo};
use e_core::git;
use e_core::language::Language;
use e_core::syntax::highlight_lines;
use e_lsp::{path_to_uri, uri_to_path, LspClient, SignatureInfo};
use e_term::Terminal;

use crate::builtin_completion;
use crate::cmd_palette::CmdPalette;
use crate::completion::{Completion, HoverState, SignatureState};
use crate::config::{self, AgentConfig, Settings};
use crate::file_ops::{copy_recursive, duplicate_name, FileOp, FileOpKind};
use crate::find::FindState;
use crate::framework_completion;
use crate::laravel::{self, LaravelData};
use crate::outline::OutlineItem;
use crate::picker::{Picker, PickerItem, PickerMode};
use crate::rename::RenameState;
use crate::session::{self, SessionData};
use crate::snippets;
use crate::styling::{
    build_diag_lines, BracketMarks, DiagLines, FindMarks, FindSpan, GitMarks, Highlights,
};

/// One open file/tab.
/// A saved database connection plus its live UI state.
#[derive(Clone)]
pub struct DbEntry {
    pub config: e_db::DbConfig,
    /// The live connection (None when disconnected).
    pub conn: RwSignal<Option<Arc<e_db::Conn>>>,
    pub expanded: RwSignal<bool>,
    pub connecting: RwSignal<bool>,
    pub tables: RwSignal<Vec<String>>,
    pub error: RwSignal<Option<String>>,
    pub filter: RwSignal<String>,
}

impl DbEntry {
    pub fn new(cx: Scope, config: e_db::DbConfig) -> Self {
        DbEntry {
            config,
            conn: cx.create_rw_signal(None),
            expanded: cx.create_rw_signal(false),
            connecting: cx.create_rw_signal(false),
            tables: cx.create_rw_signal(Vec::new()),
            error: cx.create_rw_signal(None),
            filter: cx.create_rw_signal(String::new()),
        }
    }
    pub fn key(&self) -> String {
        self.config.key()
    }
}

/// The manual add-connection form.
#[derive(Clone, Debug)]
pub struct DbForm {
    pub engine: String,
    pub host: String,
    pub port: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub path: String,
    pub group: String,
    pub use_ssh: bool,
    pub ssh_host: String,
    pub ssh_port: String,
    pub ssh_user: String,
    pub ssh_auth: String,
    pub ssh_password: String,
    pub ssh_key_path: String,
    pub ssh_passphrase: String,
}

impl Default for DbForm {
    fn default() -> Self {
        DbForm {
            engine: "mysql".into(),
            host: "127.0.0.1".into(),
            port: "3306".into(),
            database: String::new(),
            username: "root".into(),
            password: String::new(),
            path: String::new(),
            group: String::new(),
            use_ssh: false,
            ssh_host: String::new(),
            ssh_port: "22".into(),
            ssh_user: String::new(),
            ssh_auth: "key".into(),
            ssh_password: String::new(),
            ssh_key_path: String::new(),
            ssh_passphrase: String::new(),
        }
    }
}

impl DbForm {
    pub fn to_config(&self) -> e_db::DbConfig {
        e_db::DbConfig {
            engine: self.engine.clone(),
            host: self.host.clone(),
            port: self.port.parse().unwrap_or(0),
            database: self.database.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            path: self.path.clone(),
            group: self.group.clone(),
            label: String::new(),
            use_ssh: self.use_ssh,
            ssh_host: self.ssh_host.clone(),
            ssh_port: self.ssh_port.parse().unwrap_or(22),
            ssh_user: self.ssh_user.clone(),
            ssh_auth: self.ssh_auth.clone(),
            ssh_password: self.ssh_password.clone(),
            ssh_key_path: self.ssh_key_path.clone(),
            ssh_passphrase: self.ssh_passphrase.clone(),
        }
    }
}

/// A pending, agent-proposed SQL query awaiting the user's consent.
#[derive(Clone)]
pub struct DbConsent {
    pub sql: String,
    pub db_name: String,
    pub conn: Arc<e_db::Conn>,
    pub reply: std::sync::mpsc::Sender<serde_json::Value>,
}

/// Status of the TDD test-runner loop.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TddStatus {
    Idle,
    Running,
    Passed,
    Failed,
}

/// One segment of a proposed edit: unchanged context, or a reviewable change.
#[derive(Clone)]
pub enum EditSeg {
    Equal(String),
    Change {
        old: String,
        new: String,
        accepted: RwSignal<bool>,
    },
}

/// An agent-proposed edit to a file, reviewed hunk-by-hunk before applying.
#[derive(Clone)]
pub struct AgentEdit {
    pub path: PathBuf,
    pub segs: Vec<EditSeg>,
    pub reply: std::sync::mpsc::Sender<serde_json::Value>,
}

#[derive(Clone)]
pub struct Buffer {
    pub id: u64,
    pub file: FileInfo,
    pub doc: Rc<TextDocument>,
    pub dirty: RwSignal<bool>,
    pub highlights: Highlights,
    /// Per-line diagnostic spans (for inline squiggles).
    pub diag_lines: DiagLines,
    /// Per-line git change markers.
    pub git_marks: GitMarks,
    /// Per-line find-match spans.
    pub find_marks: FindMarks,
    /// Matching-bracket highlight spans.
    pub bracket_marks: BracketMarks,
    /// `file://` URI, when backed by a path (used for LSP).
    pub uri: Option<String>,
    /// The live editor, set once its view is built.
    pub editor: RwSignal<Option<Editor>>,
    /// The editor's top-left position in the window (for popups).
    pub win_origin: RwSignal<Point>,
    /// A `(line, col)` to move the caret to once the editor exists.
    pub pending_goto: RwSignal<Option<(usize, usize)>>,
    /// Last-seen modification time of the file on disk (for change detection).
    pub disk_mtime: RwSignal<Option<std::time::SystemTime>>,
    /// Set when the file changed on disk while the buffer had unsaved edits.
    pub disk_changed: RwSignal<bool>,
    /// Per-line git blame: `(author, unix_time, summary)`.
    pub blame: Rc<RefCell<Vec<(String, i64, String)>>>,
    /// LSP inlay hints: `(line, character, label)`, shown as phantom text.
    pub inlay_hints: RwSignal<Vec<(u32, u32, String)>>,
    /// Very large file — expensive per-edit features are skipped for speed.
    pub large: bool,
    /// Text encoding label (e.g. `UTF-8`, `windows-1252`).
    pub encoding: RwSignal<String>,
    /// Branching undo history (see [`e_core::undotree`]).
    pub undo: Rc<RefCell<e_core::undotree::UndoTree>>,
    /// When set, a text change is caused by undo-tree navigation, so it must
    /// not be recorded back into the tree.
    pub undo_nav: Rc<std::cell::Cell<bool>>,
}

/// One terminal session (a running shell).
#[derive(Clone)]
pub struct TermSession {
    pub id: u64,
    pub term: Rc<RefCell<Terminal>>,
    /// Custom name (empty = default "zsh N").
    pub name: RwSignal<String>,
}

/// A language server we know how to launch.
struct ServerSpec {
    id: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    language_id: &'static str,
}

/// The language server for a given language, if `e` knows one.
fn server_spec(language: Language) -> Option<ServerSpec> {
    let spec = |id, program, args, language_id| {
        Some(ServerSpec {
            id,
            program,
            args,
            language_id,
        })
    };
    match language {
        Language::Php => spec("intelephense", "intelephense", &["--stdio"], "php"),
        Language::Rust => spec("rust-analyzer", "rust-analyzer", &[], "rust"),
        Language::C => spec("clangd", "clangd", &[], "c"),
        Language::Cpp => spec("clangd", "clangd", &[], "cpp"),
        Language::TypeScript => spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "typescript",
        ),
        Language::JavaScript => spec(
            "tsserver",
            "typescript-language-server",
            &["--stdio"],
            "javascript",
        ),
        Language::Go => spec("gopls", "gopls", &[], "go"),
        Language::Python => spec("pyright", "pyright-langserver", &["--stdio"], "python"),
        _ => None,
    }
}

/// LSP `languageId` for a language, or `None` if unsupported.
fn lsp_language_id(language: Language) -> Option<&'static str> {
    server_spec(language).map(|s| s.language_id)
}

/// Global editor state.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Scope used to create per-document signals.
    pub cx: Scope,
    /// Workspace root shown in the file tree.
    pub root: RwSignal<PathBuf>,
    /// All workspace root folders (multi-root). The first is the primary root.
    pub roots: RwSignal<Vec<PathBuf>>,
    /// All open buffers, in tab order.
    pub buffers: RwSignal<Vec<Buffer>>,
    /// Pane 0's active buffer id.
    pub active: RwSignal<Option<u64>>,
    /// Pane 1's active buffer id (split view).
    pub active2: RwSignal<Option<u64>>,
    /// Is the editor split into two panes?
    pub split: RwSignal<bool>,
    /// Which pane has focus (0 or 1).
    pub focused: RwSignal<u8>,
    /// Monotonic id source.
    next_id: RwSignal<u64>,
    /// Is the command palette open?
    pub palette_open: RwSignal<bool>,
    /// The PHP language server, started lazily on first PHP file.
    /// Running language servers, keyed by server id.
    pub lsp_clients: RwSignal<HashMap<String, Arc<LspClient>>>,
    /// Server ids that failed to start (don't retry).
    lsp_failed: RwSignal<HashSet<String>>,
    /// Diagnostics keyed by `file://` URI.
    pub diagnostics: RwSignal<HashMap<String, Vec<Diagnostic>>>,
    /// Channel the LSP reader thread pushes diagnostics into.
    diag_tx: RwSignal<Sender<PublishDiagnosticsParams>>,
    /// Receiver, taken once by the UI to build a reactive signal.
    pub diag_rx: RwSignal<Option<Receiver<PublishDiagnosticsParams>>>,
    /// Completion popup state.
    pub completion: Completion,
    /// Hover popup state.
    pub hover: HoverState,
    /// Signature-help popup state.
    pub signature: SignatureState,
    /// Laravel project data (routes/views/config/env), if applicable.
    pub laravel: RwSignal<Option<Rc<LaravelData>>>,
    /// References / symbol-search picker.
    pub picker: Picker,
    /// Integrated terminal session (lazily spawned).
    /// All open terminal sessions, in tab order.
    pub terminals: RwSignal<Vec<TermSession>>,
    /// Pane 0's active terminal session id.
    pub active_terminal: RwSignal<Option<u64>>,
    /// Pane 1's active terminal (split view).
    pub active_terminal2: RwSignal<Option<u64>>,
    /// Is the terminal split into two panes?
    pub term_split: RwSignal<bool>,
    /// Which terminal pane has focus (0 or 1).
    pub term_focus_pane: RwSignal<u8>,
    next_term_id: RwSignal<u64>,
    /// Terminal-rename prompt: the session id being renamed, and its input.
    pub term_rename_id: RwSignal<Option<u64>>,
    pub term_rename_input: RwSignal<String>,
    pub terminal_open: RwSignal<bool>,
    /// Whether the terminal panel currently has keyboard focus.
    pub terminal_focused: RwSignal<bool>,
    /// Bumped whenever the terminal produces output, to trigger a repaint.
    pub term_tick: RwSignal<u64>,
    term_tx: RwSignal<Sender<()>>,
    pub term_rx: RwSignal<Option<Receiver<()>>>,
    /// Document outline of the active buffer.
    pub outline: RwSignal<Vec<OutlineItem>>,
    /// Find-in-file state.
    pub find: FindState,
    /// Local rename state.
    pub rename: RenameState,
    /// Timestamp (ms since epoch) of the last edit, for idle auto-save.
    pub last_edit: RwSignal<u128>,
    /// Markdown reading-mode preview toggle.
    pub md_preview: RwSignal<bool>,
    /// Command palette (⌘⇧P).
    pub cmd: CmdPalette,
    /// Git diff reading-mode toggle.
    pub diff_open: RwSignal<bool>,
    /// User settings loaded from config.json.
    pub settings: RwSignal<Settings>,
    /// Whether the left sidebar (file explorer) is visible.
    pub sidebar_open: RwSignal<bool>,
    /// File-operation name prompt (new/rename/duplicate).
    pub file_op: FileOp,
    /// Bumped after any filesystem change to refresh the file tree.
    pub fs_rev: RwSignal<u64>,
    /// Whether the About dialog is open.
    pub about_open: RwSignal<bool>,

    // ---- Agent panel (right side) --------------------------------------
    /// Whether the agent panel is visible (toggled with ⌘L).
    pub agent_open: RwSignal<bool>,
    /// The configured agents (from config.json or built-in defaults).
    pub agents: RwSignal<Vec<AgentConfig>>,
    /// The currently selected agent id.
    pub agent_current: RwSignal<String>,
    /// The running agent PTY, if started.
    pub agent_term: RwSignal<Option<Rc<RefCell<Terminal>>>>,
    /// Whether the agent panel currently has keyboard focus.
    pub agent_focused: RwSignal<bool>,
    /// Pulsed on open so the panel grabs focus without re-grabbing on close.
    pub agent_focus_pulse: RwSignal<u64>,

    /// Draggable panel widths (pixels).
    pub sidebar_width: RwSignal<f64>,
    pub agent_width: RwSignal<f64>,
    pub db_width: RwSignal<f64>,
    /// Height of the bottom terminal panel (drag-resizable).
    pub term_height: RwSignal<f64>,

    // ---- Database panel -------------------------------------------------
    /// Whether the Database panel is visible (toggled with ⌘3).
    pub db_open: RwSignal<bool>,
    /// Saved connections for the current project.
    pub db_conns: RwSignal<Vec<DbEntry>>,
    /// Whether the add-connection form is showing.
    pub db_adding: RwSignal<bool>,
    /// The manual-connection form contents.
    pub db_form: RwSignal<DbForm>,
    /// Results overlay (table browse / query).
    pub db_result_open: RwSignal<bool>,
    pub db_result: RwSignal<Option<e_db::QueryResult>>,
    pub db_result_title: RwSignal<String>,
    pub db_result_error: RwSignal<Option<String>>,
    pub db_result_loading: RwSignal<bool>,
    /// The connection the results view runs queries against.
    pub db_result_key: RwSignal<Option<String>>,
    /// The SQL editor text in query mode.
    pub db_query_text: RwSignal<String>,
    /// The table being browsed (None in free-query mode).
    pub db_result_table: RwSignal<Option<String>>,
    /// Results subview: `data` or `structure`.
    pub db_subview: RwSignal<String>,
    /// Structure (column) metadata for the browsed table.
    pub db_columns: RwSignal<Vec<e_db::ColumnInfo>>,
    /// Active sort: `(column, ascending)`.
    pub db_sort: RwSignal<Option<(String, bool)>>,
    /// Current page (0-based) when browsing a table.
    pub db_page: RwSignal<usize>,
    /// Test-connection state for the add form: ``/`testing`/`ok`/error.
    pub db_test_state: RwSignal<String>,
    /// The connection key being edited (None when adding a new one).
    pub db_editing_key: RwSignal<Option<String>>,
    /// Pending scroll delta for the results grid `(dx, dy, tick)`; the tick
    /// makes every key press a distinct value so the scroll effect re-fires.
    pub db_scroll: RwSignal<(f64, f64, u64)>,
    /// An agent-proposed query awaiting the user's consent.
    pub db_consent: RwSignal<Option<DbConsent>>,

    // ---- Tinker scratchpad ---------------------------------------------
    pub tinker_open: RwSignal<bool>,
    pub tinker_output: RwSignal<String>,
    pub tinker_running: RwSignal<bool>,

    // ---- Laravel architecture map --------------------------------------
    pub map_open: RwSignal<bool>,
    pub map_query: RwSignal<String>,

    // ---- Agent socket: audit log, live marker, edit proposals ----------
    /// Timeline of everything the agent did over the socket `(time, method, summary)`.
    pub agent_log: RwSignal<Vec<(String, String, String)>>,
    pub agent_log_open: RwSignal<bool>,
    /// Where the agent is currently "looking" `(path, line0)` — a ghost marker.
    pub agent_mark: RwSignal<Option<(PathBuf, usize)>>,
    /// A pending edit the agent proposed, awaiting per-hunk review.
    pub agent_edit: RwSignal<Option<AgentEdit>>,

    // ---- Semantic search -----------------------------------------------
    pub sem_open: RwSignal<bool>,
    pub sem_query: RwSignal<String>,
    pub sem_status: RwSignal<String>,
    pub sem_results: RwSignal<Vec<crate::semantic::SemHit>>,
    pub sem_index: RwSignal<Rc<RefCell<crate::semantic::SemIndex>>>,

    // ---- Undo tree -----------------------------------------------------
    pub undo_open: RwSignal<bool>,
    /// Bumped whenever the active buffer's undo tree changes (drives the panel).
    pub undo_rev: RwSignal<u64>,

    // ---- Schema diff (migrations vs live DB) ---------------------------
    pub schema_diff_open: RwSignal<bool>,
    pub schema_diff: RwSignal<Vec<crate::schema_diff::DiffRow>>,

    // ---- Eloquent relationship graph -----------------------------------
    pub rel_open: RwSignal<bool>,
    pub rel_graph: RwSignal<Vec<crate::relations::ModelNode>>,

    // ---- Inertia props contract ----------------------------------------
    pub contract_open: RwSignal<bool>,
    pub contract: RwSignal<Option<crate::contract::Contract>>,

    // ---- Related files (model ↔ migration ↔ factory ↔ …) ---------------
    pub related_open: RwSignal<bool>,
    pub related_items: RwSignal<Vec<(String, PathBuf)>>,

    // ---- Runtime insight (continuous Clockwork capture) ----------------
    pub runtime_open: RwSignal<bool>,
    pub runtime_reqs: RwSignal<Vec<RuntimeReq>>,
    pub runtime_expanded: RwSignal<Option<String>>,
    pub runtime_polling: RwSignal<bool>,

    // ---- Laravel log tail ----------------------------------------------
    pub log_open: RwSignal<bool>,
    pub log_lines: RwSignal<Vec<String>>,
    /// Cached live DB schema `table -> columns`, for Eloquent completion.
    pub db_schema_cache: RwSignal<std::collections::HashMap<String, Vec<e_db::ColumnInfo>>>,

    // ---- Request replay (from the architecture map) --------------------
    pub req_open: RwSignal<bool>,
    pub req_url: RwSignal<String>,
    pub req_status: RwSignal<Option<u16>>,
    pub req_time: RwSignal<String>,
    pub req_body: RwSignal<String>,
    /// Captured SQL queries `(sql, duration)` (via Clockwork if available).
    pub req_queries: RwSignal<Vec<(String, String)>>,
    pub req_error: RwSignal<Option<String>>,
    pub req_running: RwSignal<bool>,
    /// For an Inertia response: `(component, props)` shown as a tree.
    pub req_inertia: RwSignal<Option<(String, serde_json::Value)>>,

    // ---- Autonomous TDD loop -------------------------------------------
    pub tdd_open: RwSignal<bool>,
    pub tdd_status: RwSignal<TddStatus>,
    pub tdd_output: RwSignal<String>,
    pub tdd_iteration: RwSignal<usize>,
    /// When true, a failing run asks the agent to fix and re-runs on apply.
    pub tdd_loop: RwSignal<bool>,
    /// The cell currently being edited `(row, col, column_name)`.
    pub db_edit: RwSignal<Option<(usize, usize, String)>>,
    pub db_edit_value: RwSignal<String>,
    pub db_edit_null: RwSignal<bool>,
    /// Saved queries for the current project.
    pub db_queries: RwSignal<Vec<e_db::SavedQuery>>,
    /// Whether the "name this query" input is showing.
    pub db_saving_query: RwSignal<bool>,
    /// The name being typed for the query about to be saved.
    pub db_query_name: RwSignal<String>,

    // ---- Auto-update ----------------------------------------------------
    /// The available update, if GitHub reports a newer release.
    pub update_info: RwSignal<Option<crate::updater::UpdateInfo>>,
    /// Progress of the current check/install.
    pub update_status: RwSignal<crate::updater::UpdateStatus>,
    /// Whether the changelog is expanded in the update notice.
    pub update_notes_open: RwSignal<bool>,

    /// Go-to-line prompt state.
    pub goto: crate::editing::GotoState,
    /// Task-runner palette state + detected tasks.
    pub task: crate::task_palette::TaskState,
    pub task_list: RwSignal<Vec<crate::tasks::Task>>,
    /// Buffer id awaiting a close confirmation (unsaved changes).
    pub close_confirm: RwSignal<Option<u64>>,
    /// Most-recently-used files (newest first) and the ⌘E switcher state.
    pub recent_files: RwSignal<Vec<PathBuf>>,
    pub recent: crate::recent::RecentState,

    // Whether the graphical settings page is open.
    pub settings_open: RwSignal<bool>,
    // Pinned tab ids.
    pub pinned_tabs: RwSignal<HashSet<u64>>,

    // ---- Source control (git) ------------------------------------------
    /// Whether the left sidebar shows the Source Control panel (⌘2).
    pub git_panel_open: RwSignal<bool>,
    /// The repository root, if the workspace is inside a git repo.
    pub git_root: RwSignal<Option<PathBuf>>,
    /// Current branch name.
    pub git_branch: RwSignal<Option<String>>,
    /// Working-tree status entries.
    pub git_status: RwSignal<Vec<git::StatusEntry>>,
    /// The commit-message input.
    pub git_commit_msg: RwSignal<String>,
    /// Recent commits: `(hash, author, rel time, summary)`.
    pub git_log: RwSignal<Vec<(String, String, String, String)>>,
    /// Number of stash entries.
    pub git_stash_count: RwSignal<usize>,

    /// Editor font size (reactive, for zoom).
    pub font_size: RwSignal<usize>,
    /// Whether soft word-wrap is enabled.
    pub word_wrap: RwSignal<bool>,
    /// Navigation history (locations jumped from / to).
    pub nav_back_stack: RwSignal<Vec<(PathBuf, usize, usize)>>,
    pub nav_fwd_stack: RwSignal<Vec<(PathBuf, usize, usize)>>,
    /// Bumped when blame data finishes loading, to refresh the status bar.
    pub blame_rev: RwSignal<u64>,
}

/// Rows per page when browsing a table in the Database panel.
const DB_PAGE: usize = 200;

/// Quote a CSV field if it contains a comma, quote or newline.
fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Extract the request path (`/foo/bar`) from a full replay URL.
fn url_path(url: &str) -> String {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    match after_scheme.find('/') {
        Some(i) => {
            let p = &after_scheme[i..];
            p.split(['?', '#']).next().unwrap_or(p).to_string()
        }
        None => "/".to_string(),
    }
}

/// PascalCase test name from a path (`/users/1/edit` → `UsersEdit`).
fn pest_test_name(path: &str) -> String {
    let mut name = String::new();
    for seg in path.split('/') {
        let seg = seg.trim();
        // Skip empty and route parameters / numeric ids.
        if seg.is_empty() || seg.starts_with('{') || seg.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let clean: String = seg.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if let Some(first) = clean.chars().next() {
            name.push(first.to_ascii_uppercase());
            name.extend(clean.chars().skip(1));
        }
    }
    if name.is_empty() {
        "Home".to_string()
    } else {
        name
    }
}

/// Build Pest assertions from the response: status plus JSON structure or an
/// HTML `<title>` match where we can infer one.
fn pest_assertions(status: u16, body: &str) -> String {
    let mut out = format!("    $response->assertStatus({status});\n");
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(body)
        {
            let keys: Vec<String> = map.keys().take(8).map(|k| format!("'{k}'")).collect();
            if !keys.is_empty() {
                out.push_str(&format!(
                    "    $response->assertJsonStructure([{}]);\n",
                    keys.join(", ")
                ));
            }
        }
    } else if let Some(title) = html_title(body) {
        let esc = title.replace('\'', "\\'");
        out.push_str(&format!("    $response->assertSee('{esc}');\n"));
    }
    out
}

fn html_title(body: &str) -> Option<String> {
    let lower = body.to_lowercase();
    let start = lower.find("<title>")? + 7;
    let end = lower[start..].find("</title>")? + start;
    let t = body[start..end].trim();
    if t.is_empty() || t.len() > 80 {
        None
    } else {
        Some(t.to_string())
    }
}

/// Per-file location for the persisted undo tree (`~/.config/e/undo/<hash>.json`).
fn undo_store_path(file: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    let name = format!("{:016x}.json", h.finish());
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("e")
        .join("undo")
        .join(name)
}

/// Locate the active Laravel log file (single or the newest daily file).
fn find_laravel_log(root: &std::path::Path) -> Option<PathBuf> {
    let dir = root.join("storage").join("logs");
    let single = dir.join("laravel.log");
    if single.is_file() {
        return Some(single);
    }
    // Newest *.log by modified time (daily logs).
    std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .fold(None::<(std::time::SystemTime, PathBuf)>, |best, e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("log") {
                return best;
            }
            let m = e.metadata().and_then(|m| m.modified()).ok();
            match (best, m) {
                (Some((bt, _bp)), Some(mt)) if mt > bt => Some((mt, p)),
                (None, Some(mt)) => Some((mt, p)),
                (b, _) => b,
            }
        })
        .map(|(_, p)| p)
}

/// Read the last `max` lines from the final `bytes` of a (possibly huge) file.
fn tail_lines(path: &std::path::Path, bytes: u64, max: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(bytes);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    let from = all.len().saturating_sub(max);
    all[from..].iter().map(|s| s.to_string()).collect()
}

/// One captured request in the continuous Runtime panel (from Clockwork).
#[derive(Clone)]
pub struct RuntimeReq {
    pub id: String,
    pub method: String,
    pub uri: String,
    pub status: u16,
    pub duration_ms: f64,
    pub queries: Vec<(String, String)>,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub mails: Vec<String>,
    pub events: usize,
}

/// Parse a Clockwork `/__clockwork/latest` payload into a [`RuntimeReq`].
fn parse_clockwork_latest(v: &serde_json::Value) -> Option<RuntimeReq> {
    let id = v.get("id")?.as_str()?.to_string();
    let method = v
        .get("method")
        .and_then(|x| x.as_str())
        .unwrap_or("GET")
        .to_string();
    let uri = v
        .get("uri")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let status = v
        .get("responseStatus")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u16;
    let duration_ms = v
        .get("responseDuration")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0);
    let queries = v
        .get("databaseQueries")
        .and_then(|q| q.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|q| {
                    let sql = q.get("query").and_then(|s| s.as_str())?.to_string();
                    let dur = q.get("duration").map(|d| d.to_string()).unwrap_or_default();
                    Some((sql, dur))
                })
                .collect()
        })
        .unwrap_or_default();
    let (mut cache_hits, mut cache_misses) = (0, 0);
    if let Some(arr) = v.get("cacheQueries").and_then(|c| c.as_array()) {
        for c in arr {
            match c.get("type").and_then(|t| t.as_str()) {
                Some("hit") => cache_hits += 1,
                Some("miss") => cache_misses += 1,
                _ => {}
            }
        }
    }
    let mails = v
        .get("emailsData")
        .or_else(|| v.get("emails"))
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| {
                    m.get("data")
                        .and_then(|d| d.get("subject"))
                        .or_else(|| m.get("subject"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("(email)")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let events = v
        .get("events")
        .and_then(|e| e.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Some(RuntimeReq {
        id,
        method,
        uri,
        status,
        duration_ms,
        queries,
        cache_hits,
        cache_misses,
        mails,
        events,
    })
}

struct RequestResult {
    status: Option<u16>,
    time: String,
    body: String,
    queries: Vec<(String, String)>,
    error: Option<String>,
    /// For an Inertia response: `(component name, props JSON)`.
    inertia: Option<(String, serde_json::Value)>,
}

/// Extract the Inertia page object embedded in the initial HTML response's
/// `data-page="…"` attribute (HTML-escaped JSON).
fn extract_inertia(body: &str) -> Option<(String, serde_json::Value)> {
    let at = body.find("data-page=\"")? + "data-page=\"".len();
    let end = body[at..].find('"')? + at;
    let escaped = &body[at..end];
    let decoded = escaped
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let v: serde_json::Value = serde_json::from_str(&decoded).ok()?;
    let component = v.get("component")?.as_str()?.to_string();
    let props = v.get("props").cloned().unwrap_or(serde_json::Value::Null);
    Some((component, props))
}

/// Replace Laravel route params (`{id}`, `{id?}`) with a placeholder value.
fn substitute_route_params(uri: &str) -> String {
    let mut out = String::new();
    let mut in_brace = false;
    for c in uri.chars() {
        if in_brace {
            if c == '}' {
                in_brace = false;
            }
        } else if c == '{' {
            in_brace = true;
            out.push('1');
        } else {
            out.push(c);
        }
    }
    out
}

/// Perform the request via the system `curl` (`-k` so Grove's private-CA HTTPS
/// works), then fetch Clockwork query data if the app exposes it.
fn do_http_request(base: &str, url: &str) -> RequestResult {
    let hdr = std::env::temp_dir().join(format!("e-req-{}.hdr", std::process::id()));
    let out = std::process::Command::new("curl")
        .args([
            "-sk",
            "--max-time",
            "25",
            "-H",
            "X-Requested-With: XMLHttpRequest",
            "-H",
            "Accept: application/json, text/html",
            "-D",
        ])
        .arg(&hdr)
        .arg("-w")
        .arg("\n__E_META__%{http_code}__%{time_total}")
        .arg(url)
        .output();
    let raw = match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(e) => {
            return RequestResult {
                status: None,
                time: String::new(),
                body: String::new(),
                queries: Vec::new(),
                error: Some(format!("curl failed: {e} (is curl installed?)")),
                inertia: None,
            }
        }
    };
    let (body, status, time) = match raw.rsplit_once("\n__E_META__") {
        Some((b, meta)) => {
            let mut parts = meta.splitn(2, "__");
            let status = parts.next().and_then(|s| s.trim().parse::<u16>().ok());
            let time = parts.next().unwrap_or("").trim().to_string();
            (b.to_string(), status, time)
        }
        None => (raw, None, String::new()),
    };

    // Clockwork query capture, if the app has laravel/clockwork.
    let mut queries = Vec::new();
    if let Ok(headers) = std::fs::read_to_string(&hdr) {
        let id = headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("x-clockwork-id") {
                Some(v.trim().to_string())
            } else {
                None
            }
        });
        if let Some(id) = id {
            let cw = std::process::Command::new("curl")
                .args(["-sk", "--max-time", "10"])
                .arg(format!("{base}/__clockwork/{id}"))
                .output();
            if let Ok(o) = cw {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
                    if let Some(arr) = v.get("databaseQueries").and_then(|q| q.as_array()) {
                        for q in arr {
                            let sql = q.get("query").and_then(|s| s.as_str()).unwrap_or("");
                            let dur = q.get("duration").map(|d| d.to_string()).unwrap_or_default();
                            if !sql.is_empty() {
                                queries.push((sql.to_string(), dur));
                            }
                        }
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_file(&hdr);
    let inertia = extract_inertia(&body);
    RequestResult {
        status,
        time,
        body,
        queries,
        error: None,
        inertia,
    }
}

/// Wall-clock `HH:MM:SS` (UTC) for the agent audit log.
fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Epoch milliseconds as `u64` (for the undo tree and its panel).
pub fn now_ms_epoch() -> u64 {
    now_ms() as u64
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl AppState {
    pub fn new(cx: Scope, root: PathBuf) -> Self {
        let (tx, rx) = channel();
        let (term_tx, term_rx) = channel();
        Self {
            cx,
            roots: RwSignal::new(vec![root.clone()]),
            root: RwSignal::new(root),
            buffers: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
            active2: RwSignal::new(None),
            split: RwSignal::new(false),
            focused: RwSignal::new(0),
            next_id: RwSignal::new(1),
            palette_open: RwSignal::new(false),
            lsp_clients: RwSignal::new(HashMap::new()),
            lsp_failed: RwSignal::new(HashSet::new()),
            diagnostics: RwSignal::new(HashMap::new()),
            diag_tx: RwSignal::new(tx),
            diag_rx: RwSignal::new(Some(rx)),
            completion: Completion::new(),
            hover: HoverState::new(),
            signature: SignatureState::new(),
            laravel: RwSignal::new(None),
            picker: Picker::new(),
            terminals: RwSignal::new(Vec::new()),
            active_terminal: RwSignal::new(None),
            active_terminal2: RwSignal::new(None),
            term_split: RwSignal::new(false),
            term_focus_pane: RwSignal::new(0),
            next_term_id: RwSignal::new(1),
            term_rename_id: RwSignal::new(None),
            term_rename_input: RwSignal::new(String::new()),
            terminal_open: RwSignal::new(false),
            terminal_focused: RwSignal::new(false),
            term_tick: RwSignal::new(0),
            term_tx: RwSignal::new(term_tx),
            term_rx: RwSignal::new(Some(term_rx)),
            outline: RwSignal::new(Vec::new()),
            find: FindState::new(),
            rename: RenameState::new(),
            last_edit: RwSignal::new(0),
            md_preview: RwSignal::new(false),
            cmd: CmdPalette::new(),
            diff_open: RwSignal::new(false),
            settings: RwSignal::new(config::load_settings()),
            sidebar_open: RwSignal::new(true),
            file_op: FileOp::new(),
            fs_rev: RwSignal::new(0),
            about_open: RwSignal::new(false),
            agent_open: RwSignal::new(false),
            agents: RwSignal::new(config::load_agents()),
            agent_current: RwSignal::new(config::load_default_agent()),
            agent_term: RwSignal::new(None),
            agent_focused: RwSignal::new(false),
            agent_focus_pulse: RwSignal::new(0),
            sidebar_width: RwSignal::new(240.0),
            agent_width: RwSignal::new(460.0),
            db_width: RwSignal::new(280.0),
            term_height: RwSignal::new(320.0),
            db_open: RwSignal::new(false),
            db_conns: RwSignal::new(Vec::new()),
            db_adding: RwSignal::new(false),
            db_form: RwSignal::new(DbForm::default()),
            db_result_open: RwSignal::new(false),
            db_result: RwSignal::new(None),
            db_result_title: RwSignal::new(String::new()),
            db_result_error: RwSignal::new(None),
            db_result_loading: RwSignal::new(false),
            db_result_key: RwSignal::new(None),
            db_query_text: RwSignal::new(String::new()),
            db_result_table: RwSignal::new(None),
            db_subview: RwSignal::new("data".into()),
            db_columns: RwSignal::new(Vec::new()),
            db_sort: RwSignal::new(None),
            db_page: RwSignal::new(0),
            db_test_state: RwSignal::new(String::new()),
            db_editing_key: RwSignal::new(None),
            db_scroll: RwSignal::new((0.0, 0.0, 0)),
            db_consent: RwSignal::new(None),
            tinker_open: RwSignal::new(false),
            tinker_output: RwSignal::new(String::new()),
            tinker_running: RwSignal::new(false),
            map_open: RwSignal::new(false),
            map_query: RwSignal::new(String::new()),
            agent_log: RwSignal::new(Vec::new()),
            agent_log_open: RwSignal::new(false),
            agent_mark: RwSignal::new(None),
            agent_edit: RwSignal::new(None),
            sem_open: RwSignal::new(false),
            sem_query: RwSignal::new(String::new()),
            sem_status: RwSignal::new(String::new()),
            sem_results: RwSignal::new(Vec::new()),
            sem_index: RwSignal::new(Rc::new(RefCell::new(crate::semantic::SemIndex::default()))),
            undo_open: RwSignal::new(false),
            undo_rev: RwSignal::new(0),
            schema_diff_open: RwSignal::new(false),
            schema_diff: RwSignal::new(Vec::new()),
            rel_open: RwSignal::new(false),
            rel_graph: RwSignal::new(Vec::new()),
            contract_open: RwSignal::new(false),
            contract: RwSignal::new(None),
            related_open: RwSignal::new(false),
            related_items: RwSignal::new(Vec::new()),
            runtime_open: RwSignal::new(false),
            runtime_reqs: RwSignal::new(Vec::new()),
            runtime_expanded: RwSignal::new(None),
            runtime_polling: RwSignal::new(false),
            log_open: RwSignal::new(false),
            log_lines: RwSignal::new(Vec::new()),
            db_schema_cache: RwSignal::new(std::collections::HashMap::new()),
            req_open: RwSignal::new(false),
            req_url: RwSignal::new(String::new()),
            req_status: RwSignal::new(None),
            req_time: RwSignal::new(String::new()),
            req_body: RwSignal::new(String::new()),
            req_queries: RwSignal::new(Vec::new()),
            req_error: RwSignal::new(None),
            req_running: RwSignal::new(false),
            req_inertia: RwSignal::new(None),
            tdd_open: RwSignal::new(false),
            tdd_status: RwSignal::new(TddStatus::Idle),
            tdd_output: RwSignal::new(String::new()),
            tdd_iteration: RwSignal::new(0),
            tdd_loop: RwSignal::new(false),
            db_edit: RwSignal::new(None),
            db_edit_value: RwSignal::new(String::new()),
            db_edit_null: RwSignal::new(false),
            db_queries: RwSignal::new(Vec::new()),
            db_saving_query: RwSignal::new(false),
            db_query_name: RwSignal::new(String::new()),
            update_info: RwSignal::new(None),
            update_status: RwSignal::new(crate::updater::UpdateStatus::Idle),
            update_notes_open: RwSignal::new(false),
            goto: crate::editing::GotoState::new(),
            task: crate::task_palette::TaskState::new(),
            task_list: RwSignal::new(Vec::new()),
            close_confirm: RwSignal::new(None),
            recent_files: RwSignal::new(Vec::new()),
            recent: crate::recent::RecentState::new(),
            settings_open: RwSignal::new(false),
            pinned_tabs: RwSignal::new(HashSet::new()),
            git_panel_open: RwSignal::new(false),
            git_root: RwSignal::new(None),
            git_branch: RwSignal::new(None),
            git_status: RwSignal::new(Vec::new()),
            git_commit_msg: RwSignal::new(String::new()),
            git_log: RwSignal::new(Vec::new()),
            git_stash_count: RwSignal::new(0),
            font_size: RwSignal::new(config::load_settings().font_size),
            word_wrap: RwSignal::new(false),
            nav_back_stack: RwSignal::new(Vec::new()),
            nav_fwd_stack: RwSignal::new(Vec::new()),
            blame_rev: RwSignal::new(0),
        }
    }

    /// Load git blame for a buffer in the background.
    pub fn load_blame(&self, id: u64) {
        let Some(buf) = self.buffer_by_id(id) else {
            return;
        };
        if buf.large {
            return;
        }
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let blame_cell = buf.blame.clone();
        let rev = self.blame_rev;
        let send = create_ext_action(self.cx, move |lines: Vec<(String, i64, String)>| {
            *blame_cell.borrow_mut() = lines;
            rev.update(|r| *r += 1);
        });
        std::thread::spawn(move || {
            send(git::blame(&path));
        });
    }

    /// Blame string for the active cursor line, if available.
    pub fn active_line_blame(&self) -> Option<String> {
        let buf = self.active_buffer()?;
        let editor = buf.editor.get_untracked()?;
        let (line, _) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let b = buf.blame.borrow();
        let (author, time, summary) = b.get(line)?.clone();
        if summary.is_empty() {
            return None;
        }
        if time == 0 {
            Some(format!("{author} • {summary}"))
        } else {
            Some(format!("{author}, {} • {summary}", rel_time(time)))
        }
    }

    pub fn toggle_word_wrap(&self) {
        self.word_wrap.update(|w| *w = !*w);
    }

    /// Increase / decrease / reset the editor font size (zoom).
    pub fn zoom(&self, delta: i64) {
        let cur = self.font_size.get_untracked() as i64;
        let next = (cur + delta).clamp(8, 32) as usize;
        self.font_size.set(next);
        self.repaint_all_buffers();
    }

    pub fn zoom_reset(&self) {
        self.font_size.set(self.settings.get_untracked().font_size);
        self.repaint_all_buffers();
    }

    /// Whether any focus-grabbing overlay (palette, find, prompt, dialog) is
    /// open. The editor must not steal keyboard focus while one of these is up.
    pub fn any_overlay_open(&self) -> bool {
        self.palette_open.get()
            || self.cmd.open.get()
            || self.picker.open.get()
            || self.find.open.get()
            || self.rename.open.get()
            || self.goto.open.get()
            || self.recent.open.get()
            || self.about_open.get()
            || self.close_confirm.get().is_some()
            || self.term_rename_id.get().is_some()
    }

    /// Force a re-layout of every open buffer (e.g. after a font-size change).
    fn repaint_all_buffers(&self) {
        self.buffers.with_untracked(|bs| {
            for b in bs {
                b.doc.cache_rev().update(|r| *r += 1);
            }
        });
    }

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
    pub fn delete_path(&self, path: PathBuf) {
        let script = format!(
            "tell application \"Finder\" to delete POSIX file \"{}\"",
            path.display()
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output();
        if let Some(id) = self.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(path.as_path()))
                .map(|b| b.id)
        }) {
            self.close(id);
        }
        self.fs_rev.update(|r| *r += 1);
    }

    pub fn copy_path_to_clipboard(&self, path: &std::path::Path) {
        let _ = floem::Clipboard::set_contents(path.display().to_string());
    }

    pub fn reveal_in_finder(&self, path: &std::path::Path) {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }

    pub fn toggle_md_preview(&self) {
        let cur = self.md_preview.get_untracked();
        self.md_preview.set(!cur);
    }

    pub fn toggle_diff(&self) {
        let cur = self.diff_open.get_untracked();
        self.diff_open.set(!cur);
    }

    // ---- Local rename --------------------------------------------------

    /// Open the rename bar for the identifier under the cursor.
    pub fn open_rename(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let word = word_at(&text, offset);
        if word.is_empty() {
            return;
        }
        let r = self.rename;
        r.word.set(word.clone());
        r.new_name.set(word);
        r.open.set(true);
    }

    pub fn close_rename(&self) {
        self.rename.open.set(false);
    }

    /// Multi-cursor (⌘D): expand the caret to its word, or add a cursor at the
    /// next occurrence of the current selection.
    pub fn select_next_occurrence(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let regions = sel.regions().to_vec();
        let all_carets = regions.iter().all(|r| r.start == r.end);

        let new_sel = if all_carets {
            // Expand each caret to the surrounding word.
            let mut s = Selection::new();
            for r in &regions {
                let (a, b) = word_range(&text, r.max());
                if b > a {
                    s.add_region(SelRegion::new(a, b, None));
                } else {
                    s.add_region(*r);
                }
            }
            s
        } else {
            // Add the next occurrence of the last non-empty region's text.
            let mut s = sel.clone();
            if let Some(last) = regions.iter().rev().find(|r| r.max() > r.min()) {
                let word = text[last.min()..last.max()].to_string();
                if let Some(pos) = find_next(&text, &word, last.max()) {
                    s.add_region(SelRegion::new(pos, pos + word.len(), None));
                }
            }
            s
        };

        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(new_sel), None, None));
    }

    /// Place a cursor on every occurrence of the current word/selection (⌘⇧L).
    pub fn select_all_occurrences(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let regions = sel.regions().to_vec();
        let all_carets = regions.iter().all(|r| r.start == r.end);

        let (term, whole) = if all_carets {
            let Some(r) = regions.last() else {
                return;
            };
            let (a, b) = word_range(&text, r.max());
            if b <= a {
                return;
            }
            (text[a..b].to_string(), true)
        } else {
            let Some(last) = regions.iter().rev().find(|r| r.max() > r.min()) else {
                return;
            };
            (text[last.min()..last.max()].to_string(), false)
        };

        if term.is_empty() {
            return;
        }
        let occ = find_all_opts(&text, &term, true, whole, false);
        if occ.is_empty() {
            return;
        }
        let mut s = Selection::new();
        for (a, b) in occ {
            s.add_region(SelRegion::new(a, b, None));
        }
        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(s), None, None));
    }

    // ---- Livewire ------------------------------------------------------

    /// Completion items for a `wire:model` value, from the component's class.
    fn livewire_property_items(
        &self,
        buf: &Buffer,
        partial: &str,
    ) -> Option<Vec<lsp_types::CompletionItem>> {
        let path = buf.file.path.as_ref()?;
        let comp = crate::livewire::resolve(&self.root.get_untracked(), path)?;
        let src = std::fs::read_to_string(&comp.class_file).ok()?;
        let lower = partial.to_lowercase();
        let items: Vec<lsp_types::CompletionItem> = crate::livewire::properties(&src)
            .into_iter()
            .filter(|p| lower.is_empty() || p.to_lowercase().starts_with(&lower))
            .map(|p| lsp_types::CompletionItem {
                label: p.clone(),
                insert_text: Some(p.clone()),
                kind: Some(lsp_types::CompletionItemKind::FIELD),
                detail: Some("Livewire property".to_string()),
                ..Default::default()
            })
            .collect();
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    }

    /// Caret on an `Inertia::render('Page')` string jumps to the page component.
    fn goto_inertia_page(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        if buf.file.language != Language::Php {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let Some(name) = crate::inertia::render_at(&text, offset) else {
            return false;
        };
        if let Some(p) = crate::inertia::resolve_page(&self.root.get_untracked(), &name) {
            self.jump_to(&path_to_uri(&p), 0, 0);
            true
        } else {
            false
        }
    }

    /// Open an Inertia page component if `name` resolves to one, else fall back
    /// to Blade view resolution. Used by the architecture map.
    pub fn open_page_or_view(&self, name: &str) {
        let root = self.root.get_untracked();
        if let Some(p) = crate::inertia::resolve_page(&root, name) {
            self.jump_to(&path_to_uri(&p), 0, 0);
            return;
        }
        if let Some(data) = self.laravel.get_untracked() {
            if let Some((p, l, c)) = laravel::navigate(&data, laravel::Helper::View, name) {
                self.jump_to(&path_to_uri(&p), l, c);
            }
        }
    }

    /// Jump between a Livewire component's Blade view and its class file.
    pub fn livewire_companion(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let Some(comp) = crate::livewire::resolve(&self.root.get_untracked(), &path) else {
            Self::notify("Not a Livewire component");
            return;
        };
        let target = if path == comp.class_file {
            comp.view_file
        } else {
            comp.class_file
        };
        self.open_path(target);
    }

    /// If the caret sits on a Livewire property in the view, jump to its
    /// declaration in the class. Returns `true` if it handled the jump.
    fn livewire_goto(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let Some(path) = buf.file.path.clone() else {
            return false;
        };
        let Some(comp) = crate::livewire::resolve(&self.root.get_untracked(), &path) else {
            return false;
        };
        // Only jump view → class here (class → view is the companion command).
        if path != comp.view_file {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let word = word_at(&text, offset);
        let word = word.trim_start_matches('$');
        if word.is_empty() {
            return false;
        }
        let Ok(src) = std::fs::read_to_string(&comp.class_file) else {
            return false;
        };
        if !crate::livewire::properties(&src).iter().any(|p| p == word) {
            return false;
        }
        let line = crate::livewire::property_line(&src, word).unwrap_or(0);
        self.jump_to(&path_to_uri(&comp.class_file), line, 0);
        true
    }

    /// Rename a Livewire property across both the class and the view. Returns
    /// `true` if it handled the rename.
    fn livewire_rename(&self, old: &str, new: &str) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let Some(path) = buf.file.path.clone() else {
            return false;
        };
        let Some(comp) = crate::livewire::resolve(&self.root.get_untracked(), &path) else {
            return false;
        };
        let Ok(class_src) = std::fs::read_to_string(&comp.class_file) else {
            return false;
        };
        if !crate::livewire::properties(&class_src)
            .iter()
            .any(|p| p == old)
        {
            return false;
        }
        // Rewrite both files (targeted so unrelated tokens are left alone).
        let new_class = crate::livewire::class_rename(&class_src, old, new);
        self.rewrite_file(&comp.class_file, new_class);
        if let Ok(view_src) = std::fs::read_to_string(&comp.view_file) {
            let new_view = crate::livewire::view_rename(&view_src, old, new);
            self.rewrite_file(&comp.view_file, new_view);
        }
        Self::notify(&format!("Renamed Livewire property `{old}` → `{new}`"));
        true
    }

    /// Replace a file's contents, editing the open buffer (undoable) if it is
    /// open, otherwise writing to disk.
    fn rewrite_file(&self, path: &std::path::Path, content: String) {
        let open = self.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(path))
                .map(|b| (b.doc.clone(), b.dirty))
        });
        if let Some((doc, dirty)) = open {
            let len = doc.text().len();
            let mut it = std::iter::once((Selection::region(0, len), content.as_str()));
            doc.edit(&mut it, EditType::InsertChars);
            dirty.set(true);
        } else {
            let _ = buffer::write(path, &content);
            self.fs_rev.update(|r| *r += 1);
        }
    }

    pub fn apply_rename(&self) {
        let r = self.rename;
        if !r.open.get_untracked() {
            return;
        }
        let word = r.word.get_untracked();
        let new_name = r.new_name.get_untracked();
        r.open.set(false);
        if new_name.is_empty() || new_name == word {
            return;
        }
        // Livewire property rename spans the class *and* the view.
        let prop = word.trim_start_matches('$');
        if self.livewire_rename(prop, new_name.trim_start_matches('$')) {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let occ = whole_word_occurrences(&text, &word);
        if occ.is_empty() {
            return;
        }
        let edits: Vec<(Selection, String)> = occ
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), new_name.clone()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    /// Save all dirty buffers to disk (no formatting) — used by idle auto-save.
    pub fn maybe_autosave(&self) {
        if !self.settings.get_untracked().autosave {
            return;
        }
        let last = self.last_edit.get_untracked();
        if last == 0 || now_ms().saturating_sub(last) < 1500 {
            return;
        }
        self.last_edit.set(0);
        let buffers = self.buffers.get_untracked();
        for b in &buffers {
            if !b.dirty.get_untracked() {
                continue;
            }
            let Some(path) = b.file.path.as_ref() else {
                continue;
            };
            let text = b.doc.text().to_string();
            if buffer::write_with_encoding(path, &text, &b.encoding.get_untracked()).is_ok() {
                b.dirty.set(false);
                Self::refresh_disk_mtime(b);
                if let (Some(uri), Some(client)) =
                    (b.uri.as_ref(), self.lsp_for_language(b.file.language))
                {
                    client.did_save(uri, &text);
                }
                self.request_inlay_hints(b.id);
            }
        }
    }

    // ---- Merge conflicts ------------------------------------------------

    /// Expand an Emmet abbreviation before the cursor (HTML-family languages).
    /// Returns true when something was expanded (so Tab is consumed).
    pub fn try_emmet_expand(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        if !matches!(
            buf.file.language,
            Language::Html | Language::Php | Language::Blade | Language::Vue | Language::Svelte
        ) {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let end = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let end = end.min(text.len());
        let line_start = text[..end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_before = &text[line_start..end];

        let Some((rel_start, abbr)) = crate::emmet::abbreviation_at(line_before) else {
            return false;
        };
        if !crate::emmet::is_expandable(&abbr) {
            return false;
        }
        let unit = " ".repeat(self.settings.get_untracked().tab_width.clamp(1, 8));
        let Some(markup) = crate::emmet::expand(&abbr, &unit) else {
            return false;
        };

        // Re-indent continuation lines to the current line's indentation.
        let base = line_indent(&text, line_start);
        let markup = markup.replace('\n', &format!("\n{base}"));
        let caret = markup.find('\0').unwrap_or(markup.len());
        let markup = markup.replace('\0', "");

        let start = line_start + rel_start;
        buf.doc.edit_single(
            Selection::region(start, end),
            &markup,
            EditType::InsertChars,
        );
        let pos = start + caret;
        editor.cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(pos)),
            None,
            None,
        ));
        true
    }

    /// Convert the active buffer's line endings to CRLF (`true`) or LF.
    pub fn set_line_ending(&self, crlf: bool) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let lf = text.replace("\r\n", "\n");
        let new = if crlf { lf.replace('\n', "\r\n") } else { lf };
        if new == text {
            return;
        }
        let mut it = std::iter::once((Selection::region(0, text.len()), new.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    /// Whether the active buffer contains conflict markers.
    pub fn active_has_conflicts(&self) -> bool {
        self.active_buffer()
            .map(|b| b.doc.text().to_string().contains("<<<<<<<"))
            .unwrap_or(false)
    }

    /// The conflict block containing the caret: `(start, end, current, incoming)`.
    fn active_conflict_block(&self) -> Option<(usize, usize, String, String)> {
        let buf = self.active_buffer()?;
        let editor = buf.editor.get_untracked()?;
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        find_conflict(&text, offset)
    }

    /// Resolve the conflict at the caret: 0 = current, 1 = incoming, 2 = both.
    pub fn resolve_conflict(&self, choice: u8) {
        let Some((start, end, current, incoming)) = self.active_conflict_block() else {
            return;
        };
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let replacement = match choice {
            0 => current,
            1 => incoming,
            _ => format!("{current}{incoming}"),
        };
        let mut it = std::iter::once((Selection::region(start, end), replacement.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    // ---- External file changes -----------------------------------------

    /// Read and store the on-disk mtime for a buffer (after we write it, to
    /// avoid mistaking our own save for an external change).
    fn refresh_disk_mtime(buf: &Buffer) {
        let mtime = buf
            .file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        buf.disk_mtime.set(mtime);
    }

    /// Poll open files for on-disk changes (called on the idle tick). Clean
    /// buffers are reloaded silently; dirty ones are flagged for the user.
    pub fn check_external_changes(&self) {
        // Snapshot (id, path, last-known mtime) cheaply on the UI thread, then
        // do the actual `stat` calls on a worker thread — those can block on
        // slow/network filesystems and must never stall the UI.
        let buffers = self.buffers.get_untracked();
        let mut items: Vec<(u64, PathBuf, Option<std::time::SystemTime>)> = Vec::new();
        for b in &buffers {
            if let Some(path) = b.file.path.as_ref() {
                items.push((b.id, path.clone(), b.disk_mtime.get_untracked()));
            }
        }
        if items.is_empty() {
            return;
        }
        let state = *self;
        // (id, new mtime, is_first_observation)
        let send = create_ext_action(
            self.cx,
            move |changed: Vec<(u64, std::time::SystemTime, bool)>| {
                for (id, mtime, first) in changed {
                    let Some(b) = state.buffer_by_id(id) else {
                        continue;
                    };
                    b.disk_mtime.set(Some(mtime));
                    if first {
                        continue;
                    }
                    if b.dirty.get_untracked() {
                        b.disk_changed.set(true);
                    } else {
                        state.reload_buffer(&b);
                    }
                }
            },
        );
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for (id, path, prev) in items {
                let Some(mtime) = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                else {
                    continue;
                };
                match prev {
                    None => out.push((id, mtime, true)),
                    Some(p) if p != mtime => out.push((id, mtime, false)),
                    _ => {}
                }
            }
            send(out);
        });
    }

    /// Reload a buffer's contents from disk, discarding any unsaved edits.
    fn reload_buffer(&self, buf: &Buffer) {
        let Some(path) = buf.file.path.as_ref() else {
            return;
        };
        // Honour the file's detected encoding (a non-UTF-8 file must not be
        // re-read as raw UTF-8 on external change).
        let Ok((content, encoding)) = buffer::read_with_encoding(path) else {
            return;
        };
        buf.encoding.set(encoding);
        if content == buf.doc.text().to_string() {
            buf.disk_changed.set(false);
            return;
        }
        let old_len = buf.doc.text().len();
        let mut it = std::iter::once((Selection::region(0, old_len), content.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.dirty.set(false);
        buf.disk_changed.set(false);
        Self::refresh_disk_mtime(buf);
    }

    /// Reload the active buffer from disk (used by the conflict banner).
    pub fn reload_active_from_disk(&self) {
        if let Some(buf) = self.active_buffer() {
            self.reload_buffer(&buf);
        }
    }

    /// Dismiss the disk-change conflict, keeping the in-memory version.
    pub fn keep_active_version(&self) {
        if let Some(buf) = self.active_buffer() {
            buf.disk_changed.set(false);
        }
    }

    // ---- Find in file --------------------------------------------------

    pub fn open_find(&self) {
        self.find.open.set(true);
        self.find.replace_open.set(false);
    }

    /// Open the find bar with the replace row expanded.
    pub fn open_replace(&self) {
        self.find.open.set(true);
        self.find.replace_open.set(true);
    }

    pub fn close_find(&self) {
        self.find.open.set(false);
        self.find.matches.set(Vec::new());
        if let Some(buf) = self.active_buffer() {
            *buf.find_marks.borrow_mut() = Vec::new();
            buf.doc.cache_rev().update(|r| *r += 1);
        }
    }

    /// Recompute matches for the current query (called as the query changes).
    pub fn run_find(&self) {
        let query = self.find.query.get_untracked();
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if query.is_empty() {
            self.find.matches.set(Vec::new());
            *buf.find_marks.borrow_mut() = Vec::new();
            buf.doc.cache_rev().update(|r| *r += 1);
            return;
        }
        let text = buf.doc.text().to_string();
        let matches = find_all_opts(
            &text,
            &query,
            self.find.case_sensitive.get_untracked(),
            self.find.whole_word.get_untracked(),
            self.find.use_regex.get_untracked(),
        );
        self.find.matches.set(matches);
        self.find.current.set(0);
        self.apply_find_marks();
    }

    /// Replace the current match with the replacement text, then re-search.
    pub fn replace_current(&self) {
        let matches = self.find.matches.get_untracked();
        if matches.is_empty() {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let cur = self.find.current.get_untracked().min(matches.len() - 1);
        let (s, e) = matches[cur];
        let rep = self.find.replace.get_untracked();
        let sel = Selection::region(s, e);
        let mut it = std::iter::once((sel, rep.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        self.run_find();
    }

    /// Replace every match with the replacement text in one edit.
    pub fn replace_all(&self) {
        let matches = self.find.matches.get_untracked();
        if matches.is_empty() {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let rep = self.find.replace.get_untracked();
        let edits: Vec<(Selection, String)> = matches
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), rep.clone()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        self.run_find();
    }

    /// Rebuild per-line highlight spans and move the caret to the current match.
    fn apply_find_marks(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let matches = self.find.matches.get_untracked();
        let cur = self.find.current.get_untracked();
        let text = buf.doc.text().to_string();
        let starts = line_starts(&text);
        let mut lines: Vec<Vec<FindSpan>> = vec![Vec::new(); starts.len()];
        for (idx, (s, e)) in matches.iter().enumerate() {
            let line = line_of(&starts, *s);
            let ls = starts[line];
            lines[line].push(FindSpan {
                start: s - ls,
                end: e - ls,
                current: idx == cur,
            });
        }
        *buf.find_marks.borrow_mut() = lines;
        buf.doc.cache_rev().update(|r| *r += 1);

        if let Some(editor) = buf.editor.get_untracked() {
            if let Some((s, _)) = matches.get(cur) {
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(*s)),
                    None,
                    None,
                ));
            }
        }
    }

    pub fn find_next(&self) {
        let n = self.find.matches.with(|m| m.len());
        if n == 0 {
            return;
        }
        self.find
            .current
            .set((self.find.current.get_untracked() + 1) % n);
        self.apply_find_marks();
    }

    pub fn find_prev(&self) {
        let n = self.find.matches.with(|m| m.len());
        if n == 0 {
            return;
        }
        let cur = self.find.current.get_untracked();
        self.find.current.set((cur + n - 1) % n);
        self.apply_find_marks();
    }

    /// Recompute the matching-bracket highlight for the active buffer and
    /// repaint. Called from a cursor-tracking effect.
    pub fn update_bracket_marks(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if buf.large {
            return;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        *buf.bracket_marks.borrow_mut() = compute_bracket_marks(&text, offset);
        buf.doc.cache_rev().update(|r| *r += 1);
    }

    /// Load the document outline for the active buffer (LSP documentSymbol).
    /// Request LSP inlay hints for a buffer and store them as phantom text.
    pub fn request_inlay_hints(&self, id: u64) {
        if !self.settings.get_untracked().inlay_hints {
            return;
        }
        let Some(buf) = self.buffer_by_id(id) else {
            return;
        };
        if buf.large {
            return;
        }
        if lsp_language_id(buf.file.language).is_none() {
            return;
        }
        let (Some(client), Some(uri)) = (self.lsp_for_language(buf.file.language), buf.uri.clone())
        else {
            return;
        };
        let end_line = buf.doc.text().to_string().split('\n').count().max(1) as u32;
        let hints_sig = buf.inlay_hints;
        let cache = buf.doc.cache_rev();
        let send = create_ext_action(self.cx, move |hints: Vec<(u32, u32, String)>| {
            // Only repaint when the hints actually changed.
            if hints != hints_sig.get_untracked() {
                hints_sig.set(hints);
                cache.update(|r| *r += 1);
            }
        });
        std::thread::spawn(move || {
            let hints = client.inlay_hints(&uri, end_line).unwrap_or_default();
            send(hints);
        });
    }

    pub fn request_inlay_hints_active(&self) {
        if let Some(id) = self.focused_active_id() {
            self.request_inlay_hints(id);
        }
    }

    pub fn request_outline(&self) {
        let outline = self.outline;
        let Some(buf) = self.active_buffer() else {
            outline.set(Vec::new());
            return;
        };
        let (Some(client), Some(uri)) = (self.lsp_for_active(), buf.uri.clone()) else {
            outline.set(Vec::new());
            return;
        };
        if lsp_language_id(buf.file.language).is_none() {
            outline.set(Vec::new());
            return;
        }
        let send = create_ext_action(self.cx, move |items: Vec<OutlineItem>| outline.set(items));
        std::thread::spawn(move || {
            let syms = client.document_symbols(&uri).unwrap_or_default();
            let items = syms
                .into_iter()
                .map(|(name, kind, line, ch, depth)| OutlineItem {
                    name,
                    kind,
                    line,
                    char: ch,
                    depth,
                })
                .collect();
            send(items);
        });
    }

    // ---- Task runner ---------------------------------------------------

    /// Run a shell command in a new, named terminal tab.
    pub fn run_task(&self, name: &str, command: &str) {
        let Some(id) = self.spawn_terminal() else {
            return;
        };
        let pane = self.term_focus_pane.get_untracked();
        self.pane_active(pane).set(Some(id));
        self.terminal_open.set(true);
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

    // ---- Integrated terminal -------------------------------------------

    fn term_by_id(&self, id: u64) -> Option<Rc<RefCell<Terminal>>> {
        self.terminals
            .with_untracked(|ts| ts.iter().find(|t| t.id == id).map(|t| t.term.clone()))
    }

    /// The active-terminal signal of the given pane (0 or 1).
    fn pane_active(&self, pane: u8) -> RwSignal<Option<u64>> {
        if pane == 1 {
            self.active_terminal2
        } else {
            self.active_terminal
        }
    }

    /// The focused pane's active terminal id (reactive).
    pub fn focused_term_id(&self) -> Option<u64> {
        if self.term_focus_pane.get() == 1 {
            self.active_terminal2.get()
        } else {
            self.active_terminal.get()
        }
    }

    /// Spawn a new terminal in the focused pane, show the panel.
    pub fn new_terminal(&self) {
        let pane = self.term_focus_pane.get_untracked();
        if let Some(id) = self.spawn_terminal() {
            self.pane_active(pane).set(Some(id));
            self.terminal_open.set(true);
        }
    }

    fn spawn_terminal(&self) -> Option<u64> {
        let tx = self.term_tx.get();
        let on_update = Box::new(move || {
            let _ = tx.send(());
        });
        let root = self.root.get();
        match Terminal::spawn(&e_term::default_shell(), &root, 24, 100, on_update) {
            Ok(t) => {
                let id = self.next_term_id.get_untracked();
                self.next_term_id.set(id + 1);
                self.terminals.update(|ts| {
                    ts.push(TermSession {
                        id,
                        term: Rc::new(RefCell::new(t)),
                        name: RwSignal::new(String::new()),
                    })
                });
                Some(id)
            }
            Err(e) => {
                eprintln!("e: terminal failed: {e:#}");
                None
            }
        }
    }

    /// Split the terminal: open a new shell in the second pane.
    pub fn split_terminal(&self) {
        if let Some(id) = self.spawn_terminal() {
            self.active_terminal2.set(Some(id));
            self.term_split.set(true);
            self.term_focus_pane.set(1);
            self.terminal_open.set(true);
        }
    }

    pub fn rename_terminal(&self, id: u64, name: String) {
        self.terminals.with_untracked(|ts| {
            if let Some(s) = ts.iter().find(|t| t.id == id) {
                s.name.set(name);
            }
        });
    }

    /// Open the rename prompt for a terminal tab.
    pub fn start_term_rename(&self, id: u64) {
        let current = self.terminals.with_untracked(|ts| {
            ts.iter()
                .find(|t| t.id == id)
                .map(|t| t.name.get_untracked())
        });
        self.term_rename_input.set(current.unwrap_or_default());
        self.term_rename_id.set(Some(id));
    }

    pub fn confirm_term_rename(&self) {
        if let Some(id) = self.term_rename_id.get_untracked() {
            let name = self.term_rename_input.get_untracked().trim().to_string();
            self.rename_terminal(id, name);
        }
        self.term_rename_id.set(None);
    }

    /// Toggle the terminal panel, spawning the first shell on first use.
    pub fn toggle_terminal(&self) {
        if self.terminals.with_untracked(|t| t.is_empty()) {
            self.new_terminal();
        } else {
            let open = self.terminal_open.get_untracked();
            self.terminal_open.set(!open);
        }
    }

    /// Focus a terminal in the currently focused pane (clicking a tab).
    pub fn focus_terminal(&self, id: u64) {
        let pane = self.term_focus_pane.get_untracked();
        self.pane_active(pane).set(Some(id));
    }

    /// Close a terminal session (kills its shell).
    pub fn close_terminal(&self, id: u64) {
        let mut next = None;
        self.terminals.update(|ts| {
            if let Some(pos) = ts.iter().position(|t| t.id == id) {
                ts.remove(pos);
                if !ts.is_empty() {
                    next = Some(ts[pos.min(ts.len() - 1)].id);
                }
            }
        });
        // Replace the closed id wherever it was active.
        if self.active_terminal.get_untracked() == Some(id) {
            self.active_terminal.set(next);
        }
        if self.active_terminal2.get_untracked() == Some(id) {
            self.active_terminal2.set(None);
            self.term_split.set(false);
            self.term_focus_pane.set(0);
        }
        if self.terminals.with_untracked(|t| t.is_empty()) {
            self.terminal_open.set(false);
        }
    }

    pub fn term_input_to(&self, id: u64, bytes: &[u8]) {
        if let Some(t) = self.term_by_id(id) {
            t.borrow_mut().write(bytes);
        }
    }

    /// Resize every terminal to the pane size.
    pub fn resize_terminal(&self, rows: usize, cols: usize) {
        self.terminals.with_untracked(|ts| {
            for t in ts {
                t.term.borrow().resize(rows, cols);
            }
        });
    }

    pub fn term_runs_of(&self, id: Option<u64>) -> Vec<Vec<e_term::Run>> {
        id.and_then(|i| self.term_by_id(i))
            .map(|t| t.borrow().snapshot_runs())
            .unwrap_or_default()
    }

    pub fn term_cursor_of(&self, id: Option<u64>) -> Option<(usize, usize)> {
        id.and_then(|i| self.term_by_id(i))
            .and_then(|t| t.borrow().cursor())
    }

    /// Scroll a terminal's scrollback. `up` scrolls into history.
    pub fn term_scroll(&self, id: Option<u64>, up: bool, lines: usize) {
        if let Some(t) = id.and_then(|i| self.term_by_id(i)) {
            if up {
                t.borrow().scroll_up(lines);
            } else {
                t.borrow().scroll_down(lines);
            }
            self.term_tick.update(|x| *x += 1);
        }
    }

    pub fn term_scroll_bottom(&self, id: Option<u64>) {
        if let Some(t) = id.and_then(|i| self.term_by_id(i)) {
            t.borrow().scroll_to_bottom();
        }
    }

    // ---- Agent panel ----------------------------------------------------

    /// The currently selected agent's config.
    pub fn current_agent(&self) -> Option<AgentConfig> {
        let id = self.agent_current.get_untracked();
        self.agents
            .with_untracked(|list| list.iter().find(|a| a.id == id).cloned())
            .or_else(|| self.agents.with_untracked(|l| l.first().cloned()))
    }

    /// Toggle the agent panel, launching the agent on first open.
    pub fn toggle_agent(&self) {
        let open = self.agent_open.get_untracked();
        if open {
            self.agent_open.set(false);
        } else {
            self.agent_open.set(true);
            if self.agent_term.get_untracked().is_none() {
                self.start_agent();
            }
            self.agent_focus_pulse.update(|x| *x += 1);
        }
    }

    /// (Re)start the selected agent in a fresh PTY.
    pub fn start_agent(&self) {
        let Some(agent) = self.current_agent() else {
            eprintln!("e: no agent configured");
            return;
        };
        let cwd = if agent.cwd.trim().is_empty() {
            self.root.get_untracked()
        } else {
            PathBuf::from(&agent.cwd)
        };
        let tx = self.term_tx.get_untracked();
        let on_update = Box::new(move || {
            let _ = tx.send(());
        });
        match Terminal::spawn_command(&agent.command, &cwd, 30, 100, on_update) {
            Ok(t) => self.agent_term.set(Some(Rc::new(RefCell::new(t)))),
            Err(e) => eprintln!("e: agent '{}' failed: {e:#}", agent.name),
        }
    }

    /// Switch to a different agent and restart the panel with it.
    pub fn select_agent(&self, id: &str) {
        self.agent_current.set(id.to_string());
        config::save_default_agent(id);
        self.agent_term.set(None);
        self.start_agent();
        self.agent_focus_pulse.update(|x| *x += 1);
    }

    pub fn restart_agent(&self) {
        self.agent_term.set(None);
        self.start_agent();
        self.agent_focus_pulse.update(|x| *x += 1);
    }

    pub fn agent_input(&self, bytes: &[u8]) {
        if let Some(t) = self.agent_term.get_untracked() {
            t.borrow_mut().write(bytes);
        }
    }

    /// Send a prompt to the AI agent panel (opening/starting it if needed) and
    /// focus it. Used by "Explain with agent" / "Fix with AI" affordances.
    pub fn send_to_agent(&self, prompt: &str) {
        let just_started = self.agent_term.get_untracked().is_none();
        if !self.agent_open.get_untracked() {
            self.agent_open.set(true);
        }
        if just_started {
            self.start_agent();
        }
        let text = format!("{}\r", prompt.replace('\n', " "));
        let state = *self;
        // A freshly spawned agent needs a moment before it accepts input.
        let delay = if just_started { 700 } else { 60 };
        floem::action::exec_after(std::time::Duration::from_millis(delay), move |_| {
            state.agent_input(text.as_bytes());
            state.agent_focus_pulse.update(|x| *x += 1);
        });
    }

    pub fn agent_runs(&self) -> Vec<Vec<e_term::Run>> {
        self.agent_term
            .get_untracked()
            .map(|t| t.borrow().snapshot_runs())
            .unwrap_or_default()
    }

    pub fn agent_cursor(&self) -> Option<(usize, usize)> {
        self.agent_term
            .get_untracked()
            .and_then(|t| t.borrow().cursor())
    }

    pub fn resize_agent(&self, rows: usize, cols: usize) {
        if let Some(t) = self.agent_term.get_untracked() {
            t.borrow().resize(rows, cols);
        }
    }

    /// Open the global settings file in the editor.
    pub fn open_settings(&self) {
        if let Some(path) = config::settings_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if !path.exists() {
                let _ = std::fs::write(&path, "{\n}\n");
            }
            self.open_path(path);
        }
    }

    // ---- New file / save as --------------------------------------------

    /// Create a new, empty, untitled buffer and focus it.
    pub fn new_untitled(&self) {
        let id = self.next_id.get_untracked();
        self.next_id.set(id + 1);

        let highlights: Highlights = Rc::new(RefCell::new(Vec::new()));
        let doc = Rc::new(TextDocument::new(self.cx, String::new()));
        doc.auto_indent.set(true);
        let dirty = RwSignal::new(false);
        let undo = Rc::new(RefCell::new(e_core::undotree::UndoTree::new("")));
        let undo_nav = Rc::new(std::cell::Cell::new(false));

        {
            let app = *self;
            let doc2 = doc.clone();
            let highlights = highlights.clone();
            let undo = undo.clone();
            let undo_nav = undo_nav.clone();
            doc.clone().add_on_update(move |_| {
                dirty.set(true);
                app.last_edit.set(now_ms());
                let text = doc2.text().to_string();
                *highlights.borrow_mut() = highlight_lines(Language::PlainText, &text);
                doc2.cache_rev().update(|r| *r += 1);
                if !undo_nav.get() && undo.borrow_mut().record(&text, now_ms() as u64, 700) {
                    app.undo_rev.update(|r| *r += 1);
                }
            });
        }

        let buf = Buffer {
            id,
            file: FileInfo::scratch(),
            doc,
            dirty,
            highlights,
            diag_lines: Rc::new(RefCell::new(Vec::new())),
            git_marks: Rc::new(RefCell::new(Vec::new())),
            find_marks: Rc::new(RefCell::new(Vec::new())),
            bracket_marks: Rc::new(RefCell::new(Vec::new())),
            uri: None,
            editor: RwSignal::new(None),
            win_origin: RwSignal::new(Point::ZERO),
            pending_goto: RwSignal::new(None),
            disk_mtime: RwSignal::new(None),
            disk_changed: RwSignal::new(false),
            blame: Rc::new(RefCell::new(Vec::new())),
            inlay_hints: RwSignal::new(Vec::new()),
            large: false,
            encoding: RwSignal::new("UTF-8".to_string()),
            undo,
            undo_nav,
        };
        self.buffers.update(|bs| bs.push(buf));
        self.focused_active().set(Some(id));
    }

    /// Prompt for a path and save the active buffer there, then reopen it so it
    /// gets the right language, LSP, and git integration.
    pub fn save_active_as(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let content = buf.doc.text().to_string();
        let id = buf.id;
        let state = *self;
        let opts = floem::file::FileDialogOptions::new()
            .title("Save As")
            .force_starting_directory(self.root.get_untracked());
        floem::action::save_as(opts, move |info| {
            if let Some(path) = info.and_then(|i| i.path.into_iter().next()) {
                if buffer::write(&path, &content).is_ok() {
                    state.force_close(id);
                    state.open_path(path);
                }
            }
        });
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
        let Ok(exe) = std::env::current_exe() else {
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

    /// Show a native macOS notification banner.
    pub(crate) fn notify(message: &str) {
        let script = format!(
            "display notification \"{}\" with title \"e\"",
            message.replace('"', "'")
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .spawn();
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
        let exe = std::env::current_exe().ok();
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
        if self.update_status.get_untracked() == UpdateStatus::Downloading {
            return;
        }
        self.update_status.set(UpdateStatus::Checking);
        let info_sig = self.update_info;
        let status_sig = self.update_status;
        let send =
            create_ext_action(
                self.cx,
                move |result: Option<updater::UpdateInfo>| match result {
                    Some(info) => {
                        info_sig.set(Some(info));
                        status_sig.set(UpdateStatus::Idle);
                    }
                    None => {
                        status_sig.set(if announce_up_to_date {
                            UpdateStatus::UpToDate
                        } else {
                            UpdateStatus::Idle
                        });
                    }
                },
            );
        std::thread::spawn(move || {
            let result = updater::check().unwrap_or(None);
            send(result);
        });
    }

    /// Download and install the available update in place (non-blocking).
    pub fn install_update(&self) {
        use crate::updater::{self, UpdateStatus};
        if self.update_status.get_untracked() == UpdateStatus::Downloading {
            return;
        }
        self.update_status.set(UpdateStatus::Downloading);
        let status_sig = self.update_status;
        let info_sig = self.update_info;
        let send = create_ext_action(self.cx, move |result: Result<(), String>| match result {
            Ok(()) => {
                // Keep the bundle's Info.plist version in sync with the binary.
                if let Some(info) = info_sig.get_untracked() {
                    updater::patch_bundle_version(&info.version);
                }
                status_sig.set(UpdateStatus::Installed);
            }
            Err(e) => status_sig.set(UpdateStatus::Failed(e)),
        });
        std::thread::spawn(move || {
            let result = updater::install().map_err(|e| format!("{e:#}"));
            send(result);
        });
    }

    /// Dismiss the update notice (until the next check).
    pub fn dismiss_update(&self) {
        self.update_info.set(None);
        self.update_notes_open.set(false);
        self.update_status.set(crate::updater::UpdateStatus::Idle);
    }

    /// Relaunch the application (used after an update is installed).
    pub fn restart_app(&self) {
        let exe = std::env::current_exe().ok();
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

    pub fn buffer_by_id(&self, id: u64) -> Option<Buffer> {
        self.buffers
            .with(|bs| bs.iter().find(|b| b.id == id).cloned())
    }

    /// The active-buffer signal of the focused pane.
    fn focused_active(&self) -> RwSignal<Option<u64>> {
        if self.focused.get_untracked() == 1 {
            self.active2
        } else {
            self.active
        }
    }

    /// Buffer id active in the focused pane, tracked reactively.
    pub fn focused_active_id(&self) -> Option<u64> {
        if self.focused.get() == 1 {
            self.active2.get()
        } else {
            self.active.get()
        }
    }

    /// Focus a buffer in the currently focused pane (e.g. clicking a tab).
    pub fn focus_buffer(&self, id: u64) {
        self.focused_active().set(Some(id));
    }

    pub fn is_pinned(&self, id: u64) -> bool {
        self.pinned_tabs.with(|set| set.contains(&id))
    }

    pub fn toggle_pin(&self, id: u64) {
        self.pinned_tabs.update(|set| {
            if !set.remove(&id) {
                set.insert(id);
            }
        });
    }

    /// Close every tab except `keep` (skipping pinned tabs).
    pub fn close_others(&self, keep: u64) {
        let ids: Vec<u64> = self.buffers.with_untracked(|bs| {
            bs.iter()
                .map(|b| b.id)
                .filter(|id| *id != keep && !self.is_pinned(*id))
                .collect()
        });
        for id in ids {
            self.close(id);
        }
    }

    /// Move tab `src` to the position of `target` (drag-to-reorder).
    pub fn reorder_tab(&self, src: u64, target: u64) {
        if src == target {
            return;
        }
        self.buffers.update(|bs| {
            let Some(from) = bs.iter().position(|b| b.id == src) else {
                return;
            };
            let b = bs.remove(from);
            let to = bs.iter().position(|x| x.id == target).unwrap_or(bs.len());
            bs.insert(to, b);
        });
    }

    fn buffer_id_by_path(&self, path: &str) -> Option<u64> {
        let canon = std::path::Path::new(path).canonicalize().ok();
        self.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == canon.as_deref())
                .map(|b| b.id)
        })
    }

    /// Restore the previous session for this workspace (open files, tabs, split).
    pub fn restore_session(&self) {
        let Some(data) = session::load(&self.root.get_untracked()) else {
            return;
        };
        for p in &data.open {
            self.open_path(PathBuf::from(p));
        }
        if let Some(a) = data
            .active
            .as_deref()
            .and_then(|a| self.buffer_id_by_path(a))
        {
            self.active.set(Some(a));
        }
        if data.split {
            self.split.set(true);
            if let Some(a2) = data
                .active2
                .as_deref()
                .and_then(|a| self.buffer_id_by_path(a))
            {
                self.active2.set(Some(a2));
            }
        }
    }

    /// Persist the current session.
    pub fn save_session(&self) {
        let buffers = self.buffers.get_untracked();
        let path_of = |id: Option<u64>| -> Option<String> {
            id.and_then(|i| buffers.iter().find(|b| b.id == i))
                .and_then(|b| b.file.path.as_ref())
                .map(|p| p.display().to_string())
        };
        let open: Vec<String> = buffers
            .iter()
            .filter_map(|b| b.file.path.as_ref().map(|p| p.display().to_string()))
            .collect();
        let data = SessionData {
            open,
            active: path_of(self.active.get_untracked()),
            active2: path_of(self.active2.get_untracked()),
            split: self.split.get_untracked(),
        };
        session::save(&self.root.get_untracked(), &data);
    }

    /// Toggle the two-pane split view.
    pub fn toggle_split(&self) {
        let on = !self.split.get_untracked();
        self.split.set(on);
        if on {
            if self.active2.get_untracked().is_none() {
                self.active2.set(self.active.get_untracked());
            }
            self.focused.set(1);
        } else {
            self.focused.set(0);
        }
    }

    /// If the workspace is a Laravel project, scrape its data in the background.
    pub fn load_laravel(&self) {
        if !self.settings.get_untracked().laravel {
            return;
        }
        let root = self.root.get();
        if !laravel::is_laravel(&root) {
            return;
        }
        let laravel_sig = self.laravel;
        let send = create_ext_action(self.cx, move |data: LaravelData| {
            eprintln!("e: loaded Laravel project data");
            laravel_sig.set(Some(Rc::new(data)));
        });
        std::thread::spawn(move || {
            let data = laravel::load(&root);
            send(data);
        });
    }

    // ---- Database panel ------------------------------------------------

    pub fn toggle_db_panel(&self) {
        let open = !self.db_open.get_untracked();
        self.db_open.set(open);
        if open && self.db_conns.with_untracked(|c| c.is_empty()) {
            self.load_databases();
        }
    }

    /// Load saved connections for the project; offer `.env` detection if empty.
    pub fn load_databases(&self) {
        let root = self.root.get_untracked();
        let saved = e_db::load_connections(&root);
        let entries: Vec<DbEntry> = saved
            .into_iter()
            .map(|c| DbEntry::new(self.cx, c))
            .collect();
        self.db_conns.set(entries);
        self.db_queries.set(e_db::load_queries(&root));
    }

    /// Fetch the project's DB schema (from `.env`) into an in-memory cache for
    /// Eloquent attribute completion. Runs in the background.
    pub fn load_db_schema_cache(&self) {
        let root = self.root.get_untracked();
        let Some(cfg) = e_db::from_env(&root) else {
            return;
        };
        let sig = self.db_schema_cache;
        let send = create_ext_action(
            self.cx,
            move |m: std::collections::HashMap<String, Vec<e_db::ColumnInfo>>| sig.set(m),
        );
        std::thread::spawn(move || {
            let mut map = std::collections::HashMap::new();
            if let Ok(conn) = e_db::connect(&cfg) {
                if let Ok(tables) = e_db::tables(&conn) {
                    for t in tables {
                        if let Ok(cols) = e_db::columns(&conn, &t) {
                            map.insert(t, cols);
                        }
                    }
                }
            }
            send(map);
        });
    }

    /// Save the current query editor text under the typed name.
    pub fn db_save_query(&self) {
        let name = self.db_query_name.get_untracked().trim().to_string();
        let sql = self.db_query_text.get_untracked();
        if name.is_empty() || sql.trim().is_empty() {
            self.db_saving_query.set(false);
            return;
        }
        self.db_queries.update(|q| {
            q.retain(|x| x.name != name);
            q.push(e_db::SavedQuery { name, sql });
            q.sort_by(|a, b| a.name.cmp(&b.name));
        });
        let _ = e_db::save_queries(&self.root.get_untracked(), &self.db_queries.get_untracked());
        self.db_query_name.set(String::new());
        self.db_saving_query.set(false);
    }

    /// Load a saved query into the editor.
    pub fn db_load_query(&self, sql: String) {
        self.db_query_text.set(sql);
    }

    #[allow(dead_code)]
    pub fn db_delete_query(&self, name: String) {
        self.db_queries.update(|q| q.retain(|x| x.name != name));
        let _ = e_db::save_queries(&self.root.get_untracked(), &self.db_queries.get_untracked());
    }

    fn db_persist(&self) {
        let root = self.root.get_untracked();
        let configs: Vec<e_db::DbConfig> = self
            .db_conns
            .with_untracked(|c| c.iter().map(|e| e.config.clone()).collect());
        let _ = e_db::save_connections(&root, &configs);
    }

    /// Add the connection inferred from the project's `.env`.
    pub fn db_add_from_env(&self) {
        let root = self.root.get_untracked();
        let Some(cfg) = e_db::from_env(&root) else {
            Self::notify("No DB_CONNECTION found in .env");
            return;
        };
        self.db_add_config(cfg);
    }

    fn db_add_config(&self, cfg: e_db::DbConfig) {
        let key = cfg.key();
        if self
            .db_conns
            .with_untracked(|c| c.iter().any(|e| e.key() == key))
        {
            return;
        }
        let entry = DbEntry::new(self.cx, cfg);
        self.db_conns.update(|c| c.push(entry.clone()));
        self.db_persist();
        self.db_connect(entry);
    }

    pub fn db_remove(&self, key: String) {
        self.db_conns.update(|c| c.retain(|e| e.key() != key));
        self.db_persist();
    }

    pub fn db_connect(&self, entry: DbEntry) {
        if entry.connecting.get_untracked() {
            return;
        }
        entry.connecting.set(true);
        entry.error.set(None);
        let cfg = entry.config.clone();
        let send = create_ext_action(
            self.cx,
            move |res: Result<(Arc<e_db::Conn>, Vec<String>), String>| {
                entry.connecting.set(false);
                match res {
                    Ok((conn, tables)) => {
                        entry.conn.set(Some(conn));
                        entry.tables.set(tables);
                        entry.expanded.set(true);
                    }
                    Err(e) => entry.error.set(Some(e)),
                }
            },
        );
        std::thread::spawn(move || {
            let res = e_db::connect(&cfg).and_then(|conn| {
                let conn = Arc::new(conn);
                let tables = e_db::tables(&conn)?;
                Ok((conn, tables))
            });
            send(res);
        });
    }

    pub fn db_disconnect(&self, entry: DbEntry) {
        entry.conn.set(None);
        entry.tables.set(Vec::new());
        entry.expanded.set(false);
    }

    pub fn db_toggle(&self, entry: DbEntry) {
        if entry.conn.get_untracked().is_some() {
            entry.expanded.update(|e| *e = !*e);
        } else {
            self.db_connect(entry);
        }
    }

    pub fn db_refresh_tables(&self, entry: DbEntry) {
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let send = create_ext_action(self.cx, move |tables: Vec<String>| {
            entry.tables.set(tables);
        });
        std::thread::spawn(move || {
            let tables = e_db::tables(&conn).unwrap_or_default();
            send(tables);
        });
    }

    /// Open a table's rows in the results overlay.
    pub fn db_open_table(&self, entry: DbEntry, table: String) {
        if entry.conn.get_untracked().is_none() {
            return;
        }
        self.db_result_key.set(Some(entry.key()));
        self.db_result_table.set(Some(table.clone()));
        self.db_result_title
            .set(format!("{} · {}", entry.config.display_name(), table));
        self.db_subview.set("data".into());
        self.db_sort.set(None);
        self.db_page.set(0);
        self.db_columns.set(Vec::new());
        self.db_result_open.set(true);
        self.db_load_columns(entry.clone(), table.clone());
        self.db_reload_table();
    }

    /// (Re)run the browse query for the current table, sort and page.
    pub fn db_reload_table(&self) {
        let (Some(key), Some(table)) = (
            self.db_result_key.get_untracked(),
            self.db_result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let engine = entry.config.engine.clone();
        let page = self.db_page.get_untracked();
        let sort = self.db_sort.get_untracked();
        self.db_query_text.set({
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            e_db::browse_sql(&engine, &table, by, DB_PAGE, page * DB_PAGE)
        });
        self.db_result_loading.set(true);
        self.db_result_error.set(None);
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<e_db::QueryResult, String>| state.db_apply_result(res)
        });
        std::thread::spawn(move || {
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            let sql = e_db::browse_sql(&engine, &table, by, DB_PAGE, page * DB_PAGE);
            send(e_db::query(&conn, &sql, DB_PAGE));
        });
    }

    /// Toggle the sort on a column (asc → desc → off) and reload.
    pub fn db_sort_by(&self, col: String) {
        let next = match self.db_sort.get_untracked() {
            Some((c, true)) if c == col => Some((col, false)),
            Some((c, false)) if c == col => None,
            _ => Some((col, true)),
        };
        self.db_sort.set(next);
        self.db_page.set(0);
        self.db_reload_table();
    }

    /// Move to the next/previous page when browsing a table.
    pub fn db_page_by(&self, delta: i64) {
        let cur = self.db_page.get_untracked() as i64;
        let next = (cur + delta).max(0) as usize;
        if next == self.db_page.get_untracked() {
            return;
        }
        // Don't page past the end (a short page means we're at the last one).
        if delta > 0 {
            let len = self
                .db_result
                .with_untracked(|r| r.as_ref().map(|r| r.rows.len()).unwrap_or(0));
            if len < DB_PAGE {
                return;
            }
        }
        self.db_page.set(next);
        self.db_reload_table();
    }

    pub fn db_set_subview(&self, view: &str) {
        self.db_subview.set(view.to_string());
    }

    fn db_load_columns(&self, entry: DbEntry, table: String) {
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |cols: Vec<e_db::ColumnInfo>| state.db_columns.set(cols)
        });
        std::thread::spawn(move || {
            send(e_db::columns(&conn, &table).unwrap_or_default());
        });
    }

    /// Test the current add-form connection without saving it.
    pub fn db_test_connection(&self) {
        let cfg = self.db_form.get_untracked().to_config();
        self.db_test_state.set("testing".into());
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<(), String>| {
                state.db_test_state.set(match res {
                    Ok(()) => "ok".into(),
                    Err(e) => e,
                });
            }
        });
        std::thread::spawn(move || {
            send(e_db::test(&cfg));
        });
    }

    /// Begin editing an existing connection (load it into the form).
    pub fn db_start_edit(&self, entry: DbEntry) {
        let c = &entry.config;
        self.db_form.set(DbForm {
            engine: c.engine.clone(),
            host: c.host.clone(),
            port: if c.port == 0 {
                String::new()
            } else {
                c.port.to_string()
            },
            database: c.database.clone(),
            username: c.username.clone(),
            password: c.password.clone(),
            path: c.path.clone(),
            group: c.group.clone(),
            use_ssh: c.use_ssh,
            ssh_host: c.ssh_host.clone(),
            ssh_port: if c.ssh_port == 0 {
                "22".into()
            } else {
                c.ssh_port.to_string()
            },
            ssh_user: c.ssh_user.clone(),
            ssh_auth: if c.ssh_auth.is_empty() {
                "key".into()
            } else {
                c.ssh_auth.clone()
            },
            ssh_password: c.ssh_password.clone(),
            ssh_key_path: c.ssh_key_path.clone(),
            ssh_passphrase: c.ssh_passphrase.clone(),
        });
        self.db_editing_key.set(Some(entry.key()));
        self.db_test_state.set(String::new());
        self.db_adding.set(true);
    }

    /// Save the add/edit form: either add a new connection or replace one.
    pub fn db_submit_form(&self) {
        let cfg = self.db_form.get_untracked().to_config();
        if let Some(old_key) = self.db_editing_key.get_untracked() {
            self.db_conns.update(|c| c.retain(|e| e.key() != old_key));
            self.db_editing_key.set(None);
        }
        self.db_form.set(DbForm::default());
        self.db_adding.set(false);
        self.db_test_state.set(String::new());
        self.db_add_config(cfg);
    }

    /// Export the current result grid to a CSV file.
    pub fn db_export_csv(&self) {
        let Some(result) = self.db_result.get_untracked() else {
            return;
        };
        if result.columns.is_empty() {
            return;
        }
        let opts = floem::file::FileDialogOptions::new()
            .title("Export results as CSV")
            .default_name("results.csv");
        floem::action::save_as(opts, move |info| {
            let Some(path) = info.and_then(|i| i.path.into_iter().next()) else {
                return;
            };
            let mut out = String::new();
            out.push_str(&result.columns.join(","));
            out.push('\n');
            for row in &result.rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| csv_escape(c.as_deref().unwrap_or("")))
                    .collect();
                out.push_str(&cells.join(","));
                out.push('\n');
            }
            let _ = std::fs::write(&path, out);
        });
    }

    /// Open a blank query editor for a connection.
    pub fn db_new_query(&self, entry: DbEntry) {
        if entry.conn.get_untracked().is_none() {
            self.db_connect(entry.clone());
        }
        self.db_result_key.set(Some(entry.key()));
        self.db_result_title
            .set(format!("{} · query", entry.config.display_name()));
        self.db_result.set(None);
        self.db_result_error.set(None);
        if self.db_query_text.with_untracked(|q| q.trim().is_empty()) {
            self.db_query_text.set("SELECT 1".into());
        }
        self.db_result_open.set(true);
    }

    /// Run the SQL currently in the query editor against the bound connection.
    pub fn db_run_query(&self) {
        let Some(key) = self.db_result_key.get_untracked() else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            self.db_result_error.set(Some("Not connected".into()));
            return;
        };
        let sql = self.db_query_text.get_untracked();
        if sql.trim().is_empty() {
            return;
        }
        self.db_result_loading.set(true);
        self.db_result_error.set(None);
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<e_db::QueryResult, String>| state.db_apply_result(res)
        });
        std::thread::spawn(move || {
            send(e_db::query(&conn, &sql, e_db::MAX_ROWS));
        });
    }

    fn db_apply_result(&self, res: Result<e_db::QueryResult, String>) {
        self.db_result_loading.set(false);
        match res {
            Ok(r) => {
                self.db_result_error.set(None);
                self.db_result.set(Some(r));
            }
            Err(e) => {
                self.db_result.set(None);
                self.db_result_error.set(Some(e));
            }
        }
    }

    pub fn close_db_result(&self) {
        self.db_result_open.set(false);
        self.db_edit.set(None);
    }

    /// The user approved an agent-proposed query: run it and reply.
    pub fn db_consent_allow(&self) {
        let Some(c) = self.db_consent.get_untracked() else {
            return;
        };
        self.db_consent.set(None);
        std::thread::spawn(move || {
            let resp = match e_db::query(&c.conn, &c.sql, e_db::MAX_ROWS) {
                Ok(r) => serde_json::json!({
                    "ok": true,
                    "columns": r.columns,
                    "rows": r.rows,
                    "rows_affected": r.rows_affected,
                    "elapsed_ms": r.elapsed_ms,
                    "truncated": r.truncated,
                }),
                Err(e) => serde_json::json!({"ok": false, "error": e}),
            };
            let _ = c.reply.send(resp);
        });
    }

    pub fn toggle_tinker(&self) {
        self.tinker_open.update(|o| *o = !*o);
    }

    pub fn toggle_laravel_map(&self) {
        if !self.map_open.get_untracked() && self.laravel.get_untracked().is_none() {
            self.load_laravel();
        }
        self.map_open.update(|o| *o = !*o);
    }

    /// Run the current editor selection in Tinker (or just open the panel).
    pub fn run_tinker_selection(&self) {
        if let Some(buf) = self.active_buffer() {
            if let Some(editor) = buf.editor.get_untracked() {
                let cursor = editor.cursor.get_untracked();
                if let CursorMode::Insert(sel) = &cursor.mode {
                    if let Some(r) = sel.regions().iter().find(|r| r.min() != r.max()) {
                        let text = buf.doc.text().to_string();
                        let code =
                            text[r.min().min(text.len())..r.max().min(text.len())].to_string();
                        self.run_tinker(code);
                        return;
                    }
                }
            }
        }
        self.tinker_open.set(true);
    }

    /// Run PHP through `php artisan tinker` in the project root, capturing output.
    pub fn run_tinker(&self, code: String) {
        let root = self.root.get_untracked();
        if !root.join("artisan").is_file() {
            self.tinker_output
                .set("Not a Laravel project (no artisan).".into());
            return;
        }
        self.tinker_open.set(true);
        self.tinker_running.set(true);
        self.tinker_output.set("Running…".into());
        let out_sig = self.tinker_output;
        let running = self.tinker_running;
        let send = create_ext_action(self.cx, move |text: String| {
            out_sig.set(text);
            running.set(false);
        });
        std::thread::spawn(move || {
            let tmp = std::env::temp_dir().join(format!("e-tinker-{}.php", std::process::id()));
            let _ = std::fs::write(&tmp, code);
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let cmd = format!(
                "php -d error_reporting=0 -d display_errors=0 artisan tinker < {}",
                tmp.display()
            );
            let text = match std::process::Command::new(shell)
                .arg("-ilc")
                .arg(&cmd)
                .current_dir(&root)
                .output()
            {
                Ok(o) => {
                    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
                    let err = String::from_utf8_lossy(&o.stderr);
                    if !err.trim().is_empty() {
                        s.push_str(&err);
                    }
                    if s.trim().is_empty() {
                        s = "(no output)".to_string();
                    }
                    s
                }
                Err(e) => format!("failed to run tinker: {e}"),
            };
            let _ = std::fs::remove_file(&tmp);
            send(text);
        });
    }

    // ---- Agent socket: audit log, marker, edit review -----------------

    /// Append an entry to the agent audit timeline (capped).
    pub fn agent_log_push(&self, method: &str, summary: String) {
        let entry = (now_hms(), method.to_string(), summary);
        self.agent_log.update(|v| {
            v.push(entry);
            let len = v.len();
            if len > 500 {
                v.drain(0..len - 500);
            }
        });
    }

    pub fn toggle_agent_log(&self) {
        self.agent_log_open.update(|o| *o = !*o);
    }

    /// Record where the agent is currently looking (a ghost marker).
    pub fn set_agent_mark(&self, path: PathBuf, line: usize) {
        self.agent_mark.set(Some((path, line)));
    }

    pub fn jump_to_agent_mark(&self) {
        if let Some((path, line)) = self.agent_mark.get_untracked() {
            self.jump_to(&path_to_uri(&path), line, 0);
        }
    }

    /// The agent proposed replacing a file's contents; diff it and open a
    /// hunk-by-hunk review. `reply` is answered when the user applies/cancels.
    pub fn agent_propose_edit(
        &self,
        path: PathBuf,
        new_content: String,
        reply: std::sync::mpsc::Sender<serde_json::Value>,
    ) {
        let old = self
            .buffers
            .with_untracked(|bs| {
                bs.iter()
                    .find(|b| b.file.path.as_deref() == Some(path.as_path()))
                    .map(|b| b.doc.text().to_string())
            })
            .or_else(|| buffer::read_with_encoding(&path).map(|(s, _)| s).ok())
            .unwrap_or_default();
        let segs: Vec<EditSeg> = e_core::diff::edit_segments(&old, &new_content)
            .into_iter()
            .map(|d| {
                if d.equal {
                    EditSeg::Equal(d.old)
                } else {
                    EditSeg::Change {
                        old: d.old,
                        new: d.new,
                        accepted: self.cx.create_rw_signal(true),
                    }
                }
            })
            .collect();
        if !segs.iter().any(|s| matches!(s, EditSeg::Change { .. })) {
            let _ = reply.send(serde_json::json!({"ok": true, "applied": 0, "note": "no changes"}));
            return;
        }
        self.agent_edit.set(Some(AgentEdit { path, segs, reply }));
    }

    /// Apply the accepted hunks of the current proposal.
    pub fn agent_edit_apply(&self) {
        let Some(edit) = self.agent_edit.get_untracked() else {
            return;
        };
        self.agent_edit.set(None);
        let mut out = String::new();
        let mut applied = 0u32;
        for seg in &edit.segs {
            match seg {
                EditSeg::Equal(t) => out.push_str(t),
                EditSeg::Change { old, new, accepted } => {
                    if accepted.get_untracked() {
                        out.push_str(new);
                        applied += 1;
                    } else {
                        out.push_str(old);
                    }
                }
            }
        }
        // Apply to the open buffer (so undo works) or write to disk.
        let open = self.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(edit.path.as_path()))
                .map(|b| (b.doc.clone(), b.dirty))
        });
        if let Some((doc, dirty)) = open {
            let len = doc.text().len();
            let mut it = std::iter::once((Selection::region(0, len), out.as_str()));
            doc.edit(&mut it, EditType::InsertChars);
            dirty.set(true);
        } else {
            let _ = buffer::write(&edit.path, &out);
        }
        let _ = edit
            .reply
            .send(serde_json::json!({"ok": true, "applied": applied}));
        self.agent_log_push(
            "propose_edit",
            format!("applied {applied} hunk(s) to {}", edit.path.display()),
        );
        // In the autonomous TDD loop, a fix triggers another test run.
        if self.tdd_loop.get_untracked() && applied > 0 {
            self.run_tests();
        }
    }

    // ---- Semantic search -----------------------------------------------

    pub fn toggle_semantic_search(&self) {
        let open = !self.sem_open.get_untracked();
        self.sem_open.set(open);
        if open && self.sem_index.get_untracked().borrow().is_empty() {
            self.build_semantic_index();
        }
    }

    /// Build the project index in the background (chunks + embeddings if Ollama
    /// is available, otherwise a lexical index).
    pub fn build_semantic_index(&self) {
        let roots = self.roots.get_untracked();
        let status = self.sem_status;
        let idx_sig = self.sem_index;
        status.set("Indexing project…".to_string());
        let cx = self.cx;
        let send = create_ext_action(cx, move |index: crate::semantic::SemIndex| {
            let n = index.chunks.len();
            let mode = if index.semantic() {
                "semantic"
            } else {
                "lexical"
            };
            status.set(format!("Ready · {n} chunks · {mode}"));
            idx_sig.set(Rc::new(RefCell::new(index)));
        });
        std::thread::spawn(move || {
            let chunks = crate::semantic::build_chunks(&roots);
            let mut embeds = Vec::new();
            let mut model = String::new();
            if crate::semantic::ollama_up() {
                let m = crate::semantic::embed_model();
                let mut ok = true;
                for batch in chunks.chunks(64) {
                    let texts: Vec<String> = batch.iter().map(|c| c.text.clone()).collect();
                    match crate::semantic::embed_batch(&m, &texts) {
                        Some(mut v) => embeds.append(&mut v),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && embeds.len() == chunks.len() {
                    model = m;
                } else {
                    embeds.clear();
                }
            }
            send(crate::semantic::SemIndex {
                chunks,
                embeds,
                model,
            });
        });
    }

    /// Run the current semantic query against the index.
    pub fn run_semantic_search(&self) {
        let query = self.sem_query.get_untracked();
        if query.trim().is_empty() {
            return;
        }
        let index_rc = self.sem_index.get_untracked();
        if index_rc.borrow().is_empty() {
            self.sem_status
                .set("Building index — try again shortly…".to_string());
            self.build_semantic_index();
            return;
        }
        let results = self.sem_results;
        if index_rc.borrow().semantic() {
            // Embed the query off-thread, then rank on the UI thread.
            let model = index_rc.borrow().model.clone();
            let idx_sig = self.sem_index;
            let q = query.clone();
            let send = create_ext_action(self.cx, move |qvec: Option<Vec<f32>>| {
                let Some(qvec) = qvec else {
                    return;
                };
                let index = idx_sig.get_untracked();
                let index = index.borrow();
                let scores: Vec<f32> = index
                    .embeds
                    .iter()
                    .map(|e| crate::semantic::cosine(&qvec, e))
                    .collect();
                results.set(crate::semantic::top_hits(&index, &scores, 40));
            });
            std::thread::spawn(move || {
                send(crate::semantic::embed_one(&model, &q));
            });
        } else {
            let index = index_rc.borrow();
            let scores = crate::semantic::lexical_scores(&index.chunks, &query);
            results.set(crate::semantic::top_hits(&index, &scores, 40));
        }
    }

    pub fn open_semantic_hit(&self, hit: &crate::semantic::SemHit) {
        let uri = format!("file://{}", hit.path.display());
        self.jump_to(&uri, hit.line.saturating_sub(1), 0);
        self.sem_open.set(false);
    }

    // ---- Undo tree -----------------------------------------------------

    pub fn toggle_undo_tree(&self) {
        self.undo_open.update(|o| *o = !*o);
    }

    /// Replace the active buffer's whole text with `text` from the undo tree,
    /// suppressing re-recording of our own edit.
    fn undo_apply(&self, buf: &Buffer, text: &str) {
        buf.undo_nav.set(true);
        let len = buf.doc.text().len();
        let mut it = std::iter::once((Selection::region(0, len), text));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.undo_nav.set(false);
        buf.dirty.set(true);
        buf.doc.cache_rev().update(|r| *r += 1);
        self.undo_rev.update(|r| *r += 1);
        if let Some(p) = &buf.file.path {
            buf.undo.borrow().save(&undo_store_path(p));
        }
    }

    pub fn undo_tree_undo(&self) {
        if let Some(buf) = self.active_buffer() {
            let t = buf.undo.borrow_mut().undo();
            if let Some(text) = t {
                self.undo_apply(&buf, &text);
            }
        }
    }

    pub fn undo_tree_redo(&self) {
        if let Some(buf) = self.active_buffer() {
            let t = buf.undo.borrow_mut().redo();
            if let Some(text) = t {
                self.undo_apply(&buf, &text);
            }
        }
    }

    pub fn undo_tree_goto(&self, id: usize) {
        if let Some(buf) = self.active_buffer() {
            let t = buf.undo.borrow_mut().goto(id);
            if let Some(text) = t {
                self.undo_apply(&buf, &text);
            }
        }
    }

    // ---- Validation ----------------------------------------------------

    /// Generate `'field' => 'rules'` from the live schema and insert them at the
    /// cursor (table inferred from the active file's resource name).
    pub fn generate_validation_rules(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(name) = crate::relatedfiles::resource_name(&path) else {
            return;
        };
        let table = crate::eloquent::model_table(&root, &name);
        let cols = self
            .db_schema_cache
            .with_untracked(|m| m.get(&table).cloned());
        let Some(cols) = cols.filter(|c| !c.is_empty()) else {
            Self::notify(&format!("No live schema for table `{table}`"));
            return;
        };
        let text = crate::validation::generate_rules(&table, &cols);
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let mut it = std::iter::once((Selection::region(offset, offset), text.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.dirty.set(true);
        Self::notify(&format!("Inserted validation rules for `{table}`"));
    }

    // ---- Related files -------------------------------------------------

    /// Show the files related to the active file's resource (model, migration,
    /// factory, controller, test, …).
    pub fn show_related_files(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(name) = crate::relatedfiles::resource_name(&path) else {
            return;
        };
        let mut items = crate::relatedfiles::related(&root, &name);
        items.retain(|(_, p)| *p != path);
        if items.is_empty() {
            Self::notify("No related files found");
            return;
        }
        self.related_items.set(items);
        self.related_open.set(true);
    }

    pub fn open_related(&self, path: PathBuf) {
        self.related_open.set(false);
        self.open_path(path);
    }

    // ---- Inertia props contract ----------------------------------------

    /// Reconcile the active page component with the controller that renders it.
    pub fn compute_contract(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(page) = crate::contract::page_name_of(&root, &path) else {
            Self::notify("Open an Inertia page component first");
            return;
        };
        let src = buf.doc.text().to_string();
        let schema = self.db_schema_cache.get_untracked();
        let shared = crate::inertia::shared_props(&root);
        let routes: Vec<(String, String)> = self
            .laravel
            .get_untracked()
            .map(|d| {
                d.routes
                    .iter()
                    .map(|r| (r.name.clone(), r.action.clone()))
                    .collect()
            })
            .unwrap_or_default();
        self.contract_open.set(true);
        self.contract.set(None);
        let sig = self.contract;
        let send = create_ext_action(self.cx, move |c: Option<crate::contract::Contract>| {
            sig.set(c)
        });
        std::thread::spawn(move || {
            send(crate::contract::build(
                &root, &page, &src, &schema, &shared, &routes,
            ));
        });
    }

    /// Write TypeScript interfaces for the current contract and open them.
    pub fn generate_contract_ts(&self) {
        let Some(c) = self.contract.get_untracked() else {
            return;
        };
        let schema = self.db_schema_cache.get_untracked();
        let ts = crate::contract::generate_ts(&c, &schema);
        let root = self.root.get_untracked();
        let dir = root.join("resources/js/types");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{}.d.ts", c.page.replace('/', "")));
        if std::fs::write(&file, ts).is_ok() {
            self.contract_open.set(false);
            self.open_path(file);
        }
    }

    // ---- Eloquent relationship graph -----------------------------------

    pub fn toggle_relations(&self) {
        let open = !self.rel_open.get_untracked();
        self.rel_open.set(open);
        if open {
            self.compute_relations();
        }
    }

    /// Parse model relationships and cross-check them against the live schema's
    /// foreign keys, in the background.
    pub fn compute_relations(&self) {
        let root = self.root.get_untracked();
        let sig = self.rel_graph;
        let send = create_ext_action(self.cx, move |g: Vec<crate::relations::ModelNode>| {
            sig.set(g)
        });
        std::thread::spawn(move || {
            let fks = e_db::from_env(&root)
                .and_then(|cfg| e_db::connect(&cfg).ok())
                .and_then(|conn| e_db::foreign_keys(&conn).ok())
                .unwrap_or_default();
            send(crate::relations::build_graph(&root, &fks));
        });
    }

    // ---- Schema diff ---------------------------------------------------

    /// Diff the project's migrations against the live database schema.
    pub fn compute_schema_diff(&self) {
        let root = self.root.get_untracked();
        self.schema_diff_open.set(true);
        let sig = self.schema_diff;
        let send = create_ext_action(self.cx, move |rows: Vec<crate::schema_diff::DiffRow>| {
            sig.set(rows)
        });
        std::thread::spawn(move || {
            let expected = crate::schema_diff::parse_migrations(&root.join("database/migrations"));
            let mut actual: std::collections::HashMap<String, std::collections::HashSet<String>> =
                std::collections::HashMap::new();
            if let Some(cfg) = e_db::from_env(&root) {
                if let Ok(conn) = e_db::connect(&cfg) {
                    if let Ok(tables) = e_db::tables(&conn) {
                        for t in tables {
                            if let Ok(cols) = e_db::columns(&conn, &t) {
                                actual.insert(t, cols.into_iter().map(|c| c.name).collect());
                            }
                        }
                    }
                }
            }
            send(crate::schema_diff::diff(&expected, &actual));
        });
    }

    // ---- Laravel log tail ----------------------------------------------

    pub fn toggle_laravel_log(&self) {
        let open = !self.log_open.get_untracked();
        self.log_open.set(open);
        if open {
            self.refresh_laravel_log();
        }
    }

    /// Read the tail of the project's Laravel log (off the UI thread).
    pub fn refresh_laravel_log(&self) {
        let root = self.root.get_untracked();
        let sig = self.log_lines;
        let send = create_ext_action(self.cx, move |lines: Vec<String>| sig.set(lines));
        std::thread::spawn(move || {
            let lines = find_laravel_log(&root)
                .map(|p| tail_lines(&p, 64 * 1024, 600))
                .unwrap_or_default();
            send(lines);
        });
    }

    /// Send the recent log tail to the agent for diagnosis.
    pub fn log_fix_with_agent(&self) {
        let tail: String = self.log_lines.with_untracked(|l| {
            l.iter()
                .rev()
                .take(60)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        });
        if !tail.trim().is_empty() {
            self.send_to_agent(&format!(
                "Diagnose and fix this from the Laravel log. Use propose_edit for changes.\n{tail}"
            ));
        }
    }

    // ---- Request replay ------------------------------------------------

    pub fn close_request(&self) {
        self.req_open.set(false);
    }

    /// The app's base URL (the `app_url` setting, or the Grove `*.test` default).
    pub fn app_base(&self) -> String {
        let s = self.settings.get_untracked().app_url;
        if s.trim().is_empty() {
            let name = self
                .root
                .get_untracked()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "app".into());
            format!("https://{name}.test")
        } else {
            s.trim().trim_end_matches('/').to_string()
        }
    }

    // ---- Runtime insight -----------------------------------------------

    pub fn toggle_runtime(&self) {
        let open = !self.runtime_open.get_untracked();
        self.runtime_open.set(open);
        if open {
            self.poll_runtime();
        }
    }

    pub fn clear_runtime(&self) {
        self.runtime_reqs.set(Vec::new());
    }

    /// Poll Clockwork for the latest request and prepend it if it's new.
    /// Called on the idle tick while the Runtime panel is open.
    pub fn poll_runtime(&self) {
        if self.runtime_polling.get_untracked() {
            return;
        }
        self.runtime_polling.set(true);
        let base = self.app_base();
        let reqs = self.runtime_reqs;
        let polling = self.runtime_polling;
        let send = create_ext_action(self.cx, move |req: Option<RuntimeReq>| {
            polling.set(false);
            if let Some(req) = req {
                reqs.update(|list| {
                    if !list.iter().any(|r| r.id == req.id) {
                        list.insert(0, req);
                        list.truncate(50);
                    }
                });
            }
        });
        std::thread::spawn(move || {
            let out = std::process::Command::new("curl")
                .args(["-sk", "--max-time", "8"])
                .arg(format!("{base}/__clockwork/latest"))
                .output();
            let req = out.ok().and_then(|o| {
                serde_json::from_slice::<serde_json::Value>(&o.stdout)
                    .ok()
                    .and_then(|v| parse_clockwork_latest(&v))
            });
            send(req);
        });
    }

    /// Send a captured request to the agent for analysis.
    pub fn runtime_explain(&self, id: &str) {
        let req = self
            .runtime_reqs
            .with_untracked(|list| list.iter().find(|r| r.id == id).cloned());
        if let Some(r) = req {
            self.send_to_agent(&format!(
                "Analyze this request captured from the running app. {} {} responded {} in {:.0}ms, \
                 running {} SQL queries ({} cache hits, {} misses, {} mails, {} events). \
                 Point out N+1 problems, slow queries, and anything to improve.",
                r.method,
                r.uri,
                r.status,
                r.duration_ms,
                r.queries.len(),
                r.cache_hits,
                r.cache_misses,
                r.mails.len(),
                r.events
            ));
        }
    }

    /// Generate a Pest feature test from the last replayed request (URL, status,
    /// and key assertions derived from the actual response), open it, and hook
    /// it into the test-runner / TDD loop.
    pub fn generate_pest_test(&self) {
        let url = self.req_url.get_untracked();
        let status = self.req_status.get_untracked().unwrap_or(200);
        let body = self.req_body.get_untracked();
        let root = self.root.get_untracked();
        let path = url_path(&url);
        let name = pest_test_name(&path);
        let assertions = pest_assertions(status, &body);
        let content = format!(
            "<?php\n\nit('GET {path} responds {status}', function () {{\n    $response = $this->get('{path}');\n\n{assertions}}});\n"
        );
        let dir = root.join("tests").join("Feature");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{name}ReplayTest.php"));
        if std::fs::write(&file, &content).is_ok() {
            self.close_request();
            self.open_path(file.clone());
            self.agent_log_push("pest", format!("generated {}", file.display()));
        }
    }

    /// Replay an HTTP request against the app for a route `uri`, showing the
    /// response and (via Clockwork, if installed) the SQL queries it ran.
    pub fn send_request(&self, uri: &str) {
        let root = self.root.get_untracked();
        let base = {
            let s = self.settings.get_untracked().app_url;
            if s.trim().is_empty() {
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "app".into());
                format!("https://{name}.test")
            } else {
                s.trim().trim_end_matches('/').to_string()
            }
        };
        let path = substitute_route_params(uri);
        let url = format!("{}/{}", base, path.trim_start_matches('/'));

        self.req_open.set(true);
        self.req_running.set(true);
        self.req_error.set(None);
        self.req_url.set(url.clone());
        self.req_status.set(None);
        self.req_body.set(String::new());
        self.req_queries.set(Vec::new());
        self.req_inertia.set(None);

        let state = *self;
        let send = create_ext_action(self.cx, move |r: RequestResult| {
            state.req_running.set(false);
            state.req_status.set(r.status);
            state.req_time.set(r.time);
            state.req_body.set(r.body);
            state.req_queries.set(r.queries);
            state.req_error.set(r.error);
            state.req_inertia.set(r.inertia);
        });
        std::thread::spawn(move || {
            send(do_http_request(&base, &url));
        });
    }

    // ---- Autonomous TDD loop ------------------------------------------

    pub fn toggle_tdd(&self) {
        self.tdd_open.update(|o| *o = !*o);
    }

    /// Run the project's test suite, parse pass/fail, and (when the loop is
    /// active) ask the agent to fix failures.
    pub fn run_tests(&self) {
        let root = self.root.get_untracked();
        let Some(cmd) = crate::tasks::test_command(&root) else {
            self.tdd_output
                .set("No test command detected for this project.".into());
            self.tdd_status.set(TddStatus::Failed);
            self.tdd_open.set(true);
            return;
        };
        // Safety cap so an unproductive loop can't run forever.
        if self.tdd_loop.get_untracked() && self.tdd_iteration.get_untracked() >= 15 {
            self.tdd_loop.set(false);
            self.tdd_output
                .update(|o| o.push_str("\n\n[stopped: reached 15 iterations]"));
            return;
        }
        self.tdd_open.set(true);
        self.tdd_status.set(TddStatus::Running);
        let state = *self;
        let send = create_ext_action(self.cx, move |(code, text): (i32, String)| {
            state.tdd_iteration.update(|i| *i += 1);
            let passed = code == 0;
            state.tdd_status.set(if passed {
                TddStatus::Passed
            } else {
                TddStatus::Failed
            });
            state.tdd_output.set(text.clone());
            if passed {
                state.tdd_loop.set(false);
            } else if state.tdd_loop.get_untracked() {
                let tail: String = {
                    let n = text.len().saturating_sub(4000);
                    text[n..].to_string()
                };
                state.send_to_agent(&format!(
                    "The test suite is failing. Fix the code so the tests pass. \
                     Use propose_edit over $E_EDITOR_SOCK for your changes so I can review them. \
                     Failing output:\n{tail}"
                ));
            }
        });
        std::thread::spawn(move || {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let (code, text) = match std::process::Command::new(shell)
                .arg("-ilc")
                .arg(&cmd)
                .current_dir(&root)
                .output()
            {
                Ok(o) => {
                    let code = o.status.code().unwrap_or(-1);
                    let mut t = String::from_utf8_lossy(&o.stdout).into_owned();
                    let e = String::from_utf8_lossy(&o.stderr);
                    if !e.trim().is_empty() {
                        t.push_str(&e);
                    }
                    (code, t)
                }
                Err(e) => (-1, format!("failed to run tests: {e}")),
            };
            send((code, text));
        });
    }

    /// Start the autonomous "fix to green" loop.
    pub fn tdd_fix_to_green(&self) {
        self.tdd_iteration.set(0);
        self.tdd_loop.set(true);
        self.run_tests();
    }

    pub fn tdd_stop(&self) {
        self.tdd_loop.set(false);
    }

    pub fn agent_edit_cancel(&self) {
        if let Some(edit) = self.agent_edit.get_untracked() {
            self.agent_edit.set(None);
            let _ = edit
                .reply
                .send(serde_json::json!({"ok": true, "applied": 0, "cancelled": true}));
        }
    }

    /// The user rejected an agent-proposed query.
    pub fn db_consent_deny(&self) {
        if let Some(c) = self.db_consent.get_untracked() {
            self.db_consent.set(None);
            let _ = c
                .reply
                .send(serde_json::json!({"ok": false, "error": "denied by user"}));
        }
    }

    /// Whether the current results grid supports inline editing (a browsed table
    /// in data view, with a known primary key).
    pub fn db_editable(&self) -> bool {
        self.db_result_table.get_untracked().is_some()
            && self.db_subview.get_untracked() == "data"
            && self
                .db_columns
                .with_untracked(|c| c.iter().any(|c| c.key == "PRI"))
    }

    /// Begin editing the cell at `(row, col)`.
    pub fn db_begin_edit(&self, row: usize, col: usize) {
        if !self.db_editable() {
            return;
        }
        let Some(result) = self.db_result.get_untracked() else {
            return;
        };
        let Some(cell) = result.rows.get(row).and_then(|r| r.get(col)) else {
            return;
        };
        let column = result.columns.get(col).cloned().unwrap_or_default();
        self.db_edit_null.set(cell.is_none());
        self.db_edit_value.set(cell.clone().unwrap_or_default());
        self.db_edit.set(Some((row, col, column)));
    }

    pub fn db_cancel_edit(&self) {
        self.db_edit.set(None);
    }

    /// Write the edited cell back to the database.
    pub fn db_commit_edit(&self) {
        let Some((row, col, column)) = self.db_edit.get_untracked() else {
            return;
        };
        let (Some(key), Some(table)) = (
            self.db_result_key.get_untracked(),
            self.db_result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let Some(result) = self.db_result.get_untracked() else {
            return;
        };
        // Build the primary-key conditions from the row's current values.
        let pk_names: Vec<String> = self.db_columns.with_untracked(|cols| {
            cols.iter()
                .filter(|c| c.key == "PRI")
                .map(|c| c.name.clone())
                .collect()
        });
        let mut pk: Vec<(String, Option<String>)> = Vec::new();
        for name in &pk_names {
            if let Some(idx) = result.columns.iter().position(|c| c == name) {
                pk.push((name.clone(), result.rows[row].get(idx).cloned().flatten()));
            }
        }
        let engine = entry.config.engine.clone();
        let is_null = self.db_edit_null.get_untracked();
        let value = self.db_edit_value.get_untracked();
        let set_val = if is_null { None } else { Some(value.clone()) };

        let state = *self;
        let send = create_ext_action(self.cx, move |res: Result<u64, String>| match res {
            Ok(_) => {
                // Reflect the change in the in-memory grid.
                state.db_result.update(|r| {
                    if let Some(r) = r {
                        if let Some(cell) = r.rows.get_mut(row).and_then(|row| row.get_mut(col)) {
                            *cell = if is_null { None } else { Some(value.clone()) };
                        }
                    }
                });
                state.db_edit.set(None);
            }
            Err(e) => state.db_result_error.set(Some(e)),
        });
        std::thread::spawn(move || {
            let pk_ref = pk;
            send(e_db::update_cell(
                &conn,
                &engine,
                &table,
                &column,
                set_val.as_deref(),
                &pk_ref,
            ));
        });
    }

    /// Offer Laravel completions if the cursor is inside a helper string.
    /// Returns true when the context was handled (so we skip the LSP).
    fn try_laravel_completion(&self, buffer_id: u64) -> bool {
        let Some(data) = self.laravel.get() else {
            return false;
        };
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let text = buf.doc.text().to_string();
        let upto = offset.min(text.len());
        let line_start = text[..upto].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_before = &text[line_start..upto];

        let Some((helper, prefix)) = laravel::detect_context(line_before) else {
            return false;
        };

        let items = laravel::completions(&data, helper, &prefix);
        let start = offset - prefix.len();

        let (_, below) = editor.points_of_offset(start, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();

        let comp = self.completion;
        comp.anchor
            .set(Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0));
        comp.buffer_id.set(Some(buffer_id));
        comp.start_offset.set(start);
        if items.is_empty() {
            comp.open.set(false);
        } else {
            comp.items.set(items);
            comp.selected.set(0);
            comp.open.set(true);
        }
        true
    }

    /// Look up a running language server for `language` (does not start one).
    pub fn lsp_for_language(&self, language: Language) -> Option<Arc<LspClient>> {
        let spec = server_spec(language)?;
        self.lsp_clients.with(|m| m.get(spec.id).cloned())
    }

    /// The language server for the active buffer, if running.
    pub fn lsp_for_active(&self) -> Option<Arc<LspClient>> {
        self.lsp_for_language(self.active_buffer()?.file.language)
    }

    /// Start (or reuse) the language server for `language`.
    fn ensure_lsp(&self, language: Language) -> Option<Arc<LspClient>> {
        let spec = server_spec(language)?;
        if let Some(client) = self.lsp_clients.with(|m| m.get(spec.id).cloned()) {
            return Some(client);
        }
        if self.lsp_failed.with(|f| f.contains(spec.id)) {
            return None;
        }
        let tx = self.diag_tx.get();
        let handler: e_lsp::DiagnosticsHandler = Box::new(move |p| {
            let _ = tx.send(p);
        });
        let root = self.root.get();
        match LspClient::start(spec.program, spec.args, &root, handler) {
            Ok(client) => {
                eprintln!("e: started {} for {}", spec.id, root.display());
                self.lsp_clients.update(|m| {
                    m.insert(spec.id.to_string(), client.clone());
                });
                Some(client)
            }
            Err(e) => {
                eprintln!("e: could not start {} ({e:#})", spec.program);
                self.lsp_failed.update(|f| {
                    f.insert(spec.id.to_string());
                });
                None
            }
        }
    }

    /// Open a file by path. If it's already open, just focus it.
    pub fn open_path(&self, path: PathBuf) {
        let canon = path.canonicalize().unwrap_or(path);

        // Already open? Focus the existing tab.
        let existing = self.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(canon.as_path()))
                .map(|b| b.id)
        });
        if let Some(id) = existing {
            self.focused_active().set(Some(id));
            return;
        }

        let (content, encoding) = match buffer::read_with_encoding(&canon) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("e: open failed: {e:#}");
                return;
            }
        };

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let file = FileInfo::for_path(canon.clone());
        let language = file.language;
        let uri = file.path.as_ref().map(|p| path_to_uri(p));

        // Very large files skip tree-sitter highlighting (and other per-edit
        // work) to stay responsive.
        let large = content.len() > 1_000_000;
        let highlights: Highlights = Rc::new(RefCell::new(if large {
            Vec::new()
        } else {
            highlight_lines(language, &content)
        }));

        // Git change markers vs HEAD.
        let head_text = file.path.as_ref().and_then(|p| git::head_text(p));
        let line_count = content.split_inclusive('\n').count().max(1);
        let git_marks: GitMarks = Rc::new(RefCell::new(match &head_text {
            Some(h) => git::marks(h, &content, line_count),
            None => Vec::new(),
        }));

        let doc = Rc::new(TextDocument::new(self.cx, content.clone()));
        // Keep/auto-indent on newline (matches editor expectations).
        doc.auto_indent.set(true);
        let dirty = RwSignal::new(false);
        let version = RwSignal::new(1i64);

        // Branching undo tree, restored from disk when it still matches.
        let undo_path = file.path.as_ref().map(|p| undo_store_path(p));
        let undo = {
            let loaded = undo_path
                .as_ref()
                .filter(|_| !large)
                .and_then(|p| e_core::undotree::UndoTree::load(p));
            let t = match loaded {
                // Restore only if the tree still matches the file on disk.
                Some(mut t) if !t.is_empty() => {
                    if t.sync_to(&content) {
                        t
                    } else {
                        e_core::undotree::UndoTree::new(content.clone())
                    }
                }
                _ => e_core::undotree::UndoTree::new(content.clone()),
            };
            Rc::new(RefCell::new(t))
        };
        let undo_nav = Rc::new(std::cell::Cell::new(false));

        // Hand the document to the language server, if we have one.
        if let (Some(lang_id), Some(uri)) = (lsp_language_id(language), uri.as_ref()) {
            if let Some(client) = self.ensure_lsp(language) {
                client.did_open(uri, lang_id, 1, &content);
            }
        }

        // On every edit: mark dirty, re-highlight, invalidate the layout cache,
        // and notify the language server.
        {
            let doc = doc.clone();
            let highlights = highlights.clone();
            let git_marks = git_marks.clone();
            let head_text = head_text.clone();
            let app = *self;
            let uri = uri.clone();
            let undo = undo.clone();
            let undo_nav = undo_nav.clone();
            let undo_path = undo_path.clone();
            doc.clone().add_on_update(move |_| {
                dirty.set(true);
                app.last_edit.set(now_ms());
                let text = doc.text().to_string();
                if !undo_nav.get() {
                    let now = now_ms() as u64;
                    let mut t = undo.borrow_mut();
                    if t.record(&text, now, 700) {
                        app.undo_rev.update(|r| *r += 1);
                        if let Some(p) = &undo_path {
                            t.maybe_save(p, now);
                        }
                    }
                }
                if !large {
                    *highlights.borrow_mut() = highlight_lines(language, &text);
                    if let Some(head) = &head_text {
                        let lc = text.split_inclusive('\n').count().max(1);
                        *git_marks.borrow_mut() = git::marks(head, &text, lc);
                    }
                }
                doc.cache_rev().update(|r| *r += 1);

                if let (Some(uri), Some(client)) = (uri.as_ref(), app.lsp_for_language(language)) {
                    if lsp_language_id(language).is_some() {
                        let v = version.get() + 1;
                        version.set(v);
                        client.did_change_full(uri, v, &text);
                    }
                }
                // Trigger completion (LSP + snippets + Laravel helpers).
                app.autocomplete_after_edit(id);
            });
        }

        let disk_mtime = file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());

        let buf = Buffer {
            id,
            file,
            doc,
            dirty,
            highlights,
            diag_lines: Rc::new(RefCell::new(Vec::new())),
            git_marks,
            find_marks: Rc::new(RefCell::new(Vec::new())),
            bracket_marks: Rc::new(RefCell::new(Vec::new())),
            uri,
            editor: RwSignal::new(None),
            win_origin: RwSignal::new(Point::ZERO),
            pending_goto: RwSignal::new(None),
            disk_mtime: RwSignal::new(disk_mtime),
            disk_changed: RwSignal::new(false),
            blame: Rc::new(RefCell::new(Vec::new())),
            inlay_hints: RwSignal::new(Vec::new()),
            large,
            encoding: RwSignal::new(encoding),
            undo,
            undo_nav,
        };
        self.buffers.update(|bs| bs.push(buf));
        self.focused_active().set(Some(id));
        self.load_blame(id);
        self.request_inlay_hints(id);
    }

    /// Close a tab; focus a neighbour if it was active.
    /// Close a tab, prompting first if it has unsaved changes.
    pub fn close(&self, id: u64) {
        let dirty = self
            .buffers
            .with_untracked(|bs| {
                bs.iter()
                    .find(|b| b.id == id)
                    .map(|b| b.dirty.get_untracked())
            })
            .unwrap_or(false);
        if dirty {
            self.close_confirm.set(Some(id));
        } else {
            self.force_close(id);
        }
    }

    /// Save the pending buffer, then close it.
    pub fn confirm_close_save(&self) {
        if let Some(id) = self.close_confirm.get_untracked() {
            self.close_confirm.set(None);
            let prev = self.focused_active().get_untracked();
            self.focused_active().set(Some(id));
            self.save_active();
            self.focused_active().set(prev);
            self.force_close(id);
        }
    }

    /// Discard changes and close the pending buffer.
    pub fn confirm_close_discard(&self) {
        if let Some(id) = self.close_confirm.get_untracked() {
            self.close_confirm.set(None);
            self.force_close(id);
        }
    }

    pub fn cancel_close(&self) {
        self.close_confirm.set(None);
    }

    pub fn force_close(&self, id: u64) {
        let mut focus_next = None;
        let mut closed_uri = None;
        let mut closed_lang = None;
        self.buffers.update(|bs| {
            if let Some(pos) = bs.iter().position(|b| b.id == id) {
                closed_uri = bs[pos].uri.clone();
                closed_lang = Some(bs[pos].file.language);
                bs.remove(pos);
                if !bs.is_empty() {
                    let n = pos.min(bs.len() - 1);
                    focus_next = Some(bs[n].id);
                }
            }
        });
        if self.active.get_untracked() == Some(id) {
            self.active.set(focus_next);
        }
        if self.active2.get_untracked() == Some(id) {
            self.active2.set(focus_next);
        }
        if let (Some(uri), Some(lang)) = (closed_uri, closed_lang) {
            if let Some(client) = self.lsp_for_language(lang) {
                client.did_close(&uri);
            }
        }
    }

    pub fn active_buffer(&self) -> Option<Buffer> {
        let active = self.focused_active_id()?;
        self.buffers
            .with(|bs| bs.iter().find(|b| b.id == active).cloned())
    }

    /// Format the active buffer in place via the language server (PHP only).
    pub fn format_active(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if lsp_language_id(buf.file.language).is_none() {
            return;
        }
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return;
        };
        let edits = match client.formatting(&uri, 4, true) {
            Ok(e) if !e.is_empty() => e,
            _ => return,
        };
        // Resolve to offsets against the current text, then apply bottom-up so
        // earlier offsets stay valid.
        let mut offs: Vec<(usize, usize, String)> = edits
            .into_iter()
            .map(|e| {
                let s = editor.offset_of_line_col(
                    e.range.start.line as usize,
                    e.range.start.character as usize,
                );
                let en = editor
                    .offset_of_line_col(e.range.end.line as usize, e.range.end.character as usize);
                (s, en, e.new_text)
            })
            .collect();
        offs.sort_by_key(|b| std::cmp::Reverse(b.0));
        for (s, en, text) in offs {
            buf.doc
                .edit_single(Selection::region(s, en), &text, EditType::InsertChars);
        }
    }

    /// Strip trailing whitespace and ensure a final newline in the active buffer.
    fn trim_active(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let (edits, needs_newline) = trailing_trim_edits(&text);
        if edits.is_empty() && !needs_newline {
            return;
        }
        // Delete trailing whitespace bottom-up so offsets stay valid.
        for (s, e) in edits.into_iter().rev() {
            buf.doc
                .edit_single(Selection::region(s, e), "", EditType::Delete);
        }
        if needs_newline {
            let len = buf.doc.text().len();
            buf.doc
                .edit_single(Selection::region(len, len), "\n", EditType::InsertChars);
        }
    }

    /// Save the active buffer to disk (formatting / trimming first, if enabled).
    pub fn save_active(&self) {
        if self.settings.get_untracked().format_on_save {
            self.format_active();
        }
        if self.settings.get_untracked().trim_on_save {
            self.trim_active();
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.as_ref() else {
            self.save_active_as();
            return;
        };
        let text = buf.doc.text().to_string();
        match buffer::write_with_encoding(path, &text, &buf.encoding.get_untracked()) {
            Ok(()) => {
                buf.dirty.set(false);
                buf.disk_changed.set(false);
                Self::refresh_disk_mtime(&buf);
                self.fs_rev.update(|r| *r += 1);
                self.load_blame(buf.id);
                self.request_inlay_hints(buf.id);
                eprintln!("e: saved {}", path.display());
                if let (Some(uri), Some(client)) =
                    (buf.uri.as_ref(), self.lsp_for_language(buf.file.language))
                {
                    client.did_save(uri, &text);
                }
                self.request_outline();
            }
            Err(e) => eprintln!("e: save failed: {e:#}"),
        }
    }

    /// Rebuild a buffer's inline diagnostic spans and repaint it.
    pub fn apply_diagnostics_to_buffer(&self, uri: &str, diags: &[Diagnostic]) {
        let Some(buf) = self
            .buffers
            .with(|bs| bs.iter().find(|b| b.uri.as_deref() == Some(uri)).cloned())
        else {
            return;
        };
        let text = buf.doc.text().to_string();
        *buf.diag_lines.borrow_mut() = build_diag_lines(diags, &text);
        buf.doc.cache_rev().update(|r| *r += 1);
    }

    /// `(line, col, selection_len)` of the active editor's cursor (1-based).
    /// Reactive: reads the cursor signal, so call it inside a view closure.
    pub fn cursor_info(&self) -> Option<(usize, usize, usize)> {
        let buf = self.active_buffer()?;
        let editor = buf.editor.get()?;
        let cursor = editor.cursor.get();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);
        let sel_len = match &cursor.mode {
            CursorMode::Insert(sel) => sel.regions().iter().map(|r| r.max() - r.min()).sum(),
            _ => 0,
        };
        Some((line + 1, col + 1, sel_len))
    }

    /// `(errors, warnings)` for the active buffer.
    pub fn active_diagnostic_counts(&self) -> (usize, usize) {
        let Some(buf) = self.active_buffer() else {
            return (0, 0);
        };
        let Some(uri) = buf.uri.as_ref() else {
            return (0, 0);
        };
        self.diagnostics.with(|map| {
            let Some(diags) = map.get(uri) else {
                return (0, 0);
            };
            let mut errors = 0;
            let mut warnings = 0;
            for d in diags {
                match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => errors += 1,
                    Some(lsp_types::DiagnosticSeverity::WARNING) => warnings += 1,
                    _ => {}
                }
            }
            (errors, warnings)
        })
    }

    /// All non-empty diagnostics across open files, grouped and sorted.
    pub fn all_diagnostics(&self) -> Vec<(String, Vec<Diagnostic>)> {
        self.diagnostics.with(|map| {
            let mut groups: Vec<(String, Vec<Diagnostic>)> = map
                .iter()
                .filter(|(_, d)| !d.is_empty())
                .map(|(uri, d)| {
                    let mut dd = d.clone();
                    dd.sort_by_key(|x| x.range.start.line);
                    (uri.clone(), dd)
                })
                .collect();
            groups.sort_by(|a, b| a.0.cmp(&b.0));
            groups
        })
    }

    /// Total number of diagnostics across all open files.
    pub fn total_diagnostic_count(&self) -> usize {
        self.diagnostics.with(|m| m.values().map(|v| v.len()).sum())
    }

    /// A `file://` URI shown relative to the workspace root.
    pub fn rel_path(&self, uri: &str) -> String {
        rel_uri(uri, &self.root.get())
    }

    // ---- Completion & hover --------------------------------------------

    /// After an edit in a PHP buffer, decide whether to (re)trigger completion.
    pub fn autocomplete_after_edit(&self, buffer_id: u64) {
        // Laravel helper strings take priority over generic PHP completion.
        if self.try_laravel_completion(buffer_id) {
            return;
        }
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let before: Vec<char> = text[..offset.min(text.len())].chars().collect();
        let last = before.last().copied();
        let prev = before.get(before.len().wrapping_sub(2)).copied();

        // Signature help on call punctuation.
        match last {
            Some('(') | Some(',') => self.request_signature_help(buffer_id),
            Some(')') => self.close_signature(),
            _ => {}
        }

        let trigger = match last {
            Some(c) if is_word_char(c) => true,
            Some('>') => prev == Some('-'),
            Some(':') => prev == Some(':'),
            _ => false,
        };

        if trigger {
            self.request_completion(buffer_id);
        } else {
            self.close_completion();
        }
    }

    pub fn request_completion(&self, buffer_id: u64) {
        if self.try_laravel_completion(buffer_id) {
            return;
        }
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };

        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);

        let text = buf.doc.text().to_string();

        // Framework-aware completion (Flux UI, Livewire, Tailwind, Vue, Svelte)
        // takes priority and replaces the whole multi-segment token.
        let upto = offset.min(text.len());
        let line_start = text[..upto].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_before = &text[line_start..upto];

        // Livewire: `wire:model="…"` completes from the component's public props.
        if let Some(partial) = crate::livewire::wire_model_partial(line_before) {
            if let Some(items) = self.livewire_property_items(&buf, &partial) {
                let comp = self.completion;
                let fstart = offset.saturating_sub(partial.len());
                let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                let vp = editor.viewport.get_untracked();
                let win = buf.win_origin.get_untracked();
                let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                comp.buffer_id.set(Some(buffer_id));
                comp.start_offset.set(fstart);
                comp.anchor.set(anchor);
                comp.items.set(items);
                comp.selected.set(0);
                comp.open.set(true);
                return;
            }
        }

        // Inertia: `Inertia::render('…')` completes from existing page components.
        if let Some(partial) = crate::inertia::render_partial(line_before) {
            let pages = crate::inertia::list_pages(&self.root.get_untracked());
            let lower = partial.to_lowercase();
            let items: Vec<lsp_types::CompletionItem> = pages
                .into_iter()
                .filter(|p| lower.is_empty() || p.to_lowercase().contains(&lower))
                .map(|p| lsp_types::CompletionItem {
                    label: p.clone(),
                    insert_text: Some(p.clone()),
                    kind: Some(lsp_types::CompletionItemKind::FILE),
                    detail: Some("Inertia page".to_string()),
                    ..Default::default()
                })
                .collect();
            if !items.is_empty() {
                let comp = self.completion;
                let fstart = offset.saturating_sub(partial.len());
                let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                let vp = editor.viewport.get_untracked();
                let win = buf.win_origin.get_untracked();
                let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                comp.buffer_id.set(Some(buffer_id));
                comp.start_offset.set(fstart);
                comp.anchor.set(anchor);
                comp.items.set(items);
                comp.selected.set(0);
                comp.open.set(true);
                return;
            }
        }

        // Ziggy: `route('…')` in JS completes from the Laravel route table.
        if matches!(
            buf.file.language,
            Language::TypeScript | Language::JavaScript | Language::Vue | Language::Svelte
        ) {
            if let Some(partial) = crate::inertia::route_partial(line_before) {
                if let Some(data) = self.laravel.get_untracked() {
                    let lower = partial.to_lowercase();
                    let items: Vec<lsp_types::CompletionItem> = data
                        .routes
                        .iter()
                        .filter(|r| {
                            !r.name.is_empty()
                                && (lower.is_empty() || r.name.to_lowercase().contains(&lower))
                        })
                        .map(|r| lsp_types::CompletionItem {
                            label: r.name.clone(),
                            insert_text: Some(r.name.clone()),
                            kind: Some(lsp_types::CompletionItemKind::FUNCTION),
                            detail: Some(format!("{} {}", r.methods, r.uri)),
                            ..Default::default()
                        })
                        .collect();
                    if !items.is_empty() {
                        let comp = self.completion;
                        let fstart = offset.saturating_sub(partial.len());
                        let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                        let vp = editor.viewport.get_untracked();
                        let win = buf.win_origin.get_untracked();
                        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                        comp.buffer_id.set(Some(buffer_id));
                        comp.start_offset.set(fstart);
                        comp.anchor.set(anchor);
                        comp.items.set(items);
                        comp.selected.set(0);
                        comp.open.set(true);
                        return;
                    }
                }
            }
        }

        // Inertia shared props: `$page.props.…` from HandleInertiaRequests::share().
        if matches!(
            buf.file.language,
            Language::TypeScript | Language::JavaScript | Language::Vue | Language::Svelte
        ) {
            if let Some(partial) = crate::inertia::props_partial(line_before) {
                let lower = partial.to_lowercase();
                let items: Vec<lsp_types::CompletionItem> =
                    crate::inertia::shared_props(&self.root.get_untracked())
                        .into_iter()
                        .filter(|p| p.to_lowercase().starts_with(&lower))
                        .map(|p| lsp_types::CompletionItem {
                            label: p.clone(),
                            insert_text: Some(p.clone()),
                            kind: Some(lsp_types::CompletionItemKind::FIELD),
                            detail: Some("Inertia shared prop".to_string()),
                            ..Default::default()
                        })
                        .collect();
                if !items.is_empty() {
                    let comp = self.completion;
                    let fstart = offset.saturating_sub(partial.len());
                    let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                    let vp = editor.viewport.get_untracked();
                    let win = buf.win_origin.get_untracked();
                    let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                    comp.buffer_id.set(Some(buffer_id));
                    comp.start_offset.set(fstart);
                    comp.anchor.set(anchor);
                    comp.items.set(items);
                    comp.selected.set(0);
                    comp.open.set(true);
                    return;
                }
            }
        }

        // Laravel query builder: column names inside `where('…')`, `orderBy()`,
        // `select()`, … and relationship names inside `with()`, `whereHas()`.
        if matches!(buf.file.language, Language::Php | Language::Blade) {
            if let Some(ctx) = crate::querycomplete::context(line_before) {
                let root = self.root.get_untracked();
                if let Some(target) = crate::querycomplete::resolve_target(&text, offset, &root) {
                    let lower = ctx.partial.to_lowercase();
                    let items: Vec<lsp_types::CompletionItem> = if ctx.relation {
                        target
                            .model
                            .as_ref()
                            .map(|m| crate::relations::relation_names(&root, m))
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|r| lower.is_empty() || r.to_lowercase().starts_with(&lower))
                            .map(|r| lsp_types::CompletionItem {
                                label: r.clone(),
                                insert_text: Some(r),
                                kind: Some(lsp_types::CompletionItemKind::REFERENCE),
                                detail: Some("relation".to_string()),
                                ..Default::default()
                            })
                            .collect()
                    } else {
                        self.db_schema_cache.with_untracked(|schema| {
                            schema
                                .get(&target.table)
                                .map(|cols| {
                                    cols.iter()
                                        .filter(|c| {
                                            lower.is_empty()
                                                || c.name.to_lowercase().starts_with(&lower)
                                        })
                                        .map(|c| lsp_types::CompletionItem {
                                            label: c.name.clone(),
                                            insert_text: Some(c.name.clone()),
                                            kind: Some(lsp_types::CompletionItemKind::FIELD),
                                            detail: Some(format!(
                                                "{} · {}",
                                                c.data_type, target.table
                                            )),
                                            ..Default::default()
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                    };
                    if !items.is_empty() {
                        let comp = self.completion;
                        let fstart = offset.saturating_sub(ctx.partial.len());
                        let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                        let vp = editor.viewport.get_untracked();
                        let win = buf.win_origin.get_untracked();
                        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                        comp.buffer_id.set(Some(buffer_id));
                        comp.start_offset.set(fstart);
                        comp.anchor.set(anchor);
                        comp.items.set(items);
                        comp.selected.set(0);
                        comp.open.set(true);
                        return;
                    }
                }
            }
        }

        // Gate/policy abilities inside `can()`/`authorize()`/`@can`/`Gate::allows()`.
        if matches!(buf.file.language, Language::Php | Language::Blade) {
            if let Some(partial) =
                crate::inertia::call_string_partial(line_before, crate::policies::CALLS)
            {
                let root = self.root.get_untracked();
                let lower = partial.to_lowercase();
                let items: Vec<lsp_types::CompletionItem> = crate::policies::abilities(&root)
                    .into_iter()
                    .filter(|(n, _, _)| lower.is_empty() || n.to_lowercase().starts_with(&lower))
                    .map(|(n, _, _)| lsp_types::CompletionItem {
                        label: n.clone(),
                        insert_text: Some(n),
                        kind: Some(lsp_types::CompletionItemKind::VALUE),
                        detail: Some("ability".to_string()),
                        ..Default::default()
                    })
                    .collect();
                if !items.is_empty() {
                    let comp = self.completion;
                    let fstart = offset.saturating_sub(partial.len());
                    let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                    let vp = editor.viewport.get_untracked();
                    let win = buf.win_origin.get_untracked();
                    let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                    comp.buffer_id.set(Some(buffer_id));
                    comp.start_offset.set(fstart);
                    comp.anchor.set(anchor);
                    comp.items.set(items);
                    comp.selected.set(0);
                    comp.open.set(true);
                    return;
                }
            }
        }

        // Validation rule names inside `validate([…])` / FormRequest `rules()`.
        if matches!(buf.file.language, Language::Php | Language::Blade) {
            if let Some(partial) = crate::validation::rule_partial(line_before) {
                let items: Vec<lsp_types::CompletionItem> = crate::validation::rule_names(&partial)
                    .into_iter()
                    .map(|r| lsp_types::CompletionItem {
                        label: r.to_string(),
                        insert_text: Some(r.to_string()),
                        kind: Some(lsp_types::CompletionItemKind::KEYWORD),
                        detail: Some("validation rule".to_string()),
                        ..Default::default()
                    })
                    .collect();
                if !items.is_empty() {
                    let comp = self.completion;
                    let fstart = offset.saturating_sub(partial.len());
                    let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
                    let vp = editor.viewport.get_untracked();
                    let win = buf.win_origin.get_untracked();
                    let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
                    comp.buffer_id.set(Some(buffer_id));
                    comp.start_offset.set(fstart);
                    comp.anchor.set(anchor);
                    comp.items.set(items);
                    comp.selected.set(0);
                    comp.open.set(true);
                    return;
                }
            }
        }

        if let Some((rep, items)) =
            framework_completion::completions(buf.file.language, line_before)
        {
            let comp = self.completion;
            let fstart = offset.saturating_sub(rep);
            let (_, below) = editor.points_of_offset(fstart, cursor.affinity);
            let vp = editor.viewport.get_untracked();
            let win = buf.win_origin.get_untracked();
            let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);
            comp.buffer_id.set(Some(buffer_id));
            comp.start_offset.set(fstart);
            comp.anchor.set(anchor);
            comp.items.set(items);
            comp.selected.set(0);
            comp.open.set(true);
            return;
        }

        let start = word_start(&text, offset);
        let word = text[start..offset.min(text.len())].to_string();

        // Anchor the popup at the start of the replaced word.
        let (_, below) = editor.points_of_offset(start, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);

        let comp = self.completion;
        comp.buffer_id.set(Some(buffer_id));
        comp.start_offset.set(start);
        comp.anchor.set(anchor);

        // Snippets and built-ins (keywords / buffer words / Laravel) are
        // computed synchronously; LSP results are merged in when available.
        let snippet_items = snippets::completion_items(buf.file.language, &word);
        let mut builtin_items = builtin_completion::items(buf.file.language, &word, &text);
        // Eloquent columns from the live DB schema merge in alongside LSP results.
        if let Some((_, cols)) = crate::eloquent::complete(
            buf.file.language,
            &text,
            offset,
            &self.root.get_untracked(),
            &self.db_schema_cache.get_untracked(),
        ) {
            builtin_items.extend(cols);
        }

        let show = move |items: Vec<lsp_types::CompletionItem>| {
            let items = dedup_by_label(items);
            if items.is_empty() {
                comp.open.set(false);
            } else {
                comp.items.set(items);
                comp.selected.set(0);
                comp.open.set(true);
            }
        };

        match (self.lsp_for_active(), buf.uri.clone()) {
            (Some(client), Some(uri)) => {
                let send =
                    create_ext_action(self.cx, move |lsp: Vec<lsp_types::CompletionItem>| {
                        let mut items = snippet_items.clone();
                        items.extend(lsp);
                        items.extend(builtin_items.clone());
                        show(items);
                    });
                std::thread::spawn(move || {
                    let items = client
                        .completion(&uri, line as u32, col as u32)
                        .unwrap_or_default();
                    send(items);
                });
            }
            _ => {
                let mut items = snippet_items;
                items.extend(builtin_items);
                show(items);
            }
        }
    }

    pub fn move_completion(&self, delta: i64) {
        let comp = self.completion;
        let len = comp.items.with(|i| i.len());
        if len == 0 {
            return;
        }
        let cur = comp.selected.get_untracked() as i64;
        let next = (cur + delta).clamp(0, len as i64 - 1) as usize;
        comp.selected.set(next);
    }

    pub fn close_completion(&self) {
        if self.completion.open.get_untracked() {
            self.completion.open.set(false);
        }
    }

    /// Insert the selected completion. Returns true if something was inserted.
    pub fn accept_completion(&self) -> bool {
        let comp = self.completion;
        if !comp.open.get_untracked() {
            return false;
        }
        let items = comp.items.get_untracked();
        if items.is_empty() {
            return false;
        }
        let sel = comp.selected.get_untracked().min(items.len() - 1);
        let item = &items[sel];
        let is_snippet = item.detail.as_deref() == Some("snippet");
        let insert = item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone());
        let label = item.label.clone();

        let Some(bid) = comp.buffer_id.get_untracked() else {
            return false;
        };
        let Some(buf) = self.buffer_by_id(bid) else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };

        let end = editor.cursor.get_untracked().offset();
        let start = comp.start_offset.get_untracked().min(end);
        comp.open.set(false);

        if is_snippet {
            if let Some(body) = snippets::body(buf.file.language, &label) {
                let text = buf.doc.text().to_string();
                let indent = line_indent(&text, start);
                let (expanded, caret) = snippets::expand(&body, &indent);
                buf.doc.edit_single(
                    Selection::region(start, end),
                    &expanded,
                    EditType::InsertChars,
                );
                let pos = start + caret;
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(pos)),
                    None,
                    None,
                ));
                return true;
            }
        }

        buf.doc.edit_single(
            Selection::region(start, end),
            &insert,
            EditType::InsertChars,
        );
        // Place the caret at the end of the inserted text (not at the old
        // offset, which would land in the middle of a longer completion).
        let pos = start + insert.len();
        editor.cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(pos)),
            None,
            None,
        ));
        true
    }

    /// Resolve the Laravel helper token under the cursor, if any.
    fn laravel_token(&self) -> Option<(laravel::Helper, String, Rc<LaravelData>)> {
        let data = self.laravel.get()?;
        let buf = self.active_buffer()?;
        let editor = buf.editor.get_untracked()?;
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let upto = offset.min(text.len());
        let line_start = text[..upto].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let before = &text[line_start..upto];
        let line_end = text[upto..]
            .find('\n')
            .map(|i| upto + i)
            .unwrap_or(text.len());
        let after = &text[upto..line_end];
        let (helper, token) = laravel::token_at(before, after)?;
        Some((helper, token, data))
    }

    /// Caret on an ability string in `can()`/`authorize()`/`@can`/`Gate::allows()`
    /// jumps to the policy method or `Gate::define()` that declares it.
    fn goto_policy(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        if !matches!(buf.file.language, Language::Php | Language::Blade) {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let Some(name) = crate::inertia::call_string_at(&text, offset, crate::policies::CALLS)
        else {
            return false;
        };
        let root = self.root.get_untracked();
        if let Some((_, file, line)) = crate::policies::abilities(&root)
            .into_iter()
            .find(|(n, _, _)| *n == name)
        {
            self.jump_to(&path_to_uri(&file), line, 0);
            true
        } else {
            false
        }
    }

    /// Ziggy `route('name')` under the cursor in a JS-family file, plus the
    /// Laravel data — the same route table the PHP side uses.
    fn active_ziggy_route(&self) -> Option<(String, Rc<LaravelData>)> {
        let buf = self.active_buffer()?;
        if !matches!(
            buf.file.language,
            Language::TypeScript | Language::JavaScript | Language::Vue | Language::Svelte
        ) {
            return None;
        }
        let editor = buf.editor.get_untracked()?;
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let name = crate::inertia::route_at(&text, offset)?;
        let data = self.laravel.get()?;
        Some((name, data))
    }

    pub fn request_hover(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);

        let (_, below) = editor.points_of_offset(offset, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let anchor = Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0);

        let hover = self.hover;
        hover.anchor.set(anchor);

        // Laravel hover takes precedence inside helper strings.
        if let Some((helper, token, data)) = self.laravel_token() {
            if let Some(text) = laravel::hover_text(&data, helper, &token) {
                hover.text.set(text);
                hover.open.set(true);
                return;
            }
        }
        // Ziggy `route('…')` hover on the JS side.
        if let Some((name, data)) = self.active_ziggy_route() {
            if let Some(text) = laravel::hover_text(&data, laravel::Helper::Route, &name) {
                hover.text.set(text);
                hover.open.set(true);
                return;
            }
        }

        let (Some(client), Some(uri)) = (self.lsp_for_active(), buf.uri.clone()) else {
            return;
        };
        let send = create_ext_action(self.cx, move |text: Option<String>| match text {
            Some(text) if !text.trim().is_empty() => {
                hover.text.set(text);
                hover.open.set(true);
            }
            _ => hover.open.set(false),
        });
        std::thread::spawn(move || {
            let text = client.hover(&uri, line as u32, col as u32).ok().flatten();
            send(text);
        });
    }

    pub fn close_hover(&self) {
        if self.hover.open.get_untracked() {
            self.hover.open.set(false);
        }
    }

    pub fn request_signature_help(&self, buffer_id: u64) {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);

        // Anchor just above the caret line.
        let (above, _) = editor.points_of_offset(offset, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();
        let anchor = Point::new(win.x + above.x - vp.x0, win.y + above.y - vp.y0 - 26.0);

        let sig = self.signature;
        sig.anchor.set(anchor);
        let send = create_ext_action(self.cx, move |info: Option<SignatureInfo>| match info {
            Some(i) => {
                sig.label.set(i.label);
                sig.active
                    .set(i.active.map(|(a, b)| (a as usize, b as usize)));
                sig.open.set(true);
            }
            None => sig.open.set(false),
        });
        std::thread::spawn(move || {
            let info = client
                .signature_help(&uri, line as u32, col as u32)
                .ok()
                .flatten();
            send(info);
        });
    }

    pub fn close_signature(&self) {
        if self.signature.open.get_untracked() {
            self.signature.open.set(false);
        }
    }

    /// Jump to the definition of the symbol under the cursor (LSP).
    pub fn goto_definition(&self) {
        // Livewire: caret on a `wire:model` property jumps to its declaration.
        if self.livewire_goto() {
            return;
        }
        // Inertia: caret on `Inertia::render('Page')` jumps to the component.
        if self.goto_inertia_page() {
            return;
        }
        // Ziggy: caret on `route('name')` in JS jumps to the controller.
        if let Some((name, data)) = self.active_ziggy_route() {
            if let Some((p, l, c)) = laravel::navigate(&data, laravel::Helper::Route, &name) {
                self.jump_to(&path_to_uri(&p), l, c);
                return;
            }
        }
        // Gates/policies: caret on an ability jumps to the policy method.
        if self.goto_policy() {
            return;
        }
        // Laravel navigation first: route -> controller, view -> blade, etc.
        if let Some((helper, token, data)) = self.laravel_token() {
            if let Some((path, line, col)) = laravel::navigate(&data, helper, &token) {
                let uri = path_to_uri(&path);
                self.jump_to(&uri, line, col);
                return;
            }
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return;
        };
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let app = *self;
        let send = create_ext_action(self.cx, move |loc: Option<(String, u32, u32)>| match loc {
            Some((u, l, c)) => app.jump_to(&u, l as usize, c as usize),
            None => eprintln!("e: no definition found"),
        });
        std::thread::spawn(move || {
            let loc = client
                .definition(&uri, line as u32, col as u32)
                .ok()
                .flatten();
            send(loc);
        });
    }

    // ---- References & symbol search ------------------------------------

    /// Open the workspace symbol search (⌘T).
    pub fn open_symbol_search(&self) {
        let p = self.picker;
        p.mode.set(PickerMode::Symbols);
        p.query.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);
    }

    /// Open workspace-wide text search (⌘⇧F).
    pub fn open_global_search(&self) {
        let p = self.picker;
        p.mode.set(PickerMode::Search);
        p.query.set(String::new());
        p.replace.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);
    }

    /// Replace every occurrence of the search query across the workspace.
    pub fn replace_in_workspace(&self) {
        let query = self.picker.query.get_untracked();
        let replace = self.picker.replace.get_untracked();
        if query.is_empty() {
            return;
        }
        let root = self.root.get_untracked();
        let files = replace_in_dir(&root, &query, &replace);
        eprintln!("e: replaced in {files} file(s)");
        self.fs_rev.update(|r| *r += 1);
        // Reload affected open buffers and refresh the result list.
        self.check_external_changes();
        self.request_search(query);
    }

    /// Dispatch a picker query to the right backend for the current mode.
    pub fn run_picker_query(&self, query: String) {
        match self.picker.mode.get_untracked() {
            PickerMode::Symbols => self.request_symbols(query),
            PickerMode::Search => self.request_search(query),
            PickerMode::References => {}
        }
    }

    /// Grep the workspace for `query` (called reactively from the picker).
    pub fn request_search(&self, query: String) {
        let p = self.picker;
        if query.trim().len() < 2 {
            p.items.set(Vec::new());
            return;
        }
        let gen = p.gen.get_untracked() + 1;
        p.gen.set(gen);
        let roots = self.roots.get();
        let send = create_ext_action(self.cx, move |(g, items): (u64, Vec<PickerItem>)| {
            if g == p.gen.get_untracked() {
                p.items.set(items);
                p.selected.set(0);
            }
        });
        std::thread::spawn(move || {
            let mut items = Vec::new();
            for root in &roots {
                items.extend(grep_workspace(root, &query, 300));
                if items.len() >= 300 {
                    items.truncate(300);
                    break;
                }
            }
            send((gen, items));
        });
    }

    /// Run a workspace/symbol query (called reactively from the picker).
    pub fn request_symbols(&self, query: String) {
        let p = self.picker;
        let Some(client) = self.lsp_for_active() else {
            return;
        };
        if query.trim().is_empty() {
            p.items.set(Vec::new());
            return;
        }
        let gen = p.gen.get_untracked() + 1;
        p.gen.set(gen);
        let root = self.root.get();
        let send = create_ext_action(self.cx, move |(g, items): (u64, Vec<PickerItem>)| {
            if g == p.gen.get_untracked() {
                p.items.set(items);
                p.selected.set(0);
            }
        });
        std::thread::spawn(move || {
            let syms = client.workspace_symbol(&query).unwrap_or_default();
            let items = syms
                .into_iter()
                .take(200)
                .map(|(name, uri, line, ch)| PickerItem {
                    label: name,
                    detail: rel_uri(&uri, &root),
                    uri,
                    line,
                    char: ch,
                })
                .collect();
            send((gen, items));
        });
    }

    /// Find references to the symbol under the cursor (Shift+F12).
    pub fn request_references(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return;
        };
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());

        let p = self.picker;
        p.mode.set(PickerMode::References);
        p.query.set(String::new());
        p.items.set(Vec::new());
        p.selected.set(0);
        p.open.set(true);

        let root = self.root.get();
        let send = create_ext_action(self.cx, move |items: Vec<PickerItem>| {
            p.items.set(items);
            p.selected.set(0);
        });
        std::thread::spawn(move || {
            let refs = client
                .references(&uri, line as u32, col as u32)
                .unwrap_or_default();
            let items = refs
                .into_iter()
                .map(|(u, l, c)| PickerItem {
                    label: rel_uri(&u, &root),
                    detail: format!(":{}", l + 1),
                    uri: u,
                    line: l,
                    char: c,
                })
                .collect();
            send(items);
        });
    }

    /// Open `uri` and place the caret at `(line, col)`, recording the spot we
    /// jumped from in the navigation history.
    pub fn jump_to(&self, uri: &str, line: usize, col: usize) {
        self.record_nav();
        self.goto_location(uri_to_path(uri), line, col);
    }

    /// Open a file and place the caret at `(line, col)` without touching the
    /// navigation history (used by back/forward themselves).
    fn goto_location(&self, path: PathBuf, line: usize, col: usize) {
        self.open_path(path);
        // A freshly opened document's rope is populated on the next reactive
        // flush, so apply now and retry until it's ready for cold opens.
        if !self.apply_goto(line, col) {
            self.retry_goto(line, col, 0);
        }
    }

    fn retry_goto(&self, line: usize, col: usize, attempt: usize) {
        if attempt >= 6 {
            return;
        }
        let state = *self;
        floem::action::exec_after(std::time::Duration::from_millis(40), move |_| {
            if !state.apply_goto(line, col) {
                state.retry_goto(line, col, attempt + 1);
            }
        });
    }

    /// Place the caret at `(line, col)` in the active buffer. Returns false when
    /// the editor/document isn't ready yet (so the caller can retry).
    fn apply_goto(&self, line: usize, col: usize) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        // The document hasn't been populated yet — try again shortly.
        if line > 0 && buf.doc.text().is_empty() {
            return false;
        }
        let offset = editor.offset_of_line_col(line, col);
        editor.cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(offset)),
            None,
            None,
        ));
        buf.pending_goto.set(None);
        true
    }

    fn current_location(&self) -> Option<(PathBuf, usize, usize)> {
        let buf = self.active_buffer()?;
        let path = buf.file.path.clone()?;
        let editor = buf.editor.get_untracked()?;
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        Some((path, line, col))
    }

    /// Record the current location as a back-navigation target.
    fn record_nav(&self) {
        if let Some(loc) = self.current_location() {
            let dup = self.nav_back_stack.with_untracked(|v| {
                v.last()
                    .map(|l| l.0 == loc.0 && l.1 == loc.1)
                    .unwrap_or(false)
            });
            if !dup {
                self.nav_back_stack.update(|v| {
                    v.push(loc);
                    if v.len() > 100 {
                        v.remove(0);
                    }
                });
                self.nav_fwd_stack.update(|v| v.clear());
            }
        }
    }

    /// Navigate to the previous location in the history.
    pub fn nav_back(&self) {
        let Some(target) = self.nav_back_stack.try_update(|v| v.pop()).flatten() else {
            return;
        };
        if let Some(cur) = self.current_location() {
            self.nav_fwd_stack.update(|v| v.push(cur));
        }
        self.goto_location(target.0, target.1, target.2);
    }

    /// Navigate to the next location in the history.
    pub fn nav_forward(&self) {
        let Some(target) = self.nav_fwd_stack.try_update(|v| v.pop()).flatten() else {
            return;
        };
        if let Some(cur) = self.current_location() {
            self.nav_back_stack.update(|v| v.push(cur));
        }
        self.goto_location(target.0, target.1, target.2);
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Find the git conflict block containing `cursor`, returning
/// `(start, end, current_text, incoming_text)` in byte offsets.
fn find_conflict(text: &str, cursor: usize) -> Option<(usize, usize, String, String)> {
    let mut search = 0;
    while let Some(rel) = text[search..].find("<<<<<<<") {
        let start = search + rel;
        // Must be at the start of a line.
        if start != 0 && text.as_bytes()[start - 1] != b'\n' {
            search = start + 7;
            continue;
        }
        let after_marker = text[start..].find('\n').map(|i| start + i + 1)?;
        let sep = text[after_marker..]
            .find("\n=======")
            .map(|i| after_marker + i + 1)
            .or_else(|| {
                if text[after_marker..].starts_with("=======") {
                    Some(after_marker)
                } else {
                    None
                }
            })?;
        let after_sep = text[sep..].find('\n').map(|i| sep + i + 1)?;
        let gt = text[after_sep..]
            .find("\n>>>>>>>")
            .map(|i| after_sep + i + 1)
            .or_else(|| {
                if text[after_sep..].starts_with(">>>>>>>") {
                    Some(after_sep)
                } else {
                    None
                }
            })?;
        let end = text[gt..]
            .find('\n')
            .map(|i| gt + i + 1)
            .unwrap_or(text.len());

        if (start..end).contains(&cursor) {
            let current = text[after_marker..sep].to_string();
            let incoming = text[after_sep..gt].to_string();
            return Some((start, end, current, incoming));
        }
        search = end;
    }
    None
}

/// Replace every occurrence of `query` with `replace` in text files under
/// `root` (skipping dot-dirs, `target`, `node_modules`, and large/binary files).
/// Returns the number of files changed.
fn replace_in_dir(root: &std::path::Path, query: &str, replace: &str) -> usize {
    let mut changed = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => {
                    if entry
                        .metadata()
                        .map(|m| m.len() > 2_000_000)
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    if content.contains(query) {
                        let new = content.replace(query, replace);
                        if new != content {
                            let _ = std::fs::write(&path, new);
                            changed += 1;
                        }
                    }
                }
                Err(_) => {}
            }
        }
    }
    changed
}

/// Remove duplicate completion items, keeping the first of each label.
fn dedup_by_label(items: Vec<lsp_types::CompletionItem>) -> Vec<lsp_types::CompletionItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.label.clone()))
        .collect()
}

/// A short "x minutes ago" string for a unix timestamp.
fn rel_time(unix: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = (now - unix).max(0);
    let (n, unit) = if diff < 60 {
        return "just now".to_string();
    } else if diff < 3600 {
        (diff / 60, "minute")
    } else if diff < 86_400 {
        (diff / 3600, "hour")
    } else if diff < 2_592_000 {
        (diff / 86_400, "day")
    } else if diff < 31_536_000 {
        (diff / 2_592_000, "month")
    } else {
        (diff / 31_536_000, "year")
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

pub(crate) fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Byte ranges of trailing whitespace per line, plus whether a final newline
/// is missing. Used by trim-on-save.
fn trailing_trim_edits(text: &str) -> (Vec<(usize, usize)>, bool) {
    let mut edits = Vec::new();
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim_end_matches([' ', '\t', '\r']);
        if trimmed.len() < content.len() {
            edits.push((off + trimmed.len(), off + content.len()));
        }
        off += line.len();
    }
    let needs_newline = !text.is_empty() && !text.ends_with('\n');
    (edits, needs_newline)
}

/// Leading whitespace of the line containing `offset`.
fn line_indent(text: &str, offset: usize) -> String {
    let offset = offset.min(text.len());
    let ls = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    text[ls..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Byte range of the identifier surrounding `offset`.
fn word_range(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let mut start = offset;
    for (i, c) in text[..offset].char_indices().rev() {
        if is_word_char(c) {
            start = i;
        } else {
            break;
        }
    }
    let mut end = offset;
    for (i, c) in text[offset..].char_indices() {
        if is_word_char(c) {
            end = offset + i + c.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

/// The identifier surrounding `offset`, if any.
fn word_at(text: &str, offset: usize) -> String {
    let (start, end) = word_range(text, offset);
    text[start..end].to_string()
}

/// Next occurrence of `word` at or after `from`, wrapping to the start.
fn find_next(text: &str, word: &str, from: usize) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    let from = from.min(text.len());
    if let Some(p) = text[from..].find(word) {
        return Some(from + p);
    }
    text[..from].find(word)
}

/// Byte ranges of every whole-word (identifier-boundary) occurrence of `word`.
fn whole_word_occurrences(text: &str, word: &str) -> Vec<(usize, usize)> {
    let (hay, w) = (text.as_bytes(), word.as_bytes());
    let mut out = Vec::new();
    if w.is_empty() || w.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + w.len() <= hay.len() {
        if &hay[i..i + w.len()] == w {
            let before = i == 0 || !is_word_byte(hay[i - 1]);
            let after = i + w.len() >= hay.len() || !is_word_byte(hay[i + w.len()]);
            if before && after {
                out.push((i, i + w.len()));
                i += w.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

/// All non-overlapping matches of `query` in `text`, honouring the
/// case-sensitive / whole-word / regex options.
fn find_all_opts(
    text: &str,
    query: &str,
    case: bool,
    word: bool,
    regex: bool,
) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }

    if regex {
        let mut pat = query.to_string();
        if word {
            pat = format!(r"\b(?:{pat})\b");
        }
        if !case {
            pat = format!("(?i){pat}");
        }
        return match regex::Regex::new(&pat) {
            Ok(re) => re
                .find_iter(text)
                .filter(|m| m.end() > m.start())
                .map(|m| (m.start(), m.end()))
                .collect(),
            Err(_) => Vec::new(),
        };
    }

    let (h, n) = (text.as_bytes(), query.as_bytes());
    if n.len() > h.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= h.len() {
        let hit = (0..n.len()).all(|k| {
            if case {
                h[i + k] == n[k]
            } else {
                h[i + k].eq_ignore_ascii_case(&n[k])
            }
        });
        if hit {
            let (s, e) = (i, i + n.len());
            let boundary_ok = !word
                || ((s == 0 || !is_word_byte(h[s - 1])) && (e == h.len() || !is_word_byte(h[e])));
            if boundary_ok {
                out.push((s, e));
                i = e;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Find the matching bracket for a bracket adjacent to `offset`, returning
/// per-line highlight spans for both brackets.
fn compute_bracket_marks(text: &str, offset: usize) -> Vec<Vec<(usize, usize)>> {
    let bytes = text.as_bytes();
    let opens = b"([{";
    let closes = b")]}";

    // Prefer the bracket just before the cursor, else the one at the cursor.
    let candidates = [offset.checked_sub(1), Some(offset)];
    for pos in candidates.into_iter().flatten() {
        let Some(&b) = bytes.get(pos) else { continue };
        let other = if let Some(i) = opens.iter().position(|&o| o == b) {
            find_match(bytes, pos, closes[i], b, true)
        } else if let Some(i) = closes.iter().position(|&c| c == b) {
            find_match(bytes, pos, opens[i], b, false)
        } else {
            None
        };
        if let Some(m) = other {
            let starts = line_starts(text);
            let mut lines: Vec<Vec<(usize, usize)>> = vec![Vec::new(); starts.len()];
            for p in [pos, m] {
                let line = line_of(&starts, p);
                let ls = starts[line];
                lines[line].push((p - ls, p - ls + 1));
            }
            return lines;
        }
    }
    Vec::new()
}

/// Scan for the matching bracket. `target` is the bracket we look for, `self_ch`
/// the one we started on, `forward` the scan direction.
fn find_match(bytes: &[u8], from: usize, target: u8, self_ch: u8, forward: bool) -> Option<usize> {
    let mut depth = 0i32;
    if forward {
        let mut i = from;
        while i < bytes.len() {
            let c = bytes[i];
            if c == self_ch {
                depth += 1;
            } else if c == target {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            i += 1;
        }
    } else {
        let mut i = from as isize;
        while i >= 0 {
            let c = bytes[i as usize];
            if c == self_ch {
                depth += 1;
            } else if c == target {
                depth -= 1;
                if depth == 0 {
                    return Some(i as usize);
                }
            }
            i -= 1;
        }
    }
    None
}

/// Byte offset where each line starts.
pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        off += line.len();
        if line.ends_with('\n') {
            starts.push(off);
        }
    }
    if starts.is_empty() {
        starts.push(0);
    }
    starts
}

pub(crate) fn line_of(starts: &[usize], byte: usize) -> usize {
    starts.partition_point(|&s| s <= byte).saturating_sub(1)
}

/// Walk the workspace and collect lines matching `query` (case-insensitive).
fn grep_workspace(root: &std::path::Path, query: &str, max: usize) -> Vec<PickerItem> {
    let needle = query.to_lowercase();
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= max {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => {
                    // Skip large files; read the rest as UTF-8 (binaries fail).
                    if entry
                        .metadata()
                        .map(|m| m.len() > 2_000_000)
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    for (li, line) in content.lines().enumerate() {
                        if let Some(col) = line.to_lowercase().find(&needle) {
                            out.push(PickerItem {
                                label: line.trim_start().chars().take(120).collect(),
                                detail: format!(
                                    "{}:{}",
                                    rel_uri(&path_to_uri(&path), root),
                                    li + 1
                                ),
                                uri: path_to_uri(&path),
                                line: li as u32,
                                char: col as u32,
                            });
                            if out.len() >= max {
                                return out;
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }
    }
    out
}

/// Display a `file://` URI relative to the workspace root.
fn rel_uri(uri: &str, root: &std::path::Path) -> String {
    let path = uri_to_path(uri);
    path.strip_prefix(root)
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned()
}

/// Byte offset where the identifier ending at `offset` begins.
fn word_start(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut start = offset;
    for (i, c) in text[..offset].char_indices().rev() {
        if is_word_char(c) {
            start = i;
        } else {
            break;
        }
    }
    start
}

#[cfg(test)]
mod bracket_tests {
    use super::compute_bracket_marks;
    #[test]
    fn matches_outer_paren() {
        // "foo(bar(baz))" — cursor after first '(' (offset 4)
        let m = compute_bracket_marks("foo(bar(baz))", 4);
        let mut spans: Vec<(usize, usize)> = m.into_iter().flatten().collect();
        spans.sort();
        assert_eq!(spans, vec![(3, 4), (12, 13)]);
    }
    #[test]
    fn matches_close_brace() {
        // cursor right after the closing brace
        let m = compute_bracket_marks("a{b{c}d}", 8);
        let mut spans: Vec<(usize, usize)> = m.into_iter().flatten().collect();
        spans.sort();
        assert_eq!(spans, vec![(1, 2), (7, 8)]);
    }
}

#[cfg(test)]
mod rename_tests {
    use super::{whole_word_occurrences, word_at};

    #[test]
    fn word_boundaries() {
        let t = "let foo = foo_bar + foo;";
        // whole-word 'foo' should match positions 4 and 20, NOT inside 'foo_bar'
        let occ = whole_word_occurrences(t, "foo");
        assert_eq!(occ, vec![(4, 7), (20, 23)]);
    }

    #[test]
    fn word_under_cursor() {
        let t = "$user->name";
        assert_eq!(word_at(t, 2), "$user"); // cursor inside $user
        assert_eq!(word_at(t, 8), "name"); // cursor inside name
    }
}

#[cfg(test)]
mod inertia_replay_tests {
    use super::extract_inertia;

    #[test]
    fn extracts_page_object_from_html() {
        let body = r#"<div id="app" data-page="{&quot;component&quot;:&quot;Users/Index&quot;,&quot;props&quot;:{&quot;users&quot;:[{&quot;id&quot;:1}]}}"></div>"#;
        let (component, props) = extract_inertia(body).unwrap();
        assert_eq!(component, "Users/Index");
        assert!(props.get("users").unwrap().is_array());
        assert!(extract_inertia("<html>no inertia</html>").is_none());
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::parse_clockwork_latest;

    #[test]
    fn parses_clockwork_payload() {
        let v: serde_json::Value = serde_json::json!({
            "id": "abc123",
            "method": "POST",
            "uri": "/orders",
            "responseStatus": 201,
            "responseDuration": 84.5,
            "databaseQueries": [
                {"query": "select * from users where id = 1", "duration": 2},
                {"query": "select * from orders where user_id = 1", "duration": 3}
            ],
            "cacheQueries": [{"type": "hit"}, {"type": "miss"}, {"type": "hit"}],
            "emailsData": [{"data": {"subject": "Order shipped"}}],
            "events": [{"event": "OrderPlaced"}]
        });
        let r = parse_clockwork_latest(&v).unwrap();
        assert_eq!(r.id, "abc123");
        assert_eq!(r.method, "POST");
        assert_eq!(r.status, 201);
        assert_eq!(r.queries.len(), 2);
        assert_eq!(r.cache_hits, 2);
        assert_eq!(r.cache_misses, 1);
        assert_eq!(r.mails, vec!["Order shipped"]);
        assert_eq!(r.events, 1);
    }
}

#[cfg(test)]
mod pest_tests {
    use super::{html_title, pest_assertions, pest_test_name, url_path};

    #[test]
    fn path_and_name() {
        assert_eq!(
            url_path("https://app.test/users/1/edit?x=1"),
            "/users/1/edit"
        );
        assert_eq!(url_path("http://127.0.0.1:8000/"), "/");
        assert_eq!(pest_test_name("/users/1/edit"), "UsersEdit");
        assert_eq!(pest_test_name("/"), "Home");
    }

    #[test]
    fn assertions_from_response() {
        let json = pest_assertions(200, r#"{"data":[],"meta":{}}"#);
        assert!(json.contains("assertStatus(200)"));
        assert!(json.contains("assertJsonStructure(['data', 'meta'])"));
        let html = pest_assertions(200, "<html><head><title>Dashboard</title></head></html>");
        assert!(html.contains("assertSee('Dashboard')"));
        assert_eq!(html_title("<TITLE>Hi</TITLE>").as_deref(), Some("Hi"));
    }
}

#[cfg(test)]
mod trim_tests {
    use super::trailing_trim_edits;

    #[test]
    fn finds_trailing_and_missing_newline() {
        // "a  \nb\t\nc" : line0 trailing 2 spaces (1..3), line1 trailing tab (5..6), no final \n
        let (edits, nl) = trailing_trim_edits("a  \nb\t\nc");
        assert_eq!(edits, vec![(1, 3), (5, 6)]);
        assert!(nl);
    }

    #[test]
    fn clean_text_no_edits() {
        let (edits, nl) = trailing_trim_edits("a\nb\n");
        assert!(edits.is_empty());
        assert!(!nl);
    }
}
