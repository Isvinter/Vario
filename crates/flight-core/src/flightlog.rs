//! Facts about one recorded flight, computed from its IGC file: what a flight list
//! shows, and later the basis for a logbook with statistics.

use crate::geo::distance_m;
use crate::igc::{IgcFile, IgcFix};
use crate::time::{Date, SECONDS_PER_DAY, TimeOfDay, UtcDateTime};

/// Summary of one flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightSummary {
    /// Date from the file header, if it has one.
    pub date: Option<Date>,
    /// UTC time of the first fix.
    pub start: TimeOfDay,
    /// Time from the first to the last fix in seconds.
    pub duration_s: u32,
    pub fix_count: usize,
    /// Altitude of the first fix in metres.
    pub start_altitude_m: i32,
    /// Highest altitude in metres.
    pub max_altitude_m: i32,
    /// Length of the flown path over ground in metres.
    pub track_length_m: f64,
    /// Straight distance from the first to the last fix in metres.
    pub distance_m: f64,
}

/// The altitude to show for a fix: GPS altitude where the fix has one, otherwise
/// pressure altitude. (Pressure altitude refers to the standard atmosphere and can be
/// off by a hundred metres or more on a given day.)
pub fn display_altitude_m(fix: &IgcFix) -> i32 {
    if fix.fix_3d {
        fix.gps_altitude_m
    } else {
        fix.pressure_altitude_m
    }
}

/// Summarizes a flight. `None` if the file contains no fixes.
pub fn summarize(file: &IgcFile) -> Option<FlightSummary> {
    let mut fixes = file.fixes();
    let first = fixes.next()?;

    let mut summary = FlightSummary {
        date: file.date(),
        start: first.time,
        duration_s: 0,
        fix_count: 1,
        start_altitude_m: display_altitude_m(first),
        max_altitude_m: display_altitude_m(first),
        track_length_m: 0.0,
        distance_m: 0.0,
    };

    let mut previous = first;
    for fix in fixes {
        // IGC stores only the time of day. A time earlier than the previous one means
        // the flight went past midnight UTC.
        let step = (fix.time.seconds_since_midnight() + SECONDS_PER_DAY
            - previous.time.seconds_since_midnight())
            % SECONDS_PER_DAY;
        summary.duration_s += step;
        summary.fix_count += 1;
        summary.max_altitude_m = summary.max_altitude_m.max(display_altitude_m(fix));
        summary.track_length_m += distance_m(
            previous.latitude_deg,
            previous.longitude_deg,
            fix.latitude_deg,
            fix.longitude_deg,
        );
        previous = fix;
    }
    summary.distance_m = distance_m(
        first.latitude_deg,
        first.longitude_deg,
        previous.latitude_deg,
        previous.longitude_deg,
    );
    Some(summary)
}

/// File name for a flight that started at `start`, e.g. `2026-07-28-101502.igc`.
/// Names sort by date and time.
pub fn file_name(start: UtcDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}{:02}.igc",
        start.date.year(),
        start.date.month(),
        start.date.day(),
        start.time.hour(),
        start.time.minute(),
        start.time.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::igc::{FlightMeta, Record};

    fn fix(hour: u8, minute: u8, second: u8, latitude_deg: f64, gps_altitude_m: i32) -> IgcFix {
        IgcFix {
            time: TimeOfDay::new(hour, minute, second).unwrap(),
            latitude_deg,
            longitude_deg: 11.0,
            fix_3d: true,
            pressure_altitude_m: gps_altitude_m - 50,
            gps_altitude_m,
            extension: String::new(),
        }
    }

    fn file(fixes: Vec<IgcFix>) -> IgcFile {
        let mut file =
            IgcFile::with_header(Date::new(2026, 7, 28).unwrap(), &FlightMeta::default());
        file.records.extend(fixes.into_iter().map(Record::Fix));
        file
    }

    #[test]
    fn summarizes_times_altitudes_and_distances() {
        // One minute of arc of latitude is one nautical mile (1852 m).
        let north = 47.0 + 1.0 / 60.0;
        let summary = summarize(&file(vec![
            fix(10, 0, 0, 47.0, 1500),
            fix(10, 5, 0, north, 1900),
            fix(10, 12, 30, 47.0, 800),
        ]))
        .unwrap();

        assert_eq!(summary.date, Date::new(2026, 7, 28));
        assert_eq!(summary.start, TimeOfDay::new(10, 0, 0).unwrap());
        assert_eq!(summary.duration_s, 750);
        assert_eq!(summary.fix_count, 3);
        assert_eq!(summary.start_altitude_m, 1500);
        assert_eq!(summary.max_altitude_m, 1900);
        assert!((summary.track_length_m - 2.0 * 1852.0).abs() < 10.0);
        assert!(summary.distance_m < 1.0, "landed where it started");
    }

    #[test]
    fn duration_continues_past_midnight() {
        let summary = summarize(&file(vec![
            fix(23, 59, 50, 47.0, 1000),
            fix(0, 0, 20, 47.0, 1000),
        ]))
        .unwrap();
        assert_eq!(summary.duration_s, 30);
    }

    #[test]
    fn uses_pressure_altitude_for_fixes_without_gps_altitude() {
        let mut low_quality = fix(10, 0, 0, 47.0, 0);
        low_quality.fix_3d = false;
        low_quality.pressure_altitude_m = 1234;
        let summary = summarize(&file(vec![low_quality])).unwrap();
        assert_eq!(summary.max_altitude_m, 1234);
        assert_eq!(summary.duration_s, 0);
    }

    #[test]
    fn a_file_without_fixes_has_no_summary() {
        assert_eq!(summarize(&file(Vec::new())), None);
    }

    #[test]
    fn file_names_sort_by_start_time() {
        let start = UtcDateTime::new(
            Date::new(2026, 7, 28).unwrap(),
            TimeOfDay::new(9, 5, 2).unwrap(),
        );
        assert_eq!(file_name(start), "2026-07-28-090502.igc");
    }
}
