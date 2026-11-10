//! End-to-end tests of phase 1: read an IGC file, replay it through the simulated
//! sensors, record the replay and write it again. The result must have the same content.

use flight_core::igc::{FlightMeta, IgcFile, IgcFix};
use flight_core::sensor::{GpsSource, SensorEvent};
use flight_core::time::{Date, TimeOfDay, UtcDateTime};
use flight_core::track::TrackRecorder;
use sensor_sim::synthetic::SyntheticFlight;
use sensor_sim::{SimConfig, Simulation};

fn meta() -> FlightMeta {
    FlightMeta {
        pilot: "Test Pilot".to_owned(),
        glider_type: "Test Wing".to_owned(),
        glider_id: "D-TEST".to_owned(),
    }
}

/// Runs a simulation to its end as fast as possible and records it.
fn record<G: GpsSource>(mut simulation: Simulation<G>) -> IgcFile {
    let mut recorder = TrackRecorder::new(meta());
    simulation.run_fast(|event| recorder.handle(&event));
    recorder.to_igc().expect("the flight has fixes")
}

/// Records the synthetic demo flight, starting shortly before midnight UTC so the
/// rollover is covered too.
fn recorded_demo_flight() -> IgcFile {
    let start = UtcDateTime::new(
        Date::new(2026, 7, 28).unwrap(),
        TimeOfDay::new(23, 58, 30).unwrap(),
    );
    record(Simulation::synthetic(
        SyntheticFlight::demo(),
        start,
        &SimConfig::default(),
    ))
}

#[test]
fn synthetic_flight_is_recorded_at_one_fix_per_second() {
    let file = recorded_demo_flight();
    let fixes: Vec<&IgcFix> = file.fixes().collect();

    // 220 s flight: fixes at 0..=220 s.
    assert_eq!(fixes.len(), 221);
    assert_eq!(fixes[0].time, TimeOfDay::new(23, 58, 30).unwrap());
    assert_eq!(fixes[220].time, TimeOfDay::new(0, 2, 10).unwrap());

    // The simulated atmosphere is the standard atmosphere, so both altitudes agree.
    assert_eq!(fixes[0].pressure_altitude_m, 1500);
    assert!(
        fixes
            .iter()
            .all(|fix| fix.pressure_altitude_m == fix.gps_altitude_m)
    );
    // 30 s glide at -1.2 m/s, then thermals: the flight must gain height overall.
    assert_eq!(fixes[30].gps_altitude_m, 1464);
    assert!(fixes[130].gps_altitude_m > 1650);
}

#[test]
fn replaying_a_recorded_file_reproduces_it_exactly() {
    let original_text = recorded_demo_flight().to_string();

    let parsed = IgcFile::parse(&original_text).unwrap();
    let replay = Simulation::replay(&parsed, &SimConfig::default()).unwrap();
    let replayed_text = record(replay).to_string();

    assert_eq!(replayed_text, original_text);
}

#[test]
fn replay_is_exact_for_any_sensor_rates() {
    let original_text = recorded_demo_flight().to_string();
    let parsed = IgcFile::parse(&original_text).unwrap();

    let config = SimConfig {
        pressure_rate_hz: 10.0,
        accel_rate_hz: 100.0,
        ..SimConfig::default()
    };
    let replay = Simulation::replay(&parsed, &config).unwrap();
    assert_eq!(record(replay).to_string(), original_text);
}

#[test]
fn paced_and_fast_replay_give_the_same_result() {
    // A 5 s flight at 100x speed takes about 50 ms of real time.
    let start = UtcDateTime::new(Date::new(2026, 7, 28).unwrap(), TimeOfDay::MIDNIGHT);
    let config = SimConfig::default();
    let flight = || SyntheticFlight::thermal_circles(2.0, 5.0);

    let fast = record(Simulation::synthetic(flight(), start, &config));

    let mut simulation = Simulation::synthetic(flight(), start, &config);
    let mut recorder = TrackRecorder::new(meta());
    let mut accel_samples = 0;
    simulation.run_paced(100.0, |event| {
        if matches!(event, SensorEvent::Accel(_)) {
            accel_samples += 1;
        }
        recorder.handle(&event);
    });

    assert_eq!(recorder.to_igc().unwrap(), fast);
    assert_eq!(accel_samples, 251);
}

/// A file as another recorder might write it: different header style, extra B record
/// fields declared by an I record, comments and a signature.
const FOREIGN_FILE: &str = "AXCT0a1b2c3d4e5f\r\n\
    HFDTE280726\r\n\
    HFPLTPILOT:Jane Doe\r\n\
    HFGTYGLIDERTYPE:Some Wing\r\n\
    I013638FXA\r\n\
    B1000004730000N01115000EA0150001520005\r\n\
    B1000014730005N01115004EA0150101521005\r\n\
    B1000024730011N01115007EA0150301523004\r\n\
    B1000044730022N01115013EV0150801528012\r\n\
    B1000054730027N01115016EA0151001530005\r\n\
    LXCT comment line\r\n\
    B1000064730031N01115018EA0151101531005\r\n\
    G4E9A5C7B\r\n";

#[test]
fn foreign_file_is_rewritten_byte_for_byte() {
    let parsed = IgcFile::parse(FOREIGN_FILE).unwrap();
    assert_eq!(parsed.to_string(), FOREIGN_FILE);
}

#[test]
fn replaying_a_foreign_file_reproduces_all_fixes() {
    let parsed = IgcFile::parse(FOREIGN_FILE).unwrap();
    let replay = Simulation::replay(&parsed, &SimConfig::default()).unwrap();
    let recorded = record(replay);

    assert_eq!(recorded.date(), parsed.date());

    // Extra fields such as the GPS accuracy (FXA) are not carried by the sensor
    // interface, so they are the one thing a replay cannot reproduce.
    let expected: Vec<IgcFix> = parsed
        .fixes()
        .map(|fix| IgcFix {
            extension: String::new(),
            ..fix.clone()
        })
        .collect();
    let actual: Vec<IgcFix> = recorded.fixes().cloned().collect();
    assert_eq!(actual, expected);
}
