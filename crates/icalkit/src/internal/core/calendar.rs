// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::num::NonZeroU8;

use icu_calendar::options::{DateAddOptions, DateFromFieldsOptions, Overflow};
use icu_calendar::types::{DateDuration, DateFields, Month};
use icu_calendar::{AnyCalendar, AnyCalendarKind, Date as IcuDate, Iso};
use jiff::civil::Date;

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
    pub(crate) const fn ranges(self) -> CalendarRanges {
        let (regular_months, months, month_days, year_days) = match self {
            Self::Chinese | Self::Dangi | Self::Hebrew => (12, 13, 30, 385),
            Self::Coptic | Self::Ethiopic | Self::EthiopicAmeteAlem => (13, 13, 30, 366),
            Self::IslamicCivil | Self::IslamicTabular | Self::IslamicUmmAlQura => (12, 12, 30, 355),
            Self::Gregorian
            | Self::Buddhist
            | Self::Indian
            | Self::Iso8601
            | Self::Japanese
            | Self::Persian
            | Self::Roc => (12, 12, 31, 366),
        };
        CalendarRanges {
            regular_months,
            months,
            month_days,
            year_days,
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct CalendarRanges {
    pub(crate) regular_months: u8,
    pub(crate) months: u8,
    pub(crate) month_days: u8,
    pub(crate) year_days: u16,
}

/// A regular month number or the leap month immediately following that month.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CalendarMonth {
    number: NonZeroU8,
    leap: bool,
}

impl CalendarMonth {
    pub(crate) fn from_parts(number: u8, leap: bool) -> Option<Self> {
        let number = NonZeroU8::new(number).filter(|value| value.get() <= 99)?;
        Some(Self { number, leap })
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

#[derive(Clone, Debug)]
pub(crate) struct CalendarCoordinates {
    scale: CalendarScale,
    date: IcuDate<AnyCalendar>,
}

impl CalendarCoordinates {
    pub(crate) fn days_from_epoch(&self) -> Option<i64> {
        let epoch = IcuDate::try_new(1970.into(), Month::new(1), 1, Iso).ok()?;
        self.date
            .to_rata_die()
            .to_i64_date()
            .checked_sub(epoch.to_rata_die().to_i64_date())
    }

    pub(crate) fn days_in_month(&self) -> u8 {
        self.date.days_in_month()
    }

    pub(crate) fn days_in_year(&self) -> u16 {
        self.date.days_in_year()
    }

    pub(crate) fn months_in_year(&self) -> u8 {
        self.date.months_in_year()
    }

    pub(crate) fn new(
        scale: CalendarScale,
        year: i32,
        month: CalendarMonth,
        day: u8,
    ) -> Option<Self> {
        let mut fields = DateFields::default();
        fields.extended_year = Some(year);
        fields.month = Some(if month.is_leap() {
            Month::leap(month.number())
        } else {
            Month::new(month.number())
        });
        fields.day = Some(day);
        let mut options = DateFromFieldsOptions::default();
        options.overflow = Some(Overflow::Reject);
        let date =
            IcuDate::try_from_fields(fields, options, AnyCalendar::new(scale.kind())).ok()?;
        let result = Self { scale, date };
        (result.year() == year && result.month() == Some(month) && result.day() == day)
            .then_some(result)
    }

    pub(crate) fn from_gregorian(scale: CalendarScale, date: Date) -> Option<Self> {
        let month = u8::try_from(date.month()).ok()?;
        let day = u8::try_from(date.day()).ok()?;
        let source =
            IcuDate::try_new(i32::from(date.year()).into(), Month::new(month), day, Iso).ok()?;
        Some(Self {
            scale,
            date: source.to_calendar(AnyCalendar::new(scale.kind())),
        })
    }

    pub(crate) const fn scale(&self) -> CalendarScale {
        self.scale
    }

    pub(crate) fn year(&self) -> i32 {
        self.date.year().extended_year()
    }

    pub(crate) fn month(&self) -> Option<CalendarMonth> {
        let month = self.date.month().to_input();
        CalendarMonth::from_parts(month.number(), month.is_leap())
    }

    pub(crate) fn day(&self) -> u8 {
        self.date.day_of_month().0
    }

    pub(crate) fn to_gregorian(&self) -> Option<Date> {
        let iso = self.date.to_calendar(Iso);
        let year = i16::try_from(iso.year().extended_year()).ok()?;
        let month = i8::try_from(iso.month().ordinal).ok()?;
        let day = i8::try_from(iso.day_of_month().0).ok()?;
        Date::new(year, month, day).ok()
    }

    pub(crate) fn first_of_month(&self) -> Option<Self> {
        Self::new(self.scale, self.year(), self.month()?, 1)
    }

    pub(crate) fn first_of_year(&self) -> Option<Self> {
        Self::new(
            self.scale,
            self.year(),
            CalendarMonth::from_parts(1, false)?,
            1,
        )
    }

    pub(crate) fn checked_add_months(&self, months: i32) -> Option<Self> {
        self.checked_add(DateDuration::for_months(months))
    }

    pub(crate) fn checked_add_years(&self, years: i32) -> Option<Self> {
        self.checked_add(DateDuration::for_years(years))
    }

    fn checked_add(&self, duration: DateDuration) -> Option<Self> {
        let mut options = DateAddOptions::default();
        options.overflow = Some(Overflow::Reject);
        let date = self
            .date
            .clone()
            .try_added_with_options(duration, options)
            .ok()?;
        Some(Self {
            scale: self.scale,
            date,
        })
    }
}
