//! Shared, reactive application state.
//!
//! `AppState` is `Copy` (every field is a Floem signal or `Scope`), so it can
//! be handed to as many view closures as needed without cloning ceremony.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use std::sync::mpsc::{channel, Receiver, Sender};

use floem::ext_event::create_ext_action;
use floem::kurbo::Point;
use floem::reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::{SelRegion, Selection};
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::Editor;

mod agent_ops;
mod buffer_ops;
mod diagnostic_ops;
mod edit_ops;
mod laravel_ops;
mod lsp_ops;
mod refactor_ops;
mod workspace_ops;

pub use crate::debug::DebugPanel;
pub use crate::editor_area::EditorArea;
pub use crate::grove_state::GrovePanel;
pub use crate::laravel_state::ProjectData;
pub use crate::log::LogPanel;
pub use crate::map::LaravelViews;
pub use crate::migrations::MigrationsPanel;
pub use crate::model_wizard::ModelWizardPanel;
pub use crate::navigation::Navigation;
pub use crate::problems::Problems;
pub use crate::request::RequestPanel;
pub use crate::review::ReviewPanel;
pub use crate::runtime::RuntimePanel;
pub use crate::semantic::SemanticSearch;
pub use crate::tdd_state::TestsPanel;
pub use crate::terminal_state::TerminalPanel;
pub use crate::tinker::TinkerPanel;
pub use crate::updater::UpdatePanel;
pub use crate::verify::VerifyPanel;
use lsp_types::{Diagnostic, TextEdit};

use e_agent::{AgentClient, ChatState, Streaming};
use e_core::buffer::{self, FileInfo};
use e_core::git;
use e_core::language::Language;
use e_core::syntax::highlight_lines;
use e_lsp::{path_to_uri, uri_to_path, LspClient, MessageLevel, ServerEvent};
use e_term::Terminal;

use crate::cmd_palette::CmdPalette;
use crate::completion::{Completion, HoverState, SignatureState};
use crate::config::{self, AgentConfig, Settings};
use crate::file_ops::{copy_recursive, duplicate_name, FileOp, FileOpKind};
use crate::find::FindState;
use crate::laravel::{self, LaravelData};
use crate::lsp_registry;
use crate::outline::OutlineItem;
use crate::picker::{Picker, PickerItem};
use crate::rename::RenameState;
use crate::session::{self, SessionData};
use crate::styling::{
    build_diag_lines, BpMarks, BracketMarks, DiagLines, FindMarks, FindSpan, GitMarks, Highlights,
    StopLine, TestMarks,
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
    pub views: RwSignal<Vec<String>>,
    /// Row counts per table for the tree, mostly the server's estimates.
    pub table_counts: RwSignal<HashMap<String, e_db::RowCount>>,
    pub error: RwSignal<Option<String>>,
    pub filter: RwSignal<String>,
    /// Block writes (cell edits, …). Defaults on for production-looking targets.
    pub read_only: RwSignal<bool>,
}

impl DbEntry {
    pub fn new(cx: Scope, config: e_db::DbConfig) -> Self {
        let config_read_only = config.looks_like_prod();
        DbEntry {
            config,
            conn: cx.create_rw_signal(None),
            expanded: cx.create_rw_signal(false),
            connecting: cx.create_rw_signal(false),
            tables: cx.create_rw_signal(Vec::new()),
            views: cx.create_rw_signal(Vec::new()),
            table_counts: cx.create_rw_signal(HashMap::new()),
            error: cx.create_rw_signal(None),
            filter: cx.create_rw_signal(String::new()),
            read_only: cx.create_rw_signal(config_read_only),
        }
    }
    pub fn key(&self) -> String {
        self.config.key()
    }
}

/// The manual add-connection form.
#[derive(Clone, Debug)]
pub struct DbForm {
    /// Identity of the connection being edited, empty when adding a new one.
    /// Carried through the form so editing a connection keeps its entry in the
    /// OS credential store instead of re-keying it.
    pub id: String,
    pub secrets_in_keychain: bool,
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
    /// `""` (auto), `disable`, `require` or `verify` (see `e_db::TlsMode`).
    pub tls: String,
    pub tls_ca: String,
}

impl Default for DbForm {
    fn default() -> Self {
        DbForm {
            id: String::new(),
            secrets_in_keychain: false,
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
            tls: String::new(),
            tls_ca: String::new(),
        }
    }
}

impl DbForm {
    pub fn to_config(&self) -> e_db::DbConfig {
        e_db::DbConfig {
            // Empty for a new connection; `save_connections` assigns one on
            // first persist and moves the secrets to the credential store.
            id: self.id.clone(),
            secrets_in_keychain: self.secrets_in_keychain,
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
            tls: self.tls.clone(),
            tls_ca: self.tls_ca.trim().to_string(),
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

/// What a confirmed [`DbConfirm`] does when accepted.
#[derive(Clone)]
pub enum ConfirmRun {
    /// Execute this SQL in the console (per-statement result tabs).
    Console(String),
    /// Execute these statements as one transaction on connection `key`, then
    /// reload the table if it is the one shown.
    Transaction(DbTransaction),
}

/// Statements to run as one transaction once confirmed, with what to record
/// when they succeed.
#[derive(Clone)]
pub struct DbTransaction {
    /// The connection to run them on.
    pub key: String,
    pub statements: Vec<String>,
    /// Appended to the session write log on success.
    pub log: Vec<WriteLogEntry>,
    /// The staged grid changes these statements carry out, cleared on success.
    pub submits_pending: bool,
    /// What the message on success says was done, e.g. "Submitted".
    pub done: &'static str,
}

/// A pending destructive / non-local / submit action awaiting confirmation.
#[derive(Clone)]
pub struct DbConfirm {
    /// Button/verb, e.g. "Run" or "Submit".
    pub verb: String,
    /// The statements shown in the dialog for review.
    pub statements: Vec<String>,
    pub env: e_db::Environment,
    /// Whether an "I understand" acknowledgement is required (non-local).
    pub needs_ack: bool,
    pub ack: RwSignal<bool>,
    pub run: ConfirmRun,
}

/// A `:param` prompt awaiting values before a console run.
#[derive(Clone)]
pub struct DbParams {
    /// The original SQL (with placeholders) to run once values are supplied.
    pub sql: String,
    /// `(name, value-input)` for each parameter.
    pub fields: Vec<(String, RwSignal<String>)>,
}

/// A row's primary key as `(column, value)` predicates.
pub type RowPk = Vec<(String, Option<String>)>;

/// A staged (not-yet-committed) cell edit in the data grid.
#[derive(Clone)]
pub struct PendingEdit {
    pub column: String,
    pub pk: RowPk,
    pub new: Option<String>,
    /// The value before the edit, for the session undo-log's reverse statement.
    pub old: Option<String>,
}

/// Where the staged changes show in the grid on screen: each shown row's
/// primary key as read from the database (before any staged edit), and the
/// cells and rows that have a staged change.
#[derive(Clone, Default)]
pub struct PendingView {
    pub row_pks: Vec<Option<RowPk>>,
    pub cells: HashSet<(usize, usize)>,
    pub rows: HashSet<usize>,
}

/// A workspace-wide replace that has been planned but not yet written.
///
/// Replace All walks the workspace twice: once to build this (a preview), and
/// again to apply it once the user has said yes.
#[derive(Clone)]
pub struct ReplaceConfirm {
    pub query: String,
    pub replacement: String,
    pub opts: crate::workspace_search::SearchOpts,
    pub plan: crate::workspace_search::ReplacePlan,
}

/// One executed write in the session log: the statement, and its reverse when
/// one can be generated.
#[derive(Clone)]
pub struct WriteLogEntry {
    pub forward: String,
    pub reverse: Option<String>,
}

/// One SQL console result set, shown as a tab. Pinned tabs survive the next run;
/// unpinned ones are replaced.
#[derive(Clone)]
pub struct ResultTab {
    pub title: String,
    pub result: Option<e_db::QueryResult>,
    pub error: Option<String>,
    pub pinned: bool,
    /// Connection key, so the tab stays associated with its database.
    pub key: Option<String>,
}

/// One editable field in the "insert row" dialog, bound to its own signals.
#[derive(Clone)]
pub struct InsertField {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub value: RwSignal<String>,
    pub is_null: RwSignal<bool>,
}

/// Status of the TDD test-runner loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TddStatus {
    Idle,
    Running,
    Passed,
    Failed,
    /// Stop was pressed: neither passed nor failed.
    Stopped,
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
    /// The text as it last matched the file on disk, so undoing back to it
    /// clears `dirty` again. Shares its storage with the document's rope.
    pub saved: Rc<RefCell<Rope>>,
    pub highlights: Highlights,
    /// Per-line diagnostic spans (for inline squiggles).
    pub diag_lines: DiagLines,
    /// Per-line git change markers.
    pub git_marks: GitMarks,
    /// The file as it is in HEAD, which the change marks compare against;
    /// `None` until read, and for a file git doesn't have.
    pub head_text: Rc<RefCell<Option<String>>>,
    /// Lines carrying a debug breakpoint (0-based).
    pub bp_marks: BpMarks,
    /// The line the debugger is currently stopped on (0-based), if any.
    pub stop_line: StopLine,
    /// How the tests declared in this file last ended, by line (0-based).
    pub test_marks: TestMarks,
    /// Per-line find-match spans.
    pub find_marks: FindMarks,
    /// Matching-bracket highlight spans.
    pub bracket_marks: BracketMarks,
    /// `file://` URI, when backed by a path (used for LSP).
    pub uri: Option<String>,
    /// Document version last announced to the language servers.
    pub lsp_version: RwSignal<i64>,
    /// Bumped per edit; a lint run for an older generation is dropped.
    pub lint_gen: Rc<std::cell::Cell<u64>>,
    /// The live editor in each pane, set once its view is built; a read gets
    /// the one the buffer is being worked in (see `per_pane`).
    pub editor: crate::per_pane::PerPane<Option<Editor>>,
    /// Each pane's editor's top-left position in the window (for popups).
    pub win_origin: crate::per_pane::PerPane<Point>,
    /// Each pane's caret as last seen, kept past its editor: splitting builds
    /// the panes' editors afresh, and they start from here.
    pub caret: crate::per_pane::PerPane<Option<Cursor>>,
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
    /// Pending inline AI suggestion ("ghost text"), if any.
    pub ghost: RwSignal<Option<crate::ghost::GhostText>>,
    /// Very large file — expensive per-edit features are skipped for speed.
    pub large: bool,
    /// Text encoding label (e.g. `UTF-8`, `windows-1252`).
    pub encoding: RwSignal<String>,
    /// Laravel query-builder lint diagnostics (unknown columns), merged with LSP.
    pub lint: Rc<RefCell<Vec<Diagnostic>>>,
    /// PHPStan findings for this file, merged with LSP. Kept apart from `lint`
    /// so a run of one never erases the other's results.
    pub analysis: Rc<RefCell<Vec<Diagnostic>>>,
    /// A pending migration's risks against the data (see
    /// [`crate::migrations`]), merged with LSP like `analysis`.
    pub migration_risks: Rc<RefCell<Vec<Diagnostic>>>,
    /// `(line, text)` shown dimmed at the end of a line: the SQL a pending
    /// migration's line will run.
    pub annotations: RwSignal<Vec<(usize, String)>>,
    /// Exceptions the running app logged on lines of this file, `(line, text)`,
    /// shown in red at the end of the line.
    pub alerts: RwSignal<Vec<(usize, String)>>,
    /// Expressions evaluated with `⌘⏎`, their values at the end of the line.
    pub evals: RwSignal<Vec<crate::inline_eval::EvalMark>>,
    /// The same exceptions as diagnostics, merged with the language servers'.
    pub caught: Rc<RefCell<Vec<Diagnostic>>>,
    /// Branching undo history (see [`e_core::undotree`]).
    pub undo: Rc<RefCell<e_core::undotree::UndoTree>>,
    /// When set, a text change is caused by undo-tree navigation, so it must
    /// not be recorded back into the tree.
    pub undo_nav: Rc<std::cell::Cell<bool>>,
    /// Tab width for this buffer (from `.editorconfig`, else the global setting).
    pub tab_width: usize,
    /// Resolved EditorConfig properties for this file's path.
    pub editorconfig: e_core::editorconfig::EditorConfig,
}

/// One terminal session (a running shell).
#[derive(Clone)]
pub struct TermSession {
    pub id: u64,
    pub term: Rc<RefCell<Terminal>>,
    /// Custom name (empty = default "zsh N").
    pub name: RwSignal<String>,
}

/// An agent's request to be told the diagnostics for a file once every
/// running server has re-published after the file changed.
#[derive(Clone)]
pub struct DiagWaiter {
    pub id: u64,
    pub uri: String,
    /// Server ids we haven't heard from yet.
    pub pending: HashSet<String>,
    pub reply: std::sync::mpsc::Sender<serde_json::Value>,
}

/// Queue of `(server id, event)` from the LSP reader threads, drained on the
/// UI thread.
pub type LspQueue = Arc<Mutex<VecDeque<(String, ServerEvent)>>>;

/// How many times a language server may die and be brought back per session
/// before we stop trying. A server that keeps crashing on the same file would
/// otherwise burn CPU forever behind the user's back.
const MAX_LSP_RESTARTS: u32 = 3;

/// What we know about one language server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerHealth {
    /// Spawned; the handshake is still running on a background thread.
    Starting { restarts: u32 },
    /// Up; `restarts` says how many crashes it has come back from.
    Running { restarts: u32 },
    /// The binary isn't on `PATH`. Retried when it appears.
    NotInstalled,
    /// It started but refused to initialise. Left down until asked to retry.
    Failed(String),
    /// Died more than [`MAX_LSP_RESTARTS`] times. Left down until asked to retry.
    GaveUp { crashes: u32 },
}

// The language-server registry lives in [`crate::lsp_registry`] (a language can
// have several servers — PHP runs intelephense *and* laravel-lsp).

/// State of the AI agent panel (`⌘L`).
///
/// The second group lifted out of `AppState` after [`DbPanel`], and the reason
/// that split is a pattern rather than a one-off: the panel's terminal, native
/// chat client, composer and audit log belong together and to nothing else.
#[derive(Clone, Copy)]
pub struct AgentPanel {
    /// Whether the agent panel is visible (toggled with ⌘L).
    pub open: RwSignal<bool>,
    /// The currently selected agent id.
    pub current: RwSignal<String>,
    /// The running agent PTY, if started.
    pub term: RwSignal<Option<Rc<RefCell<Terminal>>>>,
    /// Whether the agent panel currently has keyboard focus.
    pub focused: RwSignal<bool>,
    /// Pulsed on open so the panel grabs focus without re-grabbing on close.
    pub focus_pulse: RwSignal<u64>,
    /// The running native RPC client, if started.
    pub native_client: RwSignal<Option<Rc<e_agent::AgentClient>>>,
    /// The folded conversation rendered by the native chat panel.
    pub chat: RwSignal<e_agent::ChatState>,
    /// Current text in the native composer.
    pub composer: RwSignal<String>,
    /// Backing document for the multi-line composer editor.
    pub composer_doc: RwSignal<Option<Rc<TextDocument>>>,
    /// The composer editor handle (to reset its cursor after clearing).
    pub composer_editor: RwSignal<Option<Editor>>,
    /// Shared queue of decoded events (fed by the reader-forwarder thread,
    /// drained on the UI thread). Never coalesced, so no delta is lost.
    pub events: RwSignal<Arc<Mutex<VecDeque<e_agent::AgentEvent>>>>,
    /// Sender half of the wake channel, handed to the reader-forwarder thread.
    pub(crate) wake_tx: RwSignal<std::sync::mpsc::Sender<()>>,
    /// Taken once at startup to build the UI-thread drain bridge.
    pub wake_rx: RwSignal<Option<std::sync::mpsc::Receiver<()>>>,
    pub width: RwSignal<f64>,
    /// Timeline of everything the agent did over the socket `(time, method, summary)`.
    pub log: RwSignal<Vec<(String, String, String)>>,
    pub log_open: RwSignal<bool>,
    /// Where the agent is currently "looking" `(path, line0)` — a ghost marker.
    pub mark: RwSignal<Option<(PathBuf, usize)>>,
    /// A pending edit the agent proposed, awaiting per-hunk review.
    pub edit: RwSignal<Option<AgentEdit>>,
    /// Cascade's mode: `code` (tools on) or `ask` (answer and plan only).
    pub mode: RwSignal<String>,
    /// The working tree before the turn now running, as a tree object; its
    /// receipt is made from it when the turn ends.
    pub turn_before: RwSignal<Option<String>>,
    /// The worktree (a Grove try) the agent works in, when not the project.
    pub worktree: RwSignal<Option<crate::grove::Try>>,
    /// An agent's `run` / `tinker` requests waiting for the user's answer.
    pub run_consents: RwSignal<Vec<crate::run_consent::RunConsent>>,
    /// The user allowed the agent's commands for the rest of the session.
    pub runs_allowed: RwSignal<bool>,
}

impl AgentPanel {
    /// Both halves of the wake channel are taken together: the panel owns the
    /// pairing, rather than the sender living on `AppState` and the receiver
    /// here.
    fn new(wake_tx: std::sync::mpsc::Sender<()>, wake_rx: std::sync::mpsc::Receiver<()>) -> Self {
        AgentPanel {
            wake_tx: RwSignal::new(wake_tx),
            open: RwSignal::new(false),
            current: RwSignal::new(config::load_default_agent()),
            term: RwSignal::new(None),
            focused: RwSignal::new(false),
            focus_pulse: RwSignal::new(0),
            native_client: RwSignal::new(None),
            chat: RwSignal::new(e_agent::ChatState::new()),
            composer: RwSignal::new(String::new()),
            composer_doc: RwSignal::new(None),
            composer_editor: RwSignal::new(None),
            events: RwSignal::new(Arc::new(Mutex::new(VecDeque::new()))),
            wake_rx: RwSignal::new(Some(wake_rx)),
            width: RwSignal::new(600.0),
            log: RwSignal::new(Vec::new()),
            log_open: RwSignal::new(false),
            mark: RwSignal::new(None),
            edit: RwSignal::new(None),
            mode: RwSignal::new(config::load_settings().cascade_mode),
            turn_before: RwSignal::new(None),
            worktree: RwSignal::new(None),
            run_consents: RwSignal::new(Vec::new()),
            runs_allowed: RwSignal::new(false),
        }
    }
}

/// State of the database panel (`⌘3`).
///
/// Split out of `AppState`, which had grown to 206 fields. These 56 were its
/// largest cohesive group — the whole database IDE, from the connection list to
/// the SQL console's result tabs — and nothing outside the panel needs them
/// individually. `AppState` keeps one field, `db`.
///
/// `Copy`, like `AppState` itself: every field is a signal or a handle, so this
/// is a bundle of pointers rather than the data.
#[derive(Clone, Copy)]
pub struct DbPanel {
    pub width: RwSignal<f64>,
    /// Whether the Database panel is visible (toggled with ⌘3).
    pub open: RwSignal<bool>,
    /// Saved connections for the current project.
    pub conns: RwSignal<Vec<DbEntry>>,
    /// Whether the add-connection form is showing.
    pub adding: RwSignal<bool>,
    /// The manual-connection form contents.
    pub form: RwSignal<DbForm>,
    /// Results overlay (table browse / query).
    pub result_open: RwSignal<bool>,
    pub result: RwSignal<Option<e_db::QueryResult>>,
    pub result_title: RwSignal<String>,
    pub result_error: RwSignal<Option<String>>,
    pub result_loading: RwSignal<bool>,
    /// The connection the results view runs queries against.
    pub result_key: RwSignal<Option<String>>,
    /// The SQL editor text in query mode.
    pub query_text: RwSignal<String>,
    /// The SQL console's backing document, so programmatic SQL (browse queries,
    /// run-under-cursor, saved/history queries) can be pushed into the editor.
    pub console_doc: RwSignal<Option<Rc<TextDocument>>>,
    /// Height (px) of the SQL console editor; drag the handle below it to resize.
    pub console_height: RwSignal<f64>,
    /// The SQL console's editor handle + window origin, for schema completion.
    pub console_editor: RwSignal<Option<Editor>>,
    pub console_win: RwSignal<Point>,
    /// Query-history panel: open state, current (searched) entries, search text.
    pub history_open: RwSignal<bool>,
    pub history: RwSignal<Vec<e_db::history::HistoryEntry>>,
    pub history_query: RwSignal<String>,
    /// SQL console result tabs (one per statement of the last run + pinned ones).
    pub result_tabs: RwSignal<Vec<ResultTab>>,
    pub active_tab: RwSignal<usize>,
    /// Run generation: bumped on each run and on cancel, so a cancelled/superseded
    /// query's result is discarded when it finally returns.
    pub run_gen: RwSignal<u64>,
    /// Total row count of the current browsed table (with filter), for paging.
    pub total_rows: RwSignal<Option<i64>>,
    /// EXPLAIN findings (full scans / missing indexes) for the current plan.
    pub explain_issues: RwSignal<Vec<String>>,
    /// The "search all tables" input.
    pub search_query: RwSignal<String>,
    /// Foreign-key relationships of the active DB (for the schema-relationships
    /// view), and whether that panel is open.
    pub erd: RwSignal<Vec<e_db::ForeignKey>>,
    pub erd_open: RwSignal<bool>,
    /// Session undo-log of executed writes (newest last), and the panel's state.
    pub write_log: RwSignal<Vec<WriteLogEntry>>,
    pub write_log_open: RwSignal<bool>,
    /// A destructive / non-local / submit action awaiting confirmation, if any.
    pub confirm: RwSignal<Option<DbConfirm>>,
    /// A `:param` prompt awaiting values, and the last-entered values to prefill.
    pub params: RwSignal<Option<DbParams>>,
    pub param_last: RwSignal<HashMap<String, String>>,
    /// Staged cell edits `(pk, column) -> edit` and staged row deletions, for
    /// transactional editing (Submit / Revert). They are keyed by primary key,
    /// not position, so paging, sorting and filtering keep them; they all
    /// belong to one table, `pending_target` (connection key, table).
    pub pending_edits: RwSignal<HashMap<(RowPk, String), PendingEdit>>,
    pub pending_deletes: RwSignal<HashSet<RowPk>>,
    pub pending_target: RwSignal<Option<(String, String)>>,
    /// Where the staged changes fall in the grid on screen.
    pub pending_view: RwSignal<PendingView>,
    /// The table being browsed (None in free-query mode).
    pub result_table: RwSignal<Option<String>>,
    /// Results subview: `data` or `structure`.
    pub subview: RwSignal<String>,
    /// Structure (column) metadata for the browsed table.
    pub columns: RwSignal<Vec<e_db::ColumnInfo>>,
    /// Indexes of the currently-inspected table (Structure subview).
    pub indexes: RwSignal<Vec<e_db::IndexInfo>>,
    /// Active sort: `(column, ascending)`.
    pub sort: RwSignal<Option<(String, bool)>>,
    /// Active data-view filter: `(column, Some(value) | None for IS NULL)`.
    pub filter: RwSignal<Option<(String, Option<String>)>>,
    /// Current page (0-based) when browsing a table.
    pub page: RwSignal<usize>,
    /// Test-connection state for the add form: ``/`testing`/`ok`/error.
    pub test_state: RwSignal<String>,
    /// The connection key being edited (None when adding a new one).
    pub editing_key: RwSignal<Option<String>>,
    /// Pending scroll delta for the results grid `(dx, dy, tick)`; the tick
    /// makes every key press a distinct value so the scroll effect re-fires.
    pub scroll: RwSignal<(f64, f64, u64)>,
    /// An agent-proposed query awaiting the user's consent.
    pub consent: RwSignal<Option<DbConsent>>,
    /// Cached live DB schema `table -> columns`, for Eloquent completion. Behind
    /// an `Arc` so the per-keystroke readers share it instead of copying it.
    pub schema_cache: RwSignal<Arc<std::collections::HashMap<String, Vec<e_db::ColumnInfo>>>>,
    /// The cell currently being edited `(row, col, column_name)`.
    pub edit: RwSignal<Option<(usize, usize, String)>>,
    pub edit_value: RwSignal<String>,
    pub edit_null: RwSignal<bool>,
    /// The cell (row, col) selected in the data grid, shown in the value viewer.
    pub selected_cell: RwSignal<Option<(usize, usize)>>,
    /// "Insert row" dialog: whether it's open, and one field per column.
    pub insert_open: RwSignal<bool>,
    pub insert_fields: RwSignal<Vec<InsertField>>,
    /// Saved queries for the current project.
    pub queries: RwSignal<Vec<e_db::SavedQuery>>,
    /// Whether the "name this query" input is showing.
    pub saving_query: RwSignal<bool>,
    /// The name being typed for the query about to be saved.
    pub query_name: RwSignal<String>,
}

impl DbPanel {
    fn new() -> Self {
        DbPanel {
            width: RwSignal::new(280.0),
            open: RwSignal::new(false),
            conns: RwSignal::new(Vec::new()),
            adding: RwSignal::new(false),
            form: RwSignal::new(DbForm::default()),
            result_open: RwSignal::new(false),
            result: RwSignal::new(None),
            result_title: RwSignal::new(String::new()),
            result_error: RwSignal::new(None),
            result_loading: RwSignal::new(false),
            result_key: RwSignal::new(None),
            query_text: RwSignal::new(String::new()),
            console_doc: RwSignal::new(None),
            console_height: RwSignal::new(120.0),
            console_editor: RwSignal::new(None),
            console_win: RwSignal::new(Point::ZERO),
            history_open: RwSignal::new(false),
            history: RwSignal::new(Vec::new()),
            history_query: RwSignal::new(String::new()),
            result_tabs: RwSignal::new(Vec::new()),
            active_tab: RwSignal::new(0),
            run_gen: RwSignal::new(0),
            total_rows: RwSignal::new(None),
            explain_issues: RwSignal::new(Vec::new()),
            search_query: RwSignal::new(String::new()),
            erd: RwSignal::new(Vec::new()),
            erd_open: RwSignal::new(false),
            write_log: RwSignal::new(Vec::new()),
            write_log_open: RwSignal::new(false),
            confirm: RwSignal::new(None),
            params: RwSignal::new(None),
            param_last: RwSignal::new(HashMap::new()),
            pending_edits: RwSignal::new(HashMap::new()),
            pending_deletes: RwSignal::new(HashSet::new()),
            pending_target: RwSignal::new(None),
            pending_view: RwSignal::new(PendingView::default()),
            result_table: RwSignal::new(None),
            subview: RwSignal::new("data".into()),
            columns: RwSignal::new(Vec::new()),
            indexes: RwSignal::new(Vec::new()),
            sort: RwSignal::new(None),
            filter: RwSignal::new(None),
            page: RwSignal::new(0),
            test_state: RwSignal::new(String::new()),
            editing_key: RwSignal::new(None),
            scroll: RwSignal::new((0.0, 0.0, 0)),
            consent: RwSignal::new(None),
            schema_cache: RwSignal::new(Arc::new(std::collections::HashMap::new())),
            edit: RwSignal::new(None),
            edit_value: RwSignal::new(String::new()),
            edit_null: RwSignal::new(false),
            selected_cell: RwSignal::new(None),
            insert_open: RwSignal::new(false),
            insert_fields: RwSignal::new(Vec::new()),
            queries: RwSignal::new(Vec::new()),
            saving_query: RwSignal::new(false),
            query_name: RwSignal::new(String::new()),
        }
    }
}

/// Source control: the repository, its status and the panel.
///
/// `Copy`, like `AppState`: signals and handles only.
#[derive(Clone, Copy)]
pub struct GitPanel {
    /// Whether the left sidebar shows the Source Control panel (⌘2).
    pub panel_open: RwSignal<bool>,
    /// The repository root, if the workspace is inside a git repo.
    pub root: RwSignal<Option<PathBuf>>,
    /// Current branch name.
    pub branch: RwSignal<Option<String>>,
    /// Working-tree status entries.
    pub status: RwSignal<Vec<git::StatusEntry>>,
    /// The commit-message input.
    pub commit_msg: RwSignal<String>,
    /// Recent commits: `(hash, author, rel time, summary)`.
    pub log: RwSignal<Vec<(String, String, String, String)>>,
    /// Number of stash entries.
    pub stash_count: RwSignal<usize>,
}

/// The language servers: their clients, health and incoming events.
///
/// `Copy`, like `AppState`: signals and handles only.
#[derive(Clone, Copy)]
pub struct LspState {
    /// The PHP language server, started lazily on first PHP file.
    /// Running language servers, keyed by server id.
    pub clients: RwSignal<HashMap<String, Arc<LspClient>>>,
    /// Per-server health: drives the status bar and the restart policy.
    pub health: RwSignal<HashMap<String, ServerHealth>>,
    /// What each server says it is busy with (`Indexing 42%`), while it is.
    pub progress: RwSignal<HashMap<String, String>>,
    /// Pending `(server id, event)` from the LSP reader threads.
    pub queue: RwSignal<LspQueue>,
    /// Receiver, taken once by the UI to build a reactive signal.
    pub rx: RwSignal<Option<Receiver<()>>>,
}

/// Global editor state.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Scope used to create per-document signals.
    pub cx: Scope,
    /// Workspace root shown in the file tree.
    pub root: RwSignal<PathBuf>,
    /// Whether the project at `root` may run its own code (see `trust`).
    pub trusted: RwSignal<bool>,
    /// The restricted-mode banner was put away for this session.
    pub trust_banner_dismissed: RwSignal<bool>,
    /// All workspace root folders (multi-root). The first is the primary root.
    pub roots: RwSignal<Vec<PathBuf>>,
    /// Monotonic id source.
    next_id: RwSignal<u64>,
    /// Servers whose install command is running in a terminal tab.
    lsp_installing: RwSignal<HashSet<String>>,
    /// Cached “is this a Laravel project?” (decides whether laravel-lsp runs).
    laravel_project: RwSignal<Option<bool>>,
    /// Channel the LSP reader threads wake the UI with.
    /// Wake notification only — payloads travel in [`Self::lsp_queue`], because a
    /// signal-per-message coalesces (only the last value survives a frame) and
    /// two servers publishing for the same file would drop one of them.
    lsp_tx: RwSignal<Sender<()>>,
    /// Completion popup state.
    pub completion: Completion,
    /// Hover popup state.
    pub hover: HoverState,
    /// The peek at a hunk, opened from the gutter's change bar.
    pub hunk_peek: RwSignal<Option<crate::hunk_peek::HunkPeek>>,
    /// When `.git/logs/HEAD` last changed, as last seen: it moves on every
    /// commit and checkout, also ones made outside `e`.
    pub head_moved: RwSignal<Option<std::time::SystemTime>>,
    /// Signature-help popup state.
    pub signature: SignatureState,
    /// Fingerprint of the files `LaravelData` is scraped from, as of the last
    /// check; a change means the data is stale and is reloaded.
    laravel_fp: RwSignal<Option<u64>>,
    laravel_fp_busy: RwSignal<bool>,
    /// A scrape is running; another change while it runs queues one more.
    laravel_loading: RwSignal<bool>,
    laravel_reload_pending: RwSignal<bool>,
    /// Idle ticks since the last freshness check.
    laravel_tick: RwSignal<u32>,
    /// References / symbol-search picker.
    pub picker: Picker,
    /// A planned workspace replace awaiting confirmation (`Some` shows the dialog).
    pub replace_confirm: RwSignal<Option<ReplaceConfirm>>,
    pub(crate) next_term_id: RwSignal<u64>,
    pub(crate) term_tx: RwSignal<Sender<()>>,
    /// Document outline of the active buffer.
    pub outline: RwSignal<Vec<OutlineItem>>,
    /// Find-in-file state.
    pub find: FindState,
    /// Local rename state.
    pub rename: RenameState,
    /// Command palette (⌘⇧P).
    pub cmd: CmdPalette,
    /// User settings loaded from config.json.
    pub settings: RwSignal<Settings>,
    /// File-operation name prompt (new/rename/duplicate).
    pub file_op: FileOp,
    /// Bumped after any filesystem change to refresh the file tree.
    pub fs_rev: RwSignal<u64>,

    /// The configured agents (from config.json or built-in defaults).
    pub agents: RwSignal<Vec<AgentConfig>>,

    /// Wake sender cloned into each forwarder thread to nudge the UI drain.
    // ---- Tinker scratchpad ---------------------------------------------

    // ---- New Eloquent Model / pivot wizard ------------------------------

    // ---- Laravel architecture map --------------------------------------

    // ---- Semantic search -----------------------------------------------

    // ---- Undo tree -----------------------------------------------------

    // ---- Schema diff (migrations vs live DB) ---------------------------

    // ---- Eloquent relationship graph -----------------------------------

    // ---- Event dispatch graph ------------------------------------------

    // ---- Inertia props contract ----------------------------------------

    // ---- Related files (model ↔ migration ↔ factory ↔ …) ---------------

    // ---- Runtime insight (continuous Clockwork capture) ----------------
    diag_waiter_seq: RwSignal<u64>,
    // ---- Agent session review (see [`crate::review`]) ----

    // ---- Step-debugging (DAP session) ----------------------------------

    // ---- Laravel log tail ----------------------------------------------

    // ---- Request replay (from the architecture map) --------------------

    // ---- Autonomous TDD loop -------------------------------------------

    // ---- Auto-update ----------------------------------------------------
    /// Go-to-line prompt state.
    pub goto: crate::editing::GotoState,
    /// Task-runner palette state + detected tasks.
    pub task: crate::task_palette::TaskState,
    pub task_list: RwSignal<Vec<crate::tasks::Task>>,
    /// The Artisan palette and the app's command list (loaded on first open).
    pub artisan: crate::artisan_palette::ArtisanState,
    pub recent: crate::recent::RecentState,

    // Whether the graphical settings page is open.
    // Pinned tab ids.

    // ---- Source control (git) ------------------------------------------
    /// Everything behind the database panel (⌘3).
    pub db: DbPanel,
    /// Everything behind the AI agent panel (⌘L).
    pub agent: AgentPanel,
    /// The debugger (F5): its session, where it stopped, its panel.
    pub debug: DebugPanel,
    /// The Tests panel and the fix-to-green loop.
    pub tests: TestsPanel,
    /// Grove: the project's site, its mail and hooks, and the panel.
    pub grove: GrovePanel,
    /// Session review (⌘⌥V): the changeset, its flags and the ship gate.
    pub review: ReviewPanel,
    /// Source control: the repository, its status and the panel.
    pub git: GitPanel,
    /// The language servers: their clients, health and incoming events.
    pub lsp: LspState,
    /// The integrated terminal: its sessions, panes and panel.
    pub term: TerminalPanel,
    /// Runtime Insight: the requests the app served and the lenses they put in the code.
    pub runtime: RuntimePanel,
    /// The request runner: what was sent and what came back.
    pub req: RequestPanel,
    /// Migrations: their status, the panel and the drift bar.
    pub migrations: MigrationsPanel,
    /// The model wizard: its spec, its plan and the panel.
    pub model_wizard: ModelWizardPanel,
    /// Semantic search: the index, the query and its results.
    pub sem: SemanticSearch,
    /// The Tinker panel.
    pub tinker: TinkerPanel,
    /// Verify: a route measured before and after a change.
    pub verify: VerifyPanel,
    /// The in-app updater's notice.
    pub updater: UpdatePanel,
    /// The Laravel log, the exceptions caught in it and the failed jobs.
    pub log: LogPanel,
    /// Getting around: back and forward, recent files, pinned tabs.
    pub nav: Navigation,
    /// The Laravel overlays: the map, relations, events, contracts, related files and the schema diff.
    pub views: LaravelViews,
    /// The editor area: open buffers, which pane shows which, and how text is shown.
    pub editor: EditorArea,
    /// What the language servers and linters report, and the code actions on it.
    pub problems: Problems,
    /// The window's furniture: palettes, the sidebar, About and Settings, messages and the confirm dialog.
    pub ui: Chrome,
    /// What e knows about the project: the Laravel data, ide.json's rules and the project's own rules.
    pub project: ProjectData,
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
    undo_store_dir().join(format!("{:016x}.json", h.finish()))
}

/// Where per-file undo histories are kept (`~/.config/e/undo`).
pub(crate) fn undo_store_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("e").join("undo")
}

/// Byte offset → (line, character) both 0-based, for LSP-style ranges.
/// `e`'s own refactorings at `offset`, as code-action items with edits against
/// `uri` in byte columns (what the rest of the editor speaks).
fn local_code_actions(
    text: &str,
    language: Language,
    offset: usize,
    uri: &str,
    root: &std::path::Path,
) -> Vec<e_lsp::CodeActionItem> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let position = |off: usize| {
        let off = off.min(text.len());
        let line = starts.partition_point(|&s| s <= off).saturating_sub(1);
        lsp_types::Position {
            line: line as u32,
            character: (off - starts[line]) as u32,
        }
    };
    let to_edits = |edits: Vec<(usize, usize, String)>| -> Vec<TextEdit> {
        edits
            .into_iter()
            .map(|(s, e, new_text)| TextEdit {
                range: lsp_types::Range {
                    start: position(s),
                    end: position(e),
                },
                new_text,
            })
            .collect()
    };
    let mut out: Vec<e_lsp::CodeActionItem> = crate::intentions::actions(text, language, offset)
        .into_iter()
        .map(|a| e_lsp::CodeActionItem {
            title: a.title,
            edits: vec![(uri.to_string(), to_edits(a.edits))],
            command: None,
            server: "e".to_string(),
        })
        .collect();
    // Refactorings that also bring a new file into being (a FormRequest class).
    out.extend(
        crate::intentions::file_actions(text, language, offset)
            .into_iter()
            .map(|a| {
                let mut edits = vec![(uri.to_string(), to_edits(a.edits))];
                let (rel, content) = a.new_file;
                edits.push((
                    path_to_uri(&root.join(rel)),
                    vec![TextEdit {
                        range: lsp_types::Range {
                            start: lsp_types::Position {
                                line: 0,
                                character: 0,
                            },
                            end: lsp_types::Position {
                                line: 0,
                                character: 0,
                            },
                        },
                        new_text: content,
                    }],
                ));
                e_lsp::CodeActionItem {
                    title: a.title,
                    edits,
                    command: None,
                    server: "e".to_string(),
                }
            }),
    );
    out
}

/// A cheap fingerprint of everything `LaravelData` is scraped from: the mtimes
/// of route, config, lang and env files, of component classes, and of the
/// directories under `resources/views` (a view appears or disappears with its
/// directory's mtime; an edit inside one changes no view name).
pub(crate) fn laravel_fingerprint(root: &std::path::Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn stat(h: &mut DefaultHasher, p: &std::path::Path) {
        if let Ok(m) = std::fs::metadata(p) {
            if let Ok(t) = m.modified() {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    p.hash(h);
                    d.as_nanos().hash(h);
                }
            }
        }
    }
    /// Files directly in `dir`, and (when `recurse`) below it, to a sane depth.
    fn files(h: &mut DefaultHasher, dir: &std::path::Path, recurse: bool, depth: u8) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if recurse && depth < 6 {
                    files(h, &p, true, depth + 1);
                }
            } else {
                stat(h, &p);
            }
        }
    }
    /// Directory mtimes only, recursively.
    fn dirs(h: &mut DefaultHasher, dir: &std::path::Path, depth: u8) {
        stat(h, dir);
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 8 {
                dirs(h, &p, depth + 1);
            }
        }
    }

    let mut h = DefaultHasher::new();
    stat(&mut h, &root.join(".env"));
    files(&mut h, &root.join("routes"), false, 0);
    files(&mut h, &root.join("config"), false, 0);
    files(&mut h, &root.join("lang"), true, 0);
    files(&mut h, &root.join("resources/lang"), true, 0);
    files(&mut h, &root.join("app/View/Components"), true, 0);
    dirs(&mut h, &root.join("resources/views"), 0);
    h.finish()
}

/// Unknown-column warnings for the query-builder calls in `text`, against the
/// live schema. Pure, so it runs on any thread. Columns are UTF-8 bytes, like
/// every other diagnostic the editor draws.
/// A warning on each controller method that hands a signed-in user a model
/// by its id without checking it is theirs (see [`e_laravel::authz`]).
fn unchecked_lookup_lint(
    text: &str,
    root: &std::path::Path,
    routes: &[e_laravel::laravel::RouteInfo],
) -> Vec<Diagnostic> {
    e_laravel::authz::Project::new(root)
        .scan(text, routes)
        .into_iter()
        .map(|f| {
            let line = text.lines().nth(f.line).unwrap_or("");
            let indent = line.len() - line.trim_start().len();
            Diagnostic {
                range: lsp_types::Range {
                    start: lsp_types::Position {
                        line: f.line as u32,
                        character: indent as u32,
                    },
                    end: lsp_types::Position {
                        line: f.line as u32,
                        character: line.encode_utf16().count() as u32,
                    },
                },
                severity: Some(lsp_types::DiagnosticSeverity::WARNING),
                source: Some("laravel".to_string()),
                message: f.message(),
                ..Default::default()
            }
        })
        .collect()
}

fn column_lint(
    text: &str,
    root: &std::path::Path,
    schema: &HashMap<String, Vec<e_db::ColumnInfo>>,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    if schema.is_empty() {
        return diags;
    }
    // Line starts once, rather than counting newlines from the top per finding.
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let position = |off: usize| {
        let off = off.min(text.len());
        let line = starts.partition_point(|&s| s <= off).saturating_sub(1);
        lsp_types::Position {
            line: line as u32,
            character: (off - starts[line]) as u32,
        }
    };
    for (start, end, col) in crate::querycomplete::column_args(text) {
        let Some(target) = crate::querycomplete::resolve_target(text, start, root) else {
            continue;
        };
        let Some(cols) = schema.get(&target.table) else {
            continue;
        };
        if cols.iter().any(|c| c.name == col) {
            continue;
        }
        diags.push(Diagnostic {
            range: lsp_types::Range {
                start: position(start),
                end: position(end),
            },
            severity: Some(lsp_types::DiagnosticSeverity::WARNING),
            source: Some("laravel".to_string()),
            message: format!("Column `{col}` not found in table `{}`", target.table),
            ..Default::default()
        });
    }
    diags
}

struct RequestResult {
    /// Whether the app exposed Clockwork, i.e. whether `queries` is meaningful.
    clockwork: bool,
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

/// Replay a request for the "verify the fix" loop and return just the pieces the
/// measurement core needs: `(status, duration_ms, queries)`.
/// Replay a request. The final flag reports whether query data was *visible* —
/// false means the app has no Clockwork, so an empty query list means "we could
/// not see" rather than "there were none". Conflating those two turns the
/// evidence table into a confident lie.
pub(crate) fn replay_for_verify(base: &str, url: &str) -> (u16, f64, Vec<(String, String)>, bool) {
    let started = now_ms();
    let rr = do_http_request(base, url);
    let ms = rr
        .time
        .trim()
        .parse::<f64>()
        .map(|secs| secs * 1000.0)
        .unwrap_or(0.0);
    if rr.clockwork {
        return (rr.status.unwrap_or(0), ms, rr.queries, true);
    }
    // No Clockwork in the app: Grove's causal chain has the queries when SQL
    // capture is on. Find our request in its timeline and take the SQL.
    if let Some(queries) = grove_queries_for_replay(base, url, started) {
        return (rr.status.unwrap_or(0), ms, queries, true);
    }
    (rr.status.unwrap_or(0), ms, rr.queries, rr.clockwork)
}

/// The SQL Grove attributed to the request we just replayed to `url`: the
/// newest timeline entry for that path that completed after `started_ms`.
/// Grove records an entry as the response finishes, so one or two short
/// retries cover the daemon being a step behind us.
fn grove_queries_for_replay(
    base: &str,
    url: &str,
    started_ms: u128,
) -> Option<Vec<(String, String)>> {
    let host = base.split("://").nth(1)?.trim_end_matches('/');
    let site = crate::grove::site_by_host(host)?;
    // With capture off Grove still has the request, just no SQL — and an
    // empty list from it would read as "no queries" rather than "can't see".
    if crate::grove::sql_capture() != Some(true) {
        return None;
    }
    let path = url
        .strip_prefix(base.trim_end_matches('/'))
        .filter(|p| p.starts_with('/'))
        .unwrap_or(url);
    for attempt in 0..4 {
        if let Some(list) = crate::grove::requests(&site.name, 5) {
            if let Some(r) = list
                .iter()
                .find(|r| r.path == path && r.epoch_ms >= started_ms)
            {
                let e = crate::grove::explain(r.id)?;
                let queries: Vec<(String, String)> = e
                    .queries
                    .into_iter()
                    .filter(|q| !crate::grove::is_connection_setup(q))
                    .map(|q| (q, String::new()))
                    .collect();
                // Nothing but the connection's setup is what Grove reports
                // when it misses the app's statements (it logs `Query`, and
                // PDO's prepared statements arrive as `Execute`): call that
                // unseen, not "no queries".
                return (!queries.is_empty()).then_some(queries);
            }
        }
        if attempt < 3 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
    None
}

/// Perform the request via the system `curl` (`-k` so Grove's private-CA HTTPS
/// works), then fetch Clockwork query data if the app exposes it.
fn do_http_request(base: &str, url: &str) -> RequestResult {
    // One file per request: the request panel, the verify loop and the
    // agent's `verify_route` can be replaying at the same time.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hdr = std::env::temp_dir().join(format!("e-req-{}-{seq}.hdr", std::process::id()));
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
                clockwork: false,
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
    let mut clockwork = false;
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
            clockwork = true;
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
        clockwork,
        status,
        time,
        body,
        queries,
        error: None,
        inertia,
    }
}

/// Files at or below this size are re-highlighted synchronously on every edit,
/// exactly as they always have been: the work is a few milliseconds and staying
/// on the UI thread keeps the colours in lockstep with the caret.
///
/// Above it, one keystroke costs far more than a frame. Measured with
/// `cargo test -p e-core --test highlight_cost -- --ignored`: 188 KB of Rust
/// takes 112 ms, 118 KB of Blade 80 ms, 312 KB of PHP 770 ms. Of the Rust
/// figure the tree-sitter parse alone is 45 ms and the query pass most of the
/// rest, so there is no arrangement of this work that is cheap enough to keep
/// inline — it has to leave the UI thread.
/// Whether `path` is one of the folders this window already has open.
fn is_open_project(roots: &[PathBuf], path: &std::path::Path) -> bool {
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let path = canon(path);
    roots.iter().any(|r| canon(r) == path)
}

/// Set while restored tabs are still being opened (see `restore_session`):
/// the session isn't saved until they are all back.
static RESTORING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const SYNC_HIGHLIGHT_LIMIT: usize = 64 * 1024;

/// Up to this size, a language [`e_core::syntax::SyntaxTree`] supports is
/// highlighted incrementally on the UI thread. Typing a line into a Laravel
/// controller measured 0.5 ms a keystroke at 18 KB (3.5 ms with a full
/// highlight) and 1.9 ms at 70 KB, the worst keystroke — one that leaves the
/// rest of the class unparseable — about half the full cost. Past this size
/// that worst case passes a frame, so larger files stay on the background
/// path above.
const INCREMENTAL_HIGHLIGHT_LIMIT: usize = 128 * 1024;

/// How long typing has to pause before a large file is re-highlighted. Long
/// enough that a burst of keystrokes schedules one parse rather than dozens,
/// short enough not to feel like the colours are lagging behind.
const HIGHLIGHT_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);

/// How long typing has to pause before the query-builder lint re-runs. It reads
/// model files from disk, so it should run when the user stops, not per key.
const LINT_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// Re-highlight a large file off the UI thread, debounced.
///
/// `gen` is the edit this job belongs to; if `hl_gen` has moved on by the time
/// the timer fires (or the result arrives) the work is dropped, so a slow parse
/// can never paint colours for text the user has already changed.
#[allow(clippy::too_many_arguments)]
fn schedule_highlight(
    cx: Scope,
    gen: u64,
    hl_gen: Rc<std::cell::Cell<u64>>,
    language: Language,
    text: String,
    head_text: Option<String>,
    highlights: Highlights,
    git_marks: GitMarks,
    doc: Rc<floem::views::editor::text_document::TextDocument>,
) {
    floem::action::exec_after(HIGHLIGHT_DEBOUNCE, move |_| {
        if hl_gen.get() != gen {
            return; // superseded while waiting out the debounce
        }
        let send = create_ext_action(
            cx,
            move |(spans, marks): (Vec<Vec<e_core::syntax::LineSpan>>, Option<Vec<_>>)| {
                if hl_gen.get() != gen {
                    return; // superseded while the worker was running
                }
                *highlights.borrow_mut() = spans;
                if let Some(marks) = marks {
                    *git_marks.borrow_mut() = marks;
                }
                doc.cache_rev().update(|r| *r += 1);
            },
        );
        std::thread::spawn(move || {
            let spans = highlight_lines(language, &text);
            let marks = head_text.map(|head| {
                let lc = text.split_inclusive('\n').count().max(1);
                git::marks(&head, &text, lc)
            });
            send((spans, marks));
        });
    });
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

impl Buffer {
    /// The text now is what's on disk.
    fn mark_saved(&self) {
        *self.saved.borrow_mut() = self.doc.text();
        self.dirty.set(false);
    }
}

/// Whether `rope` holds exactly `text`.
fn same_text(rope: &Rope, text: &str) -> bool {
    let mut at = 0;
    rope.len() == text.len()
        && rope.iter_chunks(..).all(|chunk| {
            let same = text.as_bytes()[at..at + chunk.len()] == *chunk.as_bytes();
            at += chunk.len();
            same
        })
}

/// Epoch milliseconds as `u64` (for the undo tree and its panel).
pub fn now_ms_epoch() -> u64 {
    now_ms() as u64
}

pub(crate) fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl AppState {
    pub fn new(cx: Scope, root: PathBuf) -> Self {
        // Each test's mark shows how it last ended, also in an earlier launch.
        let kept_tests = crate::testrun::load_kept(&root);
        let (tx, rx) = channel();
        let (term_tx, term_rx) = channel();
        let (agent_wake_tx, agent_wake_rx) = channel::<()>();
        Self {
            cx,
            roots: RwSignal::new(vec![root.clone()]),
            root: RwSignal::new(root),
            trusted: RwSignal::new(false),
            trust_banner_dismissed: RwSignal::new(false),
            next_id: RwSignal::new(1),
            lsp_installing: RwSignal::new(HashSet::new()),
            lsp_tx: RwSignal::new(tx),
            laravel_project: RwSignal::new(None),
            completion: Completion::new(),
            hover: HoverState::new(),
            hunk_peek: RwSignal::new(None),
            head_moved: RwSignal::new(None),
            signature: SignatureState::new(),
            laravel_fp: RwSignal::new(None),
            laravel_fp_busy: RwSignal::new(false),
            laravel_loading: RwSignal::new(false),
            laravel_reload_pending: RwSignal::new(false),
            laravel_tick: RwSignal::new(0),
            picker: Picker::new(),
            replace_confirm: RwSignal::new(None),
            next_term_id: RwSignal::new(1),
            term_tx: RwSignal::new(term_tx),
            outline: RwSignal::new(Vec::new()),
            find: FindState::new(),
            rename: RenameState::new(),
            cmd: CmdPalette::new(),
            settings: RwSignal::new(config::load_settings()),
            file_op: FileOp::new(),
            fs_rev: RwSignal::new(0),
            agents: RwSignal::new(config::load_agents()),
            diag_waiter_seq: RwSignal::new(0),
            goto: crate::editing::GotoState::new(),
            task: crate::task_palette::TaskState::new(),
            task_list: RwSignal::new(Vec::new()),
            artisan: crate::artisan_palette::ArtisanState::new(),
            recent: crate::recent::RecentState::new(),
            db: DbPanel::new(),
            agent: AgentPanel::new(agent_wake_tx, agent_wake_rx),
            debug: DebugPanel {
                open: RwSignal::new(false),
                status: RwSignal::new("idle".to_string()),
                thread: RwSignal::new(1),
                frames: RwSignal::new(Vec::new()),
                vars: RwSignal::new(Vec::new()),
                output: RwSignal::new(Vec::new()),
                breakpoints: RwSignal::new(std::collections::HashMap::new()),
                client: RwSignal::new(None),
                session: RwSignal::new(None),
            },
            tests: TestsPanel {
                open: RwSignal::new(false),
                filter: RwSignal::new(None),
                status: RwSignal::new(TddStatus::Idle),
                output: RwSignal::new(String::new()),
                results: RwSignal::new(Default::default()),
                iteration: RwSignal::new(0),
                fix_loop: RwSignal::new(false),
                save_run_queue: RwSignal::new(Vec::new()),
                save_run: RwSignal::new(None),
                outcomes: RwSignal::new(kept_tests),
            },
            grove: GrovePanel {
                site: RwSignal::new(None),
                sql_capture: RwSignal::new(None),
                panel_open: RwSignal::new(false),
                tab: RwSignal::new(crate::grove_state::GroveTab::Mail),
                mail: RwSignal::new(Vec::new()),
                hooks: RwSignal::new(Vec::new()),
                selected: RwSignal::new(None),
                detail: RwSignal::new(String::new()),
                refreshing: RwSignal::new(false),
            },
            review: ReviewPanel {
                open: RwSignal::new(false),
                changeset: RwSignal::new(e_review::Changeset::default()),
                base: RwSignal::new(None),
                turn: RwSignal::new(None),
                selected: RwSignal::new(None),
                busy: RwSignal::new(false),
                flags: RwSignal::new(Vec::new()),
                shipping: RwSignal::new(false),
                summary: RwSignal::new(None),
                evidence: RwSignal::new(None),
                evidence_busy: RwSignal::new(false),
            },
            git: GitPanel {
                panel_open: RwSignal::new(false),
                root: RwSignal::new(None),
                branch: RwSignal::new(None),
                status: RwSignal::new(Vec::new()),
                commit_msg: RwSignal::new(String::new()),
                log: RwSignal::new(Vec::new()),
                stash_count: RwSignal::new(0),
            },
            lsp: LspState {
                clients: RwSignal::new(HashMap::new()),
                health: RwSignal::new(HashMap::new()),
                progress: RwSignal::new(HashMap::new()),
                queue: RwSignal::new(Arc::new(Mutex::new(VecDeque::new()))),
                rx: RwSignal::new(Some(rx)),
            },
            term: TerminalPanel {
                sessions: RwSignal::new(Vec::new()),
                active: RwSignal::new(None),
                active2: RwSignal::new(None),
                split: RwSignal::new(false),
                focus_pane: RwSignal::new(0),
                rename_id: RwSignal::new(None),
                rename_input: RwSignal::new(String::new()),
                open: RwSignal::new(false),
                focused: RwSignal::new(false),
                tick: RwSignal::new(0),
                rx: RwSignal::new(Some(term_rx)),
                height: RwSignal::new(320.0),
            },
            runtime: RuntimePanel {
                open: RwSignal::new(false),
                lenses: RwSignal::new(Arc::new(Vec::new())),
                reqs: RwSignal::new(Vec::new()),
                expanded: RwSignal::new(None),
                polling: RwSignal::new(false),
            },
            req: RequestPanel {
                open: RwSignal::new(false),
                url: RwSignal::new(String::new()),
                status: RwSignal::new(None),
                time: RwSignal::new(String::new()),
                body: RwSignal::new(String::new()),
                queries: RwSignal::new(Vec::new()),
                error: RwSignal::new(None),
                running: RwSignal::new(false),
                inertia: RwSignal::new(None),
            },
            migrations: MigrationsPanel {
                open: RwSignal::new(false),
                list: RwSignal::new(Vec::new()),
                error: RwSignal::new(String::new()),
                loading: RwSignal::new(false),
                drift: RwSignal::new(None),
                drift_dismissed: RwSignal::new(None),
            },
            model_wizard: ModelWizardPanel {
                open: RwSignal::new(false),
                pivot: RwSignal::new(false),
                reset: RwSignal::new(0),
                doc: RwSignal::new(None),
                files: RwSignal::new(Vec::new()),
                errors: RwSignal::new(Vec::new()),
            },
            sem: SemanticSearch {
                open: RwSignal::new(false),
                query: RwSignal::new(String::new()),
                status: RwSignal::new(String::new()),
                results: RwSignal::new(Vec::new()),
                index: RwSignal::new(Rc::new(RefCell::new(crate::semantic::SemIndex::default()))),
            },
            tinker: TinkerPanel {
                open: RwSignal::new(false),
                output: RwSignal::new(String::new()),
                running: RwSignal::new(false),
            },
            verify: VerifyPanel {
                session: RwSignal::new(None),
                busy: RwSignal::new(false),
                open: RwSignal::new(false),
            },
            updater: UpdatePanel {
                info: RwSignal::new(None),
                status: RwSignal::new(crate::updater::UpdateStatus::Idle),
                notes_open: RwSignal::new(false),
            },
            log: LogPanel {
                open: RwSignal::new(false),
                lines: RwSignal::new(Vec::new()),
                read: RwSignal::new(None),
                caught: RwSignal::new(Vec::new()),
                failed_jobs_seen: RwSignal::new(None),
                failed_jobs_busy: RwSignal::new(false),
            },
            nav: Navigation {
                back: RwSignal::new(Vec::new()),
                forward: RwSignal::new(Vec::new()),
                recent_files: RwSignal::new(Vec::new()),
                pinned_tabs: RwSignal::new(HashSet::new()),
            },
            views: LaravelViews {
                map_open: RwSignal::new(false),
                map_query: RwSignal::new(String::new()),
                rel_open: RwSignal::new(false),
                rel_graph: RwSignal::new(Vec::new()),
                event_open: RwSignal::new(false),
                event_graph: RwSignal::new(Vec::new()),
                contract_open: RwSignal::new(false),
                contract: RwSignal::new(None),
                related_open: RwSignal::new(false),
                related_items: RwSignal::new(Vec::new()),
                schema_diff_open: RwSignal::new(false),
                schema_diff: RwSignal::new(Vec::new()),
            },
            editor: EditorArea {
                buffers: RwSignal::new(Vec::new()),
                active: RwSignal::new(None),
                active2: RwSignal::new(None),
                split: RwSignal::new(false),
                focused: RwSignal::new(0),
                last_edit: RwSignal::new(0),
                md_preview: RwSignal::new(false),
                diff_open: RwSignal::new(false),
                file_diff: RwSignal::new(None),
                undo_open: RwSignal::new(false),
                undo_rev: RwSignal::new(0),
                ghost_gen: RwSignal::new(0),
                close_confirm: RwSignal::new(None),
                font_size: RwSignal::new(config::load_settings().font_size),
                word_wrap: RwSignal::new(false),
                blame_rev: RwSignal::new(0),
            },
            problems: Problems {
                by_file: RwSignal::new(HashMap::new()),
                by_server: RwSignal::new(HashMap::new()),
                code_actions: RwSignal::new(Vec::new()),
                code_actions_open: RwSignal::new(false),
                waiters: RwSignal::new(Vec::new()),
            },
            ui: Chrome {
                palette_open: RwSignal::new(false),
                sidebar_open: RwSignal::new(true),
                about_open: RwSignal::new(false),
                sidebar_width: RwSignal::new(240.0),
                toasts: RwSignal::new(Vec::new()),
                confirm: RwSignal::new(None),
                settings_open: RwSignal::new(false),
            },
            project: ProjectData {
                laravel: RwSignal::new(None),
                ide_rules: RwSignal::new(Arc::new(Vec::new())),
                rules: RwSignal::new(Arc::new(e_review::rules::Rules::default())),
            },
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
        let rev = self.editor.blame_rev;
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
        self.editor.word_wrap.update(|w| *w = !*w);
    }

    /// Increase / decrease / reset the editor font size (zoom).
    pub fn zoom(&self, delta: i64) {
        let cur = self.editor.font_size.get_untracked() as i64;
        let next = (cur + delta).clamp(8, 32) as usize;
        self.editor.font_size.set(next);
        self.repaint_all_buffers();
    }

    pub fn zoom_reset(&self) {
        self.editor
            .font_size
            .set(self.settings.get_untracked().font_size);
        self.repaint_all_buffers();
    }

    /// A path handed to us from outside (Finder, Dock, `open -a e`): a folder
    /// becomes the project, a file opens in the editor.
    pub fn open_external(&self, path: PathBuf) {
        if path.is_dir() {
            // AppKit hands an app's launch arguments back to it as "open"
            // events, so `e .` delivers this window's own folder here. Opening
            // that as "another project" started a second instance — which got
            // its folder back the same way, and so on, by the hundred.
            if is_open_project(&self.roots.get_untracked(), &path) {
                return;
            }
            self.open_project(path);
        } else {
            self.open_path(path);
        }
    }

    /// Switch the colour theme (UI, editor chrome, syntax colours) and remember
    /// it. Editors re-lay out so the code colours change at once.
    pub fn set_theme(&self, id: &str) {
        let Some(t) = crate::theme::by_id(id) else {
            return;
        };
        crate::theme::set_theme(t.id);
        self.settings.update(|s| {
            s.theme = t.id.to_string();
            s.dark = t.dark;
        });
        config::set_str("theme", t.id);
        config::set_bool("dark", t.dark);
        self.repaint_all_buffers();
        self.term.tick.update(|t| *t += 1);
    }

    /// `F8`: between the current dark theme and light, and back.
    pub fn toggle_theme(&self) {
        let next = crate::theme::toggle_target();
        self.set_theme(next);
    }

    /// Switch the code font (editor, terminal, code in panels) and remember it.
    /// Empty means the system monospace.
    pub fn set_font_family(&self, name: &str) {
        let name = name.trim().to_string();
        self.settings.update(|s| s.font_family = name.clone());
        config::set_str("font_family", &name);
        crate::theme::set_mono_family(&name);
        self.repaint_all_buffers();
        // Terminal cells are measured from the font; redraw them too.
        self.term.tick.update(|t| *t += 1);
    }

    /// Whether any focus-grabbing overlay (palette, find, prompt, dialog) is
    /// open. The editor must not steal keyboard focus while one of these is up.
    pub fn any_overlay_open(&self) -> bool {
        self.ui.palette_open.get()
            || self.cmd.open.get()
            || self.picker.open.get()
            || self.find.open.get()
            || self.rename.open.get()
            || self.goto.open.get()
            || self.recent.open.get()
            || self.ui.about_open.get()
            || self.editor.close_confirm.get().is_some()
            || self.ui.confirm.with(Option::is_some)
            || self.term.rename_id.get().is_some()
    }

    /// Force a re-layout of every open buffer (e.g. after a font-size change).
    fn repaint_all_buffers(&self) {
        self.editor.buffers.with_untracked(|bs| {
            for b in bs {
                b.doc.cache_rev().update(|r| *r += 1);
            }
        });
    }

    /// A `file://` URI shown relative to the workspace root.
    pub fn rel_path(&self, uri: &str) -> String {
        rel_uri(uri, &self.root.get())
    }
}

/// Did this failure come from a binary that isn't there? The `io::Error` sits
/// under `anyhow`'s context, so walk the chain rather than matching on text.
fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|c| c.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::NotFound)
}

/// Ask every server the same question at once; answers come back in server
/// order. Latency is the slowest server's, not the sum of all of them. Call
/// from a background thread — each `f` blocks on its server.
pub(crate) fn fan_out<T: Send + Default>(
    clients: &[Arc<LspClient>],
    f: impl Fn(&LspClient) -> T + Sync,
) -> Vec<T> {
    if clients.len() == 1 {
        return vec![f(&clients[0])];
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = clients
            .iter()
            .map(|c| {
                let f = &f;
                scope.spawn(move || f(c))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// Is `program` runnable by name? A bare name is looked up on `PATH`; a path is
/// checked as given. Used to notice a server installed while `e` was running.
fn program_on_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

pub(crate) fn is_word_char(c: char) -> bool {
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

/// Remove duplicate completion items, keeping the first of each label.
pub(crate) fn dedup_by_label(
    items: Vec<lsp_types::CompletionItem>,
) -> Vec<lsp_types::CompletionItem> {
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
/// `edits` — `(start, end, new text)`, offsets into the document as it is now
/// — as one edit: the document, its undo history, the syntax tree, the git
/// marks and the language servers update once, not once per edit (a
/// formatter that touches 400 lines made 400 full passes on the UI thread).
pub(crate) fn apply_batch(
    doc: &TextDocument,
    edits: Vec<(usize, usize, String)>,
    edit_type: EditType,
) {
    use floem::views::editor::text::Document;
    let edits = batch_edits(edits);
    let mut iter = edits
        .iter()
        .map(|(s, e, text)| (Selection::region(*s, *e), text.as_str()));
    doc.edit(&mut iter, edit_type);
}

/// Edits in the order one batch takes them: by position, and inserts at the
/// same place joined in the order they came (the language server protocol
/// says that order is the order of the text; the batch would keep only one).
fn batch_edits(mut edits: Vec<(usize, usize, String)>) -> Vec<(usize, usize, String)> {
    edits.sort_by_key(|(s, e, _)| (*s, *e)); // stable: equal ones keep their order
    let mut out: Vec<(usize, usize, String)> = Vec::with_capacity(edits.len());
    for (s, e, text) in edits {
        if let Some(last) = out.last_mut() {
            if last.0 == s && last.1 == s && e == s {
                last.2.push_str(&text);
                continue;
            }
        }
        out.push((s, e, text));
    }
    out
}

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
pub(crate) fn line_indent(text: &str, offset: usize) -> String {
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

/// Walk the workspace and collect lines matching `query`.
///
/// A thin adaptor over [`crate::workspace_search`] — the same walker and
/// matcher that Replace All uses, so the list shown here is exactly the set
/// that would be rewritten.
pub(crate) fn grep_workspace(
    roots: &[PathBuf],
    query: &str,
    opts: crate::workspace_search::SearchOpts,
    max: usize,
    open: &crate::workspace_search::OpenTexts,
    cancelled: &dyn Fn() -> bool,
) -> Vec<PickerItem> {
    let display_root = roots.first().cloned().unwrap_or_default();
    crate::workspace_search::search(roots, query, opts, max, open, cancelled)
        .into_iter()
        .map(|hit| {
            let uri = path_to_uri(&hit.path);
            PickerItem {
                detail: format!("{}:{}", rel_uri(&uri, &display_root), hit.line + 1),
                label: hit.text,
                uri,
                line: hit.line,
                char: hit.col,
            }
        })
        .collect()
}

/// Display a `file://` URI relative to the workspace root.
pub(crate) fn rel_uri(uri: &str, root: &std::path::Path) -> String {
    let path = uri_to_path(uri);
    path.strip_prefix(root)
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned()
}

/// Byte offset where the identifier ending at `offset` begins.
pub(crate) fn word_start(text: &str, offset: usize) -> usize {
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

#[cfg(test)]
mod not_found_tests {
    use super::is_not_found;
    use anyhow::Context;

    #[test]
    fn sees_enoent_under_anyhow_context() {
        // The real shape: `Command::spawn` wrapped in `with_context`, exactly as
        // `LspClient::start` reports a language server that isn't installed.
        let err: anyhow::Error = std::process::Command::new("e-no-such-language-server")
            .spawn()
            .with_context(|| "spawning language server `e-no-such-language-server`")
            .unwrap_err();
        assert!(is_not_found(&err), "{err:#}");
    }

    #[test]
    fn a_crash_is_not_a_missing_binary() {
        assert!(!is_not_found(&anyhow::anyhow!(
            "server closed the connection"
        )));
    }
}

#[cfg(test)]
mod column_lint_tests {
    use super::column_lint;
    use std::collections::HashMap;

    #[test]
    fn flags_unknown_columns_at_byte_positions() {
        let text = "<?php\n// Håndter\nDB::table('users')->where('nmae', 1)->where('name', 2);\n";
        let mut schema = HashMap::new();
        schema.insert(
            "users".to_string(),
            vec![e_db::ColumnInfo {
                name: "name".into(),
                data_type: "varchar".into(),
                nullable: false,
                key: String::new(),
            }],
        );
        let diags = column_lint(text, std::path::Path::new("/nonexistent"), &schema);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = &diags[0];
        assert!(d.message.contains("nmae"));
        // Line 2 (the `å` on line 1 must not shift it), byte column of `nmae`.
        let line = text.lines().nth(2).unwrap();
        assert_eq!(d.range.start.line, 2);
        assert_eq!(d.range.start.character as usize, line.find("nmae").unwrap());
        assert_eq!(
            d.range.end.character as usize,
            line.find("nmae").unwrap() + 4
        );
    }

    #[test]
    fn nothing_without_a_schema() {
        let text = "DB::table('users')->where('x', 1);";
        assert!(column_lint(text, std::path::Path::new("/"), &HashMap::new()).is_empty());
    }
}

#[cfg(test)]
mod fingerprint_tests {
    use super::laravel_fingerprint;

    #[test]
    fn changes_when_a_route_file_or_view_dir_changes() {
        let dir = std::env::temp_dir().join(format!("e_fp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("routes")).unwrap();
        std::fs::create_dir_all(dir.join("resources/views")).unwrap();
        std::fs::write(dir.join("routes/web.php"), "<?php").unwrap();
        let a = laravel_fingerprint(&dir);
        // Same files, same fingerprint.
        assert_eq!(a, laravel_fingerprint(&dir));
        // A new route file changes it…
        std::fs::write(dir.join("routes/api.php"), "<?php").unwrap();
        let b = laravel_fingerprint(&dir);
        assert_ne!(a, b);
        // …and so does a new view directory.
        std::fs::create_dir_all(dir.join("resources/views/orders")).unwrap();
        let c = laravel_fingerprint(&dir);
        assert_ne!(b, c);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod open_project_tests {
    use super::is_open_project;
    use std::path::PathBuf;

    #[test]
    fn the_window_s_own_folder_is_not_another_project() {
        let root = std::env::temp_dir().join(format!("e-root-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let roots = vec![root.clone()];
        assert!(is_open_project(&roots, &root));
        // The same folder spelled differently (`e .` from inside it).
        assert!(is_open_project(&roots, &root.join("sub").join("..")));
        assert!(!is_open_project(&roots, &root.join("sub")));
        assert!(!is_open_project(&roots, &PathBuf::from("/")));
    }
}

#[cfg(test)]
mod saved_text_tests {
    use super::{same_text, Rope};

    #[test]
    fn text_matches_the_saved_rope_only_when_identical() {
        let saved = Rope::from("<?php\n\necho 1;\n");
        assert!(same_text(&saved, "<?php\n\necho 1;\n"));
        assert!(!same_text(&saved, "<?php\n\necho 2;\n"));
        assert!(!same_text(&saved, "<?php\n\necho 1;"));
        assert!(same_text(&Rope::from(""), ""));
    }

    #[test]
    fn a_rope_in_many_chunks_compares_across_them() {
        // Big enough that the rope splits into several leaves.
        let text = "fn line() {}\n".repeat(5000);
        let saved = Rope::from(text.as_str());
        assert!(saved.iter_chunks(..).count() > 1);
        assert!(same_text(&saved, &text));
        let mut edited = text.clone();
        edited.replace_range(40_000..40_001, "X");
        assert!(!same_text(&saved, &edited));
    }
}

#[cfg(test)]
mod batch_edit_tests {
    use super::{apply_batch, batch_edits, trailing_trim_edits};
    use floem::reactive::Scope;
    use floem::views::editor::core::editor::EditType;
    use floem::views::editor::text::Document;
    use floem::views::editor::text_document::TextDocument;
    use std::cell::Cell;
    use std::rc::Rc;

    /// The edits one at a time, last first: what applying them used to do.
    fn one_by_one(text: &str, edits: &[(usize, usize, &str)]) -> String {
        let mut sorted = edits.to_vec();
        sorted.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        let mut out = text.to_string();
        for (s, e, t) in sorted {
            out.replace_range(s..e, t);
        }
        out
    }

    fn doc_with(text: &str) -> (Rc<TextDocument>, Rc<Cell<usize>>) {
        let doc = Rc::new(TextDocument::new(Scope::new(), text));
        let updates = Rc::new(Cell::new(0));
        let seen = updates.clone();
        doc.clone().add_on_update(move |_| seen.set(seen.get() + 1));
        (doc, updates)
    }

    #[test]
    fn a_batch_gives_the_same_text_in_one_update() {
        let text = "fn a(){\n  x+1\n}\nfn b(){ y }\n";
        let edits = [
            (6, 6, " "),
            (10, 13, "x + 1;"),
            (0, 2, "pub fn"),
            (21, 23, "{\n    y\n}"),
        ];
        let (doc, updates) = doc_with(text);
        apply_batch(
            &doc,
            edits
                .iter()
                .map(|(s, e, t)| (*s, *e, t.to_string()))
                .collect(),
            EditType::InsertChars,
        );
        assert_eq!(doc.text().to_string(), one_by_one(text, &edits));
        assert_eq!(updates.get(), 1, "one update for the whole batch");
    }

    #[test]
    fn inserts_at_one_place_keep_their_order() {
        let edits = vec![
            (3, 3, "b".to_string()),
            (0, 1, "X".to_string()),
            (3, 3, "c".to_string()),
            (3, 5, "D".to_string()),
        ];
        assert_eq!(
            batch_edits(edits.clone()),
            vec![
                (0, 1, "X".to_string()),
                (3, 3, "bc".to_string()),
                (3, 5, "D".to_string()),
            ]
        );
        let (doc, _) = doc_with("abcdefg");
        apply_batch(&doc, edits, EditType::InsertChars);
        assert_eq!(doc.text().to_string(), "XbcbcDfg");
    }

    #[test]
    fn trimming_is_one_edit_too() {
        let text = "a  \nb\t\nc";
        let (edits, nl) = trailing_trim_edits(text);
        let mut batch: Vec<(usize, usize, String)> = edits
            .into_iter()
            .map(|(s, e)| (s, e, String::new()))
            .collect();
        if nl {
            batch.push((text.len(), text.len(), "\n".into()));
        }
        let (doc, updates) = doc_with(text);
        apply_batch(&doc, batch, EditType::Delete);
        assert_eq!(doc.text().to_string(), "a\nb\nc\n");
        assert_eq!(updates.get(), 1);
    }
}

/// The window's furniture: palettes, the sidebar, About and Settings, messages and the confirm dialog.
///
/// `Copy`, like `AppState`: signals and handles only.
#[derive(Clone, Copy)]
pub struct Chrome {
    /// Is the command palette open?
    pub palette_open: floem::reactive::RwSignal<bool>,
    /// Whether the left sidebar (file explorer) is visible.
    pub sidebar_open: floem::reactive::RwSignal<bool>,
    /// Whether the About dialog is open.
    pub about_open: floem::reactive::RwSignal<bool>,
    /// Draggable panel widths (pixels).
    pub sidebar_width: floem::reactive::RwSignal<f64>,
    /// Messages on screen (see [`crate::toast`]).
    pub toasts: floem::reactive::RwSignal<Vec<crate::toast::Toast>>,
    /// A pending "are you sure?" question (see [`crate::dialogs::Confirm`]).
    pub confirm: floem::reactive::RwSignal<Option<crate::dialogs::Confirm>>,
    pub settings_open: floem::reactive::RwSignal<bool>,
}
