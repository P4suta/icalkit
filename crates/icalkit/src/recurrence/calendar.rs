// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::num::NonZeroU8;

use icu_calendar::options::{DateFromFieldsOptions, Overflow};
use icu_calendar::types::{DateFields, Month};
use icu_calendar::{AnyCalendar, AnyCalendarKind, Date as IcuDate, Iso};
use jiff::civil::Date;

use crate::Error;

/// A registered recurrence calendar with an explicit conversion algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalendarScale {
    /// The Gregorian calendar, including its `gregory` spelling.
    Gregorian,
    /// The Buddhist calendar.
    Buddhist,
    /// The Chinese calendar.
    Chinese,
    /// The Coptic calendar.
    Coptic,
    /// The Korean calendar registered as `dangi`.
    Dangi,
    /// The Ethiopic calendar with the Amete Mihret epoch.
    Ethiopic,
    /// The Ethiopic calendar with the Amete Alem epoch.
    EthiopicAmeteAlem,
    /// The Hebrew calendar.
    Hebrew,
    /// The Indian calendar.
    Indian,
    /// The Islamic tabular calendar with the civil epoch.
    IslamicCivil,
    /// The Islamic tabular calendar with the astronomical epoch.
    IslamicTabular,
    /// The Islamic Umm al-Qura calendar.
    IslamicUmmAlQura,
    /// The Gregorian calendar registered as `iso8601`.
    Iso8601,
    /// The Japanese calendar.
    Japanese,
    /// The Persian calendar.
    Persian,
    /// The Republic of China calendar.
    Roc,
}

impl CalendarScale {
    /// Resolve a supported Unicode calendar name or alias without case sensitivity.
    pub fn parse(name: &str) -> Result<Self, Error> {
        let preferred = if name.eq_ignore_ascii_case("GREGORY") {
            "GREGORIAN"
        } else if name.eq_ignore_ascii_case("ETHIOPIC-AMETE-ALEM") {
            "ETHIOAA"
        } else if name.eq_ignore_ascii_case("ISLAMICC") {
            "ISLAMIC-CIVIL"
        } else {
            name
        };
        [
            Self::Gregorian,
            Self::Buddhist,
            Self::Chinese,
            Self::Coptic,
            Self::Dangi,
            Self::Ethiopic,
            Self::EthiopicAmeteAlem,
            Self::Hebrew,
            Self::Indian,
            Self::IslamicCivil,
            Self::IslamicTabular,
            Self::IslamicUmmAlQura,
            Self::Iso8601,
            Self::Japanese,
            Self::Persian,
            Self::Roc,
        ]
        .into_iter()
        .find(|scale| scale.name().eq_ignore_ascii_case(preferred))
        .ok_or_else(|| Error::single("icalkit.recurrence.calendar-scale-unsupported"))
    }

    /// The uppercase recurrence calendar name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gregorian => "GREGORIAN",
            Self::Buddhist => "BUDDHIST",
            Self::Chinese => "CHINESE",
            Self::Coptic => "COPTIC",
            Self::Dangi => "DANGI",
            Self::Ethiopic => "ETHIOPIC",
            Self::EthiopicAmeteAlem => "ETHIOAA",
            Self::Hebrew => "HEBREW",
            Self::Indian => "INDIAN",
            Self::IslamicCivil => "ISLAMIC-CIVIL",
            Self::IslamicTabular => "ISLAMIC-TBLA",
            Self::IslamicUmmAlQura => "ISLAMIC-UMALQURA",
            Self::Iso8601 => "ISO8601",
            Self::Japanese => "JAPANESE",
            Self::Persian => "PERSIAN",
            Self::Roc => "ROC",
        }
    }

    const fn kind(self) -> AnyCalendarKind {
        match self {
            Self::Gregorian => AnyCalendarKind::Gregorian,
            Self::Buddhist => AnyCalendarKind::Buddhist,
            Self::Chinese => AnyCalendarKind::Chinese,
            Self::Coptic => AnyCalendarKind::Coptic,
            Self::Dangi => AnyCalendarKind::Dangi,
            Self::Ethiopic => AnyCalendarKind::Ethiopian,
            Self::EthiopicAmeteAlem => AnyCalendarKind::EthiopianAmeteAlem,
            Self::Hebrew => AnyCalendarKind::Hebrew,
            Self::Indian => AnyCalendarKind::Indian,
            Self::IslamicCivil => AnyCalendarKind::HijriTabularTypeIIFriday,
            Self::IslamicTabular => AnyCalendarKind::HijriTabularTypeIIThursday,
            Self::IslamicUmmAlQura => AnyCalendarKind::HijriUmmAlQura,
            Self::Iso8601 => AnyCalendarKind::Iso,
            Self::Japanese => AnyCalendarKind::Japanese,
            Self::Persian => AnyCalendarKind::Persian,
            Self::Roc => AnyCalendarKind::Roc,
        }
    }
}

/// A regular month number or the leap month immediately following that month.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CalendarMonth {
    number: NonZeroU8,
    leap: bool,
}

impl CalendarMonth {
    /// Construct a regular month identifier within the RFC 7529 numeric syntax.
    pub fn regular(number: u8) -> Result<Self, Error> {
        Self::checked(number, false)
    }

    /// Construct a leap month identifier within the RFC 7529 numeric syntax.
    pub fn leap(number: u8) -> Result<Self, Error> {
        Self::checked(number, true)
    }

    fn checked(number: u8, leap: bool) -> Result<Self, Error> {
        let number = NonZeroU8::new(number)
            .filter(|value| value.get() <= 99)
            .ok_or_else(|| Error::single("icalkit.recurrence.calendar-month-invalid"))?;
        Ok(Self { number, leap })
    }

    /// The regular month number, independent of its ordinal position in a year.
    #[must_use]
    pub const fn number(self) -> u8 {
        self.number.get()
    }

    /// Whether this identifier names a leap month.
    #[must_use]
    pub const fn is_leap(self) -> bool {
        self.leap
    }
}

/// Validated calendar coordinates and their Gregorian date.
/// Years use the selected calendar's ICU4X extended year numbering.
///
/// ```
/// use icalkit::recurrence::{CalendarDate, CalendarMonth, CalendarScale};
///
/// let date = CalendarDate::new(
///     CalendarScale::Ethiopic,
///     2005,
///     CalendarMonth::regular(13)?,
///     1,
/// )?;
/// assert_eq!(date.to_gregorian(), jiff::civil::date(2013, 9, 6));
/// # Ok::<(), icalkit::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CalendarDate {
    scale: CalendarScale,
    year: i32,
    month: CalendarMonth,
    day: u8,
    gregorian: Date,
}

impl CalendarDate {
    /// Validate calendar coordinates without substituting another month or day.
    pub fn new(
        scale: CalendarScale,
        year: i32,
        month: CalendarMonth,
        day: u8,
    ) -> Result<Self, Error> {
        let source_month = if month.is_leap() {
            Month::leap(month.number())
        } else {
            Month::new(month.number())
        };
        let mut fields = DateFields::default();
        fields.extended_year = Some(year);
        fields.month = Some(source_month);
        fields.day = Some(day);
        let mut options = DateFromFieldsOptions::default();
        options.overflow = Some(Overflow::Reject);
        let date = IcuDate::try_from_fields(fields, options, AnyCalendar::new(scale.kind()))
            .map_err(|_| invalid_date())?;
        let result = Self::from_backend(scale, &date)?;
        if result.year != year || result.month != month || result.day != day {
            return Err(invalid_date());
        }
        Ok(result)
    }

    /// Convert an iCalendar Gregorian date into the selected calendar.
    pub fn from_gregorian(scale: CalendarScale, date: Date) -> Result<Self, Error> {
        if date.year().is_negative() {
            return Err(out_of_range());
        }
        let month = u8::try_from(date.month()).map_err(|_| invalid_date())?;
        let day = u8::try_from(date.day()).map_err(|_| invalid_date())?;
        let source = IcuDate::try_new(i32::from(date.year()).into(), Month::new(month), day, Iso)
            .map_err(|_| invalid_date())?;
        let converted = source.to_calendar(AnyCalendar::new(scale.kind()));
        Self::from_backend(scale, &converted)
    }

    fn from_backend(scale: CalendarScale, date: &IcuDate<AnyCalendar>) -> Result<Self, Error> {
        let iso = date.to_calendar(Iso);
        let year = i16::try_from(iso.year().extended_year()).map_err(|_| out_of_range())?;
        if !(0..=9999).contains(&year) {
            return Err(out_of_range());
        }
        let month = i8::try_from(iso.month().ordinal).map_err(|_| invalid_date())?;
        let day = i8::try_from(iso.day_of_month().0).map_err(|_| invalid_date())?;
        let gregorian = Date::new(year, month, day).map_err(|_| out_of_range())?;
        let month = date.month().to_input();
        Ok(Self {
            scale,
            year: date.year().extended_year(),
            month: CalendarMonth::checked(month.number(), month.is_leap())?,
            day: date.day_of_month().0,
            gregorian,
        })
    }

    /// The selected calendar.
    #[must_use]
    pub const fn scale(self) -> CalendarScale {
        self.scale
    }

    /// The calendar's extended year.
    #[must_use]
    pub const fn year(self) -> i32 {
        self.year
    }

    /// The regular or leap month identifier.
    #[must_use]
    pub const fn month(self) -> CalendarMonth {
        self.month
    }

    /// The validated day of the month.
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }

    /// The equivalent Gregorian date within iCalendar's four-digit year range.
    #[must_use]
    pub const fn to_gregorian(self) -> Date {
        self.gregorian
    }
}

fn invalid_date() -> Error {
    Error::single("icalkit.recurrence.calendar-date-invalid")
}

fn out_of_range() -> Error {
    Error::single("icalkit.recurrence.calendar-date-out-of-range")
}
