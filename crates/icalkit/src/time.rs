// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Jiff boundary types and the single application-implementable zone port.

#[cfg(feature = "system-tz")]
use alloc::boxed::Box;
use alloc::string::String;
#[cfg(feature = "system-tz")]
use alloc::string::ToString;

pub use jiff::civil::{Date, DateTime, Time, Weekday};
pub use jiff::{SignedDuration, Timestamp};

use ical_core::{CivilDateTime, DateTimeValue, UtcOffset};

/// Whether a local wall clock is exact, in a gap, or in a fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalKind {
    /// Exactly one instant has this wall-clock spelling.
    Exact,
    /// No instant has this wall-clock spelling.
    Gap,
    /// Two instants have this wall-clock spelling.
    Fold,
}

/// Resolution of one local wall clock, including its provenance and coverage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneResolution {
    kind: LocalKind,
    earlier: Option<Timestamp>,
    later: Option<Timestamp>,
    provenance: String,
    complete: bool,
}

impl ZoneResolution {
    /// Construct an unambiguous local-time answer.
    #[must_use]
    pub fn exact(instant: Timestamp, provenance: impl Into<String>, complete: bool) -> Self {
        Self {
            kind: LocalKind::Exact,
            earlier: Some(instant),
            later: None,
            provenance: provenance.into(),
            complete,
        }
    }

    /// Construct a local time that falls in a gap.
    #[must_use]
    pub fn gap(provenance: impl Into<String>, complete: bool) -> Self {
        Self {
            kind: LocalKind::Gap,
            earlier: None,
            later: None,
            provenance: provenance.into(),
            complete,
        }
    }

    /// Construct a fold answer, or refuse instants that are not in chronological order.
    #[must_use]
    pub fn fold(
        earlier: Timestamp,
        later: Timestamp,
        provenance: impl Into<String>,
        complete: bool,
    ) -> Option<Self> {
        if earlier >= later {
            return None;
        }
        Some(Self {
            kind: LocalKind::Fold,
            earlier: Some(earlier),
            later: Some(later),
            provenance: provenance.into(),
            complete,
        })
    }

    /// The ambiguity class of the local time.
    #[must_use]
    pub const fn kind(&self) -> LocalKind {
        self.kind
    }

    /// The sole instant, or the earlier instant in a fold.
    #[must_use]
    pub const fn earlier(&self) -> Option<Timestamp> {
        self.earlier
    }

    /// The later instant in a fold.
    #[must_use]
    pub const fn later(&self) -> Option<Timestamp> {
        self.later
    }

    /// A stable description of the source that answered.
    #[must_use]
    pub fn provenance(&self) -> &str {
        &self.provenance
    }

    /// Whether the source claims complete coverage at this time.
    #[must_use]
    pub const fn has_complete_coverage(&self) -> bool {
        self.complete
    }
}

/// Offset at one instant, together with provenance and coverage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffsetResolution {
    seconds: i32,
    provenance: String,
    complete: bool,
}

impl OffsetResolution {
    /// Construct an offset answer, refusing offsets outside iCalendar's range.
    #[must_use]
    pub fn new(seconds: i32, provenance: impl Into<String>, complete: bool) -> Option<Self> {
        if seconds <= -86_400 || seconds >= 86_400 {
            return None;
        }
        Some(Self {
            seconds,
            provenance: provenance.into(),
            complete,
        })
    }

    /// Seconds east of UTC.
    #[must_use]
    pub const fn seconds(&self) -> i32 {
        self.seconds
    }

    /// A stable description of the source that answered.
    #[must_use]
    pub fn provenance(&self) -> &str {
        &self.provenance
    }

    /// Whether the source claims complete coverage at this instant.
    #[must_use]
    pub const fn has_complete_coverage(&self) -> bool {
        self.complete
    }
}

/// The only application-implementable service in the public API.
pub trait ZoneDatabase: Send + Sync {
    /// Resolve a local wall clock in `tzid`.
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution>;

    /// Find the UTC offset in force at `instant` in `tzid`.
    fn offset_at(&self, tzid: &str, instant: Timestamp) -> Option<OffsetResolution>;
}

#[cfg(feature = "system-tz")]
#[derive(Debug)]
struct SystemZoneDatabase;

#[cfg(feature = "system-tz")]
impl ZoneDatabase for SystemZoneDatabase {
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution> {
        use jiff::tz::{AmbiguousOffset, TimeZone};

        let zone = TimeZone::get(tzid).ok()?;
        let ambiguous = zone.to_ambiguous_zoned(local);
        let (kind, earlier, later) = match ambiguous.offset() {
            AmbiguousOffset::Unambiguous { offset } => {
                (LocalKind::Exact, offset.to_timestamp(local).ok(), None)
            },
            AmbiguousOffset::Gap { .. } => (LocalKind::Gap, None, None),
            AmbiguousOffset::Fold { before, after } => (
                LocalKind::Fold,
                before.to_timestamp(local).ok(),
                after.to_timestamp(local).ok(),
            ),
        };
        Some(ZoneResolution {
            kind,
            earlier,
            later,
            provenance: "jiff-system-tzdb".to_string(),
            complete: true,
        })
    }

    fn offset_at(&self, tzid: &str, instant: Timestamp) -> Option<OffsetResolution> {
        let zone = jiff::tz::TimeZone::get(tzid).ok()?;
        Some(OffsetResolution {
            seconds: zone.to_offset(instant).seconds(),
            provenance: "jiff-system-tzdb".to_string(),
            complete: true,
        })
    }
}

#[cfg(feature = "system-tz")]
pub(crate) fn default_zone_database() -> Box<dyn ZoneDatabase> {
    Box::new(SystemZoneDatabase)
}

/// An iCalendar DATE or DATE-TIME, including floating/zoned form and leap-second witness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcalDateTime {
    representation: Representation,
    leap_second: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Representation {
    Date(Date),
    Floating(DateTime),
    Utc(Timestamp),
    Zoned { local: DateTime, tzid: String },
}

impl IcalDateTime {
    /// A DATE value.
    #[must_use]
    pub const fn date(value: Date) -> Self {
        Self {
            representation: Representation::Date(value),
            leap_second: false,
        }
    }

    /// A floating DATE-TIME value.
    #[must_use]
    pub const fn floating(value: DateTime) -> Self {
        Self {
            representation: Representation::Floating(value),
            leap_second: false,
        }
    }

    /// A UTC DATE-TIME value.
    #[must_use]
    pub const fn utc(value: Timestamp) -> Self {
        Self {
            representation: Representation::Utc(value),
            leap_second: false,
        }
    }

    /// A local DATE-TIME carrying an explicit `TZID`.
    #[must_use]
    pub fn zoned(local: DateTime, tzid: impl Into<String>) -> Self {
        Self {
            representation: Representation::Zoned {
                local,
                tzid: tzid.into(),
            },
            leap_second: false,
        }
    }

    /// Mark that this value was spelled with second `:60`.
    ///
    /// Jiff follows the Unix convention and cannot carry a leap second directly. The stored
    /// value is therefore the preceding `:59` second and this witness preserves the
    /// iCalendar spelling. DATE values and values not exactly on `:59` are refused.
    #[must_use]
    pub fn with_leap_second(mut self) -> Option<Self> {
        let represents_preceding_second = match self.representation {
            Representation::Date(_) => false,
            Representation::Floating(value) => {
                value.second() == 59 && value.subsec_nanosecond() == 0
            },
            Representation::Utc(value) => {
                value.as_second().rem_euclid(60) == 59 && value.subsec_nanosecond() == 0
            },
            Representation::Zoned { local, .. } => {
                local.second() == 59 && local.subsec_nanosecond() == 0
            },
        };
        if !represents_preceding_second {
            return None;
        }
        self.leap_second = true;
        Some(self)
    }

    /// Whether the source value used second `:60`.
    #[must_use]
    pub const fn has_leap_second(&self) -> bool {
        self.leap_second
    }

    /// The DATE value, when this is a DATE.
    #[must_use]
    pub const fn as_date(&self) -> Option<Date> {
        match self.representation {
            Representation::Date(value) => Some(value),
            _ => None,
        }
    }

    /// The floating civil date-time, when this is floating.
    #[must_use]
    pub const fn as_floating(&self) -> Option<DateTime> {
        match self.representation {
            Representation::Floating(value) => Some(value),
            _ => None,
        }
    }

    /// The timestamp, when this is UTC.
    #[must_use]
    pub const fn as_utc(&self) -> Option<Timestamp> {
        match self.representation {
            Representation::Utc(value) => Some(value),
            _ => None,
        }
    }

    /// The local date-time and TZID, when this is zoned.
    #[must_use]
    pub fn as_zoned(&self) -> Option<(DateTime, &str)> {
        match &self.representation {
            Representation::Zoned { local, tzid } => Some((*local, tzid)),
            _ => None,
        }
    }
}

pub(crate) fn from_core_date_time(value: DateTimeValue<'_>) -> Option<IcalDateTime> {
    match value {
        DateTimeValue::Date(value) => {
            let year = i16::try_from(value.year()).ok()?;
            let month = i8::try_from(value.month()).ok()?;
            let day = i8::try_from(value.day()).ok()?;
            Some(IcalDateTime::date(Date::new(year, month, day).ok()?))
        },
        DateTimeValue::Local(value) => {
            with_core_leap_second(IcalDateTime::floating(jiff_date_time(value)?), value)
        },
        DateTimeValue::Utc(value) => {
            let instant = value.at_offset(UtcOffset::UTC)?;
            let timestamp = Timestamp::new(instant.unix_seconds(), 0).ok()?;
            with_core_leap_second(IcalDateTime::utc(timestamp), value)
        },
        DateTimeValue::Zoned { stamp, tzid } => {
            let tzid = core::str::from_utf8(tzid).ok()?;
            with_core_leap_second(IcalDateTime::zoned(jiff_date_time(stamp)?, tzid), stamp)
        },
    }
}

fn jiff_date_time(value: CivilDateTime) -> Option<DateTime> {
    let date = value.date();
    let time = value.time();
    DateTime::new(
        i16::try_from(date.year()).ok()?,
        i8::try_from(date.month()).ok()?,
        i8::try_from(date.day()).ok()?,
        i8::try_from(time.hour()).ok()?,
        i8::try_from(time.minute()).ok()?,
        i8::try_from(time.second().min(59)).ok()?,
        0,
    )
    .ok()
}

fn with_core_leap_second(value: IcalDateTime, source: CivilDateTime) -> Option<IcalDateTime> {
    if source.time().second() == 60 {
        value.with_leap_second()
    } else {
        Some(value)
    }
}
