#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // What the build scripts and anyone else can ask: the version this
    // binary is, without starting the editor.
    let cli = e_app::cli::current();
    if cli.version {
        println!("e {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if cli.help {
        print!("{}", e_app::cli::USAGE);
        return;
    }
    // In e's terminal: the file opens in that window (and `--wait` waits
    // for it), instead of a second editor starting.
    if let Some(code) = e_app::cli::hand_to_running(&cli) {
        std::process::exit(code);
    }
    e_app::launch();
}
