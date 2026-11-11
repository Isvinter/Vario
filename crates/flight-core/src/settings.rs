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
    #[error("line {line} of the settings file is not `name = number`")]
    Malformed { line: usize },
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

// The settings file is plain text, one `name = value` per line. That keeps this crate
// free of a serialization dependency and the file readable and editable by hand.
const KEY_CLIMB: &str = "climb_threshold_mps";
const KEY_SINK: &str = "sink_threshold_mps";
const KEY_VOLUME: &str = "volume";
const KEY_DAMPING: &str = "damping_s";

impl VarioSettings {
    /// The settings as the text of a settings file. Read it back with
    /// [`VarioSettings::from_text`].
    pub fn to_text(&self) -> String {
        format!(
            "# Vario settings\n\
             {KEY_CLIMB} = {}\n{KEY_SINK} = {}\n{KEY_VOLUME} = {}\n{KEY_DAMPING} = {}\n",
            self.climb_threshold_mps, self.sink_threshold_mps, self.volume, self.damping_s
        )
    }

    /// Parses the text of a settings file.
    ///
    /// Values missing from the text keep their defaults and unknown names are ignored,
    /// so files written by older or newer versions of the app still load. The result
    /// is validated.
    pub fn from_text(text: &str) -> Result<Self, SettingsError> {
        let mut settings = VarioSettings::default();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let malformed = SettingsError::Malformed { line: index + 1 };
            let (key, value) = line.split_once('=').ok_or(malformed)?;
            let value: f64 = value.trim().parse().map_err(|_| malformed)?;
            match key.trim() {
                KEY_CLIMB => settings.climb_threshold_mps = value,
                KEY_SINK => settings.sink_threshold_mps = value,
                KEY_VOLUME => settings.volume = value,
                KEY_DAMPING => settings.damping_s = value,
                _ => {}
            }
        }
        settings.validate()?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip_keeps_all_values() {
        let settings = VarioSettings {
            climb_threshold_mps: 0.35,
            sink_threshold_mps: -3.1,
            volume: 0.45,
            damping_s: 1.25,
        };
        assert_eq!(VarioSettings::from_text(&settings.to_text()), Ok(settings));
    }

    #[test]
    fn missing_and_unknown_entries_fall_back_to_defaults() {
        let parsed = VarioSettings::from_text("volume = 0.3\nfuture_option = 7\n\n# note\n");
        assert_eq!(
            parsed,
            Ok(VarioSettings {
                volume: 0.3,
                ..VarioSettings::default()
            })
        );
        assert_eq!(VarioSettings::from_text(""), Ok(VarioSettings::default()));
    }

    #[test]
    fn broken_text_is_an_error() {
        assert_eq!(
            VarioSettings::from_text("volume = 0.3\nnonsense\n"),
            Err(SettingsError::Malformed { line: 2 })
        );
        assert_eq!(
            VarioSettings::from_text("volume = loud"),
            Err(SettingsError::Malformed { line: 1 })
        );
        assert_eq!(
            VarioSettings::from_text("volume = 2"),
            Err(SettingsError::VolumeOutOfRange)
        );
    }

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
