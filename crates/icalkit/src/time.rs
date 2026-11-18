// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Jiff boundary types and the zone database port.

#[cfg(feature = "system-tz")]
use alloc::boxed::Box;
use alloc::string::String;
#[cfg(feature = "system-tz")]
use alloc::string::ToString;
use alloc::vec::Vec;

pub use jiff::civil::{Date, DateTime, Time, Weekday};
pub use jiff::{SignedDuration, Timestamp};

use crate::Calendar;
use crate::internal::core::{
    CivilDateTime, DateTimeValue, Diagnostic, Instant, LimitExceeded, Meter, UtcOffset,
};
use crate::internal::tz::{
    AnswerBasis, LocalResolution, OffsetAnswer, Reading, VtimezoneSet, ZoneAnswer, ZoneProvenance,
    ZoneSource, read_calendar_zones,
};

pub(crate) struct ResourceZones<'a> {
    stored: VtimezoneSet,
    external: &'a dyn ZoneSource,
}

impl<'a> ResourceZones<'a> {
    pub(crate) fn read(
        calendar: Option<&Calendar>,
        external: &'a dyn ZoneSource,
        meter: &mut Meter,
        notes: &mut Vec<Diagnostic>,
    ) -> Result<Self, LimitExceeded> {
        let stored = calendar
            .and_then(|resource| resource.document.components().next())
            .map_or_else(VtimezoneSet::new, |root| {
                read_calendar_zones(root, meter, notes)
            });
        meter.admit(|| Ok(Self { stored, external }))
    }

    fn source(&self, tzid: &str) -> &dyn ZoneSource {
        self.stored
            .table(tzid)
            .map_or(self.external, |table| table as &dyn ZoneSource)
    }
}

impl ZoneSource for ResourceZones<'_> {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        self.source(tzid).resolve(tzid, local)
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        self.source(tzid).offset_at(tzid, instant)
    }

    fn recognizes(&self, tzid: &str) -> bool {
        self.source(tzid).recognizes(tzid)
    }
}

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

/// A local-time resolution with provenance and coverage.
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

    /// Return a fold answer when `earlier < later`, or `None` otherwise.
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

    /// Whether the source claims complete coverage for the resolution.
    #[must_use]
    pub const fn has_complete_coverage(&self) -> bool {
        self.complete
    }
}

/// An offset with provenance and coverage.
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

/// The public zone database port.
pub trait ZoneDatabase: Send + Sync {
    /// Resolve a local wall clock in `tzid`.
    /// Calendar workflows fold leap seconds onto second 59 before lookup.
    /// Calendar workflows refuse fractional instants in the answer.
    fn resolve_local(&self, tzid: &str, local: DateTime) -> Option<ZoneResolution>;

    /// Find the UTC offset in force at `instant` in `tzid`.
    fn offset_at(&self, tzid: &str, instant: Timestamp) -> Option<OffsetResolution>;
}

/// Private adapter from the sole public zone port to the recurrence kernel's source.
pub(crate) struct ZoneAdapter<'a>(Option<&'a dyn ZoneDatabase>);

impl<'a> ZoneAdapter<'a> {
    #[must_use]
    pub(crate) const fn new(database: Option<&'a dyn ZoneDatabase>) -> Self {
        Self(database)
    }

    fn reading(&self, tzid: &str, timestamp: Timestamp) -> Option<(Reading, bool)> {
        let database = self.0?;
        let offset = database.offset_at(tzid, timestamp)?;
        Some((
            Reading::new(
                Instant::from_unix_seconds(timestamp.as_second()),
                UtcOffset::from_seconds(offset.seconds())?,
                false,
            ),
            offset.has_complete_coverage(),
        ))
    }
}

fn coverage_basis(complete: bool) -> AnswerBasis {
    if complete {
        AnswerBasis::Computed
    } else {
        AnswerBasis::CoverageUnasserted
    }
}

impl ZoneSource for ZoneAdapter<'_> {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        let database = self.0?;
        let local = jiff_date_time(local)?;
        let answer = database.resolve_local(tzid, local)?;
        if answer
            .earlier()
            .into_iter()
            .chain(answer.later())
            .any(|instant| instant.subsec_nanosecond() != 0)
        {
            return Some(ZoneAnswer::new(
                LocalResolution::Unreadable,
                ZoneProvenance::CallerDatabase,
                coverage_basis(answer.has_complete_coverage()),
            ));
        }
        let (resolution, offsets_complete) = match answer.kind() {
            LocalKind::Exact => {
                let (reading, complete) = self.reading(tzid, answer.earlier()?)?;
                (LocalResolution::Unique { reading }, complete)
            },
            LocalKind::Fold => {
                let (earlier, earlier_complete) = self.reading(tzid, answer.earlier()?)?;
                let (later, later_complete) = self.reading(tzid, answer.later()?)?;
                (
                    LocalResolution::Ambiguous { earlier, later },
                    earlier_complete && later_complete,
                )
            },
            // Gap answers lack the transition bounds and offsets needed by the internal representation.
            LocalKind::Gap => (LocalResolution::Undetermined, true),
        };
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::CallerDatabase,
            coverage_basis(answer.has_complete_coverage() && offsets_complete),
        ))
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        let database = self.0?;
        let timestamp = Timestamp::new(instant.unix_seconds(), 0).ok()?;
        let answer = database.offset_at(tzid, timestamp)?;
        Some(OffsetAnswer::new(
            UtcOffset::from_seconds(answer.seconds())?,
            false,
            ZoneProvenance::CallerDatabase,
            coverage_basis(answer.has_complete_coverage()),
        ))
    }

    fn recognizes(&self, tzid: &str) -> bool {
        self.0
            .and_then(|database| database.offset_at(tzid, Timestamp::UNIX_EPOCH))
            .is_some()
    }
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

/// An iCalendar `DATE` or `DATE-TIME` with clock form and leap-second evidence.
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
    /// A `DATE` value.
    #[must_use]
    pub const fn date(value: Date) -> Self {
        Self {
            representation: Representation::Date(value),
            leap_second: false,
        }
    }

    /// A floating `DATE-TIME` value.
    #[must_use]
    pub const fn floating(value: DateTime) -> Self {
        Self {
            representation: Representation::Floating(value),
            leap_second: false,
        }
    }

    /// A `DATE-TIME` value in UTC.
    #[must_use]
    pub const fn utc(value: Timestamp) -> Self {
        Self {
            representation: Representation::Utc(value),
            leap_second: false,
        }
    }

    /// A local `DATE-TIME` with an explicit `TZID`.
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

    /// Mark the original iCalendar spelling as second `:60`.
    ///
    /// Jiff uses Unix time and can't represent a leap second.
    /// The stored time uses second `:59`.
    /// The witness preserves the iCalendar spelling.
    /// Reject `DATE` values and times whose second isn't `59` or whose subsecond part isn't zero.
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

    /// The `DATE` value, when present.
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

    /// The local date-time and `TZID` of a zoned value.
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

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::{DateTime, OffsetResolution, Timestamp, ZoneAdapter, ZoneDatabase, ZoneResolution};
    use crate::internal::core::{CivilDate, CivilDateTime, CivilTime, DiagnosticCode, Instant};
    use crate::internal::tz::{AnswerBasis, LocalResolution, ZoneSource};

    struct AnswerDatabase {
        answer: ZoneResolution,
        expected_local: Option<DateTime>,
        resolve_calls: AtomicUsize,
        offset_calls: AtomicUsize,
        offset_coverage: [bool; 2],
    }

    impl AnswerDatabase {
        fn new(answer: ZoneResolution) -> Self {
            Self {
                answer,
                expected_local: None,
                resolve_calls: AtomicUsize::new(0),
                offset_calls: AtomicUsize::new(0),
                offset_coverage: [true; 2],
            }
        }
    }

    impl ZoneDatabase for AnswerDatabase {
        fn resolve_local(&self, _tzid: &str, local: DateTime) -> Option<ZoneResolution> {
            self.resolve_calls.fetch_add(1, Ordering::Relaxed);
            if let Some(expected) = self.expected_local {
                assert_eq!(local, expected);
            }
            Some(self.answer.clone())
        }

        fn offset_at(&self, _tzid: &str, instant: Timestamp) -> Option<OffsetResolution> {
            self.offset_calls.fetch_add(1, Ordering::Relaxed);
            let later = self.answer.later() == Some(instant);
            let offset = if later { -3600 } else { 0 };
            OffsetResolution::new(
                offset,
                "precision-fixture",
                self.offset_coverage[usize::from(later)],
            )
        }
    }

    fn local() -> CivilDateTime {
        CivilDateTime::new(
            CivilDate::from_ymd(2024, 1, 2).unwrap(),
            CivilTime::from_hms(9, 0, 0).unwrap(),
        )
    }

    fn timestamp(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn fractional_exact_answers_cannot_become_internal_readings() {
        for text in [
            "2024-01-02T09:00:00.000000001Z",
            "2024-01-02T09:00:00.999999999Z",
            "1969-12-31T23:59:59.000000001Z",
            "1969-12-31T23:59:59.999999999Z",
        ] {
            let database = AnswerDatabase::new(ZoneResolution::exact(
                timestamp(text),
                "precision-fixture",
                true,
            ));
            let answer = ZoneAdapter::new(Some(&database)).resolve("Test/Precision", local());
            assert_eq!(
                answer.unwrap().resolution,
                LocalResolution::Unreadable,
                "{text}"
            );
        }
    }

    #[test]
    fn fractional_fold_candidates_cannot_become_internal_readings() {
        for (earlier, later) in [
            ("2024-01-02T09:00:00.5Z", "2024-01-02T10:00:00Z"),
            ("2024-01-02T09:00:00Z", "2024-01-02T10:00:00.5Z"),
            ("2024-01-02T09:00:00.25Z", "2024-01-02T09:00:00.75Z"),
        ] {
            let database = AnswerDatabase::new(
                ZoneResolution::fold(
                    timestamp(earlier),
                    timestamp(later),
                    "precision-fixture",
                    true,
                )
                .unwrap(),
            );
            let answer = ZoneAdapter::new(Some(&database)).resolve("Test/Precision", local());
            assert_eq!(
                answer.unwrap().resolution,
                LocalResolution::Unreadable,
                "{earlier} / {later}"
            );
        }
    }

    #[test]
    fn unsupported_precision_is_refused_before_offset_lookup() {
        let database = AnswerDatabase::new(ZoneResolution::exact(
            timestamp("2024-01-02T09:00:00.5Z"),
            "precision-fixture",
            true,
        ));
        let _ = ZoneAdapter::new(Some(&database)).resolve("Test/Precision", local());
        assert_eq!(database.offset_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn whole_second_answers_preserve_exact_and_fold_instants() {
        let earlier = timestamp("2024-01-02T09:00:00Z");
        let later = timestamp("2024-01-02T10:00:00Z");
        let exact = AnswerDatabase::new(ZoneResolution::exact(earlier, "precision-fixture", true));
        let answer = ZoneAdapter::new(Some(&exact))
            .resolve("Test/Precision", local())
            .unwrap();
        assert!(
            matches!(answer.resolution, LocalResolution::Unique { reading } if reading.instant.unix_seconds() == earlier.as_second())
        );
        let fold = AnswerDatabase::new(
            ZoneResolution::fold(earlier, later, "precision-fixture", true).unwrap(),
        );
        let answer = ZoneAdapter::new(Some(&fold))
            .resolve("Test/Precision", local())
            .unwrap();
        assert!(
            matches!(answer.resolution, LocalResolution::Ambiguous { earlier: first, later: second }
            if first.instant.unix_seconds() == earlier.as_second()
                && second.instant.unix_seconds() == later.as_second()
                && first.offset != second.offset)
        );
    }

    fn assert_coverage(basis: AnswerBasis, complete: bool) {
        assert_eq!(basis.is_computed(), complete);
        assert_eq!(basis.nearest_known(), None);
        assert!(!basis.is_before_known_transitions());
        assert!(!basis.is_beyond_known_transitions());
        assert_eq!(
            basis.diagnostic_code().map(DiagnosticCode::as_str),
            (!complete).then_some("time-zone-coverage-unasserted")
        );
    }

    #[test]
    fn exact_coverage_includes_local_and_offset_claims() {
        for local_complete in [false, true] {
            for offset_complete in [false, true] {
                let mut database = AnswerDatabase::new(ZoneResolution::exact(
                    timestamp("2024-01-02T09:00:00Z"),
                    "coverage-fixture",
                    local_complete,
                ));
                database.offset_coverage = [offset_complete; 2];
                let answer = ZoneAdapter::new(Some(&database))
                    .resolve("Test/Coverage", local())
                    .unwrap();
                assert_coverage(answer.basis, local_complete && offset_complete);
                assert!(matches!(answer.resolution, LocalResolution::Unique { .. }));
            }
        }
    }

    #[test]
    fn fold_coverage_includes_both_offset_claims() {
        for local_complete in [false, true] {
            for earlier_complete in [false, true] {
                for later_complete in [false, true] {
                    let mut database = AnswerDatabase::new(
                        ZoneResolution::fold(
                            timestamp("2024-01-02T09:00:00Z"),
                            timestamp("2024-01-02T10:00:00Z"),
                            "coverage-fixture",
                            local_complete,
                        )
                        .unwrap(),
                    );
                    database.offset_coverage = [earlier_complete, later_complete];
                    let answer = ZoneAdapter::new(Some(&database))
                        .resolve("Test/Coverage", local())
                        .unwrap();
                    assert_coverage(
                        answer.basis,
                        local_complete && earlier_complete && later_complete,
                    );
                    assert!(
                        matches!(answer.resolution, LocalResolution::Ambiguous { earlier, later } if earlier.instant < later.instant)
                    );
                }
            }
        }
    }

    #[test]
    fn direct_offset_coverage_keeps_its_own_claim() {
        for complete in [false, true] {
            let mut database = AnswerDatabase::new(ZoneResolution::gap("coverage-fixture", false));
            database.offset_coverage = [complete; 2];
            let answer = ZoneAdapter::new(Some(&database))
                .offset_at("Test/Coverage", Instant::EPOCH)
                .unwrap();
            assert_coverage(answer.basis, complete);
        }
    }

    #[test]
    fn gap_coverage_does_not_invent_transition_bounds() {
        for complete in [false, true] {
            let database = AnswerDatabase::new(ZoneResolution::gap("coverage-fixture", complete));
            let answer = ZoneAdapter::new(Some(&database))
                .resolve("Test/Coverage", local())
                .unwrap();
            assert_coverage(answer.basis, complete);
            assert_eq!(answer.resolution, LocalResolution::Undetermined);
        }
    }

    #[test]
    fn zone_lookup_folds_a_leap_second_without_advancing_the_clock() {
        let native = CivilDateTime::new(
            CivilDate::from_ymd(1997, 6, 30).unwrap(),
            CivilTime::from_hms(23, 59, 60).unwrap(),
        );
        let folded = DateTime::constant(1997, 6, 30, 23, 59, 59, 0);
        let expected = timestamp("1997-06-30T23:59:59Z");
        for complete in [false, true] {
            let mut database = AnswerDatabase::new(ZoneResolution::exact(
                expected,
                "leap-second-fixture",
                complete,
            ));
            database.expected_local = Some(folded);
            database.offset_coverage = [complete; 2];
            let answer = ZoneAdapter::new(Some(&database))
                .resolve("Test/Leap", native)
                .expect("the zone provider must receive the folded local clock");
            assert_eq!(database.resolve_calls.load(Ordering::Relaxed), 1);
            assert_eq!(
                answer.resolution.earliest(),
                Some(Instant::from_unix_seconds(expected.as_second()))
            );
            assert_coverage(answer.basis, complete);
        }
    }

    #[test]
    fn zone_lookup_keeps_every_ordinary_second() {
        for second in 0_u8..60 {
            let native = CivilDateTime::new(
                CivilDate::from_ymd(1997, 6, 30).unwrap(),
                CivilTime::from_hms(23, 59, second).unwrap(),
            );
            let local =
                DateTime::new(1997, 6, 30, 23, 59, i8::try_from(second).unwrap(), 0).unwrap();
            let expected: Timestamp = alloc::format!("{local}Z").parse().unwrap();
            let mut database = AnswerDatabase::new(ZoneResolution::exact(
                expected,
                "ordinary-second-fixture",
                true,
            ));
            database.expected_local = Some(local);
            let answer = ZoneAdapter::new(Some(&database))
                .resolve("Test/Leap", native)
                .unwrap();
            assert_eq!(database.resolve_calls.load(Ordering::Relaxed), 1);
            assert_eq!(
                answer.resolution.earliest(),
                Some(Instant::from_unix_seconds(expected.as_second()))
            );
        }
    }
}
