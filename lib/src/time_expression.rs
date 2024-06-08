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

//! Provides a TimeExpression type that represents a range of time.

use chrono::{DateTime, TimeZone};
use chrono_english::{parse_date_string, DateError, Dialect};

use crate::backend::Timestamp;

/// Represents an expression to match dates and times.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimeExpression {
    /// Represents all times at or after the given instant in time.
    AtOrAfter(Timestamp),
}

impl TimeExpression {
    /// Parses a string into a TimeExpression.
    pub fn parse<Tz: TimeZone>(s: &str, now: DateTime<Tz>) -> Result<TimeExpression, DateError>
    where
        Tz::Offset: Copy,
    {
        let d = parse_date_string(s, now, Dialect::Us)?;
        let ts = Timestamp::from_datetime(d.to_utc().fixed_offset());
        Ok(TimeExpression::AtOrAfter(ts))
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;

    fn test_equal<Tz: TimeZone>(now: DateTime<Tz>, expression: &str, should_equal_time: &str)
    where
        Tz::Offset: Copy,
    {
        let expression = TimeExpression::parse(expression, now).unwrap();
        assert_eq!(
            expression,
            TimeExpression::AtOrAfter(Timestamp::from_datetime(
                DateTime::parse_from_rfc3339(should_equal_time)
                    .unwrap()
                    .to_utc()
                    .fixed_offset()
            ))
        );
    }

    #[test]
    fn test_time_expression_parses_dates_without_times_as_the_date_at_local_midnight() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2023-03-25", "2023-03-25T08:00:00Z");
        test_equal(now, "3/25/2023", "2023-03-25T08:00:00Z");
        test_equal(now, "3/25/23", "2023-03-25T08:00:00Z");
    }

    #[test]
    fn test_time_expression_parses_dates_with_times_without_specifying_an_offset() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2023-03-25T00:00:00", "2023-03-25T08:00:00Z");
        test_equal(now, "2023-03-25 00:00:00", "2023-03-25T08:00:00Z");
    }

    #[test]
    fn test_time_expression_parses_dates_with_a_specified_offset() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(
            now,
            "2023-03-25T00:00:00-05:00",
            "2023-03-25T00:00:00-05:00",
        );
    }

    #[test]
    fn test_time_expression_parses_dates_with_the_z_offset() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2023-03-25T00:00:00Z", "2023-03-25T00:00:00Z");
    }

    #[test]
    fn test_time_expression_parses_relative_durations() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T00:00:00-08:00").unwrap();
        test_equal(now, "2 hours ago", "2024-01-01T06:00:00Z");
        test_equal(now, "5 minutes", "2024-01-01T08:05:00Z");
        test_equal(now, "1 week ago", "2023-12-25T08:00:00Z");
        test_equal(now, "yesterday", "2023-12-31T08:00:00Z");
        test_equal(now, "tomorrow", "2024-01-02T08:00:00Z");
    }

    #[test]
    fn test_time_expression_parses_relative_dates_with_times() {
        let now = DateTime::parse_from_rfc3339("2024-01-01T08:00:00-08:00").unwrap();
        test_equal(now, "yesterday 5pm", "2024-01-01T01:00:00Z");
        test_equal(now, "yesterday 10am", "2023-12-31T18:00:00Z");
        test_equal(now, "yesterday 10:30", "2023-12-31T18:30:00Z");
    }
}
