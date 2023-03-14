use chrono::format::StrftimeItems;
use chrono::{DateTime, FixedOffset, LocalResult, TimeZone, Utc};
use jujutsu_lib::backend::Timestamp;
use once_cell::sync::Lazy;

fn datetime_from_timestamp(context: &Timestamp) -> Option<DateTime<FixedOffset>> {
    let utc = match Utc.timestamp_opt(
        context.timestamp.0.div_euclid(1000),
        (context.timestamp.0.rem_euclid(1000)) as u32 * 1000000,
    ) {
        LocalResult::None => {
            return None;
        }
        LocalResult::Single(x) => x,
        LocalResult::Ambiguous(y, _z) => y,
    };

    Some(
        utc.with_timezone(
            &FixedOffset::east_opt(context.tz_offset * 60)
                .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap()),
        ),
    )
}

pub fn format_absolute_timestamp(timestamp: &Timestamp) -> String {
    static FORMAT_ITEMS: Lazy<Vec<chrono::format::Item>> =
        Lazy::new(|| StrftimeItems::new("%Y-%m-%d %H:%M:%S.%3f %:z").collect());
    match datetime_from_timestamp(timestamp) {
        Some(datetime) => datetime.format_with_items(FORMAT_ITEMS.iter()).to_string(),
        None => "<out-of-range date>".to_string(),
    }
}

pub fn format_duration(from: &Timestamp, to: &Timestamp, format: &timeago::Formatter) -> String {
    datetime_from_timestamp(from)
        .zip(datetime_from_timestamp(to))
        .and_then(|(from, to)| to.signed_duration_since(from).to_std().ok())
        .map(|duration| format.convert(duration))
        .unwrap_or_else(|| "<out-of-range date>".to_string())
}

pub fn format_timestamp_relative_to_now(timestamp: &Timestamp) -> String {
    format_duration(timestamp, &Timestamp::now(), &timeago::Formatter::new())
}
