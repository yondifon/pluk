use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MINUTES_PER_DAY: i64 = 24 * 60;
const MAX_CLOCK_MINUTE: i64 = MINUTES_PER_DAY - 1;

pub(crate) const DEFAULT_WINDOW_START: &str = "06:00";
pub(crate) const DEFAULT_WINDOW_END: &str = "22:00";
pub(crate) const DEFAULT_MIN_GAP_MINUTES: i64 = 45;
pub(crate) const DEFAULT_MAX_GAP_MINUTES: i64 = 90;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSettings {
    pub window_start: String,
    pub window_end: String,
    pub min_gap_minutes: i64,
    pub max_gap_minutes: i64,
}

impl ScheduleSettings {
    pub fn new(
        window_start: &str,
        window_end: &str,
        min_gap_minutes: i64,
        max_gap_minutes: i64,
    ) -> Result<Self, ScheduleError> {
        let settings = Self {
            window_start: window_start.to_owned(),
            window_end: window_end.to_owned(),
            min_gap_minutes,
            max_gap_minutes,
        };
        settings.to_config()?;
        Ok(settings)
    }

    pub(crate) fn defaults() -> Self {
        Self {
            window_start: DEFAULT_WINDOW_START.to_owned(),
            window_end: DEFAULT_WINDOW_END.to_owned(),
            min_gap_minutes: DEFAULT_MIN_GAP_MINUTES,
            max_gap_minutes: DEFAULT_MAX_GAP_MINUTES,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_minutes(
        window_start: i64,
        window_end: i64,
        min_gap_minutes: i64,
        max_gap_minutes: i64,
    ) -> Result<Self, ScheduleError> {
        let settings = Self {
            window_start: format_clock(window_start)?,
            window_end: format_clock(window_end)?,
            min_gap_minutes,
            max_gap_minutes,
        };
        settings.to_config()?;
        Ok(settings)
    }

    pub fn to_config(&self) -> Result<ScheduleConfig, ScheduleError> {
        let window_start = parse_clock(&self.window_start)?;
        let window_end = parse_clock(&self.window_end)?;
        if window_start >= window_end {
            return Err(ScheduleError::Invalid(
                "The scheduling window must end after it starts.".to_owned(),
            ));
        }
        if self.min_gap_minutes < 1 || self.max_gap_minutes < self.min_gap_minutes {
            return Err(ScheduleError::Invalid(
                "The maximum gap must be at least the minimum gap.".to_owned(),
            ));
        }
        if self.max_gap_minutes > MINUTES_PER_DAY {
            return Err(ScheduleError::Invalid(
                "The scheduling gap is too large.".to_owned(),
            ));
        }
        Ok(ScheduleConfig {
            window_start,
            window_end,
            min_gap_minutes: self.min_gap_minutes,
            max_gap_minutes: self.max_gap_minutes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduleConfig {
    pub window_start: i64,
    pub window_end: i64,
    pub min_gap_minutes: i64,
    pub max_gap_minutes: i64,
}

#[derive(Debug)]
pub enum ScheduleError {
    Invalid(String),
    LocalTimeUnavailable,
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::LocalTimeUnavailable => {
                formatter.write_str("Pluk could not read this computer's local time.")
            }
        }
    }
}

pub(crate) fn next_slot(
    now_ms: i64,
    latest_scheduled_at: Option<i64>,
    settings: &ScheduleSettings,
) -> Result<i64, ScheduleError> {
    let config = settings.to_config()?;
    let gap = random_gap(config.min_gap_minutes, config.max_gap_minutes);
    next_slot_with_gap(now_ms, latest_scheduled_at, config, gap)
}

fn next_slot_with_gap(
    now_ms: i64,
    latest_scheduled_at: Option<i64>,
    config: ScheduleConfig,
    gap_minutes: i64,
) -> Result<i64, ScheduleError> {
    if now_ms <= 0 || !(config.min_gap_minutes..=config.max_gap_minutes).contains(&gap_minutes) {
        return Err(ScheduleError::Invalid(
            "The scheduling time could not be calculated.".to_owned(),
        ));
    }
    let now = local_datetime_at(now_ms)?;
    let now_minute = now.hour as i64 * 60 + now.minute as i64;
    let mut candidate = if let Some(latest) = latest_scheduled_at.filter(|value| *value > now_ms) {
        add_minutes(local_datetime_at(latest)?, gap_minutes)
    } else if now_minute < config.window_start {
        let after_gap = add_minutes(now, gap_minutes);
        if clock_minute(after_gap) < config.window_start {
            at_clock(now, config.window_start)
        } else {
            after_gap
        }
    } else if now_minute >= config.window_end {
        at_clock(next_day(now), config.window_start)
    } else {
        add_minutes(now, gap_minutes)
    };

    for _ in 0..4 {
        let minute = clock_minute(candidate);
        if minute < config.window_start {
            candidate = at_clock(candidate, config.window_start);
        } else if minute > config.window_end {
            candidate = at_clock(next_day(candidate), config.window_start);
        }
        let candidate_ms = epoch_ms(candidate)?;
        if candidate_ms > now_ms && latest_scheduled_at.is_none_or(|latest| candidate_ms > latest) {
            return Ok(candidate_ms);
        }
        candidate = if clock_minute(candidate) >= config.window_end {
            at_clock(next_day(candidate), config.window_start)
        } else {
            add_minutes(candidate, 1)
        };
    }
    Err(ScheduleError::Invalid(
        "The scheduling time could not be placed in the configured window.".to_owned(),
    ))
}

fn random_gap(minimum: i64, maximum: i64) -> i64 {
    let range = (maximum - minimum + 1) as u128;
    minimum + (Uuid::new_v4().as_u128() % range) as i64
}

fn parse_clock(value: &str) -> Result<i64, ScheduleError> {
    let Some((hours, minutes)) = value.split_once(':') else {
        return Err(ScheduleError::Invalid(
            "Use 24-hour times such as 06:00 and 22:00.".to_owned(),
        ));
    };
    if hours.len() != 2 || minutes.len() != 2 {
        return Err(ScheduleError::Invalid(
            "Use 24-hour times such as 06:00 and 22:00.".to_owned(),
        ));
    }
    let hours = hours.parse::<i64>().map_err(|_| {
        ScheduleError::Invalid("Use 24-hour times such as 06:00 and 22:00.".to_owned())
    })?;
    let minutes = minutes.parse::<i64>().map_err(|_| {
        ScheduleError::Invalid("Use 24-hour times such as 06:00 and 22:00.".to_owned())
    })?;
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) {
        return Err(ScheduleError::Invalid(
            "Use 24-hour times such as 06:00 and 22:00.".to_owned(),
        ));
    }
    Ok(hours * 60 + minutes)
}

#[cfg(test)]
fn format_clock(value: i64) -> Result<String, ScheduleError> {
    if !(0..=MAX_CLOCK_MINUTE).contains(&value) {
        return Err(ScheduleError::Invalid(
            "The scheduling window contains an invalid time.".to_owned(),
        ));
    }
    Ok(format!("{:02}:{:02}", value / 60, value % 60))
}

/// When a slot goes out, in the owner's own clock: "today at 15:40",
/// "tomorrow at 09:10", or "on 2026-09-14 at 09:10". Falls back to the epoch
/// when local time cannot be read.
pub fn describe_slot(scheduled_at: i64, now: i64) -> String {
    let (Ok(slot), Ok(today)) = (local_datetime_at(scheduled_at), local_datetime_at(now)) else {
        return format!("at epoch {scheduled_at}");
    };
    let clock = format!("{:02}:{:02}", slot.hour, slot.minute);
    let same_day = |a: LocalDateTime, b: LocalDateTime| {
        a.year == b.year && a.month == b.month && a.day == b.day
    };
    if same_day(slot, today) {
        return format!("today at {clock}");
    }
    if same_day(slot, next_day(today)) {
        return format!("tomorrow at {clock}");
    }
    format!(
        "on {:04}-{:02}-{:02} at {clock}",
        slot.year, slot.month, slot.day
    )
}

#[derive(Clone, Copy, Debug)]
struct LocalDateTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
}

fn local_datetime_at(epoch_ms: i64) -> Result<LocalDateTime, ScheduleError> {
    os::local_datetime(epoch_ms.div_euclid(1_000)).ok_or(ScheduleError::LocalTimeUnavailable)
}

fn epoch_ms(value: LocalDateTime) -> Result<i64, ScheduleError> {
    os::epoch_seconds(value)
        .and_then(|seconds| seconds.checked_mul(1_000))
        .ok_or(ScheduleError::LocalTimeUnavailable)
}

fn clock_minute(value: LocalDateTime) -> i64 {
    value.hour as i64 * 60 + value.minute as i64
}

fn at_clock(value: LocalDateTime, minute: i64) -> LocalDateTime {
    LocalDateTime {
        hour: (minute / 60) as u32,
        minute: (minute % 60) as u32,
        ..value
    }
}

fn add_minutes(mut value: LocalDateTime, minutes: i64) -> LocalDateTime {
    let total = clock_minute(value) + minutes;
    let days = total.div_euclid(MINUTES_PER_DAY);
    let minute = total.rem_euclid(MINUTES_PER_DAY);
    for _ in 0..days {
        value = next_day(value);
    }
    LocalDateTime {
        hour: (minute / 60) as u32,
        minute: (minute % 60) as u32,
        ..value
    }
}

fn next_day(value: LocalDateTime) -> LocalDateTime {
    if value.day < days_in_month(value.year, value.month) {
        return LocalDateTime {
            day: value.day + 1,
            ..value
        };
    }
    if value.month < 12 {
        return LocalDateTime {
            month: value.month + 1,
            day: 1,
            ..value
        };
    }
    LocalDateTime {
        year: value.year + 1,
        month: 1,
        day: 1,
        ..value
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(unix)]
mod os {
    use std::os::raw::{c_char, c_int, c_long};

    use super::{LocalDateTime, MAX_CLOCK_MINUTE};

    #[repr(C)]
    #[allow(dead_code)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }

    unsafe extern "C" {
        fn localtime_r(timep: *const i64, result: *mut Tm) -> *mut Tm;
        fn mktime(timeptr: *mut Tm) -> i64;
    }

    pub(super) fn local_datetime(seconds: i64) -> Option<LocalDateTime> {
        let mut value = Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        };
        if unsafe { localtime_r(&seconds, &mut value) }.is_null() {
            return None;
        }
        let month = u32::try_from(value.tm_mon + 1).ok()?;
        let day = u32::try_from(value.tm_mday).ok()?;
        let hour = u32::try_from(value.tm_hour).ok()?;
        let minute = u32::try_from(value.tm_min).ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
            return None;
        }
        Some(LocalDateTime {
            year: value.tm_year + 1900,
            month,
            day,
            hour,
            minute,
        })
    }

    pub(super) fn epoch_seconds(value: LocalDateTime) -> Option<i64> {
        let mut raw = Tm {
            tm_sec: 0,
            tm_min: i32::try_from(value.minute).ok()?,
            tm_hour: i32::try_from(value.hour).ok()?,
            tm_mday: i32::try_from(value.day).ok()?,
            tm_mon: i32::try_from(value.month.checked_sub(1)?).ok()?,
            tm_year: value.year.checked_sub(1900)?,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: -1,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        };
        let seconds = unsafe { mktime(&mut raw) };
        (seconds >= 0 && value.hour * 60 + value.minute <= MAX_CLOCK_MINUTE as u32)
            .then_some(seconds)
    }
}

#[cfg(windows)]
mod os {
    use std::os::raw::c_int;

    use super::LocalDateTime;

    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
    }

    #[link(name = "ucrt")]
    unsafe extern "C" {
        fn _localtime64_s(result: *mut Tm, time: *const i64) -> c_int;
        fn _mktime64(time: *mut Tm) -> i64;
    }

    pub(super) fn local_datetime(seconds: i64) -> Option<LocalDateTime> {
        let mut value = Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
        };
        if unsafe { _localtime64_s(&mut value, &seconds) } != 0 {
            return None;
        }
        Some(LocalDateTime {
            year: value.tm_year + 1900,
            month: u32::try_from(value.tm_mon + 1).ok()?,
            day: u32::try_from(value.tm_mday).ok()?,
            hour: u32::try_from(value.tm_hour).ok()?,
            minute: u32::try_from(value.tm_min).ok()?,
        })
    }

    pub(super) fn epoch_seconds(value: LocalDateTime) -> Option<i64> {
        let mut raw = Tm {
            tm_sec: 0,
            tm_min: i32::try_from(value.minute).ok()?,
            tm_hour: i32::try_from(value.hour).ok()?,
            tm_mday: i32::try_from(value.day).ok()?,
            tm_mon: i32::try_from(value.month.checked_sub(1)?).ok()?,
            tm_year: value.year.checked_sub(1900)?,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: -1,
        };
        let seconds = unsafe { _mktime64(&mut raw) };
        (seconds >= 0).then_some(seconds)
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    use super::LocalDateTime;

    pub(super) fn local_datetime(_seconds: i64) -> Option<LocalDateTime> {
        None
    }

    pub(super) fn epoch_seconds(_value: LocalDateTime) -> Option<i64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_formats_settings() {
        let settings = ScheduleSettings::new("06:00", "22:00", 45, 90).unwrap();
        assert_eq!(settings.to_config().unwrap().window_start, 360);
        assert_eq!(
            ScheduleSettings::from_minutes(360, 1320, 45, 90).unwrap(),
            settings
        );
        assert!(ScheduleSettings::new("22:00", "06:00", 45, 90).is_err());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn chooses_a_future_slot_inside_the_local_window() {
        let settings = ScheduleSettings::new("06:00", "22:00", 45, 90).unwrap();
        let now = epoch_ms(LocalDateTime {
            year: 2026,
            month: 9,
            day: 10,
            hour: 10,
            minute: 0,
        })
        .unwrap();
        let slot = next_slot_with_gap(now, None, settings.to_config().unwrap(), 45).unwrap();
        let local = local_datetime_at(slot).unwrap();
        assert_eq!((local.hour, local.minute), (10, 45));
        assert!(slot > now);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn moves_after_the_latest_slot_and_rolls_to_the_next_day() {
        let settings = ScheduleSettings::new("06:00", "22:00", 45, 90).unwrap();
        let now = epoch_ms(LocalDateTime {
            year: 2026,
            month: 9,
            day: 10,
            hour: 21,
            minute: 30,
        })
        .unwrap();
        let latest = epoch_ms(LocalDateTime {
            year: 2026,
            month: 9,
            day: 10,
            hour: 21,
            minute: 30,
        })
        .unwrap();
        let slot =
            next_slot_with_gap(now, Some(latest), settings.to_config().unwrap(), 45).unwrap();
        let local = local_datetime_at(slot).unwrap();
        assert_eq!((local.day, local.hour, local.minute), (11, 6, 0));
    }
}
