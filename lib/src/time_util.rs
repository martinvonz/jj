// Copyright 2024 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Provides support for parsing and matching date ranges.

use chrono::DateTime;
use chrono::FixedOffset;
use chrono::Local;
use chrono::TimeZone;
use chrono_english::parse_date_string;
use chrono_english::DateError;
use chrono_english::Dialect;
use thiserror::Error;

use crate::backend::MillisSinceEpoch;
use crate::backend::Timestamp;

/// Context needed to create a DatePattern during revset evaluation.
#[derive(Copy, Clone, Debug)]
pub enum DatePatternContext {
    /// Interpret date patterns using the local machine's time zone
    Local(DateTime<Local>),
    /// Interpret date patterns using any FixedOffset time zone
    Fixed(DateTime<FixedOffset>),
}

impl DatePatternContext {
    /// Parses a DatePattern from the given string and kind.
    pub fn parse_relative(
        &self,
        s: &str,
        kind: &str,
    ) -> Result<DatePattern, DatePatternParseError> {
        match *self {
            DatePatternContext::Local(dt) => DatePattern::from_str_kind(s, kind, dt),
            DatePatternContext::Fixed(dt) => DatePattern::from_str_kind(s, kind, dt),
        }
    }
}

impl From<DateTime<Local>> for DatePatternContext {
    fn from(value: DateTime<Local>) -> Self {
        DatePatternContext::Local(value)
    }
}

impl From<DateTime<FixedOffset>> for DatePatternContext {
    fn from(value: DateTime<FixedOffset>) -> Self {
        DatePatternContext::Fixed(value)
    }
}

/// Error occurred during date pattern parsing.
#[derive(Debug, Error)]
pub enum DatePatternParseError {
    /// Unknown pattern kind is specified.
    #[error(r#"Invalid date pattern kind "{0}:""#)]
    InvalidKind(String),
    /// Failed to parse timestamp.
    #[error(transparent)]
    ParseError(#[from] DateError),
}

/// Represents an range of dates that may be matched against.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DatePattern {
    /// Represents all dates at or after the given instant.
    AtOrAfter(MillisSinceEpoch),
    /// Represents all dates before, but not including, the given instant.
    Before(MillisSinceEpoch),
}

impl DatePattern {
    /// Parses a string into a DatePattern.
    ///
    /// * `s` is the string to be parsed.
    ///
    /// * `kind` must be either "after" or "before". This determines whether the
    ///   pattern will match dates after or before the parsed date.
    ///
    /// * `now` is the user's current time. This is a [`DateTime<Tz>`] because
    ///   knowledge of offset changes is needed to correctly process relative
    ///   times like "today". For example, California entered DST on March 10,
    ///   2024, shifting clocks from UTC-8 to UTC-7 at 2:00 AM. If the pattern
    ///   "today" was parsed at noon on that day, it should be interpreted as
    ///   2024-03-10T00:00:00-08:00 even though the current offset is -07:00.
    pub fn from_str_kind<Tz: TimeZone>(
        s: &str,
        kind: &str,
        now: DateTime<Tz>,
    ) -> Result<DatePattern, DatePatternParseError>
    where
        Tz::Offset: Copy,
    {
        let d =
            parse_date_string(s, now, Dialect::Us).map_err(DatePatternParseError::ParseError)?;
        let millis_since_epoch = MillisSinceEpoch(d.timestamp_millis());
        match kind {
            "after" => Ok(DatePattern::AtOrAfter(millis_since_epoch)),
            "before" => Ok(DatePattern::Before(millis_since_epoch)),
            kind => Err(DatePatternParseError::InvalidKind(kind.to_owned())),
        }
    }

    /// Determines whether a given timestamp is matched by the pattern.
    pub fn matches(&self, timestamp: &Timestamp) -> bool {
        match self {
            DatePattern::AtOrAfter(earliest) => *earliest <= timestamp.timestamp,
            DatePattern::Before(latest) => timestamp.timestamp < *latest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_equal<Tz: TimeZone>(now: DateTime<Tz>, expression: &str, should_equal_time: &str)
    where
        Tz::Offset: Copy,
    {
        let expression = DatePattern::from_str_kind(expression, "after", now).unwrap();
        assert_eq!(
            expression,
            DatePattern::AtOrAfter(MillisSinceEpoch(
                DateTime::parse_from_rfc3339(should_equal_time)
                    .unwrap()
                    .timestamp_millis()
            ))
        );
    }

    #[test]
    fn test_date_pattern_parses_dates_without_times_as_the_date_at_local_midnight() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2023-03-25", "2023-03-25T08:00:00Z");
        test_equal(now, "3/25/2023", "2023-03-25T08:00:00Z");
        test_equal(now, "3/25/23", "2023-03-25T08:00:00Z");
    }

    #[test]
    fn test_date_pattern_parses_dates_with_times_without_specifying_an_offset() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2023-03-25T00:00:00", "2023-03-25T08:00:00Z");
        test_equal(now, "2023-03-25 00:00:00", "2023-03-25T08:00:00Z");
    }

    #[test]
    fn test_date_pattern_parses_dates_with_a_specified_offset() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(
            now,
            "2023-03-25T00:00:00-05:00",
            "2023-03-25T00:00:00-05:00",
        );
    }

    #[test]
    fn test_date_pattern_parses_dates_with_the_z_offset() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2023-03-25T00:00:00Z", "2023-03-25T00:00:00Z");
    }

    #[test]
    fn test_date_pattern_parses_relative_durations() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2 hours ago", "2024-01-01T06:00:00Z");
        test_equal(now, "5 minutes", "2024-01-01T08:05:00Z");
        test_equal(now, "1 week ago", "2023-12-25T08:00:00Z");
        test_equal(now, "yesterday", "2023-12-31T08:00:00Z");
        test_equal(now, "tomorrow", "2024-01-02T08:00:00Z");
    }

    #[test]
    fn test_date_pattern_parses_relative_dates_with_times() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T08:00:00-08:00").unwrap();
        test_equal(now, "yesterday 5pm", "2024-01-01T01:00:00Z");
        test_equal(now, "yesterday 10am", "2023-12-31T18:00:00Z");
        test_equal(now, "yesterday 10:30", "2023-12-31T18:30:00Z");
    }
}
