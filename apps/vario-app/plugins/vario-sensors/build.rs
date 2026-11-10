// The plugin exposes no commands to JavaScript; it is only called from Rust.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
