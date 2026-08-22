//! Two ways of saying "when": a duration (`10m`, `1h30m`) and a cron
//! expression (`0 9 * * 1-5`).
//!
//! Both are parsed here, in the domain, rather than at the CLI edge. A schedule
//! that was accepted must be one the scheduler can actually evaluate, and the
//! only way to guarantee that is to let the same type do both jobs.

use std::fmt;

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

/// A span of time, stored as whole seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Interval(u64);

impl Interval {
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs)
    }

    pub const fn as_secs(self) -> u64 {
        self.0
    }

    pub fn to_duration(self) -> Duration {
        Duration::seconds(self.0 as i64)
    }

    /// Parses `30s`, `10m`, `1h`, `2d`, `1h30m`, and a bare number as minutes.
    ///
    /// A bare number means minutes because every place a user types one of
    /// these — a heartbeat, a poll — minutes is the unit they meant.
    pub fn parse(input: &str) -> Result<Self> {
        let text = input.trim().to_ascii_lowercase();
        if text.is_empty() {
            return Err(DomainError::invalid("empty interval"));
        }

        if let Ok(minutes) = text.parse::<u64>() {
            return Self::checked(minutes.saturating_mul(60));
        }

        let mut total: u64 = 0;
        let mut digits = String::new();
        let mut saw_unit = false;

        for ch in text.chars() {
            match ch {
                '0'..='9' => digits.push(ch),
                's' | 'm' | 'h' | 'd' | 'w' => {
                    if digits.is_empty() {
                        return Err(DomainError::invalid(format!(
                            "interval '{input}' has a unit with no number"
                        )));
                    }
                    let value: u64 = digits
                        .parse()
                        .map_err(|_| DomainError::invalid(format!("interval '{input}' overflows")))?;
                    let scale = match ch {
                        's' => 1,
                        'm' => 60,
                        'h' => 3_600,
                        'd' => 86_400,
                        _ => 604_800,
                    };
                    total = total.saturating_add(value.saturating_mul(scale));
                    digits.clear();
                    saw_unit = true;
                }
                ' ' => {}
                _ => {
                    return Err(DomainError::invalid(format!(
                        "interval '{input}' contains '{ch}'; use forms like 30s, 10m, 1h30m"
                    )))
                }
            }
        }

        if !digits.is_empty() || !saw_unit {
            return Err(DomainError::invalid(format!(
                "interval '{input}' is incomplete; use forms like 30s, 10m, 1h30m"
            )));
        }

        Self::checked(total)
    }

    fn checked(secs: u64) -> Result<Self> {
        if secs == 0 {
            return Err(DomainError::invalid("interval must be greater than zero"));
        }
        if secs > 365 * 86_400 {
            return Err(DomainError::invalid("interval must be a year or less"));
        }
        Ok(Self(secs))
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut left = self.0;
        let mut out = String::new();
        for (scale, suffix) in [(86_400, 'd'), (3_600, 'h'), (60, 'm'), (1, 's')] {
            let n = left / scale;
            if n > 0 {
                out.push_str(&format!("{n}{suffix}"));
                left -= n * scale;
            }
        }
        f.write_str(&out)
    }
}

/// A five-field cron expression: minute, hour, day-of-month, month, day-of-week.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CronExpr {
    source: String,
    minute: FieldSet,
    hour: FieldSet,
    day_of_month: FieldSet,
    month: FieldSet,
    day_of_week: FieldSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldSet {
    /// One bit per legal value, indexed from `min`.
    bits: u64,
    min: u8,
    /// True when the field was written as `*` — cron's day-of-month /
    /// day-of-week rule depends on knowing this, not just on which bits are set.
    wildcard: bool,
}

impl FieldSet {
    fn contains(&self, value: u8) -> bool {
        if value < self.min {
            return false;
        }
        let offset = value - self.min;
        offset < 64 && self.bits & (1u64 << offset) != 0
    }

    fn parse(field: &str, min: u8, max: u8, names: &[(&str, u8)]) -> Result<Self> {
        let mut bits = 0u64;
        let wildcard = field == "*";

        for part in field.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(DomainError::invalid("cron field has an empty list entry"));
            }

            let (range_text, step) = match part.split_once('/') {
                Some((range, step_text)) => {
                    let step: u8 = step_text.parse().map_err(|_| {
                        DomainError::invalid(format!("cron step '{step_text}' is not a number"))
                    })?;
                    if step == 0 {
                        return Err(DomainError::invalid("cron step must be greater than zero"));
                    }
                    (range, step)
                }
                None => (part, 1),
            };

            let (lo, hi) = if range_text == "*" {
                (min, max)
            } else if let Some((a, b)) = range_text.split_once('-') {
                (
                    resolve(a, min, max, names)?,
                    resolve(b, min, max, names)?,
                )
            } else {
                let value = resolve(range_text, min, max, names)?;
                // A bare value with a step means "from here to the end", which
                // is what `5/15` means in every cron implementation worth
                // matching.
                if step > 1 {
                    (value, max)
                } else {
                    (value, value)
                }
            };

            if lo > hi {
                return Err(DomainError::invalid(format!(
                    "cron range '{range_text}' runs backwards"
                )));
            }

            let mut value = lo;
            while value <= hi {
                bits |= 1u64 << (value - min);
                value = match value.checked_add(step) {
                    Some(next) => next,
                    None => break,
                };
            }
        }

        if bits == 0 {
            return Err(DomainError::invalid("cron field matches nothing"));
        }

        Ok(Self {
            bits,
            min,
            wildcard,
        })
    }
}

fn resolve(token: &str, min: u8, max: u8, names: &[(&str, u8)]) -> Result<u8> {
    let lower = token.trim().to_ascii_lowercase();
    if let Some((_, value)) = names.iter().find(|(name, _)| *name == lower) {
        return Ok(*value);
    }
    let value: u8 = lower
        .parse()
        .map_err(|_| DomainError::invalid(format!("cron value '{token}' is not a number")))?;
    // Cron lets 7 mean Sunday as well as 0.
    let value = if min == 0 && max == 6 && value == 7 { 0 } else { value };
    if value < min || value > max {
        return Err(DomainError::invalid(format!(
            "cron value '{token}' is outside {min}-{max}"
        )));
    }
    Ok(value)
}

const MONTHS: &[(&str, u8)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

const DAYS: &[(&str, u8)] = &[
    ("sun", 0),
    ("mon", 1),
    ("tue", 2),
    ("wed", 3),
    ("thu", 4),
    ("fri", 5),
    ("sat", 6),
];

impl CronExpr {
    pub fn parse(source: &str) -> Result<Self> {
        let fields: Vec<&str> = source.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(DomainError::invalid(format!(
                "cron needs 5 fields (minute hour day month weekday), got {}",
                fields.len()
            )));
        }

        Ok(Self {
            source: fields.join(" "),
            minute: FieldSet::parse(fields[0], 0, 59, &[])?,
            hour: FieldSet::parse(fields[1], 0, 23, &[])?,
            day_of_month: FieldSet::parse(fields[2], 1, 31, &[])?,
            month: FieldSet::parse(fields[3], 1, 12, MONTHS)?,
            day_of_week: FieldSet::parse(fields[4], 0, 6, DAYS)?,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Does this expression fire during the minute containing `at`?
    pub fn matches(&self, at: DateTime<Utc>) -> bool {
        if !self.minute.contains(at.minute() as u8) || !self.hour.contains(at.hour() as u8) {
            return false;
        }
        if !self.month.contains(at.month() as u8) {
            return false;
        }

        let dom_hit = self.day_of_month.contains(at.day() as u8);
        let dow_hit = self
            .day_of_week
            .contains(at.weekday().num_days_from_sunday() as u8);

        // Cron's oldest wart, faithfully reproduced: when both day fields are
        // restricted the expression fires if *either* matches; when one is `*`
        // it must be the other that decides.
        match (self.day_of_month.wildcard, self.day_of_week.wildcard) {
            (true, true) => true,
            (false, true) => dom_hit,
            (true, false) => dow_hit,
            (false, false) => dom_hit || dow_hit,
        }
    }

    /// The first firing strictly after `after`, or `None` if the expression
    /// cannot fire within a year (`0 0 30 2 *` — February the 30th).
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut cursor = Utc
            .with_ymd_and_hms(
                after.year(),
                after.month(),
                after.day(),
                after.hour(),
                after.minute(),
                0,
            )
            .single()?
            + Duration::minutes(1);

        let horizon = after + Duration::days(366);
        while cursor <= horizon {
            if self.matches(cursor) {
                return Some(cursor);
            }
            // Whole days can be skipped when the date itself cannot match,
            // which keeps `0 0 1 1 *` from walking half a million minutes.
            if !self.date_could_match(cursor) {
                cursor = (cursor + Duration::days(1))
                    .with_hour(0)?
                    .with_minute(0)?;
                continue;
            }
            cursor += Duration::minutes(1);
        }
        None
    }

    fn date_could_match(&self, at: DateTime<Utc>) -> bool {
        if !self.month.contains(at.month() as u8) {
            return false;
        }
        let dom_hit = self.day_of_month.contains(at.day() as u8);
        let dow_hit = self
            .day_of_week
            .contains(at.weekday().num_days_from_sunday() as u8);
        match (self.day_of_month.wildcard, self.day_of_week.wildcard) {
            (true, true) => true,
            (false, true) => dom_hit,
            (true, false) => dow_hit,
            (false, false) => dom_hit || dow_hit,
        }
    }
}

impl fmt::Display for CronExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl TryFrom<String> for CronExpr {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<CronExpr> for String {
    fn from(value: CronExpr) -> Self {
        value.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn parses_compound_intervals() {
        assert_eq!(Interval::parse("30s").unwrap().as_secs(), 30);
        assert_eq!(Interval::parse("1h30m").unwrap().as_secs(), 5_400);
        assert_eq!(Interval::parse("2d").unwrap().as_secs(), 172_800);
    }

    #[test]
    fn bare_number_means_minutes() {
        assert_eq!(Interval::parse("10").unwrap().as_secs(), 600);
    }

    #[test]
    fn rejects_nonsense_intervals() {
        for bad in ["", "0m", "h", "10x", "-5m"] {
            assert!(Interval::parse(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn interval_round_trips_through_display() {
        let interval = Interval::parse("1h30m").unwrap();
        assert_eq!(interval.to_string(), "1h30m");
        assert_eq!(Interval::parse(&interval.to_string()).unwrap(), interval);
    }

    #[test]
    fn weekday_cron_skips_the_weekend() {
        let cron = CronExpr::parse("0 9 * * 1-5").unwrap();
        // Friday 09:00 -> Monday 09:00.
        let next = cron.next_after(at("2026-08-14T09:00:00Z")).unwrap();
        assert_eq!(next, at("2026-08-17T09:00:00Z"));
    }

    #[test]
    fn step_fields_fire_every_nth_minute() {
        let cron = CronExpr::parse("*/15 * * * *").unwrap();
        assert_eq!(
            cron.next_after(at("2026-08-16T10:07:00Z")).unwrap(),
            at("2026-08-16T10:15:00Z")
        );
    }

    #[test]
    fn named_fields_are_accepted() {
        let cron = CronExpr::parse("0 0 1 jan *").unwrap();
        assert_eq!(
            cron.next_after(at("2026-08-16T10:00:00Z")).unwrap(),
            at("2027-01-01T00:00:00Z")
        );
    }

    #[test]
    fn both_day_fields_restricted_means_either() {
        // The 1st of the month, or any Monday.
        let cron = CronExpr::parse("0 0 1 * 1").unwrap();
        assert!(cron.matches(at("2026-09-01T00:00:00Z"))); // a Tuesday, but the 1st
        assert!(cron.matches(at("2026-08-17T00:00:00Z"))); // a Monday, not the 1st
        assert!(!cron.matches(at("2026-08-18T00:00:00Z")));
    }

    #[test]
    fn impossible_dates_terminate_instead_of_looping() {
        let cron = CronExpr::parse("0 0 30 2 *").unwrap();
        assert!(cron.next_after(at("2026-08-16T10:00:00Z")).is_none());
    }

    #[test]
    fn rejects_malformed_expressions() {
        for bad in ["* * * *", "60 * * * *", "* 24 * * *", "*/0 * * * *", "5-1 * * * *"] {
            assert!(CronExpr::parse(bad).is_err(), "{bad} should not parse");
        }
    }
}
