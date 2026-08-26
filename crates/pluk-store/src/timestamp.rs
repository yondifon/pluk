//! Timestamps as SQLite writes them.
//!
//! Every `created_at` column holds the output of SQLite's `datetime('now')`:
//! UTC, formatted `yyyy-MM-dd HH:mm:ss`. The Swift app parses this format with
//! an explicit POSIX locale and UTC zone because locale-default parsing breaks;
//! this module does the equivalent work explicitly on the Rust side.
//!
//! Inserts keep relying on the columns' SQL defaults so rows carry the
//! database's own clock — the same clock retention and range queries compare
//! against.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current UTC instant, formatted like `datetime('now')`.
pub fn now_utc_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_unix_seconds(secs)
}

/// Format Unix seconds as `yyyy-MM-dd HH:mm:ss` (UTC).
///
/// Civil-date conversion via Howard Hinnant's date algorithms.
pub fn format_unix_seconds(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Parse `yyyy-MM-dd HH:mm:ss` into Unix seconds (UTC).
///
/// Returns `None` for anything else — including ISO strings with a `T`
/// separator or fractional seconds, which are not what this schema stores.
pub fn parse_to_unix_seconds(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b' '
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<u32>().ok();
    let year = num(0..4)? as i64;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)? as i64;
    let minute = num(14..16)? as i64;
    let second = num(17..19)? as i64;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(m) + 9) % 12; // Mar=0 … Feb=11
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], Mar=0 … Feb=11
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp >= 10 { mp - 9 } else { mp + 3 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants_like_sqlite() {
        assert_eq!(format_unix_seconds(0), "1970-01-01 00:00:00");
        assert_eq!(format_unix_seconds(1_787_747_696), "2026-08-26 12:34:56");
        assert_eq!(format_unix_seconds(1_709_164_800), "2024-02-29 00:00:00");
        assert_eq!(format_unix_seconds(-1), "1969-12-31 23:59:59");
    }

    #[test]
    fn parses_the_stored_format_and_nothing_else() {
        assert_eq!(
            parse_to_unix_seconds("2026-08-26 12:34:56"),
            Some(1_787_747_696)
        );
        assert_eq!(parse_to_unix_seconds("1970-01-01 00:00:00"), Some(0));
        assert_eq!(parse_to_unix_seconds("2026-08-26T12:34:56"), None);
        assert_eq!(parse_to_unix_seconds("2026-08-26 12:34:56.789"), None);
        assert_eq!(parse_to_unix_seconds("not a date"), None);
        assert_eq!(parse_to_unix_seconds("2026-13-26 12:34:56"), None);
    }

    #[test]
    fn format_and_parse_round_trip() {
        for secs in [0, 951_782_400, 1_600_000_000, 1_787_747_696, 2_147_483_647] {
            let formatted = format_unix_seconds(secs);
            assert_eq!(formatted.len(), 19, "{formatted}");
            assert_eq!(parse_to_unix_seconds(&formatted), Some(secs));
        }
    }
}
