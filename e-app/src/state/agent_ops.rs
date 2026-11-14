//! The agent panel and the agent socket's edits.

use super::*;

impl AppState {
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
        if !self.settings.get_untracked().cascade {
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
        self.start_native_agent_with(None);
    }

    /// Start Elyra headless, on the model and thinking level Cascade remembers,
    /// with the API keys from Settings in its environment — and, given a
    /// session file, resuming that conversation.
    pub(crate) fn start_native_agent_with(&self, session: Option<&std::path::Path>) {
        let Some(agent) = self.current_agent() else {
            return;
        };
        self.mark_review_session();
        // In a worktree session, the agent works there; otherwise where the
        // agent is configured to, or the project.
        let cwd = if self.agent.worktree.with_untracked(Option::is_some) {
            self.agent_root()
        } else if agent.cwd.trim().is_empty() {
            self.root.get_untracked()
        } else {
            PathBuf::from(&agent.cwd)
        };
        // Run through the user's login shell so the full PATH (nvm/npm/Grove
        // node, etc.) is available — a GUI app launched from Finder/Dock inherits
        // only a minimal PATH, so `elyra` (and the `node` its shebang needs)
        // wouldn't be found by a direct spawn. This mirrors the terminal agent.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let settings = self.settings.get_untracked();
        let mut cmdline = format!("{} --mode rpc", agent.command.trim());
        if let Some((provider, model)) = settings.cascade_model.split_once('/') {
            cmdline.push_str(&format!(
                " --provider {} --model {}",
                crate::cascade::shell_quote(provider),
                crate::cascade::shell_quote(model)
            ));
        }
        if !settings.cascade_thinking.is_empty() {
            cmdline.push_str(&format!(
                " --thinking {}",
                crate::cascade::shell_quote(&settings.cascade_thinking)
            ));
        }
        if let Some(path) = session {
            cmdline.push_str(&format!(
                " --session {}",
                crate::cascade::shell_quote(&path.to_string_lossy())
            ));
        }
        let shell_args = vec!["-lc".to_string(), cmdline];
        // The keys from Settings, as the environment variables elyra reads.
        let mut env = crate::secrets::elyra_env();
        env.extend(crate::agent_sync::socket_env());

        // Another agent, over the Agent Client Protocol. It signs in by
        // itself, so Elyra's keys stay out of its environment.
        let spawned = match self.cascade_acp() {
            Some((_, command)) => AgentClient::spawn_acp(
                &shell,
                &["-lc".to_string(), command],
                &cwd,
                &crate::agent_sync::socket_env(),
            ),
            None => AgentClient::spawn(&shell, &shell_args, &cwd, &env),
        };
        match spawned {
            Ok((client, rx)) => {
                self.agent.chat.set(ChatState::new());
                // What model are we on, what else is there — and, when resuming,
                // the conversation so far.
                let _ = client.get_state();
                let _ = client.get_available_models();
                let _ = client.get_session_stats();
                if session.is_some() {
                    let _ = client.get_messages();
                }
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
    /// the agent is streaming, the message is queued rather than rejected: to
    /// steer the running work, or — `next` — to run once it is done.
    pub fn send_native_prompt(&self, text: &str) {
        self.send_native_prompt_as(text, false);
    }

    pub fn send_native_prompt_as(&self, text: &str, next: bool) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self.agent.native_client.get_untracked().is_none() {
            self.start_native_agent();
        }
        // Whether a turn was already running before this message.
        let busy = self.agent.chat.with_untracked(|c| c.running);
        let Some(client) = self.agent.native_client.get_untracked() else {
            // Show the message anyway, with why nothing will answer it.
            self.agent.chat.update(|c| c.push_user(text));
            self.agent.chat.update(|c| {
                c.apply(e_agent::AgentEvent::Error {
                    message: "The agent isn't running — check that `elyra` is installed and on your PATH."
                        .to_string(),
                })
            });
            self.agent.composer.set(String::new());
            return;
        };
        let streaming = busy.then_some(if next {
            Streaming::FollowUp
        } else {
            Streaming::Steer
        });
        let shown = text.to_string();
        // Ask mode: the same agent, told to keep its hands off the files.
        let ask = self.agent.mode.get_untracked() == "ask";
        let text = if ask {
            format!(
                "[Ask mode] Answer, explain or plan only. Do not edit, create or delete files, \
                 and do not run commands that change anything.\n\n{text}"
            )
        } else {
            text.to_string()
        };
        // A new session's first message carries the project's rules.
        let first = self.agent.chat.with_untracked(|c| {
            !c.items
                .iter()
                .any(|i| matches!(i, e_agent::ChatItem::User { .. }))
        });
        let text = match self.rules_preamble().filter(|_| first) {
            Some(rules) => format!("{rules}\n{text}"),
            None => text,
        };
        // Idle, it shows at once; queued, where the agent takes it up.
        self.agent.chat.update(|c| c.send(&shown, &text, busy));
        let send = move || {
            if let Err(e) = client.prompt(&text, streaming) {
                eprintln!("e: agent prompt failed: {e:#}");
            }
        };
        // A new turn starts from a snapshot of the tree, for its receipt; a
        // message queued into a running turn belongs to that one.
        if busy {
            send();
        } else {
            self.turn_begin(send);
        }
        self.agent.composer.set(String::new());
    }

    /// Send the composer's contents and clear it. Both the network send and the
    /// document edit are deferred out of the current event: mutating the editor's
    /// document from inside its own key handler is a reentrant borrow and aborts
    /// across the objc FFI boundary.
    pub fn send_composer(&self) {
        self.send_composer_as(false);
    }

    /// Send the composer's contents to run once the agent is done with its
    /// current work (⌥↵), rather than to steer it.
    pub fn send_composer_next(&self) {
        self.send_composer_as(true);
    }

    pub(crate) fn send_composer_as(&self, next: bool) {
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
            st.send_native_prompt_as(&text, next);
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
        let env = crate::agent_sync::socket_env();
        match Terminal::spawn_command(&agent.command, &cwd, &env, 30, 100, on_update) {
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
            .editor
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
        let open = self.editor.buffers.with_untracked(|bs| {
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
        if self.tests.fix_loop.get_untracked() && applied > 0 {
            self.run_tests();
        }
    }
}
