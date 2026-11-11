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
const MAP_STYLE_FILE_NAME: &str = "map-style.txt";

/// Default map style. OpenFreeMap allows this kind of use without an API key; the
/// attribution it requires is shown by the map itself.
pub const DEFAULT_MAP_STYLE: &str = "https://tiles.openfreemap.org/styles/liberty";

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

fn config_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(name))
        .map_err(|error| error.to_string())
}

fn file_path(app: &AppHandle) -> Result<PathBuf, String> {
    config_file(app, FILE_NAME)
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
    write_file(file_path(app)?, &settings.to_text())
}

/// The URL of the map style: the saved one, or the default.
pub fn load_map_style(app: &AppHandle) -> String {
    config_file(app, MAP_STYLE_FILE_NAME)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| checked_map_style(&text).ok())
        .unwrap_or_else(|| DEFAULT_MAP_STYLE.to_owned())
}

/// Saves the URL of the map style and returns it as stored. An empty URL selects the
/// default.
pub fn save_map_style(app: &AppHandle, url: &str) -> Result<String, String> {
    let url = checked_map_style(url)?;
    write_file(config_file(app, MAP_STYLE_FILE_NAME)?, &url)?;
    Ok(url)
}

fn checked_map_style(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        Ok(DEFAULT_MAP_STYLE.to_owned())
    } else if url.contains("tile.openstreetmap.org") {
        // The OSM tile servers are not meant for apps like this one.
        Err("tile.openstreetmap.org must not be used as a tile source".to_owned())
    } else if (url.starts_with("https://") || url.starts_with("http://"))
        && !url.contains(char::is_whitespace)
    {
        Ok(url.to_owned())
    } else {
        Err("the map style must be an http(s) address".to_owned())
    }
}

fn write_file(path: PathBuf, text: &str) -> Result<(), String> {
    let describe = |error: std::io::Error| format!("could not save settings: {error}");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(describe)?;
    }
    // Write to a temporary file and rename it, so an interrupted write can never
    // leave a half-written settings file behind.
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, text).map_err(describe)?;
    std::fs::rename(&temporary, &path).map_err(describe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_style_must_be_a_web_address_and_not_the_osm_tile_server() {
        assert_eq!(checked_map_style("  "), Ok(DEFAULT_MAP_STYLE.to_owned()));
        assert_eq!(
            checked_map_style(" https://example.org/style.json\n"),
            Ok("https://example.org/style.json".to_owned())
        );
        assert!(checked_map_style("file:///etc/passwd").is_err());
        assert!(checked_map_style("https://exa mple.org").is_err());
        assert!(checked_map_style("https://tile.openstreetmap.org/{z}/{x}/{y}.png").is_err());
    }
}
