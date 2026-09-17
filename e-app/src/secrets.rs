//! API keys for the model providers, kept out of `config.json`.
//!
//! On macOS a key goes into the login Keychain through the `security` tool
//! (one generic-password item per provider, account = the user); elsewhere
//! into `~/.config/e/secrets.json` with mode 0600. Elyra reads providers'
//! keys from environment variables, so [`elyra_env`] hands the stored keys to
//! it when it is spawned — the user enters a key once, in Settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

/// The providers the Settings page offers a key field for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
    Gemini,
    Grok,
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::Anthropic,
        Provider::OpenAi,
        Provider::Gemini,
        Provider::Grok,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAi => "OpenAI",
            Provider::Gemini => "Gemini",
            Provider::Grok => "Grok (xAI)",
        }
    }

    /// The environment variable elyra reads the key from.
    pub fn env_var(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAi => "OPENAI_API_KEY",
            Provider::Gemini => "GEMINI_API_KEY",
            Provider::Grok => "XAI_API_KEY",
        }
    }

    /// elyra's provider id, as its model objects name it.
    pub fn elyra_id(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Gemini => "google",
            Provider::Grok => "xai",
        }
    }

    pub fn from_elyra_id(id: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.elyra_id() == id)
    }

    /// What a key from this provider looks like, for the placeholder.
    pub fn key_hint(self) -> &'static str {
        match self {
            Provider::Anthropic => "sk-ant-…",
            Provider::OpenAi => "sk-…",
            Provider::Gemini => "AIza…",
            Provider::Grok => "xai-…",
        }
    }

    /// The storage key: also the Keychain service name.
    fn slot(self) -> String {
        format!("e: {} API key", self.label())
    }
}

/// Where keys live. Chosen once per call so tests can point at a file.
enum Store {
    #[cfg(target_os = "macos")]
    Keychain,
    File(PathBuf),
}

fn store() -> Store {
    if let Some(p) = std::env::var_os("E_SECRETS_FILE") {
        return Store::File(PathBuf::from(p));
    }
    #[cfg(target_os = "macos")]
    {
        Store::Keychain
    }
    #[cfg(not(target_os = "macos"))]
    {
        Store::File(default_file())
    }
}

#[cfg(not(target_os = "macos"))]
fn default_file() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".config")
        .join("e")
        .join("secrets.json")
}

/// The stored key for `p`, if any.
pub fn get(p: Provider) -> Option<String> {
    match store() {
        #[cfg(target_os = "macos")]
        Store::Keychain => keychain_get(&p.slot()),
        Store::File(path) => file_read(&path).get(&p.slot()).cloned(),
    }
    .filter(|k| !k.trim().is_empty())
}

/// Store (or replace) the key for `p`. An empty key removes it.
pub fn set(p: Provider, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return clear(p);
    }
    match store() {
        #[cfg(target_os = "macos")]
        Store::Keychain => keychain_set(&p.slot(), key),
        Store::File(path) => {
            let mut all = file_read(&path);
            all.insert(p.slot(), key.to_string());
            file_write(&path, &all)
        }
    }
}

/// Remove the key for `p`.
pub fn clear(p: Provider) -> Result<(), String> {
    match store() {
        #[cfg(target_os = "macos")]
        Store::Keychain => keychain_delete(&p.slot()),
        Store::File(path) => {
            let mut all = file_read(&path);
            all.remove(&p.slot());
            file_write(&path, &all)
        }
    }
}

pub fn has(p: Provider) -> bool {
    get(p).is_some()
}

/// Whether any provider has a key.
pub fn any_configured() -> bool {
    Provider::ALL.into_iter().any(has)
}

/// `(ENV_VAR, key)` for every stored key — the environment elyra is spawned
/// with. A variable already set in the process wins, so a shell export still
/// works the way it always did.
pub fn elyra_env() -> Vec<(String, String)> {
    Provider::ALL
        .into_iter()
        .filter(|p| std::env::var_os(p.env_var()).is_none())
        .filter_map(|p| get(p).map(|k| (p.env_var().to_string(), k)))
        .collect()
}

/// `sk-ant-api03-…-1234` → `••••1234`, for showing that a key exists.
pub fn mask(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("••••{tail}")
}

/// Whether elyra has credentials of its own (`/login`, or keys added to
/// `~/.elyra/agent/auth.json`). elyra writes an empty `{}` there on its first
/// start, so the file existing means nothing; its entries do.
pub fn elyra_has_credentials() -> bool {
    std::env::var_os("HOME")
        .and_then(|h| std::fs::read_to_string(PathBuf::from(h).join(".elyra/agent/auth.json")).ok())
        .map(|text| auth_has_credentials(&text))
        .unwrap_or(false)
}

/// Does an elyra `auth.json` hold a usable credential: an `api_key` entry with
/// a key, or an OAuth entry?
pub fn auth_has_credentials(text: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    let Some(map) = v.as_object() else {
        return false;
    };
    map.values().any(|entry| {
        let ty = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let key = entry.get("key").and_then(|k| k.as_str()).unwrap_or("");
        match ty {
            "api_key" => !key.trim().is_empty(),
            "oauth" => true,
            _ => entry.get("access").is_some() || entry.get("refresh").is_some(),
        }
    })
}

// ---- Keychain (macOS) ------------------------------------------------------

#[cfg(target_os = "macos")]
fn account() -> String {
    std::env::var("USER").unwrap_or_else(|_| "e".to_string())
}

#[cfg(target_os = "macos")]
fn keychain_get(service: &str) -> Option<String> {
    let out = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-a",
            &account(),
            "-s",
            service,
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn keychain_set(service: &str, key: &str) -> Result<(), String> {
    // `-U` updates the item in place when it exists.
    let out = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            &account(),
            "-s",
            service,
            "-l",
            service,
            "-w",
            key,
        ])
        .output()
        .map_err(|e| format!("couldn't run security: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "macos")]
fn keychain_delete(service: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-a", &account(), "-s", service])
        .output()
        .map_err(|e| format!("couldn't run security: {e}"))?;
    // Deleting what isn't there is fine.
    if out.status.success() || String::from_utf8_lossy(&out.stderr).contains("could not be found") {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// ---- File store ------------------------------------------------------------

fn file_read(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn file_write(path: &Path, all: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(all).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_map_to_elyra_ids_and_env_vars() {
        assert_eq!(Provider::from_elyra_id("google"), Some(Provider::Gemini));
        assert_eq!(Provider::from_elyra_id("xai"), Some(Provider::Grok));
        assert_eq!(Provider::from_elyra_id("mistral"), None);
        assert_eq!(Provider::Grok.env_var(), "XAI_API_KEY");
        assert_eq!(Provider::Gemini.env_var(), "GEMINI_API_KEY");
    }

    #[test]
    fn mask_keeps_only_the_tail() {
        assert_eq!(mask("sk-ant-api03-abcdef-1234"), "••••1234");
        assert_eq!(mask("ab"), "••••ab");
    }

    #[test]
    fn file_store_round_trips_and_is_private() {
        let dir = std::env::temp_dir().join(format!("e-secrets-{}", std::process::id()));
        let path = dir.join("secrets.json");
        let mut all = BTreeMap::new();
        all.insert(Provider::OpenAi.slot(), "sk-test".to_string());
        file_write(&path, &all).unwrap();
        assert_eq!(
            file_read(&path)
                .get(&Provider::OpenAi.slot())
                .map(String::as_str),
            Some("sk-test")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        all.remove(&Provider::OpenAi.slot());
        file_write(&path, &all).unwrap();
        assert!(file_read(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn elyras_auth_file_counts_only_with_real_entries() {
        assert!(!auth_has_credentials("{}"));
        assert!(!auth_has_credentials("not json"));
        assert!(!auth_has_credentials(
            r#"{"anthropic": {"type": "api_key", "key": ""}}"#
        ));
        assert!(auth_has_credentials(
            r#"{"anthropic": {"type": "api_key", "key": "sk-ant-x"}}"#
        ));
        assert!(auth_has_credentials(
            r#"{"openai-codex": {"type": "oauth", "access": "a", "refresh": "r"}}"#
        ));
    }
}
