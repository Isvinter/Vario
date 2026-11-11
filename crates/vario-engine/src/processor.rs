//! The engine's logic without any threading: feed it samples, ask it for the status.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use flight_core::flightlog;
use flight_core::igc::FlightMeta;
use flight_core::sensor::{GpsFix, SensorEvent};
use flight_core::settings::VarioSettings;
use flight_core::time::Timestamp;
use flight_core::track::TrackRecorder;
use flight_core::vario::VarioFilter;
use serde::Serialize;
use vario_audio::voice::ToneControl;

/// Number of summary lines kept for display.
const MAX_SUMMARIES: usize = 400;
/// Shortest time between two track points. Slightly below one second so that a GPS
/// delivering at 1 Hz with a little jitter does not lose every other point.
const TRACK_INTERVAL_S: f64 = 0.9;

/// State of the flight recording.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingInfo {
    /// Name of the IGC file; `None` until the first fix has been saved.
    pub file_name: Option<String>,
    /// Number of fixes recorded so far (one per second).
    pub fix_count: usize,
    /// Why the last attempt to save failed, if it did.
    pub error: Option<String>,
}

/// One point of the flown track, for drawing it on the map.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPoint {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
}

/// A snapshot of the engine state for the user interface.
///
/// `Serialize` lets the UI layer turn this into JSON; `rename_all` makes the field names
/// follow the JavaScript convention (`climbRateMps`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    /// Seconds since the engine was started.
    pub uptime_s: f64,
    pub altitude_m: Option<f64>,
    pub climb_rate_mps: Option<f64>,
    /// Estimated accelerometer bias, for diagnostics.
    pub accel_bias_mps2: Option<f64>,
    /// `true` while a test climb rate overrides the sensors for the tone.
    pub test_tone: bool,
    /// The flight recording, if recording is on.
    pub recording: Option<RecordingInfo>,
    pub fix: Option<FixInfo>,
    pub accel: SensorStats,
    pub pressure: SensorStats,
    pub gps: SensorStats,
    /// One line per summary interval, oldest first.
    pub summaries: Vec<String>,
    /// Path of the raw sensor log, if one is being written.
    pub log_file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixInfo {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
    pub fix_3d: bool,
    pub ground_speed_mps: Option<f64>,
    pub course_deg: Option<f64>,
    /// UTC time of the fix as `HH:MM:SS`.
    pub utc: String,
}

/// How regularly one sensor delivers.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorStats {
    /// Samples received since start.
    pub total: u64,
    /// Average rate during the last completed summary interval.
    pub rate_hz: f64,
    /// Longest pause between two samples during the last completed summary interval.
    pub max_gap_s: f64,
    /// Longest pause between two samples since start.
    pub longest_gap_s: f64,
    /// Seconds since the last sample arrived; `None` if none arrived yet.
    pub age_s: Option<f64>,
}

/// Bookkeeping behind [`SensorStats`].
#[derive(Debug, Default)]
struct Tracker {
    total: u64,
    last_time: Option<Timestamp>,
    last_arrival: Option<Instant>,
    longest_gap_s: f64,
    // Current, still running interval.
    interval_count: u32,
    interval_max_gap_s: f64,
    // Last completed interval.
    rate_hz: f64,
    max_gap_s: f64,
}

impl Tracker {
    fn record(&mut self, time: Timestamp) {
        if let Some(last) = self.last_time {
            let gap = time.seconds_since(last);
            self.interval_max_gap_s = self.interval_max_gap_s.max(gap);
            self.longest_gap_s = self.longest_gap_s.max(gap);
        }
        self.total += 1;
        self.interval_count += 1;
        self.last_time = Some(time);
        self.last_arrival = Some(Instant::now());
    }

    fn close_interval(&mut self, interval_s: f64) {
        self.rate_hz = if interval_s > 0.0 {
            f64::from(self.interval_count) / interval_s
        } else {
            0.0
        };
        self.max_gap_s = self.interval_max_gap_s;
        self.interval_count = 0;
        self.interval_max_gap_s = 0.0;
    }

    fn stats(&self) -> SensorStats {
        SensorStats {
            total: self.total,
            rate_hz: self.rate_hz,
            max_gap_s: self.max_gap_s,
            longest_gap_s: self.longest_gap_s,
            age_s: self.last_arrival.map(|at| at.elapsed().as_secs_f64()),
        }
    }
}

struct Logs {
    raw: BufWriter<File>,
    summary: BufWriter<File>,
    raw_path: String,
}

/// Records the flight and keeps its IGC file up to date.
struct Recording {
    recorder: TrackRecorder,
    dir: PathBuf,
    /// Chosen when the first fix is saved, because the name contains the start time.
    path: Option<PathBuf>,
    saved_fixes: usize,
    error: Option<String>,
}

impl Recording {
    /// Writes the flight to its file if fixes were added since the last save.
    ///
    /// The whole file is written to a temporary file which then replaces the old one.
    /// Whatever happens (crash, battery empty), the file on disk is always a complete,
    /// valid IGC file, at most one save interval behind.
    fn save(&mut self) -> io::Result<()> {
        let fix_count = self.recorder.fixes().len();
        let (Some(start), Some(file)) = (self.recorder.start(), self.recorder.to_igc()) else {
            return Ok(());
        };
        if fix_count == self.saved_fixes {
            return Ok(());
        }
        let path = match &self.path {
            Some(path) => path.clone(),
            None => {
                std::fs::create_dir_all(&self.dir)?;
                let path = unused_path(&self.dir, &flightlog::file_name(start));
                self.path = Some(path.clone());
                path
            }
        };
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, file.to_string())?;
        std::fs::rename(&temporary, &path)?;
        self.saved_fixes = fix_count;
        Ok(())
    }
}

/// `dir/name`, or with `-2`, `-3`, ... before the extension if that file exists, so an
/// existing flight is never overwritten.
fn unused_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name.strip_suffix(".igc").unwrap_or(name);
    (2..)
        .map(|number| dir.join(format!("{stem}-{number}.igc")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

pub struct Processor {
    control: ToneControl,
    filter: VarioFilter,
    test_climb_mps: Option<f64>,
    last_fix: Option<GpsFix>,
    /// Track points not yet handed out by [`Processor::take_track_points`].
    new_track_points: Vec<TrackPoint>,
    last_track_time: Option<Timestamp>,
    accel: Tracker,
    pressure: Tracker,
    gps: Tracker,
    summaries: VecDeque<String>,
    logs: Option<Logs>,
    recording: Option<Recording>,
}

impl Processor {
    pub fn new(control: ToneControl, settings: &VarioSettings) -> Self {
        control.set_settings(settings);
        let mut filter = VarioFilter::default();
        filter.set_damping(settings.damping_s);
        Processor {
            control,
            filter,
            test_climb_mps: None,
            last_fix: None,
            new_track_points: Vec::new(),
            last_track_time: None,
            accel: Tracker::default(),
            pressure: Tracker::default(),
            gps: Tracker::default(),
            summaries: VecDeque::new(),
            logs: None,
            recording: None,
        }
    }

    /// Starts writing `sensors-<name>.csv` (every sample) and `summary-<name>.txt` (one
    /// line per summary interval) into `dir`.
    pub fn open_logs(&mut self, dir: &Path, name: &str) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let raw_path = dir.join(format!("sensors-{name}.csv"));
        let mut raw = BufWriter::new(File::create(&raw_path)?);
        writeln!(
            raw,
            "# a,time_s,vertical_mps2 | p,time_s,pressure_pa | \
             g,time_s,utc,lat_deg,lon_deg,alt_m,speed_mps,course_deg,fix_3d"
        )?;
        let summary = BufWriter::new(File::create(dir.join(format!("summary-{name}.txt")))?);
        self.logs = Some(Logs {
            raw,
            summary,
            raw_path: raw_path.to_string_lossy().into_owned(),
        });
        Ok(())
    }

    /// Turns on flight recording: the track is saved as an IGC file in `dir`, named
    /// after the time of the first GPS fix. Nothing is written before the first fix.
    pub fn record_flight(&mut self, dir: &Path) {
        self.recording = Some(Recording {
            recorder: TrackRecorder::new(FlightMeta::default()),
            dir: dir.to_owned(),
            path: None,
            saved_fixes: 0,
            error: None,
        });
    }

    /// Brings the flight file up to date. Called regularly and at the end.
    pub fn save_flight(&mut self) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        let error = recording.save().err().map(|error| error.to_string());
        // Report a problem once, not every few seconds; and keep trying, because the
        // track is still in memory and the next attempt may succeed.
        if error.is_some() && error != recording.error {
            let line = format!(
                "saving the flight failed: {}",
                error.as_deref().unwrap_or("")
            );
            recording.error = error;
            self.push_summary(line);
        } else {
            recording.error = error;
        }
    }

    /// Applies changed settings: thresholds and volume to the tone, damping to the
    /// filter. Takes effect immediately, without restarting anything.
    pub fn set_settings(&mut self, settings: &VarioSettings) {
        self.control.set_settings(settings);
        self.filter.set_damping(settings.damping_s);
        self.update_tone();
    }

    /// See [`crate::Engine::set_test_climb`].
    pub fn set_test_climb(&mut self, climb_rate_mps: Option<f64>) {
        self.test_climb_mps = climb_rate_mps;
        self.update_tone();
    }

    pub fn handle(&mut self, event: &SensorEvent) {
        match event {
            SensorEvent::Accel(sample) => self.accel.record(sample.time),
            SensorEvent::Pressure(sample) => self.pressure.record(sample.time),
            SensorEvent::Gps(fix) => {
                self.gps.record(fix.time);
                self.last_fix = Some(*fix);
                self.add_track_point(fix);
            }
        }
        self.filter.handle(event);
        if let Some(recording) = &mut self.recording {
            recording.recorder.handle(event);
        }
        self.update_tone();

        let written = match &mut self.logs {
            Some(logs) => write_raw(&mut logs.raw, event),
            None => Ok(()),
        };
        self.note_log_result(written);
    }

    fn add_track_point(&mut self, fix: &GpsFix) {
        let due = self
            .last_track_time
            .is_none_or(|last| fix.time.seconds_since(last) >= TRACK_INTERVAL_S);
        if due {
            self.last_track_time = Some(fix.time);
            self.new_track_points.push(TrackPoint {
                latitude_deg: fix.latitude_deg,
                longitude_deg: fix.longitude_deg,
                altitude_m: fix.altitude_m,
            });
        }
    }

    /// Hands out the track points (at most one per second) recorded since the last
    /// call. `std::mem::take` moves the list out and leaves an empty one behind.
    pub fn take_track_points(&mut self) -> Vec<TrackPoint> {
        std::mem::take(&mut self.new_track_points)
    }

    /// Closes the current statistics interval of `interval_s` seconds and records a
    /// summary line starting with `label` (usually the time of day).
    pub fn summarize(&mut self, interval_s: f64, label: &str) {
        for tracker in [&mut self.accel, &mut self.pressure, &mut self.gps] {
            tracker.close_interval(interval_s);
        }
        let part = |name: &str, tracker: &Tracker| {
            format!(
                "{name} {:5.1} Hz gap {:5.2} s",
                tracker.rate_hz, tracker.max_gap_s
            )
        };
        let vario = match self.filter.reading() {
            Some(reading) => format!(
                "alt {:6.1} m climb {:+5.2} m/s",
                reading.altitude_m, reading.climb_rate_mps
            ),
            None => "no vario reading".to_owned(),
        };
        let line = format!(
            "{label}  {} | {} | {} | {vario}",
            part("accel", &self.accel),
            part("baro", &self.pressure),
            part("gps", &self.gps),
        );

        let written = match &mut self.logs {
            Some(logs) => writeln!(logs.summary, "{line}")
                .and_then(|()| logs.summary.flush())
                .and_then(|()| logs.raw.flush()),
            None => Ok(()),
        };
        self.push_summary(line);
        self.note_log_result(written);
        self.save_flight();
    }

    /// Turns the tone off.
    pub fn silence(&mut self) {
        self.control.set_climb_rate(0.0);
    }

    pub fn status(&self, uptime_s: f64, running: bool) -> Status {
        let reading = self.filter.reading();
        Status {
            running,
            uptime_s,
            altitude_m: reading.map(|r| r.altitude_m),
            climb_rate_mps: reading.map(|r| r.climb_rate_mps),
            accel_bias_mps2: self.filter.accel_bias_mps2(),
            test_tone: self.test_climb_mps.is_some(),
            recording: self.recording.as_ref().map(|recording| RecordingInfo {
                file_name: recording
                    .path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned()),
                fix_count: recording.recorder.fixes().len(),
                error: recording.error.clone(),
            }),
            fix: self.last_fix.map(|fix| FixInfo {
                latitude_deg: fix.latitude_deg,
                longitude_deg: fix.longitude_deg,
                altitude_m: fix.altitude_m,
                fix_3d: fix.fix_3d,
                ground_speed_mps: fix.ground_speed_mps,
                course_deg: fix.course_deg,
                utc: format!(
                    "{:02}:{:02}:{:02}",
                    fix.utc.time.hour(),
                    fix.utc.time.minute(),
                    fix.utc.time.second()
                ),
            }),
            accel: self.accel.stats(),
            pressure: self.pressure.stats(),
            gps: self.gps.stats(),
            summaries: self.summaries.iter().cloned().collect(),
            log_file: self.logs.as_ref().map(|logs| logs.raw_path.clone()),
        }
    }

    fn update_tone(&mut self) {
        let climb = self
            .test_climb_mps
            .or_else(|| self.filter.reading().map(|r| r.climb_rate_mps));
        if let Some(climb) = climb {
            self.control.set_climb_rate(climb);
        }
    }

    fn push_summary(&mut self, line: String) {
        if self.summaries.len() == MAX_SUMMARIES {
            self.summaries.pop_front();
        }
        self.summaries.push_back(line);
    }

    /// A failing log (disk full, ...) must never stop the vario: give up logging and
    /// leave a note instead.
    fn note_log_result(&mut self, result: io::Result<()>) {
        if let Err(error) = result {
            self.logs = None;
            self.push_summary(format!("logging stopped: {error}"));
        }
    }
}

fn write_raw(out: &mut impl Write, event: &SensorEvent) -> io::Result<()> {
    match event {
        SensorEvent::Accel(sample) => writeln!(
            out,
            "a,{:.4},{:.4}",
            sample.time.as_secs(),
            sample.vertical_mps2
        ),
        SensorEvent::Pressure(sample) => writeln!(
            out,
            "p,{:.4},{:.2}",
            sample.time.as_secs(),
            sample.pressure_pa
        ),
        SensorEvent::Gps(fix) => {
            let optional = |value: Option<f64>| value.map_or(String::new(), |v| format!("{v:.2}"));
            writeln!(
                out,
                "g,{:.4},{:02}{:02}{:02},{:.6},{:.6},{:.1},{},{},{}",
                fix.time.as_secs(),
                fix.utc.time.hour(),
                fix.utc.time.minute(),
                fix.utc.time.second(),
                fix.latitude_deg,
                fix.longitude_deg,
                fix.altitude_m,
                optional(fix.ground_speed_mps),
                optional(fix.course_deg),
                u8::from(fix.fix_3d),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flight_core::time::{Date, TimeOfDay, UtcDateTime};
    use sensor_sim::synthetic::SyntheticFlight;
    use sensor_sim::{SimConfig, Simulation};
    use vario_audio::voice::VarioVoice;

    fn climb_simulation(seconds: f64) -> Simulation<sensor_sim::sources::SimGps> {
        let start = UtcDateTime::new(
            Date::new(2026, 7, 28).unwrap(),
            TimeOfDay::new(10, 0, 0).unwrap(),
        );
        Simulation::synthetic(
            SyntheticFlight::steady_climb(2.0, seconds),
            start,
            &SimConfig::default(),
        )
    }

    fn processor() -> (Processor, ToneControl) {
        let settings = VarioSettings {
            damping_s: 0.0,
            ..VarioSettings::default()
        };
        let control = ToneControl::new(&settings);
        (Processor::new(control.clone(), &settings), control)
    }

    fn is_audible(control: &ToneControl) -> bool {
        let mut voice = VarioVoice::new(control.clone(), 48_000);
        let mut buffer = vec![0.0; 48_000];
        voice.render(&mut buffer);
        buffer.iter().any(|sample| sample.abs() > 0.1)
    }

    #[test]
    fn status_reflects_the_flight_and_the_sensor_rates() {
        let (mut processor, control) = processor();
        climb_simulation(20.0).run_fast(|event| processor.handle(&event));
        processor.summarize(20.0, "10:00:20");

        let status = processor.status(20.0, true);
        assert!((status.climb_rate_mps.unwrap() - 2.0).abs() < 0.05);
        assert!((status.altitude_m.unwrap() - 1540.0).abs() < 0.5);
        assert_eq!(status.accel.total, 1001);
        assert_eq!(status.pressure.total, 21);
        assert!((status.accel.rate_hz - 50.0).abs() < 0.1);
        assert!((status.accel.max_gap_s - 0.02).abs() < 1e-9);
        assert!((status.gps.max_gap_s - 1.0).abs() < 1e-9);
        assert!(status.gps.age_s.unwrap() < 5.0);

        let fix = status.fix.unwrap();
        assert_eq!(fix.utc, "10:00:20");
        assert_eq!(fix.ground_speed_mps, Some(10.0));

        assert_eq!(status.summaries.len(), 1);
        assert_eq!(
            status.summaries[0],
            "10:00:20  accel  50.0 Hz gap  0.02 s | baro   1.1 Hz gap  1.00 s | \
             gps   1.1 Hz gap  1.00 s | alt 1540.0 m climb +2.00 m/s"
        );
        assert!(is_audible(&control), "climbing at 2 m/s must beep");
    }

    #[test]
    fn track_has_one_point_per_second_and_is_handed_out_once() {
        let (mut processor, _) = processor();
        climb_simulation(20.0).run_fast(|event| processor.handle(&event));

        let points = processor.take_track_points();
        assert_eq!(points.len(), 21, "fixes at 0 s to 20 s");
        assert!((points[20].altitude_m - points[0].altitude_m - 40.0).abs() < 1.0);
        assert!(processor.take_track_points().is_empty());

        // Fixes arriving faster than 1 Hz are thinned out.
        let mut first_fix = None;
        climb_simulation(1.0).run_fast(|event| {
            if let SensorEvent::Gps(fix) = event {
                first_fix.get_or_insert(fix);
            }
        });
        let fix = first_fix.expect("the simulation delivers GPS fixes");
        for tenth in 0..25 {
            let time = Timestamp::from_secs(100.0 + f64::from(tenth) * 0.1);
            processor.handle(&SensorEvent::Gps(GpsFix { time, ..fix }));
        }
        assert_eq!(processor.take_track_points().len(), 3);
    }

    #[test]
    fn records_the_flight_as_an_igc_file_and_never_overwrites_one() {
        let dir = std::env::temp_dir().join(format!("vario-flight-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let (mut first, _) = processor();
        first.record_flight(&dir);
        first.save_flight();
        assert!(!dir.exists(), "nothing is written before the first fix");

        // Save half way through, as the engine does regularly, and again at the end.
        let mut half_way_saved = false;
        climb_simulation(20.0).run_fast(|event| {
            if event.time().as_secs() > 10.0 && !half_way_saved {
                first.save_flight();
                half_way_saved = true;
            }
            first.handle(&event);
        });
        first.save_flight();

        let info = first.status(20.0, true).recording.unwrap();
        assert_eq!(info.file_name.as_deref(), Some("2026-07-28-100000.igc"));
        assert_eq!(info.fix_count, 21);
        assert_eq!(info.error, None);

        let text = std::fs::read_to_string(dir.join("2026-07-28-100000.igc")).unwrap();
        let file = flight_core::igc::IgcFile::parse(&text).unwrap();
        let summary = flightlog::summarize(&file).unwrap();
        assert_eq!(summary.fix_count, 21);
        assert_eq!(summary.duration_s, 20);
        assert_eq!(summary.max_altitude_m - summary.start_altitude_m, 40);
        let first = file.fixes().next().unwrap();
        assert!(
            (first.pressure_altitude_m - 1500).abs() <= 1,
            "pressure altitude recorded"
        );

        // A second flight starting in the same second gets a name of its own.
        let (mut second, _) = processor();
        second.record_flight(&dir);
        climb_simulation(3.0).run_fast(|event| second.handle(&event));
        second.save_flight();
        assert!(dir.join("2026-07-28-100000-2.igc").exists());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            2,
            "no temporary files left"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gaps_in_the_data_are_reported() {
        let (mut processor, _) = processor();
        climb_simulation(8.0).run_fast(|event| {
            // Drop everything between 3 s and 6 s, as if the app had been suspended.
            let t = event.time().as_secs();
            if !(3.0..6.0).contains(&t) {
                processor.handle(&event);
            }
        });
        processor.summarize(8.0, "x");
        let status = processor.status(8.0, true);
        assert!((status.accel.longest_gap_s - 3.02).abs() < 1e-6);
        assert!((status.pressure.longest_gap_s - 4.0).abs() < 1e-6);
    }

    #[test]
    fn changed_settings_take_effect_while_running() {
        let (mut processor, control) = processor();
        climb_simulation(10.0).run_fast(|event| processor.handle(&event));
        assert!(is_audible(&control), "climbing at 2 m/s beeps by default");

        // Raise the climb threshold above the current climb rate: silence.
        let mut settings = VarioSettings {
            climb_threshold_mps: 3.0,
            damping_s: 0.0,
            ..VarioSettings::default()
        };
        processor.set_settings(&settings);
        assert!(!is_audible(&control));

        settings.climb_threshold_mps = 0.2;
        processor.set_settings(&settings);
        assert!(is_audible(&control));
    }

    #[test]
    fn test_climb_overrides_the_sensors_for_the_tone_only() {
        let (mut processor, control) = processor();
        // Level flight: silent.
        let start = UtcDateTime::new(Date::new(2026, 7, 28).unwrap(), TimeOfDay::MIDNIGHT);
        let mut level = Simulation::synthetic(
            SyntheticFlight::steady_climb(0.0, 5.0),
            start,
            &SimConfig::default(),
        );
        level.run_fast(|event| processor.handle(&event));
        assert!(!is_audible(&control));

        processor.set_test_climb(Some(3.0));
        assert!(is_audible(&control));
        let status = processor.status(5.0, true);
        assert!(status.test_tone);
        assert!(
            status.climb_rate_mps.unwrap().abs() < 0.05,
            "display unaffected"
        );

        processor.set_test_climb(None);
        assert!(!is_audible(&control));
    }

    #[test]
    fn writes_raw_and_summary_logs() {
        let dir = std::env::temp_dir().join(format!("vario-engine-test-{}", std::process::id()));
        let (mut processor, _) = processor();
        processor.open_logs(&dir, "test").unwrap();
        climb_simulation(2.0).run_fast(|event| processor.handle(&event));
        processor.summarize(2.0, "10:00:02");

        let status = processor.status(2.0, true);
        let raw_path = status.log_file.unwrap();
        let raw = std::fs::read_to_string(&raw_path).unwrap();
        let summary = std::fs::read_to_string(dir.join("summary-test.txt")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let lines: Vec<&str> = raw.lines().collect();
        assert!(lines[0].starts_with('#'));
        assert_eq!(lines[1], "a,0.0000,0.0000");
        assert_eq!(lines[2], "p,0.0000,84555.99");
        assert_eq!(
            lines[3],
            "g,0.0000,100000,47.000000,11.000000,1500.0,10.00,0.00,1"
        );
        assert_eq!(lines.iter().filter(|l| l.starts_with("a,")).count(), 101);
        assert_eq!(lines.iter().filter(|l| l.starts_with("p,")).count(), 3);
        assert_eq!(lines.iter().filter(|l| l.starts_with("g,")).count(), 3);
        assert!(summary.starts_with("10:00:02  accel"));
    }
}
