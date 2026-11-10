//! The simulation clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Simulated time in seconds since the start of the flight, shared by all simulated
/// sensors.
///
/// Cloning a `SimClock` does not create a second clock: all clones point to the same
/// value (`Arc` = shared ownership). The value lives in an atomic, so it can be read and
/// written from several threads without a lock. There is no atomic `f64` in the standard
/// library, so the bit pattern of the `f64` is stored in an `AtomicU64`.
#[derive(Debug, Clone, Default)]
pub struct SimClock {
    seconds_bits: Arc<AtomicU64>,
}

impl SimClock {
    /// A new clock at time zero. (The bit pattern of 0.0 is all zeros.)
    pub fn new() -> Self {
        Self::default()
    }

    pub fn now(&self) -> f64 {
        f64::from_bits(self.seconds_bits.load(Ordering::Relaxed))
    }

    pub fn set(&self, seconds: f64) {
        self.seconds_bits
            .store(seconds.to_bits(), Ordering::Relaxed);
    }

    pub fn advance(&self, seconds: f64) {
        self.set(self.now() + seconds);
    }
}

/// Moves a [`SimClock`] along with the wall clock, optionally faster than real time.
#[derive(Debug)]
pub struct Pacer {
    clock: SimClock,
    speed: f64,
    started: Instant,
    offset_s: f64,
}

impl Pacer {
    /// Starts pacing from the clock's current time. `speed` 1.0 is real time, 10.0 is
    /// ten times faster.
    pub fn start(clock: SimClock, speed: f64) -> Self {
        let offset_s = clock.now();
        Pacer {
            clock,
            speed,
            started: Instant::now(),
            offset_s,
        }
    }

    /// Sets the clock to where it should be right now and returns that time.
    pub fn update(&self) -> f64 {
        let now = self.offset_s + self.started.elapsed().as_secs_f64() * self.speed;
        self.clock.set(now);
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_same_time() {
        let clock = SimClock::new();
        let other = clock.clone();
        clock.set(12.5);
        other.advance(0.5);
        assert_eq!(clock.now(), 13.0);
    }

    #[test]
    fn pacer_runs_faster_than_real_time() {
        let clock = SimClock::new();
        clock.set(100.0);
        let pacer = Pacer::start(clock.clone(), 1000.0);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let now = pacer.update();
        assert_eq!(clock.now(), now);
        // 20 ms at 1000x are 20 simulated seconds; allow generous scheduling slack.
        assert!(now >= 120.0, "{now}");
        assert!(now < 1100.0, "{now}");
    }
}
