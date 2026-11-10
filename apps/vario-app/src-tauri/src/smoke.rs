//! Desktop smoke test, for checking the app without a person clicking.
//!
//! Run with `VARIO_SMOKE_TEST=1 cargo run -p vario-app`: the app presses its own Start
//! button, lets the simulated flight run for a while, prints what the page shows and
//! exits. Because it goes through the real button and the real page, it also catches
//! errors in the JavaScript and in the connection between page and Rust.

use std::time::Duration;

use tauri::{AppHandle, Manager};

const ENV_VAR: &str = "VARIO_SMOKE_TEST";
/// Long enough for one 10-second summary line to appear.
const RUN_TIME: Duration = Duration::from_secs(14);

fn enabled() -> bool {
    std::env::var_os(ENV_VAR).is_some()
}

/// Starts the smoke test if the environment variable is set.
pub fn maybe_run(app: &AppHandle) {
    if !enabled() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(window) = app.get_webview_window("main") else {
            eprintln!("smoke test: no main window");
            app.exit(1);
            return;
        };
        // Give the page time to load.
        std::thread::sleep(Duration::from_secs(3));
        let _ = window.eval("document.getElementById('toggle').click()");
        std::thread::sleep(RUN_TIME);
        let _ = window.eval(
            "window.__TAURI__.core.invoke('smoke_report', { text: document.body.innerText })",
        );
        // If the page never answers, it is broken: fail instead of hanging.
        std::thread::sleep(Duration::from_secs(5));
        eprintln!("smoke test: the page did not report back");
        app.exit(1);
    });
}

/// Called by the page at the end of the smoke test with its visible text.
#[tauri::command]
pub fn smoke_report(app: AppHandle, text: String) {
    if enabled() {
        println!("--- page content ---\n{text}\n--- end ---");
        app.exit(0);
    }
}
