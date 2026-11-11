//! Loading and saving the settings file, and the form in which settings travel
//! between Rust and the user interface.
//!
//! What the settings mean, which values are allowed and the file format are defined in
//! `flight-core`; this module only adds the file location and the JSON form.

use std::path::PathBuf;

use flight_core::settings::VarioSettings;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const FILE_NAME: &str = "settings.txt";

/// The settings as JavaScript sees them (`climbThresholdMps` and so on).
///
/// A separate type, because `flight-core` deliberately has no JSON dependency. The
/// two `From` implementations below convert in both directions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    climb_threshold_mps: f64,
    sink_threshold_mps: f64,
    volume: f64,
    damping_s: f64,
}

impl From<VarioSettings> for SettingsDto {
    fn from(settings: VarioSettings) -> Self {
        SettingsDto {
            climb_threshold_mps: settings.climb_threshold_mps,
            sink_threshold_mps: settings.sink_threshold_mps,
            volume: settings.volume,
            damping_s: settings.damping_s,
        }
    }
}

impl From<SettingsDto> for VarioSettings {
    fn from(dto: SettingsDto) -> Self {
        VarioSettings {
            climb_threshold_mps: dto.climb_threshold_mps,
            sink_threshold_mps: dto.sink_threshold_mps,
            volume: dto.volume,
            damping_s: dto.damping_s,
        }
    }
}

fn file_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(FILE_NAME))
        .map_err(|error| error.to_string())
}

/// Reads the settings file. A missing or unreadable file gives the defaults: the app
/// must always start.
pub fn load(app: &AppHandle) -> VarioSettings {
    let Ok(path) = file_path(app) else {
        return VarioSettings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => VarioSettings::from_text(&text).unwrap_or_else(|error| {
            eprintln!("ignoring {}: {error}", path.display());
            VarioSettings::default()
        }),
        Err(_) => VarioSettings::default(),
    }
}

pub fn save(app: &AppHandle, settings: &VarioSettings) -> Result<(), String> {
    let path = file_path(app)?;
    let describe = |error: std::io::Error| format!("could not save settings: {error}");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(describe)?;
    }
    // Write to a temporary file and rename it, so an interrupted write can never
    // leave a half-written settings file behind.
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, settings.to_text()).map_err(describe)?;
    std::fs::rename(&temporary, &path).map_err(describe)
}
