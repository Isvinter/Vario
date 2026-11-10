//! Plays a flight through the simulated sensors and prints one line per GPS fix.
//!
//! ```text
//! cargo run -p sensor-sim --example replay -- [OPTIONS] <FILE.igc | --synthetic NAME>
//!
//!   --synthetic NAME   demo | climb | thermal | sink (instead of an IGC file)
//!   --speed FACTOR     playback speed, 1 = real time (default 10)
//!   --fast             run as fast as possible
//!   --out FILE         write the replayed flight to an IGC file
//! ```

use std::error::Error;
use std::process::ExitCode;

use flight_core::atmosphere::pressure_to_altitude;
use flight_core::igc::{FlightMeta, IgcFile};
use flight_core::sensor::{GpsSource, SensorEvent};
use flight_core::time::{Date, TimeOfDay, UtcDateTime};
use flight_core::track::TrackRecorder;
use sensor_sim::synthetic::SyntheticFlight;
use sensor_sim::{SimConfig, Simulation};

struct Options {
    input: Option<String>,
    synthetic: Option<String>,
    /// `None` means as fast as possible.
    speed: Option<f64>,
    out: Option<String>,
}

// `Box<dyn Error>` can hold any error type. That is convenient in a small program whose
// only reaction to an error is to print it.
fn parse_args() -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        input: None,
        synthetic: None,
        speed: Some(10.0),
        out: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--synthetic" => options.synthetic = Some(value("--synthetic")?),
            "--speed" => options.speed = Some(value("--speed")?.parse()?),
            "--fast" => options.speed = None,
            "--out" => options.out = Some(value("--out")?),
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}").into());
            }
            path => options.input = Some(path.to_owned()),
        }
    }
    if options.speed.is_some_and(|speed| speed <= 0.0) {
        return Err("--speed must be positive".into());
    }
    Ok(options)
}

fn play<G: GpsSource>(
    mut simulation: Simulation<G>,
    options: &Options,
) -> Result<(), Box<dyn Error>> {
    let mut recorder = TrackRecorder::new(FlightMeta::default());
    let mut pressure_altitude_m = f64::NAN;
    let mut accel_count = 0u32;

    println!(
        "flight duration {:.0} s\n  UTC         lat        lon    GPS alt  baro alt   speed  course  accel samples",
        simulation.duration_s
    );
    let on_event = |event: SensorEvent| {
        recorder.handle(&event);
        match event {
            SensorEvent::Accel(_) => accel_count += 1,
            SensorEvent::Pressure(sample) => {
                pressure_altitude_m = pressure_to_altitude(sample.pressure_pa);
            }
            SensorEvent::Gps(fix) => {
                let time = fix.utc.time;
                println!(
                    "{:02}:{:02}:{:02}  {:9.5}  {:9.5}  {:6.0} m  {:6.1} m  {:>6}  {:>6}  {:>6}",
                    time.hour(),
                    time.minute(),
                    time.second(),
                    fix.latitude_deg,
                    fix.longitude_deg,
                    fix.altitude_m,
                    pressure_altitude_m,
                    fix.ground_speed_mps
                        .map_or("-".to_owned(), |v| format!("{:.0} km/h", v * 3.6)),
                    fix.course_deg
                        .map_or("-".to_owned(), |c| format!("{c:.0}°")),
                    accel_count,
                );
                accel_count = 0;
            }
        }
    };

    match options.speed {
        Some(speed) => simulation.run_paced(speed, on_event),
        None => simulation.run_fast(on_event),
    }

    if let Some(path) = &options.out {
        let file = recorder.to_igc().ok_or("nothing was recorded")?;
        std::fs::write(path, file.to_string())?;
        println!("wrote {} fixes to {path}", recorder.fixes().len());
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_args()?;
    let config = SimConfig::default();

    if let Some(name) = &options.synthetic {
        let flight = match name.as_str() {
            "demo" => SyntheticFlight::demo(),
            "climb" => SyntheticFlight::steady_climb(2.0, 60.0),
            "thermal" => SyntheticFlight::thermal_circles(2.5, 120.0),
            "sink" => SyntheticFlight::strong_sink(4.0, 30.0),
            other => return Err(format!("unknown synthetic profile {other}").into()),
        };
        let start = UtcDateTime::new(
            Date::new(2026, 7, 28).expect("valid date"),
            TimeOfDay::new(10, 0, 0).expect("valid time"),
        );
        play(Simulation::synthetic(flight, start, &config), &options)
    } else if let Some(path) = &options.input {
        let file = IgcFile::parse_bytes(&std::fs::read(path)?)?;
        play(Simulation::replay(&file, &config)?, &options)
    } else {
        Err("give an IGC file or --synthetic NAME (demo, climb, thermal, sink)".into())
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
