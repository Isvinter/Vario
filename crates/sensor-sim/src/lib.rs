//! Simulated sensors for development and tests.
//!
//! Two kinds of flights can be simulated:
//!
//! * a recorded flight, replayed from an IGC file ([`replay`]);
//! * a synthetic flight built from segments with known climb rates ([`synthetic`]).
//!
//! Both are described by a [`trajectory::Trajectory`]. The sensor sources in [`sources`]
//! sample that trajectory at their own rates, driven by a shared [`clock::SimClock`], so
//! a flight can run in real time, accelerated, or as fast as the computer allows.
//! [`Simulation`] ties everything together.

pub mod clock;
pub mod replay;
pub mod sources;
pub mod synthetic;
pub mod trajectory;

use std::sync::Arc;
use std::time::Duration;

use flight_core::igc::IgcFile;
use flight_core::sensor::{GpsSource, SensorEvent, SensorSet};
use flight_core::time::UtcDateTime;

use crate::clock::{Pacer, SimClock};
use crate::replay::{IgcReplay, ReplayError, ReplayGps};
use crate::sources::{SimAccel, SimBaro, SimGps};
use crate::synthetic::SyntheticFlight;
use crate::trajectory::Trajectory;

/// Sample rates and sensor imperfections of the simulated sensors.
#[derive(Debug, Clone, PartialEq)]
pub struct SimConfig {
    /// GPS rate for synthetic flights. A replay always uses the fix times of the file.
    pub gps_rate_hz: f64,
    /// The iPhone barometer delivers roughly one value per second.
    pub pressure_rate_hz: f64,
    pub accel_rate_hz: f64,
    /// Standard deviation of the pressure noise in pascal (1 Pa is about 8 cm).
    pub pressure_noise_pa: f64,
    /// Standard deviation of the acceleration noise in m/s².
    pub accel_noise_mps2: f64,
    /// Constant acceleration offset in m/s².
    pub accel_bias_mps2: f64,
    /// Seed of the noise generator. The same seed always gives the same noise.
    pub seed: u64,
}

impl Default for SimConfig {
    /// Ideal sensors without noise at phone-like sample rates.
    fn default() -> Self {
        SimConfig {
            gps_rate_hz: 1.0,
            pressure_rate_hz: 1.0,
            accel_rate_hz: 50.0,
            pressure_noise_pa: 0.0,
            accel_noise_mps2: 0.0,
            accel_bias_mps2: 0.0,
            seed: 1,
        }
    }
}

/// A complete simulated flight: clock, the three sensor sources and the flight duration.
///
/// `G` is the type of the GPS source: [`SimGps`] for synthetic flights, [`ReplayGps`]
/// for IGC replay.
pub struct Simulation<G> {
    pub clock: SimClock,
    pub sensors: SensorSet<G, SimBaro, SimAccel>,
    pub duration_s: f64,
}

impl Simulation<SimGps> {
    /// Simulates a synthetic flight that starts at the given UTC time.
    pub fn synthetic(flight: SyntheticFlight, start: UtcDateTime, config: &SimConfig) -> Self {
        let clock = SimClock::new();
        // `Arc` is a reference-counted pointer: the three sources share one trajectory
        // without copying it, and it is freed when the last source is dropped.
        let trajectory: Arc<dyn Trajectory> = Arc::new(flight);
        let gps = SimGps::new(
            Arc::clone(&trajectory),
            clock.clone(),
            config.gps_rate_hz,
            start,
        );
        Self::assemble(clock, gps, trajectory, config)
    }
}

impl Simulation<ReplayGps> {
    /// Replays a recorded flight. GPS fixes come out exactly as stored in the file;
    /// pressure and acceleration are derived from the altitude profile.
    pub fn replay(file: &IgcFile, config: &SimConfig) -> Result<Self, ReplayError> {
        let clock = SimClock::new();
        let replay = IgcReplay::new(file)?;
        let gps = replay.gps(clock.clone());
        let trajectory: Arc<dyn Trajectory> = Arc::new(replay.into_trajectory());
        Ok(Self::assemble(clock, gps, trajectory, config))
    }
}

impl<G: GpsSource> Simulation<G> {
    fn assemble(
        clock: SimClock,
        gps: G,
        trajectory: Arc<dyn Trajectory>,
        config: &SimConfig,
    ) -> Self {
        let duration_s = trajectory.duration_s();
        let baro = SimBaro::new(Arc::clone(&trajectory), clock.clone(), config);
        let accel = SimAccel::new(trajectory, clock.clone(), config);
        Simulation {
            clock,
            sensors: SensorSet::new(gps, baro, accel),
            duration_s,
        }
    }

    /// `true` once the simulated time has reached the end of the flight.
    pub fn finished(&self) -> bool {
        self.clock.now() >= self.duration_s
    }

    /// Runs the whole flight as fast as possible, passing every sample to `on_event`.
    pub fn run_fast(&mut self, mut on_event: impl FnMut(SensorEvent)) {
        self.clock.set(self.duration_s);
        while let Some(event) = self.sensors.next_event() {
            on_event(event);
        }
    }

    /// Runs the flight against the wall clock, `speed` times faster than real time
    /// (1.0 = real time). Blocks the calling thread until the flight is over.
    pub fn run_paced(&mut self, speed: f64, mut on_event: impl FnMut(SensorEvent)) {
        let pacer = Pacer::start(self.clock.clone(), speed);
        loop {
            pacer.update();
            // Check before draining, so the samples of the final instant are not lost.
            let finished = self.finished();
            while let Some(event) = self.sensors.next_event() {
                on_event(event);
            }
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
