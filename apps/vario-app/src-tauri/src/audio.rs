//! Keeps the audio output alive on a thread of its own.
//!
//! The audio stream handle of `cpal` must stay on the thread that created it (it is not
//! `Send`), but Tauri's app state must be usable from any thread. So a dedicated thread
//! creates the stream and then waits until it is told to stop.
//!
//! While waiting, the thread checks a few times per second whether the native side
//! reported the end of an audio interruption (a phone call, an alarm, ...). On iOS the
//! old stream stays silent after an interruption, so the thread then opens a new one.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use vario_audio::output::AudioOutput;
use vario_audio::voice::ToneControl;

/// How often the audio thread looks for the end of an interruption. The tone comes back
/// at most this much later.
const RESUME_CHECK_INTERVAL: Duration = Duration::from_millis(250);

pub struct AudioThread {
    // Dropping the sender is the stop signal; see `Drop` below.
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioThread {
    /// Opens the audio output. Returns the thread handle and a short description of
    /// the output for display.
    pub fn start(control: ToneControl) -> Result<(AudioThread, String), String> {
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (stop_sender, stop_receiver) = mpsc::channel::<()>();

        let thread = std::thread::Builder::new()
            .name("vario-audio".to_owned())
            .spawn(move || {
                let mut resumes_seen = sensor_bridge::audio_resume_count();
                let mut output = match AudioOutput::start(control.clone()) {
                    Ok(output) => {
                        let _ = ready_sender.send(Ok(describe(&output)));
                        Some(output)
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error.to_string()));
                        return;
                    }
                };

                // Wakes up every interval. Ends once the `AudioThread` dropped its
                // sender (or sent something); `output` is dropped afterwards, which
                // stops the sound.
                while let Err(RecvTimeoutError::Timeout) =
                    stop_receiver.recv_timeout(RESUME_CHECK_INTERVAL)
                {
                    let resumes = sensor_bridge::audio_resume_count();
                    if resumes == resumes_seen {
                        continue;
                    }
                    resumes_seen = resumes;
                    // Close the old stream before opening the new one.
                    output = None;
                    match AudioOutput::start(control.clone()) {
                        Ok(new_output) => {
                            eprintln!("audio output reopened: {}", describe(&new_output));
                            output = Some(new_output);
                        }
                        // Stays silent until the next report from the native side.
                        Err(error) => eprintln!("could not reopen audio output: {error}"),
                    }
                }
                drop(output);
            })
            .map_err(|error| error.to_string())?;

        let info = ready_receiver
            .recv()
            .map_err(|_| "audio thread ended unexpectedly".to_owned())??;
        Ok((
            AudioThread {
                stop: Some(stop_sender),
                thread: Some(thread),
            },
            info,
        ))
    }
}

fn describe(output: &AudioOutput) -> String {
    let buffer = output
        .buffer_ms()
        .map_or("default buffer".to_owned(), |ms| {
            format!("{ms:.1} ms buffer")
        });
    format!("{} Hz, {buffer}", output.sample_rate_hz())
}

impl Drop for AudioThread {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
