//! Notices a recording that ended without "Stop".
//!
//! Starting the vario writes a small marker file with the start time; "Stop" and an
//! orderly exit delete it. If the marker is still there when the app starts, the last
//! session ended unexpectedly (the app was killed for lack of memory, crashed, or the
//! battery ran out), and the user is told which flight was affected and up to when it
//! was saved.
//!
//! The marker lives in the app's private configuration folder, not in the documents
//! folder that the iOS "Files" app shows.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use flight_core::time::UtcDateTime;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use vario_engine::flights::FlightEntry;

const FILE_NAME: &str = "recording-session.txt";

/// What the user interface shows about a session that ended unexpectedly.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptedSession {
    /// Start of the session in UTC, `2026-07-28T10:00:00Z`.
    pub started_utc: String,
    /// The flight recorded in that session, if it got any GPS fix.
    pub flight: Option<FlightEntry>,
}

/// `None` during the smoke test, so it neither leaves a marker nor removes the user's.
fn path(app: &AppHandle) -> Option<PathBuf> {
    if crate::smoke::enabled() {
        return None;
    }
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join(FILE_NAME))
}

/// Notes that a session starts now.
pub fn create(app: &AppHandle) {
    let Some(path) = path(app) else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    // Not being able to write the marker only costs the notice after a crash.
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, format!("{now}\n")));
    if let Err(error) = written {
        eprintln!("could not write the session marker: {error}");
    }
}

/// Notes that the session ended in an orderly way.
pub fn remove(app: &AppHandle) {
    if let Some(path) = path(app) {
        let _ = std::fs::remove_file(path);
    }
}

/// When the file at `path` was last written, as Unix time.
pub fn modified_unix(path: &std::path::Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let seconds = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(seconds).ok()
}

/// If the last session ended unexpectedly: its start as Unix time. Removes the marker,
/// so the notice appears only once.
pub fn take_interrupted(app: &AppHandle) -> Option<i64> {
    let path = path(app)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    text.trim().parse().ok()
}

/// Describes the interrupted session that started at `started_unix`, given the
/// recorded flights with the time (Unix) their file was last written.
///
/// The session's flight is the one written since the start of the session. (The file
/// name is no reliable guide: it holds the time of the first fix.)
pub fn describe(started_unix: i64, flights: &[(FlightEntry, i64)]) -> InterruptedSession {
    let start = UtcDateTime::from_unix_seconds(started_unix);
    let flight = flights
        .iter()
        .filter(|(_, modified)| *modified >= started_unix)
        .max_by_key(|(_, modified)| *modified)
        .map(|(flight, _)| flight.clone());
    InterruptedSession {
        started_utc: format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            start.date.year(),
            start.date.month(),
            start.date.day(),
            start.time.hour(),
            start.time.minute(),
            start.time.second()
        ),
        flight,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(file_name: &str) -> FlightEntry {
        FlightEntry {
            file_name: file_name.to_owned(),
            summary: None,
        }
    }

    // 2026-11-17 23:09:15 UTC
    const STARTED: i64 = 1_794_956_955;

    #[test]
    fn finds_the_flight_written_since_the_start() {
        let flights = [
            // First fix (cached by iOS) before the start, written during the session.
            (entry("2026-11-17-230901.igc"), STARTED + 50),
            (entry("2026-11-17-224722.igc"), STARTED - 400),
        ];
        let session = describe(STARTED, &flights);
        assert_eq!(session.started_utc, "2026-11-17T23:09:15Z");
        assert_eq!(
            session.flight.map(|flight| flight.file_name).as_deref(),
            Some("2026-11-17-230901.igc")
        );
    }

    #[test]
    fn a_session_without_a_fix_has_no_flight() {
        let flights = [(entry("2026-11-17-224722.igc"), STARTED - 400)];
        assert_eq!(describe(STARTED, &flights).flight, None);
    }
}
