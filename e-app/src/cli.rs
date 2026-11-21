//! What `e` makes of its command line. Flags only before a `--`; after it,
//! everything is a path — `e -- -V` opens the file `-V`.

/// `e --help`'s text.
pub const USAGE: &str = "\
Usage: e [options] [--] [path]

Opens path (a folder, or a file in its folder) in the editor; with no path,
the last project.

Options:
  -f, --foreground  Stay attached to the terminal (logs on stderr)
  -V, --version     Print the version and exit
  -h, --help        Print this help and exit
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    pub version: bool,
    pub help: bool,
    pub foreground: bool,
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
    fn after_a_double_dash_everything_is_a_path() {
        let c = p(&["--", "-V"]);
        assert!(!c.version);
        assert_eq!(c.path.as_deref(), Some("-V"));
        let c = p(&["-f", "--", "--help"]);
        assert!(c.foreground && !c.help);
        assert_eq!(c.path.as_deref(), Some("--help"));
    }
}
