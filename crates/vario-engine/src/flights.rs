//! The folder of recorded flights: listing, loading and deleting IGC files.

use std::io;
use std::path::{Path, PathBuf};

use flight_core::flightlog::{self, FlightSummary};
use flight_core::igc::IgcFile;
use serde::Serialize;

use crate::processor::TrackPoint;

/// One file in the flights folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlightEntry {
    pub file_name: String,
    /// `None` if the file could not be read as a flight.
    pub summary: Option<SummaryInfo>,
}

/// [`FlightSummary`] in the form the user interface receives.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryInfo {
    /// Start in UTC as `2026-07-28T10:00:00Z`, or `None` if the file has no date.
    /// JavaScript turns this format directly into local time.
    pub start_utc: Option<String>,
    pub duration_s: u32,
    pub fix_count: usize,
    pub start_altitude_m: i32,
    pub max_altitude_m: i32,
    pub track_length_m: f64,
    pub distance_m: f64,
}

impl From<FlightSummary> for SummaryInfo {
    fn from(summary: FlightSummary) -> Self {
        SummaryInfo {
            start_utc: summary.date.map(|date| {
                format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                    date.year(),
                    date.month(),
                    date.day(),
                    summary.start.hour(),
                    summary.start.minute(),
                    summary.start.second()
                )
            }),
            duration_s: summary.duration_s,
            fix_count: summary.fix_count,
            start_altitude_m: summary.start_altitude_m,
            max_altitude_m: summary.max_altitude_m,
            track_length_m: summary.track_length_m,
            distance_m: summary.distance_m,
        }
    }
}

/// All flights in `dir`, newest first. A missing folder is an empty list.
pub fn list(dir: &Path) -> io::Result<Vec<FlightEntry>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut flights = Vec::new();
    for entry in entries {
        let file_name = entry?.file_name().to_string_lossy().into_owned();
        if !is_flight_file_name(&file_name) {
            continue;
        }
        let summary = read(dir, &file_name)
            .ok()
            .and_then(|file| flightlog::summarize(&file))
            .map(SummaryInfo::from);
        flights.push(FlightEntry { file_name, summary });
    }
    // File names start with date and time, so sorting by name sorts by time.
    flights.sort_by(|a, b| b.file_name.cmp(&a.file_name));
    Ok(flights)
}

/// The track of one flight, for drawing it on the map.
pub fn track(dir: &Path, file_name: &str) -> io::Result<Vec<TrackPoint>> {
    let file = read(dir, file_name)?;
    Ok(file
        .fixes()
        .map(|fix| TrackPoint {
            latitude_deg: fix.latitude_deg,
            longitude_deg: fix.longitude_deg,
            altitude_m: f64::from(flightlog::display_altitude_m(fix)),
        })
        .collect())
}

/// Deletes one flight for good.
pub fn delete(dir: &Path, file_name: &str) -> io::Result<()> {
    std::fs::remove_file(checked_path(dir, file_name)?)
}

fn read(dir: &Path, file_name: &str) -> io::Result<IgcFile> {
    let bytes = std::fs::read(checked_path(dir, file_name)?)?;
    IgcFile::parse_bytes(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// The file names come from the user interface. Accept only plain names of IGC files,
/// so that a name like `../../something` can never reach a file outside the folder.
fn is_flight_file_name(name: &str) -> bool {
    name.ends_with(".igc")
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.contains("..")
}

fn checked_path(dir: &Path, file_name: &str) -> io::Result<PathBuf> {
    if is_flight_file_name(file_name) {
        Ok(dir.join(file_name))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a flight file name",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flight_core::igc::{FlightMeta, IgcFix, Record};
    use flight_core::time::{Date, TimeOfDay};

    fn write_flight(dir: &Path, name: &str, day: u8, fixes: u8) {
        let mut file =
            IgcFile::with_header(Date::new(2026, 7, day).unwrap(), &FlightMeta::default());
        for second in 0..fixes {
            file.records.push(Record::Fix(IgcFix {
                time: TimeOfDay::new(10, 0, second).unwrap(),
                latitude_deg: 47.0 + f64::from(second) * 0.001,
                longitude_deg: 11.0,
                fix_3d: true,
                pressure_altitude_m: 1000,
                gps_altitude_m: 1000 + i32::from(second),
                extension: String::new(),
            }));
        }
        std::fs::write(dir.join(name), file.to_string()).unwrap();
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vario-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_flights_newest_first_and_tolerates_other_files() {
        let dir = test_dir("list");
        write_flight(&dir, "2026-07-27-100000.igc", 27, 5);
        write_flight(&dir, "2026-07-28-100000.igc", 28, 10);
        std::fs::write(dir.join("broken.igc"), "B123\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a flight").unwrap();
        std::fs::write(dir.join("2026-07-28-100000.tmp"), "half written").unwrap();

        let flights = list(&dir).unwrap();
        let names: Vec<&str> = flights.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(
            names,
            [
                "broken.igc",
                "2026-07-28-100000.igc",
                "2026-07-27-100000.igc"
            ]
        );
        assert_eq!(flights[0].summary, None);
        let newest = flights[1].summary.as_ref().unwrap();
        assert_eq!(newest.start_utc.as_deref(), Some("2026-07-28T10:00:00Z"));
        assert_eq!(newest.duration_s, 9);
        assert_eq!(newest.max_altitude_m, 1009);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_folder_is_an_empty_list() {
        let dir = std::env::temp_dir().join("vario-does-not-exist");
        assert_eq!(list(&dir).unwrap(), Vec::new());
    }

    #[test]
    fn loads_and_deletes_one_flight() {
        let dir = test_dir("load");
        write_flight(&dir, "2026-07-28-100000.igc", 28, 4);

        let points = track(&dir, "2026-07-28-100000.igc").unwrap();
        assert_eq!(points.len(), 4);
        assert!((points[3].latitude_deg - 47.003).abs() < 1e-5);
        assert!((points[3].altitude_m - 1003.0).abs() < 1e-9);

        delete(&dir, "2026-07-28-100000.igc").unwrap();
        assert_eq!(list(&dir).unwrap(), Vec::new());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_names_that_point_outside_the_folder() {
        let dir = test_dir("names");
        for name in [
            "../secret.igc",
            "a/b.igc",
            "..\\x.igc",
            ".hidden.igc",
            "flight.txt",
            "",
        ] {
            assert!(track(&dir, name).is_err(), "{name:?} must be rejected");
            assert!(delete(&dir, name).is_err(), "{name:?} must be rejected");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
