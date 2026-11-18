// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resolve `EXDATE` values on a recurrence series' nominal timeline.
//!
//! RFC 5545 section 3.8.5.1 requires `EXDATE` and `DTSTART` to agree on `DATE` or `DATE-TIME`.
//! The reader reports `exdate-value-type-mismatch` whenever they differ.
//! The exclusion policy chooses whether a date excludes a single instant or a whole local day.
//!
//! [`ResolvedExclusions::instants`] supplies distinct keys in ascending order for the recurrence merge.
//! [`ResolvedExclusions::spans`] supplies half-open intervals for whole-day exclusions.
//! The caller checks each occurrence key with [`ResolvedExclusions::excludes`].
//!
//! A whole-day span begins at the first local midnight and ends at the next day's first midnight.
//! At a fold, the boundary uses the earlier reading.
//! At a gap, the boundary uses the transition that ends the gap.
//! Consecutive spans share a boundary even when they differ in length.
//! An unknown zone uses the plain civil midnight.
//! An unreadable boundary records the nominal start in [`ResolvedExclusions::unplaced`].
//!
//! A UTC exception requires the zone's offset to place it on the nominal timeline.
//! An unknown series zone leaves the UTC instant unplaced and produces `exdate-zone-unknown`.
//! The caller's exclusion-entry limit bounds the list.
//! The meter tracks diagnostic refusals.

use alloc::vec::Vec;

use crate::internal::core::{
    CivilDate, CivilDateTime, CivilTime, DateTimeValue, Diagnostic, DiagnosticCode, DiagnosticSink,
    Instant, Meter, Severity, UtcOffset, ValueType, report_diagnostic,
};

use crate::internal::tz::answer::{LocalResolution, ZoneSource};
use crate::internal::tz::seam::{ExclusionReading, LocalInterval, nominal};
use crate::internal::tz::series::ZonedSeries;

/// The exclusion instants, whole-day spans, and unresolved keys for one series.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedExclusions {
    /// Distinct exclusion instants in ascending order.
    instants: Vec<Instant>,
    /// The nominal spans excluded whole, ascending by start and non-overlapping.
    spans: Vec<LocalInterval>,
    /// The real instants no zone could place on this series' timeline, ascending.
    unplaced: Vec<Instant>,
}

impl ResolvedExclusions {
    /// Exclusions that exclude nothing.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            instants: Vec::new(),
            spans: Vec::new(),
            unplaced: Vec::new(),
        }
    }

    /// Resolve `excluded` for `series` using the `DTSTART` value type in `dtstart_kind`.
    ///
    /// Each mismatched type produces `exdate-value-type-mismatch`.
    /// The series policy determines how a date excludes occurrences from a date-time series.
    /// The result exposes distinct exclusion keys in ascending order.
    #[must_use]
    pub fn read<S, D>(
        series: &ZonedSeries<'_, S>,
        dtstart_kind: ValueType,
        excluded: &[DateTimeValue<'_>],
        meter: &mut Meter,
        sink: &mut D,
    ) -> Self
    where
        S: ZoneSource + ?Sized,
        D: DiagnosticSink + ?Sized,
    {
        let mut resolved = Self::empty();
        let dtstart_dated = matches!(dtstart_kind, ValueType::Date);
        for value in excluded {
            resolved.admit(series, dtstart_dated, *value, meter, sink);
        }
        // Sort once after reading the entries in source order.
        resolved.instants.sort_unstable();
        resolved.instants.dedup();
        resolved.spans.sort_unstable();
        resolved.spans.dedup();
        resolved.unplaced.sort_unstable();
        resolved.unplaced.dedup();
        resolved
    }

    /// Distinct exclusion instants in ascending order for the recurrence merge.
    #[must_use]
    pub fn instants(&self) -> &[Instant] {
        &self.instants
    }

    /// The spans excluded whole, ascending by start.
    #[must_use]
    pub fn spans(&self) -> &[LocalInterval] {
        &self.spans
    }

    /// Whether a whole-day span contains `key`.
    /// The recurrence merge handles individual instants.
    #[must_use]
    pub fn excludes(&self, key: Instant) -> bool {
        self.spans.iter().any(|span| span.contains(key))
    }

    /// The unresolved exclusion keys in ascending order.
    ///
    /// A UTC exclusion retains its instant when the series zone is unknown.
    /// A whole-day exclusion retains its nominal midnight when either boundary is unreadable.
    #[must_use]
    pub fn unplaced(&self) -> &[Instant] {
        &self.unplaced
    }

    /// Whether the result has no instants, spans, or unresolved keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instants.is_empty() && self.spans.is_empty() && self.unplaced.is_empty()
    }

    /// Read one `EXDATE` value and report a mismatch with the `DTSTART` value type.
    fn admit<S, D>(
        &mut self,
        series: &ZonedSeries<'_, S>,
        dtstart_dated: bool,
        value: DateTimeValue<'_>,
        meter: &mut Meter,
        sink: &mut D,
    ) where
        S: ZoneSource + ?Sized,
        D: DiagnosticSink + ?Sized,
    {
        // A UTC exclusion still identifies a real instant when the zone can't supply its nominal key.
        let Some(point) = project(series, value) else {
            if let DateTimeValue::Utc(stamp) = value
                && let Some(instant) = stamp.at_offset(UtcOffset::UTC)
            {
                self.keep_unplaced(instant, DiagnosticCode::ExdateZoneUnknown, meter, sink);
            }
            return;
        };
        if matches!(value, DateTimeValue::Date(_)) == dtstart_dated {
            self.instants.push(point);
            return;
        }
        report_diagnostic(
            sink,
            meter,
            Diagnostic::at_instant(
                DiagnosticCode::ExdateValueTypeMismatch,
                Severity::Violation,
                point,
            ),
        );
        match series.policy().exclusions() {
            ExclusionReading::Instantaneous => self.instants.push(point),
            ExclusionReading::WholeDay => match whole_day(series, value.date()) {
                Ok(Some(span)) => self.spans.push(span),
                Ok(None) => self.instants.push(point),
                Err(code) => self.keep_unplaced(point, code, meter, sink),
            },
        }
    }

    fn keep_unplaced<D>(
        &mut self,
        named: Instant,
        code: DiagnosticCode,
        meter: &mut Meter,
        sink: &mut D,
    ) where
        D: DiagnosticSink + ?Sized,
    {
        self.unplaced.push(named);
        report_diagnostic(
            sink,
            meter,
            Diagnostic::at_instant(code, Severity::Violation, named),
        );
    }
}

/// Convert an exception to the series' nominal clock.
/// Dates and floating times already use local fields.
/// UTC values require the zone offset at the named instant.
fn project<S>(series: &ZonedSeries<'_, S>, value: DateTimeValue<'_>) -> Option<Instant>
where
    S: ZoneSource + ?Sized,
{
    match value {
        DateTimeValue::Date(date) => nominal(CivilDateTime::new(date, CivilTime::MIDNIGHT)),
        DateTimeValue::Local(stamp) => nominal(stamp),
        DateTimeValue::Utc(stamp) => series.to_nominal(stamp.at_offset(UtcOffset::UTC)?),
        DateTimeValue::Zoned { stamp, tzid } => project_zoned(series, stamp, tzid),
    }
}

/// Read a zoned exception on the series clock.
/// Matching identifiers need no conversion.
/// For another zone, place the instant there and project it onto the series clock.
/// Use the written wall clock if that conversion fails.
fn project_zoned<S>(
    series: &ZonedSeries<'_, S>,
    stamp: CivilDateTime,
    tzid: &[u8],
) -> Option<Instant>
where
    S: ZoneSource + ?Sized,
{
    let foreign = core::str::from_utf8(tzid)
        .ok()
        .filter(|name| *name != series.tzid());
    let Some(name) = foreign else {
        return nominal(stamp);
    };
    let policy = series.policy();
    series
        .source()
        .resolve(name, stamp)
        .and_then(|answer| answer.resolution.pick(policy.gaps(), policy.folds()))
        .and_then(|real| series.to_nominal(real))
        .or_else(|| nominal(stamp))
}

/// The nominal span the named day covers in the series' zone.
///
/// A skipped date or the final civil date retains the instantaneous exclusion.
/// An unreadable boundary leaves the day unresolved.
fn whole_day<S>(
    series: &ZonedSeries<'_, S>,
    date: CivilDate,
) -> Result<Option<LocalInterval>, DiagnosticCode>
where
    S: ZoneSource + ?Sized,
{
    let Some(tomorrow) = date.checked_add_days(1) else {
        return Ok(None);
    };
    let start = day_boundary(series, date)?;
    let end = day_boundary(series, tomorrow)?;
    Ok(LocalInterval::new(start, end))
}

/// Find the nominal boundary for `date`.
/// Take the earlier reading of a fold or the end of a gap.
/// An unknown zone uses plain civil midnight, and an unreadable boundary returns an error.
fn day_boundary<S>(series: &ZonedSeries<'_, S>, date: CivilDate) -> Result<Instant, DiagnosticCode>
where
    S: ZoneSource + ?Sized,
{
    let midnight = CivilDateTime::new(date, CivilTime::MIDNIGHT);
    let plain = nominal(midnight).ok_or(DiagnosticCode::ExdateBoundaryUnreadable);
    let Some(answer) = series.source().resolve(series.tzid(), midnight) else {
        return plain;
    };
    let real = match answer.resolution {
        LocalResolution::Unique { reading } => reading.instant,
        LocalResolution::Ambiguous { earlier, .. } => earlier.instant,
        LocalResolution::Nonexistent { gap_end, .. } => gap_end,
        // A source with no transition data leaves the plain civil midnight.
        LocalResolution::Undetermined => return plain,
        LocalResolution::Unreadable => return Err(DiagnosticCode::ExdateBoundaryUnreadable),
    };
    series
        .to_nominal(real)
        .ok_or(DiagnosticCode::ExdateBoundaryUnreadable)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, DateTimeValue, Diagnostic, DiagnosticCode, Instant,
        Limits, Meter, UtcOffset, ValueType,
    };

    use super::ResolvedExclusions;
    use crate::internal::tz::answer::{
        AnswerBasis, FoldPolicy, LocalResolution, OffsetAnswer, Reading, ZoneAnswer,
        ZoneProvenance, ZoneSource,
    };
    use crate::internal::tz::seam::{ExclusionReading, LocalInterval, ResolutionPolicy, nominal};
    use crate::internal::tz::series::ZonedSeries;

    /// One transition and the offset and classification that follow it.
    #[derive(Clone, Copy, Debug)]
    struct Shift {
        /// The instant the zone moved, in seconds from the Unix epoch.
        at: i64,
        /// Seconds east of UTC from then on.
        east: i32,
        /// Whether the observance from then on is the zone's daylight one.
        daylight: bool,
    }

    /// Independent transition data for exclusion tests.
    #[derive(Clone, Copy, Debug)]
    struct Zone {
        /// The identifier this zone answers to, compared by exact bytes.
        tzid: &'static str,
        /// Seconds east of UTC before the first transition.
        east: i32,
        /// Transitions in chronological order.
        shifts: &'static [Shift],
        /// The last date the table covers, `None` when its rules run on.
        known_through: Option<(u16, u8, u8)>,
    }

    /// The offset and observance in force, with no claim about how far the table reached.
    #[derive(Clone, Copy, Debug)]
    struct State {
        /// Seconds east of UTC.
        east: i32,
        /// Whether the observance is the zone's daylight one.
        daylight: bool,
    }

    impl Zone {
        /// What the zone was running at `at`.
        fn state_at(self, at: Instant) -> State {
            let mut state = State {
                east: self.east,
                daylight: false,
            };
            for shift in self.shifts {
                if at.unix_seconds() >= shift.at {
                    state = State {
                        east: shift.east,
                        daylight: shift.daylight,
                    };
                }
            }
            state
        }

        /// Whether `at` is past everything this table actually knows, and how far it knew.
        fn basis_at(self, at: Instant) -> AnswerBasis {
            let last = self.shifts.last().map_or(i64::MIN, |shift| shift.at);
            match self.known_through {
                Some((year, month, day)) if at.unix_seconds() >= last => {
                    let end = CivilDate::from_ymd(year, month, day).unwrap();
                    AnswerBasis::BeyondKnownTransitions(end)
                },
                _ => AnswerBasis::Computed,
            }
        }

        /// Every reading of `local` that the zone agrees with, ascending.
        ///
        /// The standard test: a candidate offset produces an instant, and the instant is a real reading exactly when the zone was running that offset there.
        /// Two survive a fold, none survive a gap, one survives every other hour of the year.
        fn readings_of(self, local: CivilDateTime) -> Vec<Reading> {
            let mut found: Vec<Reading> = Vec::new();
            let candidates =
                core::iter::once(self.east).chain(self.shifts.iter().map(|shift| shift.east));
            for east in candidates {
                let offset = UtcOffset::from_seconds(east).unwrap();
                let Some(instant) = local.at_offset(offset) else {
                    continue;
                };
                let state = self.state_at(instant);
                if state.east == east && !found.iter().any(|kept| kept.instant == instant) {
                    found.push(Reading::new(instant, offset, state.daylight));
                }
            }
            found.sort_unstable();
            found
        }

        /// Find the forward jump that skipped `local`.
        /// A reading under the preceding offset must fall within the width of that jump.
        fn gap_at(self, local: CivilDateTime) -> Option<LocalResolution> {
            let mut before = self.east;
            for shift in self.shifts {
                let earlier = UtcOffset::from_seconds(before).unwrap();
                let later = UtcOffset::from_seconds(shift.east).unwrap();
                let width = i64::from(shift.east.checked_sub(before).unwrap());
                let closed = Instant::from_unix_seconds(shift.at);
                let read_ahead = local.at_offset(earlier);
                let inside = width > 0
                    && read_ahead.is_some_and(|at| {
                        at >= closed && at < closed.checked_add_seconds(width).unwrap()
                    });
                if inside {
                    return Some(LocalResolution::Nonexistent {
                        gap_start: closed.checked_add_seconds(-1).unwrap(),
                        gap_end: closed,
                        offset_before: earlier,
                        offset_after: later,
                        shifted: read_ahead.unwrap(),
                    });
                }
                before = shift.east;
            }
            None
        }
    }

    /// The zones one test calendar can name.
    #[derive(Clone, Copy, Debug)]
    struct Zones(&'static [Zone]);

    impl Zones {
        /// Look up the exact zone identifier.
        fn find(self, tzid: &str) -> Option<Zone> {
            self.0.iter().copied().find(|zone| zone.tzid == tzid)
        }
    }

    impl ZoneSource for Zones {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            let zone = self.find(tzid)?;
            let readings = zone.readings_of(local);
            let resolution = match (readings.first(), readings.get(1)) {
                (Some(only), None) => LocalResolution::Unique { reading: *only },
                (Some(first), Some(second)) => LocalResolution::Ambiguous {
                    earlier: *first,
                    later: *second,
                },
                _ => zone.gap_at(local)?,
            };
            let seen = match resolution {
                LocalResolution::Unique { reading } => reading.instant,
                LocalResolution::Ambiguous { earlier, .. } => earlier.instant,
                LocalResolution::Nonexistent { gap_end, .. } => gap_end,
                LocalResolution::Undetermined | LocalResolution::Unreadable => return None,
            };
            Some(ZoneAnswer::new(
                resolution,
                ZoneProvenance::EmbeddedVtimezone,
                zone.basis_at(seen),
            ))
        }

        fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
            let zone = self.find(tzid)?;
            let state = zone.state_at(instant);
            Some(OffsetAnswer::new(
                UtcOffset::from_seconds(state.east).unwrap(),
                state.daylight,
                ZoneProvenance::EmbeddedVtimezone,
                zone.basis_at(instant),
            ))
        }
    }

    /// In `Europe/Berlin`, `CET` is `+01:00` and `CEST` is `+02:00`.
    /// Both 2026 transitions occur at 01:00 UTC.
    const BERLIN: &[Shift] = &[
        Shift {
            at: 1_774_746_000,
            east: 7200,
            daylight: true,
        },
        Shift {
            at: 1_792_890_000,
            east: 3600,
            daylight: false,
        },
    ];

    /// In `America/New_York`, `EST` is `-05:00` and `EDT` is `-04:00`.
    /// The 2026 transitions occur at 02:00 local time on `2026-03-08` and `2026-11-01`.
    const NEW_YORK: &[Shift] = &[
        Shift {
            at: 1_772_953_200,
            east: -14400,
            daylight: true,
        },
        Shift {
            at: 1_793_512_800,
            east: -18000,
            daylight: false,
        },
    ];

    /// In `America/Havana`, `CST` is `-05:00` and `CDT` is `-04:00`.
    /// The clock skips midnight on `2026-03-08` and repeats midnight on `2026-11-01`.
    const HAVANA: &[Shift] = &[
        Shift {
            at: 1_772_946_000,
            east: -14400,
            daylight: true,
        },
        Shift {
            at: 1_793_509_200,
            east: -18000,
            daylight: false,
        },
    ];

    /// In `Pacific/Apia`, the change from `-10:00` to `+14:00` skipped `2011-12-30`.
    const APIA: &[Shift] = &[Shift {
        at: 1_325_239_200,
        east: 50400,
        daylight: true,
    }];

    /// The finite Berlin table ends with the transition on `2029-10-28`.
    const THROUGH_2029: &[Shift] = &[Shift {
        at: 1_887_843_600,
        east: 3600,
        daylight: false,
    }];

    /// The calendar's zones, including two identifiers no tz database has.
    static SOURCE: Zones = Zones(&[
        Zone {
            tzid: "Europe/Berlin",
            east: 3600,
            shifts: BERLIN,
            known_through: None,
        },
        Zone {
            tzid: "America/New_York",
            east: -18000,
            shifts: NEW_YORK,
            known_through: None,
        },
        Zone {
            tzid: "America/Havana",
            east: -18000,
            shifts: HAVANA,
            known_through: None,
        },
        // The fixture source maps the Exchange identifier to Berlin.
        Zone {
            tzid: "W. Europe Standard Time",
            east: 3600,
            shifts: BERLIN,
            known_through: None,
        },
        Zone {
            tzid: "Pacific/Apia",
            east: -36000,
            shifts: APIA,
            known_through: None,
        },
        Zone {
            tzid: "/mozilla.org/20050126_1/Europe/Berlin",
            east: 3600,
            shifts: THROUGH_2029,
            known_through: Some((2029, 10, 28)),
        },
    ]);

    fn date(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        CivilDateTime::new(
            date(year, month, day),
            CivilTime::from_hms(hour, minute, 0).unwrap(),
        )
    }

    /// Encode a wall clock as a nominal cadence key.
    fn key(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        nominal(stamp(year, month, day, hour, minute)).unwrap()
    }

    fn series(tzid: &'static str, reading: ExclusionReading) -> ZonedSeries<'static, Zones> {
        ZonedSeries::new(
            &SOURCE,
            tzid,
            ResolutionPolicy::DEFAULT.with_exclusions(reading),
        )
    }

    fn meter() -> Meter {
        Meter::new(Limits::DEFAULT)
    }

    fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
        reported.iter().copied().map(Diagnostic::code).collect()
    }

    fn seconds(span: LocalInterval) -> i64 {
        span.start().checked_seconds_until(span.end()).unwrap()
    }

    /// Both exclusion policies report the mismatch and keep its intended reading.
    #[test]
    fn a_date_exclusion_against_a_date_time_series_is_never_a_silent_no_op() {
        let excluded = [DateTimeValue::Date(date(2026, 3, 29))];
        for reading in [ExclusionReading::Instantaneous, ExclusionReading::WholeDay] {
            let zoned = series("Europe/Berlin", reading);
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &excluded,
                &mut ledger,
                &mut reported,
            );
            assert!(!resolved.is_empty(), "{reading:?} removed nothing at all");
            assert_eq!(
                codes(&reported),
                [DiagnosticCode::ExdateValueTypeMismatch],
                "{reading:?}"
            );
            assert_eq!(reported[0].instant(), Some(key(2026, 3, 29, 0, 0)));
        }

        let instantaneous = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let literal = ResolvedExclusions::read(
            &instantaneous,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(literal.instants(), [key(2026, 3, 29, 0, 0)]);
        assert_eq!(literal.spans(), []);
        // Which is exactly the failure: a 09:00 series has no key at midnight, so the literal reading removes nothing and only the diagnostic survives to say the producer tried.
        assert!(!literal.excludes(key(2026, 3, 29, 9, 0)));

        let whole = series("Europe/Berlin", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let expanded = ResolvedExclusions::read(
            &whole,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(expanded.instants(), []);
        assert!(expanded.excludes(key(2026, 3, 29, 9, 0)));
        assert!(!expanded.excludes(key(2026, 3, 30, 9, 0)));
        assert!(!expanded.excludes(key(2026, 3, 28, 9, 0)));
    }

    /// Havana's midnight gap produces adjacent nominal spans of 25 and 23 hours.
    #[test]
    fn a_whole_day_span_is_the_zone_s_own_day_and_not_a_fixed_86_400_seconds() {
        let cases = [
            (
                date(2026, 3, 7),
                key(2026, 3, 7, 0, 0),
                key(2026, 3, 8, 1, 0),
                90_000,
            ),
            (
                date(2026, 3, 8),
                key(2026, 3, 8, 1, 0),
                key(2026, 3, 9, 0, 0),
                82_800,
            ),
            (
                date(2026, 11, 1),
                key(2026, 11, 1, 0, 0),
                key(2026, 11, 2, 0, 0),
                86_400,
            ),
            (
                date(2026, 6, 15),
                key(2026, 6, 15, 0, 0),
                key(2026, 6, 16, 0, 0),
                86_400,
            ),
        ];
        for (day, start, end, length) in cases {
            let zoned = series("America/Havana", ExclusionReading::WholeDay);
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &[DateTimeValue::Date(day)],
                &mut ledger,
                &mut reported,
            );
            let span = resolved.spans()[0];
            assert_eq!((span.start(), span.end()), (start, end), "{day:?}");
            assert_eq!(seconds(span), length, "{day:?}");
        }

        // The adjacent spans meet at the end of the gap.
        let zoned = series("America/Havana", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let pair = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[
                DateTimeValue::Date(date(2026, 3, 8)),
                DateTimeValue::Date(date(2026, 3, 7)),
            ],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(pair.spans().len(), 2);
        assert_eq!(pair.spans()[0].end(), pair.spans()[1].start());
        assert!(
            pair.excludes(key(2026, 3, 8, 0, 30)),
            "a key in the gap is the earlier day's"
        );
        assert!(pair.excludes(key(2026, 3, 8, 9, 0)));
        assert!(!pair.excludes(key(2026, 3, 9, 0, 0)));
    }

    /// A provider failure at either midnight leaves the whole-day exclusion unresolved.
    #[test]
    fn unreadable_midnight_leaves_the_whole_day_unplaced() {
        struct UnreadableAt(CivilDateTime);
        impl ZoneSource for UnreadableAt {
            fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
                let mut answer = SOURCE.resolve(tzid, local)?;
                if local == self.0 {
                    answer.resolution = LocalResolution::Unreadable;
                }
                Some(answer)
            }
            fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
                SOURCE.offset_at(tzid, instant)
            }
        }
        for boundary in [15, 16] {
            let source = UnreadableAt(stamp(2026, 6, boundary, 0, 0));
            let zone = ZonedSeries::new(
                &source,
                "Europe/Berlin",
                ResolutionPolicy::DEFAULT.with_exclusions(ExclusionReading::WholeDay),
            );
            let mut notes = Vec::new();
            let excluded = ResolvedExclusions::read(
                &zone,
                ValueType::DateTime,
                &[DateTimeValue::Date(date(2026, 6, 15))],
                &mut meter(),
                &mut notes,
            );
            assert_eq!(excluded.unplaced(), &[key(2026, 6, 15, 0, 0)]);
            assert_eq!(excluded.instants(), []);
            assert_eq!(
                codes(&notes),
                [
                    DiagnosticCode::ExdateValueTypeMismatch,
                    DiagnosticCode::ExdateBoundaryUnreadable
                ]
            );
        }
    }

    #[test]
    fn a_date_the_zone_skipped_entirely_falls_back_to_the_instant() {
        let zoned = series("Pacific/Apia", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let resolved = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Date(date(2011, 12, 30))],
            &mut ledger,
            &mut reported,
        );
        assert!(
            resolved.spans().is_empty(),
            "a day of no width is not a span"
        );
        assert_eq!(resolved.instants(), [key(2011, 12, 30, 0, 0)]);
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        // The next local date starts when Samoa crossed the date line.
        let mut ledger = meter();
        let mut reported = Vec::new();
        let after = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Date(date(2011, 12, 31))],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(seconds(after.spans()[0]), 86_400);
        assert!(after.excludes(key(2011, 12, 31, 9, 0)));
    }

    /// Havana's fall-back day contains 25 elapsed hours and 24 nominal hours.
    #[test]
    fn the_real_day_and_the_nominal_span_are_different_lengths_on_purpose() {
        let midnight = SOURCE
            .resolve("America/Havana", stamp(2026, 11, 1, 0, 0))
            .unwrap();
        assert!(midnight.resolution.is_ambiguous());
        let next = SOURCE
            .resolve("America/Havana", stamp(2026, 11, 2, 0, 0))
            .unwrap();
        let opened = midnight.resolution.earliest().unwrap();
        let closed = next.resolution.earliest().unwrap();
        assert_eq!(
            opened.checked_seconds_until(closed).unwrap(),
            90_000,
            "Cuba's 2026-11-01 really is twenty-five hours long"
        );

        let sprang = SOURCE
            .resolve("America/Havana", stamp(2026, 3, 8, 0, 0))
            .unwrap();
        assert!(
            sprang.resolution.is_nonexistent(),
            "midnight on 2026-03-08 is a local time Cuba never showed"
        );
    }

    /// Matching value types project each exception onto the series clock without a mismatch diagnostic.
    #[test]
    fn an_agreeing_exclusion_crosses_the_seam_and_is_reported_by_nothing() {
        // In Berlin, `09:00` on `2026-07-01` and `07:00Z` identify the same occurrence.
        let cases = [
            DateTimeValue::Local(stamp(2026, 7, 1, 9, 0)),
            DateTimeValue::Zoned {
                stamp: stamp(2026, 7, 1, 9, 0),
                tzid: b"Europe/Berlin",
            },
            DateTimeValue::Utc(stamp(2026, 7, 1, 7, 0)),
            DateTimeValue::Zoned {
                stamp: stamp(2026, 7, 1, 3, 0),
                tzid: b"America/New_York",
            },
        ];
        for value in cases {
            let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &[value],
                &mut ledger,
                &mut reported,
            );
            assert_eq!(
                resolved.instants(),
                [key(2026, 7, 1, 9, 0)],
                "{value:?} did not land on the series' own 09:00"
            );
            assert!(
                reported.is_empty(),
                "{value:?} was reported and should not be"
            );
        }

        // The bug the conversion prevents, stated as the wrong answer it would give: reading 07:00Z as though it were already nominal excludes 07:00 local, two hours early.
        assert_ne!(key(2026, 7, 1, 7, 0), key(2026, 7, 1, 9, 0));
    }

    /// A date-time exclusion also differs from a date-only start.
    #[test]
    fn a_date_time_exclusion_against_an_all_day_series_is_reported_as_well() {
        let excluded = [DateTimeValue::Zoned {
            stamp: stamp(2026, 7, 1, 9, 0),
            tzid: b"Europe/Berlin",
        }];
        let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let literal = ResolvedExclusions::read(
            &zoned,
            ValueType::Date,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(literal.instants(), [key(2026, 7, 1, 9, 0)]);
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        let whole = series("Europe/Berlin", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let expanded = ResolvedExclusions::read(
            &whole,
            ValueType::Date,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert!(
            expanded.excludes(key(2026, 7, 1, 0, 0)),
            "an all-day series' key is midnight, and the whole day covers it"
        );
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        let mut ledger = meter();
        let mut reported = Vec::new();
        let agreed = ResolvedExclusions::read(
            &zoned,
            ValueType::Date,
            &[DateTimeValue::Date(date(2026, 7, 1))],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(agreed.instants(), [key(2026, 7, 1, 0, 0)]);
        assert_eq!(reported.as_slice(), []);
    }

    /// The recurrence input requires unique instants in ascending order.
    #[test]
    fn the_instants_come_out_strictly_ascending_and_deduplicated() {
        let excluded = [
            DateTimeValue::Local(stamp(2026, 7, 3, 9, 0)),
            DateTimeValue::Local(stamp(2026, 7, 1, 9, 0)),
            DateTimeValue::Utc(stamp(2026, 7, 1, 7, 0)),
            DateTimeValue::Local(stamp(2026, 7, 2, 9, 0)),
        ];
        let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let resolved = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(
            resolved.instants(),
            [
                key(2026, 7, 1, 9, 0),
                key(2026, 7, 2, 9, 0),
                key(2026, 7, 3, 9, 0),
            ],
            "the UTC entry names the same occurrence as the floating one and is folded into it"
        );
        assert!(resolved.instants().windows(2).all(|pair| pair[0] < pair[1]));

        let empty =
            ResolvedExclusions::read(&zoned, ValueType::DateTime, &[], &mut ledger, &mut reported);
        assert!(empty.is_empty());
        assert!(!empty.excludes(key(2026, 7, 1, 9, 0)));
    }

    /// A table that ran out still answers, and the exclusion still lands: `BeyondKnownTransitions` is a fact about the answer and never a reason to drop the producer's exception.
    #[test]
    fn an_exclusion_past_the_end_of_a_finite_table_is_still_applied() {
        let zoned = series(
            "/mozilla.org/20050126_1/Europe/Berlin",
            ExclusionReading::WholeDay,
        );
        let mut ledger = meter();
        let mut reported = Vec::new();
        let resolved = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Date(date(2035, 3, 15))],
            &mut ledger,
            &mut reported,
        );
        let span = resolved.spans()[0];
        assert_eq!(span.start(), key(2035, 3, 15, 0, 0));
        assert_eq!(
            seconds(span),
            86_400,
            "the continued observance has no transition in it"
        );
        assert!(resolved.excludes(key(2035, 3, 15, 9, 0)));
        assert_eq!(
            codes(&reported),
            [DiagnosticCode::ExdateValueTypeMismatch],
            "the exhausted table is unit 5's report at the occurrence, not this unit's here"
        );

        let answer = SOURCE
            .resolve(
                "/mozilla.org/20050126_1/Europe/Berlin",
                stamp(2035, 3, 15, 0, 0),
            )
            .unwrap();
        assert_eq!(
            answer.basis,
            AnswerBasis::BeyondKnownTransitions(date(2029, 10, 28)),
            "the source really is out of data, which is what makes the span above a continuation"
        );
    }

    /// Only the source interprets a vendor identifier.
    #[test]
    fn an_identifier_that_is_not_a_database_name_is_neither_parsed_nor_refused() {
        let excluded = [DateTimeValue::Date(date(2026, 3, 29))];
        let exchange = series("W. Europe Standard Time", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let answered = ResolvedExclusions::read(
            &exchange,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(answered.spans()[0].start(), key(2026, 3, 29, 0, 0));
        assert_eq!(seconds(answered.spans()[0]), 86_400);
        assert!(answered.excludes(key(2026, 3, 29, 9, 0)));

        let stranger = series("Customized Time Zone", ExclusionReading::WholeDay);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let unknown = ResolvedExclusions::read(
            &stranger,
            ValueType::DateTime,
            &excluded,
            &mut ledger,
            &mut reported,
        );
        assert_eq!(
            (unknown.spans()[0].start(), seconds(unknown.spans()[0])),
            (key(2026, 3, 29, 0, 0), 86_400),
            "nothing knows the zone, so the plain civil day is the only reading left"
        );
        assert!(unknown.excludes(key(2026, 3, 29, 9, 0)));
        assert_eq!(codes(&reported), [DiagnosticCode::ExdateValueTypeMismatch]);

        // An exclusion written in a zone nothing knows keeps its own wall clock rather than vanishing, which is the same refusal to drop input stated at the other end.
        let berlin = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let kept = ResolvedExclusions::read(
            &berlin,
            ValueType::DateTime,
            &[DateTimeValue::Zoned {
                stamp: stamp(2026, 7, 1, 9, 0),
                tzid: b"Customized Time Zone",
            }],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(kept.instants(), [key(2026, 7, 1, 9, 0)]);
        assert_eq!(reported.as_slice(), []);
    }

    /// A foreign zone's gap or fold uses the caller's resolution policy.
    #[test]
    fn an_awkward_hour_in_a_foreign_zone_is_collapsed_under_the_stated_policy() {
        // In New York, `2026-11-01T01:30` names `05:30Z` under `EDT` and `06:30Z` under `EST`.
        // Berlin reads those instants as `06:30` and `07:30`.
        let value = [DateTimeValue::Zoned {
            stamp: stamp(2026, 11, 1, 1, 30),
            tzid: b"America/New_York",
        }];
        let cases = [
            (FoldPolicy::Earlier, key(2026, 11, 1, 6, 30)),
            (FoldPolicy::Later, key(2026, 11, 1, 7, 30)),
        ];
        for (folds, expected) in cases {
            let zoned = ZonedSeries::new(
                &SOURCE,
                "Europe/Berlin",
                ResolutionPolicy::DEFAULT.with_folds(folds),
            );
            let mut ledger = meter();
            let mut reported = Vec::new();
            let resolved = ResolvedExclusions::read(
                &zoned,
                ValueType::DateTime,
                &value,
                &mut ledger,
                &mut reported,
            );
            assert_eq!(resolved.instants(), [expected], "{folds:?}");
        }

        // New York skips `2026-03-08T02:30`.
        // The default gap policy leaves the written wall clock for this exclusion.
        let zoned = series("Europe/Berlin", ExclusionReading::Instantaneous);
        let mut ledger = meter();
        let mut reported = Vec::new();
        let sprang = ResolvedExclusions::read(
            &zoned,
            ValueType::DateTime,
            &[DateTimeValue::Zoned {
                stamp: stamp(2026, 3, 8, 2, 30),
                tzid: b"America/New_York",
            }],
            &mut ledger,
            &mut reported,
        );
        assert_eq!(sprang.instants(), [key(2026, 3, 8, 2, 30)]);
    }
}
