// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a date *is*. What it *means* under a zone belongs to `ical-tz`.
//!
//! These types sit here rather than in `ical-tz` because of the crate graph and not because
//! of taste: `ical-recur` and `ical-tz` are siblings, `ical-dav` needs an instant for
//! `time-range` filters and does not depend on `ical-tz`, and an inherent method cannot be
//! added from downstream. So the common root owns the types and the crate above owns the
//! meaning (`docs/adr/0011`).
//!
//! Every construction here is validated and total: an impossible date is `None`, never a
//! silently clamped one. The checked arithmetic over these types — days from the epoch, the
//! weekday, adding months, converting against an offset — is implemented alongside them and
//! obeys the same rule, so that a recurrence instance which does not exist is filtered per
//! RFC 5545 section 3.3.10 rather than moved to a nearby one.

use crate::view::ValueType;

/// The last year expressible as the four digits RFC 5545 section 3.3.4 gives a `DATE`.
///
/// Bounded on construction rather than at write time, so a value that cannot be written back
/// cannot be built in the first place.
const MAX_YEAR: u16 = 9999;

/// A date in the proleptic Gregorian calendar, with no zone and no time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    /// Year, `0` through [`MAX_YEAR`].
    year: u16,
    /// Month, `1` through `12`.
    month: u8,
    /// Day, `1` through the length of the month.
    day: u8,
}

impl CivilDate {
    /// The date, or `None` when there is no such date.
    ///
    /// February 30th is `None` and never February 28th. A calendar that clamps here would
    /// turn a malformed export into a plausible wrong answer, which is the failure mode this
    /// whole crate is arranged against.
    #[must_use]
    pub const fn from_ymd(year: u16, month: u8, day: u8) -> Option<Self> {
        if year > MAX_YEAR || day == 0 {
            return None;
        }
        match Self::days_in_month(year, month) {
            Some(length) if day <= length => Some(Self { year, month, day }),
            _ => None,
        }
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }

    /// The month, `1` through `12`.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The day of the month, from `1`.
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }

    /// Whether `year` is a leap year in the proleptic Gregorian calendar.
    #[must_use]
    pub const fn is_leap_year(year: u16) -> bool {
        year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    }

    /// How many days `month` has in `year`, or `None` when there is no such month.
    #[must_use]
    pub const fn days_in_month(year: u16, month: u8) -> Option<u8> {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
            4 | 6 | 9 | 11 => Some(30),
            2 if Self::is_leap_year(year) => Some(29),
            2 => Some(28),
            _ => None,
        }
    }
}

/// A time of day, with no zone and no date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilTime {
    /// Hour, `0` through `23`.
    hour: u8,
    /// Minute, `0` through `59`.
    minute: u8,
    /// Second, `0` through `60`.
    second: u8,
}

impl CivilTime {
    /// Midnight.
    pub const MIDNIGHT: Self = Self {
        hour: 0,
        minute: 0,
        second: 0,
    };

    /// The time, or `None` when there is no such time.
    ///
    /// Second `60` is accepted because RFC 5545 section 3.3.12 accepts it for a positive leap
    /// second. Rejecting it would make a conforming file unreadable to prove a point about
    /// timekeeping that the format already decided.
    #[must_use]
    pub const fn from_hms(hour: u8, minute: u8, second: u8) -> Option<Self> {
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        Some(Self {
            hour,
            minute,
            second,
        })
    }

    /// The hour.
    #[must_use]
    pub const fn hour(self) -> u8 {
        self.hour
    }

    /// The minute.
    #[must_use]
    pub const fn minute(self) -> u8 {
        self.minute
    }

    /// The second, which may be `60`.
    #[must_use]
    pub const fn second(self) -> u8 {
        self.second
    }
}

/// A date and a time of day together, with no zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDateTime {
    /// The date part.
    date: CivilDate,
    /// The time part.
    time: CivilTime,
}

impl CivilDateTime {
    /// The two parts together.
    #[must_use]
    pub const fn new(date: CivilDate, time: CivilTime) -> Self {
        Self { date, time }
    }

    /// The date part.
    #[must_use]
    pub const fn date(self) -> CivilDate {
        self.date
    }

    /// The time part.
    #[must_use]
    pub const fn time(self) -> CivilTime {
        self.time
    }
}

/// A fixed offset from UTC, as RFC 5545 section 3.3.14 writes one.
///
/// A fixed offset is not a time zone. It cannot say when a transition happens, which is why
/// resolving a `TZID` is `ical-tz`'s job and produces one of these rather than being one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcOffset {
    /// Seconds east of UTC, negative for west.
    seconds: i32,
}

impl UtcOffset {
    /// No offset at all.
    pub const UTC: Self = Self { seconds: 0 };

    /// The offset, or `None` when it is a day or more from UTC.
    ///
    /// RFC 5545 section 3.3.14 writes an offset as at most `+hhmmss`, so a whole day is not
    /// expressible and a value that reached one came from arithmetic that went wrong.
    #[must_use]
    pub const fn from_seconds(seconds: i32) -> Option<Self> {
        if seconds <= -86_400 || seconds >= 86_400 {
            return None;
        }
        Some(Self { seconds })
    }

    /// Seconds east of UTC.
    #[must_use]
    pub const fn seconds(self) -> i32 {
        self.seconds
    }
}

/// A span of time with no year and no month field, as RFC 5545 section 3.3.6 defines one.
///
/// The absence is the point. Section 3.3.6's ABNF has no `Y` and no `M` designator, so `P1M`
/// is not a value this type could hold, and "add a month to a date" is closed off at the type
/// level rather than in a review comment. Adding months is [`CivilDate`]'s operation, and it
/// answers with a [`MonthAddOutcome`] that can say the day did not exist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration {
    /// Whole days.
    days: i64,
    /// Seconds beyond those days.
    seconds: i64,
}

impl Duration {
    /// No time at all.
    pub const ZERO: Self = Self {
        days: 0,
        seconds: 0,
    };

    /// A span of `days` days and `seconds` seconds.
    ///
    /// The two parts are kept as written rather than normalized into one, because a producer
    /// that wrote `P1DT0H` and one that wrote `PT24H` wrote different text and both get their
    /// own back.
    #[must_use]
    pub const fn new(days: i64, seconds: i64) -> Self {
        Self { days, seconds }
    }

    /// Whole days.
    #[must_use]
    pub const fn days(self) -> i64 {
        self.days
    }

    /// Seconds beyond the whole days.
    #[must_use]
    pub const fn seconds(self) -> i64 {
        self.seconds
    }
}

/// A day of the week, ordered as RFC 5545 section 3.3.10's `BYDAY` orders one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    /// `MO`.
    Monday,
    /// `TU`.
    Tuesday,
    /// `WE`.
    Wednesday,
    /// `TH`.
    Thursday,
    /// `FR`.
    Friday,
    /// `SA`.
    Saturday,
    /// `SU`.
    Sunday,
}

impl Weekday {
    /// Every weekday, Monday first.
    pub const ALL: [Self; 7] = [
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
        Self::Sunday,
    ];

    /// The two-letter name RFC 5545 section 3.3.10 writes.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8; 2] {
        match self {
            Self::Monday => b"MO",
            Self::Tuesday => b"TU",
            Self::Wednesday => b"WE",
            Self::Thursday => b"TH",
            Self::Friday => b"FR",
            Self::Saturday => b"SA",
            Self::Sunday => b"SU",
        }
    }

    /// Position in the week counted from Monday, which is `0`.
    ///
    /// Monday rather than Sunday because `WKST` defaults to Monday, and a second origin would
    /// be a second place for an off-by-one to live.
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }

    /// The weekday at `index` counted from Monday, or `None` past the end of the week.
    #[must_use]
    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Monday),
            1 => Some(Self::Tuesday),
            2 => Some(Self::Wednesday),
            3 => Some(Self::Thursday),
            4 => Some(Self::Friday),
            5 => Some(Self::Saturday),
            6 => Some(Self::Sunday),
            _ => None,
        }
    }
}

/// What happened when months were added to a date.
///
/// Three answers rather than an `Option`, because "the 31st of a 30-day month" and "the year
/// 12000" are different failures and a recurrence rule treats them differently. `Clamped`
/// keeps the day that was asked for so that a caller obeying RFC 5545 section 3.3.10 can drop
/// the instance and still say why it vanished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MonthAddOutcome {
    /// The requested day exists in the resulting month.
    Exact(CivilDate),
    /// The requested day does not exist in the resulting month.
    Clamped {
        /// The last day of the resulting month.
        date: CivilDate,
        /// The day that was asked for and is not there.
        requested_day: u8,
    },
    /// The resulting year is not representable.
    Overflow,
}

impl MonthAddOutcome {
    /// The resulting date, `None` only on overflow.
    ///
    /// A caller that wants section 3.3.10's filtering rule must match on the variant instead:
    /// this accessor deliberately hands back the clamped date, and taking it as the answer is
    /// the coercion the specification forbids.
    #[must_use]
    pub const fn date(self) -> Option<CivilDate> {
        match self {
            Self::Exact(date) | Self::Clamped { date, .. } => Some(date),
            Self::Overflow => None,
        }
    }
}

/// A property value that is a date or a date-time, in the four shapes RFC 5545 gives one.
///
/// Four rather than the three forms section 3.3.4 and section 3.3.5 write, because a
/// date-time's *meaning* is decided by the `TZID` parameter beside it as much as by the octets
/// of the value, and `docs/adr/0001` requires that a date-time cannot be constructed apart from
/// the parameter set it implies. A floating time and a zoned one are written identically and
/// are not the same value; keeping them one variant made [`EncodeValue::coupled_parameters`]
/// unable to state a `TZID`, so every write of a zoned `DTSTART` dropped the zone.
///
/// [`EncodeValue::coupled_parameters`]: crate::EncodeValue::coupled_parameters
///
/// The zone identifier is borrowed rather than owned so that this type stays `Copy` — it is a
/// parameter's octets, held by the property the value was read from or by the caller writing
/// one. It is not resolved here and cannot be: a `TZID` names a zone that only a caller-supplied
/// source can turn into an offset (`docs/adr/0003`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DateTimeValue<'a> {
    /// A `DATE`, written under `VALUE=DATE`.
    Date(CivilDate),
    /// A floating `DATE-TIME`, with no `Z` and no `TZID`, which is a wall clock anywhere.
    Local(CivilDateTime),
    /// A `DATE-TIME` in UTC, written with a trailing `Z`.
    Utc(CivilDateTime),
    /// A `DATE-TIME` read under the zone a `TZID` parameter names.
    Zoned {
        /// The wall clock the value's octets spell.
        stamp: CivilDateTime,
        /// The zone identifier, exactly as the parameter carried it, `DQUOTE`s removed.
        tzid: &'a [u8],
    },
}

impl<'a> DateTimeValue<'a> {
    /// The date part, whichever shape this is.
    #[must_use]
    pub const fn date(self) -> CivilDate {
        match self {
            Self::Date(date) => date,
            Self::Local(stamp) | Self::Utc(stamp) | Self::Zoned { stamp, .. } => stamp.date(),
        }
    }

    /// The time part, `None` for a `DATE`.
    #[must_use]
    pub const fn time(self) -> Option<CivilTime> {
        match self {
            Self::Date(_) => None,
            Self::Local(stamp) | Self::Utc(stamp) | Self::Zoned { stamp, .. } => Some(stamp.time()),
        }
    }

    /// The zone identifier this value was read under, `None` for the three that carry none.
    ///
    /// A `DATE` and a UTC date-time carry no zone because neither can; a floating date-time
    /// carries none because it asserts the absence of one, which is a claim rather than a gap.
    #[must_use]
    pub const fn tzid(self) -> Option<&'a [u8]> {
        match self {
            Self::Date(_) | Self::Local(_) | Self::Utc(_) => None,
            Self::Zoned { tzid, .. } => Some(tzid),
        }
    }

    /// The value type this shape is written under, which a write has to emit as a parameter.
    #[must_use]
    pub const fn value_type(self) -> ValueType {
        match self {
            Self::Date(_) => ValueType::Date,
            Self::Local(_) | Self::Utc(_) | Self::Zoned { .. } => ValueType::DateTime,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CivilDate, CivilTime, DateTimeValue, MonthAddOutcome, UtcOffset, Weekday};
    use crate::view::ValueType;

    #[test]
    fn an_impossible_date_is_refused_rather_than_clamped() {
        assert_eq!(CivilDate::from_ymd(2026, 2, 30), None);
        assert_eq!(CivilDate::from_ymd(2026, 13, 1), None);
        assert_eq!(CivilDate::from_ymd(2026, 1, 0), None);
        assert!(CivilDate::from_ymd(2026, 2, 28).is_some());
    }

    #[test]
    fn the_gregorian_century_rule_is_the_one_applied() {
        assert!(CivilDate::is_leap_year(2000));
        assert!(!CivilDate::is_leap_year(1900));
        assert_eq!(CivilDate::days_in_month(2024, 2), Some(29));
        assert_eq!(CivilDate::days_in_month(2026, 2), Some(28));
    }

    #[test]
    fn a_year_that_cannot_be_written_back_cannot_be_built() {
        assert_eq!(CivilDate::from_ymd(10_000, 1, 1), None);
        assert!(CivilDate::from_ymd(9999, 12, 31).is_some());
    }

    #[test]
    fn a_leap_second_is_accepted_because_the_format_accepts_it() {
        assert!(CivilTime::from_hms(23, 59, 60).is_some());
        assert_eq!(CivilTime::from_hms(24, 0, 0), None);
    }

    #[test]
    fn an_offset_of_a_whole_day_is_not_an_offset() {
        assert_eq!(UtcOffset::from_seconds(86_400), None);
        assert_eq!(UtcOffset::from_seconds(-86_400), None);
        assert_eq!(UtcOffset::UTC.seconds(), 0);
    }

    #[test]
    fn weekday_indices_round_trip_from_monday() {
        for day in Weekday::ALL {
            assert_eq!(Weekday::from_index(day.index()), Some(day));
        }
        assert_eq!(Weekday::from_index(7), None);
        assert_eq!(Weekday::Sunday.as_bytes(), b"SU");
    }

    #[test]
    fn a_clamped_outcome_keeps_the_day_that_was_asked_for() {
        let last = CivilDate::from_ymd(2026, 4, 30).unwrap();
        let outcome = MonthAddOutcome::Clamped {
            date: last,
            requested_day: 31,
        };
        assert_eq!(outcome.date(), Some(last));
        assert_eq!(MonthAddOutcome::Overflow.date(), None);
    }

    #[test]
    fn the_value_type_follows_the_form_and_not_the_zone() {
        let date = CivilDate::from_ymd(2026, 8, 15).unwrap();
        assert_eq!(DateTimeValue::Date(date).value_type(), ValueType::Date);
        assert_eq!(DateTimeValue::Date(date).time(), None);
    }
}
