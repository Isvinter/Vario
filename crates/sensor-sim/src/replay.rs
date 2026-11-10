//! Replaying a recorded flight from an IGC file.
//!
//! The GPS source hands out the fixes of the file unchanged at their original times.
//! Barometer and accelerometer need values *between* the fixes, so the altitude profile
//! is turned into a smooth curve through all fixes ([`IgcTrajectory`]).

use flight_core::geo::{bearing_deg, distance_m};
use flight_core::igc::IgcFile;
use flight_core::sensor::{GpsFix, GpsSource};
use flight_core::time::{SECONDS_PER_DAY, Timestamp, UtcDateTime};
use thiserror::Error;

use crate::clock::SimClock;
use crate::trajectory::{Trajectory, TruthState};

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ReplayError {
    #[error("the IGC file has no valid date header")]
    MissingDate,
    #[error("the IGC file needs at least two fixes with increasing time")]
    TooFewFixes,
}

/// A recorded flight prepared for replay.
#[derive(Debug, Clone)]
pub struct IgcReplay {
    fixes: Vec<GpsFix>,
    trajectory: IgcTrajectory,
}

impl IgcReplay {
    pub fn new(file: &IgcFile) -> Result<Self, ReplayError> {
        let date = file.date().ok_or(ReplayError::MissingDate)?;

        let mut fixes: Vec<GpsFix> = Vec::new();
        let mut pressure_altitudes: Vec<f64> = Vec::new();
        let mut first_second = 0;
        let mut previous_second_of_day = 0;
        let mut day = 0;

        for igc_fix in file.fixes() {
            let second_of_day = igc_fix.time.seconds_since_midnight();
            if fixes.is_empty() {
                first_second = second_of_day;
            } else if second_of_day < previous_second_of_day {
                // The time of day jumped backwards: the flight crossed midnight UTC.
                day += 1;
            } else if second_of_day == previous_second_of_day {
                // Duplicate timestamp: nothing sensible to replay.
                continue;
            }
            previous_second_of_day = second_of_day;

            let elapsed_s =
                f64::from(day * SECONDS_PER_DAY + second_of_day) - f64::from(first_second);
            let (ground_speed_mps, course_deg) = match fixes.last() {
                Some(previous) => {
                    let (lat0, lon0) = (previous.latitude_deg, previous.longitude_deg);
                    let (lat1, lon1) = (igc_fix.latitude_deg, igc_fix.longitude_deg);
                    let dt = elapsed_s - previous.time.as_secs();
                    (
                        Some(distance_m(lat0, lon0, lat1, lon1) / dt),
                        Some(bearing_deg(lat0, lon0, lat1, lon1)),
                    )
                }
                None => (None, None),
            };

            fixes.push(GpsFix {
                time: Timestamp::from_secs(elapsed_s),
                utc: UtcDateTime::new(date.plus_days(i64::from(day)), igc_fix.time),
                latitude_deg: igc_fix.latitude_deg,
                longitude_deg: igc_fix.longitude_deg,
                altitude_m: f64::from(igc_fix.gps_altitude_m),
                fix_3d: igc_fix.fix_3d,
                ground_speed_mps,
                course_deg,
            });
            pressure_altitudes.push(f64::from(igc_fix.pressure_altitude_m));
        }

        if fixes.len() < 2 {
            return Err(ReplayError::TooFewFixes);
        }

        // Recorders without a pressure sensor (many phone apps) write 0 everywhere. In
        // that case the GPS altitude is the only altitude there is.
        let has_pressure_altitude = pressure_altitudes.iter().any(|&altitude| altitude != 0.0);
        let nodes = fixes
            .iter()
            .zip(&pressure_altitudes)
            .map(|(fix, &pressure_altitude)| Node {
                time_s: fix.time.as_secs(),
                latitude_deg: fix.latitude_deg,
                longitude_deg: fix.longitude_deg,
                altitude_m: if has_pressure_altitude {
                    pressure_altitude
                } else {
                    fix.altitude_m
                },
            })
            .collect();

        Ok(IgcReplay {
            fixes,
            trajectory: IgcTrajectory::new(nodes),
        })
    }

    /// A GPS source that replays the original fixes.
    pub fn gps(&self, clock: SimClock) -> ReplayGps {
        ReplayGps {
            fixes: self.fixes.clone(),
            next: 0,
            clock,
        }
    }

    /// The interpolated flight path. Consumes the replay (`self` by value) because
    /// nothing else is needed from it afterwards.
    pub fn into_trajectory(self) -> IgcTrajectory {
        self.trajectory
    }
}

/// GPS source handing out recorded fixes at their original times.
pub struct ReplayGps {
    fixes: Vec<GpsFix>,
    next: usize,
    clock: SimClock,
}

impl GpsSource for ReplayGps {
    fn poll_fix(&mut self) -> Option<GpsFix> {
        let fix = *self.fixes.get(self.next)?;
        if fix.time.as_secs() > self.clock.now() {
            return None;
        }
        self.next += 1;
        Some(fix)
    }
}

#[derive(Debug, Clone, Copy)]
struct Node {
    time_s: f64,
    latitude_deg: f64,
    longitude_deg: f64,
    altitude_m: f64,
}

/// Flight path interpolated between recorded fixes.
///
/// Position is interpolated linearly. Altitude uses a cubic Hermite spline: a smooth
/// curve that passes exactly through every fix and has a continuous climb rate, so the
/// derived vertical acceleration stays finite. Between fixes the curve is a plausible
/// guess, not a measurement.
#[derive(Debug, Clone)]
pub struct IgcTrajectory {
    nodes: Vec<Node>,
    /// Climb rate at each node, estimated from its neighbours.
    climb_rates: Vec<f64>,
}

impl IgcTrajectory {
    fn new(nodes: Vec<Node>) -> Self {
        let slope = |a: &Node, b: &Node| (b.altitude_m - a.altitude_m) / (b.time_s - a.time_s);
        let last = nodes.len() - 1;
        let climb_rates = (0..nodes.len())
            .map(|i| slope(&nodes[i.saturating_sub(1)], &nodes[(i + 1).min(last)]))
            .collect();
        IgcTrajectory { nodes, climb_rates }
    }
}

impl Trajectory for IgcTrajectory {
    fn duration_s(&self) -> f64 {
        self.nodes[self.nodes.len() - 1].time_s
    }

    fn state_at(&self, t: f64) -> TruthState {
        let t = t.clamp(0.0, self.duration_s());
        // Index of the interval [i, i + 1] containing `t`.
        let i = self
            .nodes
            .partition_point(|node| node.time_s <= t)
            .saturating_sub(1)
            .min(self.nodes.len() - 2);
        let (a, b) = (&self.nodes[i], &self.nodes[i + 1]);
        let (rate_a, rate_b) = (self.climb_rates[i], self.climb_rates[i + 1]);

        let h = b.time_s - a.time_s;
        let u = (t - a.time_s) / h;
        let (u2, u3) = (u * u, u * u * u);

        // Cubic Hermite basis functions and their first and second derivatives. At
        // u = 0 the altitude is exactly a.altitude_m, at u = 1 exactly b.altitude_m.
        let altitude_m = (2.0 * u3 - 3.0 * u2 + 1.0) * a.altitude_m
            + (u3 - 2.0 * u2 + u) * h * rate_a
            + (-2.0 * u3 + 3.0 * u2) * b.altitude_m
            + (u3 - u2) * h * rate_b;
        let vertical_speed_mps = ((6.0 * u2 - 6.0 * u) * a.altitude_m
            + (3.0 * u2 - 4.0 * u + 1.0) * h * rate_a
            + (-6.0 * u2 + 6.0 * u) * b.altitude_m
            + (3.0 * u2 - 2.0 * u) * h * rate_b)
            / h;
        let vertical_accel_mps2 = ((12.0 * u - 6.0) * a.altitude_m
            + (6.0 * u - 4.0) * h * rate_a
            + (-12.0 * u + 6.0) * b.altitude_m
            + (6.0 * u - 2.0) * h * rate_b)
            / (h * h);

        TruthState {
            latitude_deg: a.latitude_deg + (b.latitude_deg - a.latitude_deg) * u,
            longitude_deg: a.longitude_deg + (b.longitude_deg - a.longitude_deg) * u,
            altitude_m,
            vertical_speed_mps,
            vertical_accel_mps2,
            ground_speed_mps: distance_m(
                a.latitude_deg,
                a.longitude_deg,
                b.latitude_deg,
                b.longitude_deg,
            ) / h,
            course_deg: bearing_deg(
                a.latitude_deg,
                a.longitude_deg,
                b.latitude_deg,
                b.longitude_deg,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flight_core::time::{Date, TimeOfDay};

    fn parse(text: &str) -> IgcFile {
        IgcFile::parse(text).unwrap()
    }

    #[test]
    fn needs_a_date_and_two_fixes() {
        let no_date =
            parse("B1000004700000N01100000EA0100001000\r\nB1000014700000N01100000EA0100001000\r\n");
        assert_eq!(
            IgcReplay::new(&no_date).unwrap_err(),
            ReplayError::MissingDate
        );

        let one_fix = parse("HFDTE280726\r\nB1000004700000N01100000EA0100001000\r\n");
        assert_eq!(
            IgcReplay::new(&one_fix).unwrap_err(),
            ReplayError::TooFewFixes
        );
    }

    #[test]
    fn fixes_keep_their_values_and_cross_midnight() {
        let file = parse(
            "HFDTE280726\r\n\
             B2359584700000N01100000EA0100001010\r\n\
             B2359594700000N01100000EA0100101011\r\n\
             B2359594700000N01100000EA0100101011\r\n\
             B0000024700000N01100000EV0100401014\r\n",
        );
        let replay = IgcReplay::new(&file).unwrap();
        let clock = SimClock::new();
        clock.set(1000.0);
        let mut gps = replay.gps(clock);
        let fixes: Vec<GpsFix> = std::iter::from_fn(|| gps.poll_fix()).collect();

        // The duplicate 23:59:59 is dropped.
        let times: Vec<f64> = fixes.iter().map(|f| f.time.as_secs()).collect();
        assert_eq!(times, [0.0, 1.0, 4.0]);
        assert_eq!(fixes[2].utc.date, Date::new(2026, 7, 29).unwrap());
        assert_eq!(fixes[2].utc.time, TimeOfDay::new(0, 0, 2).unwrap());
        assert_eq!(fixes[2].altitude_m, 1014.0);
        assert!(!fixes[2].fix_3d);
        assert_eq!(replay.into_trajectory().duration_s(), 4.0);
    }

    #[test]
    fn speed_and_course_come_from_consecutive_fixes() {
        // 0.010 arc minutes of latitude per second are 18.5 m/s due north.
        let file = parse(
            "HFDTE280726\r\n\
             B1000004700000N01100000EA0100001000\r\n\
             B1000014700010N01100000EA0100001000\r\n",
        );
        let replay = IgcReplay::new(&file).unwrap();
        let clock = SimClock::new();
        clock.set(10.0);
        let mut gps = replay.gps(clock);
        let first = gps.poll_fix().unwrap();
        let second = gps.poll_fix().unwrap();

        assert_eq!(first.ground_speed_mps, None);
        assert!((second.ground_speed_mps.unwrap() - 18.53).abs() < 0.01);
        assert!(second.course_deg.unwrap().abs() < 1e-6);
    }

    #[test]
    fn altitude_curve_passes_through_fixes_with_plausible_climb_rate() {
        // Constant 2 m/s climb, fixes every 2 s.
        let file = parse(
            "HFDTE280726\r\n\
             B1000004700000N01100000EA0100001000\r\n\
             B1000024700000N01100000EA0100401000\r\n\
             B1000044700000N01100000EA0100801000\r\n\
             B1000064700000N01100000EA0101201000\r\n",
        );
        let trajectory = IgcReplay::new(&file).unwrap().into_trajectory();

        assert_eq!(trajectory.state_at(2.0).altitude_m, 1004.0);
        assert_eq!(trajectory.state_at(6.0).altitude_m, 1012.0);
        for t in [0.0, 0.7, 3.0, 5.9] {
            let state = trajectory.state_at(t);
            assert!((state.altitude_m - (1000.0 + 2.0 * t)).abs() < 1e-9, "{t}");
            assert!((state.vertical_speed_mps - 2.0).abs() < 1e-9, "{t}");
            assert!(state.vertical_accel_mps2.abs() < 1e-9, "{t}");
        }
    }

    #[test]
    fn falls_back_to_gps_altitude_without_pressure_altitude() {
        let file = parse(
            "HFDTE280726\r\n\
             B1000004700000N01100000EA0000001000\r\n\
             B1000014700000N01100000EA0000001003\r\n",
        );
        let trajectory = IgcReplay::new(&file).unwrap().into_trajectory();
        assert_eq!(trajectory.state_at(1.0).altitude_m, 1003.0);
    }
}
