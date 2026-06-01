use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc, Weekday};

/// Expected latest completed US equity daily session for vendor EOD bars.
///
/// We intentionally use a conservative UTC close cutoff. Before 22:00 UTC,
/// today's daily bar is not expected yet, so the latest expected session is the
/// previous trading day. This avoids flagging pre-market Monday as stale when
/// Friday is the newest complete bar.
#[must_use]
pub fn expected_latest_us_equity_session(now: DateTime<Utc>) -> NaiveDate {
    let cutoff = NaiveTime::from_hms_opt(22, 0, 0).expect("valid cutoff");
    let mut candidate = now.date_naive();
    if now.time() < cutoff {
        candidate -= Duration::days(1);
    }
    previous_us_equity_session(candidate)
}

#[must_use]
pub fn previous_us_equity_session(mut day: NaiveDate) -> NaiveDate {
    while !is_us_equity_session(day) {
        day -= Duration::days(1);
    }
    day
}

#[must_use]
pub fn is_us_equity_session(day: NaiveDate) -> bool {
    !matches!(day.weekday(), Weekday::Sat | Weekday::Sun) && !is_nyse_holiday(day)
}

fn is_nyse_holiday(day: NaiveDate) -> bool {
    // Include adjacent-year New Year observations, e.g. Jan 1 observed on Dec 31.
    nyse_holidays(day.year()).contains(&day) || nyse_holidays(day.year() + 1).contains(&day)
}

fn nyse_holidays(year: i32) -> Vec<NaiveDate> {
    let mut out = vec![
        observed_fixed(year, 1, 1),
        nth_weekday(year, 1, Weekday::Mon, 3), // MLK Day
        nth_weekday(year, 2, Weekday::Mon, 3), // Presidents' Day
        good_friday(year),
        last_weekday(year, 5, Weekday::Mon), // Memorial Day
        observed_fixed(year, 7, 4),
        nth_weekday(year, 9, Weekday::Mon, 1),  // Labor Day
        nth_weekday(year, 11, Weekday::Thu, 4), // Thanksgiving
        observed_fixed(year, 12, 25),
    ];
    if year >= 2022 {
        out.push(observed_fixed(year, 6, 19));
    }
    out
}

fn observed_fixed(year: i32, month: u32, day: u32) -> NaiveDate {
    let actual = NaiveDate::from_ymd_opt(year, month, day).expect("valid fixed holiday");
    match actual.weekday() {
        Weekday::Sat => actual - Duration::days(1),
        Weekday::Sun => actual + Duration::days(1),
        _ => actual,
    }
}

fn nth_weekday(year: i32, month: u32, weekday: Weekday, nth: u32) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let offset = (7 + weekday.num_days_from_monday() as i64
        - first.weekday().num_days_from_monday() as i64)
        % 7;
    first + Duration::days(offset + 7 * (nth as i64 - 1))
}

fn last_weekday(year: i32, month: u32, weekday: Weekday) -> NaiveDate {
    let next_month = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1).expect("valid next year")
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1).expect("valid next month")
    };
    let last = next_month - Duration::days(1);
    let offset = (7 + last.weekday().num_days_from_monday() as i64
        - weekday.num_days_from_monday() as i64)
        % 7;
    last - Duration::days(offset)
}

fn good_friday(year: i32) -> NaiveDate {
    // Anonymous Gregorian computus.
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = ((h + l - 7 * m + 114) % 31) + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("valid easter")
        - Duration::days(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn monday_premarket_expects_prior_friday() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap();
        assert_eq!(
            expected_latest_us_equity_session(now),
            NaiveDate::from_ymd_opt(2026, 5, 29).unwrap()
        );
    }

    #[test]
    fn after_cutoff_expects_same_trading_day() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 22, 30, 0).unwrap();
        assert_eq!(
            expected_latest_us_equity_session(now),
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()
        );
    }

    #[test]
    fn holidays_roll_back_to_prior_session() {
        assert_eq!(
            previous_us_equity_session(NaiveDate::from_ymd_opt(2026, 11, 26).unwrap()),
            NaiveDate::from_ymd_opt(2026, 11, 25).unwrap()
        );
    }
}
