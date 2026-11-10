//! Keeps the audio output alive on a thread of its own.
//!
//! The audio stream handle of `cpal` must stay on the thread that created it (it is not
//! `Send`), but Tauri's app state must be usable from any thread. So a dedicated thread
//! creates the stream and then simply waits until it is told to stop.

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use vario_audio::output::AudioOutput;
use vario_audio::voice::ToneControl;

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
            .spawn(move || match AudioOutput::start(control) {
                Ok(output) => {
                    let buffer = output
                        .buffer_ms()
                        .map_or("default buffer".to_owned(), |ms| {
                            format!("{ms:.1} ms buffer")
                        });
                    let _ =
                        ready_sender.send(Ok(format!("{} Hz, {buffer}", output.sample_rate_hz())));
                    // Blocks until the `AudioThread` drops its sender. `output` is
                    // dropped afterwards, which stops the sound.
                    let _ = stop_receiver.recv();
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(error.to_string()));
                }
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

impl Drop for AudioThread {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
