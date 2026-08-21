//! Minimal calendar helpers for displaying unix timestamps, replacing chrono.

const SECS_PER_DAY: i64 = 86_400;

const MONTH_ABBREV: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A calendar date and time of day, in some timezone.
#[derive(Clone, Copy)]
struct Civil {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl Civil {
    fn month_abbrev(&self) -> &'static str {
        MONTH_ABBREV[self.month as usize - 1]
    }
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Days since 1970-01-01 for a Gregorian calendar date.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    // Standard civil-calendar conversion, working in 400-year eras whose day
    // count is fixed, with years starting in March so leap days come last.
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * i64::from(shifted_month) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Gregorian calendar date for a number of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    ((year + i64::from(month <= 2)) as i32, month, day)
}

fn civil_utc(unix: i64) -> Civil {
    let (year, month, day) = civil_from_days(unix.div_euclid(SECS_PER_DAY));
    let secs = unix.rem_euclid(SECS_PER_DAY) as u32;
    Civil {
        year,
        month,
        day,
        hour: secs / 3600,
        minute: secs / 60 % 60,
        second: secs % 60,
    }
}

#[cfg(unix)]
fn civil_local(unix: i64) -> Civil {
    let t = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return civil_utc(unix);
    }
    Civil {
        year: tm.tm_year + 1900,
        month: tm.tm_mon as u32 + 1,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    }
}

#[cfg(windows)]
fn civil_local(unix: i64) -> Civil {
    use windows_sys::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
    };

    // Windows FILETIME counts 100ns ticks since 1601-01-01.
    const UNIX_EPOCH_AS_FILETIME_SECS: i64 = 11_644_473_600;
    let ticks = match (unix + UNIX_EPOCH_AS_FILETIME_SECS).checked_mul(10_000_000) {
        Some(ticks) if ticks >= 0 => ticks as u64,
        _ => return civil_utc(unix),
    };
    let filetime = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let converted = unsafe {
        FileTimeToSystemTime(&filetime, &mut utc) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
    };
    if !converted {
        return civil_utc(unix);
    }
    Civil {
        year: local.wYear as i32,
        month: local.wMonth as u32,
        day: local.wDay as u32,
        hour: local.wHour as u32,
        minute: local.wMinute as u32,
        second: local.wSecond as u32,
    }
}

/// Unix timestamp of midnight UTC for the given date, `None` if the date is invalid.
pub fn ymd_to_unix(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * SECS_PER_DAY)
}

/// Format a date as "Mar 12, 2026", UTC.
pub fn format_date(unix: i64) -> String {
    let c = civil_utc(unix);
    format!("{} {}, {}", c.month_abbrev(), c.day, c.year)
}

/// Format a date and time as "Mar. 12, 2026 - 14:03:05", local time.
pub fn format_date_time(unix: i64) -> String {
    let c = civil_local(unix);
    format!(
        "{}. {:02}, {} - {:02}:{:02}:{:02}",
        c.month_abbrev(),
        c.day,
        c.year,
        c.hour,
        c.minute,
        c.second
    )
}

/// Format a date and time as "Mar 12, 2026 at 14:30", local time.
pub fn format_date_at_time(unix: i64) -> String {
    let c = civil_local(unix);
    format!(
        "{} {}, {} at {:02}:{:02}",
        c.month_abbrev(),
        c.day,
        c.year,
        c.hour,
        c.minute
    )
}

/// Format a date and time as "2026-03-12 14:03:05", UTC. Used for CSV exports.
pub fn format_export_date_time(unix: i64) -> String {
    let c = civil_utc(unix);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

/// Format a date and time as "2026-03-12T14-03-05", local time. Used for default filenames.
pub fn format_filename_date_time(unix: i64) -> String {
    let c = civil_local(unix);
    format!(
        "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ymd_to_unix_values() {
        assert_eq!(ymd_to_unix(1970, 1, 1), Some(0));
        assert_eq!(ymd_to_unix(2000, 1, 1), Some(946_684_800));
        assert_eq!(ymd_to_unix(2024, 2, 29), Some(1_709_164_800));
        assert_eq!(ymd_to_unix(2024, 4, 30), Some(1_714_435_200));
        assert_eq!(ymd_to_unix(2024, 12, 31), Some(1_735_603_200));
        // Invalid dates: non-leap Feb 29, Feb 30, bad months, day past month end.
        assert_eq!(ymd_to_unix(2023, 2, 29), None);
        assert_eq!(ymd_to_unix(2024, 2, 30), None);
        assert_eq!(ymd_to_unix(2024, 0, 1), None);
        assert_eq!(ymd_to_unix(2024, 13, 1), None);
        assert_eq!(ymd_to_unix(2024, 4, 31), None);
        assert_eq!(ymd_to_unix(2024, 1, 0), None);
    }

    #[test]
    fn utc_formatting() {
        assert_eq!(format_date(0), "Jan 1, 1970");
        assert_eq!(format_date(1_709_210_096), "Feb 29, 2024");
        assert_eq!(format_date(1_787_320_985), "Aug 21, 2026");
        assert_eq!(format_export_date_time(0), "1970-01-01 00:00:00");
        assert_eq!(
            format_export_date_time(1_709_210_096),
            "2024-02-29 12:34:56"
        );
        assert_eq!(
            format_export_date_time(1_787_320_985),
            "2026-08-21 14:03:05"
        );
        assert_eq!(format_export_date_time(-1), "1969-12-31 23:59:59");
    }

    #[cfg(unix)]
    #[test]
    fn local_formatting_respects_tz() {
        // Not declared by the libc crate for unix targets.
        extern "C" {
            fn tzset();
        }
        std::env::set_var("TZ", "America/New_York");
        unsafe { tzset() };
        // 2024-03-10 06:30:00 UTC is 01:30:00 EST, half an hour before the DST jump.
        assert_eq!(format_date_time(1_710_052_200), "Mar. 10, 2024 - 01:30:00");
        assert_eq!(
            format_filename_date_time(1_710_052_200),
            "2024-03-10T01-30-00"
        );
        // 2024-03-10 07:30:00 UTC is 03:30:00 EDT, just after the jump.
        assert_eq!(format_date_time(1_710_055_800), "Mar. 10, 2024 - 03:30:00");
        assert_eq!(format_date_at_time(1_710_055_800), "Mar 10, 2024 at 03:30");
    }
}
