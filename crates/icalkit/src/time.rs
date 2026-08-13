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
        }
    }

    /// A floating DATE-TIME value.
    #[must_use]
    pub const fn floating(value: DateTime) -> Self {
        Self {
            representation: Representation::Floating(value),
        }
    }

    /// A UTC DATE-TIME value.
    #[must_use]
    pub const fn utc(value: Timestamp) -> Self {
        Self {
            representation: Representation::Utc(value),
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
        }
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
