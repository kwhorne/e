#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // What the build scripts and anyone else can ask: the version this
    // binary is, without starting the editor.
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        println!("e {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    e_app::launch();
}
