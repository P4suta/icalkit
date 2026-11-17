// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use jiff::civil::Date;

use crate::Error;
use crate::internal::core::CalendarCoordinates;

pub use crate::internal::core::{CalendarMonth, CalendarScale};

impl CalendarScale {
    /// Resolve a supported Unicode calendar name or alias without case sensitivity.
    pub fn parse(name: &str) -> Result<Self, Error> {
        Self::from_name(name)
            .ok_or_else(|| Error::single("icalkit.recurrence.calendar-scale-unsupported"))
    }
}

impl CalendarMonth {
    /// Construct a regular month identifier within the RFC 7529 numeric syntax.
    pub fn regular(number: u8) -> Result<Self, Error> {
        Self::from_parts(number, false)
            .ok_or_else(|| Error::single("icalkit.recurrence.calendar-month-invalid"))
    }

    /// Construct a leap month identifier within the RFC 7529 numeric syntax.
    pub fn leap(number: u8) -> Result<Self, Error> {
        Self::from_parts(number, true)
            .ok_or_else(|| Error::single("icalkit.recurrence.calendar-month-invalid"))
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
        let source = CalendarCoordinates::new(scale, year, month, day).ok_or_else(invalid_date)?;
        Self::from_coordinates(&source)
    }

    /// Convert an iCalendar Gregorian date into the selected calendar.
    pub fn from_gregorian(scale: CalendarScale, date: Date) -> Result<Self, Error> {
        if date.year().is_negative() {
            return Err(out_of_range());
        }
        let source = CalendarCoordinates::from_gregorian(scale, date).ok_or_else(invalid_date)?;
        Self::from_coordinates(&source)
    }

    fn from_coordinates(source: &CalendarCoordinates) -> Result<Self, Error> {
        let gregorian = source
            .to_gregorian()
            .filter(|date| (0..=9999).contains(&date.year()))
            .ok_or_else(out_of_range)?;
        Ok(Self {
            scale: source.scale(),
            year: source.year(),
            month: source.month().ok_or_else(invalid_date)?,
            day: source.day(),
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
