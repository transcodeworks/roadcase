//! Dates, without a calendar crate.

/// Seconds since 1970 now, or `$SOURCE_DATE_EPOCH` when it is set, so that
/// a rebuilt repository can come out the same.
pub(crate) fn now() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default()
        })
}

/// RFC 2822 in UTC, as apt's `Release` has it: `Tue, 17 Nov 2026 12:00:00 UTC`.
pub(crate) fn rfc2822(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} UTC",
        WEEKDAYS[days.rem_euclid(7) as usize],
        MONTHS[(month - 1) as usize],
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_are_written_as_apt_reads_them() {
        assert_eq!(super::rfc2822(0), "Thu, 01 Jan 1970 00:00:00 UTC");
        assert_eq!(
            super::rfc2822(1_795_435_200),
            "Mon, 23 Nov 2026 12:00:00 UTC"
        );
        assert_eq!(super::rfc2822(951_782_400), "Tue, 29 Feb 2000 00:00:00 UTC");
    }
}
