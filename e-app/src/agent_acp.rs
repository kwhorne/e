//! Which agent Cascade runs. Elyra speaks its own RPC; the others speak the
//! Agent Client Protocol (see [`e_agent::acp`]), through the agent's own ACP
//! mode or an adapter — so Claude Code, Codex, Gemini CLI and the rest get the
//! same panel: the transcript, turn receipts with Undo, worktree sessions, the
//! context meter. They differ in what ACP lacks: no steering (a message sent
//! while the agent works runs next) and no model menu (the agent's own
//! settings choose it). They ask before acting, and the answer is a card in
//! the transcript.

use std::path::Path;

use e_agent::ChatItem;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::config;
use crate::state::AppState;

/// An ACP agent Cascade can run.
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// The command line that speaks ACP, run through the login shell.
    pub command: &'static str,
    /// The program that must be installed (and signed in) for it to work.
    pub needs: &'static str,
    /// How to get it ready, when it isn't.
    pub hint: &'static str,
}

/// The agents, in the order offered. Adapters run through `npx` are pinned,
/// so a session isn't changed under the user by an adapter update.
pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "claude",
        name: "Claude Code",
        command: "npx -y @agentclientprotocol/claude-agent-acp@0.98.0",
        needs: "claude",
        hint: "Install Claude Code and sign in with `claude` in a terminal",
    },
    Provider {
        id: "codex",
        name: "Codex",
        command: "npx -y @agentclientprotocol/codex-acp@2.3.0",
        needs: "codex",
        hint: "Install Codex and run `codex login` in a terminal",
    },
    Provider {
        id: "gemini",
        name: "Gemini CLI",
        command: "gemini --experimental-acp",
        needs: "gemini",
        hint: "Install Gemini CLI and sign in by running `gemini` in a terminal",
    },
    Provider {
        id: "copilot",
        name: "GitHub Copilot",
        command: "copilot --acp",
        needs: "copilot",
        hint: "Install the Copilot CLI and sign in by running `copilot` in a terminal",
    },
    Provider {
        id: "cursor",
        name: "Cursor",
        command: "cursor-agent acp",
        needs: "cursor-agent",
        hint: "Install cursor-agent and run `cursor-agent login` in a terminal",
    },
    Provider {
        id: "opencode",
        name: "OpenCode",
        command: "opencode acp",
        needs: "opencode",
        hint: "Install OpenCode and run `opencode auth login` in a terminal",
    },
    Provider {
        id: "goose",
        name: "Goose",
        command: "goose acp",
        needs: "goose",
        hint: "Install Goose and run `goose configure` in a terminal",
    },
    Provider {
        id: "qwen",
        name: "Qwen Code",
        command: "qwen --experimental-acp",
        needs: "qwen",
        hint: "Install Qwen Code and sign in by running `qwen` in a terminal",
    },
    Provider {
        id: "kimi",
        name: "Kimi CLI",
        command: "kimi acp",
        needs: "kimi",
        hint: "Install Kimi CLI and sign in by running `kimi` in a terminal",
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// Whether `program` is on the PATH (the login shell's, which `e` adopts
/// at startup).
pub fn installed(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let p = dir.join(program);
            p.is_file() && is_executable(&p)
        })
    })
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        true
    }
}

impl AppState {
    /// The ACP agent Cascade runs — its name and the command line that
    /// speaks ACP — or `None` for Elyra. `custom` is any other ACP agent,
    /// given by `cascade_custom_command` in config.json.
    pub(crate) fn cascade_acp(&self) -> Option<(String, String)> {
        let id = self.settings.get_untracked().cascade_agent;
        if id == "custom" {
            let command = config::read()
                .get("cascade_custom_command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            return (!command.is_empty()).then(|| ("Custom agent".to_string(), command));
        }
        provider(&id).map(|p| (p.name.to_string(), p.command.to_string()))
    }

    /// The name of the agent Cascade runs: "Elyra", "Claude Code"…
    pub(crate) fn cascade_agent_name(&self) -> String {
        self.cascade_acp()
            .map_or_else(|| "Elyra".to_string(), |(name, _)| name)
    }

    /// Run `id` in Cascade from now on ("elyra", "claude", …), in a new
    /// session.
    pub fn set_cascade_agent(&self, id: &str) {
        if let Some(p) = provider(id) {
            if !installed(p.needs) {
                self.agent.chat.update(|c| {
                    c.items.push(ChatItem::Notice {
                        text: format!("{} isn't installed: {}", p.name, p.hint),
                        error: true,
                    })
                });
                return;
            }
        }
        let id = if provider(id).is_some() || id == "custom" {
            id
        } else {
            "elyra"
        };
        self.settings.update(|s| s.cascade_agent = id.to_string());
        config::set_str("cascade_agent", id);
        if let Some(client) = self.agent.native_client.get_untracked() {
            client.shutdown();
        }
        self.agent.native_client.set(None);
        self.agent.turn_before.set(None);
        self.start_native_agent();
        self.agent.focus_pulse.update(|x| *x += 1);
    }

    /// Answer the permission request at `index` in the transcript with the
    /// choice `choice` (its id or its name).
    pub fn answer_permission(&self, index: usize, choice: &str) -> Result<String, String> {
        let ask = self
            .agent
            .chat
            .with_untracked(|c| match c.items.get(index) {
                Some(ChatItem::Permission(p)) => Some(p.clone()),
                _ => None,
            });
        let ask = ask.ok_or("there is no permission request there")?;
        if ask.answered.is_some() {
            return Err("it is answered already".into());
        }
        let picked = ask
            .choices
            .iter()
            .find(|c| c.id == choice || c.name.eq_ignore_ascii_case(choice))
            .ok_or_else(|| {
                format!(
                    "no choice {choice:?}: {}",
                    ask.choices
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let client = self
            .agent
            .native_client
            .get_untracked()
            .ok_or("the agent isn't running")?;
        client.answer_permission(&ask.request_id, &picked.id);
        let name = picked.name.clone();
        self.agent
            .chat
            .update(|c| c.mark_answered(index, name.clone()));
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_command_and_a_way_to_get_ready() {
        for p in PROVIDERS {
            assert!(
                !p.command.is_empty() && !p.needs.is_empty() && !p.hint.is_empty(),
                "{}",
                p.id
            );
        }
        assert!(provider("claude").is_some_and(|p| p.command.contains("claude-agent-acp@")));
        assert!(provider("elyra").is_none());
    }

    #[test]
    fn a_program_is_installed_when_it_is_on_the_path() {
        assert!(installed("sh"));
        assert!(!installed("surely-not-a-real-program-xyz"));
    }
}
