//! Reads OpenAir files and prints what was found: airspaces per category and type,
//! limits and all issues.
//!
//! ```text
//! cargo run -p flight-core --example airspace -- de_asp_v2.txt at_asp_v2.txt
//! ```

use std::collections::BTreeMap;

use flight_core::airspace;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: airspace FILE.txt …");
        std::process::exit(2);
    }
    for path in paths {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("{path}: {error}");
                std::process::exit(1);
            }
        };
        let file = airspace::parse(&bytes);
        let points: usize = file.airspaces.iter().map(|a| a.outline.len()).sum();
        println!(
            "{path}: {} airspaces, {points} outline points, {} issues",
            file.airspaces.len(),
            file.issues.len()
        );
        let mut categories: BTreeMap<String, usize> = BTreeMap::new();
        for a in &file.airspaces {
            let key = format!(
                "{:?} ({} / {})",
                a.category(),
                a.class,
                a.kind.as_deref().unwrap_or("-")
            );
            *categories.entry(key).or_default() += 1;
        }
        for (key, count) in categories {
            println!("  {count:5}  {key}");
        }
        for issue in file.issues.iter().take(20) {
            println!("  line {}: {}", issue.line, issue.message);
        }
    }
}
