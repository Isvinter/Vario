//! Sensor samples and the traits every sensor source implements.
//!
//! The simulator, a Linux backend and the iOS bridge are all just implementations of
//! these traits, so the rest of the app never knows where its data comes from. A trait is
//! Rust's way of saying "any type that can do this"; code written against a trait works
//! with every implementation.
//!
//! All sources are *polled*: `poll_*` returns the next sample that is already available,
//! or `None` if there is nothing new right now. It never blocks. Platform backends that
//! receive callbacks (like iOS) put samples into a queue and hand them out here.

use crate::time::{Timestamp, UtcDateTime};

/// One GPS position fix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsFix {
    /// Monotonic time of the fix.
    pub time: Timestamp,
    /// UTC date and time reported by the GPS.
    pub utc: UtcDateTime,
    /// Latitude in degrees, north positive.
    pub latitude_deg: f64,
    /// Longitude in degrees, east positive.
    pub longitude_deg: f64,
    /// GPS altitude in metres.
    pub altitude_m: f64,
    /// `false` if the fix has no usable altitude (2D fix).
    pub fix_3d: bool,
    /// Speed over ground in m/s, if known.
    pub ground_speed_mps: Option<f64>,
    /// Course over ground in degrees (0 = north, 90 = east), if known.
    pub course_deg: Option<f64>,
}

/// One static air pressure reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressureSample {
    pub time: Timestamp,
    /// Static pressure in pascal.
    pub pressure_pa: f64,
}

/// One acceleration reading, already reduced to the vertical axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccelSample {
    pub time: Timestamp,
    /// Vertical acceleration in the earth frame in m/s², upwards positive, with gravity
    /// removed: 0 in unaccelerated flight. Rotating the raw device acceleration into the
    /// earth frame is the job of the platform backend.
    pub vertical_mps2: f64,
}

pub trait GpsSource {
    fn poll_fix(&mut self) -> Option<GpsFix>;
}

pub trait PressureSource {
    fn poll_pressure(&mut self) -> Option<PressureSample>;
}

pub trait AccelSource {
    fn poll_accel(&mut self) -> Option<AccelSample>;
}

/// A sample from any of the three sensors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SensorEvent {
    Accel(AccelSample),
    Pressure(PressureSample),
    Gps(GpsFix),
}

impl SensorEvent {
    pub fn time(&self) -> Timestamp {
        match self {
            SensorEvent::Accel(sample) => sample.time,
            SensorEvent::Pressure(sample) => sample.time,
            SensorEvent::Gps(fix) => fix.time,
        }
    }
}

/// Bundles one source of each kind and merges their samples into a single stream ordered
/// by timestamp.
///
/// The three type parameters `G`, `P`, `A` make this struct *generic*: the compiler
/// generates a specialised version for each combination of concrete source types, so
/// there is no runtime cost for the abstraction.
///
/// Samples with equal timestamps come out as acceleration, then pressure, then GPS. With
/// live sensors the ordering is best effort: a sample that arrives late cannot be put
/// before one that was already handed out.
pub struct SensorSet<G, P, A> {
    gps: G,
    pressure: P,
    accel: A,
    // One-sample look-ahead per source, needed to compare timestamps before deciding
    // which sample to hand out.
    pending_fix: Option<GpsFix>,
    pending_pressure: Option<PressureSample>,
    pending_accel: Option<AccelSample>,
}

impl<G: GpsSource, P: PressureSource, A: AccelSource> SensorSet<G, P, A> {
    pub fn new(gps: G, pressure: P, accel: A) -> Self {
        SensorSet {
            gps,
            pressure,
            accel,
            pending_fix: None,
            pending_pressure: None,
            pending_accel: None,
        }
    }

    /// The oldest available sample, or `None` if no source has anything new.
    pub fn next_event(&mut self) -> Option<SensorEvent> {
        if self.pending_accel.is_none() {
            self.pending_accel = self.accel.poll_accel();
        }
        if self.pending_pressure.is_none() {
            self.pending_pressure = self.pressure.poll_pressure();
        }
        if self.pending_fix.is_none() {
            self.pending_fix = self.gps.poll_fix();
        }

        // Candidates in tie-break order. `min_by` returns the first of several equal
        // minima, so on equal timestamps the earlier entry in this array wins.
        let candidates = [
            self.pending_accel.map(SensorEvent::Accel),
            self.pending_pressure.map(SensorEvent::Pressure),
            self.pending_fix.map(SensorEvent::Gps),
        ];
        let next = candidates
            .into_iter()
            .flatten()
            .min_by(|a, b| a.time().as_secs().total_cmp(&b.time().as_secs()))?;

        match next {
            SensorEvent::Accel(_) => self.pending_accel = None,
            SensorEvent::Pressure(_) => self.pending_pressure = None,
            SensorEvent::Gps(_) => self.pending_fix = None,
        }
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Date, TimeOfDay};
    use std::collections::VecDeque;

    // A fake source backed by a queue; implements all three traits for the tests.
    struct Queue<T>(VecDeque<T>);

    impl GpsSource for Queue<GpsFix> {
        fn poll_fix(&mut self) -> Option<GpsFix> {
            self.0.pop_front()
        }
    }
    impl PressureSource for Queue<PressureSample> {
        fn poll_pressure(&mut self) -> Option<PressureSample> {
            self.0.pop_front()
        }
    }
    impl AccelSource for Queue<AccelSample> {
        fn poll_accel(&mut self) -> Option<AccelSample> {
            self.0.pop_front()
        }
    }

    fn fix(secs: f64) -> GpsFix {
        GpsFix {
            time: Timestamp::from_secs(secs),
            utc: UtcDateTime::new(Date::new(2026, 1, 1).unwrap(), TimeOfDay::MIDNIGHT),
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            altitude_m: 0.0,
            fix_3d: true,
            ground_speed_mps: None,
            course_deg: None,
        }
    }

    fn pressure(secs: f64) -> PressureSample {
        PressureSample {
            time: Timestamp::from_secs(secs),
            pressure_pa: 90_000.0,
        }
    }

    fn accel(secs: f64) -> AccelSample {
        AccelSample {
            time: Timestamp::from_secs(secs),
            vertical_mps2: 0.0,
        }
    }

    #[test]
    fn merges_by_timestamp_with_fixed_tie_order() {
        let mut set = SensorSet::new(
            Queue(VecDeque::from([fix(0.0), fix(1.0)])),
            Queue(VecDeque::from([pressure(0.0), pressure(1.0)])),
            Queue(VecDeque::from([accel(0.0), accel(0.5), accel(1.0)])),
        );

        let mut order = Vec::new();
        while let Some(event) = set.next_event() {
            let kind = match event {
                SensorEvent::Accel(_) => 'a',
                SensorEvent::Pressure(_) => 'p',
                SensorEvent::Gps(_) => 'g',
            };
            order.push((kind, event.time().as_secs()));
        }

        assert_eq!(
            order,
            [
                ('a', 0.0),
                ('p', 0.0),
                ('g', 0.0),
                ('a', 0.5),
                ('a', 1.0),
                ('p', 1.0),
                ('g', 1.0),
            ]
        );
    }

    #[test]
    fn returns_none_when_all_sources_are_empty() {
        let mut set = SensorSet::new(
            Queue::<GpsFix>(VecDeque::new()),
            Queue::<PressureSample>(VecDeque::new()),
            Queue::<AccelSample>(VecDeque::new()),
        );
        assert!(set.next_event().is_none());
    }
}
