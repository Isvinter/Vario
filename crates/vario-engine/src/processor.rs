//! The engine's logic without any threading: feed it samples, ask it for the status.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use flight_core::sensor::{GpsFix, SensorEvent};
use flight_core::settings::VarioSettings;
use flight_core::time::Timestamp;
use flight_core::vario::VarioFilter;
use serde::Serialize;
use vario_audio::voice::ToneControl;

/// Number of summary lines kept for display.
const MAX_SUMMARIES: usize = 400;

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

pub struct Processor {
    control: ToneControl,
    filter: VarioFilter,
    test_climb_mps: Option<f64>,
    last_fix: Option<GpsFix>,
    accel: Tracker,
    pressure: Tracker,
    gps: Tracker,
    summaries: VecDeque<String>,
    logs: Option<Logs>,
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
            accel: Tracker::default(),
            pressure: Tracker::default(),
            gps: Tracker::default(),
            summaries: VecDeque::new(),
            logs: None,
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
            }
        }
        self.filter.handle(event);
        self.update_tone();

        let written = match &mut self.logs {
            Some(logs) => write_raw(&mut logs.raw, event),
            None => Ok(()),
        };
        self.note_log_result(written);
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
