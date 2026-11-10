//! Desktop only: feeds a simulated flight into the sensor channel, in place of the
//! native sensors.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flight_core::time::UtcDateTime;
use sensor_sim::clock::Pacer;
use sensor_sim::synthetic::{Segment, SyntheticFlight};
use sensor_sim::{SimConfig, Simulation};

pub struct Simulator {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Simulator {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("vario-simulator".to_owned())
            .spawn(move || run(&thread_stop))
            .ok();
        Simulator { stop, thread }
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(stop: &AtomicBool) {
    let unix_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let config = SimConfig {
        // Phone-like sensor noise.
        pressure_noise_pa: 3.0,
        accel_noise_mps2: 0.15,
        accel_bias_mps2: 0.15,
        ..SimConfig::default()
    };
    let mut simulation = Simulation::synthetic(
        long_flight(),
        UtcDateTime::from_unix_seconds(unix_now),
        &config,
    );

    let pacer = Pacer::start(simulation.clock.clone(), 1.0);
    while !stop.load(Ordering::Relaxed) && !simulation.finished() {
        pacer.update();
        while let Some(event) = simulation.sensors.next_event() {
            sensor_bridge::push(event);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// About 45 minutes of glides, thermals and sink, repeating every four and a half
/// minutes, with no net altitude change per round.
fn long_flight() -> SyntheticFlight {
    let mut builder = SyntheticFlight::builder();
    for _ in 0..10 {
        builder = builder
            .segment(Segment::straight(30.0, -1.2))
            .segment(Segment::circling(40.0, 1.0, 20.0))
            .segment(Segment::circling(60.0, 3.0, 18.0))
            .segment(Segment::straight(30.0, -1.2))
            .segment(Segment::straight(20.0, -3.5))
            .segment(Segment::straight(88.0, -1.0));
    }
    builder.build()
}
