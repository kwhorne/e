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
    e_app::launch();
}
