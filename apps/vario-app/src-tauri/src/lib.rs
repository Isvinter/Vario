//! The app: glue between the user interface and the crates that do the work.
//!
//! This first version is the test app for iOS background operation: it plays the vario
//! tone from the sensors, shows how regularly each sensor delivers, and logs everything.
//! On the desktop the sensors are replaced by a simulated flight.

mod audio;
#[cfg(desktop)]
mod sim;

use std::path::PathBuf;
use std::sync::Mutex;

use flight_core::settings::VarioSettings;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_vario_sensors::{SensorStart, VarioSensorsExt};
use vario_audio::voice::ToneControl;
use vario_engine::{Engine, Status};

use crate::audio::AudioThread;

/// Everything that exists only while the vario is running. Dropping it stops it all.
struct Session {
    engine: Engine,
    // The two fields below are never read; they are kept because dropping them stops
    // the audio output and the simulator.
    _audio: Option<AudioThread>,
    #[cfg(desktop)]
    _simulator: sim::Simulator,
}

/// State managed by Tauri and handed to the commands below.
#[derive(Default)]
struct AppState {
    session: Mutex<Option<Session>>,
}

impl AppState {
    fn session(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What `start` reports back to the user interface.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartInfo {
    sensors: SensorStart,
    /// Description of the audio output, or the reason why there is none.
    audio: String,
    log_dir: Option<String>,
}

// A function marked `#[tauri::command]` can be called from JavaScript with
// `invoke("start")`. Commands declared `async` run on a background thread; that is
// required here because the plugin call waits for work done on the main thread.
#[tauri::command]
async fn start(app: AppHandle, state: State<'_, AppState>) -> Result<StartInfo, String> {
    if state.session().is_some() {
        return Err("already running".to_owned());
    }

    let settings = VarioSettings::default();
    let control = ToneControl::new(&settings);
    // Open the channel first so no sample of the sensors started next is lost.
    let events = sensor_bridge::connect();

    // On iOS this also activates the audio session, which must happen before the
    // audio output is opened.
    let sensors = app
        .vario_sensors()
        .start()
        .inspect_err(|_| sensor_bridge::disconnect())?;

    let (audio, audio_info) = match AudioThread::start(control.clone()) {
        Ok((thread, info)) => (Some(thread), info),
        Err(error) => (None, format!("no audio: {error}")),
    };

    let log_dir = log_dir(&app);
    let engine = Engine::start(events, control, &settings, log_dir.clone())
        .map_err(|error| format!("could not start: {error}"))
        .inspect_err(|_| sensor_bridge::disconnect())?;

    *state.session() = Some(Session {
        engine,
        _audio: audio,
        #[cfg(desktop)]
        _simulator: sim::Simulator::start(),
    });

    Ok(StartInfo {
        sensors,
        audio: audio_info,
        log_dir: log_dir.map(|dir| dir.to_string_lossy().into_owned()),
    })
}

#[tauri::command]
async fn stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // `take` moves the session out of the state and leaves `None` behind.
    let session = state.session().take();
    let result = app.vario_sensors().stop();
    sensor_bridge::disconnect();
    drop(session);
    result
}

/// The current engine status, or `null` while stopped.
#[tauri::command]
fn status(state: State<'_, AppState>) -> Option<Status> {
    state
        .session()
        .as_ref()
        .map(|session| session.engine.status())
}

/// Plays the tone for the given climb rate regardless of the sensors; `null` ends it.
#[tauri::command]
fn test_tone(state: State<'_, AppState>, climb_rate_mps: Option<f64>) {
    if let Some(session) = state.session().as_ref() {
        session.engine.set_test_climb(climb_rate_mps);
    }
}

/// Where the sensor logs go. On iOS this is the app's Documents folder, which the
/// Files app shows (see `Info.ios.plist`).
fn log_dir(app: &AppHandle) -> Option<PathBuf> {
    let base = if cfg!(target_os = "ios") {
        app.path().document_dir()
    } else {
        app.path().app_data_dir()
    };
    base.ok().map(|dir| dir.join("logs"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_vario_sensors::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![start, stop, status, test_tone])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
