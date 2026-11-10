//! The tone generator.
//!
//! Produces audio one sample at a time. It is written for the audio callback: no memory
//! allocation, no locks, no system calls, only arithmetic on a few numbers.
//!
//! # Avoiding clicks
//!
//! A click is a sudden jump in the output signal. Three things prevent them:
//!
//! * The oscillator keeps a running *phase* and only ever changes how fast the phase
//!   advances. Changing the pitch therefore bends the wave instead of breaking it.
//! * Pitch changes glide to the new value over a few milliseconds.
//! * Beeps are switched on and off through an envelope that fades over a few
//!   milliseconds; volume changes are smoothed the same way.

use std::f32::consts::TAU;

use crate::tone::Tone;

/// Fade time of a beep's start and end.
const FADE_S: f32 = 0.005;
/// Time constant of pitch glides.
const GLIDE_S: f32 = 0.015;
/// Time constant of volume changes.
const VOLUME_SMOOTHING_S: f32 = 0.02;

#[derive(Debug, Clone)]
pub struct Synth {
    sample_rate: f32,
    tone: Tone,
    target_volume: f32,

    /// Position within one wave cycle, 0..1.
    phase: f32,
    /// Position within one beep cycle, 0..1.
    beat: f32,
    /// Current, smoothed frequency in Hz.
    frequency_hz: f32,
    /// Current envelope position, 0 (off) ..= 1 (fully on).
    envelope: f32,
    /// Current, smoothed volume.
    volume: f32,

    fade_step: f32,
    glide_coefficient: f32,
    volume_coefficient: f32,
}

impl Synth {
    pub fn new(sample_rate_hz: u32) -> Self {
        let sample_rate = sample_rate_hz as f32;
        // Per-sample factor of a one-pole low-pass with the given time constant.
        let coefficient =
            |time_constant_s: f32| 1.0 - (-1.0 / (time_constant_s * sample_rate)).exp();
        Synth {
            sample_rate,
            tone: Tone::Silence,
            target_volume: 1.0,
            phase: 0.0,
            beat: 0.0,
            frequency_hz: 0.0,
            envelope: 0.0,
            volume: 1.0,
            fade_step: 1.0 / (FADE_S * sample_rate),
            glide_coefficient: coefficient(GLIDE_S),
            volume_coefficient: coefficient(VOLUME_SMOOTHING_S),
        }
    }

    pub fn set_tone(&mut self, tone: Tone) {
        // Coming out of silence, start with a beep rather than a pause, so the pilot
        // hears the lift immediately.
        if self.tone == Tone::Silence && tone != Tone::Silence {
            self.beat = 0.0;
        }
        self.tone = tone;
    }

    /// Volume from 0.0 to 1.0.
    pub fn set_volume(&mut self, volume: f32) {
        self.target_volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// The next audio sample, in the range -1.0..=1.0.
    pub fn next_sample(&mut self) -> f32 {
        let (target_frequency, gate_open) = match self.tone {
            Tone::Silence => (self.frequency_hz, false),
            Tone::Continuous { frequency_hz } => (frequency_hz, true),
            Tone::Beep {
                frequency_hz,
                cycle_s,
                duty,
            } => {
                let open = self.beat < duty;
                // Advancing a beat *phase* (instead of counting samples) lets the
                // beep rate change smoothly in the middle of a cycle.
                self.beat += 1.0 / (cycle_s.max(0.01) * self.sample_rate);
                if self.beat >= 1.0 {
                    self.beat -= 1.0;
                }
                (frequency_hz, open)
            }
        };

        if self.envelope == 0.0 {
            // Nothing is audible: jump to the new pitch instead of gliding up from
            // the pitch of the previous beep.
            self.frequency_hz = target_frequency;
        } else {
            self.frequency_hz += (target_frequency - self.frequency_hz) * self.glide_coefficient;
        }

        let envelope_target = if gate_open { 1.0 } else { 0.0 };
        if self.envelope < envelope_target {
            self.envelope = (self.envelope + self.fade_step).min(1.0);
        } else if self.envelope > envelope_target {
            self.envelope = (self.envelope - self.fade_step).max(0.0);
        }
        self.volume += (self.target_volume - self.volume) * self.volume_coefficient;

        self.phase += self.frequency_hz / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        // "Smoothstep" turns the linear fade into an S-curve without corners.
        let fade = self.envelope * self.envelope * (3.0 - 2.0 * self.envelope);
        waveform(self.phase) * fade * self.volume
    }

    /// Fills a buffer with samples.
    pub fn render(&mut self, buffer: &mut [f32]) {
        for sample in buffer {
            *sample = self.next_sample();
        }
    }
}

/// One cycle of the tone for a phase of 0..1: a sine with some third harmonic. The
/// overtone makes the tone easier to hear on small speakers and in wind noise than a
/// pure sine. The peak value is about 0.92.
fn waveform(phase: f32) -> f32 {
    let angle = TAU * phase;
    angle.sin() + 0.3 * (3.0 * angle).sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 48_000;

    fn render(synth: &mut Synth, seconds: f32) -> Vec<f32> {
        let mut buffer = vec![0.0; (seconds * SAMPLE_RATE as f32) as usize];
        synth.render(&mut buffer);
        buffer
    }

    /// The largest jump between neighbouring samples.
    fn max_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    /// The largest step a clean tone of this frequency can have: its steepest slope
    /// (the derivative of the waveform peaks at 1 + 0.9) times the sample interval.
    fn max_clean_step(frequency_hz: f32) -> f32 {
        TAU * frequency_hz / SAMPLE_RATE as f32 * 1.9
    }

    fn frequency_of(samples: &[f32]) -> f32 {
        let rising_crossings = samples
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        rising_crossings as f32 * SAMPLE_RATE as f32 / samples.len() as f32
    }

    /// Lengths of the audible stretches, in seconds.
    fn beep_lengths(samples: &[f32]) -> Vec<f32> {
        let mut lengths = Vec::new();
        let mut current = 0u32;
        let mut quiet = 0u32;
        for sample in samples {
            if sample.abs() > 1e-4 {
                current += 1 + quiet;
                quiet = 0;
            } else if current > 0 {
                quiet += 1;
                // Zero crossings of the wave are short; a real pause is longer.
                if quiet > 100 {
                    lengths.push(current as f32 / SAMPLE_RATE as f32);
                    current = 0;
                    quiet = 0;
                }
            }
        }
        lengths
    }

    #[test]
    fn silence_is_exactly_zero() {
        let mut synth = Synth::new(SAMPLE_RATE);
        assert!(render(&mut synth, 0.1).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn continuous_tone_has_the_requested_pitch_and_no_gaps() {
        let mut synth = Synth::new(SAMPLE_RATE);
        synth.set_tone(Tone::Continuous {
            frequency_hz: 300.0,
        });
        let samples = render(&mut synth, 1.0);

        assert!((frequency_of(&samples) - 300.0).abs() < 2.0);
        let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!((0.85..=1.0).contains(&peak), "peak {peak}");
        assert_eq!(beep_lengths(&samples).len(), 0, "tone must not pause");
    }

    #[test]
    fn beeps_follow_cycle_and_duty() {
        let mut synth = Synth::new(SAMPLE_RATE);
        synth.set_tone(Tone::Beep {
            frequency_hz: 1000.0,
            cycle_s: 0.25,
            duty: 0.5,
        });
        let samples = render(&mut synth, 2.0);
        let lengths = beep_lengths(&samples);

        assert_eq!(lengths.len(), 8, "{lengths:?}");
        for length in lengths {
            // Half of the cycle plus the fade-out.
            assert!((length - 0.13).abs() < 0.005, "beep of {length} s");
        }
        // The first beep starts right away.
        assert!(samples[..480].iter().any(|s| s.abs() > 0.1));
    }

    #[test]
    fn volume_scales_the_output() {
        let mut synth = Synth::new(SAMPLE_RATE);
        synth.set_tone(Tone::Continuous {
            frequency_hz: 500.0,
        });
        synth.set_volume(0.25);
        let samples = render(&mut synth, 1.0);
        let peak = samples[24_000..]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!((peak - 0.25 * 0.92).abs() < 0.01, "peak {peak}");

        synth.set_volume(f32::NAN);
        let samples = render(&mut synth, 1.0);
        assert!(samples[24_000..].iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn no_clicks_when_the_tone_changes_abruptly() {
        let mut synth = Synth::new(SAMPLE_RATE);
        let mut samples = Vec::new();
        // A worst-case sequence, switching at awkward moments in the wave and the beep.
        let tones = [
            Tone::Continuous {
                frequency_hz: 300.0,
            },
            Tone::Beep {
                frequency_hz: 1800.0,
                cycle_s: 0.12,
                duty: 0.5,
            },
            Tone::Silence,
            Tone::Beep {
                frequency_hz: 600.0,
                cycle_s: 0.6,
                duty: 0.5,
            },
            Tone::Beep {
                frequency_hz: 1800.0,
                cycle_s: 0.12,
                duty: 0.5,
            },
            Tone::Continuous {
                frequency_hz: 180.0,
            },
            Tone::Silence,
            Tone::Continuous {
                frequency_hz: 350.0,
            },
        ];
        for (index, tone) in tones.iter().cycle().take(200).enumerate() {
            synth.set_tone(*tone);
            synth.set_volume(if index % 3 == 0 { 0.3 } else { 1.0 });
            samples.extend(render(&mut synth, 0.0137 + 0.001 * (index % 7) as f32));
        }

        let step = max_step(&samples);
        let limit = max_clean_step(1800.0) * 1.1;
        assert!(step <= limit, "step {step} exceeds {limit}");
    }

    #[test]
    fn pitch_glides_instead_of_jumping() {
        let mut synth = Synth::new(SAMPLE_RATE);
        synth.set_tone(Tone::Continuous {
            frequency_hz: 400.0,
        });
        render(&mut synth, 0.5);
        synth.set_tone(Tone::Continuous {
            frequency_hz: 800.0,
        });
        let glide = render(&mut synth, 0.2);

        // Right after the change the wave still moves as slowly as the old pitch ...
        assert!(max_step(&glide[..48]) <= max_clean_step(430.0));
        // ... and 100 ms later it has arrived at the new one.
        assert!((frequency_of(&glide[4800..]) - 800.0).abs() < 10.0);
    }

    #[test]
    fn beep_rate_changes_in_the_middle_of_a_cycle() {
        let mut synth = Synth::new(SAMPLE_RATE);
        let slow = Tone::Beep {
            frequency_hz: 600.0,
            cycle_s: 0.6,
            duty: 0.5,
        };
        let fast = Tone::Beep {
            frequency_hz: 1400.0,
            cycle_s: 0.2,
            duty: 0.5,
        };
        synth.set_tone(slow);
        // 0.4 s: inside the pause of the first slow cycle (0.3..0.6 s).
        render(&mut synth, 0.4);
        synth.set_tone(fast);
        let samples = render(&mut synth, 0.2);

        // A third of the cycle was left; at the fast rate that is under 70 ms. Waiting
        // for the slow cycle to finish would take 200 ms.
        let first_sound = samples.iter().position(|s| s.abs() > 0.1).unwrap();
        assert!(
            first_sound < (0.08 * SAMPLE_RATE as f32) as usize,
            "{first_sound}"
        );
    }
}
