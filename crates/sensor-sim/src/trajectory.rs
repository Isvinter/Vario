//! The "ground truth" of a simulated flight.

/// The exact state of the glider at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TruthState {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    /// Altitude in metres. The simulated barometer reports the standard-atmosphere
    /// pressure of this altitude.
    pub altitude_m: f64,
    /// Climb rate in m/s, upwards positive. This is what a perfect vario would show.
    pub vertical_speed_mps: f64,
    /// Vertical acceleration in m/s², upwards positive.
    pub vertical_accel_mps2: f64,
    pub ground_speed_mps: f64,
    /// Course over ground in degrees, 0 = north, 90 = east.
    pub course_deg: f64,
}

/// A flight path that can be evaluated at any time.
///
/// `Send + Sync` are marker traits promising that a trajectory may be moved to and shared
/// between threads. They are required here because the sensor sources share one
/// trajectory and may later run on a different thread than the UI.
pub trait Trajectory: Send + Sync {
    /// Length of the flight in seconds.
    fn duration_s(&self) -> f64;

    /// State at `t` seconds after the start. Times outside the flight are clamped to its
    /// start or end.
    fn state_at(&self, t: f64) -> TruthState;
}
