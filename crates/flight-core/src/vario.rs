//! The variometer: estimates altitude and climb rate from pressure and acceleration.
//!
//! # Why a Kalman filter
//!
//! A barometer alone gives altitude. Its climb rate is the difference of two noisy
//! readings, and a phone barometer only delivers about one reading per second, so a
//! baro-only vario is either jumpy or late. The accelerometer is fast (50 Hz and more)
//! but drifts: integrating it gives a climb rate that wanders off within seconds.
//!
//! A Kalman filter combines both. It keeps an estimate of the state together with how
//! uncertain that estimate is, and works in two steps:
//!
//! * **predict**: move the state forward in time using the measured acceleration. The
//!   uncertainty grows.
//! * **update**: when a pressure reading arrives, pull the state towards it, weighted by
//!   how much the prediction and the reading can each be trusted. The uncertainty
//!   shrinks.
//!
//! The estimated state is altitude, climb rate, and the accelerometer's bias (its
//! constant error). Estimating the bias is what stops the drift.
//!
//! Because both steps work with the actual time difference between samples, the two
//! sensors may have any sample rate, and the rates may be irregular.

// Matrix code reads most clearly with explicit row and column indices.
#![allow(clippy::needless_range_loop)]

use crate::atmosphere::pressure_to_altitude;
use crate::sensor::{AccelSample, PressureSample, SensorEvent};
use crate::time::Timestamp;

/// Tuning parameters of the filter. The defaults are meant for phone sensors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VarioFilterConfig {
    /// How far the true vertical acceleration may deviate from the measured one, as a
    /// noise density in m/s² per √Hz. Larger values trust the barometer more.
    pub accel_noise_density: f64,
    /// Standard deviation of the barometric altitude in metres.
    pub altitude_noise_m: f64,
    /// How quickly the accelerometer bias may change, in m/s² per √s.
    pub bias_walk: f64,
    /// Acceleration uncertainty used while no acceleration data is available, in m/s²
    /// per √Hz. This is how hard a glider typically accelerates vertically.
    pub coasting_noise_density: f64,
    /// An acceleration sample older than this is no longer used.
    pub accel_timeout_s: f64,
    /// After a gap this long without any data the filter starts from scratch.
    pub reset_gap_s: f64,
}

impl Default for VarioFilterConfig {
    fn default() -> Self {
        VarioFilterConfig {
            accel_noise_density: 0.05,
            altitude_noise_m: 0.3,
            bias_walk: 0.02,
            coasting_noise_density: 1.0,
            accel_timeout_s: 0.5,
            reset_gap_s: 10.0,
        }
    }
}

/// The filter's current estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VarioReading {
    pub time: Timestamp,
    /// Pressure altitude in metres.
    pub altitude_m: f64,
    /// Climb rate in m/s, upwards positive, averaged over the damping time.
    pub climb_rate_mps: f64,
    /// Climb rate straight from the filter, without damping.
    pub raw_climb_rate_mps: f64,
}

// Indices into the state vector.
const ALT: usize = 0;
const VEL: usize = 1;
const BIAS: usize = 2;

type Matrix = [[f64; 3]; 3];

#[derive(Debug, Clone)]
pub struct VarioFilter {
    config: VarioFilterConfig,
    damping_s: f64,
    /// `None` until the first pressure sample arrives.
    running: Option<Running>,
    last_accel: Option<AccelSample>,
}

#[derive(Debug, Clone)]
struct Running {
    time: Timestamp,
    /// Altitude (m), climb rate (m/s), accelerometer bias (m/s²).
    state: [f64; 3],
    /// Covariance matrix: the uncertainty of the state and how the errors of its three
    /// components are related.
    covariance: Matrix,
    damped_climb_rate: f64,
}

impl VarioFilter {
    pub fn new(config: VarioFilterConfig) -> Self {
        VarioFilter {
            config,
            damping_s: 0.0,
            running: None,
            last_accel: None,
        }
    }

    /// Sets the averaging time of [`VarioReading::climb_rate_mps`] in seconds.
    pub fn set_damping(&mut self, seconds: f64) {
        self.damping_s = seconds.max(0.0);
    }

    /// Feeds any sensor sample. GPS fixes are ignored.
    pub fn handle(&mut self, event: &SensorEvent) {
        match event {
            SensorEvent::Accel(sample) => self.on_accel(*sample),
            SensorEvent::Pressure(sample) => self.on_pressure(*sample),
            SensorEvent::Gps(_) => {}
        }
    }

    pub fn on_accel(&mut self, sample: AccelSample) {
        self.last_accel = Some(sample);
        self.predict(sample.time);
    }

    pub fn on_pressure(&mut self, sample: PressureSample) {
        let measured_altitude = pressure_to_altitude(sample.pressure_pa);
        let variance = self.config.altitude_noise_m.powi(2);

        self.predict(sample.time);

        // `let ... else` handles the "not running yet" case and leaves the happy path
        // unindented.
        let Some(running) = &mut self.running else {
            self.running = Some(Running {
                time: sample.time,
                state: [measured_altitude, 0.0, 0.0],
                // Altitude known as well as the barometer allows; climb rate within
                // about 2 m/s; bias within about 0.5 m/s².
                covariance: [[variance, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 0.25]],
                damped_climb_rate: 0.0,
            });
            return;
        };

        // Update step for a measurement of the altitude component alone.
        let p = &mut running.covariance;
        let innovation = measured_altitude - running.state[ALT];
        let innovation_variance = p[ALT][ALT] + variance;
        // The gain says how much of the innovation each state component takes over.
        let gain = [
            p[ALT][ALT] / innovation_variance,
            p[VEL][ALT] / innovation_variance,
            p[BIAS][ALT] / innovation_variance,
        ];
        for i in 0..3 {
            running.state[i] += gain[i] * innovation;
        }
        let altitude_row = p[ALT];
        for i in 0..3 {
            for j in 0..3 {
                p[i][j] -= gain[i] * altitude_row[j];
            }
        }
        symmetrize(p);

        // Without damping the output must follow the corrected state at once. With
        // damping, the correction is blended in by the following predict steps.
        if self.damping_s == 0.0 {
            running.damped_climb_rate = running.state[VEL];
        }
    }

    /// The current estimate, or `None` before the first pressure sample.
    pub fn reading(&self) -> Option<VarioReading> {
        self.running.as_ref().map(|running| VarioReading {
            time: running.time,
            altitude_m: running.state[ALT],
            climb_rate_mps: running.damped_climb_rate,
            raw_climb_rate_mps: running.state[VEL],
        })
    }

    /// The estimated accelerometer bias in m/s², for diagnostics.
    pub fn accel_bias_mps2(&self) -> Option<f64> {
        self.running.as_ref().map(|running| running.state[BIAS])
    }

    /// Moves the estimate forward to `time`.
    fn predict(&mut self, time: Timestamp) {
        let Some(running) = &mut self.running else {
            return;
        };
        let dt = time.seconds_since(running.time);
        if dt <= 0.0 {
            // Late or duplicate sample: the state is already at or past this time.
            return;
        }
        if dt > self.config.reset_gap_s {
            self.running = None;
            return;
        }

        // Use the latest acceleration if it is recent enough. Otherwise "coast":
        // assume no acceleration, but with a large uncertainty.
        let accel = self
            .last_accel
            .filter(|sample| time.seconds_since(sample.time) <= self.config.accel_timeout_s);
        let (measured_accel, noise_density, use_bias) = match accel {
            Some(sample) => (sample.vertical_mps2, self.config.accel_noise_density, 1.0),
            None => (0.0, self.config.coasting_noise_density, 0.0),
        };

        let [altitude, velocity, bias] = running.state;
        let net_accel = measured_accel - use_bias * bias;
        running.state = [
            altitude + velocity * dt + 0.5 * net_accel * dt * dt,
            velocity + net_accel * dt,
            bias,
        ];

        // State transition matrix: how an error in each component at the start of the
        // step carries over to the end of the step.
        let f: Matrix = [
            [1.0, dt, -use_bias * 0.5 * dt * dt],
            [0.0, 1.0, -use_bias * dt],
            [0.0, 0.0, 1.0],
        ];
        let mut p = multiply(&multiply(&f, &running.covariance), &transpose(&f));

        // Process noise: uncertainty added during the step by acceleration the sensor
        // did not capture (continuous white-noise acceleration model).
        let q = noise_density * noise_density;
        p[ALT][ALT] += q * dt * dt * dt / 3.0;
        p[ALT][VEL] += q * dt * dt / 2.0;
        p[VEL][ALT] += q * dt * dt / 2.0;
        p[VEL][VEL] += q * dt;
        p[BIAS][BIAS] += self.config.bias_walk * self.config.bias_walk * dt;
        running.covariance = p;

        // Damping: exponential moving average with time constant `damping_s`.
        let weight = if self.damping_s > 0.0 {
            1.0 - (-dt / self.damping_s).exp()
        } else {
            1.0
        };
        running.damped_climb_rate += weight * (running.state[VEL] - running.damped_climb_rate);
        running.time = time;
    }
}

impl Default for VarioFilter {
    fn default() -> Self {
        Self::new(VarioFilterConfig::default())
    }
}

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut result = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                result[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    result
}

fn transpose(m: &Matrix) -> Matrix {
    let mut result = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            result[i][j] = m[j][i];
        }
    }
    result
}

/// Rounding errors slowly make the covariance asymmetric; average them away.
fn symmetrize(m: &mut Matrix) {
    for i in 0..3 {
        for j in (i + 1)..3 {
            let mean = 0.5 * (m[i][j] + m[j][i]);
            m[i][j] = mean;
            m[j][i] = mean;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::altitude_to_pressure;

    fn pressure(time_s: f64, altitude_m: f64) -> PressureSample {
        PressureSample {
            time: Timestamp::from_secs(time_s),
            pressure_pa: altitude_to_pressure(altitude_m),
        }
    }

    fn accel(time_s: f64, vertical_mps2: f64) -> AccelSample {
        AccelSample {
            time: Timestamp::from_secs(time_s),
            vertical_mps2,
        }
    }

    /// Feeds a constant climb: pressure at 1 Hz, acceleration (if any) at 50 Hz.
    fn feed_constant_climb(
        filter: &mut VarioFilter,
        rate: f64,
        seconds: u32,
        with_accel: bool,
        accel_bias: f64,
    ) {
        for step in 0..=(seconds * 50) {
            let t = f64::from(step) / 50.0;
            if with_accel {
                filter.on_accel(accel(t, accel_bias));
            }
            if step % 50 == 0 {
                filter.on_pressure(pressure(t, 1000.0 + rate * t));
            }
        }
    }

    #[test]
    fn has_no_reading_before_the_first_pressure_sample() {
        let mut filter = VarioFilter::default();
        assert!(filter.reading().is_none());
        filter.on_accel(accel(0.0, 0.0));
        assert!(filter.reading().is_none());
        filter.on_pressure(pressure(0.1, 1234.0));
        let reading = filter.reading().unwrap();
        assert!((reading.altitude_m - 1234.0).abs() < 1e-6);
        assert_eq!(reading.climb_rate_mps, 0.0);
    }

    #[test]
    fn converges_on_a_constant_climb_with_both_sensors() {
        let mut filter = VarioFilter::default();
        feed_constant_climb(&mut filter, 2.0, 15, true, 0.0);
        let reading = filter.reading().unwrap();
        assert!(
            (reading.raw_climb_rate_mps - 2.0).abs() < 0.05,
            "{reading:?}"
        );
        assert!((reading.altitude_m - 1030.0).abs() < 0.1, "{reading:?}");
    }

    #[test]
    fn works_with_the_barometer_alone() {
        let mut filter = VarioFilter::default();
        feed_constant_climb(&mut filter, -3.0, 15, false, 0.0);
        let reading = filter.reading().unwrap();
        assert!(
            (reading.raw_climb_rate_mps + 3.0).abs() < 0.05,
            "{reading:?}"
        );
    }

    #[test]
    fn learns_the_accelerometer_bias() {
        let mut filter = VarioFilter::default();
        // Level flight, but the accelerometer constantly reports 0.3 m/s² too much.
        feed_constant_climb(&mut filter, 0.0, 60, true, 0.3);
        let bias = filter.accel_bias_mps2().unwrap();
        assert!((bias - 0.3).abs() < 0.03, "bias {bias}");
        let reading = filter.reading().unwrap();
        assert!(reading.raw_climb_rate_mps.abs() < 0.05, "{reading:?}");
    }

    #[test]
    fn stale_acceleration_is_ignored() {
        let mut filter = VarioFilter::default();
        filter.on_pressure(pressure(0.0, 1000.0));
        // One wild acceleration sample, then the accelerometer goes silent.
        filter.on_accel(accel(0.02, 50.0));
        for second in 1..=15 {
            filter.on_pressure(pressure(f64::from(second), 1000.0));
        }
        let reading = filter.reading().unwrap();
        assert!(reading.raw_climb_rate_mps.abs() < 0.1, "{reading:?}");
        assert!((reading.altitude_m - 1000.0).abs() < 0.5, "{reading:?}");
    }

    #[test]
    fn damping_delays_the_climb_rate() {
        let mut undamped = VarioFilter::default();
        let mut damped = VarioFilter::default();
        damped.set_damping(2.0);
        for filter in [&mut undamped, &mut damped] {
            feed_constant_climb(filter, 3.0, 3, true, 0.0);
        }
        let fast = undamped.reading().unwrap();
        let slow = damped.reading().unwrap();
        assert_eq!(fast.climb_rate_mps, fast.raw_climb_rate_mps);
        assert!(slow.climb_rate_mps < fast.climb_rate_mps - 0.3);
        assert!(slow.climb_rate_mps > 0.5);
    }

    #[test]
    fn out_of_order_samples_do_not_move_time_backwards() {
        let mut filter = VarioFilter::default();
        filter.on_pressure(pressure(5.0, 1000.0));
        filter.on_accel(accel(4.0, 3.0));
        assert_eq!(filter.reading().unwrap().time, Timestamp::from_secs(5.0));
    }

    #[test]
    fn restarts_after_a_long_gap() {
        let mut filter = VarioFilter::default();
        feed_constant_climb(&mut filter, 4.0, 10, true, 0.0);
        // Sensor data resumes a minute later at a completely different altitude.
        // Without a reset the filter would report a huge sink rate.
        filter.on_pressure(pressure(70.0, 500.0));
        let reading = filter.reading().unwrap();
        assert!((reading.altitude_m - 500.0).abs() < 1e-6);
        assert_eq!(reading.raw_climb_rate_mps, 0.0);
    }
}
