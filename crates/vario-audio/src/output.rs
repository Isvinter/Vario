//! Plays the vario tone on the default sound output through `cpal`.
//!
//! `cpal` talks to ALSA on Linux and to CoreAudio on iOS and macOS. On iOS the app must
//! additionally configure the audio session (category "playback") before starting; that
//! is done on the Swift side.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
    SupportedBufferSize,
};
use thiserror::Error;

use crate::voice::{ToneControl, VarioVoice};

/// Preferred length of one audio buffer. The tone can react to a new climb rate at
/// most this much later, so shorter is better; too short risks dropouts.
const TARGET_BUFFER_S: f64 = 0.005;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no audio output device found")]
    NoDevice,
    #[error("unsupported audio sample format {0}")]
    UnsupportedFormat(SampleFormat),
    #[error("audio output failed: {0}")]
    Backend(#[from] cpal::Error),
}

/// A running audio output. The tone stops when this value is dropped.
pub struct AudioOutput {
    // Never read, but owning the stream is what keeps it playing.
    _stream: Stream,
    sample_rate_hz: u32,
    buffer_frames: Option<u32>,
}

impl AudioOutput {
    /// Opens the default output device and starts playing whatever `control` asks for.
    pub fn start(control: ToneControl) -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioError::NoDevice)?;
        let supported = device.default_output_config()?;
        let sample_rate_hz = supported.sample_rate();

        // Ask for a short buffer if the device says which sizes it supports.
        let buffer_frames = match *supported.buffer_size() {
            SupportedBufferSize::Range { min, max } => {
                let wanted = (f64::from(sample_rate_hz) * TARGET_BUFFER_S) as u32;
                Some(wanted.clamp(min, max))
            }
            SupportedBufferSize::Unknown => None,
        };

        let format = supported.sample_format();
        let mut config: StreamConfig = supported.config();
        config.buffer_size = buffer_frames.map_or(BufferSize::Default, BufferSize::Fixed);

        // Some devices refuse a fixed buffer size even inside their advertised range;
        // fall back to their default in that case.
        let (stream, buffer_frames) = match build(&device, config, format, &control) {
            Ok(stream) => (stream, buffer_frames),
            Err(_) if buffer_frames.is_some() => {
                config.buffer_size = BufferSize::Default;
                (build(&device, config, format, &control)?, None)
            }
            Err(error) => return Err(error),
        };
        stream.play()?;

        Ok(AudioOutput {
            _stream: stream,
            sample_rate_hz,
            buffer_frames,
        })
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Length of one audio buffer in milliseconds, if a fixed size could be set.
    pub fn buffer_ms(&self) -> Option<f64> {
        self.buffer_frames
            .map(|frames| f64::from(frames) * 1000.0 / f64::from(self.sample_rate_hz))
    }
}

fn build(
    device: &Device,
    config: StreamConfig,
    format: SampleFormat,
    control: &ToneControl,
) -> Result<Stream, AudioError> {
    match format {
        SampleFormat::F32 => build_typed::<f32>(device, config, control),
        SampleFormat::I16 => build_typed::<i16>(device, config, control),
        SampleFormat::U16 => build_typed::<u16>(device, config, control),
        SampleFormat::I32 => build_typed::<i32>(device, config, control),
        SampleFormat::U8 => build_typed::<u8>(device, config, control),
        other => Err(AudioError::UnsupportedFormat(other)),
    }
}

/// Builds the stream for one concrete sample type `T` of the sound card.
fn build_typed<T>(
    device: &Device,
    config: StreamConfig,
    control: &ToneControl,
) -> Result<Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let mut voice = VarioVoice::new(control.clone(), config.sample_rate);

    // This closure is the audio callback. `move` gives it ownership of `voice`; it runs
    // on the sound card's real-time thread whenever more samples are needed.
    let stream = device.build_output_stream(
        config,
        move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
            voice.sync();
            // One frame holds one sample per channel; all channels get the same signal.
            for frame in output.chunks_mut(channels) {
                let sample = T::from_sample(voice.next_sample());
                frame.fill(sample);
            }
        },
        |error| eprintln!("audio stream error: {error}"),
        None,
    )?;
    Ok(stream)
}
