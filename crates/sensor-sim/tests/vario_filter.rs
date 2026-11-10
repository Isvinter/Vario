//! Tests of the vario filter against synthetic flights, where the true climb rate is
//! known at every instant. Sensor rates and noise are chosen to resemble a phone:
//! barometer at 1 Hz, accelerometer at 50 Hz with noise and a constant bias.

use flight_core::sensor::SensorEvent;
use flight_core::vario::VarioFilter;
use sensor_sim::synthetic::{Segment, SyntheticFlight};
use sensor_sim::trajectory::Trajectory;
use sensor_sim::{SimConfig, Simulation};

use flight_core::time::{Date, TimeOfDay, UtcDateTime};

fn noisy_phone() -> SimConfig {
    SimConfig {
        // 3 Pa are about 25 cm of altitude noise.
        pressure_noise_pa: 3.0,
        accel_noise_mps2: 0.15,
        accel_bias_mps2: 0.15,
        ..SimConfig::default()
    }
}

/// How the filter is fed.
#[derive(Clone, Copy, PartialEq)]
enum Sensors {
    Both,
    BarometerOnly,
}

/// One comparison of the filter output with the truth.
struct Sample {
    time_s: f64,
    estimate: f64,
    truth: f64,
}

impl Sample {
    fn error(&self) -> f64 {
        self.estimate - self.truth
    }
}

/// Runs a flight through the filter and compares after every sensor sample.
fn run(flight: SyntheticFlight, config: &SimConfig, sensors: Sensors) -> Vec<Sample> {
    let truth = flight.clone();
    let start = UtcDateTime::new(Date::new(2026, 7, 28).unwrap(), TimeOfDay::MIDNIGHT);
    let mut simulation = Simulation::synthetic(flight, start, config);
    let mut filter = VarioFilter::default();
    let mut samples = Vec::new();

    simulation.run_fast(|event| {
        if sensors == Sensors::BarometerOnly && matches!(event, SensorEvent::Accel(_)) {
            return;
        }
        filter.handle(&event);
        if let Some(reading) = filter.reading() {
            let time_s = reading.time.as_secs();
            samples.push(Sample {
                time_s,
                estimate: reading.raw_climb_rate_mps,
                truth: truth.state_at(time_s).vertical_speed_mps,
            });
        }
    });
    samples
}

fn rms_error<'a>(samples: impl Iterator<Item = &'a Sample>) -> f64 {
    let (sum, count) = samples.fold((0.0, 0u32), |(sum, count), sample| {
        (sum + sample.error().powi(2), count + 1)
    });
    (sum / f64::from(count)).sqrt()
}

fn max_error<'a>(samples: impl Iterator<Item = &'a Sample>) -> f64 {
    samples
        .map(|sample| sample.error().abs())
        .fold(0.0, f64::max)
}

#[test]
fn ideal_sensors_give_the_exact_climb_rate() {
    for rate in [-4.0, -1.2, 0.5, 3.0] {
        let flight = SyntheticFlight::steady_climb(rate, 30.0);
        let samples = run(flight, &SimConfig::default(), Sensors::Both);
        let settled = samples.iter().filter(|s| s.time_s > 10.0);
        assert!(max_error(settled) < 0.02, "climb rate {rate}");
    }
}

#[test]
fn noisy_sensors_steady_climb() {
    let flight = SyntheticFlight::steady_climb(2.0, 120.0);
    let samples = run(flight, &noisy_phone(), Sensors::Both);
    let settled = || samples.iter().filter(|s| s.time_s > 30.0);

    let rms = rms_error(settled());
    let max = max_error(settled());
    println!("steady climb: rms {rms:.3} m/s, max {max:.3} m/s");
    assert!(rms < 0.12, "rms {rms}");
    assert!(max < 0.35, "max {max}");
}

#[test]
fn noisy_sensors_thermal_circles_and_strong_sink() {
    for flight in [
        SyntheticFlight::thermal_circles(2.5, 120.0),
        SyntheticFlight::strong_sink(4.0, 110.0),
    ] {
        let samples = run(flight, &noisy_phone(), Sensors::Both);
        let rms = rms_error(samples.iter().filter(|s| s.time_s > 30.0));
        println!("rms {rms:.3} m/s");
        assert!(rms < 0.12, "rms {rms}");
    }
}

#[test]
fn noisy_sensors_whole_demo_flight() {
    let samples = run(SyntheticFlight::demo(), &noisy_phone(), Sensors::Both);
    let settled = || samples.iter().filter(|s| s.time_s > 20.0);

    let rms = rms_error(settled());
    let max = max_error(settled());
    println!("demo flight: rms {rms:.3} m/s, max {max:.3} m/s");
    assert!(rms < 0.15, "rms {rms}");
    assert!(max < 0.5, "max {max}");
}

/// Glide, then the climb rate changes from -1 to +3 m/s within one second (entering a
/// strong thermal), starting at t = 40.2 s, right after a barometer sample.
fn thermal_entry() -> SyntheticFlight {
    SyntheticFlight::builder()
        .transition(1.0)
        .segment(Segment::straight(40.2, -1.0))
        .segment(Segment::straight(30.0, 3.0))
        .build()
}

#[test]
fn accelerometer_makes_the_vario_react_without_delay() {
    let samples = run(thermal_entry(), &noisy_phone(), Sensors::Both);

    // While the climb rate is changing, and before the next barometer sample could
    // have told a baro-only vario much about it, the estimate already follows.
    let during_change = || samples.iter().filter(|s| (40.2..42.0).contains(&s.time_s));
    let max = max_error(during_change());
    println!("thermal entry with accelerometer: max error {max:.3} m/s");
    assert!(max < 0.35, "max {max}");
}

#[test]
fn barometer_alone_still_converges_but_lags() {
    let samples = run(thermal_entry(), &noisy_phone(), Sensors::BarometerOnly);

    let lag = max_error(samples.iter().filter(|s| (40.2..42.0).contains(&s.time_s)));
    let later = rms_error(samples.iter().filter(|s| s.time_s > 50.0));
    println!("thermal entry, barometer only: max error {lag:.3} m/s, later rms {later:.3}");
    // With one pressure sample per second the change cannot be seen in time ...
    assert!(lag > 1.0, "lag {lag}");
    // ... but the estimate settles on the new climb rate afterwards.
    assert!(later < 0.5, "later rms {later}");
}

#[test]
fn works_with_a_fast_barometer_too() {
    let config = SimConfig {
        pressure_rate_hz: 25.0,
        ..noisy_phone()
    };
    let samples = run(SyntheticFlight::demo(), &config, Sensors::Both);
    let rms = rms_error(samples.iter().filter(|s| s.time_s > 20.0));
    println!("demo flight, 25 Hz barometer: rms {rms:.3} m/s");
    assert!(rms < 0.1, "rms {rms}");
}
