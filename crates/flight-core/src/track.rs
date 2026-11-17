//! Recording a flight track from sensor samples.

use std::collections::VecDeque;

use crate::atmosphere::pressure_to_altitude;
use crate::igc::{FlightMeta, IgcFile, IgcFix, Record};
use crate::sensor::{GpsFix, PressureSample, SensorEvent};
use crate::time::{Timestamp, UtcDateTime};

/// Two pressure samples further apart than this are not interpolated between.
const MAX_PRESSURE_GAP_S: f64 = 5.0;

/// A pressure sample further than this from a fix (in either direction) is not used for
/// it when there is no pair of samples around the fix to interpolate between. Large
/// enough that the last fixes of a recording still get the newest sample although the
/// samples after them have not arrived yet.
const MAX_PRESSURE_DISTANCE_S: f64 = 3.0;

/// How long a fix waits for the pressure samples around its time. Barometer samples can
/// arrive seconds after the time they were measured (on the iPhone about 2.7 s), so the
/// pressure altitude of a fix is often filled in after the fix was recorded.
const PRESSURE_WAIT_S: f64 = 10.0;

/// Collects GPS fixes at no more than one per second, together with the pressure altitude
/// at the time of each fix, and turns them into an IGC file.
///
/// The pressure altitude is interpolated between the pressure samples measured just
/// before and just after the fix. Samples are matched by their measurement time, not by
/// the order in which they arrive.
#[derive(Debug, Clone)]
pub struct TrackRecorder {
    meta: FlightMeta,
    /// Recent pressure samples, oldest first, spanning at most [`PRESSURE_WAIT_S`].
    pressures: VecDeque<PressureSample>,
    /// Fixes still waiting for their pressure altitude: index into `fixes` and the
    /// monotonic time of the fix. Oldest first.
    pending: VecDeque<(usize, Timestamp)>,
    start: Option<UtcDateTime>,
    last_recorded: Option<UtcDateTime>,
    fixes: Vec<IgcFix>,
    revision: u64,
}

impl TrackRecorder {
    pub fn new(meta: FlightMeta) -> Self {
        TrackRecorder {
            meta,
            pressures: VecDeque::new(),
            pending: VecDeque::new(),
            start: None,
            last_recorded: None,
            fixes: Vec::new(),
            revision: 0,
        }
    }

    /// Feeds any sensor sample. Acceleration samples are ignored.
    pub fn handle(&mut self, event: &SensorEvent) {
        match event {
            SensorEvent::Pressure(sample) => self.on_pressure(*sample),
            SensorEvent::Gps(fix) => self.on_fix(fix),
            SensorEvent::Accel(_) => {}
        }
    }

    pub fn on_pressure(&mut self, sample: PressureSample) {
        // Keep the samples sorted by measurement time, even if one arrives out of order.
        let position = self
            .pressures
            .iter()
            .rposition(|earlier| earlier.time <= sample.time)
            .map_or(0, |index| index + 1);
        self.pressures.insert(position, sample);
        while self
            .pressures
            .front()
            .is_some_and(|oldest| sample.time.seconds_since(oldest.time) > PRESSURE_WAIT_S)
        {
            self.pressures.pop_front();
        }
        self.resolve_pending();
    }

    pub fn on_fix(&mut self, fix: &GpsFix) {
        // IGC has one-second resolution: drop fixes that do not advance the UTC second.
        if self.last_recorded.is_some_and(|last| fix.utc <= last) {
            return;
        }

        self.fixes.push(IgcFix {
            time: fix.utc.time,
            latitude_deg: fix.latitude_deg,
            longitude_deg: fix.longitude_deg,
            fix_3d: fix.fix_3d,
            // IGC convention: pressure altitude 0 means "no pressure sensor". Filled in
            // as soon as the pressure samples around the fix are known.
            pressure_altitude_m: 0,
            gps_altitude_m: fix.altitude_m.round() as i32,
            extension: String::new(),
        });
        self.pending.push_back((self.fixes.len() - 1, fix.time));
        self.start.get_or_insert(fix.utc);
        self.last_recorded = Some(fix.utc);
        self.revision += 1;

        // Give up on fixes that waited too long (no barometer, or it stopped).
        while self
            .pending
            .front()
            .is_some_and(|&(_, time)| fix.time.seconds_since(time) > PRESSURE_WAIT_S)
        {
            self.pending.pop_front();
        }
        self.resolve_pending();
    }

    /// Fills in the pressure altitude of every pending fix whose time is covered by the
    /// pressure samples received so far.
    fn resolve_pending(&mut self) {
        let Some(newest) = self.pressures.back() else {
            return;
        };
        let newest_time = newest.time;
        while let Some(&(index, time)) = self.pending.front() {
            if time > newest_time {
                // Fixes are in time order: the remaining ones are newer still.
                break;
            }
            self.pending.pop_front();
            if let Some(altitude) = pressure_altitude_at(&self.pressures, time) {
                self.fixes[index].pressure_altitude_m = altitude;
                self.revision += 1;
            }
        }
    }

    /// UTC date and time of the first recorded fix.
    pub fn start(&self) -> Option<UtcDateTime> {
        self.start
    }

    /// The fixes recorded so far.
    pub fn fixes(&self) -> &[IgcFix] {
        &self.fixes
    }

    /// A number that changes whenever the recorded track changes: a new fix or a
    /// pressure altitude filled in later. Lets a caller skip saving an unchanged track.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The recorded track as an IGC file, or `None` if nothing was recorded yet.
    ///
    /// Fixes still waiting for later pressure samples get the pressure altitude of the
    /// nearest sample, if it is close enough in time.
    pub fn to_igc(&self) -> Option<IgcFile> {
        let start = self.start?;
        let mut fixes = self.fixes.clone();
        for &(index, time) in &self.pending {
            if let Some(altitude) = pressure_altitude_at(&self.pressures, time) {
                fixes[index].pressure_altitude_m = altitude;
            }
        }
        let mut file = IgcFile::with_header(start.date, &self.meta);
        file.records.extend(fixes.into_iter().map(Record::Fix));
        Some(file)
    }
}

/// Pressure altitude at `time` in whole metres: interpolated between the samples around
/// `time`, or taken from the nearest sample if there is no such pair. `None` if no sample
/// is close enough. `samples` must be sorted by time.
fn pressure_altitude_at(samples: &VecDeque<PressureSample>, time: Timestamp) -> Option<i32> {
    let after = samples.iter().position(|sample| sample.time >= time);
    let before = match after {
        Some(0) => None,
        Some(index) => Some(index - 1),
        None => samples.len().checked_sub(1),
    };
    let altitude = match (before.map(|i| samples[i]), after.map(|i| samples[i])) {
        (Some(a), Some(b)) if b.time.seconds_since(a.time) <= MAX_PRESSURE_GAP_S => {
            let span = b.time.seconds_since(a.time);
            let fraction = if span > 0.0 {
                time.seconds_since(a.time) / span
            } else {
                0.0
            };
            let (alt_a, alt_b) = (
                pressure_to_altitude(a.pressure_pa),
                pressure_to_altitude(b.pressure_pa),
            );
            alt_a + (alt_b - alt_a) * fraction
        }
        (a, b) => {
            let nearest = [a, b].into_iter().flatten().min_by(|x, y| {
                let dx = time.seconds_since(x.time).abs();
                let dy = time.seconds_since(y.time).abs();
                dx.total_cmp(&dy)
            })?;
            if time.seconds_since(nearest.time).abs() > MAX_PRESSURE_DISTANCE_S {
                return None;
            }
            pressure_to_altitude(nearest.pressure_pa)
        }
    };
    Some(altitude.round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::altitude_to_pressure;
    use crate::time::{Date, TimeOfDay, Timestamp};

    fn fix(secs: f64, utc_second: u8) -> GpsFix {
        GpsFix {
            time: Timestamp::from_secs(secs),
            utc: UtcDateTime::new(
                Date::new(2026, 7, 28).unwrap(),
                TimeOfDay::new(10, 0, utc_second).unwrap(),
            ),
            latitude_deg: 47.5,
            longitude_deg: 11.25,
            altitude_m: 1500.4,
            fix_3d: true,
            ground_speed_mps: None,
            course_deg: None,
        }
    }

    fn pressure(secs: f64, altitude_m: f64) -> PressureSample {
        PressureSample {
            time: Timestamp::from_secs(secs),
            pressure_pa: altitude_to_pressure(altitude_m),
        }
    }

    #[test]
    fn empty_recorder_has_no_file() {
        assert!(TrackRecorder::new(FlightMeta::default()).to_igc().is_none());
    }

    #[test]
    fn records_fix_with_pressure_altitude() {
        let mut recorder = TrackRecorder::new(FlightMeta::default());
        recorder.on_pressure(pressure(0.0, 1432.0));
        recorder.on_fix(&fix(0.0, 0));

        let file = recorder.to_igc().unwrap();
        assert_eq!(file.date(), Date::new(2026, 7, 28));
        let fixes: Vec<_> = file.fixes().collect();
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].to_string(), "B1000004730000N01115000EA0143201500");
    }

    #[test]
    fn missing_or_stale_pressure_gives_zero_pressure_altitude() {
        let mut recorder = TrackRecorder::new(FlightMeta::default());
        recorder.on_fix(&fix(0.0, 0));
        recorder.on_pressure(pressure(5.0, 1432.0));
        // More than 3 s from both fixes: too far to be used.
        recorder.on_fix(&fix(10.0, 10));

        let altitudes: Vec<_> = igc_pressure_altitudes(&recorder);
        assert_eq!(altitudes, [0, 0]);
    }

    fn igc_pressure_altitudes(recorder: &TrackRecorder) -> Vec<i32> {
        recorder
            .to_igc()
            .unwrap()
            .fixes()
            .map(|f| f.pressure_altitude_m)
            .collect()
    }

    /// Like on the iPhone: each barometer sample arrives 2.7 s after it was measured,
    /// so it is always older than the latest GPS fix when it comes in.
    #[test]
    fn interpolates_late_pressure_samples_by_measurement_time() {
        let mut recorder = TrackRecorder::new(FlightMeta::default());
        let mut events: Vec<(f64, SensorEvent)> = Vec::new();
        for second in 0..10u8 {
            let t = f64::from(second);
            events.push((t, SensorEvent::Gps(fix(t, second))));
            // Barometer at ~0.94 Hz, steady climb of 2 m/s, delivered 2.7 s late.
            let measured = t * 1.066 + 0.3;
            let sample = pressure(measured, 1000.0 + 2.0 * measured);
            events.push((measured + 2.7, SensorEvent::Pressure(sample)));
        }
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, event) in &events {
            recorder.handle(event);
        }

        let altitudes = igc_pressure_altitudes(&recorder);
        // Fix 0 at t = 0 is before the first sample (0.3 s) and takes that one.
        assert_eq!(altitudes[0], 1001);
        for (second, &altitude) in altitudes.iter().enumerate().skip(1) {
            let expected = 1000.0 + 2.0 * second as f64;
            assert!(
                (f64::from(altitude) - expected).abs() <= 1.0,
                "fix {second}: {altitude} m, expected {expected} m"
            );
        }
    }

    #[test]
    fn filling_in_a_pressure_altitude_changes_the_revision() {
        let mut recorder = TrackRecorder::new(FlightMeta::default());
        recorder.on_pressure(pressure(0.0, 1000.0));
        recorder.on_fix(&fix(1.0, 1));
        let after_fix = recorder.revision();
        assert_eq!(recorder.fixes()[0].pressure_altitude_m, 0, "still pending");

        recorder.on_pressure(pressure(2.0, 1010.0));
        assert_ne!(recorder.revision(), after_fix);
        assert_eq!(recorder.fixes()[0].pressure_altitude_m, 1005);
    }

    #[test]
    fn does_not_interpolate_across_a_long_pressure_gap() {
        let mut recorder = TrackRecorder::new(FlightMeta::default());
        recorder.on_pressure(pressure(0.0, 1000.0));
        recorder.on_fix(&fix(4.0, 4));
        recorder.on_pressure(pressure(8.0, 1100.0));
        assert_eq!(igc_pressure_altitudes(&recorder), [0]);
    }

    #[test]
    fn keeps_at_most_one_fix_per_second() {
        let mut recorder = TrackRecorder::new(FlightMeta::default());
        recorder.on_fix(&fix(0.0, 0));
        recorder.on_fix(&fix(0.2, 0));
        recorder.on_fix(&fix(0.4, 0));
        recorder.on_fix(&fix(1.0, 1));
        assert_eq!(recorder.fixes().len(), 2);
    }
}
