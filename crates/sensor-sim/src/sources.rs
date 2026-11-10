//! Simulated sensors that sample a [`Trajectory`].
//!
//! Each source produces its samples lazily: when polled, it checks whether its next
//! sample time has been reached on the shared [`SimClock`] and only then computes the
//! sample. Sample `k` of a source with rate `r` is due at exactly `k / r` seconds.

use std::sync::Arc;

use flight_core::atmosphere::altitude_to_pressure;
use flight_core::sensor::{
    AccelSample, AccelSource, GpsFix, GpsSource, PressureSample, PressureSource,
};
use flight_core::time::{Timestamp, UtcDateTime};

use crate::SimConfig;
use crate::clock::SimClock;
use crate::trajectory::Trajectory;

/// Hands out sample times `k / rate` once the clock has reached them.
struct Schedule {
    clock: SimClock,
    rate_hz: f64,
    duration_s: f64,
    next_index: u64,
}

impl Schedule {
    fn new(clock: SimClock, rate_hz: f64, duration_s: f64) -> Self {
        assert!(rate_hz > 0.0, "sample rate must be positive");
        Schedule {
            clock,
            rate_hz,
            duration_s,
            next_index: 0,
        }
    }

    /// The time of the next sample if it is due, advancing to the one after it.
    fn next_due(&mut self) -> Option<f64> {
        let t = self.next_index as f64 / self.rate_hz;
        if t > self.clock.now() || t > self.duration_s {
            return None;
        }
        self.next_index += 1;
        Some(t)
    }
}

/// GPS receiver for synthetic flights.
pub struct SimGps {
    trajectory: Arc<dyn Trajectory>,
    schedule: Schedule,
    start: UtcDateTime,
}

impl SimGps {
    pub fn new(
        trajectory: Arc<dyn Trajectory>,
        clock: SimClock,
        rate_hz: f64,
        start: UtcDateTime,
    ) -> Self {
        let schedule = Schedule::new(clock, rate_hz, trajectory.duration_s());
        SimGps {
            trajectory,
            schedule,
            start,
        }
    }
}

impl GpsSource for SimGps {
    fn poll_fix(&mut self) -> Option<GpsFix> {
        let t = self.schedule.next_due()?;
        let state = self.trajectory.state_at(t);
        Some(GpsFix {
            time: Timestamp::from_secs(t),
            utc: self.start.plus_seconds(t as u64),
            latitude_deg: state.latitude_deg,
            longitude_deg: state.longitude_deg,
            altitude_m: state.altitude_m,
            fix_3d: true,
            ground_speed_mps: Some(state.ground_speed_mps),
            course_deg: Some(state.course_deg),
        })
    }
}

/// Barometer: reports the standard-atmosphere pressure at the true altitude.
pub struct SimBaro {
    trajectory: Arc<dyn Trajectory>,
    schedule: Schedule,
    noise_pa: f64,
    rng: Rng,
}

impl SimBaro {
    pub fn new(trajectory: Arc<dyn Trajectory>, clock: SimClock, config: &SimConfig) -> Self {
        let schedule = Schedule::new(clock, config.pressure_rate_hz, trajectory.duration_s());
        SimBaro {
            trajectory,
            schedule,
            noise_pa: config.pressure_noise_pa,
            rng: Rng::new(config.seed),
        }
    }
}

impl PressureSource for SimBaro {
    fn poll_pressure(&mut self) -> Option<PressureSample> {
        let t = self.schedule.next_due()?;
        let altitude_m = self.trajectory.state_at(t).altitude_m;
        Some(PressureSample {
            time: Timestamp::from_secs(t),
            pressure_pa: altitude_to_pressure(altitude_m) + self.rng.noise(self.noise_pa),
        })
    }
}

/// Accelerometer: reports the true vertical acceleration plus bias and noise.
pub struct SimAccel {
    trajectory: Arc<dyn Trajectory>,
    schedule: Schedule,
    noise_mps2: f64,
    bias_mps2: f64,
    rng: Rng,
}

impl SimAccel {
    pub fn new(trajectory: Arc<dyn Trajectory>, clock: SimClock, config: &SimConfig) -> Self {
        let schedule = Schedule::new(clock, config.accel_rate_hz, trajectory.duration_s());
        SimAccel {
            trajectory,
            schedule,
            noise_mps2: config.accel_noise_mps2,
            bias_mps2: config.accel_bias_mps2,
            // A different stream than the barometer, so their noise is uncorrelated.
            rng: Rng::new(config.seed.wrapping_add(1)),
        }
    }
}

impl AccelSource for SimAccel {
    fn poll_accel(&mut self) -> Option<AccelSample> {
        let t = self.schedule.next_due()?;
        let truth = self.trajectory.state_at(t).vertical_accel_mps2;
        Some(AccelSample {
            time: Timestamp::from_secs(t),
            vertical_mps2: truth + self.bias_mps2 + self.rng.noise(self.noise_mps2),
        })
    }
}

/// Small deterministic random number generator (xorshift64). Statistical quality is
/// irrelevant here; what matters is that the same seed always gives the same noise, so
/// tests are repeatable. This avoids a dependency on the `rand` crate.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Mix the seed so that small seeds give unrelated streams; the state must not
        // be zero or the generator would stay at zero forever.
        let mixed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
        Rng(mixed.max(1))
    }

    /// Uniformly distributed in (0, 1].
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        // Use the top 53 bits, the precision of an f64.
        ((self.0 >> 11) + 1) as f64 / (1u64 << 53) as f64
    }

    /// Normally distributed with mean 0 and the given standard deviation.
    fn noise(&mut self, std_dev: f64) -> f64 {
        if std_dev == 0.0 {
            return 0.0;
        }
        // Box-Muller transform: turns two uniform numbers into a normal one.
        let radius = (-2.0 * self.uniform().ln()).sqrt();
        let angle = std::f64::consts::TAU * self.uniform();
        std_dev * radius * angle.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::SyntheticFlight;
    use flight_core::atmosphere::pressure_to_altitude;
    use flight_core::time::{Date, TimeOfDay};

    fn climb() -> Arc<dyn Trajectory> {
        Arc::new(SyntheticFlight::steady_climb(2.0, 10.0))
    }

    #[test]
    fn samples_appear_only_when_the_clock_reaches_them() {
        let clock = SimClock::new();
        let mut baro = SimBaro::new(climb(), clock.clone(), &SimConfig::default());

        assert!(baro.poll_pressure().is_some(), "sample at t = 0");
        assert!(baro.poll_pressure().is_none());

        clock.set(2.5);
        let times: Vec<f64> = std::iter::from_fn(|| baro.poll_pressure())
            .map(|s| s.time.as_secs())
            .collect();
        assert_eq!(times, [1.0, 2.0]);
    }

    #[test]
    fn sources_stop_at_the_end_of_the_flight() {
        let clock = SimClock::new();
        clock.set(1000.0);
        let config = SimConfig::default();
        let mut baro = SimBaro::new(climb(), clock.clone(), &config);
        let mut accel = SimAccel::new(climb(), clock, &config);

        // 0..=10 s inclusive: 11 samples at 1 Hz, 501 at 50 Hz.
        assert_eq!(std::iter::from_fn(|| baro.poll_pressure()).count(), 11);
        assert_eq!(std::iter::from_fn(|| accel.poll_accel()).count(), 501);
    }

    #[test]
    fn barometer_reports_pressure_of_true_altitude() {
        let clock = SimClock::new();
        clock.set(5.0);
        let mut baro = SimBaro::new(climb(), clock, &SimConfig::default());
        let last = std::iter::from_fn(|| baro.poll_pressure()).last().unwrap();
        assert!((pressure_to_altitude(last.pressure_pa) - 1510.0).abs() < 1e-6);
    }

    #[test]
    fn gps_reports_position_and_utc_time() {
        let clock = SimClock::new();
        clock.set(3.0);
        let start = UtcDateTime::new(
            Date::new(2026, 7, 28).unwrap(),
            TimeOfDay::new(23, 59, 58).unwrap(),
        );
        let mut gps = SimGps::new(climb(), clock, 1.0, start);
        let fixes: Vec<GpsFix> = std::iter::from_fn(|| gps.poll_fix()).collect();

        assert_eq!(fixes.len(), 4);
        assert_eq!(fixes[3].altitude_m, 1506.0);
        assert_eq!(fixes[3].utc.date, Date::new(2026, 7, 29).unwrap());
        assert_eq!(fixes[3].utc.time, TimeOfDay::new(0, 0, 1).unwrap());
        assert_eq!(fixes[3].ground_speed_mps, Some(10.0));
    }

    #[test]
    fn noise_is_repeatable_and_has_the_requested_spread() {
        let config = SimConfig {
            accel_noise_mps2: 0.5,
            accel_bias_mps2: 0.2,
            ..SimConfig::default()
        };
        let run = || {
            let clock = SimClock::new();
            clock.set(1000.0);
            let flight: Arc<dyn Trajectory> = Arc::new(SyntheticFlight::steady_climb(0.0, 200.0));
            let mut accel = SimAccel::new(flight, clock, &config);
            std::iter::from_fn(move || accel.poll_accel())
                .map(|s| s.vertical_mps2)
                .collect::<Vec<f64>>()
        };

        let values = run();
        assert_eq!(values, run(), "same seed must give the same noise");

        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        assert!((mean - 0.2).abs() < 0.02, "mean {mean}");
        assert!(
            (variance.sqrt() - 0.5).abs() < 0.02,
            "std {}",
            variance.sqrt()
        );
    }
}
