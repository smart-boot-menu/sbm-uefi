/// Convert a civil date and time to a Unix timestamp.
///
/// The firmware clock is used as given. Time zones are not applied.
pub fn unix_timestamp(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<i64> {
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month as i32, day as i32);
    Some(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

fn days_from_civil(mut year: i32, month: i32, day: i32) -> i64 {
    year -= i32::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400) as i64;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + i64::from(day_of_year);
    i64::from(era) * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_and_known_dates() {
        assert_eq!(unix_timestamp(1970, 1, 1, 0, 0, 0), Some(0));
        assert_eq!(unix_timestamp(1970, 1, 2, 0, 0, 0), Some(86_400));
        assert_eq!(unix_timestamp(2000, 1, 1, 0, 0, 0), Some(946_684_800));
        assert_eq!(unix_timestamp(1969, 12, 31, 0, 0, 0), None);
        assert_eq!(unix_timestamp(2026, 13, 1, 0, 0, 0), None);
    }
}
