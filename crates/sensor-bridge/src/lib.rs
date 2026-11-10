//! Entry point for sensor data that native platform code pushes into Rust.
//!
//! On iOS, sensors deliver their values through callbacks in Swift. Swift calls the
//! `vario_push_*` functions below directly (plain C calling convention), which put the
//! sample into a channel. The Rust side takes the samples out of the channel on its own
//! thread. No JavaScript and no web view are involved, so this keeps working while the
//! screen is locked.
//!
//! A channel is a thread-safe queue with a sending and a receiving end. The receiver can
//! sleep until something arrives, which costs no CPU time (important on battery).
//!
//! The simulator on the desktop feeds the same channel through [`push`], so everything
//! behind the channel is identical on all platforms.

// Exporting a function under a fixed C name is "unsafe" in Rust's sense because two
// functions with the same name would silently clash at link time. The names here carry
// a `vario_` prefix to rule that out.
#![allow(unsafe_code)]

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};

use flight_core::sensor::{AccelSample, GpsFix, PressureSample, SensorEvent};
use flight_core::time::{Timestamp, UtcDateTime};

/// The sending end of the current channel, if a receiver is connected.
static SENDER: Mutex<Option<Sender<SensorEvent>>> = Mutex::new(None);

/// Opens the channel and returns its receiving end. Samples pushed before this call, or
/// after the receiver is dropped, are discarded. Calling it again replaces the channel.
pub fn connect() -> Receiver<SensorEvent> {
    let (sender, receiver) = mpsc::channel();
    *lock_sender() = Some(sender);
    receiver
}

/// Closes the channel; the receiver then sees it as disconnected.
pub fn disconnect() {
    *lock_sender() = None;
}

/// Puts a sample into the channel.
pub fn push(event: SensorEvent) {
    let mut sender = lock_sender();
    // `if let ... && ...` checks both: a channel is open, and sending into it failed.
    if let Some(open) = sender.as_ref()
        && open.send(event).is_err()
    {
        // The receiver is gone: stop sending until someone connects again.
        *sender = None;
    }
}

fn lock_sender() -> std::sync::MutexGuard<'static, Option<Sender<SensorEvent>>> {
    // A mutex is "poisoned" if a thread panicked while holding it. The value inside is
    // still fine for us, and panicking in a function called from Swift would abort the
    // whole app, so carry on.
    SENDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Vertical acceleration sample. `time_s`: seconds on the platform's monotonic clock.
/// `vertical_mps2`: earth-frame vertical acceleration, upwards positive, without gravity.
#[unsafe(no_mangle)]
pub extern "C" fn vario_push_accel(time_s: f64, vertical_mps2: f64) {
    if time_s.is_finite() && vertical_mps2.is_finite() {
        push(SensorEvent::Accel(AccelSample {
            time: Timestamp::from_secs(time_s),
            vertical_mps2,
        }));
    }
}

/// Static pressure sample in pascal.
#[unsafe(no_mangle)]
pub extern "C" fn vario_push_pressure(time_s: f64, pressure_pa: f64) {
    if time_s.is_finite() && pressure_pa.is_finite() && pressure_pa > 0.0 {
        push(SensorEvent::Pressure(PressureSample {
            time: Timestamp::from_secs(time_s),
            pressure_pa,
        }));
    }
}

/// GPS fix. Follows the CoreLocation conventions: a negative `speed_mps`, `course_deg`
/// or `vertical_accuracy_m` means that the value is not available.
#[unsafe(no_mangle)]
pub extern "C" fn vario_push_gps(
    time_s: f64,
    unix_time_s: f64,
    latitude_deg: f64,
    longitude_deg: f64,
    altitude_m: f64,
    speed_mps: f64,
    course_deg: f64,
    vertical_accuracy_m: f64,
) {
    if let Some(fix) = gps_fix(
        time_s,
        unix_time_s,
        latitude_deg,
        longitude_deg,
        altitude_m,
        speed_mps,
        course_deg,
        vertical_accuracy_m,
    ) {
        push(SensorEvent::Gps(fix));
    }
}

/// Validates and converts raw GPS values. Returns `None` for unusable input.
// The argument list mirrors the C function above one to one.
#[allow(clippy::too_many_arguments)]
fn gps_fix(
    time_s: f64,
    unix_time_s: f64,
    latitude_deg: f64,
    longitude_deg: f64,
    altitude_m: f64,
    speed_mps: f64,
    course_deg: f64,
    vertical_accuracy_m: f64,
) -> Option<GpsFix> {
    let usable = time_s.is_finite()
        && unix_time_s.is_finite()
        && unix_time_s.abs() < 1e11
        && (-90.0..=90.0).contains(&latitude_deg)
        && (-180.0..=180.0).contains(&longitude_deg);
    if !usable {
        return None;
    }
    let fix_3d = altitude_m.is_finite() && vertical_accuracy_m >= 0.0;
    // `then_some` turns a bool into an Option: Some(value) if true, None if false.
    let non_negative = |value: f64| (value.is_finite() && value >= 0.0).then_some(value);

    Some(GpsFix {
        time: Timestamp::from_secs(time_s),
        utc: UtcDateTime::from_unix_seconds(unix_time_s.floor() as i64),
        latitude_deg,
        longitude_deg,
        altitude_m: if fix_3d { altitude_m } else { 0.0 },
        fix_3d,
        ground_speed_mps: non_negative(speed_mps),
        course_deg: non_negative(course_deg),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flight_core::time::{Date, TimeOfDay};
    use std::sync::mpsc::TryRecvError;

    // The channel is global, so everything that uses it lives in one test; separate
    // tests run in parallel and would disturb each other.
    #[test]
    fn pushed_samples_arrive_in_order_and_invalid_ones_are_dropped() {
        vario_push_accel(0.5, 1.0); // nobody connected yet: discarded
        let receiver = connect();

        vario_push_accel(1.0, 0.25);
        vario_push_accel(f64::NAN, 0.25);
        vario_push_pressure(1.5, 95_000.0);
        vario_push_pressure(1.6, -1.0);
        vario_push_gps(2.0, 1_794_325_600.9, 47.5, 11.25, 1500.0, 9.0, 270.0, 4.0);
        vario_push_gps(2.1, 1_794_325_601.0, 91.0, 11.25, 1500.0, 9.0, 270.0, 4.0);

        let events: Vec<SensorEvent> = receiver.try_iter().collect();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            SensorEvent::Accel(AccelSample {
                time: Timestamp::from_secs(1.0),
                vertical_mps2: 0.25
            })
        );
        assert!(matches!(events[1], SensorEvent::Pressure(p) if p.pressure_pa == 95_000.0));
        assert!(matches!(events[2], SensorEvent::Gps(_)));

        disconnect();
        vario_push_accel(3.0, 0.0);
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));

        // Dropping the receiver must not break later pushes or a new connection.
        drop(connect());
        vario_push_accel(4.0, 0.0);
        let receiver = connect();
        vario_push_accel(5.0, 0.0);
        assert_eq!(receiver.try_iter().count(), 1);
        disconnect();
    }

    #[test]
    fn gps_values_follow_core_location_conventions() {
        let fix = gps_fix(2.0, 1_794_325_600.9, 47.5, -11.25, 1500.0, 9.0, 270.0, 4.0).unwrap();
        assert_eq!(fix.utc.date, Date::new(2026, 11, 10).unwrap());
        assert_eq!(fix.utc.time, TimeOfDay::new(15, 46, 40).unwrap());
        assert_eq!(fix.longitude_deg, -11.25);
        assert!(fix.fix_3d);
        assert_eq!(fix.ground_speed_mps, Some(9.0));
        assert_eq!(fix.course_deg, Some(270.0));

        // Negative accuracy, speed and course mean "not available".
        let poor = gps_fix(2.0, 1_794_325_600.0, 47.5, 11.25, 1500.0, -1.0, -1.0, -1.0).unwrap();
        assert!(!poor.fix_3d);
        assert_eq!(poor.altitude_m, 0.0);
        assert_eq!(poor.ground_speed_mps, None);
        assert_eq!(poor.course_deg, None);

        assert!(gps_fix(2.0, f64::INFINITY, 47.5, 11.25, 0.0, 0.0, 0.0, 1.0).is_none());
        assert!(gps_fix(2.0, 0.0, 47.5, 181.0, 0.0, 0.0, 0.0, 1.0).is_none());
    }
}
