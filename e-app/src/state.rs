//! Shared, reactive application state.
//!
//! `AppState` is `Copy` (every field is a Floem signal or `Scope`), so it can
//! be handed to as many view closures as needed without cloning ceremony.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

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

use e_agent::{AgentClient, ChatState, Streaming};
use e_core::buffer::{self, FileInfo};
use e_core::git;
use e_core::language::Language;
use e_core::syntax::highlight_lines;
use e_lsp::{path_to_uri, uri_to_path, LspClient};
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
use crate::runtime::RuntimeReq;
use crate::session::{self, SessionData};
use crate::styling::{
    build_diag_lines, BpMarks, BracketMarks, DiagLines, FindMarks, FindSpan, GitMarks, Highlights,
    StopLine,
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
    /// Lazily-loaded approximate row counts per table (shown in the tree).
    pub table_counts: RwSignal<HashMap<String, i64>>,
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
    /// Execute these statements as one transaction, then reload the table.
    Transaction(Vec<String>),
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
    /// Lines carrying a debug breakpoint (0-based).
    pub bp_marks: BpMarks,
    /// The line the debugger is currently stopped on (0-based), if any.
    pub stop_line: StopLine,
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

/// Queue of diagnostics waiting to be applied on the UI thread.
pub type DiagQueue = Arc<Mutex<VecDeque<(String, PublishDiagnosticsParams)>>>;

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
    /// Log entries for the in-flight submit, appended to the log on success.
    pub pending_log: RwSignal<Vec<WriteLogEntry>>,
    /// A destructive / non-local / submit action awaiting confirmation, if any.
    pub confirm: RwSignal<Option<DbConfirm>>,
    /// A `:param` prompt awaiting values, and the last-entered values to prefill.
    pub params: RwSignal<Option<DbParams>>,
    pub param_last: RwSignal<HashMap<String, String>>,
    /// Staged cell edits `(row, col) -> edit` and staged row deletions
    /// `row -> pk`, for transactional editing (Submit / Revert).
    pub pending_edits: RwSignal<HashMap<(usize, usize), PendingEdit>>,
    pub pending_deletes: RwSignal<HashMap<usize, RowPk>>,
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
    /// Cached live DB schema `table -> columns`, for Eloquent completion.
    pub schema_cache: RwSignal<std::collections::HashMap<String, Vec<e_db::ColumnInfo>>>,
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
            pending_log: RwSignal::new(Vec::new()),
            confirm: RwSignal::new(None),
            params: RwSignal::new(None),
            param_last: RwSignal::new(HashMap::new()),
            pending_edits: RwSignal::new(HashMap::new()),
            pending_deletes: RwSignal::new(HashMap::new()),
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
            schema_cache: RwSignal::new(std::collections::HashMap::new()),
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
    /// Merged view: uri -> diagnostics from every server. Readers use this.
    pub diagnostics: RwSignal<HashMap<String, Vec<Diagnostic>>>,
    /// Per-server storage behind [`Self::diagnostics`], keyed `(server id, uri)`,
    /// so two servers publishing for the same file don't clobber each other.
    pub diag_by_server: RwSignal<HashMap<(String, String), Vec<Diagnostic>>>,
    /// Cached “is this a Laravel project?” (decides whether laravel-lsp runs).
    laravel_project: RwSignal<Option<bool>>,
    /// Channel the LSP reader thread pushes diagnostics into.
    /// Wake notification only — payloads travel in [`Self::diag_queue`], because a
    /// signal-per-message coalesces (only the last value survives a frame) and
    /// two servers publishing for the same file would drop one of them.
    diag_tx: RwSignal<Sender<()>>,
    /// Pending `(server id, params)` from the LSP reader threads.
    pub diag_queue: RwSignal<DiagQueue>,
    /// Receiver, taken once by the UI to build a reactive signal.
    pub diag_rx: RwSignal<Option<Receiver<()>>>,
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
    /// A planned workspace replace awaiting confirmation (`Some` shows the dialog).
    pub replace_confirm: RwSignal<Option<ReplaceConfirm>>,
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
    pub(crate) next_term_id: RwSignal<u64>,
    /// Terminal-rename prompt: the session id being renamed, and its input.
    pub term_rename_id: RwSignal<Option<u64>>,
    pub term_rename_input: RwSignal<String>,
    pub terminal_open: RwSignal<bool>,
    /// Whether the terminal panel currently has keyboard focus.
    pub terminal_focused: RwSignal<bool>,
    /// Bumped whenever the terminal produces output, to trigger a repaint.
    pub term_tick: RwSignal<u64>,
    pub(crate) term_tx: RwSignal<Sender<()>>,
    pub term_rx: RwSignal<Option<Receiver<()>>>,
    /// Document outline of the active buffer.
    pub outline: RwSignal<Vec<OutlineItem>>,
    /// Find-in-file state.
    pub find: FindState,
    /// Local rename state.
    pub rename: RenameState,
    /// A rename that has been planned but not yet written. `Some` shows the
    /// preview; nothing touches a file until it is confirmed.
    pub rename_plan: RwSignal<Option<crate::rename_preview::RenamePlan>>,
    /// A planned class move awaiting confirmation.
    pub move_plan: RwSignal<Option<crate::move_class::MovePlan>>,
    /// Whether the rename prompt is currently asking for a new fully-qualified
    /// class name rather than a symbol — it reuses the same input.
    pub rename_is_move: RwSignal<bool>,
    pub rename_busy: RwSignal<bool>,
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

    /// The configured agents (from config.json or built-in defaults).
    pub agents: RwSignal<Vec<AgentConfig>>,

    /// Wake sender cloned into each forwarder thread to nudge the UI drain.

    /// Draggable panel widths (pixels).
    pub sidebar_width: RwSignal<f64>,
    /// Height of the bottom terminal panel (drag-resizable).
    pub term_height: RwSignal<f64>,

    /// File-comparison panel: `(left name, right name, diff lines)` when open.
    pub file_diff: RwSignal<Option<(String, String, Vec<e_core::git::DiffLine>)>>,
    /// LSP code actions (quick fixes / refactors) offered at the cursor, and
    /// whether the picker is open.
    pub code_actions: RwSignal<Vec<e_lsp::CodeActionItem>>,
    pub code_actions_open: RwSignal<bool>,

    // ---- Tinker scratchpad ---------------------------------------------
    pub tinker_open: RwSignal<bool>,
    pub tinker_output: RwSignal<String>,
    pub tinker_running: RwSignal<bool>,

    // ---- Laravel architecture map --------------------------------------
    pub map_open: RwSignal<bool>,
    pub map_query: RwSignal<String>,

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

    // ---- Event dispatch graph ------------------------------------------
    pub event_open: RwSignal<bool>,
    pub event_graph: RwSignal<Vec<crate::events::EventNode>>,

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
    /// The in-progress "verify the fix" session, if any (see [`crate::verify`]).
    pub verify_session: RwSignal<Option<crate::verify::VerifySession>>,
    /// A verify measurement (baseline or after) is in flight.
    pub verify_busy: RwSignal<bool>,
    /// Whether the verify panel is open (single source of truth for visibility).
    pub verify_open: RwSignal<bool>,
    // ---- Agent session review (see [`crate::review`]) ----
    pub review_open: RwSignal<bool>,
    pub review_changeset: RwSignal<e_review::Changeset>,
    /// Commit the session started at; `None` = review everything uncommitted.
    pub review_base: RwSignal<Option<String>>,
    pub review_selected: RwSignal<Option<String>>,
    pub review_busy: RwSignal<bool>,
    /// Automated diff inspections over the changeset.
    pub review_flags: RwSignal<Vec<e_review::flags::Flag>>,
    /// A branch/commit/PR is being created.
    pub review_shipping: RwSignal<bool>,
    /// The agent's written summary of the session (used as the PR description);
    /// set over the sync socket by `{"method":"review_summary"}`.
    pub review_summary: RwSignal<Option<String>>,
    /// Measured evidence for the routes this changeset touches, once replayed.
    /// `None` until someone asks for it — replaying hits the running app.
    pub review_evidence: RwSignal<Option<Vec<e_verify::RouteEvidence>>>,
    pub review_evidence_busy: RwSignal<bool>,

    // ---- Step-debugging (DAP session) ----------------------------------
    pub debug_open: RwSignal<bool>,
    pub debug_status: RwSignal<String>,
    pub debug_thread: RwSignal<i64>,
    pub debug_frames: RwSignal<Vec<crate::debug::DebugFrame>>,
    pub debug_vars: RwSignal<Vec<crate::debug::DebugVar>>,
    pub debug_output: RwSignal<Vec<String>>,
    /// Breakpoints keyed by file path → 1-based line numbers.
    pub debug_breakpoints: RwSignal<std::collections::HashMap<String, Vec<u32>>>,
    /// The live adapter client, if a session is running.
    pub debug_client: RwSignal<Option<std::sync::Arc<e_dap::DapClient>>>,

    /// Generation counter debouncing inline AI-completion requests.
    pub ghost_gen: RwSignal<u64>,

    // ---- Laravel log tail ----------------------------------------------
    pub log_open: RwSignal<bool>,
    pub log_lines: RwSignal<Vec<String>>,

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
    /// Parsed per-test results from the last run, when the runner could produce
    /// them. Empty for runners that can't write JUnit XML.
    pub tdd_results: RwSignal<crate::testrun::TestRun>,
    pub tdd_iteration: RwSignal<usize>,
    /// When true, a failing run asks the agent to fix and re-runs on apply.
    pub tdd_loop: RwSignal<bool>,

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
    /// Everything behind the database panel (⌘3).
    pub db: DbPanel,
    /// Everything behind the AI agent panel (⌘L).
    pub agent: AgentPanel,
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

/// Byte offset → (line, character) both 0-based, for LSP-style ranges.
fn offset_to_lc(text: &str, off: usize) -> (u32, u32) {
    let up = &text[..off.min(text.len())];
    let line = up.bytes().filter(|b| *b == b'\n').count() as u32;
    let col = up
        .rsplit('\n')
        .next()
        .map(|s| s.chars().count())
        .unwrap_or(0) as u32;
    (line, col)
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
    let rr = do_http_request(base, url);
    let ms = rr
        .time
        .trim()
        .parse::<f64>()
        .map(|secs| secs * 1000.0)
        .unwrap_or(0.0);
    (rr.status.unwrap_or(0), ms, rr.queries, rr.clockwork)
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
const SYNC_HIGHLIGHT_LIMIT: usize = 64 * 1024;

/// How long typing has to pause before a large file is re-highlighted. Long
/// enough that a burst of keystrokes schedules one parse rather than dozens,
/// short enough not to feel like the colours are lagging behind.
const HIGHLIGHT_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);

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
        let (agent_wake_tx, agent_wake_rx) = channel::<()>();
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
            diag_queue: RwSignal::new(Arc::new(Mutex::new(VecDeque::new()))),
            diag_by_server: RwSignal::new(HashMap::new()),
            laravel_project: RwSignal::new(None),
            completion: Completion::new(),
            hover: HoverState::new(),
            signature: SignatureState::new(),
            laravel: RwSignal::new(None),
            picker: Picker::new(),
            replace_confirm: RwSignal::new(None),
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
            rename_plan: RwSignal::new(None),
            move_plan: RwSignal::new(None),
            rename_is_move: RwSignal::new(false),
            rename_busy: RwSignal::new(false),
            last_edit: RwSignal::new(0),
            md_preview: RwSignal::new(false),
            cmd: CmdPalette::new(),
            diff_open: RwSignal::new(false),
            settings: RwSignal::new(config::load_settings()),
            sidebar_open: RwSignal::new(true),
            file_op: FileOp::new(),
            fs_rev: RwSignal::new(0),
            about_open: RwSignal::new(false),
            agents: RwSignal::new(config::load_agents()),
            sidebar_width: RwSignal::new(240.0),
            term_height: RwSignal::new(320.0),
            file_diff: RwSignal::new(None),
            code_actions: RwSignal::new(Vec::new()),
            code_actions_open: RwSignal::new(false),
            tinker_open: RwSignal::new(false),
            tinker_output: RwSignal::new(String::new()),
            tinker_running: RwSignal::new(false),
            map_open: RwSignal::new(false),
            map_query: RwSignal::new(String::new()),
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
            event_open: RwSignal::new(false),
            event_graph: RwSignal::new(Vec::new()),
            contract_open: RwSignal::new(false),
            contract: RwSignal::new(None),
            related_open: RwSignal::new(false),
            related_items: RwSignal::new(Vec::new()),
            runtime_open: RwSignal::new(false),
            runtime_reqs: RwSignal::new(Vec::new()),
            runtime_expanded: RwSignal::new(None),
            runtime_polling: RwSignal::new(false),
            verify_session: RwSignal::new(None),
            verify_busy: RwSignal::new(false),
            verify_open: RwSignal::new(false),
            review_open: RwSignal::new(false),
            review_changeset: RwSignal::new(e_review::Changeset::default()),
            review_base: RwSignal::new(None),
            review_selected: RwSignal::new(None),
            review_busy: RwSignal::new(false),
            review_flags: RwSignal::new(Vec::new()),
            review_shipping: RwSignal::new(false),
            review_summary: RwSignal::new(None),
            review_evidence: RwSignal::new(None),
            review_evidence_busy: RwSignal::new(false),
            debug_open: RwSignal::new(false),
            debug_status: RwSignal::new("idle".to_string()),
            debug_thread: RwSignal::new(1),
            debug_frames: RwSignal::new(Vec::new()),
            debug_vars: RwSignal::new(Vec::new()),
            debug_output: RwSignal::new(Vec::new()),
            debug_breakpoints: RwSignal::new(std::collections::HashMap::new()),
            debug_client: RwSignal::new(None),
            ghost_gen: RwSignal::new(0),
            log_open: RwSignal::new(false),
            log_lines: RwSignal::new(Vec::new()),
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
            tdd_results: RwSignal::new(Default::default()),
            tdd_iteration: RwSignal::new(0),
            tdd_loop: RwSignal::new(false),
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
            db: DbPanel::new(),
            agent: AgentPanel::new(agent_wake_tx, agent_wake_rx),
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
        let sig = self.file_diff;
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
        self.file_diff.set(None);
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

    /// Add a caret one line above the top-most caret / below the bottom-most
    /// caret at the same column (column editing). `delta` is -1 (above) or +1.
    fn add_cursor_line(&self, delta: i64) {
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
        let regions = sel.regions().to_vec();
        if regions.is_empty() {
            return;
        }
        // Anchor on the extreme caret in the direction we're growing.
        let anchor = if delta < 0 {
            regions.iter().map(|r| r.max()).min()
        } else {
            regions.iter().map(|r| r.max()).max()
        };
        let Some(anchor) = anchor else {
            return;
        };
        let (line, col) = editor.offset_to_line_col(anchor);
        let target = line as i64 + delta;
        if target < 0 {
            return;
        }
        let line_count = buf.doc.text().to_string().lines().count();
        if target as usize >= line_count {
            return;
        }
        let new_offset = editor.offset_of_line_col(target as usize, col);
        let mut s = sel.clone();
        s.add_region(SelRegion::new(new_offset, new_offset, None));
        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(s), None, None));
    }

    /// Add a caret on the line above (column editing).
    pub fn add_cursor_above(&self) {
        self.add_cursor_line(-1);
    }

    /// Add a caret on the line below (column editing).
    pub fn add_cursor_below(&self) {
        self.add_cursor_line(1);
    }

    // ---- Livewire ------------------------------------------------------

    /// Completion items for a `wire:model` value, from the component's class.
    pub(crate) fn livewire_property_items(
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
    pub(crate) fn goto_inertia_page(&self) -> bool {
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
    pub(crate) fn livewire_goto(&self) -> bool {
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
        let mut ok = self.rewrite_file(&comp.class_file, new_class);
        if let Ok(view_src) = std::fs::read_to_string(&comp.view_file) {
            let new_view = crate::livewire::view_rename(&view_src, old, new);
            ok &= self.rewrite_file(&comp.view_file, new_view);
        }
        // Only claim success if every write actually landed; on failure
        // `rewrite_file` has already told the user what went wrong.
        if ok {
            Self::notify(&format!("Renamed Livewire property `{old}` → `{new}`"));
        }
        ok
    }

    /// Replace a file's contents, editing the open buffer (undoable) if it is
    /// open, otherwise writing to disk. Returns whether the change landed — a
    /// disk write can fail (full/read-only disk), and callers must not report
    /// success when it did.
    fn rewrite_file(&self, path: &std::path::Path, content: String) -> bool {
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
            true
        } else {
            let ok = Self::write_or_notify(path, &content);
            self.fs_rev.update(|r| *r += 1);
            ok
        }
    }

    /// Run `work` on a background thread and deliver its result to `on_done` on
    /// the UI thread. Replaces the `create_ext_action` + `thread::spawn`
    /// boilerplate repeated across the app.
    pub(crate) fn spawn_bg<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        on_done: impl FnOnce(T) + 'static,
    ) {
        let send = create_ext_action(self.cx, on_done);
        std::thread::spawn(move || send(work()));
    }

    /// Write user data to disk, surfacing failures as a notification instead of
    /// swallowing them. Returns whether the write succeeded.
    pub(crate) fn write_or_notify(path: &std::path::Path, content: &str) -> bool {
        match buffer::write(path, content) {
            Ok(()) => true,
            Err(e) => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
                Self::notify(&format!("Could not write {name}: {e}"));
                false
            }
        }
    }

    /// The textual rename that predates the language-server path.
    ///
    /// Whole-word matches in the active buffer only, so it also renames inside
    /// strings and comments — which is why it is the fallback and not the
    /// default.
    fn rename_textually(&self, word: &str, new_name: &str) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let occ = whole_word_occurrences(&text, word);
        if occ.is_empty() {
            return;
        }
        let edits: Vec<(Selection, String)> = occ
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), new_name.to_string()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    // ---- Move class ------------------------------------------------------

    /// The project's PSR-4 maps, from `composer.json`.
    fn psr4(&self) -> Vec<crate::move_class::Psr4Root> {
        let root = self.root.get_untracked();
        std::fs::read_to_string(root.join("composer.json"))
            .map(|t| crate::move_class::psr4_roots(&t))
            .unwrap_or_default()
    }

    /// Open the prompt for moving the active file's class.
    pub fn open_move_class(&self) {
        let roots = self.psr4();
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let project = self.root.get_untracked();
        let Ok(rel) = path.strip_prefix(&project) else {
            Self::notify("that file is outside the project");
            return;
        };
        let Some(fqn) = crate::move_class::fqn_for(&roots, rel) else {
            Self::notify("no PSR-4 mapping covers that file");
            return;
        };
        let r = self.rename;
        r.word.set(fqn.clone());
        r.new_name.set(fqn);
        self.rename_is_move.set(true);
        r.open.set(true);
    }

    /// Work out what moving the active class would do, and show it.
    pub fn plan_move_class(&self, new_fqn: &str) {
        self.rename_is_move.set(false);
        let roots = self.psr4();
        let old_fqn = self.rename.word.get_untracked();
        if new_fqn.trim().is_empty() || new_fqn == old_fqn {
            return;
        }
        let project = self.root.get_untracked();
        // The same walker search uses, so ignore rules apply and `vendor/`
        // never reaches the planner.
        let files: Vec<std::path::PathBuf> = crate::workspace_search::search(
            std::slice::from_ref(&project),
            crate::move_class::class_of(&old_fqn),
            Default::default(),
            5000,
        )
        .into_iter()
        .filter_map(|h| h.path.strip_prefix(&project).ok().map(|p| p.to_path_buf()))
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

        let app = *self;
        let plan = crate::move_class::plan_move(&roots, &old_fqn, new_fqn, &files, |p| {
            let abs = project.join(p);
            app.buffers
                .with_untracked(|bs| {
                    bs.iter()
                        .find(|b| b.file.path.as_deref() == Some(abs.as_path()))
                        .map(|b| b.doc.text().to_string())
                })
                .or_else(|| std::fs::read_to_string(&abs).ok())
        });
        match plan {
            Some(p) => self.move_plan.set(Some(p)),
            None => Self::notify("that move has no PSR-4 destination"),
        }
    }

    /// Carry out a planned move: rewrite the referrers, then move the file.
    pub fn confirm_move_class(&self) {
        let Some(plan) = self.move_plan.get_untracked() else {
            return;
        };
        self.move_plan.set(None);
        let project = self.root.get_untracked();
        let mut failed: Vec<String> = Vec::new();

        for r in plan.referrers.iter() {
            let abs = project.join(&r.path);
            if std::fs::write(&abs, &r.updated).is_err() {
                failed.push(r.path.display().to_string());
            }
        }
        // The moved file last: if a referrer write failed, the class is at least
        // still where every reference expects it.
        if let Some(moved) = &plan.moved {
            let to = project.join(&plan.to);
            if let Some(dir) = to.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(&to, &moved.updated).is_err() {
                failed.push(plan.to.display().to_string());
            } else {
                let _ = std::fs::remove_file(project.join(&plan.from));
            }
        }

        self.fs_rev.update(|r| *r += 1);
        self.check_external_changes();
        if failed.is_empty() {
            Self::notify(&format!(
                "moved to {} ({})",
                plan.to.display(),
                plan.summary()
            ));
        } else {
            Self::notify(&format!("move could not write: {}", failed.join(", ")));
        }
    }

    pub fn cancel_move_class(&self) {
        self.move_plan.set(None);
    }

    /// Ask the language server what this rename would change, and show it.
    ///
    /// Returns whether it took over. When there is no server, or it declines,
    /// the caller falls back to the textual rename that was here before — worse,
    /// but better than nothing.
    fn plan_rename_via_lsp(&self, word: &str, new_name: &str) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return false;
        };
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let (word, new_name) = (word.to_string(), new_name.to_string());
        let name_for_request = new_name.clone();
        let app = *self;

        self.rename_busy.set(true);
        let busy = self.rename_busy;
        let out = self.rename_plan;
        let send = create_ext_action(
            self.cx,
            move |edits: Option<Vec<(String, Vec<lsp_types::TextEdit>)>>| {
                busy.set(false);
                let Some(edits) = edits.filter(|e| !e.is_empty()) else {
                    // The server had nothing for us; fall back rather than
                    // leaving the user with a rename that silently did nothing.
                    app.rename_textually(&word, &new_name);
                    return;
                };
                let plan = crate::rename_preview::plan(&word, &new_name, &edits, |p| {
                    // Prefer the open buffer: renaming against a stale copy of a
                    // file the user has edited would preview one thing and write
                    // another.
                    app.buffers
                        .with_untracked(|bs| {
                            bs.iter()
                                .find(|b| b.file.path.as_deref() == Some(p))
                                .map(|b| b.doc.text().to_string())
                        })
                        .or_else(|| std::fs::read_to_string(p).ok())
                });
                if plan.is_empty() {
                    app.rename_textually(&word, &new_name);
                    return;
                }
                out.set(Some(plan));
            },
        );
        std::thread::spawn(move || {
            send(
                client
                    .rename(&uri, line as u32, col as u32, &name_for_request)
                    .ok(),
            );
        });
        true
    }

    /// Write a planned rename to every file it touches.
    pub fn confirm_rename(&self) {
        let Some(plan) = self.rename_plan.get_untracked() else {
            return;
        };
        self.rename_plan.set(None);
        let mut written = 0usize;
        let mut failed: Vec<String> = Vec::new();
        for file in &plan.files {
            // Open buffers go through the document, so the change is undoable
            // and the editor stays in sync; the rest are written to disk.
            let open = self.buffers.with_untracked(|bs| {
                bs.iter()
                    .find(|b| b.file.path.as_deref() == Some(file.path.as_path()))
                    .cloned()
            });
            match open {
                Some(buf) => {
                    let text = buf.doc.text().to_string();
                    let updated = crate::rename_preview::apply_edits(&text, &file.edits);
                    if updated != text {
                        let len = text.len();
                        buf.doc
                            .edit_single(Selection::region(0, len), &updated, EditType::Other);
                        written += 1;
                    }
                }
                None => {
                    let Ok(text) = std::fs::read_to_string(&file.path) else {
                        failed.push(file.path.display().to_string());
                        continue;
                    };
                    let updated = crate::rename_preview::apply_edits(&text, &file.edits);
                    if updated != text && std::fs::write(&file.path, updated).is_err() {
                        failed.push(file.path.display().to_string());
                    } else {
                        written += 1;
                    }
                }
            }
        }
        self.fs_rev.update(|r| *r += 1);
        self.check_external_changes();
        if !failed.is_empty() {
            Self::notify(&format!("rename could not write: {}", failed.join(", ")));
        }
        eprintln!("e: renamed in {written} file(s)");
    }

    pub fn cancel_rename(&self) {
        self.rename_plan.set(None);
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
            self.rename_is_move.set(false);
            return;
        }
        if self.rename_is_move.get_untracked() {
            self.plan_move_class(&new_name);
            return;
        }
        // Livewire property rename spans the class *and* the view.
        let prop = word.trim_start_matches('$');
        if self.livewire_rename(prop, new_name.trim_start_matches('$')) {
            return;
        }
        // Ask the language server first: it knows which occurrences are the
        // symbol and which are a word that happens to match inside a string or
        // a comment, and it sees the whole workspace rather than this buffer.
        if self.plan_rename_via_lsp(&word, &new_name) {
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
                if let Some(uri) = b.uri.as_ref() {
                    for client in self.lsp_clients_for(b.file.language) {
                        client.did_save(uri, &text);
                    }
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
        if self.lsp_language_id(buf.file.language).is_none() {
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
        if self.lsp_language_id(buf.file.language).is_none() {
            outline.set(Vec::new());
            return;
        }
        self.spawn_bg(
            move || {
                client
                    .document_symbols(&uri)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(name, kind, line, ch, depth)| OutlineItem {
                        name,
                        kind,
                        line,
                        char: ch,
                        depth,
                    })
                    .collect::<Vec<_>>()
            },
            move |items| outline.set(items),
        );
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

    // ---- Agent panel ----------------------------------------------------

    /// The currently selected agent's config.
    pub fn current_agent(&self) -> Option<AgentConfig> {
        let id = self.agent.current.get_untracked();
        self.agents
            .with_untracked(|list| list.iter().find(|a| a.id == id).cloned())
            .or_else(|| self.agents.with_untracked(|l| l.first().cloned()))
    }

    /// Toggle the agent panel, launching the agent on first open.
    pub fn toggle_agent(&self) {
        let open = self.agent.open.get_untracked();
        if open {
            self.agent.open.set(false);
        } else {
            self.agent.open.set(true);
            if self.use_native_agent() {
                if self.agent.native_client.get_untracked().is_none() {
                    self.start_native_agent();
                }
                // Focus the composer once the panel has actually become visible
                // (a synchronous focus request while still hidden is dropped).
                let st = *self;
                floem::action::exec_after(std::time::Duration::from_millis(30), move |_| {
                    st.agent.focus_pulse.update(|x| *x += 1);
                });
            } else if self.agent.term.get_untracked().is_none() {
                self.start_agent();
            }
            self.agent.focus_pulse.update(|x| *x += 1);
        }
    }

    /// Whether the current agent should use the native (elyra RPC) chat panel
    /// rather than a terminal PTY. Only elyra speaks the RPC protocol today.
    pub fn use_native_agent(&self) -> bool {
        // Opt-in (experimental, off by default): only Elyra speaks the RPC
        // protocol the native chat panel renders. Every other agent, and Elyra
        // when the toggle is off, uses the terminal (PTY) panel.
        if !self.settings.get_untracked().native_agent {
            return false;
        }
        let Some(agent) = self.current_agent() else {
            return false;
        };
        let program = agent.command.split_whitespace().next().unwrap_or("");
        agent.id == "elyra" || program == "elyra" || program.rsplit('/').next() == Some("elyra")
    }

    /// Spawn `elyra --mode rpc` and wire its event stream into the chat state.
    /// The reader thread pushes decoded events onto a shared queue and nudges a
    /// wake channel; the UI-thread drain (installed in `app`) applies them.
    pub fn start_native_agent(&self) {
        let Some(agent) = self.current_agent() else {
            return;
        };
        self.mark_review_session();
        let cwd = if agent.cwd.trim().is_empty() {
            self.root.get_untracked()
        } else {
            PathBuf::from(&agent.cwd)
        };
        // Run through the user's login shell so the full PATH (nvm/npm/Grove
        // node, etc.) is available — a GUI app launched from Finder/Dock inherits
        // only a minimal PATH, so `elyra` (and the `node` its shebang needs)
        // wouldn't be found by a direct spawn. This mirrors the terminal agent.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let cmdline = format!("{} --mode rpc", agent.command.trim());
        let shell_args = vec!["-lc".to_string(), cmdline];

        match AgentClient::spawn(&shell, &shell_args, &cwd, &[]) {
            Ok((client, rx)) => {
                self.agent.chat.set(ChatState::new());
                let queue = self.agent.events.get_untracked();
                let wake = self.agent.wake_tx.get_untracked();
                std::thread::Builder::new()
                    .name("e-agent-forward".into())
                    .spawn(move || {
                        while let Ok(ev) = rx.recv() {
                            if let Ok(mut q) = queue.lock() {
                                q.push_back(ev);
                            }
                            if wake.send(()).is_err() {
                                break;
                            }
                        }
                    })
                    .ok();
                self.agent.native_client.set(Some(Rc::new(client)));
            }
            Err(e) => {
                eprintln!("e: native agent failed: {e:#}");
                self.agent.chat.update(|c| {
                    c.apply(e_agent::AgentEvent::Error {
                        message: format!("Could not start the agent: {e:#}"),
                    })
                });
            }
        }
    }

    /// Send the composer text as a prompt (starting the agent if needed). While
    /// the agent is streaming, the message is steered rather than rejected.
    pub fn send_native_prompt(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self.agent.native_client.get_untracked().is_none() {
            self.start_native_agent();
        }
        // Capture whether a turn was already running *before* we add this
        // message (push_user flips `running` on).
        let busy = self.agent.chat.with_untracked(|c| c.running);
        // Always show the user's message, even if the agent isn't available.
        self.agent.chat.update(|c| c.push_user(text));
        let Some(client) = self.agent.native_client.get_untracked() else {
            self.agent.chat.update(|c| {
                c.apply(e_agent::AgentEvent::Error {
                    message: "The agent isn't running — check that `elyra` is installed and on your PATH."
                        .to_string(),
                })
            });
            self.agent.composer.set(String::new());
            return;
        };
        let streaming = busy.then_some(Streaming::Steer);
        if let Err(e) = client.prompt(text, streaming) {
            eprintln!("e: agent prompt failed: {e:#}");
        }
        self.agent.composer.set(String::new());
    }

    /// Send the composer's contents and clear it. Both the network send and the
    /// document edit are deferred out of the current event: mutating the editor's
    /// document from inside its own key handler is a reentrant borrow and aborts
    /// across the objc FFI boundary.
    pub fn send_composer(&self) {
        let Some(doc) = self.agent.composer_doc.get_untracked() else {
            return;
        };
        let text = doc.text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let st = *self;
        let text = text.trim().to_string();
        floem::action::exec_after(std::time::Duration::ZERO, move |_| {
            st.send_native_prompt(&text);
            // Clear via InsertChars (replace-all with empty), matching the SQL
            // console. EditType::Delete here left the buffer's revision history
            // inconsistent and the *next* keystroke aborted in xi-rope's
            // Subset::transform (mk_new_rev).
            let len = doc.text().len();
            if len > 0 {
                doc.edit_single(Selection::region(0, len), "", EditType::InsertChars);
                // Reset the editor's cursor to the (now empty) start. Without
                // this the cursor keeps its old offset, past the end of the
                // cleared buffer, and the next keystroke's delta aborts in
                // xi-rope's Subset::transform.
                if let Some(editor) = st.agent.composer_editor.get_untracked() {
                    editor.cursor.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(0)),
                        None,
                        None,
                    ));
                }
            }
        });
    }

    /// Abort the current native agent turn.
    pub fn native_agent_abort(&self) {
        if let Some(client) = self.agent.native_client.get_untracked() {
            let _ = client.abort();
        }
    }

    /// Restart the native agent in a fresh session.
    pub fn native_agent_new_session(&self) {
        if let Some(client) = self.agent.native_client.get_untracked() {
            let _ = client.new_session();
        }
        self.agent.chat.set(ChatState::new());
    }

    /// (Re)start the selected agent in a fresh PTY.
    pub fn start_agent(&self) {
        let Some(agent) = self.current_agent() else {
            eprintln!("e: no agent configured");
            return;
        };
        // Remember where this session starts, so "Session review" can show
        // exactly what it changed.
        self.mark_review_session();
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
            Ok(t) => self.agent.term.set(Some(Rc::new(RefCell::new(t)))),
            Err(e) => eprintln!("e: agent '{}' failed: {e:#}", agent.name),
        }
    }

    /// Switch to a different agent and restart the panel with it.
    pub fn select_agent(&self, id: &str) {
        self.agent.current.set(id.to_string());
        config::save_default_agent(id);
        // Tear down whichever backend was running for the previous agent.
        if let Some(client) = self.agent.native_client.get_untracked() {
            client.shutdown();
        }
        self.agent.native_client.set(None);
        self.agent.term.set(None);
        if self.use_native_agent() {
            self.start_native_agent();
        } else {
            self.start_agent();
        }
        self.agent.focus_pulse.update(|x| *x += 1);
    }

    pub fn restart_agent(&self) {
        if self.use_native_agent() {
            if let Some(client) = self.agent.native_client.get_untracked() {
                client.shutdown();
            }
            self.agent.native_client.set(None);
            self.start_native_agent();
        } else {
            self.agent.term.set(None);
            self.start_agent();
        }
        self.agent.focus_pulse.update(|x| *x += 1);
    }

    pub fn agent_input(&self, bytes: &[u8]) {
        if let Some(t) = self.agent.term.get_untracked() {
            t.borrow_mut().write(bytes);
        }
    }

    /// Send a prompt to the AI agent panel (opening/starting it if needed) and
    /// focus it. Used by "Explain with agent" / "Fix with AI" affordances.
    pub fn send_to_agent(&self, prompt: &str) {
        if self.use_native_agent() {
            if !self.agent.open.get_untracked() {
                self.agent.open.set(true);
            }
            self.send_native_prompt(prompt);
            self.agent.focus_pulse.update(|x| *x += 1);
            return;
        }
        let just_started = self.agent.term.get_untracked().is_none();
        if !self.agent.open.get_untracked() {
            self.agent.open.set(true);
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
            state.agent.focus_pulse.update(|x| *x += 1);
        });
    }

    pub fn agent_runs(&self) -> Vec<Vec<e_term::Run>> {
        self.agent
            .term
            .get_untracked()
            .map(|t| t.borrow().snapshot_runs())
            .unwrap_or_default()
    }

    pub fn agent_cursor(&self) -> Option<(usize, usize)> {
        self.agent
            .term
            .get_untracked()
            .and_then(|t| t.borrow().cursor())
    }

    pub fn resize_agent(&self, rows: usize, cols: usize) {
        if let Some(t) = self.agent.term.get_untracked() {
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
        self.new_untitled_with(String::new());
    }

    /// Create an untitled buffer seeded with `initial` content and focus it.
    pub fn new_untitled_with(&self, initial: String) {
        let id = self.next_id.get_untracked();
        self.next_id.set(id + 1);

        let highlights: Highlights =
            Rc::new(RefCell::new(highlight_lines(Language::PlainText, &initial)));
        let doc = Rc::new(TextDocument::new(self.cx, initial.clone()));
        doc.auto_indent.set(true);
        let dirty = RwSignal::new(!initial.is_empty());
        let undo = Rc::new(RefCell::new(e_core::undotree::UndoTree::new(&initial)));
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
                // Record into the undo tree, then release the borrow *before*
                // bumping undo_rev: the signal update runs effects synchronously
                // (e.g. the undo-tree view re-reads `undo.borrow()`), so holding
                // the mutable borrow across it double-borrows and aborts. Note a
                // `borrow_mut()` temporary in an `if` condition lives until the
                // end of the whole `if`, hence the explicit `let`.
                let recorded =
                    !undo_nav.get() && undo.borrow_mut().record(&text, now_ms() as u64, 700);
                if recorded {
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
            bp_marks: Rc::new(RefCell::new(Default::default())),
            stop_line: Rc::new(RefCell::new(None)),
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
            ghost: RwSignal::new(None),
            large: false,
            encoding: RwSignal::new("UTF-8".to_string()),
            lint: Rc::new(RefCell::new(Vec::new())),
            analysis: Rc::new(RefCell::new(Vec::new())),
            undo,
            undo_nav,
            tab_width: self.settings.get_untracked().tab_width,
            editorconfig: e_core::editorconfig::EditorConfig::default(),
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
        // The silent startup check is throttled so frequent restarts don't burn
        // the unauthenticated GitHub rate limit (60/hr); a manual check
        // (`announce_up_to_date`) always runs.
        let now = now_ms_epoch() / 1000;
        if !announce_up_to_date && now.saturating_sub(config::last_update_check()) < 6 * 3600 {
            return;
        }
        config::set_last_update_check(now);

        self.update_status.set(UpdateStatus::Checking);
        let info_sig = self.update_info;
        let status_sig = self.update_status;
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
        for text in &data.untitled {
            self.new_untitled_with(text.clone());
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
        // Unsaved untitled scratch buffers with content, so a quit doesn't lose
        // them (they're recreated as untitled tabs on restore).
        let untitled: Vec<String> = buffers
            .iter()
            .filter(|b| b.file.path.is_none())
            .map(|b| b.doc.text().to_string())
            .filter(|t| !t.trim().is_empty())
            .collect();
        let data = SessionData {
            open,
            untitled,
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
        self.agent.log.update(|v| {
            v.push(entry);
            let len = v.len();
            if len > 500 {
                v.drain(0..len - 500);
            }
        });
    }

    pub fn toggle_agent_log(&self) {
        self.agent.log_open.update(|o| *o = !*o);
    }

    /// Record where the agent is currently looking (a ghost marker).
    pub fn set_agent_mark(&self, path: PathBuf, line: usize) {
        self.agent.mark.set(Some((path, line)));
    }

    pub fn jump_to_agent_mark(&self) {
        if let Some((path, line)) = self.agent.mark.get_untracked() {
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
        self.agent.edit.set(Some(AgentEdit { path, segs, reply }));
    }

    /// Apply the accepted hunks of the current proposal.
    pub fn agent_edit_apply(&self) {
        let Some(edit) = self.agent.edit.get_untracked() else {
            return;
        };
        self.agent.edit.set(None);
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
        } else if !Self::write_or_notify(&edit.path, &out) {
            // Don't tell the agent it succeeded when the file wasn't written.
            let _ = edit
                .reply
                .send(serde_json::json!({"ok": false, "error": "could not write file"}));
            return;
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

    // ---- Event dispatch graph ------------------------------------------

    pub fn toggle_event_graph(&self) {
        let open = !self.event_open.get_untracked();
        self.event_open.set(open);
        if open {
            let root = self.root.get_untracked();
            let sig = self.event_graph;
            let send =
                create_ext_action(self.cx, move |g: Vec<crate::events::EventNode>| sig.set(g));
            std::thread::spawn(move || send(crate::events::dispatch_map(&root)));
        }
    }

    /// Caret on a dispatched event class jumps to a listener.
    pub(crate) fn goto_event(&self) -> bool {
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
        let word = word_at(&text, offset);
        if word.is_empty()
            || !word
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            return false;
        }
        let root = self.root.get_untracked();
        if let Some(node) = crate::events::dispatch_map(&root)
            .into_iter()
            .find(|n| n.event == word)
        {
            if let Some((_, Some(file))) = node.listeners.into_iter().find(|(_, f)| f.is_some()) {
                self.jump_to(&path_to_uri(&file), 0, 0);
                return true;
            }
        }
        false
    }

    pub fn open_event_file(&self, path: PathBuf) {
        self.jump_to(&path_to_uri(&path), 0, 0);
    }

    // ---- Code generation -----------------------------------------------

    /// Generate an Eloquent model from the table currently open in the database
    /// panel — fillable, casts, and relationships from the live schema + FKs.
    pub fn generate_model_from_table(&self) {
        let Some(table) = self.db.result_table.get_untracked() else {
            Self::notify("Open a table in the database panel first");
            return;
        };
        if !crate::codegen::valid_table(&table) {
            return;
        }
        let root = self.root.get_untracked();
        let name = crate::codegen::model_name(&table);
        let file = root.join(format!("app/Models/{name}.php"));
        if file.is_file() {
            Self::notify(&format!("{name} already exists — opening it"));
            self.open_path(file);
            return;
        }
        let state = *self;
        let file2 = file.clone();
        let send = create_ext_action(self.cx, move |content: Option<String>| match content {
            Some(c) => {
                if let Some(dir) = file2.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if std::fs::write(&file2, c).is_ok() {
                    state.open_path(file2.clone());
                    Self::notify(&format!("Generated {}", file2.display()));
                }
            }
            None => Self::notify("Could not read the table schema"),
        });
        std::thread::spawn(move || {
            let content = e_db::from_env(&root)
                .and_then(|cfg| e_db::connect(&cfg).ok())
                .and_then(|conn| {
                    let cols = e_db::columns(&conn, &table).ok()?;
                    let fks = e_db::foreign_keys(&conn).unwrap_or_default();
                    Some(crate::codegen::generate_model(&table, &cols, &fks))
                });
            send(content);
        });
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
            .db
            .schema_cache
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
        let schema = self.db.schema_cache.get_untracked();
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
        let schema = self.db.schema_cache.get_untracked();
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

    pub fn agent_edit_cancel(&self) {
        if let Some(edit) = self.agent.edit.get_untracked() {
            self.agent.edit.set(None);
            let _ = edit
                .reply
                .send(serde_json::json!({"ok": true, "applied": 0, "cancelled": true}));
        }
    }

    /// Offer Laravel completions if the cursor is inside a helper string.
    /// Returns true when the context was handled (so we skip the LSP).
    pub(crate) fn try_laravel_completion(&self, buffer_id: u64) -> bool {
        // When the official Laravel server is running it owns these contexts —
        // it's project-accurate, understands more of them (middleware, Inertia,
        // validation rules) and is maintained upstream. Our built-in helpers stay
        // as the fallback for when it isn't installed or is switched off.
        if self.laravel_lsp_running() {
            return false;
        }
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
        let spec = lsp_registry::primary_spec(language, self.is_laravel_project())?;
        self.lsp_clients.with(|m| m.get(spec.id).cloned())
    }

    /// The language server for the active buffer, if running.
    pub fn lsp_for_active(&self) -> Option<Arc<LspClient>> {
        self.lsp_for_language(self.active_buffer()?.file.language)
    }

    /// Record one server's diagnostics for a file and rebuild the merged view.
    /// Keyed per server, so intelephense and laravel-lsp can both report on the
    /// same file without overwriting each other.
    pub fn publish_diagnostics(&self, server_id: &str, uri: &str, diags: Vec<Diagnostic>) {
        let key = (server_id.to_string(), uri.to_string());
        self.diag_by_server.update(|m| {
            if diags.is_empty() {
                m.remove(&key);
            } else {
                m.insert(key, diags);
            }
        });
        let merged: Vec<Diagnostic> = self.diag_by_server.with_untracked(|m| {
            m.iter()
                .filter(|((_, u), _)| u == uri)
                .flat_map(|(_, d)| d.iter().cloned())
                .collect()
        });
        self.diagnostics.update(|map| {
            if merged.is_empty() {
                map.remove(uri);
            } else {
                map.insert(uri.to_string(), merged.clone());
            }
        });
        self.apply_diagnostics_to_buffer(uri, &merged);
    }

    /// Is the workspace a Laravel project? Cheap fs check, cached for the
    /// session (opening another project spawns a new window).
    pub(crate) fn is_laravel_project(&self) -> bool {
        if let Some(v) = self.laravel_project.get_untracked() {
            return v;
        }
        let st = self.settings.get_untracked();
        let v = st.laravel && st.laravel_lsp && laravel::is_laravel(&self.root.get_untracked());
        self.laravel_project.set(Some(v));
        v
    }

    /// Is the official Laravel language server up?
    pub(crate) fn laravel_lsp_running(&self) -> bool {
        self.lsp_clients.with(|m| m.contains_key("laravel-lsp"))
    }

    /// The LSP `languageId` for `language`, or `None` when nothing handles it.
    pub(crate) fn lsp_language_id(&self, language: Language) -> Option<&'static str> {
        lsp_registry::language_id(language, self.is_laravel_project())
    }

    /// Every *running* server for `language` — requests that can merge (completion,
    /// code actions, document sync) go to all of them.
    pub fn lsp_clients_for(&self, language: Language) -> Vec<Arc<LspClient>> {
        let specs = lsp_registry::server_specs(language, self.is_laravel_project());
        self.lsp_clients.with(|m| {
            specs
                .iter()
                .filter_map(|s| m.get(s.id).cloned())
                .collect::<Vec<_>>()
        })
    }

    /// Every running server for the active buffer's language.
    pub fn lsp_all_for_active(&self) -> Vec<Arc<LspClient>> {
        match self.active_buffer() {
            Some(b) => self.lsp_clients_for(b.file.language),
            None => Vec::new(),
        }
    }

    /// Start (or reuse) the language server for `language`.
    fn ensure_lsp(&self, language: Language) -> Option<Arc<LspClient>> {
        let specs = lsp_registry::server_specs(language, self.is_laravel_project());
        // Start every server this language wants; a missing optional one (e.g.
        // laravel-lsp not installed) just means fewer features, never an error.
        for spec in &specs {
            self.ensure_one_lsp(spec);
        }
        specs
            .first()
            .and_then(|s| self.lsp_clients.with(|m| m.get(s.id).cloned()))
    }

    /// Start one server if it isn't already running (and hasn't already failed).
    fn ensure_one_lsp(&self, spec: &lsp_registry::ServerSpec) -> Option<Arc<LspClient>> {
        if let Some(client) = self.lsp_clients.with(|m| m.get(spec.id).cloned()) {
            return Some(client);
        }
        if self.lsp_failed.with(|f| f.contains(spec.id)) {
            return None;
        }
        let tx = self.diag_tx.get();
        let queue = self.diag_queue.get_untracked();
        // Tag diagnostics with the server that produced them so two servers on
        // the same file don't overwrite each other.
        let server_id = spec.id.to_string();
        let handler: e_lsp::DiagnosticsHandler = Box::new(move |p| {
            if let Ok(mut q) = queue.lock() {
                q.push_back((server_id.clone(), p));
            }
            let _ = tx.send(());
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
        if let (Some(lang_id), Some(uri)) = (self.lsp_language_id(language), uri.as_ref()) {
            // Starts every server for this language; each needs its own didOpen.
            self.ensure_lsp(language);
            for client in self.lsp_clients_for(language) {
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
            // Generation of the newest scheduled background highlight. Bumped on
            // every edit so a job that finishes after a later keystroke is dropped
            // instead of painting stale colours.
            let hl_gen: Rc<std::cell::Cell<u64>> = Rc::new(std::cell::Cell::new(0));
            doc.clone().add_on_update(move |_| {
                dirty.set(true);
                app.last_edit.set(now_ms());
                let text = doc.text().to_string();
                if !undo_nav.get() {
                    let now = now_ms() as u64;
                    // Do the recording (and any save) inside a scope so the
                    // mutable borrow is dropped before undo_rev.update runs the
                    // undo-tree view's effect, which borrows `undo` again.
                    let recorded = {
                        let mut t = undo.borrow_mut();
                        let rec = t.record(&text, now, 700);
                        if rec {
                            if let Some(p) = &undo_path {
                                t.maybe_save(p, now);
                            }
                        }
                        rec
                    };
                    if recorded {
                        app.undo_rev.update(|r| *r += 1);
                    }
                }
                if !large {
                    if text.len() <= SYNC_HIGHLIGHT_LIMIT {
                        *highlights.borrow_mut() = highlight_lines(language, &text);
                        if let Some(head) = &head_text {
                            let lc = text.split_inclusive('\n').count().max(1);
                            *git_marks.borrow_mut() = git::marks(head, &text, lc);
                        }
                    } else {
                        // Too expensive to keep inline — see SYNC_HIGHLIGHT_LIMIT.
                        // The previous colours stay on screen until the new ones
                        // land, which reads as a brief lag rather than a freeze.
                        let gen = hl_gen.get() + 1;
                        hl_gen.set(gen);
                        schedule_highlight(
                            app.cx,
                            gen,
                            hl_gen.clone(),
                            language,
                            text.clone(),
                            head_text.clone(),
                            highlights.clone(),
                            git_marks.clone(),
                            doc.clone(),
                        );
                    }
                }
                doc.cache_rev().update(|r| *r += 1);

                if let Some(uri) = uri.as_ref() {
                    let clients = app.lsp_clients_for(language);
                    if !clients.is_empty() {
                        let v = version.get() + 1;
                        version.set(v);
                        for client in clients {
                            client.did_change_full(uri, v, &text);
                        }
                    }
                }
                // Trigger completion (LSP + snippets + Laravel helpers).
                app.autocomplete_after_edit(id);
                // Request an inline AI suggestion (debounced; no-op if disabled).
                app.request_ghost(id);
                // Laravel query-builder lint (unknown columns).
                app.refresh_lint(id);
            });
        }

        let disk_mtime = file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());

        // Per-file EditorConfig (indent / trailing whitespace / final newline).
        let ec = e_core::editorconfig::resolve(&canon);
        let ec_tab = ec
            .effective_tab_width()
            .unwrap_or_else(|| self.settings.get_untracked().tab_width)
            .clamp(1, 16);
        let buf = Buffer {
            id,
            file,
            doc,
            dirty,
            highlights,
            diag_lines: Rc::new(RefCell::new(Vec::new())),
            git_marks,
            bp_marks: Rc::new(RefCell::new(Default::default())),
            stop_line: Rc::new(RefCell::new(None)),
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
            ghost: RwSignal::new(None),
            large,
            encoding: RwSignal::new(encoding),
            lint: Rc::new(RefCell::new(Vec::new())),
            analysis: Rc::new(RefCell::new(Vec::new())),
            undo,
            undo_nav,
            tab_width: ec_tab,
            editorconfig: ec,
        };
        self.buffers.update(|bs| bs.push(buf));
        self.focused_active().set(Some(id));
        self.sync_bp_marks(&canon.to_string_lossy());
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
            for client in self.lsp_clients_for(lang) {
                client.did_close(&uri);
            }
        }
    }

    pub fn active_buffer(&self) -> Option<Buffer> {
        let active = self.focused_active_id()?;
        self.buffers
            .with(|bs| bs.iter().find(|b| b.id == active).cloned())
    }

    /// Format through the project's Pint, if it has one. Returns whether it ran.
    fn format_active_with_pint(&self, buf: &Buffer) -> bool {
        if !matches!(buf.file.language, Language::Php) {
            return false;
        }
        let Some(path) = buf.file.path.clone() else {
            return false;
        };
        let root = self.root.get_untracked();
        let text = buf.doc.text().to_string();
        let Some(formatted) = crate::phptools::pint_format(&root, &path, &text) else {
            return false;
        };
        // One whole-document edit, so it lands in the undo tree as a single step.
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let end = text.len();
        editor.doc().edit_single(
            floem::views::editor::core::selection::Selection::region(0, end),
            &formatted,
            floem_editor_core::editor::EditType::Other,
        );
        true
    }

    /// Format the active buffer in place.
    ///
    /// A Laravel project's formatting is whatever Pint says it is — that is what
    /// CI enforces — so when the project ships Pint it wins over the language
    /// server, which formats PHP to its own taste and would fight it.
    pub fn format_active(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if self.format_active_with_pint(&buf) {
            return;
        }
        if self.lsp_language_id(buf.file.language).is_none() {
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

    /// Request LSP code actions (quick fixes / refactors like extract) at the
    /// cursor or selection, and open the picker.
    pub fn request_code_actions(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let clients = self.lsp_all_for_active();
        let (Some(uri), Some(editor)) = (buf.uri.clone(), buf.editor.get_untracked()) else {
            return;
        };
        if clients.is_empty() {
            Self::notify("No language server for this file");
            return;
        }
        let cursor = editor.cursor.get_untracked();
        let (sl, sc, el, ec) = if let CursorMode::Insert(sel) = cursor.mode.clone() {
            match sel.regions().first() {
                Some(r) => {
                    let (al, ac) = editor.offset_to_line_col(r.min());
                    let (bl, bc) = editor.offset_to_line_col(r.max());
                    (al as u32, ac as u32, bl as u32, bc as u32)
                }
                None => (0, 0, 0, 0),
            }
        } else {
            let (l, c) = editor.offset_to_line_col(cursor.offset());
            (l as u32, c as u32, l as u32, c as u32)
        };
        let diags = self
            .diagnostics
            .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
        let list_sig = self.code_actions;
        let open_sig = self.code_actions_open;
        self.spawn_bg(
            move || {
                // Offer every server's fixes together (intelephense quick fixes
                // plus laravel-lsp's framework fixes).
                let mut list = Vec::new();
                for client in &clients {
                    list.extend(
                        client
                            .code_actions(&uri, sl, sc, el, ec, &diags)
                            .unwrap_or_default(),
                    );
                }
                list
            },
            move |list: Vec<e_lsp::CodeActionItem>| {
                if list.is_empty() {
                    Self::notify("No code actions here");
                    return;
                }
                list_sig.set(list);
                open_sig.set(true);
            },
        );
    }

    /// Apply the chosen code action's edits to the matching open buffers.
    pub fn apply_code_action(&self, index: usize) {
        self.code_actions_open.set(false);
        let Some(item) = self.code_actions.with_untracked(|l| l.get(index).cloned()) else {
            return;
        };
        let buffers = self.buffers.get_untracked();
        for (uri, edits) in &item.edits {
            let Some(buf) = buffers
                .iter()
                .find(|b| b.uri.as_deref() == Some(uri.as_str()))
            else {
                continue; // v1: only edits to already-open files are applied
            };
            let Some(editor) = buf.editor.get_untracked() else {
                continue;
            };
            let mut offs: Vec<(usize, usize, String)> = edits
                .iter()
                .map(|e| {
                    let s = editor.offset_of_line_col(
                        e.range.start.line as usize,
                        e.range.start.character as usize,
                    );
                    let en = editor.offset_of_line_col(
                        e.range.end.line as usize,
                        e.range.end.character as usize,
                    );
                    (s, en, e.new_text.clone())
                })
                .collect();
            offs.sort_by_key(|b| std::cmp::Reverse(b.0));
            for (s, en, text) in offs {
                buf.doc
                    .edit_single(Selection::region(s, en), &text, EditType::InsertChars);
            }
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
        // EditorConfig `trim_trailing_whitespace` overrides the global setting.
        let ec = self
            .active_buffer()
            .map(|b| b.editorconfig)
            .unwrap_or_default();
        let trim = ec
            .trim_trailing_whitespace
            .unwrap_or_else(|| self.settings.get_untracked().trim_on_save);
        if trim {
            self.trim_active();
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.as_ref() else {
            self.save_active_as();
            return;
        };
        // EditorConfig `insert_final_newline`: ensure exactly one trailing \n.
        if ec.insert_final_newline == Some(true) {
            let t = buf.doc.text().to_string();
            if !t.is_empty() && !t.ends_with('\n') {
                let len = t.len();
                buf.doc.edit_single(
                    Selection::caret(len),
                    "\n",
                    floem::views::editor::core::editor::EditType::InsertChars,
                );
            }
        }
        let text = buf.doc.text().to_string();
        match buffer::write_with_encoding(path, &text, &buf.encoding.get_untracked()) {
            Ok(()) => {
                buf.dirty.set(false);
                buf.disk_changed.set(false);
                Self::refresh_disk_mtime(&buf);
                self.fs_rev.update(|r| *r += 1);
                self.load_blame(buf.id);
                // PHPStan reads the file, so it can only run once it is written.
                self.run_phpstan(buf.id);
                self.request_inlay_hints(buf.id);
                eprintln!("e: saved {}", path.display());
                if let Some(uri) = buf.uri.as_ref() {
                    for client in self.lsp_clients_for(buf.file.language) {
                        client.did_save(uri, &text);
                    }
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
        // Merge the LSP diagnostics with our Laravel query lint.
        let mut all = diags.to_vec();
        all.extend(buf.lint.borrow().iter().cloned());
        all.extend(buf.analysis.borrow().iter().cloned());
        *buf.diag_lines.borrow_mut() = build_diag_lines(&all, &text);
        buf.doc.cache_rev().update(|r| *r += 1);
    }

    /// Run PHPStan over one file and attach its findings to the buffer.
    ///
    /// Off the UI thread, and a no-op unless the project actually ships PHPStan
    /// with a config — analysing a project that never opted in would be slow and
    /// wrong. Analysing the single saved file rather than the whole project
    /// keeps it to something worth doing on every save.
    pub fn run_phpstan(&self, buffer_id: u64) {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        if !matches!(buf.file.language, Language::Php) {
            return;
        }
        let (Some(path), Some(uri)) = (buf.file.path.clone(), buf.uri.clone()) else {
            return;
        };
        let root = self.root.get_untracked();
        let (Some(bin), true) = (
            crate::phptools::phpstan_binary(&root),
            crate::phptools::has_phpstan_config(&root),
        ) else {
            return;
        };

        let app = *self;
        let send = create_ext_action(self.cx, move |diags: Vec<Diagnostic>| {
            let Some(buf) = app
                .buffers
                .with_untracked(|bs| bs.iter().find(|b| b.uri.as_deref() == Some(&uri)).cloned())
            else {
                return;
            };
            if *buf.analysis.borrow() == diags {
                return;
            }
            *buf.analysis.borrow_mut() = diags;
            let lsp = app
                .diagnostics
                .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
            app.apply_diagnostics_to_buffer(&uri, &lsp);
        });

        std::thread::spawn(move || {
            let out = std::process::Command::new(&bin)
                .args(["analyse", "--error-format=json", "--no-progress"])
                .arg(&path)
                .current_dir(&root)
                .output();
            let Ok(out) = out else { return };
            let text = String::from_utf8_lossy(&out.stdout);
            let Some(report) = crate::phptools::parse_phpstan(&text) else {
                return;
            };
            for e in &report.errors {
                eprintln!("e: phpstan: {e}");
            }
            send(crate::phptools::diagnostics_for(&report, &path));
        });
    }

    /// Recompute Laravel query-builder lint (unknown columns) for a buffer and
    /// re-render its diagnostics. Cheap no-op without a live schema.
    pub fn refresh_lint(&self, buffer_id: u64) {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        if buf.large || !matches!(buf.file.language, Language::Php | Language::Blade) {
            return;
        }
        let root = self.root.get_untracked();
        let text = buf.doc.text().to_string();
        let mut diags: Vec<Diagnostic> = Vec::new();
        self.db.schema_cache.with_untracked(|schema| {
            if schema.is_empty() {
                return;
            }
            for (start, end, col) in crate::querycomplete::column_args(&text) {
                let Some(target) = crate::querycomplete::resolve_target(&text, start, &root) else {
                    continue;
                };
                if let Some(cols) = schema.get(&target.table) {
                    if !cols.iter().any(|c| c.name == col) {
                        let (sl, sc) = offset_to_lc(&text, start);
                        let (el, ec) = offset_to_lc(&text, end);
                        diags.push(Diagnostic {
                            range: lsp_types::Range {
                                start: lsp_types::Position {
                                    line: sl,
                                    character: sc,
                                },
                                end: lsp_types::Position {
                                    line: el,
                                    character: ec,
                                },
                            },
                            severity: Some(lsp_types::DiagnosticSeverity::WARNING),
                            source: Some("laravel".to_string()),
                            message: format!(
                                "Column `{col}` not found in table `{}`",
                                target.table
                            ),
                            ..Default::default()
                        });
                    }
                }
            }
        });
        let changed = *buf.lint.borrow() != diags;
        *buf.lint.borrow_mut() = diags;
        if changed {
            if let Some(uri) = buf.uri.clone() {
                let lsp = self
                    .diagnostics
                    .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
                self.apply_diagnostics_to_buffer(&uri, &lsp);
            }
        }
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
) -> Vec<PickerItem> {
    let display_root = roots.first().cloned().unwrap_or_default();
    crate::workspace_search::search(roots, query, opts, max)
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
