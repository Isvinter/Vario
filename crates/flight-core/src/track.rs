//! Recording a flight track from sensor samples.

use crate::atmosphere::pressure_to_altitude;
use crate::igc::{FlightMeta, IgcFile, IgcFix, Record};
use crate::sensor::{GpsFix, PressureSample, SensorEvent};
use crate::time::UtcDateTime;

/// A pressure sample older than this (relative to a GPS fix) is not used for the fix.
const MAX_PRESSURE_AGE_S: f64 = 2.0;

/// Collects GPS fixes at no more than one per second, together with the pressure altitude
/// at the time of each fix, and turns them into an IGC file.
#[derive(Debug, Clone)]
pub struct TrackRecorder {
    meta: FlightMeta,
    last_pressure: Option<PressureSample>,
    start: Option<UtcDateTime>,
    last_recorded: Option<UtcDateTime>,
    fixes: Vec<IgcFix>,
}

impl TrackRecorder {
    pub fn new(meta: FlightMeta) -> Self {
        TrackRecorder {
            meta,
            last_pressure: None,
            start: None,
            last_recorded: None,
            fixes: Vec::new(),
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
        self.last_pressure = Some(sample);
    }

    pub fn on_fix(&mut self, fix: &GpsFix) {
        // IGC has one-second resolution: drop fixes that do not advance the UTC second.
        if self.last_recorded.is_some_and(|last| fix.utc <= last) {
            return;
        }

        let pressure_altitude_m = self
            .last_pressure
            .filter(|sample| {
                let age = fix.time.seconds_since(sample.time);
                (0.0..=MAX_PRESSURE_AGE_S).contains(&age)
            })
            // IGC convention: pressure altitude 0 means "no pressure sensor".
            .map_or(0, |sample| {
                pressure_to_altitude(sample.pressure_pa).round() as i32
            });

        self.fixes.push(IgcFix {
            time: fix.utc.time,
            latitude_deg: fix.latitude_deg,
            longitude_deg: fix.longitude_deg,
            fix_3d: fix.fix_3d,
            pressure_altitude_m,
            gps_altitude_m: fix.altitude_m.round() as i32,
            extension: String::new(),
        });
        self.start.get_or_insert(fix.utc);
        self.last_recorded = Some(fix.utc);
    }

    /// The fixes recorded so far.
    pub fn fixes(&self) -> &[IgcFix] {
        &self.fixes
    }

    /// The recorded track as an IGC file, or `None` if nothing was recorded yet.
    pub fn to_igc(&self) -> Option<IgcFile> {
        let start = self.start?;
        let mut file = IgcFile::with_header(start.date, &self.meta);
        file.records
            .extend(self.fixes.iter().cloned().map(Record::Fix));
        Some(file)
    }
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
        recorder.on_pressure(pressure(1.0, 1432.0));
        recorder.on_fix(&fix(10.0, 10));

        let altitudes: Vec<_> = recorder
            .fixes()
            .iter()
            .map(|f| f.pressure_altitude_m)
            .collect();
        assert_eq!(altitudes, [0, 0]);
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
