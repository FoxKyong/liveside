//! Where a new day starts in a channel, so a line from last week does not
//! read as one from this evening: the clock beside a line says only the
//! hour.
//!
//! Both frontends draw the same rule: a line saying the date goes before
//! the first line of a day, and before the first line on screen when that
//! one is not from today.

use jiff::Timestamp as JiffTimestamp;
pub use jiff::civil::Date;
use jiff::civil::Weekday;
use jiff::tz::TimeZone;
use liveside_proto::Timestamp;

use crate::lang::{Strings, fill};

/// The local day of a moment, with the offset in force then.
pub fn day_of(time: Timestamp) -> Option<Date> {
    JiffTimestamp::from_millisecond(time.0)
        .ok()
        .map(|stamp| stamp.to_zoned(TimeZone::system()).date())
}

/// The day to announce before a line from `this` day, after a line from
/// `previous`; nothing when it is the same day. The first line of a view
/// (`previous` is `None`) is announced only when it is not from `today`.
pub fn day_starts(previous: Option<Date>, this: Date, today: Date) -> Option<Date> {
    match previous {
        Some(previous) if previous == this => None,
        None if this == today => None,
        _ => Some(this),
    }
}

/// Today, where this machine is.
pub fn today() -> Date {
    jiff::Zoned::now().date()
}

/// The date, the way the language writes it, with the day of the week.
pub fn day_line(strings: &Strings, date: Date) -> String {
    let weekday = match date.weekday() {
        Weekday::Monday => strings.monday,
        Weekday::Tuesday => strings.tuesday,
        Weekday::Wednesday => strings.wednesday,
        Weekday::Thursday => strings.thursday,
        Weekday::Friday => strings.friday,
        Weekday::Saturday => strings.saturday,
        Weekday::Sunday => strings.sunday,
    };
    // The numbers are strftime's, so each language puts them its own way
    // with the same one placeholder; a pattern it cannot read shows the
    // date the way most of the world can.
    let pattern = fill(strings.day_line, &[("weekday", weekday)]);
    jiff::fmt::strtime::format(pattern.as_bytes(), date)
        .unwrap_or_else(|_| format!("{weekday} {date}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Language;
    use jiff::civil::date;

    #[test]
    fn a_new_day_is_said_once_and_today_needs_no_saying_at_the_top() {
        let today = date(2026, 10, 29);
        let before = date(2026, 10, 11);
        assert_eq!(day_starts(None, today, today), None);
        assert_eq!(day_starts(None, before, today), Some(before));
        assert_eq!(day_starts(Some(before), before, today), None);
        assert_eq!(day_starts(Some(before), today, today), Some(today));
    }

    #[test]
    fn the_date_is_written_the_way_the_language_writes_it() {
        let day = date(2026, 10, 7);
        assert_eq!(
            day_line(Language::English.strings(), day),
            "Wednesday, 2026-10-07"
        );
        assert_eq!(
            day_line(Language::Czech.strings(), day),
            "Středa 7. 10. 2026"
        );
        for language in Language::ALL {
            let line = day_line(language.strings(), day);
            assert!(
                line.contains("2026") && !line.contains('%') && !line.contains('{'),
                "{language:?}: {line}"
            );
        }
    }
}
