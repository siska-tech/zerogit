//! The current time, the local time zone offset and Git's date formats.
//!
//! The offset of the local time zone is read from the operating system
//! without extra dependencies: `localtime_r` on Unix and
//! `SystemTimeToTzSpecificLocalTime` on Windows. Both take the daylight
//! saving rules in effect at the given time into account, as Git does.

/// The current time in seconds since the Unix epoch.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The offset of the local time zone from UTC, in minutes, at `timestamp`
/// (seconds since the Unix epoch). East of UTC is positive (`+0900` is
/// 540). Where the offset cannot be determined, it is 0 (UTC).
pub(crate) fn local_offset_minutes(timestamp: i64) -> i32 {
    sys::local_seconds(timestamp)
        .map(|local| ((local - timestamp) / 60) as i32)
        .unwrap_or(0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = month as i64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The date (year, month, day) of a day count since 1970-01-01.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Seconds since the epoch of a date and time taken as UTC.
fn seconds_of(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    days_from_civil(year, month, day) * 86400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second)
}

/// Parses a date as Git accepts it in `GIT_AUTHOR_DATE` and
/// `GIT_COMMITTER_DATE`, returning the timestamp and the offset in minutes.
///
/// Supported forms:
/// - Git's internal format: `<seconds> <+hhmm>`, optionally with `@`
///   before the seconds.
/// - ISO 8601: `2005-04-07T22:13:13`, with a space instead of `T`, optional
///   seconds and fraction, and `Z`, `+hh`, `+hhmm` or `+hh:mm`.
/// - RFC 2822: `Thu, 07 Apr 2005 22:13:13 +0200` (the weekday is optional).
///
/// A date without a zone is in the local time zone.
pub(crate) fn parse_git_date(text: &str) -> Option<(i64, i32)> {
    let text = text.trim();
    parse_raw(text)
        .or_else(|| parse_iso(text))
        .or_else(|| parse_rfc2822(text))
}

fn parse_raw(text: &str) -> Option<(i64, i32)> {
    let text = text.strip_prefix('@').unwrap_or(text);
    let mut parts = text.split_whitespace();
    let seconds = parts.next()?;
    if seconds.is_empty() || !seconds.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: i64 = seconds.parse().ok()?;
    let offset = match parts.next() {
        Some(zone) => parse_zone(zone)?,
        None => local_offset_minutes(seconds),
    };
    if parts.next().is_some() {
        return None;
    }
    Some((seconds, offset))
}

/// `+hhmm`, `-hhmm`, `+hh:mm`, `+hh` or `Z`, in minutes.
fn parse_zone(zone: &str) -> Option<i32> {
    if zone == "Z" || zone == "z" {
        return Some(0);
    }
    let (sign, rest) = match zone.as_bytes().first()? {
        b'+' => (1, &zone[1..]),
        b'-' => (-1, &zone[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|&c| c != ':').collect();
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (hours, minutes) = match digits.len() {
        2 => (digits.parse::<i32>().ok()?, 0),
        4 => (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        ),
        _ => return None,
    };
    if hours > 14 || minutes >= 60 {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

fn number(text: &str, len: usize) -> Option<u32> {
    if text.len() != len || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Seconds since the epoch of a local or zoned date and time.
fn resolve(
    (year, month, day): (i64, u32, u32),
    (hour, minute, second): (u32, u32, u32),
    zone: Option<i32>,
) -> Option<(i64, i32)> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let wall = seconds_of(year, month, day, hour, minute, second);
    let offset = match zone {
        Some(offset) => offset,
        // The offset at the wall time read as UTC is the right one except
        // within hours of a daylight saving change; check it once more.
        None => {
            let guess = local_offset_minutes(wall);
            local_offset_minutes(wall - i64::from(guess) * 60)
        }
    };
    Some((wall - i64::from(offset) * 60, offset))
}

fn parse_iso(text: &str) -> Option<(i64, i32)> {
    let (date, rest) = text.split_at(text.find(['T', 't', ' '])?);
    let rest = &rest[1..];
    let mut ymd = date.split('-');
    let year = number(ymd.next()?, 4)? as i64;
    let month = number(ymd.next()?, 2)?;
    let day = number(ymd.next()?, 2)?;
    if ymd.next().is_some() {
        return None;
    }

    // The time ends where the zone starts.
    let rest = rest.trim();
    let zone_start = rest.find(['Z', 'z', '+', '-', ' ']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(zone_start);
    let zone = zone.trim();
    let zone = if zone.is_empty() {
        None
    } else {
        Some(parse_zone(zone)?)
    };
    let time = time.split('.').next()?;
    let mut hms = time.split(':');
    let hour = number(hms.next()?, 2)?;
    let minute = number(hms.next()?, 2)?;
    let second = match hms.next() {
        Some(s) => number(s, 2)?,
        None => 0,
    };
    if hms.next().is_some() {
        return None;
    }
    resolve((year, month, day), (hour, minute, second), zone)
}

fn parse_rfc2822(text: &str) -> Option<(i64, i32)> {
    let text = match text.split_once(',') {
        Some((_, rest)) => rest,
        None => text,
    };
    let mut parts = text.split_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let month_name = parts.next()?.to_ascii_lowercase();
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| month_name.starts_with(m))? as u32
        + 1;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut hms = parts.next()?.split(':');
    let hour: u32 = hms.next()?.parse().ok()?;
    let minute: u32 = hms.next()?.parse().ok()?;
    let second: u32 = match hms.next() {
        Some(s) => s.parse().ok()?,
        None => 0,
    };
    let zone = match parts.next() {
        Some(zone) => Some(parse_zone(zone)?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    resolve((year, month, day), (hour, minute, second), zone)
}

#[cfg(all(unix, target_pointer_width = "64"))]
mod sys {
    use std::os::raw::{c_int, c_long};

    /// The leading, standard fields of `struct tm`, with room for the
    /// platform-specific fields that follow them.
    #[allow(dead_code)] // Written by `localtime_r`; only some fields are read.
    #[repr(C, align(8))]
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
        _rest: [u8; 64],
    }

    extern "C" {
        fn localtime_r(time: *const c_long, result: *mut Tm) -> *mut Tm;
    }

    /// The local wall clock time at `timestamp`, read as UTC seconds.
    pub(super) fn local_seconds(timestamp: i64) -> Option<i64> {
        let time = timestamp as c_long;
        let mut tm = Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            _rest: [0; 64],
        };
        // SAFETY: `time` is a valid `time_t` (a 64-bit `long` on these
        // targets) and `tm` is larger than, and as aligned as, any
        // platform's `struct tm`; `localtime_r` only writes into it.
        let result = unsafe { localtime_r(&time, &mut tm) };
        if result.is_null() {
            return None;
        }
        Some(super::seconds_of(
            i64::from(tm.tm_year) + 1900,
            (tm.tm_mon + 1) as u32,
            tm.tm_mday as u32,
            tm.tm_hour as u32,
            tm.tm_min as u32,
            tm.tm_sec as u32,
        ))
    }
}

#[cfg(windows)]
mod sys {
    use std::os::raw::{c_int, c_void};

    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SystemTimeToTzSpecificLocalTime(
            time_zone: *const c_void,
            universal: *const SystemTime,
            local: *mut SystemTime,
        ) -> c_int;
    }

    /// The local wall clock time at `timestamp`, read as UTC seconds.
    pub(super) fn local_seconds(timestamp: i64) -> Option<i64> {
        let days = timestamp.div_euclid(86400);
        let secs = timestamp.rem_euclid(86400);
        let (year, month, day) = super::civil_from_days(days);
        // SYSTEMTIME covers the years 1601 to 30827.
        let year = u16::try_from(year).ok().filter(|y| *y >= 1601)?;
        let universal = SystemTime {
            year,
            month: month as u16,
            day: day as u16,
            hour: (secs / 3600) as u16,
            minute: (secs % 3600 / 60) as u16,
            second: (secs % 60) as u16,
            ..SystemTime::default()
        };
        let mut local = SystemTime::default();
        // SAFETY: both pointers refer to valid SYSTEMTIME structures; a null
        // time zone selects the current one.
        let ok =
            unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &universal, &mut local) };
        if ok == 0 {
            return None;
        }
        Some(super::seconds_of(
            i64::from(local.year),
            u32::from(local.month),
            u32::from(local.day),
            u32::from(local.hour),
            u32::from(local.minute),
            u32::from(local.second),
        ))
    }
}

#[cfg(not(any(all(unix, target_pointer_width = "64"), windows)))]
mod sys {
    /// The local time zone is unknown here; dates are written in UTC.
    pub(super) fn local_seconds(_timestamp: i64) -> Option<i64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        for days in [-800_000, -1, 0, 1, 59, 60, 11016, 11017, 19_723, 2_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn local_offset_is_whole_minutes_within_a_day() {
        for t in [0, 1_700_000_000, 1_719_792_000] {
            let offset = local_offset_minutes(t);
            assert!(offset.abs() < 24 * 60, "{}", offset);
        }
    }

    #[test]
    fn parses_raw_dates() {
        assert_eq!(parse_git_date("1700000000 +0900"), Some((1700000000, 540)));
        assert_eq!(parse_git_date("@1700000000 -0130"), Some((1700000000, -90)));
        assert_eq!(parse_git_date("1700000000 +09:00"), Some((1700000000, 540)));
        assert_eq!(parse_git_date("1700000000 +0960"), None);
        assert_eq!(parse_git_date("1700000000 +0900 x"), None);
    }

    #[test]
    fn parses_iso_dates() {
        assert_eq!(
            parse_git_date("2024-01-01T00:00:00Z"),
            Some((1704067200, 0))
        );
        assert_eq!(
            parse_git_date("2005-04-07T22:13:13+0200"),
            Some((1112904793, 120))
        );
        assert_eq!(
            parse_git_date("2005-04-07 22:13:13 +02:00"),
            Some((1112904793, 120))
        );
        assert_eq!(
            parse_git_date("2005-04-07T22:13:13.250-05"),
            Some((1112929993, -300))
        );
        assert_eq!(parse_git_date("2005-04-07T22:13Z"), Some((1112911980, 0)));
        assert_eq!(parse_git_date("2005-13-07T22:13:13Z"), None);
        // Without a zone, the date is local time.
        let (t, offset) = parse_git_date("2024-06-01T12:00:00").unwrap();
        assert_eq!(t + i64::from(offset) * 60, 1717243200);
    }

    #[test]
    fn parses_rfc2822_dates() {
        assert_eq!(
            parse_git_date("Thu, 07 Apr 2005 22:13:13 +0200"),
            Some((1112904793, 120))
        );
        assert_eq!(
            parse_git_date("7 Apr 2005 22:13:13 -0000"),
            Some((1112911993, 0))
        );
        assert_eq!(parse_git_date("tomorrow"), None);
        assert_eq!(parse_git_date(""), None);
    }
}
