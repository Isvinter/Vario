//! Airspaces read from OpenAir files.
//!
//! OpenAir is the plain text airspace format that most flight instruments read (XCTrack,
//! XCSoar, SeeYou, …); openAIP and other providers export it. An airspace starts with an
//! `AC` record, followed by its name, limits and outline:
//!
//! ```text
//! * comment
//! AC D                          class, or a type such as CTR, R, Q, P, RMZ, TMZ, GP
//! AY CTR                        type (extended format, optional)
//! AN Example CTR                name
//! AF 118.500                    frequency (extended format, optional)
//! AG Example Tower              station (extended format, optional)
//! AL GND                        lower limit
//! AH 3500ft MSL                 upper limit
//! DP 47:40:00 N 011:20:00 E     outline point
//! V X=47:35:00 N 011:30:00 E    centre of the following circle or arcs
//! V D=-                         arc direction: + clockwise (default), - counterclockwise
//! DC 2.5                        circle, radius in nautical miles
//! DA 5,270,90                   arc: radius (NM), start and end bearing in degrees
//! DB 47:40:00 N 011:25:00 E, 47:30:00 N 011:25:00 E   arc between two points
//! ```
//!
//! Circles and arcs become outline points here, so users of this module only see
//! polygons. The reader is lenient: a broken airspace is skipped and reported as an
//! [`Issue`], the rest of the file is still used.

use crate::geo;

/// One foot in metres.
pub const FOOT_M: f64 = 0.3048;
/// One nautical mile in metres.
const NAUTICAL_MILE_M: f64 = 1852.0;
/// Angle between two outline points on circles and arcs. 2° keeps the outline within
/// about 6 m of the true circle for a radius of 20 NM.
const ARC_STEP_DEG: f64 = 2.0;

/// Unit an altitude was given in; kept so that it can be shown the way the source wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Feet,
    Metres,
}

/// What an altitude is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    /// Above mean sea level.
    Msl,
    /// Above ground.
    Agl,
}

/// Lower or upper limit of an airspace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Limit {
    /// The ground (GND, SFC).
    Ground,
    /// A height in metres above sea level or above ground.
    Height {
        metres: f64,
        unit: Unit,
        reference: Reference,
    },
    /// A flight level: hundreds of feet of pressure altitude at standard pressure
    /// (1013.25 hPa).
    FlightLevel(u16),
    /// No upper limit.
    Unlimited,
}

impl Limit {
    /// Reads a limit as written in an OpenAir file, e.g. `GND`, `FL95`, `4500ft MSL`,
    /// `1500 ft AGL`, `1000m`, `UNL`. A number without unit is in feet, a number without
    /// reference is above sea level.
    pub fn parse(text: &str) -> Result<Limit, String> {
        let upper = text.trim().to_uppercase();
        let invalid = || format!("invalid altitude limit '{}'", text.trim());

        if let Some(level) = upper.strip_prefix("FL") {
            return level
                .trim()
                .parse()
                .map(Limit::FlightLevel)
                .map_err(|_| invalid());
        }
        let number_end = upper
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(upper.len());
        if number_end == 0 {
            return match upper.as_str() {
                "GND" | "SFC" | "AGL" => Ok(Limit::Ground),
                "MSL" => Ok(Limit::Height {
                    metres: 0.0,
                    unit: Unit::Feet,
                    reference: Reference::Msl,
                }),
                "UNL" | "UNLIM" | "UNLTD" | "UNLIMITED" => Ok(Limit::Unlimited),
                _ => Err(invalid()),
            };
        }

        let value: f64 = upper[..number_end].parse().map_err(|_| invalid())?;
        let rest = upper[number_end..].trim_start();
        // "M" is metres, but "MSL" is a reference with the default unit (feet).
        let (unit, rest) = if let Some(rest) = rest.strip_prefix("FT") {
            (Unit::Feet, rest)
        } else if rest.starts_with('M') && !rest.starts_with("MSL") {
            (Unit::Metres, &rest[1..])
        } else if rest == "F" || rest.starts_with("F ") {
            (Unit::Feet, &rest[1..])
        } else {
            (Unit::Feet, rest)
        };
        let metres = match unit {
            Unit::Feet => value * FOOT_M,
            Unit::Metres => value,
        };
        let height = |reference| Limit::Height {
            metres,
            unit,
            reference,
        };
        match rest.trim() {
            // A bare 0 is the ground.
            "" if value == 0.0 => Ok(Limit::Ground),
            "" | "MSL" | "AMSL" | "ALT" => Ok(height(Reference::Msl)),
            "AGL" | "GND" | "SFC" | "ASFC" | "AGND" => Ok(height(Reference::Agl)),
            // Pressure altitude at standard pressure, the same as a flight level.
            "STD" => Ok(Limit::FlightLevel((value / 100.0).round() as u16)),
            _ => Err(invalid()),
        }
    }

    /// Short label in the unit of the source, e.g. `GND`, `FL95`, `4500 ft`,
    /// `1500 ft AGL`, `1000 m`, `UNL`.
    pub fn label(&self) -> String {
        match *self {
            Limit::Ground => "GND".to_string(),
            Limit::FlightLevel(level) => format!("FL{level}"),
            Limit::Unlimited => "UNL".to_string(),
            Limit::Height {
                metres,
                unit,
                reference,
            } => {
                let value = match unit {
                    Unit::Feet => format!("{} ft", (metres / FOOT_M).round()),
                    Unit::Metres => format!("{} m", metres.round()),
                };
                match reference {
                    Reference::Msl => value,
                    Reference::Agl => format!("{value} AGL"),
                }
            }
        }
    }

    /// The limit in metres: above its reference for heights, the pressure altitude at
    /// standard pressure for flight levels, 0 for the ground and `None` without limit.
    pub fn metres(&self) -> Option<f64> {
        match *self {
            Limit::Ground => Some(0.0),
            Limit::Height { metres, .. } => Some(metres),
            Limit::FlightLevel(level) => Some(f64::from(level) * 100.0 * FOOT_M),
            Limit::Unlimited => None,
        }
    }
}

/// Rough kind of an airspace, for colours and filters on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Control zone around an airport.
    Ctr,
    /// Controlled airspace of class A to D (including TMA/CTA).
    Controlled,
    /// Class E.
    ClassE,
    Restricted,
    Danger,
    /// Prohibited, also "no gliders" (GP).
    Prohibited,
    /// Transponder mandatory zone.
    Tmz,
    /// Radio mandatory zone.
    Rmz,
    /// Gliding sectors and wave windows.
    GliderSector,
    /// Temporarily reserved for an activity (TRA, TSA). In Austria these are mostly
    /// areas for gliders, hang gliders and paragliders inside controlled airspace.
    Reserved,
    /// Everything else: information sectors, military low flying areas, FIR, ….
    Other,
}

/// One airspace with its outline.
#[derive(Debug, Clone, PartialEq)]
pub struct Airspace {
    /// Class from the `AC` record, upper case (`D`, `CTR`, `R`, …).
    pub class: String,
    /// Type from the `AY` record of the extended format, upper case.
    pub kind: Option<String>,
    pub name: String,
    pub frequency: Option<String>,
    pub station: Option<String>,
    pub lower: Limit,
    pub upper: Limit,
    /// Outline as `(latitude_deg, longitude_deg)`; the first point is not repeated at the end.
    pub outline: Vec<(f64, f64)>,
}

impl Airspace {
    /// The kind of airspace, from the type (`AY`) if it is known, otherwise from the class.
    pub fn category(&self) -> Category {
        self.kind
            .as_deref()
            .and_then(category_of_kind)
            .unwrap_or_else(|| category_of_class(&self.class))
    }

    /// Whether a position lies inside the outline (horizontally; the limits are not
    /// checked). Treats latitude and longitude as plane coordinates, which is exact enough
    /// for airspaces of a few dozen kilometres away from the poles and the 180° meridian.
    pub fn contains(&self, lat_deg: f64, lon_deg: f64) -> bool {
        // Ray casting: count how often a line from the position to the east crosses the
        // outline; an odd count means inside.
        let mut inside = false;
        let points = &self.outline;
        for (i, &(lat_a, lon_a)) in points.iter().enumerate() {
            let (lat_b, lon_b) = points[(i + 1) % points.len()];
            if (lat_a > lat_deg) != (lat_b > lat_deg) {
                let lon_cross = lon_a + (lat_deg - lat_a) / (lat_b - lat_a) * (lon_b - lon_a);
                if lon_cross > lon_deg {
                    inside = !inside;
                }
            }
        }
        inside
    }
}

/// Category from the type (`AY`) of the OpenAir 2 format; `None` for types such as TMA
/// or NONE, where the ICAO class says more.
fn category_of_kind(kind: &str) -> Option<Category> {
    let category = match kind {
        "CTR" => Category::Ctr,
        "TMZ" => Category::Tmz,
        // Zones where radio contact is required.
        "RMZ" | "ATZ" | "MATZ" | "TIZ" | "TIA" | "HTZ" => Category::Rmz,
        "P" => Category::Prohibited,
        // OFR: overflight restrictions, e.g. above national parks in Austria.
        "R" | "OFR" | "TFR" => Category::Restricted,
        // ASRA: parachute jumping areas.
        "Q" | "ASRA" | "ALERT" | "WARNING" => Category::Danger,
        "GSEC" => Category::GliderSector,
        "TRA" | "TRAFR" | "TSA" => Category::Reserved,
        "FIS" | "MTA" | "MTR" | "LTA" | "VFRSEC" | "VFRR" | "AWY" | "ADIZ" | "TRZ" => {
            Category::Other
        }
        _ => return None,
    };
    Some(category)
}

/// Category from the class (`AC`); also understands the types that older OpenAir files
/// put there (CTR, R, Q, P, GP, …).
fn category_of_class(class: &str) -> Category {
    match class {
        "CTR" => Category::Ctr,
        "A" | "B" | "C" | "D" => Category::Controlled,
        "E" => Category::ClassE,
        "R" => Category::Restricted,
        "Q" => Category::Danger,
        "P" | "GP" => Category::Prohibited,
        "TMZ" => Category::Tmz,
        "RMZ" => Category::Rmz,
        "W" | "GSEC" => Category::GliderSector,
        _ => Category::Other,
    }
}

/// Something in the file that could not be used.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    /// Line number, starting at 1.
    pub line: usize,
    pub message: String,
}

/// Everything read from one file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AirspaceFile {
    pub airspaces: Vec<Airspace>,
    pub issues: Vec<Issue>,
}

/// Reads an OpenAir file. UTF-8 is expected; files in Latin-1 (older exports with
/// umlauts) are read as well.
pub fn parse(bytes: &[u8]) -> AirspaceFile {
    let text = decode(bytes);
    let mut file = AirspaceFile::default();
    let mut draft: Option<Draft> = None;

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let content = raw.trim();
        if content.is_empty() || content.starts_with('*') {
            continue;
        }
        let (record, rest) = match content.split_once(char::is_whitespace) {
            Some((record, rest)) => (record.to_uppercase(), rest.trim()),
            None => (content.to_uppercase(), ""),
        };

        if record == "AC" {
            if let Some(previous) = draft.take() {
                previous.finish(&mut file);
            }
            draft = Some(Draft::new(line, rest));
            continue;
        }
        let Some(current) = draft.as_mut() else {
            file.issues.push(Issue {
                line,
                message: format!("record '{record}' outside of an airspace (missing AC)"),
            });
            continue;
        };
        match current.apply(&record, rest) {
            Ok(Record::Used) => {}
            Ok(Record::Unknown) => file.issues.push(Issue {
                line,
                message: format!("unknown record '{record}' ignored"),
            }),
            Err(message) => {
                current.broken = true;
                file.issues.push(Issue { line, message });
            }
        }
    }
    if let Some(last) = draft {
        last.finish(&mut file);
    }
    file
}

/// UTF-8 if valid, otherwise Latin-1 (every byte is one character).
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&byte| char::from(byte)).collect(),
    }
}

/// Whether a record was understood.
enum Record {
    Used,
    /// Not part of the format as far as this reader knows; the airspace is kept.
    Unknown,
}

/// An airspace while its records are being read.
struct Draft {
    line: usize,
    class: String,
    kind: Option<String>,
    name: String,
    frequency: Option<String>,
    station: Option<String>,
    lower: Option<Limit>,
    upper: Option<Limit>,
    outline: Vec<(f64, f64)>,
    centre: Option<(f64, f64)>,
    clockwise: bool,
    /// A record could not be read, so the airspace is incomplete and gets skipped.
    broken: bool,
}

impl Draft {
    fn new(line: usize, class: &str) -> Draft {
        Draft {
            line,
            class: class.to_uppercase(),
            kind: None,
            name: String::new(),
            frequency: None,
            station: None,
            lower: None,
            upper: None,
            outline: Vec::new(),
            centre: None,
            clockwise: true,
            broken: false,
        }
    }

    /// Uses one record; an error means the airspace is incomplete.
    fn apply(&mut self, record: &str, rest: &str) -> Result<Record, String> {
        let text = || (!rest.is_empty()).then(|| rest.to_string());
        match record {
            "AN" => self.name = rest.to_string(),
            "AY" => self.kind = text().map(|kind| kind.to_uppercase()),
            "AF" => self.frequency = text(),
            "AG" => self.station = text(),
            "AL" => self.lower = Some(Limit::parse(rest)?),
            "AH" => self.upper = Some(Limit::parse(rest)?),
            "DP" => self.outline.push(parse_coordinate(rest)?),
            "V" => self.set_variable(rest)?,
            "DC" => {
                let centre = self.centre()?;
                let radius = parse_number(rest, "circle radius")? * NAUTICAL_MILE_M;
                let steps = (360.0 / ARC_STEP_DEG) as usize;
                self.outline.extend((0..steps).map(|step| {
                    geo::destination(centre.0, centre.1, step as f64 * ARC_STEP_DEG, radius)
                }));
            }
            "DA" => {
                let centre = self.centre()?;
                let values: Vec<&str> = rest.split(',').collect();
                let [radius, start, end] = values[..] else {
                    return Err(format!("arc needs radius, start and end angle: '{rest}'"));
                };
                let radius = parse_number(radius, "arc radius")? * NAUTICAL_MILE_M;
                let start = parse_number(start, "arc start angle")?;
                let end = parse_number(end, "arc end angle")?;
                self.outline
                    .extend(arc(centre, radius, start, end, self.clockwise));
            }
            "DB" => {
                let centre = self.centre()?;
                let Some((from, to)) = rest.split_once(',') else {
                    return Err(format!("arc needs two points: '{rest}'"));
                };
                let (from, to) = (parse_coordinate(from)?, parse_coordinate(to)?);
                let radius = geo::distance_m(centre.0, centre.1, from.0, from.1);
                let start = geo::bearing_deg(centre.0, centre.1, from.0, from.1);
                let end = geo::bearing_deg(centre.0, centre.1, to.0, to.1);
                let mut points = arc(centre, radius, start, end, self.clockwise);
                // The end point may lie at a slightly different radius; use both exactly.
                if let Some(first) = points.first_mut() {
                    *first = from;
                }
                if let Some(last) = points.last_mut() {
                    *last = to;
                }
                self.outline.extend(points);
            }
            "DY" => return Err("airways (DY) are not supported".to_string()),
            // Label positions, display styles, transponder codes and activation times
            // are not used (yet).
            "AT" | "SP" | "SB" | "AX" | "AA" => {}
            _ => return Ok(Record::Unknown),
        }
        Ok(Record::Used)
    }

    fn set_variable(&mut self, rest: &str) -> Result<(), String> {
        let Some((name, value)) = rest.split_once('=') else {
            return Err(format!("invalid variable '{rest}'"));
        };
        match name.trim().to_uppercase().as_str() {
            "X" => self.centre = Some(parse_coordinate(value)?),
            "D" => {
                self.clockwise = match value.trim() {
                    "+" => true,
                    "-" => false,
                    other => return Err(format!("invalid arc direction '{other}'")),
                }
            }
            // Airway width (W) and other variables are not used.
            _ => {}
        }
        Ok(())
    }

    fn centre(&self) -> Result<(f64, f64), String> {
        self.centre
            .ok_or_else(|| "circle or arc without centre (V X=)".to_string())
    }

    /// Adds the finished airspace to `file`, or reports why it is skipped.
    fn finish(mut self, file: &mut AirspaceFile) {
        let what = if self.name.is_empty() {
            "airspace".to_string()
        } else {
            format!("airspace '{}'", self.name)
        };
        let mut skip = |reason: &str| {
            file.issues.push(Issue {
                line: self.line,
                message: format!("{what} skipped: {reason}"),
            });
        };
        if self.broken {
            return skip("it contains errors");
        }
        let (Some(lower), Some(upper)) = (self.lower, self.upper) else {
            return skip("lower or upper limit missing");
        };
        if self.outline.len() > 1 && self.outline.first() == self.outline.last() {
            self.outline.pop();
        }
        if self.outline.len() < 3 {
            return skip("fewer than 3 outline points");
        }
        file.airspaces.push(Airspace {
            class: self.class,
            kind: self.kind,
            name: self.name,
            frequency: self.frequency,
            station: self.station,
            lower,
            upper,
            outline: self.outline,
        });
    }
}

/// Points on an arc around `centre` from bearing `start` to `end`, both ends included.
fn arc(
    centre: (f64, f64),
    radius_m: f64,
    start: f64,
    end: f64,
    clockwise: bool,
) -> Vec<(f64, f64)> {
    let (sweep, direction) = if clockwise {
        ((end - start).rem_euclid(360.0), 1.0)
    } else {
        ((start - end).rem_euclid(360.0), -1.0)
    };
    let steps = (sweep / ARC_STEP_DEG).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|step| {
            let bearing = start + direction * sweep * step as f64 / steps as f64;
            geo::destination(centre.0, centre.1, bearing, radius_m)
        })
        .collect()
}

fn parse_number(text: &str, what: &str) -> Result<f64, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("invalid {what} '{}'", text.trim()))
}

/// Reads a position like `47:30:00 N 011:20:30 E`, `47:30.5N 11:20.5E` or
/// `47:30:00.5 N 011:20:00 W`. Returns `(latitude_deg, longitude_deg)`.
pub fn parse_coordinate(text: &str) -> Result<(f64, f64), String> {
    let upper = text.trim().to_uppercase();
    let invalid = || format!("invalid coordinate '{}'", text.trim());

    let lat_end = upper.find(['N', 'S']).ok_or_else(invalid)?;
    let lon_text = &upper[lat_end + 1..];
    let lon_end = lon_text.find(['E', 'W']).ok_or_else(invalid)?;
    if !lon_text[lon_end + 1..].trim().is_empty() {
        return Err(invalid());
    }
    let latitude = angle(&upper[..lat_end])
        .filter(|lat| *lat <= 90.0)
        .ok_or_else(invalid)?;
    let longitude = angle(&lon_text[..lon_end])
        .filter(|lon| *lon <= 180.0)
        .ok_or_else(invalid)?;
    let latitude = if upper[lat_end..].starts_with('S') {
        -latitude
    } else {
        latitude
    };
    let longitude = if lon_text[lon_end..].starts_with('W') {
        -longitude
    } else {
        longitude
    };
    Ok((latitude, longitude))
}

/// Degrees from `DD:MM:SS`, `DD:MM.mmm` or `DD` (also separated by spaces).
fn angle(text: &str) -> Option<f64> {
    let parts: Vec<f64> = text
        .split([':', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    let (degrees, minutes, seconds) = match parts[..] {
        [d] => (d, 0.0, 0.0),
        [d, m] => (d, m, 0.0),
        [d, m, s] => (d, m, s),
        _ => return None,
    };
    let valid = degrees >= 0.0 && (0.0..60.0).contains(&minutes) && (0.0..60.0).contains(&seconds);
    valid.then(|| degrees + minutes / 60.0 + seconds / 3600.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feet(value: f64, reference: Reference) -> Limit {
        Limit::Height {
            metres: value * FOOT_M,
            unit: Unit::Feet,
            reference,
        }
    }

    #[test]
    fn reads_the_usual_limit_spellings() {
        let cases = [
            ("GND", Limit::Ground),
            ("SFC", Limit::Ground),
            ("0", Limit::Ground),
            ("FL95", Limit::FlightLevel(95)),
            ("FL 100", Limit::FlightLevel(100)),
            ("UNL", Limit::Unlimited),
            ("UNLTD", Limit::Unlimited),
            ("4500ft MSL", feet(4500.0, Reference::Msl)),
            ("4500 FT AMSL", feet(4500.0, Reference::Msl)),
            ("4500MSL", feet(4500.0, Reference::Msl)),
            ("4500", feet(4500.0, Reference::Msl)),
            ("3500 ALT", feet(3500.0, Reference::Msl)),
            ("1500ft AGL", feet(1500.0, Reference::Agl)),
            ("1500 ft GND", feet(1500.0, Reference::Agl)),
            ("1500F SFC", feet(1500.0, Reference::Agl)),
            ("10000ft STD", Limit::FlightLevel(100)),
            (
                "1000m",
                Limit::Height {
                    metres: 1000.0,
                    unit: Unit::Metres,
                    reference: Reference::Msl,
                },
            ),
            (
                "300 M AGL",
                Limit::Height {
                    metres: 300.0,
                    unit: Unit::Metres,
                    reference: Reference::Agl,
                },
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(Limit::parse(text), Ok(expected), "{text}");
        }
        for text in ["", "FLX", "high", "1500 ft above"] {
            assert!(Limit::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn labels_use_the_unit_of_the_source() {
        let labels = [
            "GND",
            "FL95",
            "UNL",
            "4500 ft",
            "1500 ft AGL",
            "1000 m",
            "300 m AGL",
        ];
        for label in labels {
            let limit = Limit::parse(&label.replace(" m", "m")).unwrap();
            assert_eq!(limit.label(), label);
        }
        assert!((Limit::FlightLevel(100).metres().unwrap() - 3048.0).abs() < 1e-9);
        assert_eq!(Limit::Unlimited.metres(), None);
    }

    #[test]
    fn categories_of_openair_2_types() {
        let airspace = |class: &str, kind: Option<&str>| Airspace {
            class: class.to_string(),
            kind: kind.map(str::to_string),
            name: String::new(),
            frequency: None,
            station: None,
            lower: Limit::Ground,
            upper: Limit::Unlimited,
            outline: Vec::new(),
        };
        let cases = [
            ("D", Some("CTR"), Category::Ctr),
            ("C", Some("TMA"), Category::Controlled),
            ("E", Some("TMA"), Category::ClassE),
            ("E", Some("TMZ"), Category::Tmz),
            ("E", Some("NONE"), Category::ClassE),
            ("UNC", Some("R"), Category::Restricted),
            ("UNC", Some("OFR"), Category::Restricted),
            ("UNC", Some("ASRA"), Category::Danger),
            ("UNC", Some("GSEC"), Category::GliderSector),
            ("UNC", Some("TRA"), Category::Reserved),
            ("UNC", Some("FIS"), Category::Other),
            ("UNC", Some("SOMETHING NEW"), Category::Other),
            ("R", None, Category::Restricted),
        ];
        for (class, kind, expected) in cases {
            assert_eq!(
                airspace(class, kind).category(),
                expected,
                "{class} {kind:?}"
            );
        }
    }

    #[test]
    fn finds_positions_inside_the_outline() {
        // An L shape: the notch at the north-east is outside.
        let file = parse(
            b"AC D\nAN L\nAL GND\nAH FL100\n\
              DP 47:00:00 N 011:00:00 E\nDP 47:00:00 N 011:20:00 E\nDP 47:10:00 N 011:20:00 E\n\
              DP 47:10:00 N 011:10:00 E\nDP 47:20:00 N 011:10:00 E\nDP 47:20:00 N 011:00:00 E\n",
        );
        let l = &file.airspaces[0];
        assert!(l.contains(47.05, 11.05));
        assert!(l.contains(47.05, 11.3 - 0.15));
        assert!(l.contains(47.3, 11.05));
        assert!(!l.contains(47.25, 11.25), "in the notch");
        assert!(!l.contains(46.9, 11.05), "south of it");
        assert!(!l.contains(47.05, 11.4), "east of it");
        // A circle: inside at 2 NM, outside at 3 NM from the centre.
        let file = parse(b"AC R\nAN C\nAL GND\nAH FL100\nV X=47:30:00 N 011:30:00 E\nDC 2.5\n");
        let circle = &file.airspaces[0];
        let near = geo::destination(47.5, 11.5, 30.0, 2.0 * NAUTICAL_MILE_M);
        let far = geo::destination(47.5, 11.5, 30.0, 3.0 * NAUTICAL_MILE_M);
        assert!(circle.contains(near.0, near.1));
        assert!(!circle.contains(far.0, far.1));
    }

    #[test]
    fn reads_coordinates() {
        let cases = [
            ("47:30:00 N 011:20:00 E", (47.5, 11.0 + 1.0 / 3.0)),
            ("47:30:00N 011:20:00E", (47.5, 11.0 + 1.0 / 3.0)),
            (
                "47:30.5 N 11:20.5 E",
                (47.0 + 30.5 / 60.0, 11.0 + 20.5 / 60.0),
            ),
            ("47:30:36.0 S 011:00:00 W", (-47.51, -11.0)),
            ("47 30 00 N 011 20 00 E", (47.5, 11.0 + 1.0 / 3.0)),
        ];
        for (text, (lat, lon)) in cases {
            let (got_lat, got_lon) = parse_coordinate(text).unwrap();
            assert!(
                (got_lat - lat).abs() < 1e-9 && (got_lon - lon).abs() < 1e-9,
                "{text}"
            );
        }
        for text in [
            "",
            "47:30:00 011:20:00 E",
            "47:70:00 N 011:20:00 E",
            "47:30 N 11:20 E x",
        ] {
            assert!(parse_coordinate(text).is_err(), "{text}");
        }
    }

    #[test]
    fn reads_a_polygon_airspace() {
        let file = parse(
            b"* test file\n\
              AC D\n\
              AY CTR\n\
              AN Example CTR\n\
              AF 118.500\n\
              AG Example Tower\n\
              AL GND\n\
              AH 3500ft MSL\n\
              SP 0,1,0,0,255\n\
              AA 2026-01-01T00:01:00Z/2026-12-31T23:00:00Z\n\
              DP 47:40:00 N 011:20:00 E\n\
              DP 47:40:00 N 011:30:00 E\n\
              DP 47:30:00 N 011:30:00 E\n\
              DP 47:40:00 N 011:20:00 E\n",
        );
        assert_eq!(file.issues, []);
        let [airspace] = &file.airspaces[..] else {
            panic!("{:?}", file.airspaces);
        };
        assert_eq!(airspace.class, "D");
        assert_eq!(airspace.kind.as_deref(), Some("CTR"));
        assert_eq!(airspace.name, "Example CTR");
        assert_eq!(airspace.frequency.as_deref(), Some("118.500"));
        assert_eq!(airspace.station.as_deref(), Some("Example Tower"));
        assert_eq!(airspace.lower, Limit::Ground);
        assert_eq!(airspace.upper, feet(3500.0, Reference::Msl));
        assert_eq!(airspace.category(), Category::Ctr);
        // The repeated first point is dropped.
        assert_eq!(airspace.outline.len(), 3);
    }

    fn distance_from(centre: (f64, f64), point: (f64, f64)) -> f64 {
        geo::distance_m(centre.0, centre.1, point.0, point.1)
    }

    #[test]
    fn turns_a_circle_into_points_at_the_radius() {
        let file =
            parse(b"AC R\nAN Circle\nAL GND\nAH FL100\nV X=47:35:00 N 011:30:00 E\nDC 2.5\n");
        assert_eq!(file.issues, []);
        let airspace = &file.airspaces[0];
        assert_eq!(airspace.category(), Category::Restricted);
        assert_eq!(airspace.outline.len(), 180);
        let centre = (47.0 + 35.0 / 60.0, 11.5);
        for point in &airspace.outline {
            assert!((distance_from(centre, *point) - 2.5 * NAUTICAL_MILE_M).abs() < 0.01);
        }
    }

    #[test]
    fn arcs_follow_the_direction() {
        let centre = (47.5, 11.5);
        let bearings = |file: &AirspaceFile| -> Vec<f64> {
            file.airspaces[0]
                .outline
                .iter()
                .map(|p| geo::bearing_deg(centre.0, centre.1, p.0, p.1))
                .collect()
        };
        // Clockwise from west to east goes through north …
        let clockwise =
            parse(b"AC Q\nAN Arc\nAL GND\nAH FL100\nV X=47:30:00 N 011:30:00 E\nDA 3,270,90\n");
        assert_eq!(clockwise.issues, []);
        let b = bearings(&clockwise);
        assert!((b[0] - 270.0).abs() < 1e-6);
        assert!(b.iter().any(|b| b.abs() < 1e-6 || (b - 360.0).abs() < 1e-6));
        assert!(!b.iter().any(|b| (b - 180.0).abs() < 1.0));
        // … counterclockwise through south.
        let counterclockwise = parse(
            b"AC Q\nAN Arc\nAL GND\nAH FL100\nV X=47:30:00 N 011:30:00 E\nV D=-\nDA 3,270,90\n",
        );
        let b = bearings(&counterclockwise);
        assert!(b.iter().any(|b| (b - 180.0).abs() < 1e-6));
        assert!(!b.iter().any(|b| b.abs() < 1.0));
    }

    #[test]
    fn arc_between_two_points_keeps_both_points() {
        let file = parse(
            b"AC C\nAN Arc\nAL 2500ft\nAH FL100\nV X=47:30:00 N 011:30:00 E\n\
              DB 47:33:00 N 011:30:00 E, 47:27:00 N 011:30:00 E\n",
        );
        assert_eq!(file.issues, []);
        let outline = &file.airspaces[0].outline;
        let from = parse_coordinate("47:33:00 N 011:30:00 E").unwrap();
        let to = parse_coordinate("47:27:00 N 011:30:00 E").unwrap();
        assert_eq!(outline.first(), Some(&from));
        assert_eq!(outline.last(), Some(&to));
        // Clockwise from north to south: all points east of the centre.
        let radius = distance_from((47.5, 11.5), from);
        for point in &outline[1..outline.len() - 1] {
            assert!(point.1 > 11.5);
            assert!((distance_from((47.5, 11.5), *point) - radius).abs() < 0.01);
        }
    }

    #[test]
    fn skips_broken_airspaces_and_keeps_the_rest() {
        let file = parse(
            b"AC D\nAN Broken\nAL GND\nAH somewhere\nDP 47:40:00 N 011:20:00 E\nDP 47:40:00 N 011:30:00 E\nDP 47:30:00 N 011:30:00 E\n\
              AC D\nAN No limits\nDP 47:40:00 N 011:20:00 E\nDP 47:40:00 N 011:30:00 E\nDP 47:30:00 N 011:30:00 E\n\
              AC D\nAN Good\nAL GND\nAH FL65\nZZ unknown\nDP 47:40:00 N 011:20:00 E\nDP 47:40:00 N 011:30:00 E\nDP 47:30:00 N 011:30:00 E\n",
        );
        let names: Vec<&str> = file.airspaces.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Good"]);
        // Broken limit, its airspace skipped, missing limits, unknown record.
        let lines: Vec<usize> = file.issues.iter().map(|issue| issue.line).collect();
        assert_eq!(lines, [4, 1, 8, 17]);
    }

    #[test]
    fn reads_latin_1_and_resets_the_arc_direction() {
        let mut bytes =
            b"AC E\nAN M\xFCnchen\nAL 2500ft\nAH FL100\nV X=47:30:00 N 011:30:00 E\nV D=-\nDC 1\n"
                .to_vec();
        bytes.extend_from_slice(b"AC E\nAN Second\nAL GND\nAH FL100\nV X=47:30:00 N 011:30:00 E\nDA 1,0,90\nDP 47:30:00 N 011:30:00 E\n");
        let file = parse(&bytes);
        assert_eq!(file.issues, []);
        assert_eq!(file.airspaces[0].name, "München");
        assert_eq!(file.airspaces[0].category(), Category::ClassE);
        // The second airspace starts clockwise again: its arc stays between 0° and 90°
        // (counterclockwise it would run through the other three quarters).
        let centre = (47.5, 11.5);
        for p in &file.airspaces[1].outline {
            let bearing = geo::bearing_deg(centre.0, centre.1, p.0, p.1);
            assert!(bearing < 90.0 + 1e-6 || bearing > 360.0 - 1e-6, "{bearing}");
        }
    }
}
