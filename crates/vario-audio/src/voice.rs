//! Connects the climb rate to the tone generator across threads.
//!
//! The vario filter runs on one thread, the sound card calls the audio callback on
//! another. The callback must never wait, so the two sides share a handful of atomics
//! instead of a lock: [`ToneControl`] writes them, [`VarioVoice`] reads them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use flight_core::settings::VarioSettings;

use crate::synth::Synth;
use crate::tone::{Thresholds, tone_for};

/// An `f32` that can be shared between threads without a lock. The standard library has
/// no atomic float, so the bits of the `f32` are stored in an `AtomicU32`.
#[derive(Debug)]
struct AtomicF32(AtomicU32);

impl AtomicF32 {
    fn new(value: f32) -> Self {
        AtomicF32(AtomicU32::new(value.to_bits()))
    }

    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct Shared {
    climb_rate_mps: AtomicF32,
    climb_threshold_mps: AtomicF32,
    sink_threshold_mps: AtomicF32,
    volume: AtomicF32,
}

/// Handle for telling the tone generator what to play. Cheap to clone; all clones
/// control the same voice.
#[derive(Debug, Clone)]
pub struct ToneControl {
    shared: Arc<Shared>,
}

impl ToneControl {
    /// Starts silent, at a climb rate of zero.
    pub fn new(settings: &VarioSettings) -> Self {
        let control = ToneControl {
            shared: Arc::new(Shared {
                climb_rate_mps: AtomicF32::new(0.0),
                climb_threshold_mps: AtomicF32::new(0.0),
                sink_threshold_mps: AtomicF32::new(0.0),
                volume: AtomicF32::new(0.0),
            }),
        };
        control.set_settings(settings);
        control
    }

    pub fn set_climb_rate(&self, climb_rate_mps: f64) {
        self.shared.climb_rate_mps.set(climb_rate_mps as f32);
    }

    /// Applies thresholds and volume. (Damping belongs to the vario filter.)
    pub fn set_settings(&self, settings: &VarioSettings) {
        self.shared
            .climb_threshold_mps
            .set(settings.climb_threshold_mps as f32);
        self.shared
            .sink_threshold_mps
            .set(settings.sink_threshold_mps as f32);
        self.shared.volume.set(settings.volume as f32);
    }
}

/// The sound-producing side: reads the control values and generates samples.
#[derive(Debug)]
pub struct VarioVoice {
    control: ToneControl,
    synth: Synth,
}

impl VarioVoice {
    pub fn new(control: ToneControl, sample_rate_hz: u32) -> Self {
        VarioVoice {
            control,
            synth: Synth::new(sample_rate_hz),
        }
    }

    /// Picks up the current climb rate and settings. Call once per audio buffer.
    pub fn sync(&mut self) {
        let shared = &self.control.shared;
        let thresholds = Thresholds {
            climb_mps: shared.climb_threshold_mps.get(),
            sink_mps: shared.sink_threshold_mps.get(),
        };
        self.synth
            .set_tone(tone_for(shared.climb_rate_mps.get(), thresholds));
        self.synth.set_volume(shared.volume.get());
    }

    pub fn next_sample(&mut self) -> f32 {
        self.synth.next_sample()
    }

    /// Syncs, then fills a mono buffer.
    pub fn render(&mut self, buffer: &mut [f32]) {
        self.sync();
        self.synth.render(buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loudness(voice: &mut VarioVoice) -> f32 {
        let mut buffer = vec![0.0; 24_000];
        voice.render(&mut buffer);
        buffer.iter().fold(0.0, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn climb_rate_and_settings_reach_the_synth() {
        let settings = VarioSettings::default();
        let control = ToneControl::new(&settings);
        let mut voice = VarioVoice::new(control.clone(), 48_000);

        assert_eq!(loudness(&mut voice), 0.0, "silent at zero climb");

        control.set_climb_rate(2.0);
        assert!(loudness(&mut voice) > 0.6, "beeps when climbing");

        control.set_climb_rate(-3.0);
        assert!(loudness(&mut voice) > 0.6, "sink tone");

        // Raising the sink threshold beyond the current sink rate silences it.
        control.set_settings(&VarioSettings {
            sink_threshold_mps: -4.0,
            ..settings
        });
        loudness(&mut voice); // let the fade-out finish
        assert_eq!(loudness(&mut voice), 0.0);

        control.set_climb_rate(2.0);
        control.set_settings(&VarioSettings {
            volume: 0.1,
            ..settings
        });
        loudness(&mut voice); // let the volume settle
        let quiet = loudness(&mut voice);
        assert!((0.05..0.1).contains(&quiet), "{quiet}");
    }
}
