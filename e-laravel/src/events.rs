//! Event → listener navigation and a dispatch graph. Without this, event-driven
//! apps are an archaeology project; with it the architecture map is complete:
//! routes, views, and the asynchronous flow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[derive(Clone)]
pub struct EventNode {
    pub event: String,
    pub event_file: Option<PathBuf>,
    pub listeners: Vec<(String, Option<PathBuf>)>,
}

/// The class name that precedes `::class` ending at byte `end`.
fn class_before(src: &str, end: usize) -> String {
    let cls: String = src[..end]
        .chars()
        .rev()
        .take_while(|c| is_ident(*c) || *c == '\\')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    cls.rsplit('\\').next().unwrap_or(&cls).to_string()
}

/// Parse `Event::class => [Listener::class, …]` entries from an EventServiceProvider.
fn parse_listen(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(rel) = src[search..].find("::class") {
        let at = search + rel;
        search = at + 7;
        // Is this a `=> [` array key?
        let after = src[at + 7..].trim_start();
        if !after.starts_with("=>") {
            continue;
        }
        let event = class_before(src, at);
        if event.is_empty() {
            continue;
        }
        // Find the `[ … ]` value and its `X::class` listeners.
        let rest = &src[at + 7..];
        if let Some(ob) = rest.find('[') {
            if let Some(cb) = rest[ob..].find(']') {
                let inner = &rest[ob..ob + cb];
                let mut s = 0;
                while let Some(r) = inner[s..].find("::class") {
                    let a = s + r;
                    s = a + 7;
                    let listener = class_before(inner, a);
                    if !listener.is_empty() {
                        out.push((event.clone(), listener));
                    }
                }
            }
        }
    }
    out
}

/// The event type hinted in a listener's `handle()` method.
fn handle_event(src: &str) -> Option<String> {
    let at = src.find("function handle(")? + "function handle(".len();
    let end = src[at..].find(')')? + at;
    let param = src[at..end].split(',').next()?;
    let ty: String = param
        .trim()
        .trim_start_matches('\\')
        .chars()
        .take_while(|c| is_ident(*c) || *c == '\\')
        .collect();
    let base = ty.rsplit('\\').next().unwrap_or(&ty).to_string();
    if base.is_empty()
        || base
            .chars()
            .next()
            .map(|c| c.is_lowercase())
            .unwrap_or(true)
    {
        None
    } else {
        Some(base)
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(read) = std::fs::read_dir(dir) {
        for e in read.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files(&p, out);
            } else if p.extension().and_then(|x| x.to_str()) == Some("php") {
                out.push(p);
            }
        }
    }
}

/// Build the event → listeners map for the project.
pub fn dispatch_map(root: &Path) -> Vec<EventNode> {
    // Listener name → file.
    let mut listener_files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut listeners = Vec::new();
    collect_files(&root.join("app/Listeners"), &mut listeners);
    let mut map: BTreeMap<String, Vec<(String, Option<PathBuf>)>> = BTreeMap::new();

    for f in &listeners {
        if let Some(stem) = f.file_stem().and_then(|s| s.to_str()) {
            listener_files.insert(stem.to_string(), f.clone());
        }
    }
    // Auto-discovered listeners: handle(EventType $event).
    for f in &listeners {
        if let Ok(src) = std::fs::read_to_string(f) {
            if let Some(event) = handle_event(&src) {
                let name = f
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                map.entry(event).or_default().push((name, Some(f.clone())));
            }
        }
    }
    // EventServiceProvider `$listen`, and `Event::listen(...)`.
    let mut providers = Vec::new();
    collect_files(&root.join("app/Providers"), &mut providers);
    for f in &providers {
        if let Ok(src) = std::fs::read_to_string(f) {
            for (event, listener) in parse_listen(&src) {
                let file = listener_files.get(&listener).cloned();
                let list = map.entry(event).or_default();
                if !list.iter().any(|(n, _)| *n == listener) {
                    list.push((listener, file));
                }
            }
        }
    }

    // Event files.
    let mut event_files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut events = Vec::new();
    collect_files(&root.join("app/Events"), &mut events);
    for f in &events {
        if let Some(stem) = f.file_stem().and_then(|s| s.to_str()) {
            event_files.insert(stem.to_string(), f.clone());
        }
    }

    map.into_iter()
        .map(|(event, listeners)| EventNode {
            event_file: event_files.get(&event).cloned(),
            event,
            listeners,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listen_array() {
        let src = r#"protected $listen = [
            OrderShipped::class => [
                SendShipmentNotification::class,
                UpdateInventory::class,
            ],
        ];"#;
        let pairs = parse_listen(src);
        assert!(pairs.contains(&(
            "OrderShipped".to_string(),
            "SendShipmentNotification".to_string()
        )));
        assert!(pairs.contains(&("OrderShipped".to_string(), "UpdateInventory".to_string())));
    }

    #[test]
    fn reads_handle_event() {
        let src = "public function handle(OrderShipped $event): void { }";
        assert_eq!(handle_event(src).as_deref(), Some("OrderShipped"));
        assert!(handle_event("public function handle($event) {}").is_none());
    }
}
