//! Airspaces for the map: OpenAir files in the folder `airspace` next to `maps`.
//!
//! The files are read with `flight_core::airspace` and handed to the map as one GeoJSON
//! feature collection. The folder is created if it is missing, so that it shows up in
//! the Files app on iOS and the files can be copied into it.

use std::path::{Path, PathBuf};

use flight_core::airspace::{self, Airspace, Category, Limit, Unit};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::AppHandle;

/// File in the config folder listing the categories the map does not show, one per line.
const HIDDEN_FILE_NAME: &str = "airspace-hidden.txt";
/// Hidden as long as nothing was chosen: information sectors, FIR, military low flying
/// areas and the like would only clutter the map.
const HIDDEN_BY_DEFAULT: [&str; 1] = ["other"];

/// All categories with the names the map style uses (see `map.js`).
const CATEGORIES: [(Category, &str); 11] = [
    (Category::Ctr, "ctr"),
    (Category::Controlled, "controlled"),
    (Category::ClassE, "classE"),
    (Category::Restricted, "restricted"),
    (Category::Danger, "danger"),
    (Category::Prohibited, "prohibited"),
    (Category::Tmz, "tmz"),
    (Category::Rmz, "rmz"),
    (Category::GliderSector, "gliderSector"),
    (Category::Reserved, "reserved"),
    (Category::Other, "other"),
];

/// File name endings read as OpenAir.
const EXTENSIONS: [&str; 2] = ["txt", "openair"];
/// At most this many issues per file are passed on; a broken file can have thousands.
const MAX_ISSUES: usize = 20;

/// What was read from one file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSummary {
    pub name: String,
    pub airspaces: usize,
    /// Number of issues; `issues` holds at most the first few.
    pub issue_count: usize,
    pub issues: Vec<String>,
    /// Set if the file could not be read at all.
    pub error: Option<String>,
}

/// All airspaces for the map.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Airspaces {
    pub files: Vec<FileSummary>,
    /// GeoJSON feature collection, one polygon per airspace.
    pub geojson: Value,
    /// Categories the map does not show.
    pub hidden: Vec<String>,
}

/// The folder holding the OpenAir files.
pub fn dir(app: &AppHandle) -> Option<PathBuf> {
    crate::data_dir(app).map(|dir| dir.join("airspace"))
}

/// Reads all OpenAir files of the app.
pub fn load(app: &AppHandle) -> Result<Airspaces, String> {
    let dir = dir(app).ok_or("no data folder")?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut airspaces = load_dir(&dir)?;
    airspaces.hidden = load_hidden(app);
    Ok(airspaces)
}

/// The categories the map does not show.
pub fn load_hidden(app: &AppHandle) -> Vec<String> {
    crate::settings::config_file(app, HIDDEN_FILE_NAME)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse_hidden(&text))
        .unwrap_or_else(|| HIDDEN_BY_DEFAULT.map(str::to_string).to_vec())
}

/// Saves the categories the map does not show and returns them as stored (unknown names
/// are dropped).
pub fn save_hidden(app: &AppHandle, hidden: &[String]) -> Result<Vec<String>, String> {
    let hidden = parse_hidden(&hidden.join("\n"));
    let path = crate::settings::config_file(app, HIDDEN_FILE_NAME)?;
    // An empty file means "show everything", which is different from no file at all.
    crate::settings::write_file(path, &hidden.join("\n"))?;
    Ok(hidden)
}

/// Known category names from `text`, one per line, in the order of [`CATEGORIES`].
fn parse_hidden(text: &str) -> Vec<String> {
    let names: Vec<&str> = text.lines().map(str::trim).collect();
    CATEGORIES
        .iter()
        .filter(|(_, name)| names.contains(name))
        .map(|(_, name)| name.to_string())
        .collect()
}

/// Reads all OpenAir files in `dir`, in the order of their names.
pub fn load_dir(dir: &Path) -> Result<Airspaces, String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        })
        .collect();
    paths.sort();

    let mut files = Vec::new();
    let mut features = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read(&path) {
            Ok(bytes) => {
                let file = airspace::parse(&bytes);
                features.extend(file.airspaces.iter().map(feature));
                files.push(FileSummary {
                    name,
                    airspaces: file.airspaces.len(),
                    issue_count: file.issues.len(),
                    issues: file
                        .issues
                        .iter()
                        .take(MAX_ISSUES)
                        .map(|issue| format!("line {}: {}", issue.line, issue.message))
                        .collect(),
                    error: None,
                });
            }
            Err(error) => files.push(FileSummary {
                name,
                airspaces: 0,
                issue_count: 0,
                issues: Vec::new(),
                error: Some(error.to_string()),
            }),
        }
    }
    Ok(Airspaces {
        files,
        geojson: json!({ "type": "FeatureCollection", "features": features }),
        hidden: Vec::new(),
    })
}

/// One airspace as a GeoJSON feature.
fn feature(airspace: &Airspace) -> Value {
    let lower = limit_label(&airspace.lower);
    let upper = limit_label(&airspace.upper);
    json!({
        "type": "Feature",
        "properties": {
            "name": airspace.name,
            "category": category_name(airspace.category()),
            "class": airspace.class,
            "kind": airspace.kind,
            "frequency": airspace.frequency,
            "lower": lower,
            "upper": upper,
            // For the label along the border: name, then upper over lower limit.
            "label": format!("{} {upper} / {lower}", airspace.name),
        },
        "geometry": { "type": "Polygon", "coordinates": [ring(&airspace.outline)] },
    })
}

/// The outline as a closed GeoJSON ring of `[longitude, latitude]`, counterclockwise as
/// GeoJSON asks for (RFC 7946). The map relies on this to draw the band on the inside.
fn ring(outline: &[(f64, f64)]) -> Vec<[f64; 2]> {
    // Five decimals are about 1 m: plenty for the map and a third less to transfer.
    let round = |value: f64| (value * 1e5).round() / 1e5;
    let mut ring: Vec<[f64; 2]> = outline
        .iter()
        .map(|&(lat, lon)| [round(lon), round(lat)])
        .collect();
    if signed_area(&ring) < 0.0 {
        ring.reverse();
    }
    if let Some(&first) = ring.first() {
        ring.push(first);
    }
    ring
}

/// Twice the area enclosed by the points, positive if they run counterclockwise (with
/// x = longitude to the east and y = latitude to the north).
fn signed_area(points: &[[f64; 2]]) -> f64 {
    let pairs = points.iter().zip(points.iter().cycle().skip(1));
    pairs.map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum()
}

/// A limit as the source wrote it, plus metres in parentheses: `FL95 (2900 m)`,
/// `4500 ft (1370 m)`, `1500 ft AGL (460 m)`. Ground, unlimited and metre values stay
/// as they are.
fn limit_label(limit: &Limit) -> String {
    let label = limit.label();
    match (limit, limit.metres()) {
        (Limit::FlightLevel(_), Some(metres)) => format!("{label} ({} m)", round_to_ten(metres)),
        (
            Limit::Height {
                unit: Unit::Feet, ..
            },
            Some(metres),
        ) if metres > 0.0 => {
            format!("{label} ({} m)", round_to_ten(metres))
        }
        _ => label,
    }
}

fn round_to_ten(metres: f64) -> i64 {
    ((metres / 10.0).round() * 10.0) as i64
}

/// The name the map style uses for `category`.
fn category_name(category: Category) -> &'static str {
    CATEGORIES
        .iter()
        .find(|(known, _)| *known == category)
        .map_or("other", |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTR: &str = "AC D\nAY CTR\nAN Example CTR\nAL GND\nAH 3500ft MSL\n\
        DP 47:40:00 N 011:20:00 E\nDP 47:30:00 N 011:30:00 E\nDP 47:40:00 N 011:30:00 E\n";

    #[test]
    fn hidden_categories_keep_only_known_names() {
        assert_eq!(parse_hidden("other\n tmz \nnonsense\n"), ["tmz", "other"]);
        assert_eq!(parse_hidden(""), Vec::<String>::new());
    }

    #[test]
    fn labels_show_the_source_unit_and_metres() {
        let label = |text| limit_label(&Limit::parse(text).unwrap());
        assert_eq!(label("GND"), "GND");
        assert_eq!(label("UNL"), "UNL");
        assert_eq!(label("FL95"), "FL95 (2900 m)");
        assert_eq!(label("4500ft MSL"), "4500 ft (1370 m)");
        assert_eq!(label("1500ft AGL"), "1500 ft AGL (460 m)");
        assert_eq!(label("1000m"), "1000 m");
    }

    #[test]
    fn rings_are_closed_and_counterclockwise() {
        // Clockwise as given: north, then east.
        let outline = [(47.0, 11.0), (48.0, 11.0), (48.0, 12.0)];
        let ring = ring(&outline);
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.first(), ring.last());
        assert!(signed_area(&ring[..3]) > 0.0);
        let mut reversed = outline;
        reversed.reverse();
        assert!(signed_area(&super::ring(&reversed)[..3]) > 0.0);
    }

    #[test]
    fn reads_the_folder_and_reports_each_file() {
        let dir = std::env::temp_dir().join(format!("vario-airspace-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.txt"), format!("{CTR}AC D\nAN Broken\n")).unwrap();
        std::fs::write(dir.join("a.TXT"), CTR).unwrap();
        std::fs::write(dir.join("notes.md"), "not an airspace file").unwrap();

        let airspaces = load_dir(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let names: Vec<&str> = airspaces.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.TXT", "b.txt"]);
        assert_eq!(airspaces.files[1].airspaces, 1);
        assert_eq!(airspaces.files[1].issue_count, 1);
        let features = airspaces.geojson["features"].as_array().unwrap();
        assert_eq!(features.len(), 2);
        let properties = &features[0]["properties"];
        assert_eq!(properties["category"], "ctr");
        assert_eq!(properties["upper"], "3500 ft (1070 m)");
        assert_eq!(properties["label"], "Example CTR 3500 ft (1070 m) / GND");
        let ring = features[0]["geometry"]["coordinates"][0]
            .as_array()
            .unwrap();
        assert_eq!(ring.len(), 4);
        assert_eq!(ring[0], json!([11.33333, 47.66667]));
    }
}
