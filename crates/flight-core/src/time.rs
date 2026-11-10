//! Time types.
//!
//! Two different notions of time are used:
//!
//! * [`Timestamp`]: monotonic seconds since the sensor source started. Used for all
//!   filtering and ordering. It has nothing to do with the wall clock.
//! * [`UtcDateTime`]: calendar time as reported by GPS, with whole-second resolution.
//!   Only needed for things humans read, such as IGC files.

/// Monotonic time in seconds since the sensor source started.
///
/// This is a "newtype": a struct wrapping a single `f64`. It costs nothing at runtime but
/// stops us from accidentally mixing up a time with, say, an altitude.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Timestamp(f64);

impl Timestamp {
    pub const ZERO: Timestamp = Timestamp(0.0);

    pub fn from_secs(secs: f64) -> Self {
        Timestamp(secs)
    }

    pub fn as_secs(self) -> f64 {
        self.0
    }

    /// Seconds elapsed from `earlier` to `self` (negative if `earlier` is later).
    pub fn seconds_since(self, earlier: Timestamp) -> f64 {
        self.0 - earlier.0
    }
}

/// A calendar date (proleptic Gregorian calendar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

impl Date {
    /// Returns `None` if the date does not exist (e.g. 30 February).
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }

    /// The date `days` days later (or earlier, if negative).
    pub fn plus_days(self, days: i64) -> Date {
        let (year, month, day) = civil_from_days(days_from_civil(self) + days);
        Date { year, month, day }
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

// Day number conversion after Howard Hinnant's "chrono-compatible low-level date
// algorithms". Day 0 is 1970-01-01.
fn days_from_civil(date: Date) -> i64 {
    let month = i64::from(date.month);
    let year = i64::from(date.year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(date.day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

/// A time of day with whole-second resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
    second: u8,
}

impl TimeOfDay {
    pub const MIDNIGHT: TimeOfDay = TimeOfDay {
        hour: 0,
        minute: 0,
        second: 0,
    };

    /// Returns `None` for values outside 00:00:00 ..= 23:59:59.
    pub fn new(hour: u8, minute: u8, second: u8) -> Option<Self> {
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        Some(TimeOfDay {
            hour,
            minute,
            second,
        })
    }

    pub fn hour(self) -> u8 {
        self.hour
    }

    pub fn minute(self) -> u8 {
        self.minute
    }

    pub fn second(self) -> u8 {
        self.second
    }

    pub fn seconds_since_midnight(self) -> u32 {
        u32::from(self.hour) * 3600 + u32::from(self.minute) * 60 + u32::from(self.second)
    }

    /// Returns `None` if `seconds` is a full day or more.
    pub fn from_seconds_since_midnight(seconds: u32) -> Option<Self> {
        if seconds >= SECONDS_PER_DAY {
            return None;
        }
        Some(TimeOfDay {
            hour: (seconds / 3600) as u8,
            minute: (seconds / 60 % 60) as u8,
            second: (seconds % 60) as u8,
        })
    }
}

pub const SECONDS_PER_DAY: u32 = 86_400;

/// A UTC calendar date and time with whole-second resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcDateTime {
    pub date: Date,
    pub time: TimeOfDay,
}

impl UtcDateTime {
    pub fn new(date: Date, time: TimeOfDay) -> Self {
        UtcDateTime { date, time }
    }

    /// The date and time `seconds` later, rolling over midnight as needed.
    pub fn plus_seconds(self, seconds: u64) -> UtcDateTime {
        let total = u64::from(self.time.seconds_since_midnight()) + seconds;
        let per_day = u64::from(SECONDS_PER_DAY);
        let time = TimeOfDay::from_seconds_since_midnight((total % per_day) as u32)
            .expect("remainder is always less than one day");
        UtcDateTime {
            date: self.date.plus_days((total / per_day) as i64),
            time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_dates() {
        assert!(Date::new(2026, 2, 29).is_none());
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2026, 13, 1).is_none());
        assert!(Date::new(2026, 4, 31).is_none());
        assert!(Date::new(2026, 4, 0).is_none());
    }

    #[test]
    fn plus_days_crosses_month_and_year() {
        let date = Date::new(2026, 12, 31).unwrap();
        assert_eq!(date.plus_days(1), Date::new(2027, 1, 1).unwrap());
        assert_eq!(date.plus_days(-365), Date::new(2025, 12, 31).unwrap());
        let leap = Date::new(2024, 2, 28).unwrap();
        assert_eq!(leap.plus_days(2), Date::new(2024, 3, 1).unwrap());
    }

    #[test]
    fn day_number_round_trips() {
        assert_eq!(days_from_civil(Date::new(1970, 1, 1).unwrap()), 0);
        for days in [-800_000, -1, 0, 1, 20_000, 800_000] {
            let (year, month, day) = civil_from_days(days);
            let date = Date::new(year, month, day).unwrap();
            assert_eq!(days_from_civil(date), days);
        }
    }

    #[test]
    fn time_of_day_validation_and_seconds() {
        assert!(TimeOfDay::new(24, 0, 0).is_none());
        assert!(TimeOfDay::new(0, 60, 0).is_none());
        let time = TimeOfDay::new(13, 45, 10).unwrap();
        assert_eq!(time.seconds_since_midnight(), 49_510);
        assert_eq!(TimeOfDay::from_seconds_since_midnight(49_510), Some(time));
        assert_eq!(TimeOfDay::from_seconds_since_midnight(86_400), None);
    }

    #[test]
    fn plus_seconds_rolls_over_midnight() {
        let start = UtcDateTime::new(
            Date::new(2026, 7, 31).unwrap(),
            TimeOfDay::new(23, 59, 58).unwrap(),
        );
        let later = start.plus_seconds(5);
        assert_eq!(later.date, Date::new(2026, 8, 1).unwrap());
        assert_eq!(later.time, TimeOfDay::new(0, 0, 3).unwrap());
    }
}
