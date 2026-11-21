//! What `e` makes of its command line. Flags only before a `--`; after it,
//! everything is a path — `e -- -V` opens the file `-V`.

/// `e --help`'s text.
pub const USAGE: &str = "\
Usage: e [options] [--] [path]

Opens path (a folder, or a file in its folder) in the editor; with no path,
the last project. In e's own terminal a file opens in that window.

Options:
  -w, --wait        Wait for the file's tab to be closed before returning:
                    for GIT_EDITOR, EDITOR and the like
  -f, --foreground  Stay attached to the terminal (logs on stderr)
  -V, --version     Print the version and exit
  -h, --help        Print this help and exit
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    pub version: bool,
    pub help: bool,
    pub foreground: bool,
    pub wait: bool,
    /// The first path given.
    pub path: Option<String>,
}

/// Read the arguments after the program name. Flags it doesn't know are
/// passed over, as before: macOS has handed apps flags of its own.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Cli {
    let mut cli = Cli::default();
    let mut flags = true;
    for arg in args {
        if flags && arg == "--" {
            flags = false;
            continue;
        }
        if flags && arg.starts_with('-') && arg.len() > 1 {
            match arg.as_str() {
                "--version" | "-V" => cli.version = true,
                "--help" | "-h" => cli.help = true,
                "--foreground" | "-f" => cli.foreground = true,
                "--wait" | "-w" => cli.wait = true,
                _ => {}
            }
            continue;
        }
        if cli.path.is_none() {
            cli.path = Some(arg);
        }
    }
    cli
}

/// Hand the file to the editor this runs under — in e's terminal,
/// `$E_EDITOR_SOCK` names its socket — and with `--wait`, return once its
/// tab is closed. The exit code, or `None` to start an editor of its own (no
/// socket, a folder, no such file).
#[cfg(unix)]
pub fn hand_to_running(cli: &Cli) -> Option<i32> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let sock = std::env::var_os("E_EDITOR_SOCK").filter(|s| !s.is_empty())?;
    let Some(path) = &cli.path else {
        if cli.wait {
            eprintln!("e: --wait needs a file to edit");
            return Some(2);
        }
        return None;
    };
    let cwd = std::env::current_dir().ok()?;
    let abs = cwd.join(path);
    if !abs.is_file() {
        if cli.wait {
            eprintln!("e: {} is not a file", abs.display());
            return Some(1);
        }
        return None;
    }
    let abs = abs.canonicalize().unwrap_or(abs);
    let Ok(stream) = UnixStream::connect(&sock) else {
        // The editor that started this terminal is gone: be one.
        return None;
    };
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut writer = stream;
    let mut ask = |req: serde_json::Value| -> Option<serde_json::Value> {
        writeln!(writer, "{req}").ok()?;
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    };
    let path = abs.to_string_lossy();
    let opened = ask(serde_json::json!({"method": "edit", "path": path}));
    match opened {
        Some(v) if v["ok"] == true => {}
        Some(v) => {
            eprintln!(
                "e: {}",
                v["error"].as_str().unwrap_or("the file didn't open")
            );
            return Some(1);
        }
        None => {
            eprintln!("e: the editor didn't answer");
            return Some(1);
        }
    }
    if !cli.wait {
        return Some(0);
    }
    loop {
        match ask(serde_json::json!({"method": "wait_closed", "path": path, "timeout_ms": 240_000}))
        {
            Some(v) if v["closed"] == true => return Some(0),
            Some(v) if v["ok"] == true => continue,
            // The editor quit with the file open: not a finished edit.
            _ => {
                eprintln!("e: the editor closed before the file was");
                return Some(1);
            }
        }
    }
}

#[cfg(not(unix))]
pub fn hand_to_running(cli: &Cli) -> Option<i32> {
    let _ = cli;
    None
}

/// This process's own arguments, read.
pub fn current() -> Cli {
    parse(std::env::args().skip(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Cli {
        parse(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn flags_and_the_path() {
        assert!(p(&["-V"]).version);
        assert!(p(&["--help"]).help);
        assert!(p(&["-h"]).help);
        let c = p(&["--foreground", "app/Models/User.php", "other"]);
        assert!(c.foreground);
        assert_eq!(c.path.as_deref(), Some("app/Models/User.php"));
        assert_eq!(p(&[]), Cli::default());
    }

    #[test]
    fn wait_is_a_flag() {
        let c = p(&["--wait", "COMMIT_EDITMSG"]);
        assert!(c.wait);
        assert_eq!(c.path.as_deref(), Some("COMMIT_EDITMSG"));
        assert!(p(&["-w", "x"]).wait);
    }

    #[test]
    fn after_a_double_dash_everything_is_a_path() {
        let c = p(&["--", "-V"]);
        assert!(!c.version);
        assert_eq!(c.path.as_deref(), Some("-V"));
        let c = p(&["-f", "--", "--help"]);
        assert!(c.foreground && !c.help);
        assert_eq!(c.path.as_deref(), Some("--help"));
    }
}
