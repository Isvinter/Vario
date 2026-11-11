//! The app: glue between the user interface and the crates that do the work.
//!
//! This first version is the test app for iOS background operation: it plays the vario
//! tone from the sensors, shows how regularly each sensor delivers, and logs everything.
//! On the desktop the sensors are replaced by a simulated flight.

mod audio;
mod settings;
#[cfg(desktop)]
mod sim;
mod smoke;

use std::path::PathBuf;
use std::sync::Mutex;

use flight_core::settings::VarioSettings;
use serde::Serialize;

use crate::settings::SettingsDto;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_vario_sensors::{SensorStart, VarioSensorsExt};
use vario_audio::voice::ToneControl;
use vario_engine::flights::{self, FlightEntry};
use vario_engine::{Engine, Status, TrackPoint};

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
    /// The current settings; loaded from the settings file when the app starts.
    settings: Mutex<VarioSettings>,
}

impl AppState {
    fn session(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn settings(&self) -> std::sync::MutexGuard<'_, VarioSettings> {
        self.settings
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

    // `*` copies the settings out of the lock, which is released right away.
    let settings = *state.settings();
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
    let engine = Engine::start(
        events,
        control,
        &settings,
        log_dir.clone(),
        flights_dir(&app),
    )
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

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> SettingsDto {
    (*state.settings()).into()
}

#[tauri::command]
fn default_settings() -> SettingsDto {
    VarioSettings::default().into()
}

#[tauri::command]
fn get_map_style(app: AppHandle) -> String {
    settings::load_map_style(&app)
}

/// Saves the map style URL and returns it as stored (empty input gives the default).
#[tauri::command]
fn set_map_style(app: AppHandle, url: String) -> Result<String, String> {
    settings::save_map_style(&app, &url)
}

/// Checks, applies and saves new settings. A running vario uses them immediately.
#[tauri::command]
fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: SettingsDto,
) -> Result<(), String> {
    let settings = VarioSettings::from(settings);
    settings.validate().map_err(|error| error.to_string())?;
    *state.settings() = settings;
    if let Some(session) = state.session().as_ref() {
        session.engine.set_settings(settings);
    }
    settings::save(&app, &settings)
}

/// The current engine status, or `null` while stopped.
#[tauri::command]
fn status(state: State<'_, AppState>) -> Option<Status> {
    state
        .session()
        .as_ref()
        .map(|session| session.engine.status())
}

/// The track points from index `from` on; empty while stopped.
#[tauri::command]
fn track(state: State<'_, AppState>, from: usize) -> Vec<TrackPoint> {
    state
        .session()
        .as_ref()
        .map(|session| session.engine.track_since(from))
        .unwrap_or_default()
}

/// All recorded flights, newest first.
#[tauri::command]
async fn list_flights(app: AppHandle) -> Result<Vec<FlightEntry>, String> {
    let dir = flights_dir(&app).ok_or("no flights folder")?;
    flights::list(&dir).map_err(|error| error.to_string())
}

/// The track of one recorded flight.
#[tauri::command]
async fn flight_track(app: AppHandle, file_name: String) -> Result<Vec<TrackPoint>, String> {
    let dir = flights_dir(&app).ok_or("no flights folder")?;
    flights::track(&dir, &file_name).map_err(|error| error.to_string())
}

#[tauri::command]
async fn delete_flight(app: AppHandle, file_name: String) -> Result<(), String> {
    let dir = flights_dir(&app).ok_or("no flights folder")?;
    flights::delete(&dir, &file_name).map_err(|error| error.to_string())
}

/// Plays the tone for the given climb rate regardless of the sensors; `null` ends it.
#[tauri::command]
fn test_tone(state: State<'_, AppState>, climb_rate_mps: Option<f64>) {
    if let Some(session) = state.session().as_ref() {
        session.engine.set_test_climb(climb_rate_mps);
    }
}

/// Where the files the user may want to get at are kept. On iOS this is the app's
/// Documents folder, which the Files app shows (see `Info.ios.plist`).
fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    let base = if cfg!(target_os = "ios") {
        app.path().document_dir()
    } else {
        app.path().app_data_dir()
    };
    base.ok()
}

/// Where the sensor logs go.
fn log_dir(app: &AppHandle) -> Option<PathBuf> {
    data_dir(app).map(|dir| dir.join("logs"))
}

/// Where the recorded flights (IGC files) go.
fn flights_dir(app: &AppHandle) -> Option<PathBuf> {
    if smoke::enabled() {
        // Keep the test's flights out of the real flight list.
        return Some(std::env::temp_dir().join("vario-smoke-test-flights"));
    }
    data_dir(app).map(|dir| dir.join("flights"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_vario_sensors::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            start,
            stop,
            status,
            track,
            list_flights,
            flight_track,
            delete_flight,
            test_tone,
            get_settings,
            set_settings,
            default_settings,
            get_map_style,
            set_map_style,
            smoke::smoke_report
        ])
        .setup(|app| {
            *app.state::<AppState>().settings() = settings::load(app.handle());
            smoke::maybe_run(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
