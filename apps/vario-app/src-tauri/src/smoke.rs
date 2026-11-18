//! Desktop smoke test, for checking the app without a person clicking.
//!
//! Run with `VARIO_SMOKE_TEST=1 cargo run -p vario-app`: the app presses its own Start
//! button, lets the simulated flight run for a while, opens the map, the flight list and
//! the recorded flight, prints what the pages show (plus the number of track points and
//! any script errors) and exits. Because it goes through the real button and the real page, it also catches
//! errors in the JavaScript and in the connection between page and Rust.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Listener, Manager};

const ENV_VAR: &str = "VARIO_SMOKE_TEST";
/// Long enough for one 10-second summary line to appear.
const RUN_TIME: Duration = Duration::from_secs(14);
/// Address of a desktop share (e.g. the outdoor_app's `share-serve`): if set, the
/// smoke test also connects to it and downloads the first region it offers. That
/// writes into the app's real `maps` folder, so run it with `HOME` set to an empty
/// temporary folder.
const SHARE_ENV_VAR: &str = "VARIO_SMOKE_SHARE";
/// How long the region download may take.
const DOWNLOAD_TIME: Duration = Duration::from_secs(120);

pub fn enabled() -> bool {
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
        // Give the page time to load. How long that takes varies (first launch on a
        // phone is slow), so try a few times; once started the button no longer says
        // "Start" and further attempts do nothing.
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(1500));
            let _ = window.eval(
                "{ const b = document.getElementById('toggle'); \
                 if (b && b.textContent === 'Start') b.click(); }",
            );
        }
        std::thread::sleep(RUN_TIME / 2);
        // Visit the map so that errors in creating it are caught as well.
        let _ = window.eval("document.querySelector('[data-page=map]').click()");
        std::thread::sleep(RUN_TIME / 2);
        // The flight list, then the first flight on the map.
        let _ = window.eval("document.querySelector('[data-page=flights]').click()");
        std::thread::sleep(Duration::from_secs(1));
        let _ = window.eval("document.querySelector('.flight .button')?.click()");
        std::thread::sleep(Duration::from_secs(1));
        // Tap the middle of the map like a user would, with following on (the map moves
        // under the finger). The events go to whatever lies on top there, as with a real
        // tap. The airspace list should open.
        let _ = window.eval(
            "document.querySelector('[data-page=map]').click();\
             document.getElementById('close-saved-flight').click();\
             if (!document.getElementById('follow').classList.contains('on')) {\
               document.getElementById('follow').click();\
             }\
             setTimeout(() => {\
               const box = document.querySelector('#map canvas').getBoundingClientRect();\
               const at = { bubbles: true, button: 0, clientX: box.left + box.width / 2,\
                            clientY: box.top + box.height / 2 };\
               const canvas = document.elementFromPoint(at.clientX, at.clientY);\
               canvas.dispatchEvent(new PointerEvent('pointerdown', { ...at, pointerType: 'mouse', isPrimary: true }));\
               canvas.dispatchEvent(new MouseEvent('mousedown', at));\
               setTimeout(() => {\
                 canvas.dispatchEvent(new PointerEvent('pointerup', { ...at, pointerType: 'mouse', isPrimary: true }));\
                 canvas.dispatchEvent(new MouseEvent('mouseup', at));\
                 canvas.dispatchEvent(new MouseEvent('click', at));\
               }, 150);\
             }, 300)",
        );
        std::thread::sleep(Duration::from_secs(1));
        if let Some(address) = share_address() {
            download_from_share(&window, &address);
        }
        let _ = window.eval(
            "window.__TAURI__.core.invoke('smoke_report', { text: \
             [...document.querySelectorAll('.page')].map((page) => page.innerText).join('\\n') + \
             '\\ntrack points: ' + FlightMap.trackLength() + \
             '\\nairspaces: ' + FlightMap.airspaceCount() + \
             '\\ntapped airspaces: ' + (document.getElementById('airspace-info').hidden ? '(list not shown) ' : '') + \
             document.getElementById('airspace-info-list').innerText.replace(/\\n/g, ' | ') + \
             '\\npermission warnings: ' + \
             document.querySelectorAll('#permission-problems li').length + \
             '\\nscript errors: ' + JSON.stringify(window.scriptErrors) })",
        );
        // If the page never answers, it is broken: fail instead of hanging.
        std::thread::sleep(Duration::from_secs(5));
        eprintln!("smoke test: the page did not report back");
        app.exit(1);
    });
}

fn share_address() -> Option<String> {
    std::env::var(SHARE_ENV_VAR)
        .ok()
        .filter(|address| !address.is_empty())
}

/// Goes through the settings like a person: enters the address, connects, presses the
/// first "Laden" or "Aktualisieren" button and waits until the page says how it ended.
fn download_from_share(window: &tauri::WebviewWindow, address: &str) {
    // The download reports its end as an event; listen to it here in Rust.
    let ended = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ended);
    window.listen_any(crate::share::DOWNLOAD_EVENT, move |event| {
        if !event.payload().contains("\"state\":\"running\"") {
            println!("region download: {}", event.payload());
            flag.store(true, Ordering::SeqCst);
        }
    });
    let _ = window.eval("document.querySelector('[data-page=settings]').click()");
    std::thread::sleep(Duration::from_secs(1));
    let address = serde_json::to_string(address).unwrap_or_default();
    let _ = window.eval(format!(
        "document.getElementById('set-map-source').value = {address}; \
         document.getElementById('connect-map-source').click();"
    ));
    std::thread::sleep(Duration::from_secs(3));
    let _ = window.eval(
        "[...document.querySelectorAll('#regions .button')] \
         .find((b) => b.textContent === 'Laden' || b.textContent === 'Aktualisieren')?.click()",
    );
    // Wait for the end of the download, then give the page a moment to show it.
    let started = std::time::Instant::now();
    while started.elapsed() < DOWNLOAD_TIME {
        std::thread::sleep(Duration::from_millis(500));
        if ended.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(2));
            return;
        }
    }
    eprintln!("smoke test: the region download did not end in time");
}

/// Called by the page at the end of the smoke test with its visible text.
#[tauri::command]
pub fn smoke_report(app: AppHandle, text: String) {
    if enabled() {
        println!("--- page content ---\n{text}");
        println!(
            "offline map tiles served: {}",
            crate::maps::TILES_SERVED.load(std::sync::atomic::Ordering::Relaxed)
        );
        println!("--- end ---");
        app.exit(0);
    }
}
