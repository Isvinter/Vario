//! Mapping from climb rate to tone.

/// What the vario should sound like at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Silence,
    /// Repeating beeps: climbing.
    Beep {
        frequency_hz: f32,
        /// Duration of one beep plus the pause after it, in seconds.
        cycle_s: f32,
        /// Fraction of the cycle during which the tone sounds.
        duty: f32,
    },
    /// Uninterrupted tone: strong sink.
    Continuous {
        frequency_hz: f32,
    },
}

/// The two climb rates at which the tones start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Beeps above this climb rate in m/s.
    pub climb_mps: f32,
    /// Continuous tone below this climb rate in m/s.
    pub sink_mps: f32,
}

/// Climb rate (m/s) -> pitch (Hz) and beep cycle (s). Values in between are
/// interpolated; beyond the ends the end values are used.
const CLIMB_TABLE: [(f32, f32, f32); 7] = [
    (0.0, 600.0, 0.60),
    (1.0, 800.0, 0.42),
    (2.0, 1000.0, 0.32),
    (3.0, 1150.0, 0.26),
    (5.0, 1400.0, 0.19),
    (8.0, 1650.0, 0.14),
    (10.0, 1800.0, 0.12),
];

/// Climb rate (m/s) -> pitch (Hz) of the sink tone: the stronger the sink, the lower.
const SINK_TABLE: [(f32, f32); 4] = [(-10.0, 180.0), (-5.0, 240.0), (-2.5, 300.0), (-1.0, 350.0)];

const BEEP_DUTY: f32 = 0.5;

/// The tone for a climb rate. Pitch and beep rate depend only on the climb rate itself;
/// the thresholds merely decide whether anything is heard at all.
pub fn tone_for(climb_rate_mps: f32, thresholds: Thresholds) -> Tone {
    if climb_rate_mps.is_nan() {
        return Tone::Silence;
    }
    if climb_rate_mps >= thresholds.climb_mps {
        let frequency_hz = interpolate(climb_rate_mps, CLIMB_TABLE.map(|(x, hz, _)| (x, hz)));
        let cycle_s = interpolate(climb_rate_mps, CLIMB_TABLE.map(|(x, _, cycle)| (x, cycle)));
        Tone::Beep {
            frequency_hz,
            cycle_s,
            duty: BEEP_DUTY,
        }
    } else if climb_rate_mps <= thresholds.sink_mps {
        Tone::Continuous {
            frequency_hz: interpolate(climb_rate_mps, SINK_TABLE),
        }
    } else {
        Tone::Silence
    }
}

/// Piecewise linear interpolation in a table sorted by `x`.
///
/// `const N: usize` is a "const generic": the function works for arrays of any length,
/// and the length is known at compile time, so no heap allocation is needed.
fn interpolate<const N: usize>(x: f32, table: [(f32, f32); N]) -> f32 {
    let (first_x, first_y) = table[0];
    if x <= first_x {
        return first_y;
    }
    for pair in table.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    table[N - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: Thresholds = Thresholds {
        climb_mps: 0.2,
        sink_mps: -2.5,
    };

    fn beep(climb: f32) -> (f32, f32) {
        match tone_for(climb, DEFAULT) {
            Tone::Beep {
                frequency_hz,
                cycle_s,
                ..
            } => (frequency_hz, cycle_s),
            other => panic!("expected a beep at {climb} m/s, got {other:?}"),
        }
    }

    #[test]
    fn silence_between_the_thresholds() {
        for climb in [-2.49, -1.0, 0.0, 0.19] {
            assert_eq!(tone_for(climb, DEFAULT), Tone::Silence, "{climb}");
        }
        assert_eq!(tone_for(f32::NAN, DEFAULT), Tone::Silence);
    }

    #[test]
    fn beeps_get_higher_and_faster_with_stronger_climb() {
        let mut previous = beep(0.2);
        for step in 1..=60 {
            let current = beep(0.2 + step as f32 * 0.2);
            assert!(current.0 >= previous.0, "pitch must not fall");
            assert!(current.1 <= previous.1, "beeps must not slow down");
            previous = current;
        }
        assert_eq!(beep(1.0), (800.0, 0.42));
        assert_eq!(beep(1.5), (900.0, 0.37));
        assert_eq!(beep(25.0), (1800.0, 0.12));
    }

    #[test]
    fn continuous_tone_gets_lower_with_stronger_sink() {
        let frequency = |climb| match tone_for(climb, DEFAULT) {
            Tone::Continuous { frequency_hz } => frequency_hz,
            other => panic!("expected a continuous tone, got {other:?}"),
        };
        assert_eq!(frequency(-2.5), 300.0);
        assert_eq!(frequency(-5.0), 240.0);
        assert_eq!(frequency(-7.5), 210.0);
        assert_eq!(frequency(-30.0), 180.0);
    }

    #[test]
    fn thresholds_move_the_onset_but_not_the_pitch() {
        let sensitive = Thresholds {
            climb_mps: -0.3,
            sink_mps: -1.5,
        };
        assert!(
            matches!(tone_for(-0.2, sensitive), Tone::Beep { frequency_hz, .. } if frequency_hz == 600.0)
        );
        assert!(matches!(tone_for(-1.5, sensitive), Tone::Continuous { .. }));

        let lazy = Thresholds {
            climb_mps: 1.0,
            sink_mps: -4.0,
        };
        assert_eq!(tone_for(0.9, lazy), Tone::Silence);
        assert_eq!(tone_for(-3.0, lazy), Tone::Silence);
        assert_eq!(tone_for(1.0, lazy), tone_for(1.0, DEFAULT));
    }
}
