//! The engine: takes sensor samples from a channel and runs them through the vario
//! filter, the tone control and the logs, on its own background thread.
//!
//! It knows nothing about the user interface. The UI only asks for a [`Status`] snapshot
//! now and then, so the engine keeps working when the UI is suspended (locked screen).
//!
//! * [`processor`] holds the actual logic, without threads, so it can be tested directly.
//! * [`Engine`] wraps it in a thread.

pub mod processor;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flight_core::sensor::SensorEvent;
use flight_core::settings::VarioSettings;
use flight_core::time::UtcDateTime;
use vario_audio::voice::ToneControl;

pub use crate::processor::{FixInfo, Processor, SensorStats, Status, TrackPoint};

/// How often a summary line is written and the sensor rates are refreshed.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(10);
/// How long the engine thread sleeps at most while no samples arrive.
const IDLE_WAKEUP: Duration = Duration::from_millis(250);

/// State shared between the engine thread and its owner.
#[derive(Default)]
struct Shared {
    status: Mutex<Status>,
    stop: AtomicBool,
    /// Climb rate that overrides the filter output, for testing the tone.
    test_climb_mps: Mutex<Option<f64>>,
    /// Settings changed by the user that the engine thread has not applied yet.
    pending_settings: Mutex<Option<VarioSettings>>,
    /// The flown track since the engine started, one point per second.
    track: Mutex<Vec<TrackPoint>>,
}

/// A running engine. Stops when [`Engine::stop`] is called or the value is dropped.
pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Starts the engine thread.
    ///
    /// `log_dir`: if given, a raw sensor log and a summary log are written there.
    pub fn start(
        events: Receiver<SensorEvent>,
        control: ToneControl,
        settings: &VarioSettings,
        log_dir: Option<PathBuf>,
    ) -> io::Result<Engine> {
        let mut processor = Processor::new(control, settings);
        if let Some(dir) = &log_dir {
            processor.open_logs(dir, &timestamp_for_file_name())?;
        }

        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("vario-engine".to_owned())
            .spawn(move || run(processor, events, &thread_shared))?;

        Ok(Engine {
            shared,
            thread: Some(thread),
        })
    }

    /// A snapshot of the current state.
    pub fn status(&self) -> Status {
        lock(&self.shared.status).clone()
    }

    /// Makes the tone play as if climbing at this rate, regardless of the sensors.
    /// `None` returns to normal operation.
    pub fn set_test_climb(&self, climb_rate_mps: Option<f64>) {
        *lock(&self.shared.test_climb_mps) = climb_rate_mps;
    }

    /// The track points from index `from` on. The UI remembers how many points it
    /// already has and asks only for the new ones, so the amount of data per request
    /// stays small however long the flight is.
    pub fn track_since(&self, from: usize) -> Vec<TrackPoint> {
        lock(&self.shared.track)
            .get(from..)
            .map(<[TrackPoint]>::to_vec)
            .unwrap_or_default()
    }

    /// Changes the settings while running. The engine thread picks them up within a
    /// fraction of a second.
    pub fn set_settings(&self, settings: VarioSettings) {
        *lock(&self.shared.pending_settings) = Some(settings);
    }

    /// Stops the thread and waits until it has written its logs.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // An error here means the thread panicked; there is nothing left to do.
            let _ = thread.join();
        }
    }
}

// `Drop` runs when a value goes out of scope. This guarantees the thread never
// outlives the `Engine`, even if `stop` was not called.
impl Drop for Engine {
    fn drop(&mut self) {
        self.shut_down();
    }
}

fn run(mut processor: Processor, events: Receiver<SensorEvent>, shared: &Shared) {
    let started = Instant::now();
    let mut last_summary = Instant::now();

    while !shared.stop.load(Ordering::Relaxed) {
        // `take` empties the slot, so each change is applied exactly once.
        if let Some(settings) = lock(&shared.pending_settings).take() {
            processor.set_settings(&settings);
        }
        // Sleep until a sample arrives, but wake up regularly to notice `stop`.
        match events.recv_timeout(IDLE_WAKEUP) {
            Ok(event) => {
                processor.set_test_climb(*lock(&shared.test_climb_mps));
                processor.handle(&event);
                // Handle everything else that is already waiting before publishing.
                for event in events.try_iter() {
                    processor.handle(&event);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                processor.set_test_climb(*lock(&shared.test_climb_mps));
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if last_summary.elapsed() >= SUMMARY_INTERVAL {
            processor.summarize(last_summary.elapsed().as_secs_f64(), &utc_clock_label());
            last_summary = Instant::now();
        }
        let new_points = processor.take_track_points();
        if !new_points.is_empty() {
            lock(&shared.track).extend(new_points);
        }
        *lock(&shared.status) = processor.status(started.elapsed().as_secs_f64(), true);
    }

    processor.summarize(last_summary.elapsed().as_secs_f64(), &utc_clock_label());
    processor.silence();
    *lock(&shared.status) = processor.status(started.elapsed().as_secs_f64(), false);
}

/// Locks a mutex, ignoring "poisoning" (a panic in another thread while it held the
/// lock): the status data is still usable.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn utc_now() -> UtcDateTime {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    UtcDateTime::from_unix_seconds(unix)
}

fn utc_clock_label() -> String {
    let time = utc_now().time;
    format!(
        "{:02}:{:02}:{:02}",
        time.hour(),
        time.minute(),
        time.second()
    )
}

fn timestamp_for_file_name() -> String {
    let now = utc_now();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.date.year(),
        now.date.month(),
        now.date.day(),
        now.time.hour(),
        now.time.minute(),
        now.time.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flight_core::sensor::{AccelSample, PressureSample};
    use flight_core::time::Timestamp;
    use std::sync::mpsc;

    fn wait_for(engine: &Engine, condition: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = engine.status();
            if condition(&status) {
                return status;
            }
            assert!(Instant::now() < deadline, "timed out, status {status:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn engine_processes_events_from_another_thread_and_stops() {
        let (sender, receiver) = mpsc::channel();
        let control = ToneControl::new(&VarioSettings::default());
        let engine = Engine::start(receiver, control, &VarioSettings::default(), None).unwrap();

        sender
            .send(SensorEvent::Pressure(PressureSample {
                time: Timestamp::from_secs(1.0),
                pressure_pa: 90_000.0,
            }))
            .unwrap();
        sender
            .send(SensorEvent::Accel(AccelSample {
                time: Timestamp::from_secs(1.02),
                vertical_mps2: 0.0,
            }))
            .unwrap();

        let status = wait_for(&engine, |status| status.accel.total == 1);
        assert!(status.running);
        assert_eq!(status.pressure.total, 1);
        assert!((status.altitude_m.unwrap() - 988.5).abs() < 1.0);

        engine.set_test_climb(Some(2.0));
        let status = wait_for(&engine, |status| status.test_tone);
        assert!(status.test_tone);

        engine.stop();
    }

    #[test]
    fn engine_ends_when_the_channel_closes() {
        let (sender, receiver) = mpsc::channel::<SensorEvent>();
        let control = ToneControl::new(&VarioSettings::default());
        let engine = Engine::start(receiver, control, &VarioSettings::default(), None).unwrap();
        drop(sender);
        let status = wait_for(&engine, |status| !status.running && status.uptime_s > 0.0);
        assert!(!status.running);
    }
}
