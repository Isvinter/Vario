//! Command line vario: plays a recorded or synthetic flight through the vario filter
//! and makes the vario tone audible.

use std::error::Error;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use flight_core::igc::IgcFile;
use flight_core::sensor::{GpsSource, SensorEvent};
use flight_core::settings::VarioSettings;
use flight_core::time::{Date, TimeOfDay, UtcDateTime};
use flight_core::vario::VarioFilter;
use sensor_sim::synthetic::SyntheticFlight;
use sensor_sim::{SimConfig, Simulation};
use vario_audio::output::AudioOutput;
use vario_audio::voice::{ToneControl, VarioVoice};
use vario_audio::wav::write_wav;

/// Sample rate of WAV files written with --wav.
const WAV_SAMPLE_RATE_HZ: u32 = 44_100;

// clap generates the argument parser, `--help` text and error messages from this
// struct; the `///` comments become the help text.
/// Plays a flight through the vario filter and tone.
#[derive(Parser, Debug)]
#[command(name = "vario", version)]
struct Args {
    /// IGC file to play
    #[arg(required_unless_present = "synthetic", conflicts_with = "synthetic")]
    file: Option<String>,

    /// Play a synthetic flight instead: demo, climb, thermal or sink
    #[arg(long, value_name = "NAME")]
    synthetic: Option<String>,

    /// Playback speed; 1 is real time
    #[arg(long, default_value_t = 1.0)]
    speed: f64,

    /// Climb tone starts above this climb rate in m/s
    #[arg(long, value_name = "M/S", allow_negative_numbers = true,
          default_value_t = VarioSettings::default().climb_threshold_mps)]
    climb_threshold: f64,

    /// Sink tone starts below this climb rate in m/s
    #[arg(long, value_name = "M/S", allow_negative_numbers = true,
          default_value_t = VarioSettings::default().sink_threshold_mps)]
    sink_threshold: f64,

    /// Tone volume from 0 to 1
    #[arg(long, default_value_t = VarioSettings::default().volume)]
    volume: f64,

    /// Averaging time of the climb rate in seconds; 0 turns it off
    #[arg(long, value_name = "SECONDS",
          default_value_t = VarioSettings::default().damping_s)]
    damping: f64,

    /// Simulated barometer rate in Hz (an iPhone delivers about 1)
    #[arg(long, value_name = "HZ", default_value_t = 1.0)]
    pressure_rate: f64,

    /// Add realistic noise to the simulated sensors
    #[arg(long)]
    noise: bool,

    /// Do not play sound, only print the values
    #[arg(long)]
    no_audio: bool,

    /// Write the tone to a WAV file instead of playing it; runs as fast as possible
    #[arg(long, value_name = "FILE", conflicts_with = "no_audio")]
    wav: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<(), Box<dyn Error>> {
    let settings = VarioSettings {
        climb_threshold_mps: args.climb_threshold,
        sink_threshold_mps: args.sink_threshold,
        volume: args.volume,
        damping_s: args.damping,
    };
    settings.validate()?;
    // Written this way round so that NaN is rejected too.
    if !(args.speed > 0.0 && args.speed.is_finite()) {
        return Err("--speed must be positive".into());
    }
    if !(args.pressure_rate > 0.0 && args.pressure_rate <= 1000.0) {
        return Err("--pressure-rate must be between 0 and 1000 Hz".into());
    }

    let mut config = SimConfig {
        pressure_rate_hz: args.pressure_rate,
        ..SimConfig::default()
    };
    if args.noise {
        config.pressure_noise_pa = 3.0;
        config.accel_noise_mps2 = 0.15;
        config.accel_bias_mps2 = 0.15;
    }

    if let Some(name) = &args.synthetic {
        let flight = match name.as_str() {
            "demo" => SyntheticFlight::demo(),
            "climb" => SyntheticFlight::steady_climb(2.0, 60.0),
            "thermal" => SyntheticFlight::thermal_circles(2.5, 120.0),
            "sink" => SyntheticFlight::strong_sink(4.0, 30.0),
            other => {
                return Err(format!(
                    "unknown synthetic flight '{other}' (use demo, climb, thermal or sink)"
                )
                .into());
            }
        };
        let start = UtcDateTime::new(
            Date::new(2026, 7, 28).expect("valid date"),
            TimeOfDay::new(10, 0, 0).expect("valid time"),
        );
        play(
            Simulation::synthetic(flight, start, &config),
            args,
            &settings,
        )
    } else {
        let path = args.file.as_deref().expect("clap requires a file");
        let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
        let file = IgcFile::parse_bytes(&bytes)?;
        play(Simulation::replay(&file, &config)?, args, &settings)
    }
}

fn play<G: GpsSource>(
    mut simulation: Simulation<G>,
    args: &Args,
    settings: &VarioSettings,
) -> Result<(), Box<dyn Error>> {
    let control = ToneControl::new(settings);
    let mut filter = VarioFilter::default();
    filter.set_damping(settings.damping_s);
    let mut display = Display::default();

    println!("flight duration {}", clock_time(simulation.duration_s));

    if let Some(path) = &args.wav {
        // Offline: generate exactly as much audio as simulated time passes.
        let mut voice = VarioVoice::new(control.clone(), WAV_SAMPLE_RATE_HZ);
        let mut samples: Vec<f32> = Vec::new();
        simulation.run_fast(|event| {
            let due = (event.time().as_secs() * f64::from(WAV_SAMPLE_RATE_HZ)) as usize;
            if due > samples.len() {
                let rendered = samples.len();
                samples.resize(due, 0.0);
                voice.render(&mut samples[rendered..]);
            }
            step(&mut filter, &control, &mut display, &event, false);
        });
        let file = std::fs::File::create(path).map_err(|error| format!("{path}: {error}"))?;
        write_wav(std::io::BufWriter::new(file), WAV_SAMPLE_RATE_HZ, &samples)?;
        println!(
            "wrote {} of audio to {path}",
            clock_time(simulation.duration_s)
        );
        return Ok(());
    }

    // The output must stay alive while we play, hence the variable.
    let _output = if args.no_audio {
        None
    } else {
        let output = AudioOutput::start(control.clone())?;
        match output.buffer_ms() {
            Some(ms) => println!("audio: {} Hz, {ms:.1} ms buffer", output.sample_rate_hz()),
            None => println!(
                "audio: {} Hz, device default buffer",
                output.sample_rate_hz()
            ),
        }
        Some(output)
    };

    simulation.run_paced(args.speed, |event| {
        step(&mut filter, &control, &mut display, &event, true);
    });

    // Let the last tone fade out before the output is closed.
    control.set_climb_rate(0.0);
    std::thread::sleep(Duration::from_millis(100));
    println!();
    Ok(())
}

/// Handles one sensor sample: filter, tone, display.
fn step(
    filter: &mut VarioFilter,
    control: &ToneControl,
    display: &mut Display,
    event: &SensorEvent,
    live: bool,
) {
    filter.handle(event);
    if let SensorEvent::Gps(fix) = event {
        display.ground_speed_mps = fix.ground_speed_mps;
        display.course_deg = fix.course_deg;
    }
    if let Some(reading) = filter.reading() {
        control.set_climb_rate(reading.climb_rate_mps);
        if live {
            display.show(
                reading.time.as_secs(),
                reading.altitude_m,
                reading.climb_rate_mps,
            );
        }
    }
}

/// The status line, redrawn in place a few times per second.
#[derive(Default)]
struct Display {
    ground_speed_mps: Option<f64>,
    course_deg: Option<f64>,
    last_shown_s: Option<f64>,
}

impl Display {
    const INTERVAL_S: f64 = 0.2;

    fn show(&mut self, time_s: f64, altitude_m: f64, climb_rate_mps: f64) {
        if self
            .last_shown_s
            .is_some_and(|last| time_s - last < Self::INTERVAL_S)
        {
            return;
        }
        self.last_shown_s = Some(time_s);

        let speed = self
            .ground_speed_mps
            .map_or("  - km/h".to_owned(), |v| format!("{:3.0} km/h", v * 3.6));
        let course = self
            .course_deg
            .map_or("  -°".to_owned(), |c| format!("{c:3.0}°"));
        // "\r" returns to the start of the line, so the next print overwrites it.
        print!(
            "\r{}  {altitude_m:6.0} m  {climb_rate_mps:+5.1} m/s  {}  {speed}  {course} ",
            clock_time(time_s),
            climb_bar(climb_rate_mps),
        );
        // Without a newline the terminal would not show the text until later.
        let _ = std::io::stdout().flush();
    }
}

/// A bar from -5 to +5 m/s with the zero point in the middle.
fn climb_bar(climb_rate_mps: f64) -> String {
    const HALF_WIDTH: i32 = 10;
    let filled = (climb_rate_mps / 5.0 * f64::from(HALF_WIDTH))
        .round()
        .clamp(-f64::from(HALF_WIDTH), f64::from(HALF_WIDTH)) as i32;
    let mut bar = String::new();
    for position in -HALF_WIDTH..=HALF_WIDTH {
        let inside = (filled < 0 && (filled..0).contains(&position))
            || (filled > 0 && (1..=filled).contains(&position));
        bar.push(if position == 0 {
            '|'
        } else if inside {
            '#'
        } else {
            '.'
        });
    }
    bar
}

/// Seconds as `m:ss`.
fn clock_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climb_bar_grows_from_the_middle() {
        assert_eq!(climb_bar(0.0), "..........|..........");
        assert_eq!(climb_bar(2.5), "..........|#####.....");
        assert_eq!(climb_bar(-1.0), "........##|..........");
        assert_eq!(climb_bar(99.0), "..........|##########");
        assert_eq!(climb_bar(-99.0), "##########|..........");
    }

    #[test]
    fn clock_time_formats_minutes_and_seconds() {
        assert_eq!(clock_time(0.0), "0:00");
        assert_eq!(clock_time(75.9), "1:15");
        assert_eq!(clock_time(3600.0), "60:00");
    }

    #[test]
    fn arguments_are_parsed_including_negative_numbers() {
        let args = Args::try_parse_from([
            "vario",
            "--synthetic",
            "demo",
            "--sink-threshold",
            "-3.5",
            "--climb-threshold",
            "-0.1",
            "--volume",
            "0.3",
        ])
        .unwrap();
        assert_eq!(args.sink_threshold, -3.5);
        assert_eq!(args.climb_threshold, -0.1);
        assert_eq!(args.volume, 0.3);
        assert_eq!(args.damping, 0.5);

        assert!(Args::try_parse_from(["vario"]).is_err(), "needs an input");
        assert!(Args::try_parse_from(["vario", "a.igc", "--synthetic", "demo"]).is_err());
    }
}
