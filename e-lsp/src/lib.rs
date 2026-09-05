//! A small LSP client over stdio.
//!
//! Spawns a language server (e.g. `intelephense --stdio`), performs the
//! `initialize`/`initialized` handshake on a background thread, and streams
//! text document notifications. Everything the server pushes back — diagnostics,
//! workspace edits, messages, progress, readiness, and its own exit — is
//! delivered as a [`ServerEvent`] to one callback.
//!
//! ## Nothing here blocks the caller on the server
//!
//! [`LspClient::start`] returns as soon as the process is spawned. Until the
//! handshake completes, document notifications only update the client's mirror
//! of each document, which is then opened on the server in one go; other
//! notifications are queued in order; requests wait (within their own timeout).
//! All writes to the server go through a writer thread, so a server that is slow
//! to read its stdin never stalls the thread that called us. Dropping the client
//! asks the server to exit and lets a background thread make sure it does.
//!
//! ## Columns are bytes on this side of the API
//!
//! The editor counts columns in UTF-8 bytes; LSP counts them in UTF-16 code
//! units unless the server agrees to something else. The document mirror lets
//! this client convert at the boundary in both directions, so callers never see
//! a server column. Positions in files that aren't open are converted against
//! the file on disk.
//!
//! The mirror also pays for itself on every keystroke: when the server supports
//! incremental sync, `didChange` carries only the bytes that changed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, unbounded, Sender};
use lsp_types::{
    CompletionItem, CompletionResponse, CompletionTextEdit, Diagnostic, GotoDefinitionResponse,
    Hover, HoverContents, MarkedString, Position, PublishDiagnosticsParams, Range, TextEdit,
};
use serde_json::{json, Value};

/// A flattened document symbol: `(name, kind, line, character, depth)`.
pub type DocumentSymbol = (String, i64, u32, u32, usize);

/// Active signature info for the signature-help popup.
#[derive(Clone, Debug)]
pub struct SignatureInfo {
    pub label: String,
    /// Character range of the active parameter within `label`, if known.
    pub active: Option<(u32, u32)>,
}

/// A code action with its concrete text edits, grouped by document URI.
///
/// Some actions carry no edits, only a `command`; running it through
/// [`LspClient::execute_command`] makes the server send the edits back as a
/// `workspace/applyEdit` ([`ServerEvent::ApplyEdit`]).
#[derive(Clone, Debug)]
pub struct CodeActionItem {
    pub title: String,
    pub edits: Vec<(String, Vec<TextEdit>)>,
    /// The `command` to execute when `edits` is empty.
    pub command: Option<Value>,
    /// The program that offered it (see [`LspClient::name`]).
    pub server: String,
}

/// How a server counts `character` in positions (LSP 3.17 `positionEncoding`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    /// The protocol default, and the only one most servers speak.
    Utf16,
    Utf32,
}

impl PositionEncoding {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "utf-8" => Some(Self::Utf8),
            "utf-16" => Some(Self::Utf16),
            "utf-32" => Some(Self::Utf32),
            _ => None,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Utf8 => 0,
            Self::Utf16 => 1,
            Self::Utf32 => 2,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Utf8,
            2 => Self::Utf32,
            _ => Self::Utf16,
        }
    }
}

/// Severity of a `window/showMessage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageLevel {
    Error,
    Warning,
    Info,
    Log,
}

impl MessageLevel {
    fn from_lsp(v: Option<&Value>) -> Self {
        match v.and_then(|t| t.as_u64()) {
            Some(1) => Self::Error,
            Some(2) => Self::Warning,
            Some(3) => Self::Info,
            _ => Self::Log,
        }
    }
}

/// Something the server pushed at us. Delivered on a background thread;
/// positions are already in editor (byte) columns and URIs are in canonical form.
#[derive(Debug)]
pub enum ServerEvent {
    /// The handshake completed; requests are being answered from now on.
    Ready,
    /// The process started but `initialize` failed. The client is useless.
    InitFailed(String),
    Diagnostics(PublishDiagnosticsParams),
    /// `workspace/applyEdit`: the server wants these edits applied.
    ApplyEdit {
        label: Option<String>,
        edits: Vec<(String, Vec<TextEdit>)>,
    },
    /// `window/showMessage` or `window/showMessageRequest`.
    Message {
        level: MessageLevel,
        text: String,
    },
    /// `$/progress` (e.g. "Indexing 42%"). `done` marks the end of a job.
    Progress {
        title: String,
        message: Option<String>,
        percentage: Option<u32>,
        done: bool,
    },
    /// The server process went away without us asking it to.
    Exited,
}

/// Callback invoked (on a background thread) for each [`ServerEvent`].
pub type EventHandler = Box<dyn Fn(ServerEvent) + Send + Sync>;

// ---- Column conversion ------------------------------------------------------

/// The server-side column for a byte column within `line`.
pub fn col_to_lsp(line: &str, byte_col: usize, enc: PositionEncoding) -> u32 {
    if enc == PositionEncoding::Utf8 {
        return byte_col.min(line.len()) as u32;
    }
    let mut n = 0u32;
    for (i, ch) in line.char_indices() {
        if i >= byte_col {
            break;
        }
        n += match enc {
            PositionEncoding::Utf16 => ch.len_utf16() as u32,
            _ => 1,
        };
    }
    n
}

/// The byte column within `line` for a server-side column. A column that lands
/// inside a character rounds up to the next boundary; past the end clamps.
pub fn col_from_lsp(line: &str, col: u32, enc: PositionEncoding) -> usize {
    if enc == PositionEncoding::Utf8 {
        let c = (col as usize).min(line.len());
        // A UTF-8 server sends byte columns; still never split a character.
        return (0..=c)
            .rev()
            .find(|&i| line.is_char_boundary(i))
            .unwrap_or(0);
    }
    let mut n = 0u32;
    for (i, ch) in line.char_indices() {
        if n >= col {
            return i;
        }
        n += match enc {
            PositionEncoding::Utf16 => ch.len_utf16() as u32,
            _ => 1,
        };
    }
    line.len()
}

/// The smallest single replacement turning `old` into `new`: `(start, end)` in
/// `old`'s bytes and the replacement text. `None` when they're equal. Both ends
/// sit on character boundaries.
pub fn minimal_change<'n>(old: &str, new: &'n str) -> Option<(usize, usize, &'n str)> {
    if old == new {
        return None;
    }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut prefix = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    // The first differing byte may sit inside a character in one string and not
    // the other; back up to where both agree it's a boundary. Every byte before
    // `prefix` is shared, so the two strings agree about boundaries there.
    while prefix > 0 && !(old.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let max_suffix = ob.len().min(nb.len()) - prefix;
    let mut suffix = ob
        .iter()
        .rev()
        .zip(nb.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while suffix > 0
        && !(old.is_char_boundary(ob.len() - suffix) && new.is_char_boundary(nb.len() - suffix))
    {
        suffix -= 1;
    }
    Some((prefix, ob.len() - suffix, &new[prefix..nb.len() - suffix]))
}

/// One document as the server currently knows it.
struct Doc {
    text: String,
    /// Byte offset of the start of each line (including one past a final `\n`,
    /// which the protocol counts as an empty last line).
    line_starts: Vec<usize>,
    language_id: String,
    version: i64,
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    v.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    v
}

impl Doc {
    fn new(text: &str, language_id: &str, version: i64) -> Self {
        Self {
            line_starts: line_starts(text),
            text: text.to_string(),
            language_id: language_id.to_string(),
            version,
        }
    }

    fn replace(&mut self, text: &str, version: i64) {
        self.text.clear();
        self.text.push_str(text);
        self.line_starts = line_starts(text);
        self.version = version;
    }

    /// Line `n` without its terminator.
    fn line(&self, n: usize) -> Option<&str> {
        let start = *self.line_starts.get(n)?;
        let end = self
            .line_starts
            .get(n + 1)
            .copied()
            .unwrap_or(self.text.len());
        Some(self.text[start..end].trim_end_matches(['\n', '\r']))
    }

    /// The server-side position of a byte offset.
    fn position(&self, byte: usize, enc: PositionEncoding) -> Position {
        let byte = byte.min(self.text.len());
        let line = self
            .line_starts
            .partition_point(|&s| s <= byte)
            .saturating_sub(1);
        let start = self.line_starts[line];
        let line_text = self.line(line).unwrap_or("");
        Position::new(line as u32, col_to_lsp(line_text, byte - start, enc))
    }
}

/// Readiness of the handshake.
const STARTING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// State shared between the client, its threads and converters.
struct Inner {
    name: String,
    root_uri: String,
    pending: Mutex<HashMap<i64, Sender<Result<Value, Value>>>>,
    docs: Mutex<HashMap<String, Doc>>,
    /// Notifications sent before the handshake finished, flushed after it.
    /// Also the lock under which readiness flips, so nothing slips between
    /// "queue it" and "the queue was flushed".
    queued: Mutex<Vec<Value>>,
    ready: (Mutex<u8>, Condvar),
    encoding: AtomicU8,
    /// The server's `textDocumentSync.change` kind (2 = incremental).
    sync_kind: AtomicU8,
    alive: AtomicBool,
    /// Set by `Drop` before asking the server to exit, so the reader thread can
    /// tell a shutdown we requested from a crash.
    closing: AtomicBool,
    /// Frames for the writer thread, which owns the server's stdin.
    writer: Sender<Vec<u8>>,
    /// The outstanding request per superseding method, so a newer one can
    /// cancel it (see `LspClient::request_superseding`).
    latest: Mutex<HashMap<&'static str, i64>>,
}

impl Inner {
    fn encoding(&self) -> PositionEncoding {
        PositionEncoding::from_u8(self.encoding.load(Ordering::SeqCst))
    }

    fn ready_state(&self) -> u8 {
        *self.ready.0.lock().unwrap()
    }

    fn set_ready(&self, state: u8) {
        let mut g = self.ready.0.lock().unwrap();
        *g = state;
        self.ready.1.notify_all();
    }

    /// Hand a message to the writer thread. Never blocks on the server.
    fn send_value(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_vec(msg)?;
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(&body);
        self.writer
            .send(frame)
            .map_err(|_| anyhow!("language server `{}` stdin closed", self.name))
    }

    fn fail_pending(&self, why: &str) {
        let mut map = self.pending.lock().unwrap();
        for (_, tx) in map.drain() {
            let _ = tx.send(Err(Value::String(why.into())));
        }
    }
}

/// Converts positions between editor byte columns and the server's encoding.
/// Lines come from the document mirror, else from disk — cached for the life of
/// the converter, so a reference list spanning thirty files reads each once.
struct Positions<'a> {
    inner: &'a Inner,
    enc: PositionEncoding,
    disk: HashMap<String, Option<Doc>>,
}

impl<'a> Positions<'a> {
    fn new(inner: &'a Inner) -> Self {
        Self {
            inner,
            enc: inner.encoding(),
            disk: HashMap::new(),
        }
    }

    fn with_line<R>(&mut self, uri: &str, line: u32, f: impl FnOnce(&str) -> R) -> Option<R> {
        if let Ok(docs) = self.inner.docs.lock() {
            if let Some(d) = docs.get(uri) {
                return d.line(line as usize).map(f);
            }
        }
        let d = self.disk.entry(uri.to_string()).or_insert_with(|| {
            std::fs::read_to_string(uri_to_path(uri))
                .ok()
                .map(|t| Doc::new(&t, "", 0))
        });
        d.as_ref().and_then(|d| d.line(line as usize)).map(f)
    }

    fn pos_to_editor(&mut self, uri: &str, pos: &mut Position) {
        let enc = self.enc;
        if enc == PositionEncoding::Utf8 {
            return;
        }
        let col = pos.character;
        if let Some(c) = self.with_line(uri, pos.line, |l| col_from_lsp(l, col, enc)) {
            pos.character = c as u32;
        }
    }

    fn pos_to_server(&mut self, uri: &str, pos: &mut Position) {
        let enc = self.enc;
        if enc == PositionEncoding::Utf8 {
            return;
        }
        let col = pos.character as usize;
        if let Some(c) = self.with_line(uri, pos.line, |l| col_to_lsp(l, col, enc)) {
            pos.character = c;
        }
    }

    fn range_to_editor(&mut self, uri: &str, r: &mut Range) {
        self.pos_to_editor(uri, &mut r.start);
        self.pos_to_editor(uri, &mut r.end);
    }

    fn range_to_server(&mut self, uri: &str, r: &mut Range) {
        self.pos_to_server(uri, &mut r.start);
        self.pos_to_server(uri, &mut r.end);
    }

    fn edits_to_editor(&mut self, uri: &str, edits: &mut [TextEdit]) {
        for e in edits {
            self.range_to_editor(uri, &mut e.range);
        }
    }

    fn edits_to_server(&mut self, uri: &str, edits: &mut [TextEdit]) {
        for e in edits {
            self.range_to_server(uri, &mut e.range);
        }
    }

    fn diags_to_editor(&mut self, uri: &str, diags: &mut [Diagnostic]) {
        for d in diags {
            self.range_to_editor(uri, &mut d.range);
        }
    }

    fn diags_to_server(&mut self, uri: &str, diags: &mut [Diagnostic]) {
        for d in diags {
            self.range_to_server(uri, &mut d.range);
        }
    }

    fn item_to_editor(&mut self, uri: &str, item: &mut CompletionItem) {
        match &mut item.text_edit {
            Some(CompletionTextEdit::Edit(e)) => self.range_to_editor(uri, &mut e.range),
            Some(CompletionTextEdit::InsertAndReplace(e)) => {
                self.range_to_editor(uri, &mut e.insert);
                self.range_to_editor(uri, &mut e.replace);
            }
            None => {}
        }
        if let Some(extra) = &mut item.additional_text_edits {
            self.edits_to_editor(uri, extra);
        }
    }

    fn item_to_server(&mut self, uri: &str, item: &mut CompletionItem) {
        match &mut item.text_edit {
            Some(CompletionTextEdit::Edit(e)) => self.range_to_server(uri, &mut e.range),
            Some(CompletionTextEdit::InsertAndReplace(e)) => {
                self.range_to_server(uri, &mut e.insert);
                self.range_to_server(uri, &mut e.replace);
            }
            None => {}
        }
        if let Some(extra) = &mut item.additional_text_edits {
            self.edits_to_server(uri, extra);
        }
    }

    /// Canonicalise every URI and convert every edit of a workspace edit.
    fn workspace_edit_to_editor(&mut self, edits: &mut [(String, Vec<TextEdit>)]) {
        for (uri, list) in edits.iter_mut() {
            *uri = normalize_uri(uri);
            self.edits_to_editor(uri, list);
        }
    }
}

pub struct LspClient {
    /// Taken by `Drop`, which hands it to a thread that waits for the exit.
    child: Option<Child>,
    next_id: AtomicI64,
    inner: Arc<Inner>,
    /// The server's `initialize` result capabilities (once ready).
    capabilities: Mutex<Value>,
}

impl LspClient {
    /// Spawn a server. Returns as soon as the process is running; the handshake
    /// happens on a background thread and ends in [`ServerEvent::Ready`] or
    /// [`ServerEvent::InitFailed`]. A missing binary is still an immediate `Err`.
    pub fn start(
        program: &str,
        args: &[&str],
        root: &Path,
        on_event: EventHandler,
    ) -> Result<Arc<Self>> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning language server `{program}`"))?;

        let mut stdin = child.stdin.take().context("server stdin")?;
        let stdout = child.stdout.take().context("server stdout")?;

        // Surface the server's stderr (crashes, config errors) instead of
        // discarding it — invaluable when a language server misbehaves.
        if let Some(stderr) = child.stderr.take() {
            let name = program.to_string();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[lsp:{name}] {line}");
                }
            });
        }

        // Writer thread: the only thing that ever touches the server's stdin,
        // so a server slow to read never blocks the UI (a full document is
        // bigger than a pipe buffer). Exits when every sender is gone.
        let (writer, frames) = unbounded::<Vec<u8>>();
        thread::spawn(move || {
            for frame in frames {
                if stdin.write_all(&frame).is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });

        let inner = Arc::new(Inner {
            name: program.to_string(),
            root_uri: path_to_uri(root),
            pending: Mutex::new(HashMap::new()),
            docs: Mutex::new(HashMap::new()),
            queued: Mutex::new(Vec::new()),
            ready: (Mutex::new(STARTING), Condvar::new()),
            encoding: AtomicU8::new(PositionEncoding::Utf16.as_u8()),
            sync_kind: AtomicU8::new(1),
            alive: AtomicBool::new(true),
            closing: AtomicBool::new(false),
            writer,
            latest: Mutex::new(HashMap::new()),
        });
        let on_event = Arc::new(on_event);

        // Reader thread: dispatch responses / notifications / server requests.
        {
            let inner = inner.clone();
            let on_event = on_event.clone();
            thread::spawn(move || {
                read_loop(stdout, inner, on_event);
            });
        }

        let client = Arc::new(LspClient {
            child: Some(child),
            next_id: AtomicI64::new(1),
            inner,
            capabilities: Mutex::new(Value::Null),
        });

        // Handshake thread: `initialize` can take a while (a PHP server boots an
        // application; an indexer warms up), and nobody should wait for it.
        {
            let client = client.clone();
            thread::spawn(move || match client.handshake() {
                Ok(()) => {
                    client.become_ready();
                    on_event(ServerEvent::Ready);
                }
                Err(e) => {
                    client.inner.queued.lock().unwrap().clear();
                    client.inner.set_ready(FAILED);
                    on_event(ServerEvent::InitFailed(format!("{e:#}")));
                }
            });
        }

        Ok(client)
    }

    /// The program this client runs (`intelephense`, `laravel-lsp`, …).
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Is the server process still talking to us?
    pub fn is_alive(&self) -> bool {
        self.inner.alive.load(Ordering::SeqCst)
    }

    /// Has the handshake completed? (`false` while starting, and after a failure.)
    pub fn is_ready(&self) -> bool {
        self.inner.ready_state() == READY
    }

    /// Block until the handshake completes, fails, or `timeout` passes.
    /// Returns whether the server is ready for requests.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &self.inner.ready;
        let guard = lock.lock().unwrap();
        if *guard != STARTING {
            return *guard == READY;
        }
        let (g, _) = cvar
            .wait_timeout_while(guard, timeout, |st| *st == STARTING)
            .unwrap();
        *g == READY
    }

    /// The column encoding negotiated with the server (meaningful once ready).
    pub fn position_encoding(&self) -> PositionEncoding {
        self.inner.encoding()
    }

    /// Does the server accept `didChange` as ranges rather than whole documents?
    pub fn incremental_sync(&self) -> bool {
        self.inner.sync_kind.load(Ordering::SeqCst) == 2
    }

    /// Does the server fill in completion items lazily (`completionItem/resolve`)?
    pub fn supports_completion_resolve(&self) -> bool {
        self.capabilities
            .lock()
            .ok()
            .and_then(|c| {
                c.pointer("/completionProvider/resolveProvider")
                    .and_then(|v| v.as_bool())
            })
            .unwrap_or(false)
    }

    fn handshake(&self) -> Result<()> {
        let root_uri = &self.inner.root_uri;
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "general": {
                    // Byte columns are what the editor speaks; a server that
                    // agrees (rust-analyzer, clangd) saves both sides the
                    // conversion. UTF-16 stays available for everyone else.
                    "positionEncodings": ["utf-8", "utf-16"]
                },
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": true },
                    "completion": {
                        "completionItem": {
                            "snippetSupport": false,
                            "resolveSupport": {
                                "properties": ["documentation", "detail", "additionalTextEdits"]
                            }
                        },
                        "contextSupport": true
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "signatureHelp": {},
                    "definition": {},
                    "references": {},
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "codeAction": {
                        "dynamicRegistration": false,
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": ["quickfix", "refactor", "source", "source.organizeImports"]
                            }
                        }
                    },
                    "formatting": {},
                    "rename": {},
                    "inlayHint": {},
                },
                "window": {
                    "workDoneProgress": true,
                    "showMessage": {}
                },
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "applyEdit": true,
                    "workspaceEdit": { "documentChanges": true },
                    "symbol": {}
                }
            },
            "workspaceFolders": [ { "uri": root_uri, "name": "root" } ],
        });

        let result = self.request_raw("initialize", params, Duration::from_secs(30), None)?;
        let caps = result.get("capabilities").cloned().unwrap_or(Value::Null);
        if let Some(enc) = caps
            .get("positionEncoding")
            .and_then(|v| v.as_str())
            .and_then(PositionEncoding::parse)
        {
            self.inner.encoding.store(enc.as_u8(), Ordering::SeqCst);
        }
        // `textDocumentSync` is either the kind itself or an object with `change`.
        let sync = match caps.get("textDocumentSync") {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(1),
            Some(Value::Object(o)) => o.get("change").and_then(|c| c.as_u64()).unwrap_or(1),
            _ => 1,
        };
        self.inner.sync_kind.store(sync as u8, Ordering::SeqCst);
        if let Ok(mut c) = self.capabilities.lock() {
            *c = caps;
        }
        self.inner.send_value(&json!({
            "jsonrpc": "2.0", "method": "initialized", "params": {}
        }))?;
        Ok(())
    }

    /// Open the gate: the server sees every mirrored document as it is *now*
    /// (edits made while it was starting are folded into one `didOpen`), then
    /// everything else that was queued, in order.
    fn become_ready(&self) {
        let mut q = self.inner.queued.lock().unwrap();
        {
            let docs = self.inner.docs.lock().unwrap();
            for (uri, doc) in docs.iter() {
                let _ = self.inner.send_value(&did_open_msg(
                    uri,
                    &doc.language_id,
                    doc.version,
                    &doc.text,
                ));
            }
        }
        for m in q.drain(..) {
            let _ = self.inner.send_value(&m);
        }
        self.inner.set_ready(READY);
    }

    /// Send a request and block for the response. Waits for the handshake first
    /// (within `timeout`), since a server answers nothing before it.
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let started = Instant::now();
        if !self.wait_ready(timeout) {
            return Err(anyhow!(
                "language server `{}` is not ready for `{method}`",
                self.inner.name
            ));
        }
        let left = timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(100));
        self.request_raw(method, params, left, None)
    }

    /// Like [`Self::request`], but a newer call for the same `method` cancels
    /// the one still outstanding: the answer to a question the user has typed
    /// past is worthless, and the server can stop working on it.
    fn request_superseding(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let prev = self.inner.latest.lock().unwrap().remove(method);
        if let Some(prev) = prev {
            if let Some(tx) = self.inner.pending.lock().unwrap().remove(&prev) {
                let _ = tx.send(Err(Value::String("superseded".into())));
                let _ = self.inner.send_value(&json!({
                    "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": prev }
                }));
            }
        }
        let started = Instant::now();
        if !self.wait_ready(timeout) {
            return Err(anyhow!(
                "language server `{}` is not ready for `{method}`",
                self.inner.name
            ));
        }
        let left = timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(100));
        self.request_raw(method, params, left, Some(method))
    }

    /// The request machinery itself; `track` registers the id as `method`'s
    /// outstanding request for `request_superseding`.
    fn request_raw(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        track: Option<&'static str>,
    ) -> Result<Value> {
        if !self.is_alive() {
            return Err(anyhow!("language server `{}` has exited", self.inner.name));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = bounded(1);
        self.inner.pending.lock().unwrap().insert(id, tx);
        if let Some(key) = track {
            self.inner.latest.lock().unwrap().insert(key, id);
        }

        let sent = self.inner.send_value(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }));
        let out = match sent {
            Err(e) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Err(e)
            }
            Ok(()) => match rx.recv_timeout(timeout) {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(err)) => Err(anyhow!("LSP error for {method}: {err}")),
                Err(_) => {
                    self.inner.pending.lock().unwrap().remove(&id);
                    Err(anyhow!("LSP request `{method}` timed out"))
                }
            },
        };
        if let Some(key) = track {
            let mut latest = self.inner.latest.lock().unwrap();
            if latest.get(key) == Some(&id) {
                latest.remove(key);
            }
        }
        out
    }

    /// Send a notification (fire and forget). Queued until the handshake
    /// completes; dropped if it failed.
    pub fn notify(&self, method: &str, params: Value) {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut q = self.inner.queued.lock().unwrap();
        match self.inner.ready_state() {
            READY => {
                drop(q);
                let _ = self.inner.send_value(&msg);
            }
            STARTING => q.push(msg),
            _ => {}
        }
    }

    fn positions(&self) -> Positions<'_> {
        Positions::new(&self.inner)
    }

    pub fn did_open(&self, uri: &str, language_id: &str, version: i64, text: &str) {
        let q = self.inner.queued.lock().unwrap();
        self.inner
            .docs
            .lock()
            .unwrap()
            .insert(uri.to_string(), Doc::new(text, language_id, version));
        if self.inner.ready_state() != READY {
            return; // opened from the mirror once the handshake completes
        }
        drop(q);
        let _ = self
            .inner
            .send_value(&did_open_msg(uri, language_id, version, text));
    }

    /// The document changed to `text`. Sends only the changed bytes when the
    /// server supports incremental sync, the whole text otherwise — and, while
    /// the server is still starting, only updates the mirror (it will be opened
    /// with the latest text).
    pub fn did_change(&self, uri: &str, version: i64, text: &str) {
        let q = self.inner.queued.lock().unwrap();
        let ready = self.inner.ready_state() == READY;
        let mut docs = self.inner.docs.lock().unwrap();
        let doc = docs
            .entry(uri.to_string())
            .or_insert_with(|| Doc::new("", "", 0));
        if !ready {
            doc.replace(text, version);
            return;
        }
        let changes = if self.incremental_sync() {
            match minimal_change(&doc.text, text) {
                None => {
                    doc.version = version;
                    return;
                }
                Some((start, end, new_text)) => {
                    let enc = self.inner.encoding();
                    let range = Range::new(doc.position(start, enc), doc.position(end, enc));
                    json!([{ "range": range, "text": new_text }])
                }
            }
        } else {
            json!([{ "text": text }])
        };
        doc.replace(text, version);
        drop(docs);
        drop(q);
        let _ = self.inner.send_value(&json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": changes
            }
        }));
    }

    pub fn did_save(&self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri }, "text": text }),
        );
    }

    pub fn did_close(&self, uri: &str) {
        let q = self.inner.queued.lock().unwrap();
        self.inner.docs.lock().unwrap().remove(uri);
        if self.inner.ready_state() != READY {
            return; // never opened on the server
        }
        drop(q);
        let _ = self.inner.send_value(&json!({
            "jsonrpc": "2.0", "method": "textDocument/didClose",
            "params": { "textDocument": { "uri": uri } }
        }));
    }

    /// The server-side JSON for a position given in editor columns.
    fn server_position(&self, cv: &mut Positions<'_>, uri: &str, line: u32, col: u32) -> Value {
        let mut pos = Position::new(line, col);
        cv.pos_to_server(uri, &mut pos);
        json!({ "line": pos.line, "character": pos.character })
    }

    /// Request completions at a position. Blocking; call off the UI thread.
    /// A newer completion request cancels an older one still in flight.
    pub fn completion(&self, uri: &str, line: u32, character: u32) -> Result<Vec<CompletionItem>> {
        let mut cv = self.positions();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": self.server_position(&mut cv, uri, line, character),
            "context": { "triggerKind": 1 }
        });
        let res =
            self.request_superseding("textDocument/completion", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(Vec::new());
        }
        let parsed: CompletionResponse = serde_json::from_value(res)?;
        let mut items = match parsed {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        for it in &mut items {
            cv.item_to_editor(uri, it);
        }
        Ok(items)
    }

    /// Fill in a completion item's lazy parts (documentation, and the
    /// `additionalTextEdits` that add a `use` import). Blocking.
    pub fn resolve_completion(&self, uri: &str, item: &CompletionItem) -> Result<CompletionItem> {
        let mut cv = self.positions();
        let mut out = item.clone();
        cv.item_to_server(uri, &mut out);
        let res = self.request(
            "completionItem/resolve",
            serde_json::to_value(&out)?,
            Duration::from_secs(3),
        )?;
        let mut resolved: CompletionItem = serde_json::from_value(res)?;
        cv.item_to_editor(uri, &mut resolved);
        Ok(resolved)
    }

    /// Request the definition location of the symbol at a position.
    /// Returns `(uri, line, character)`. Blocking; call off the UI thread.
    pub fn definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<(String, u32, u32)>> {
        let mut cv = self.positions();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": self.server_position(&mut cv, uri, line, character)
        });
        let res = self.request("textDocument/definition", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(None);
        }
        let resp: GotoDefinitionResponse = serde_json::from_value(res)?;
        let loc = match resp {
            GotoDefinitionResponse::Scalar(l) => Some((l.uri.to_string(), l.range.start)),
            GotoDefinitionResponse::Array(v) => v
                .into_iter()
                .next()
                .map(|l| (l.uri.to_string(), l.range.start)),
            GotoDefinitionResponse::Link(v) => v
                .into_iter()
                .next()
                .map(|l| (l.target_uri.to_string(), l.target_range.start)),
        };
        Ok(loc.map(|(uri, mut pos)| {
            let uri = normalize_uri(&uri);
            cv.pos_to_editor(&uri, &mut pos);
            (uri, pos.line, pos.character)
        }))
    }

    /// Find all references to the symbol at a position.
    /// Returns `(uri, line, character)` per reference.
    pub fn references(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<(String, u32, u32)>> {
        let mut cv = self.positions();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": self.server_position(&mut cv, uri, line, character),
            "context": { "includeDeclaration": true }
        });
        let res = self.request("textDocument/references", params, Duration::from_secs(5))?;
        Ok(locations_from_value(&res)
            .into_iter()
            .map(|(u, l, c)| to_editor_location(&mut cv, u, l, c))
            .collect())
    }

    /// Search workspace symbols by name. Returns `(name, uri, line, character)`.
    /// A newer query cancels an older one still in flight.
    pub fn workspace_symbol(&self, query: &str) -> Result<Vec<(String, String, u32, u32)>> {
        let params = json!({ "query": query });
        let res = self.request_superseding("workspace/symbol", params, Duration::from_secs(5))?;
        let mut cv = self.positions();
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for s in arr {
                let Some(name) = s.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                let loc = &s["location"];
                if let (Some(uri), Some(line), Some(ch)) = (
                    loc["uri"].as_str(),
                    loc["range"]["start"]["line"].as_u64(),
                    loc["range"]["start"]["character"].as_u64(),
                ) {
                    let (uri, line, ch) =
                        to_editor_location(&mut cv, uri.to_string(), line as u32, ch as u32);
                    out.push((name.to_string(), uri, line, ch));
                }
            }
        }
        Ok(out)
    }

    /// Request whole-document formatting. Returns the edits to apply.
    pub fn formatting(
        &self,
        uri: &str,
        tab_size: u32,
        insert_spaces: bool,
    ) -> Result<Vec<TextEdit>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": tab_size, "insertSpaces": insert_spaces }
        });
        let res = self.request("textDocument/formatting", params, Duration::from_secs(8))?;
        if res.is_null() {
            return Ok(Vec::new());
        }
        let mut edits: Vec<TextEdit> = serde_json::from_value(res)?;
        self.positions().edits_to_editor(uri, &mut edits);
        Ok(edits)
    }

    /// Request code actions (quick-fixes) for a range. Blocking.
    #[allow(clippy::too_many_arguments)]
    pub fn code_actions(
        &self,
        uri: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<CodeActionItem>> {
        let mut cv = self.positions();
        let mut range = Range::new(
            Position::new(start_line, start_char),
            Position::new(end_line, end_char),
        );
        cv.range_to_server(uri, &mut range);
        let mut diags = diagnostics.to_vec();
        cv.diags_to_server(uri, &mut diags);
        let params = json!({
            "textDocument": { "uri": uri },
            "range": range,
            "context": { "diagnostics": serde_json::to_value(diags)? }
        });
        let res = self.request("textDocument/codeAction", params, Duration::from_secs(5))?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for it in arr {
                let Some(title) = it.get("title").and_then(|t| t.as_str()) else {
                    continue;
                };
                let mut edits = parse_workspace_edit(it.get("edit"));
                cv.workspace_edit_to_editor(&mut edits);
                // A bare `Command` (no `edit` key) is its own command; a
                // `CodeAction` carries one under `command`.
                let command = match it.get("command") {
                    Some(Value::Object(_)) => it.get("command").cloned(),
                    Some(Value::String(_)) => Some(it.clone()),
                    _ => None,
                };
                out.push(CodeActionItem {
                    title: title.to_string(),
                    edits,
                    command,
                    server: self.inner.name.clone(),
                });
            }
        }
        Ok(out)
    }

    /// Run a server command (from a code action without edits). The server
    /// answers with the edits as a `workspace/applyEdit` event.
    pub fn execute_command(&self, command: &Value) -> Result<Value> {
        let params = json!({
            "command": command.get("command").cloned().unwrap_or(Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(Value::Array(Vec::new()))
        });
        self.request("workspace/executeCommand", params, Duration::from_secs(8))
    }

    /// Document symbols for `uri` as a flat list `(name, kind, line, char, depth)`.
    /// A newer request cancels an older one still in flight.
    pub fn document_symbols(&self, uri: &str) -> Result<Vec<DocumentSymbol>> {
        let params = json!({ "textDocument": { "uri": uri } });
        let res = self.request_superseding(
            "textDocument/documentSymbol",
            params,
            Duration::from_secs(5),
        )?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for s in arr {
                collect_symbol(s, 0, &mut out);
            }
        }
        let mut cv = self.positions();
        for (_, _, line, ch, _) in &mut out {
            let mut pos = Position::new(*line, *ch);
            cv.pos_to_editor(uri, &mut pos);
            *ch = pos.character;
        }
        Ok(out)
    }

    /// Rename the symbol at a position. Returns per-URI edits to apply.
    pub fn rename(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Vec<(String, Vec<TextEdit>)>> {
        let mut cv = self.positions();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": self.server_position(&mut cv, uri, line, character),
            "newName": new_name
        });
        let res = self.request("textDocument/rename", params, Duration::from_secs(8))?;
        let mut edits = parse_workspace_edit(Some(&res));
        cv.workspace_edit_to_editor(&mut edits);
        Ok(edits)
    }

    /// Request signature help at a position (function call hints).
    /// A newer request cancels an older one still in flight.
    pub fn signature_help(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<SignatureInfo>> {
        let mut cv = self.positions();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": self.server_position(&mut cv, uri, line, character)
        });
        let res =
            self.request_superseding("textDocument/signatureHelp", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(None);
        }
        let sigs = match res["signatures"].as_array() {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(None),
        };
        let active_sig = res["activeSignature"].as_u64().unwrap_or(0) as usize;
        let sig = sigs.get(active_sig).or_else(|| sigs.first()).unwrap();
        let label = sig["label"].as_str().unwrap_or("").to_string();

        let active_param = res["activeParameter"]
            .as_u64()
            .or_else(|| sig["activeParameter"].as_u64())
            .map(|v| v as usize);
        let active = active_param
            .and_then(|ap| sig["parameters"].as_array().and_then(|ps| ps.get(ap)))
            .and_then(|p| param_range(&p["label"], &label));

        Ok(Some(SignatureInfo { label, active }))
    }

    /// Request hover text at a position. Blocking; call off the UI thread.
    /// A newer request cancels an older one still in flight.
    pub fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Option<String>> {
        let mut cv = self.positions();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": self.server_position(&mut cv, uri, line, character)
        });
        let res = self.request_superseding("textDocument/hover", params, Duration::from_secs(5))?;
        if res.is_null() {
            return Ok(None);
        }
        let hover: Hover = serde_json::from_value(res)?;
        Ok(Some(hover_to_string(hover.contents)))
    }

    /// Request inlay hints for lines `0..=end_line`. Returns `(line, character,
    /// label)` per hint. Blocking; call off the UI thread.
    /// A newer request cancels an older one still in flight.
    pub fn inlay_hints(&self, uri: &str, end_line: u32) -> Result<Vec<(u32, u32, String)>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": end_line, "character": 0 }
            }
        });
        let res =
            self.request_superseding("textDocument/inlayHint", params, Duration::from_secs(5))?;
        let mut cv = self.positions();
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for h in arr {
                let Some(pos) = h.get("position") else {
                    continue;
                };
                let line = pos.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let ch = pos.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let label = match h.get("label") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(parts)) => parts
                        .iter()
                        .filter_map(|p| p.get("value").and_then(|v| v.as_str()))
                        .collect::<String>(),
                    _ => continue,
                };
                if !label.is_empty() {
                    let mut p = Position::new(line, ch);
                    cv.pos_to_editor(uri, &mut p);
                    out.push((p.line, p.character, label.trim().to_string()));
                }
            }
        }
        Ok(out)
    }
}

fn did_open_msg(uri: &str, language_id: &str, version: i64, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
            "uri": uri, "languageId": language_id, "version": version, "text": text
        }}
    })
}

/// Canonicalise a server-supplied location and convert its column.
fn to_editor_location(
    cv: &mut Positions<'_>,
    uri: String,
    line: u32,
    ch: u32,
) -> (String, u32, u32) {
    let uri = normalize_uri(&uri);
    let mut pos = Position::new(line, ch);
    cv.pos_to_editor(&uri, &mut pos);
    (uri, pos.line, pos.character)
}

/// Recursively flatten a `DocumentSymbol` (or `SymbolInformation`).
fn collect_symbol(s: &Value, depth: usize, out: &mut Vec<(String, i64, u32, u32, usize)>) {
    let Some(name) = s.get("name").and_then(|n| n.as_str()) else {
        return;
    };
    let kind = s.get("kind").and_then(|k| k.as_i64()).unwrap_or(0);
    // DocumentSymbol uses selectionRange/range; SymbolInformation uses location.range.
    let pos = s
        .get("selectionRange")
        .or_else(|| s.get("range"))
        .or_else(|| s.pointer("/location/range"));
    let (line, ch) = pos
        .map(|r| {
            (
                r["start"]["line"].as_u64().unwrap_or(0) as u32,
                r["start"]["character"].as_u64().unwrap_or(0) as u32,
            )
        })
        .unwrap_or((0, 0));
    out.push((name.to_string(), kind, line, ch, depth));
    if let Some(children) = s.get("children").and_then(|c| c.as_array()) {
        for c in children {
            collect_symbol(c, depth + 1, out);
        }
    }
}

/// Resolve a parameter label (string or `[start,end]` offsets) to a char range.
fn param_range(plabel: &Value, sig_label: &str) -> Option<(u32, u32)> {
    if let Some(arr) = plabel.as_array() {
        let a = arr.first()?.as_u64()? as u32;
        let b = arr.get(1)?.as_u64()? as u32;
        return Some((a, b));
    }
    if let Some(s) = plabel.as_str() {
        let idx = sig_label.find(s)? as u32;
        return Some((idx, idx + s.chars().count() as u32));
    }
    None
}

/// Parse a `WorkspaceEdit` (`changes` or `documentChanges`) into per-URI edits.
/// Positions are left in the server's encoding; see `Positions`.
fn parse_workspace_edit(edit: Option<&Value>) -> Vec<(String, Vec<TextEdit>)> {
    let Some(edit) = edit else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            if let Ok(te) = serde_json::from_value::<Vec<TextEdit>>(edits.clone()) {
                out.push((uri.clone(), te));
            }
        }
    } else if let Some(dc) = edit.get("documentChanges").and_then(|d| d.as_array()) {
        for change in dc {
            if let (Some(uri), Some(edits)) =
                (change["textDocument"]["uri"].as_str(), change.get("edits"))
            {
                if let Ok(te) = serde_json::from_value::<Vec<TextEdit>>(edits.clone()) {
                    out.push((uri.to_string(), te));
                }
            }
        }
    }
    out
}

/// Extract `(uri, line, character)` from a `Location` or `Location[]` value.
fn locations_from_value(res: &Value) -> Vec<(String, u32, u32)> {
    fn one(v: &Value) -> Option<(String, u32, u32)> {
        let uri = v["uri"].as_str()?;
        let line = v["range"]["start"]["line"].as_u64()? as u32;
        let ch = v["range"]["start"]["character"].as_u64()? as u32;
        Some((uri.to_string(), line, ch))
    }
    match res {
        Value::Array(arr) => arr.iter().filter_map(one).collect(),
        Value::Null => Vec::new(),
        v => one(v).into_iter().collect(),
    }
}

fn hover_to_string(contents: HoverContents) -> String {
    fn marked(m: MarkedString) -> String {
        match m {
            MarkedString::String(s) => s,
            MarkedString::LanguageString(ls) => ls.value,
        }
    }
    match contents {
        HoverContents::Scalar(m) => marked(m),
        HoverContents::Array(arr) => arr.into_iter().map(marked).collect::<Vec<_>>().join("\n\n"),
        HoverContents::Markup(mk) => mk.value,
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Tell the reader thread this exit is ours, so it isn't reported as a
        // crash. Then the graceful sequence per the LSP spec (shutdown request,
        // exit notification) without waiting for a reply, and a background
        // thread gives the process a second to leave before killing it — so
        // closing a window or replacing a server never stalls the caller.
        self.inner.closing.store(true, Ordering::SeqCst);
        if self.is_alive() && self.is_ready() {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let _ = self.inner.send_value(&json!({
                "jsonrpc": "2.0", "id": id, "method": "shutdown", "params": null
            }));
            let _ = self.inner.send_value(&json!({
                "jsonrpc": "2.0", "method": "exit", "params": null
            }));
        }
        if let Some(mut child) = self.child.take() {
            thread::spawn(move || {
                for _ in 0..50 {
                    if let Ok(Some(_)) = child.try_wait() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

fn read_loop(stdout: impl Read, inner: Arc<Inner>, on_event: Arc<EventHandler>) {
    let mut reader = BufReader::new(stdout);
    // `$/progress` reports carry only a token; the title arrived with `begin`.
    let mut progress_titles: HashMap<String, String> = HashMap::new();
    loop {
        let msg = match read_message(&mut reader) {
            Ok(Some(msg)) => msg,
            // EOF or error: the server exited. Fail any pending requests fast
            // instead of letting them wait for their timeouts.
            Ok(None) | Err(_) => break,
        };

        let has_id = msg.get("id").map(|v| !v.is_null()).unwrap_or(false);
        let has_method = msg.get("method").is_some();

        if has_id && !has_method {
            // Response to one of our requests.
            let id = msg["id"].as_i64().unwrap_or(-1);
            if let Some(tx) = inner.pending.lock().unwrap().remove(&id) {
                if let Some(err) = msg.get("error") {
                    let _ = tx.send(Err(err.clone()));
                } else {
                    let _ = tx.send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
                }
            }
        } else if has_id && has_method {
            // Server -> client request: must reply or the server may stall.
            respond_to_server_request(&msg, &inner, &on_event);
        } else if has_method {
            handle_notification(&msg, &inner, &on_event, &mut progress_titles);
        }
    }
    inner.alive.store(false, Ordering::SeqCst);
    inner.fail_pending("server exited");
    // Anyone waiting for the handshake stops waiting.
    if inner.ready_state() == STARTING {
        inner.set_ready(FAILED);
    }
    if !inner.closing.load(Ordering::SeqCst) {
        on_event(ServerEvent::Exited);
    }
}

fn handle_notification(
    msg: &Value,
    inner: &Inner,
    on_event: &EventHandler,
    progress_titles: &mut HashMap<String, String>,
) {
    let method = msg["method"].as_str().unwrap_or("");
    let params = msg.get("params");
    match method {
        "textDocument/publishDiagnostics" => {
            let Some(params) = params else { return };
            let Ok(mut p) = serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
            else {
                return;
            };
            // Canonical URI first, so the mirror lookup and the editor's own
            // buffer key agree with it.
            let uri = normalize_uri(p.uri.as_str());
            if let Ok(u) = url::Url::parse(&uri) {
                p.uri = u;
            }
            Positions::new(inner).diags_to_editor(&uri, &mut p.diagnostics);
            on_event(ServerEvent::Diagnostics(p));
        }
        "window/showMessage" => {
            let Some(params) = params else { return };
            on_event(ServerEvent::Message {
                level: MessageLevel::from_lsp(params.get("type")),
                text: params["message"].as_str().unwrap_or("").to_string(),
            });
        }
        "window/logMessage" => {
            if let Some(text) = params.and_then(|p| p["message"].as_str()) {
                eprintln!("[lsp:{}] {text}", inner.name);
            }
        }
        "$/progress" => {
            let Some(params) = params else { return };
            let token = match &params["token"] {
                Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            let value = &params["value"];
            let message = value["message"].as_str().map(str::to_string);
            let percentage = value["percentage"].as_u64().map(|p| p as u32);
            match value["kind"].as_str() {
                Some("begin") => {
                    let title = value["title"].as_str().unwrap_or("Working").to_string();
                    progress_titles.insert(token, title.clone());
                    on_event(ServerEvent::Progress {
                        title,
                        message,
                        percentage,
                        done: false,
                    });
                }
                Some("report") => {
                    let title = progress_titles.get(&token).cloned().unwrap_or_default();
                    on_event(ServerEvent::Progress {
                        title,
                        message,
                        percentage,
                        done: false,
                    });
                }
                Some("end") => {
                    let title = progress_titles.remove(&token).unwrap_or_default();
                    on_event(ServerEvent::Progress {
                        title,
                        message,
                        percentage: None,
                        done: true,
                    });
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn respond_to_server_request(msg: &Value, inner: &Inner, on_event: &EventHandler) {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg["method"].as_str().unwrap_or("");
    let params = msg.get("params");

    let result: Result<Value, Value> = match method {
        "workspace/configuration" => {
            let n = params
                .and_then(|p| p["items"].as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(Value::Array(vec![Value::Null; n]))
        }
        "workspace/workspaceFolders" => Ok(json!([{ "uri": inner.root_uri, "name": "root" }])),
        "workspace/applyEdit" => {
            let mut edits = parse_workspace_edit(params.and_then(|p| p.get("edit")));
            Positions::new(inner).workspace_edit_to_editor(&mut edits);
            let label = params
                .and_then(|p| p.get("label"))
                .and_then(|l| l.as_str())
                .map(str::to_string);
            on_event(ServerEvent::ApplyEdit { label, edits });
            Ok(json!({ "applied": true }))
        }
        "window/showMessageRequest" => {
            if let Some(p) = params {
                on_event(ServerEvent::Message {
                    level: MessageLevel::from_lsp(p.get("type")),
                    text: p["message"].as_str().unwrap_or("").to_string(),
                });
            }
            // We show the message but offer no buttons; `null` means dismissed.
            Ok(Value::Null)
        }
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability" => Ok(Value::Null),
        _ => Err(json!({ "code": -32601, "message": format!("method not found: {method}") })),
    };

    let reply = match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
    };
    let _ = inner.send_value(&reply);
}

/// Read one LSP message (headers + JSON body) from `reader`.
fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse().context("bad Content-Length")?);
        }
    }

    let len = content_length.ok_or_else(|| anyhow!("message without Content-Length"))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

// ---- URIs -------------------------------------------------------------------

/// Convert a `file://` URI back to a filesystem path, decoding every
/// percent-escape (`%C3%B8` → `ø`, `%23` → `#`), not just spaces.
pub fn uri_to_path(uri: &str) -> PathBuf {
    if let Ok(u) = url::Url::parse(uri) {
        if let Ok(p) = u.to_file_path() {
            return p;
        }
    }
    // Not a well-formed file URL (a bare path, say): take it as it is.
    PathBuf::from(
        uri.strip_prefix("file://")
            .unwrap_or(uri)
            .replace("%20", " "),
    )
}

/// Convert a filesystem path to a `file://` URI with proper percent-encoding.
pub fn path_to_uri(path: &Path) -> String {
    match url::Url::from_file_path(path) {
        Ok(u) => u.to_string(),
        // Only relative paths fail; keep them recognisable rather than panic.
        Err(()) => format!("file://{}", path.to_string_lossy().replace(' ', "%20")),
    }
}

/// The one spelling of a file URI the editor uses as a key. Servers echo URIs
/// back in their own escaping; this makes `ø` and `%C3%B8` the same file.
pub fn normalize_uri(uri: &str) -> String {
    if uri.starts_with("file:") {
        path_to_uri(&uri_to_path(uri))
    } else {
        uri.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_inner() -> Inner {
        let (writer, _frames) = unbounded();
        Inner {
            name: "test".into(),
            root_uri: "file:///p".into(),
            pending: Mutex::new(HashMap::new()),
            docs: Mutex::new(HashMap::new()),
            queued: Mutex::new(Vec::new()),
            ready: (Mutex::new(READY), Condvar::new()),
            encoding: AtomicU8::new(PositionEncoding::Utf16.as_u8()),
            sync_kind: AtomicU8::new(2),
            alive: AtomicBool::new(true),
            closing: AtomicBool::new(false),
            writer,
            latest: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn uri_roundtrip() {
        let p = Path::new("/tmp/my project/main.rs");
        let uri = path_to_uri(p);
        assert_eq!(uri, "file:///tmp/my%20project/main.rs");
        assert_eq!(uri_to_path(&uri), p.to_path_buf());
    }

    #[test]
    fn uri_encodes_non_ascii_and_reserved_characters() {
        let p = Path::new("/Users/kh/Økonomi/#2 rapport/fil.php");
        let uri = path_to_uri(p);
        assert_eq!(uri, "file:///Users/kh/%C3%98konomi/%232%20rapport/fil.php");
        assert_eq!(uri_to_path(&uri), p.to_path_buf());
    }

    #[test]
    fn normalize_makes_server_spellings_match_ours() {
        // A server that doesn't escape `ø`, and one that does, name the same file.
        let raw = "file:///Users/kh/Økonomi/fil.php";
        let escaped = "file:///Users/kh/%C3%98konomi/fil.php";
        assert_eq!(normalize_uri(raw), normalize_uri(escaped));
        assert_eq!(normalize_uri(escaped), escaped);
        // Non-file schemes pass through untouched.
        assert_eq!(normalize_uri("untitled:1"), "untitled:1");
    }

    #[test]
    fn parse_location_array() {
        let v = json!([
            { "uri": "file:///a.rs", "range": { "start": { "line": 3, "character": 5 }, "end": {"line":3,"character":9} } }
        ]);
        assert_eq!(
            locations_from_value(&v),
            vec![("file:///a.rs".to_string(), 3, 5)]
        );
    }

    #[test]
    fn parse_location_null() {
        assert!(locations_from_value(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn utf16_columns_roundtrip_through_norwegian_text() {
        // `ø` is 2 bytes / 1 UTF-16 unit; the emoji is 4 bytes / 2 units.
        let line = "$navn = 'Bjørn 🚀';";
        let byte_col = line.find("🚀").unwrap();
        // 16 bytes before it (the `ø` took two), 15 UTF-16 units.
        assert_eq!(byte_col, 16);
        let u16_col = col_to_lsp(line, byte_col, PositionEncoding::Utf16);
        assert_eq!(u16_col, 15);
        assert_eq!(
            col_from_lsp(line, u16_col, PositionEncoding::Utf16),
            byte_col
        );
        // After the emoji: bytes jump by 4, UTF-16 by 2.
        let after = byte_col + "🚀".len();
        assert_eq!(col_to_lsp(line, after, PositionEncoding::Utf16), 17);
        assert_eq!(col_from_lsp(line, 17, PositionEncoding::Utf16), after);
        // End of line and beyond clamp to the line length.
        assert_eq!(col_from_lsp(line, 999, PositionEncoding::Utf16), line.len());
    }

    #[test]
    fn utf8_and_utf32_columns() {
        let line = "ær";
        assert_eq!(col_to_lsp(line, 2, PositionEncoding::Utf8), 2);
        assert_eq!(col_from_lsp(line, 2, PositionEncoding::Utf8), 2);
        // A UTF-8 column inside a character snaps back to its start.
        assert_eq!(col_from_lsp(line, 1, PositionEncoding::Utf8), 0);
        assert_eq!(col_to_lsp(line, 2, PositionEncoding::Utf32), 1);
        assert_eq!(col_from_lsp(line, 1, PositionEncoding::Utf32), 2);
    }

    #[test]
    fn ascii_columns_are_unchanged() {
        let line = "return $user->name;";
        for c in 0..=line.len() {
            assert_eq!(col_to_lsp(line, c, PositionEncoding::Utf16) as usize, c);
            assert_eq!(col_from_lsp(line, c as u32, PositionEncoding::Utf16), c);
        }
    }

    #[test]
    fn doc_lines_follow_the_protocol() {
        let d = Doc::new("a\r\nbø\n", "php", 1);
        assert_eq!(d.line(0), Some("a"));
        assert_eq!(d.line(1), Some("bø"));
        // A trailing newline means one more, empty line.
        assert_eq!(d.line(2), Some(""));
        assert_eq!(d.line(3), None);
    }

    #[test]
    fn doc_positions_are_in_the_servers_units() {
        let d = Doc::new("// Håndter\nx\n", "php", 1);
        let enc = PositionEncoding::Utf16;
        // Byte 11 is the `\n` ending line 0: after `// Håndter` = 10 chars, 11 bytes.
        assert_eq!(d.position(11, enc), Position::new(0, 10));
        // Byte 12 starts line 1.
        assert_eq!(d.position(12, enc), Position::new(1, 0));
        // The end of the text (after the final `\n`) is the empty line 2.
        assert_eq!(d.position(14, enc), Position::new(2, 0));
        assert_eq!(d.position(999, enc), Position::new(2, 0));
    }

    #[test]
    fn minimal_change_finds_the_edit() {
        // Insertion in the middle.
        assert_eq!(
            minimal_change("$user->name", "$user->fullname"),
            Some((7, 7, "full"))
        );
        // Deletion.
        assert_eq!(minimal_change("abcdef", "abef"), Some((2, 4, "")));
        // Replacement.
        assert_eq!(
            minimal_change("foo bar baz", "foo qux baz"),
            Some((4, 7, "qux"))
        );
        // Typing at the end, and deleting everything.
        assert_eq!(minimal_change("ab", "abc"), Some((2, 2, "c")));
        assert_eq!(minimal_change("abc", ""), Some((0, 3, "")));
        assert_eq!(minimal_change("same", "same"), None);
        // Repeated text: a pure insert of `a` into `aaa` must not overlap the
        // prefix and suffix (`suffix` is capped by what's left after the prefix).
        assert_eq!(minimal_change("aaa", "aaaa"), Some((3, 3, "a")));
    }

    #[test]
    fn minimal_change_never_splits_a_character() {
        // `ø` (C3 B8) → `å` (C3 A5): the first byte is shared, but the edit must
        // cover the whole character in both strings.
        let (s, e, t) = minimal_change("Bjørn", "Bjårn").unwrap();
        assert_eq!((s, e, t), (2, 4, "å"));
        // Same on the suffix side: `…ø` vs `…å` share nothing usable at the end.
        let (s, e, t) = minimal_change("xø", "xå").unwrap();
        assert_eq!((s, e, t), (1, 3, "å"));
    }

    #[test]
    fn diagnostics_arrive_in_byte_columns() {
        let inner = test_inner();
        let uri = "file:///p/a.php";
        inner.docs.lock().unwrap().insert(
            uri.into(),
            Doc::new("// Håndter feil\n$x = fungerer();", "php", 1),
        );
        // The server flags `fungerer` on line 1: UTF-16 columns 5..13 (ASCII
        // line, so unchanged), and something at UTF-16 column 10 on line 0,
        // which sits after the 2-byte `å`, so bytes = 11.
        let mut diags = vec![
            Diagnostic::new_simple(
                Range::new(Position::new(1, 5), Position::new(1, 13)),
                "undefined".into(),
            ),
            Diagnostic::new_simple(
                Range::new(Position::new(0, 10), Position::new(0, 14)),
                "spelling".into(),
            ),
        ];
        Positions::new(&inner).diags_to_editor(uri, &mut diags);
        assert_eq!(diags[0].range.start.character, 5);
        assert_eq!(diags[0].range.end.character, 13);
        assert_eq!(diags[1].range.start.character, 11);
        assert_eq!(diags[1].range.end.character, 15);
    }

    #[test]
    fn workspace_edit_parses_both_shapes() {
        let changes = json!({ "changes": { "file:///a.php": [
            { "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1} }, "newText": "x" }
        ]}});
        let dc = json!({ "documentChanges": [ { "textDocument": { "uri": "file:///a.php", "version": 1 }, "edits": [
            { "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1} }, "newText": "x" }
        ]}]});
        for v in [changes, dc] {
            let edits = parse_workspace_edit(Some(&v));
            assert_eq!(edits.len(), 1);
            assert_eq!(edits[0].0, "file:///a.php");
            assert_eq!(edits[0].1[0].new_text, "x");
        }
    }

    /// A client over a dead channel: nothing is written, but the mirror and the
    /// queue behave as they would against a real server.
    fn offline_client(ready: u8) -> (LspClient, crossbeam_channel::Receiver<Vec<u8>>) {
        let (writer, frames) = unbounded();
        let mut inner = test_inner();
        inner.writer = writer;
        inner.ready = (Mutex::new(ready), Condvar::new());
        let client = LspClient {
            child: None,
            next_id: AtomicI64::new(1),
            inner: Arc::new(inner),
            capabilities: Mutex::new(Value::Null),
        };
        (client, frames)
    }

    fn method_of(frame: &[u8]) -> String {
        let body = &frame[frame.iter().position(|&b| b == b'{').unwrap()..];
        let v: Value = serde_json::from_slice(body).unwrap();
        v["method"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn edits_while_starting_fold_into_one_open_at_ready() {
        let (client, frames) = offline_client(STARTING);
        let uri = "file:///p/a.php";
        client.did_open(uri, "php", 1, "<?php\n");
        client.did_change(uri, 2, "<?php\n$a");
        client.did_change(uri, 3, "<?php\n$ab");
        client.did_save(uri, "<?php\n$ab");
        assert!(
            frames.is_empty(),
            "nothing reaches a server that isn't ready"
        );

        client.become_ready();
        let sent: Vec<Vec<u8>> = frames.try_iter().collect();
        let methods: Vec<String> = sent.iter().map(|f| method_of(f)).collect();
        assert_eq!(methods, ["textDocument/didOpen", "textDocument/didSave"]);
        // The open carries the latest text and version, not the original.
        let body = &sent[0][sent[0].iter().position(|&b| b == b'{').unwrap()..];
        let v: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(v["params"]["textDocument"]["text"], "<?php\n$ab");
        assert_eq!(v["params"]["textDocument"]["version"], 3);
    }

    #[test]
    fn incremental_change_sends_only_the_difference() {
        let (client, frames) = offline_client(READY);
        let uri = "file:///p/a.php";
        client.did_open(uri, "php", 1, "// Håndter\n$user->name;\n");
        let _ = frames.recv().unwrap(); // the didOpen
        client.did_change(uri, 2, "// Håndter\n$user->fullname;\n");
        let frame = frames.recv().unwrap();
        let body = &frame[frame.iter().position(|&b| b == b'{').unwrap()..];
        let v: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(v["method"], "textDocument/didChange");
        let change = &v["params"]["contentChanges"][0];
        assert_eq!(change["text"], "full");
        assert_eq!(
            change["range"]["start"],
            json!({ "line": 1, "character": 7 })
        );
        assert_eq!(change["range"]["end"], json!({ "line": 1, "character": 7 }));
        // A no-op change sends nothing at all.
        client.did_change(uri, 3, "// Håndter\n$user->fullname;\n");
        assert!(frames.is_empty());
    }

    #[test]
    fn full_sync_servers_get_the_whole_text() {
        let (client, frames) = offline_client(READY);
        client.inner.sync_kind.store(1, Ordering::SeqCst);
        let uri = "file:///p/a.php";
        client.did_open(uri, "php", 1, "a");
        let _ = frames.recv().unwrap();
        client.did_change(uri, 2, "ab");
        let frame = frames.recv().unwrap();
        let body = &frame[frame.iter().position(|&b| b == b'{').unwrap()..];
        let v: Value = serde_json::from_slice(body).unwrap();
        let change = &v["params"]["contentChanges"][0];
        assert_eq!(change["text"], "ab");
        assert!(change.get("range").is_none());
    }
}
