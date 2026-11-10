//! Synthetic flights built from segments with known climb rates.
//!
//! Everything is computed with closed formulas, so the true altitude, climb rate and
//! vertical acceleration are known exactly at every instant. That makes these flights the
//! reference for testing the vario filter.

use std::f64::consts::TAU;

use flight_core::geo::offset_position;

use crate::trajectory::{Trajectory, TruthState};

/// One leg of a synthetic flight with constant target climb rate, ground speed and turn
/// rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub duration_s: f64,
    /// Climb rate in m/s the glider settles on, upwards positive.
    pub climb_rate_mps: f64,
    pub ground_speed_mps: f64,
    /// Seconds per full circle; positive turns right, negative left. `None` flies
    /// straight ahead.
    pub turn_period_s: Option<f64>,
}

impl Segment {
    /// Straight flight at a typical paraglider trim speed.
    pub fn straight(duration_s: f64, climb_rate_mps: f64) -> Self {
        Segment {
            duration_s,
            climb_rate_mps,
            ground_speed_mps: 10.0,
            turn_period_s: None,
        }
    }

    /// Right-hand circles, slightly slower than trim speed as when thermalling.
    pub fn circling(duration_s: f64, climb_rate_mps: f64, turn_period_s: f64) -> Self {
        Segment {
            duration_s,
            climb_rate_mps,
            ground_speed_mps: 8.0,
            turn_period_s: Some(turn_period_s),
        }
    }
}

/// Builds a [`SyntheticFlight`] segment by segment.
#[derive(Debug, Clone)]
pub struct SyntheticFlightBuilder {
    latitude_deg: f64,
    longitude_deg: f64,
    altitude_m: f64,
    heading_deg: f64,
    transition_s: f64,
    segments: Vec<Segment>,
}

impl SyntheticFlightBuilder {
    /// Start position and altitude. Defaults to 47°N 11°E at 1500 m.
    pub fn start(mut self, latitude_deg: f64, longitude_deg: f64, altitude_m: f64) -> Self {
        self.latitude_deg = latitude_deg;
        self.longitude_deg = longitude_deg;
        self.altitude_m = altitude_m;
        self
    }

    /// Initial heading in degrees. Defaults to 0 (north).
    pub fn heading(mut self, heading_deg: f64) -> Self {
        self.heading_deg = heading_deg;
        self
    }

    /// Time the climb rate takes to change smoothly from one segment's value to the
    /// next. Defaults to 2 s. A real glider cannot change its climb rate instantly, and
    /// an instant change would mean infinite acceleration.
    pub fn transition(mut self, seconds: f64) -> Self {
        self.transition_s = seconds;
        self
    }

    pub fn segment(mut self, segment: Segment) -> Self {
        self.segments.push(segment);
        self
    }

    /// # Panics
    ///
    /// Panics if there is no segment or a segment has a non-positive duration or a zero
    /// turn period. These are programming errors, not runtime conditions.
    pub fn build(self) -> SyntheticFlight {
        assert!(!self.segments.is_empty(), "a flight needs a segment");

        let mut legs = Vec::with_capacity(self.segments.len());
        let mut state = LegStart {
            time_s: 0.0,
            altitude_m: self.altitude_m,
            // The flight starts already settled on the first segment's climb rate.
            climb_rate_mps: self.segments[0].climb_rate_mps,
            north_m: 0.0,
            east_m: 0.0,
            heading_rad: self.heading_deg.to_radians(),
        };

        for segment in self.segments {
            assert!(
                segment.duration_s > 0.0,
                "segment duration must be positive"
            );
            assert!(
                segment.turn_period_s != Some(0.0),
                "turn period must not be zero"
            );
            let leg = Leg {
                start: state,
                segment,
                transition_s: self.transition_s.clamp(0.0, segment.duration_s),
            };
            state = leg.end();
            legs.push(leg);
        }

        SyntheticFlight {
            latitude_deg: self.latitude_deg,
            longitude_deg: self.longitude_deg,
            duration_s: state.time_s,
            legs,
        }
    }
}

/// A flight made of [`Segment`]s. Create one with [`SyntheticFlight::builder`] or one of
/// the ready-made profiles.
#[derive(Debug, Clone)]
pub struct SyntheticFlight {
    latitude_deg: f64,
    longitude_deg: f64,
    duration_s: f64,
    legs: Vec<Leg>,
}

impl SyntheticFlight {
    pub fn builder() -> SyntheticFlightBuilder {
        SyntheticFlightBuilder {
            latitude_deg: 47.0,
            longitude_deg: 11.0,
            altitude_m: 1500.0,
            heading_deg: 0.0,
            transition_s: 2.0,
            segments: Vec::new(),
        }
    }

    /// Straight flight with a constant climb (or sink) rate.
    pub fn steady_climb(climb_rate_mps: f64, duration_s: f64) -> Self {
        Self::builder()
            .segment(Segment::straight(duration_s, climb_rate_mps))
            .build()
    }

    /// Circling in a thermal: one full circle every 20 seconds.
    pub fn thermal_circles(climb_rate_mps: f64, duration_s: f64) -> Self {
        Self::builder()
            .segment(Segment::circling(duration_s, climb_rate_mps, 20.0))
            .build()
    }

    /// Normal glide followed by strong sink, as when flying into heavy lee-side sink.
    pub fn strong_sink(sink_rate_mps: f64, duration_s: f64) -> Self {
        Self::builder()
            .segment(Segment::straight(10.0, -1.2))
            .segment(Segment::straight(duration_s, -sink_rate_mps.abs()))
            .build()
    }

    /// A short flight touching every vario tone range: glide, weak and strong thermal,
    /// glide, strong sink, glide. About four minutes.
    pub fn demo() -> Self {
        Self::builder()
            .segment(Segment::straight(30.0, -1.2))
            .segment(Segment::circling(40.0, 1.0, 20.0))
            .segment(Segment::circling(60.0, 3.0, 18.0))
            .segment(Segment::straight(30.0, -1.2))
            .segment(Segment::straight(20.0, -3.5))
            .segment(Segment::straight(40.0, -1.0))
            .build()
    }
}

impl Trajectory for SyntheticFlight {
    fn duration_s(&self) -> f64 {
        self.duration_s
    }

    fn state_at(&self, t: f64) -> TruthState {
        let t = t.clamp(0.0, self.duration_s);
        // Index of the last leg that starts at or before `t`.
        let index = self
            .legs
            .partition_point(|leg| leg.start.time_s <= t)
            .saturating_sub(1);
        let leg = &self.legs[index];
        let local = leg.at(t - leg.start.time_s);

        let (latitude_deg, longitude_deg) = offset_position(
            self.latitude_deg,
            self.longitude_deg,
            local.north_m,
            local.east_m,
        );
        TruthState {
            latitude_deg,
            longitude_deg,
            altitude_m: local.altitude_m,
            vertical_speed_mps: local.climb_rate_mps,
            vertical_accel_mps2: local.vertical_accel_mps2,
            ground_speed_mps: leg.segment.ground_speed_mps,
            course_deg: local.heading_rad.to_degrees().rem_euclid(360.0),
        }
    }
}

/// State at the beginning of a leg, relative to the start position of the flight.
#[derive(Debug, Clone, Copy)]
struct LegStart {
    time_s: f64,
    altitude_m: f64,
    climb_rate_mps: f64,
    north_m: f64,
    east_m: f64,
    heading_rad: f64,
}

#[derive(Debug, Clone, Copy)]
struct LegState {
    altitude_m: f64,
    climb_rate_mps: f64,
    vertical_accel_mps2: f64,
    north_m: f64,
    east_m: f64,
    heading_rad: f64,
}

#[derive(Debug, Clone, Copy)]
struct Leg {
    start: LegStart,
    segment: Segment,
    transition_s: f64,
}

impl Leg {
    /// State `s` seconds into the leg.
    fn at(&self, s: f64) -> LegState {
        let v0 = self.start.climb_rate_mps;
        let dv = self.segment.climb_rate_mps - v0;
        let tau = self.transition_s;

        // The climb rate blends from v0 to the target with the "smoothstep" curve
        // 3u² - 2u³, which starts and ends with zero slope, so the acceleration is
        // continuous. Altitude is the exact integral of the climb rate.
        let (blend, blend_integral, blend_slope) = if s < tau {
            let u = s / tau;
            (
                3.0 * u * u - 2.0 * u * u * u,
                tau * (u * u * u - 0.5 * u * u * u * u),
                6.0 * u * (1.0 - u) / tau,
            )
        } else {
            (1.0, 0.5 * tau + (s - tau), 0.0)
        };

        let speed = self.segment.ground_speed_mps;
        let h0 = self.start.heading_rad;
        let (heading_rad, north, east) = match self.segment.turn_period_s {
            Some(period) => {
                // Constant turn rate: the path is an arc of a circle.
                let omega = TAU / period;
                let h = h0 + omega * s;
                (
                    h,
                    speed / omega * (h.sin() - h0.sin()),
                    -speed / omega * (h.cos() - h0.cos()),
                )
            }
            None => (h0, speed * h0.cos() * s, speed * h0.sin() * s),
        };

        LegState {
            altitude_m: self.start.altitude_m + v0 * s + dv * blend_integral,
            climb_rate_mps: v0 + dv * blend,
            vertical_accel_mps2: dv * blend_slope,
            north_m: self.start.north_m + north,
            east_m: self.start.east_m + east,
            heading_rad,
        }
    }

    fn end(&self) -> LegStart {
        let state = self.at(self.segment.duration_s);
        LegStart {
            time_s: self.start.time_s + self.segment.duration_s,
            altitude_m: state.altitude_m,
            climb_rate_mps: state.climb_rate_mps,
            north_m: state.north_m,
            east_m: state.east_m,
            heading_rad: state.heading_rad,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flight_core::geo::distance_m;

    #[test]
    fn steady_climb_gains_rate_times_time() {
        let flight = SyntheticFlight::steady_climb(2.0, 60.0);
        assert_eq!(flight.duration_s(), 60.0);
        let state = flight.state_at(30.0);
        assert!((state.altitude_m - 1560.0).abs() < 1e-9);
        assert_eq!(state.vertical_speed_mps, 2.0);
        assert_eq!(state.vertical_accel_mps2, 0.0);
    }

    #[test]
    fn straight_flight_covers_speed_times_time() {
        let flight = SyntheticFlight::builder()
            .heading(90.0)
            .segment(Segment::straight(100.0, 0.0))
            .build();
        let end = flight.state_at(100.0);
        let d = distance_m(47.0, 11.0, end.latitude_deg, end.longitude_deg);
        assert!((d - 1000.0).abs() < 1.0, "{d}");
        assert!((end.latitude_deg - 47.0).abs() < 1e-9);
        assert!(end.longitude_deg > 11.0);
        assert!((end.course_deg - 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_full_circle_returns_to_its_start() {
        let flight = SyntheticFlight::thermal_circles(2.0, 60.0);
        let start = flight.state_at(0.0);
        let half = flight.state_at(10.0);
        let full = flight.state_at(20.0);

        let closed = distance_m(
            start.latitude_deg,
            start.longitude_deg,
            full.latitude_deg,
            full.longitude_deg,
        );
        assert!(closed < 0.01, "{closed}");

        // Circumference 8 m/s * 20 s = 160 m, so the diameter is 160/pi = 50.9 m.
        let diameter = distance_m(
            start.latitude_deg,
            start.longitude_deg,
            half.latitude_deg,
            half.longitude_deg,
        );
        assert!(
            (diameter - 160.0 / std::f64::consts::PI).abs() < 0.1,
            "{diameter}"
        );
        assert!((half.course_deg - 180.0).abs() < 1e-6);
    }

    #[test]
    fn climb_rate_changes_smoothly_between_segments() {
        let flight = SyntheticFlight::builder()
            .transition(2.0)
            .segment(Segment::straight(10.0, -1.0))
            .segment(Segment::straight(10.0, 3.0))
            .build();

        assert_eq!(flight.state_at(9.99).vertical_speed_mps, -1.0);
        let mid = flight.state_at(11.0);
        assert!((mid.vertical_speed_mps - 1.0).abs() < 1e-9);
        // Peak acceleration of a smoothstep over 2 s spanning 4 m/s: 1.5 * 4 / 2.
        assert!((mid.vertical_accel_mps2 - 3.0).abs() < 1e-9);
        assert_eq!(flight.state_at(12.0).vertical_speed_mps, 3.0);
        assert_eq!(flight.state_at(12.0).vertical_accel_mps2, 0.0);
    }

    #[test]
    fn altitude_speed_and_acceleration_are_consistent() {
        // Check numerically that speed is the derivative of altitude and acceleration
        // the derivative of speed, across all segment boundaries of the demo flight.
        let flight = SyntheticFlight::demo();
        let h = 1e-4;
        let mut t = 0.5;
        while t < flight.duration_s() - 0.5 {
            let before = flight.state_at(t - h);
            let here = flight.state_at(t);
            let after = flight.state_at(t + h);

            let speed = (after.altitude_m - before.altitude_m) / (2.0 * h);
            assert!(
                (speed - here.vertical_speed_mps).abs() < 1e-5,
                "speed at {t}"
            );
            let accel = (after.vertical_speed_mps - before.vertical_speed_mps) / (2.0 * h);
            assert!(
                (accel - here.vertical_accel_mps2).abs() < 1e-3,
                "acceleration at {t}"
            );
            t += 0.37;
        }
    }

    #[test]
    fn times_outside_the_flight_are_clamped() {
        let flight = SyntheticFlight::steady_climb(1.0, 10.0);
        assert_eq!(flight.state_at(-5.0), flight.state_at(0.0));
        assert_eq!(flight.state_at(99.0), flight.state_at(10.0));
    }
}
