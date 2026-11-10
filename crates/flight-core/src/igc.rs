//! Reading and writing IGC flight recorder files.
//!
//! An IGC file is plain text, one record per line. The first letter of a line tells the
//! record type. Only two things are interpreted here:
//!
//! * `B` records, the position fixes;
//! * the date header (`HFDTE...`).
//!
//! Every other line is kept verbatim, so a file that is read and written again comes out
//! unchanged. Files written by this app carry no valid security signature (`G` record)
//! and are therefore not accepted where a certified recorder is required.

use std::fmt;

use thiserror::Error;

use crate::time::{Date, TimeOfDay};

/// Line ending required by the IGC specification.
const LINE_ENDING: &str = "\r\n";

/// Length of the fixed part of a B record.
const FIX_RECORD_LEN: usize = 35;

/// A complete IGC file as a list of records in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct IgcFile {
    pub records: Vec<Record>,
}

/// One line of an IGC file.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    /// A `B` record.
    Fix(IgcFix),
    /// Any other record, stored exactly as it appeared (without the line ending).
    Other(String),
}

/// A position fix (`B` record).
#[derive(Debug, Clone, PartialEq)]
pub struct IgcFix {
    /// UTC time of day. The date comes from the file's date header.
    pub time: TimeOfDay,
    /// Latitude in degrees, north positive. IGC resolution is 1/1000 arc minute.
    pub latitude_deg: f64,
    /// Longitude in degrees, east positive.
    pub longitude_deg: f64,
    /// Fix validity: `true` is written as `A` (3D fix), `false` as `V` (2D or no fix).
    pub fix_3d: bool,
    /// Pressure altitude in metres (ISA, 1013.25 hPa). 0 if there is no pressure sensor.
    pub pressure_altitude_m: i32,
    /// GPS altitude in metres.
    pub gps_altitude_m: i32,
    /// Optional extra fields after column 35 (defined by the file's `I` record), verbatim.
    pub extension: String,
}

/// Descriptive data written into the header of a new file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlightMeta {
    pub pilot: String,
    pub glider_type: String,
    pub glider_id: String,
}

/// A parse error together with the (1-based) line it occurred on.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("IGC line {line}: {kind}")]
pub struct IgcError {
    pub line: usize,
    pub kind: IgcErrorKind,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum IgcErrorKind {
    #[error("B record is shorter than {FIX_RECORD_LEN} characters")]
    FixTooShort,
    #[error("invalid {0} in B record")]
    InvalidField(&'static str),
}

impl IgcFile {
    /// Creates a file containing only the header records for a new flight.
    pub fn with_header(date: Date, meta: &FlightMeta) -> Self {
        let lines = [
            // "XXX" is the manufacturer code reserved for non-approved recorders.
            "AXXXVAR".to_owned(),
            format!(
                "HFDTEDATE:{:02}{:02}{:02},01",
                date.day(),
                date.month(),
                date.year().rem_euclid(100)
            ),
            format!("HFPLTPILOTINCHARGE:{}", single_line(&meta.pilot)),
            format!("HFGTYGLIDERTYPE:{}", single_line(&meta.glider_type)),
            format!("HFGIDGLIDERID:{}", single_line(&meta.glider_id)),
            "HFDTMGPSDATUM:WGS84".to_owned(),
            "HFFTYFRTYPE:Vario,prototype".to_owned(),
        ];
        IgcFile {
            records: lines.into_iter().map(Record::Other).collect(),
        }
    }

    /// Parses the text of an IGC file. Empty lines are skipped.
    pub fn parse(text: &str) -> Result<Self, IgcError> {
        let mut records = Vec::new();
        // `lines()` splits at "\n" and also strips a preceding "\r".
        for (index, line) in text.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let record = if line.starts_with('B') {
                let fix = parse_fix(line).map_err(|kind| IgcError {
                    line: index + 1,
                    kind,
                })?;
                Record::Fix(fix)
            } else {
                Record::Other(line.to_owned())
            };
            records.push(record);
        }
        Ok(IgcFile { records })
    }

    /// Parses the raw bytes of an IGC file.
    ///
    /// The specification only allows ASCII, but real files sometimes contain names in
    /// Latin-1. If the bytes are not valid UTF-8 they are interpreted as Latin-1.
    pub fn parse_bytes(bytes: &[u8]) -> Result<Self, IgcError> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::parse(text),
            Err(_) => {
                // Every Latin-1 byte maps to the Unicode code point of the same number.
                let text: String = bytes.iter().map(|&byte| char::from(byte)).collect();
                Self::parse(&text)
            }
        }
    }

    /// The flight date from the date header, if the file has a valid one.
    pub fn date(&self) -> Option<Date> {
        self.records.iter().find_map(|record| match record {
            Record::Other(line) => parse_date_header(line),
            Record::Fix(_) => None,
        })
    }

    /// All position fixes in file order.
    pub fn fixes(&self) -> impl Iterator<Item = &IgcFix> {
        self.records.iter().filter_map(|record| match record {
            Record::Fix(fix) => Some(fix),
            Record::Other(_) => None,
        })
    }
}

/// Formats the file as IGC text. Because `Display` is implemented, `file.to_string()`
/// also works.
impl fmt::Display for IgcFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for record in &self.records {
            match record {
                Record::Fix(fix) => write!(f, "{fix}")?,
                Record::Other(line) => f.write_str(line)?,
            }
            f.write_str(LINE_ENDING)?;
        }
        Ok(())
    }
}

/// Formats the fix as a B record without line ending.
impl fmt::Display for IgcFix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (lat_deg, lat_min) = split_angle(self.latitude_deg, 90);
        let (lon_deg, lon_min) = split_angle(self.longitude_deg, 180);
        write!(
            f,
            "B{:02}{:02}{:02}{:02}{:05}{}{:03}{:05}{}{}{:05}{:05}{}",
            self.time.hour(),
            self.time.minute(),
            self.time.second(),
            lat_deg,
            lat_min,
            // `is_sign_negative` instead of `< 0.0` so that "0000000S" survives a
            // round trip (it parses to -0.0).
            if self.latitude_deg.is_sign_negative() {
                'S'
            } else {
                'N'
            },
            lon_deg,
            lon_min,
            if self.longitude_deg.is_sign_negative() {
                'W'
            } else {
                'E'
            },
            if self.fix_3d { 'A' } else { 'V' },
            // Five columns including a possible minus sign.
            self.pressure_altitude_m.clamp(-9999, 99_999),
            self.gps_altitude_m.clamp(-9999, 99_999),
            self.extension,
        )
    }
}

/// Splits an angle into whole degrees and thousandths of arc minutes, ignoring the sign.
fn split_angle(angle_deg: f64, max_deg: u32) -> (u32, u32) {
    const THOUSANDTHS_PER_DEGREE: f64 = 60_000.0;
    let total = (angle_deg.abs() * THOUSANDTHS_PER_DEGREE).round() as u32;
    let total = total.min(max_deg * 60_000);
    (total / 60_000, total % 60_000)
}

/// Removes characters that would break the one-record-per-line structure.
fn single_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn parse_fix(line: &str) -> Result<IgcFix, IgcErrorKind> {
    if line.len() < FIX_RECORD_LEN {
        return Err(IgcErrorKind::FixTooShort);
    }

    let time = TimeOfDay::new(
        number(line, 1, 3, "time")?,
        number(line, 3, 5, "time")?,
        number(line, 5, 7, "time")?,
    )
    .ok_or(IgcErrorKind::InvalidField("time"))?;

    let latitude_deg = parse_angle(line, 7, 9, 14, 90, ['N', 'S'], "latitude")?;
    let longitude_deg = parse_angle(line, 15, 18, 23, 180, ['E', 'W'], "longitude")?;

    let fix_3d = match field(line, 24, 25, "fix validity")? {
        "A" => true,
        "V" => false,
        _ => return Err(IgcErrorKind::InvalidField("fix validity")),
    };

    Ok(IgcFix {
        time,
        latitude_deg,
        longitude_deg,
        fix_3d,
        pressure_altitude_m: number(line, 25, 30, "pressure altitude")?,
        gps_altitude_m: number(line, 30, 35, "GPS altitude")?,
        extension: field(line, FIX_RECORD_LEN, line.len(), "extension")?.to_owned(),
    })
}

/// Parses `DDMMmmmH` / `DDDMMmmmH`: degrees from `start..minutes_start`, thousandths of
/// minutes up to `hemisphere_at`, then one hemisphere letter.
fn parse_angle(
    line: &str,
    start: usize,
    minutes_start: usize,
    hemisphere_at: usize,
    max_deg: u32,
    hemispheres: [char; 2],
    name: &'static str,
) -> Result<f64, IgcErrorKind> {
    let degrees: u32 = number(line, start, minutes_start, name)?;
    let minute_thousandths: u32 = number(line, minutes_start, hemisphere_at, name)?;
    let total = degrees * 60_000 + minute_thousandths;
    if minute_thousandths >= 60_000 || total > max_deg * 60_000 {
        return Err(IgcErrorKind::InvalidField(name));
    }
    let angle = f64::from(total) / 60_000.0;

    let hemisphere = field(line, hemisphere_at, hemisphere_at + 1, name)?;
    let [positive, negative] = hemispheres;
    if hemisphere.starts_with(positive) {
        Ok(angle)
    } else if hemisphere.starts_with(negative) {
        Ok(-angle)
    } else {
        Err(IgcErrorKind::InvalidField(name))
    }
}

/// The text in byte columns `start..end`. Fails instead of panicking if the range is out
/// of bounds or cuts through a multi-byte character.
fn field<'a>(
    line: &'a str,
    start: usize,
    end: usize,
    name: &'static str,
) -> Result<&'a str, IgcErrorKind> {
    line.get(start..end).ok_or(IgcErrorKind::InvalidField(name))
}

fn number<T: std::str::FromStr>(
    line: &str,
    start: usize,
    end: usize,
    name: &'static str,
) -> Result<T, IgcErrorKind> {
    field(line, start, end, name)?
        .parse()
        .map_err(|_| IgcErrorKind::InvalidField(name))
}

/// Parses `HFDTE280726` (old style) or `HFDTEDATE:280726,01` (current style).
fn parse_date_header(line: &str) -> Option<Date> {
    // Column 1 is the data source (F = flight recorder, O = observer, P = pilot).
    if !line.starts_with('H') || line.get(2..5) != Some("DTE") {
        return None;
    }
    let rest = line.get(5..)?;
    let digits = rest.strip_prefix("DATE:").unwrap_or(rest);
    let day: u8 = digits.get(0..2)?.parse().ok()?;
    let month: u8 = digits.get(2..4)?.parse().ok()?;
    let year: i32 = digits.get(4..6)?.parse().ok()?;
    // Two-digit year: the format dates from the 1990s, so 80..99 means 19xx.
    let century = if year >= 80 { 1900 } else { 2000 };
    Date::new(century + year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "AXCT1234567890\r\n\
        HFDTE280726\r\n\
        HFPLTPILOT:Jane Doe\r\n\
        I013638FXA\r\n\
        B1101355206343N00006198WA0058700558012\r\n\
        B1101365206350N00006210WV-012300559013\r\n\
        LXCT some comment\r\n\
        G0123456789ABCDEF\r\n";

    #[test]
    fn parses_the_example_fix_from_the_specification() {
        let fix = parse_fix("B1101355206343N00006198WA0058700558").unwrap();
        assert_eq!(fix.time, TimeOfDay::new(11, 1, 35).unwrap());
        assert!((fix.latitude_deg - (52.0 + 6.343 / 60.0)).abs() < 1e-9);
        assert!((fix.longitude_deg + 6.198 / 60.0).abs() < 1e-9);
        assert!(fix.fix_3d);
        assert_eq!(fix.pressure_altitude_m, 587);
        assert_eq!(fix.gps_altitude_m, 558);
        assert_eq!(fix.extension, "");
    }

    #[test]
    fn parses_negative_altitude_validity_and_extension() {
        let file = IgcFile::parse(SAMPLE).unwrap();
        let fixes: Vec<_> = file.fixes().collect();
        assert_eq!(fixes.len(), 2);
        assert!(!fixes[1].fix_3d);
        assert_eq!(fixes[1].pressure_altitude_m, -123);
        assert_eq!(fixes[1].extension, "013");
    }

    #[test]
    fn reading_and_writing_reproduces_the_file_exactly() {
        let file = IgcFile::parse(SAMPLE).unwrap();
        assert_eq!(file.to_string(), SAMPLE);
    }

    #[test]
    fn every_representable_angle_survives_a_round_trip() {
        // Step through the whole range in a stride that hits many different minute values.
        for total in (0..=180 * 60_000u32).step_by(7919) {
            let line = format!(
                "B120000{:02}{:05}S{:03}{:05}WA0010000100",
                (total / 2) / 60_000,
                (total / 2) % 60_000,
                total / 60_000,
                total % 60_000
            );
            let fix = parse_fix(&line).unwrap();
            assert_eq!(fix.to_string(), line);
        }
    }

    #[test]
    fn reads_both_date_header_styles() {
        let old = IgcFile::parse("HFDTE280726\r\n").unwrap();
        assert_eq!(old.date(), Date::new(2026, 7, 28));
        let new = IgcFile::parse("HFDTEDATE:010299,02\r\n").unwrap();
        assert_eq!(new.date(), Date::new(1999, 2, 1));
        let none = IgcFile::parse("HFPLTPILOT:Nobody\r\n").unwrap();
        assert_eq!(none.date(), None);
    }

    #[test]
    fn new_header_contains_a_readable_date() {
        let date = Date::new(2026, 11, 10).unwrap();
        let file = IgcFile::with_header(date, &FlightMeta::default());
        assert_eq!(file.date(), Some(date));
        assert!(
            file.to_string()
                .starts_with("AXXXVAR\r\nHFDTEDATE:101126,01\r\n")
        );
    }

    #[test]
    fn header_text_cannot_inject_extra_records() {
        let meta = FlightMeta {
            pilot: "Eve\r\nB1101355206343N00006198WA0058700558".to_owned(),
            ..FlightMeta::default()
        };
        let file = IgcFile::with_header(Date::new(2026, 1, 1).unwrap(), &meta);
        let reparsed = IgcFile::parse(&file.to_string()).unwrap();
        assert_eq!(reparsed.fixes().count(), 0);
    }

    #[test]
    fn reports_the_line_of_a_broken_fix() {
        let text = "AXXXVAR\r\nB110135\r\n";
        assert_eq!(
            IgcFile::parse(text),
            Err(IgcError {
                line: 2,
                kind: IgcErrorKind::FixTooShort
            })
        );

        let text = "B1101355206343X00006198WA0058700558\r\n";
        assert_eq!(
            IgcFile::parse(text),
            Err(IgcError {
                line: 1,
                kind: IgcErrorKind::InvalidField("latitude")
            })
        );

        let text = "B2501355206343N00006198WA0058700558\r\n";
        assert_eq!(
            IgcFile::parse(text).unwrap_err().kind,
            IgcErrorKind::InvalidField("time")
        );
    }

    #[test]
    fn non_ascii_text_in_a_fix_is_an_error_not_a_panic() {
        let text = "B11013552063ä3N00006198WA0058700558\r\n";
        assert!(IgcFile::parse(text).is_err());
    }

    #[test]
    fn accepts_latin1_bytes() {
        // "HFPLTPILOT:Jörg" with ö as the single Latin-1 byte 0xF6.
        let bytes = b"HFPLTPILOT:J\xF6rg\r\nB1101355206343N00006198WA0058700558\r\n";
        let file = IgcFile::parse_bytes(bytes).unwrap();
        assert_eq!(file.records[0], Record::Other("HFPLTPILOT:Jörg".to_owned()));
        assert_eq!(file.fixes().count(), 1);
    }
}
