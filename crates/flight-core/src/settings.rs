//! User settings.

use thiserror::Error;

/// Everything the user can adjust about the vario.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VarioSettings {
    /// Climb tone starts above this climb rate in m/s.
    pub climb_threshold_mps: f64,
    /// Sink tone starts below this (negative) climb rate in m/s.
    pub sink_threshold_mps: f64,
    /// Tone volume from 0.0 (silent) to 1.0.
    pub volume: f64,
    /// Averaging time of the displayed and audible climb rate in seconds. 0 turns
    /// averaging off; larger values are calmer but react later.
    pub damping_s: f64,
}

impl Default for VarioSettings {
    fn default() -> Self {
        VarioSettings {
            climb_threshold_mps: 0.2,
            sink_threshold_mps: -2.5,
            volume: 0.8,
            damping_s: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum SettingsError {
    #[error("the sink threshold must be below the climb threshold")]
    ThresholdsOverlap,
    #[error("the volume must be between 0 and 1")]
    VolumeOutOfRange,
    #[error("the damping must be between 0 and 10 seconds")]
    DampingOutOfRange,
    #[error("settings must be finite numbers")]
    NotFinite,
}

impl VarioSettings {
    /// Checks that the values make sense together.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let values = [
            self.climb_threshold_mps,
            self.sink_threshold_mps,
            self.volume,
            self.damping_s,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            return Err(SettingsError::NotFinite);
        }
        if self.sink_threshold_mps >= self.climb_threshold_mps {
            return Err(SettingsError::ThresholdsOverlap);
        }
        if !(0.0..=1.0).contains(&self.volume) {
            return Err(SettingsError::VolumeOutOfRange);
        }
        if !(0.0..=10.0).contains(&self.damping_s) {
            return Err(SettingsError::DampingOutOfRange);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(VarioSettings::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_inconsistent_values() {
        let defaults = VarioSettings::default();
        let overlapping = VarioSettings {
            sink_threshold_mps: 0.5,
            ..defaults
        };
        assert_eq!(
            overlapping.validate(),
            Err(SettingsError::ThresholdsOverlap)
        );
        let loud = VarioSettings {
            volume: 1.5,
            ..defaults
        };
        assert_eq!(loud.validate(), Err(SettingsError::VolumeOutOfRange));
        let slow = VarioSettings {
            damping_s: -1.0,
            ..defaults
        };
        assert_eq!(slow.validate(), Err(SettingsError::DampingOutOfRange));
        let nan = VarioSettings {
            volume: f64::NAN,
            ..defaults
        };
        assert_eq!(nan.validate(), Err(SettingsError::NotFinite));
    }
}
