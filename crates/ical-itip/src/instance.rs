// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which instance a message names, once a zone has been asked.
//!
//! Specification: RFC 5545 section 3.8.4.4 (`RECURRENCE-ID`), section 3.2.13 (`RANGE`) and
//! section 3.6.5 (`VTIMEZONE`); RFC 5546 section 3.7.1 for what a `RECURRENCE-ID` addresses in
//! a scheduling message. Every instant crossing into or out of this module does so under the
//! contract [`ical_tz::seam`] states, and this module may not restate or reinterpret it.
//!
//! This is the only place `ical-itip` asks a zone anything. The gate holds no zone by design —
//! [`crate::evaluate_message`] takes a message, a state and a party, and nothing else — so
//! everything a zone decides is decided here, before the gate runs, and is carried into it on
//! the values the caller already passes.
//!
//! # Which half of a repeated hour, which closes agenda item 1
//!
//! The two halves of an hour a zone falls back through are **one cadence key** on the nominal
//! timeline, so `20261101T053000Z` and `20261101T063000Z` in `America/New_York` are two
//! meetings and one key. [`FoldSide`] is which of them an identity means, and
//! [`resolve_instance`] is what derives one from the zone the series actually runs on.
//!
//! What it can and cannot recover is worth stating plainly, because the limit is the file's and
//! not this code's. A `RECURRENCE-ID` written with a trailing `Z` names a real instant, so it
//! picks its own half and resolves to [`FoldSide::Earlier`] or [`FoldSide::Later`]. One written
//! as a wall clock — with a `TZID` or with nothing — names *both* halves, so nothing here can
//! pick between them and the answer stays [`FoldSide::Unresolved`]. That is reported as
//! `scheduling-instance-ambiguous` and it stays a denial: [`crate::InstanceRef::compare`]
//! answers [`crate::InstanceMatch::Ambiguous`], [`crate::evaluate_message`] refuses it as
//! [`crate::AuthorizationDenied::AmbiguousInstance`], and a guess between the two halves
//! cancels somebody else's meeting.
//!
//! # A continuation is made visible and is not decided, which closes agenda item 4
//!
//! A source asked past either end of its transition table continues the observance at that end
//! and says so through [`AnswerBasis`]. When such an answer decided *identity* rather than
//! merely rendered a time, `scheduling-zone-continued` travels and
//! [`ResolvedInstance::nearest_known`] carries the edge of the source's knowledge, so a
//! scheduler deciding whether to send can see whether the continuation reached one day or six
//! years. Which of those is acceptable is the caller's policy and is not encoded here: this
//! unit's job is to make the distance visible, not to take the decision.
//!
//! # An exclusion no zone could place, which closes agenda item 3
//!
//! A `Z`-terminated `EXDATE` on a series whose `TZID` no source recognizes names a real instant
//! that nothing can put on the series' own wall clock; `ical-tz` keeps it in
//! [`ical_tz::ResolvedExclusions::unplaced`] rather than dropping it. Which instances such a
//! series has is then not decidable, so an instance-addressed `CANCEL` or `COUNTER` against it
//! is a **refusal and not a guess**: ignoring the exclusion reinstates a meeting the user
//! cancelled, and guessing cancels a different one.
//!
//! [`exclusions_are_placeable`] is that precondition and
//! [`check_exclusions_are_placeable`] is it with the report attached. It is wired **before**
//! [`crate::evaluate_message`] and not inside it, because the gate is handed no zone and no
//! exclusion list and inventing a parameter for one would put a zone inside the authorization
//! decision:
//!
//! ```text
//! let exclusions = ResolvedExclusions::read(&series, kind, &exdates, &mut meter, &mut sink);
//! if !check_exclusions_are_placeable(&exclusions, &mut meter, &mut sink) {
//!     return Refused::UndecidableSeries;   // the caller's own refusal, not an
//! }                                        // AuthorizationDenied: no gate ran
//! let authorization = evaluate_message(&message, current, actor)?;
//! ```
//!
//! # What is emitted here, and what is deliberately not
//!
//! Three codes and no others: `scheduling-instance-ambiguous`, `scheduling-zone-continued` and
//! `scheduling-exclusion-unplaced`. `ambiguous-local-time`, `nonexistent-local-time`,
//! `time-zone-coverage-exhausted` and `exdate-zone-unknown` are `ical-tz`'s own, emitted where
//! that crate resolves an occurrence or reads an exclusion list, and reporting one of them a
//! second time here would make one defect look like two.
//!
//! `scheduling-zone-continued` in particular is not a rename of
//! `time-zone-coverage-exhausted`. That code says a rendered time rested on a continuation;
//! this one says an *identity* did — which meeting a message is about — and the two are read by
//! different people for different decisions.
//!
//! A local time the zone never showed is reported by neither: it resolves to nothing, the
//! identity stays unresolved, and the message is refused for naming an instance the state does
//! not have. That refusal is the report.

use ical_core::{
    CivilDate, Diagnostic, DiagnosticCode, DiagnosticSink, Instant, Meter, Severity,
    report_diagnostic,
};
use ical_tz::{AnswerBasis, ResolvedExclusions, ZoneAnswer, ZoneSource, ZonedSeries};

use crate::identity::{FoldSide, InstanceClock, InstanceRef};

/// What a zone said about one instance identity, and how much of the zone stood behind it.
///
/// [`resolve_instance`]'s answer. It is an [`InstanceRef`] carrying the side that was resolved,
/// plus the one fact an `InstanceRef` has nowhere to keep: which end of a source's transition
/// table the answer came from. The brief for this unit spelled the return type as a bare
/// `InstanceRef`; that type is frozen and holds no [`AnswerBasis`], so carrying the basis in a
/// wrapper is how [`ResolvedInstance::nearest_known`] exists at all. The
/// `From<ResolvedInstance>` implementation and [`ResolvedInstance::reference`] hand back exactly
/// the value the brief named, so nothing above has to know this type to use one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResolvedInstance {
    /// The reference, carrying whatever side the zone resolved.
    reference: InstanceRef,
    /// How much of the source's data stood behind the answer, absent when nothing answered.
    basis: Option<AnswerBasis>,
}

impl ResolvedInstance {
    /// The reference, with the resolved side on it.
    #[must_use]
    pub const fn reference(self) -> InstanceRef {
        self.reference
    }

    /// Which half of a repeated wall clock this identity means.
    #[must_use]
    pub const fn side(self) -> FoldSide {
        self.reference.side()
    }

    /// Whether a zone told this identity from its neighbor.
    ///
    /// `false` means the comparison below it answers [`crate::InstanceMatch::Ambiguous`] and
    /// the gate refuses, which is the conservative direction and the intended one.
    #[must_use]
    pub const fn is_resolved(self) -> bool {
        self.reference.side().is_resolved()
    }

    /// How much of the source's data stood behind the answer, absent when nothing answered.
    ///
    /// `None` means the source did not recognize this series' identifier at all — the one
    /// condition [`ical_tz::ZoneSource::resolve`] reserves its own `None` for — and never that
    /// the zone had no data for the question, which is [`AnswerBasis`]' own business.
    #[must_use]
    pub const fn basis(self) -> Option<AnswerBasis> {
        self.basis
    }

    /// Whether the answer continued past one end of the source's transition table.
    #[must_use]
    pub const fn is_continued(self) -> bool {
        match self.basis {
            Some(basis) => !basis.is_computed(),
            None => false,
        }
    }

    /// The nearest date the source has real data for, absent when it had data for the question.
    ///
    /// Agenda item 4's whole point. A source that continued one day and one that continued six
    /// years are the same variant of [`AnswerBasis`], and this date is what tells them apart —
    /// so a scheduler deciding whether to send can weigh the distance itself rather than have
    /// this crate weigh it for every caller at once.
    #[must_use]
    pub const fn nearest_known(self) -> Option<CivilDate> {
        match self.basis {
            Some(basis) => basis.nearest_known(),
            None => None,
        }
    }
}

impl From<ResolvedInstance> for InstanceRef {
    /// The reference alone, for a caller that wants the value the identity vocabulary is
    /// written in and has already read whatever it needed from the basis.
    fn from(resolved: ResolvedInstance) -> Self {
        resolved.reference
    }
}

/// Resolve `reference`'s wall clock through `series`' zone and attach the side it names.
///
/// The three shapes a `RECURRENCE-ID` takes are three different questions, and only one of them
/// has an answer that can pick a side:
///
/// - Written with a trailing `Z`, it names a real instant. That instant is projected onto the
///   series' own timeline, the wall clock there is resolved, and the instant is compared
///   against the two readings of a fold — so it picks its own half.
/// - Written with a `TZID`, it is already a wall clock on that timeline (`ical_tz::seam`), so
///   it is resolved as written and names *both* halves of a fold, which is
///   [`FoldSide::Unresolved`] and a reported ambiguity.
/// - Written with neither, it is a wall clock with no zone of its own. It is read on the
///   series' clock, which is the reading a client that ignores zones performs and is at least a
///   reading; it can no more pick a half than a zoned one can.
///
/// A source that does not recognize this series' identifier answers nothing, and nothing is
/// reported: no zone said this wall clock repeats, so there is no ambiguity to claim. The side
/// stays [`FoldSide::Unresolved`], which is what a caller with no zone would have had anyway.
#[must_use]
pub fn resolve_instance<S, D>(
    series: &ZonedSeries<'_, S>,
    reference: InstanceRef,
    meter: &mut Meter,
    sink: &mut D,
) -> ResolvedInstance
where
    S: ZoneSource + ?Sized,
    D: DiagnosticSink + ?Sized,
{
    let Some(answer) = ask_zone(series, reference) else {
        return ResolvedInstance {
            reference,
            basis: None,
        };
    };
    let side = FoldSide::from_resolution(answer.resolution, real_instant(reference));
    // The ambiguity first and the continuation second, for the reason `ical_tz::series` orders
    // its two the same way: the fact about this identity's own wall clock is the one a reader
    // is looking for, and the fact about the table behind it is the qualification.
    if answer.resolution.is_ambiguous() && !side.is_resolved() {
        report_at(
            DiagnosticCode::SchedulingInstanceAmbiguous,
            reference.named(),
            meter,
            sink,
        );
    }
    if !answer.basis.is_computed() {
        report_at(
            DiagnosticCode::SchedulingZoneContinued,
            reference.named(),
            meter,
            sink,
        );
    }
    ResolvedInstance {
        reference: reference.with_side(side),
        basis: Some(answer.basis),
    }
}

/// Whether every exclusion on this series could be placed on its timeline.
///
/// The precondition an instance-addressed message is judged behind. `false` means the series
/// carries an exclusion no zone could place, so which instances it has is not decidable and a
/// `CANCEL` or `COUNTER` naming one of them must be refused rather than guessed at.
///
/// Pure, so a caller that has already reported the condition can ask again without reporting it
/// twice; [`check_exclusions_are_placeable`] is the same question with the report attached.
#[must_use]
pub fn exclusions_are_placeable(exclusions: &ResolvedExclusions) -> bool {
    exclusions.unplaced().is_empty()
}

/// Answer [`exclusions_are_placeable`], reporting each exclusion that could not be placed.
///
/// One `scheduling-exclusion-unplaced` per unplaced instant, at that instant. The count is
/// bounded by whoever read the `EXDATE` list against `Limits::exdate_entries`, which is where
/// ADR-0010 charges it; charging it again here would bill one calendar twice.
///
/// The answer must be read. Ignoring it is exactly the failure this function exists to prevent
/// — the message gets judged against a series whose instances nobody could enumerate.
#[must_use]
pub fn check_exclusions_are_placeable<D>(
    exclusions: &ResolvedExclusions,
    meter: &mut Meter,
    sink: &mut D,
) -> bool
where
    D: DiagnosticSink + ?Sized,
{
    for unplaced in exclusions.unplaced() {
        report_at(
            DiagnosticCode::SchedulingExclusionUnplaced,
            *unplaced,
            meter,
            sink,
        );
    }
    exclusions_are_placeable(exclusions)
}

/// What `series`' zone says about the wall clock `reference` stands for.
///
/// The two directions of `ical_tz::seam`, and the whole of this module's dependence on a zone.
/// A `Z`-terminated value names a real instant and has to be projected onto the series'
/// timeline before the zone can be asked about the clock it shows there; every other value is
/// already on that timeline and is asked about as written.
fn ask_zone<S>(series: &ZonedSeries<'_, S>, reference: InstanceRef) -> Option<ZoneAnswer>
where
    S: ZoneSource + ?Sized,
{
    match reference.clock() {
        InstanceClock::Utc => series.answer_for(series.to_nominal(reference.named())?),
        InstanceClock::Zoned | InstanceClock::Floating => series.answer_for(reference.named()),
    }
}

/// The real instant `reference` names, present only when the value named one.
///
/// What [`FoldSide::from_resolution`] compares against the two readings of a fold. A wall clock
/// names both of them and therefore names neither, so it is `None` here rather than an
/// arithmetic coincidence that would pick a half nobody wrote.
const fn real_instant(reference: InstanceRef) -> Option<Instant> {
    match reference.clock() {
        InstanceClock::Utc => Some(reference.named()),
        InstanceClock::Zoned | InstanceClock::Floating => None,
    }
}

/// Report `code` about the identity at `at`, on the channel that code travels on.
fn report_at<D>(code: DiagnosticCode, at: Instant, meter: &mut Meter, sink: &mut D)
where
    D: DiagnosticSink + ?Sized,
{
    report_diagnostic(
        sink,
        meter,
        Diagnostic::at_instant(code, channel_for(code), at),
    );
}

/// The channel a code emitted here travels on, as `docs/diagnostic-codes.md` fixes it.
///
/// A continued zone answer is a `Note` for the reason `time-zone-coverage-exhausted` is one:
/// the file is legal and continuing is the defensible answer. The other two are `Violation`,
/// because an identity nothing could resolve and a series nobody could enumerate are both
/// states in which a message must not be applied.
///
/// The last arm is required rather than chosen, since [`DiagnosticCode`] is
/// `#[non_exhaustive]`, and it lands on `Violation` because under-stating a claim is the
/// failure that hides.
fn channel_for(code: DiagnosticCode) -> Severity {
    match code {
        DiagnosticCode::SchedulingZoneContinued => Severity::Note,
        _ => Severity::Violation,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, DateTimeValue, Diagnostic, DiagnosticCode, Instant,
        Limits, Meter, Severity, UtcOffset, ValueType,
    };
    use ical_recur::OverrideRange;
    use ical_tz::{
        AnswerBasis, LocalResolution, OffsetAnswer, Reading, ResolutionPolicy, ResolvedExclusions,
        ZoneAnswer, ZoneProvenance, ZoneSource, ZonedSeries, nominal,
    };

    use super::{
        ResolvedInstance, check_exclusions_are_placeable, exclusions_are_placeable,
        resolve_instance,
    };
    use crate::identity::{
        FoldSide, InstanceClock, InstanceMatch, InstanceRef, Revision, SequenceRead,
    };

    /// One transition of a real zone: when the clock moved, and what it moved between.
    ///
    /// Every value below is transcribed from the rules the zone actually ran, never read off
    /// this crate's answers.
    #[derive(Clone, Copy, Debug)]
    struct Shift {
        /// The instant the offset changed.
        at: Instant,
        /// Seconds east of UTC before it.
        before: i32,
        /// Seconds east of UTC from it.
        after: i32,
        /// Whether the observance beginning here is the zone's daylight one.
        daylight: bool,
    }

    /// A zone source built from published transitions, standing in for a read `VTIMEZONE`.
    ///
    /// Written here rather than reached for from the unit that owns the real source, for the
    /// reason every other test module in this workspace gives: what is under test is this
    /// unit, and a test leaning on another unit's file would be testing that unit too.
    #[derive(Clone, Debug)]
    struct TestZone {
        /// The identifier this source answers to, compared by exact bytes.
        tzid: &'static str,
        /// Seconds east of UTC before the first transition.
        base: i32,
        /// The transitions, ascending.
        shifts: Vec<Shift>,
        /// The first date backed by real data, absent when the table reaches back forever.
        known_from: Option<CivilDate>,
        /// The last date backed by real data, absent when the table runs on forever.
        known_through: Option<CivilDate>,
    }

    impl TestZone {
        /// The offset and daylight flag in force at `instant`.
        fn state_at(&self, instant: Instant) -> (i32, bool) {
            let mut state = (self.base, false);
            for shift in &self.shifts {
                if shift.at <= instant {
                    state = (shift.after, shift.daylight);
                }
            }
            state
        }

        /// How much of this zone's data stands behind a question about `instant`.
        fn basis_at(&self, instant: Instant) -> AnswerBasis {
            let earlier = self
                .known_from
                .filter(|_| self.shifts.first().is_some_and(|first| instant < first.at));
            let later = self
                .known_through
                .filter(|_| self.shifts.last().is_some_and(|last| instant >= last.at));
            match (earlier, later) {
                (Some(edge), _) => AnswerBasis::BeforeKnownTransitions(edge),
                (_, Some(edge)) => AnswerBasis::BeyondKnownTransitions(edge),
                _ => AnswerBasis::Computed,
            }
        }

        /// Every offset this zone ever runs at.
        fn offsets(&self) -> Vec<i32> {
            let mut seen = alloc::vec![self.base];
            for shift in &self.shifts {
                if !seen.contains(&shift.after) {
                    seen.push(shift.after);
                }
            }
            seen
        }

        /// Every reading `local` has, ascending — none in a gap, two in a fold.
        fn readings(&self, local: CivilDateTime) -> Vec<Reading> {
            let mut found: Vec<Reading> = Vec::new();
            for seconds in self.offsets() {
                let Some(offset) = UtcOffset::from_seconds(seconds) else {
                    continue;
                };
                let Some(instant) = local.at_offset(offset) else {
                    continue;
                };
                let (in_force, daylight) = self.state_at(instant);
                if in_force == seconds {
                    found.push(Reading::new(instant, offset, daylight));
                }
            }
            found.sort_unstable();
            found
        }

        /// The gap `local` fell in, on the readings `ical_tz::LocalResolution` states.
        fn gap(&self, local: CivilDateTime) -> Option<LocalResolution> {
            for shift in &self.shifts {
                let offset_before = UtcOffset::from_seconds(shift.before)?;
                let offset_after = UtcOffset::from_seconds(shift.after)?;
                let opened = CivilDateTime::from_instant(shift.at, offset_before)?;
                let closed = CivilDateTime::from_instant(shift.at, offset_after)?;
                if opened <= local && local < closed {
                    return Some(LocalResolution::Nonexistent {
                        gap_start: shift.at.checked_add_seconds(-1)?,
                        gap_end: shift.at,
                        offset_before,
                        offset_after,
                        shifted: local.at_offset(offset_before)?,
                    });
                }
            }
            None
        }
    }

    impl ZoneSource for TestZone {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            if tzid != self.tzid {
                return None;
            }
            let found = self.readings(local);
            let resolution = match found.as_slice() {
                [reading] => LocalResolution::Unique { reading: *reading },
                [earlier, later] => LocalResolution::Ambiguous {
                    earlier: *earlier,
                    later: *later,
                },
                _ => self.gap(local)?,
            };
            let seen = match resolution {
                LocalResolution::Unique { reading } => reading.instant,
                LocalResolution::Ambiguous { earlier, .. } => earlier.instant,
                LocalResolution::Nonexistent { gap_end, .. } => gap_end,
                // This fixture always holds transitions, so it never answers `Undetermined`;
                // the arm is what `#[non_exhaustive]` asks of a match on another crate's enum.
                _ => return None,
            };
            Some(ZoneAnswer::new(
                resolution,
                ZoneProvenance::EmbeddedVtimezone,
                self.basis_at(seen),
            ))
        }

        fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
            if tzid != self.tzid {
                return None;
            }
            let (seconds, daylight) = self.state_at(instant);
            Some(OffsetAnswer::new(
                UtcOffset::from_seconds(seconds)?,
                daylight,
                ZoneProvenance::EmbeddedVtimezone,
                self.basis_at(instant),
            ))
        }
    }

    fn date(year: u16, month: u8, day: u8) -> CivilDate {
        CivilDate::from_ymd(year, month, day).unwrap()
    }

    /// A wall clock with no zone attached to it yet.
    fn clock(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        CivilDateTime::new(
            date(year, month, day),
            CivilTime::from_hms(hour, minute, 0).unwrap(),
        )
    }

    /// The instant a published UTC timestamp names.
    fn utc(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        clock(year, month, day, hour, minute)
            .at_offset(UtcOffset::UTC)
            .unwrap()
    }

    /// The nominal cadence key a wall clock is, which is what a zoned value carries.
    fn key(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Instant {
        nominal(clock(year, month, day, hour, minute)).unwrap()
    }

    fn shift(at: Instant, before: i32, after: i32, daylight: bool) -> Shift {
        Shift {
            at,
            before,
            after,
            daylight,
        }
    }

    /// `America/New_York` in 2026: forward at 07:00Z on March 8th, back at 06:00Z on November
    /// 1st, between EST at -05:00 and EDT at -04:00.
    ///
    /// So local 01:30 on November 1st happens twice — 05:30Z under EDT and 06:30Z under EST —
    /// and local 02:30 on March 8th happens never.
    fn new_york() -> TestZone {
        TestZone {
            tzid: "America/New_York",
            base: -18_000,
            shifts: alloc::vec![
                shift(utc(2026, 3, 8, 7, 0), -18_000, -14_400, true),
                shift(utc(2026, 11, 1, 6, 0), -14_400, -18_000, false),
            ],
            known_from: None,
            known_through: None,
        }
    }

    /// Berlin's rules written out as explicit dates for 2027 through 2029 and stopping at both
    /// ends, which is what an `RDATE`-driven `VTIMEZONE` from Exchange is.
    ///
    /// A question about 2035 continues the final observance and a question about 2020 continues
    /// the first one backwards; both are answers, and both say what they rest on.
    fn finite_table() -> TestZone {
        TestZone {
            tzid: "W. Europe Standard Time",
            base: 3_600,
            shifts: alloc::vec![
                shift(utc(2027, 3, 28, 1, 0), 3_600, 7_200, true),
                shift(utc(2027, 10, 31, 1, 0), 7_200, 3_600, false),
                shift(utc(2028, 3, 26, 1, 0), 3_600, 7_200, true),
                shift(utc(2028, 10, 29, 1, 0), 7_200, 3_600, false),
                shift(utc(2029, 3, 25, 1, 0), 3_600, 7_200, true),
                shift(utc(2029, 10, 28, 1, 0), 7_200, 3_600, false),
            ],
            known_from: CivilDate::from_ymd(2027, 3, 28),
            known_through: CivilDate::from_ymd(2029, 10, 28),
        }
    }

    /// A meter and a sink, which every reporting call needs and no test wants to spell twice.
    fn ledger() -> (Meter, Vec<Diagnostic>) {
        (Meter::new(Limits::DEFAULT), Vec::new())
    }

    /// The codes reported, in the order they were reported.
    fn codes(reported: &[Diagnostic]) -> Vec<DiagnosticCode> {
        reported.iter().map(|entry| entry.code()).collect()
    }

    /// A `RECURRENCE-ID` naming `at`, written in the clock `written` says, addressing that
    /// instance alone.
    fn reference(at: Instant, written: InstanceClock) -> InstanceRef {
        InstanceRef::new(at, written, OverrideRange::ThisOnly)
    }

    /// One identity resolved against `tzid` through the zone `source` describes.
    fn resolved(
        source: &TestZone,
        tzid: &'static str,
        named: InstanceRef,
        reported: &mut Vec<Diagnostic>,
    ) -> ResolvedInstance {
        let series = ZonedSeries::new(source, tzid, ResolutionPolicy::DEFAULT);
        let mut meter = Meter::new(Limits::DEFAULT);
        resolve_instance(&series, named, &mut meter, reported)
    }

    /// What a caller must do with a message, in the gate's own order: identity first, revision
    /// second (RFC 5546 sections 2.1.4 and 2.1.5).
    ///
    /// Not the gate itself, which is `authorize.rs`'s and holds no zone. This is the shape of
    /// the composition this unit exists to feed, written out so the table below can state an
    /// outcome a user could be shown.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Verdict {
        /// The caller may act on the message.
        Act,
        /// Nothing could tell the named instance from its neighbor.
        AmbiguousInstance,
        /// The message names an instance this state is not.
        NoMatchingInstance,
        /// The message is an older revision than the one held.
        SequenceStale,
        /// The message is the same revision with an older `DTSTAMP`.
        DtstampStale,
    }

    fn verdict(
        message: ResolvedInstance,
        current: ResolvedInstance,
        offered: Revision,
        holding: Revision,
    ) -> Verdict {
        let matched = message.reference().compare(current.reference());
        if matched == InstanceMatch::Ambiguous {
            return Verdict::AmbiguousInstance;
        }
        if !matched.is_same() {
            return Verdict::NoMatchingInstance;
        }
        if !offered.is_stale_against(holding) {
            return Verdict::Act;
        }
        if offered.sequence() < holding.sequence() {
            return Verdict::SequenceStale;
        }
        Verdict::DtstampStale
    }

    /// Agenda item 1, from both directions. A `Z`-terminated `RECURRENCE-ID` names a real
    /// instant and picks its half of the repeated hour; a wall clock names both halves, picks
    /// neither, and says so rather than guessing.
    ///
    /// Every instant here is New York's published 2026 rule, not this crate's answer: EDT is
    /// four hours west, EST five, so local 01:30 on November 1st is 05:30Z and then 06:30Z.
    #[test]
    fn a_z_terminated_recurrence_id_picks_its_half_of_a_repeated_hour() {
        let zone = new_york();
        let cases = [
            (
                reference(utc(2026, 11, 1, 5, 30), InstanceClock::Utc),
                FoldSide::Earlier,
                alloc::vec![],
            ),
            (
                reference(utc(2026, 11, 1, 6, 30), InstanceClock::Utc),
                FoldSide::Later,
                alloc::vec![],
            ),
            (
                reference(key(2026, 11, 1, 1, 30), InstanceClock::Zoned),
                FoldSide::Unresolved,
                alloc::vec![DiagnosticCode::SchedulingInstanceAmbiguous],
            ),
            (
                reference(key(2026, 11, 1, 1, 30), InstanceClock::Floating),
                FoldSide::Unresolved,
                alloc::vec![DiagnosticCode::SchedulingInstanceAmbiguous],
            ),
            (
                reference(key(2026, 11, 1, 9, 0), InstanceClock::Zoned),
                FoldSide::Once,
                alloc::vec![],
            ),
            (
                reference(utc(2026, 7, 1, 13, 0), InstanceClock::Utc),
                FoldSide::Once,
                alloc::vec![],
            ),
        ];
        for (named, side, expected) in cases {
            let mut reported = Vec::new();
            let answer = resolved(&zone, "America/New_York", named, &mut reported);
            assert_eq!(answer.side(), side, "{named:?}");
            assert_eq!(codes(&reported), expected, "{named:?}");
            assert_eq!(answer.basis(), Some(AnswerBasis::Computed), "{named:?}");
            assert_eq!(answer.nearest_known(), None, "{named:?}");
            assert_eq!(answer.reference().named(), named.named(), "{named:?}");
        }

        let mut reported = Vec::new();
        let ambiguous = resolved(
            &zone,
            "America/New_York",
            reference(key(2026, 11, 1, 1, 30), InstanceClock::Zoned),
            &mut reported,
        );
        assert!(!ambiguous.is_resolved());
        assert_eq!(reported[0].severity(), Severity::Violation);
        assert_eq!(reported[0].instant(), Some(key(2026, 11, 1, 1, 30)));
    }

    /// An instance the series never had resolves to nothing, and this unit reports none of it:
    /// a local time the zone sprang over is `ical-tz`'s `nonexistent-local-time` at the
    /// occurrence, and an identifier nothing recognizes is nobody's ambiguity to claim.
    ///
    /// Both leave the side unresolved, which is what the gate above refuses on.
    #[test]
    fn an_instance_the_series_never_had_resolves_to_nothing_and_is_not_reported_here() {
        let zone = new_york();
        let sprang = reference(key(2026, 3, 8, 2, 30), InstanceClock::Zoned);
        let mut reported = Vec::new();
        let missing = resolved(&zone, "America/New_York", sprang, &mut reported);
        assert_eq!(missing.side(), FoldSide::Unresolved);
        assert!(
            reported.is_empty(),
            "a local time the zone sprang over is unit 5's report at the occurrence"
        );

        let mut elsewhere = Vec::new();
        let unknown = resolved(
            &zone,
            "Customized Time Zone",
            reference(key(2026, 11, 1, 1, 30), InstanceClock::Zoned),
            &mut elsewhere,
        );
        assert_eq!(unknown.side(), FoldSide::Unresolved);
        assert_eq!(unknown.basis(), None);
        assert!(!unknown.is_continued());
        assert!(
            elsewhere.is_empty(),
            "no zone said this clock repeats, so there is no ambiguity to report"
        );
    }

    /// Agenda item 4: a continued answer still answers, says that it was continued, and hands
    /// over how far the continuation reached — without deciding what to do about it.
    ///
    /// The last transition the table holds is 2029-10-28 and the first is 2027-03-28. The two
    /// continuations below are one day and roughly six years wide, they report the same code,
    /// and the date is the only thing that tells them apart. That is the decision this unit
    /// deliberately leaves to a scheduler.
    #[test]
    fn a_continued_zone_answer_is_visible_and_the_distance_stays_the_caller_s() {
        let zone = finite_table();
        let last = date(2029, 10, 28);
        let first = date(2027, 3, 28);
        let cases = [
            (
                reference(key(2028, 6, 15, 12, 0), InstanceClock::Zoned),
                AnswerBasis::Computed,
                None,
                alloc::vec![],
            ),
            (
                reference(key(2029, 10, 29, 12, 0), InstanceClock::Zoned),
                AnswerBasis::BeyondKnownTransitions(last),
                Some(last),
                alloc::vec![DiagnosticCode::SchedulingZoneContinued],
            ),
            (
                reference(key(2035, 6, 15, 12, 0), InstanceClock::Zoned),
                AnswerBasis::BeyondKnownTransitions(last),
                Some(last),
                alloc::vec![DiagnosticCode::SchedulingZoneContinued],
            ),
            (
                reference(key(2020, 7, 1, 12, 0), InstanceClock::Zoned),
                AnswerBasis::BeforeKnownTransitions(first),
                Some(first),
                alloc::vec![DiagnosticCode::SchedulingZoneContinued],
            ),
        ];
        for (named, basis, nearest, expected) in cases {
            let mut reported = Vec::new();
            let answer = resolved(&zone, "W. Europe Standard Time", named, &mut reported);
            assert_eq!(answer.basis(), Some(basis), "{named:?}");
            assert_eq!(answer.nearest_known(), nearest, "{named:?}");
            assert_eq!(answer.is_continued(), nearest.is_some(), "{named:?}");
            assert_eq!(codes(&reported), expected, "{named:?}");
            assert_eq!(
                answer.side(),
                FoldSide::Once,
                "a continued observance is still one offset, so the clock does not repeat"
            );
        }

        let mut reported = Vec::new();
        let far = resolved(
            &zone,
            "W. Europe Standard Time",
            reference(key(2035, 6, 15, 12, 0), InstanceClock::Zoned),
            &mut reported,
        );
        assert_eq!(reported[0].severity(), Severity::Note);
        assert_eq!(reported[0].instant(), Some(key(2035, 6, 15, 12, 0)));
        assert!(
            far.is_resolved(),
            "a continuation resolves; it does not refuse"
        );
    }

    /// Agenda item 3: an exclusion no zone could place makes the series undecidable, and the
    /// caller is told before it asks the gate anything.
    #[test]
    fn an_exclusion_no_zone_could_place_makes_the_series_undecidable() {
        let zone = new_york();
        let stranger = ZonedSeries::new(&zone, "Customized Time Zone", ResolutionPolicy::DEFAULT);
        let (mut meter, mut noise) = ledger();
        let unplaceable = ResolvedExclusions::read(
            &stranger,
            ValueType::DateTime,
            &[DateTimeValue::Utc(clock(2026, 7, 1, 13, 0))],
            &mut meter,
            &mut noise,
        );
        assert_eq!(
            unplaceable.unplaced(),
            [utc(2026, 7, 1, 13, 0)],
            "a Z-terminated exclusion needs the zone the series has not got"
        );

        let mut reported = Vec::new();
        assert!(!exclusions_are_placeable(&unplaceable));
        assert!(!check_exclusions_are_placeable(
            &unplaceable,
            &mut meter,
            &mut reported
        ));
        assert_eq!(
            codes(&reported),
            [DiagnosticCode::SchedulingExclusionUnplaced]
        );
        assert_eq!(reported[0].severity(), Severity::Violation);
        assert_eq!(reported[0].instant(), Some(utc(2026, 7, 1, 13, 0)));

        let series = ZonedSeries::new(&zone, "America/New_York", ResolutionPolicy::DEFAULT);
        let placeable = ResolvedExclusions::read(
            &series,
            ValueType::DateTime,
            &[DateTimeValue::Local(clock(2026, 7, 1, 9, 0))],
            &mut meter,
            &mut noise,
        );
        let mut quiet = Vec::new();
        assert!(exclusions_are_placeable(&placeable));
        assert!(check_exclusions_are_placeable(
            &placeable, &mut meter, &mut quiet
        ));
        assert!(exclusions_are_placeable(&ResolvedExclusions::empty()));
        assert!(quiet.is_empty());
    }

    /// The composition this unit feeds, as the table the milestone asks for: prior state,
    /// incoming message and applying party reduced to what a zone actually decides — which
    /// instance each side names — and then RFC 5546's own revision rules on top.
    ///
    /// Every expectation is the RFC's text. Section 2.1.4: an older `SEQUENCE` never overwrites
    /// a newer state. Section 2.1.5: at an equal `SEQUENCE` the older `DTSTAMP` loses. Section
    /// 3.7.1 and ADR-0005 amendment 3: an instance nothing could tell from its neighbor is
    /// refused rather than guessed at, and an instance the state does not have is refused too.
    #[test]
    fn the_gate_reads_identity_before_revision_and_refuses_both_kinds_of_doubt() {
        let zone = new_york();
        let mut noise = Vec::new();
        let held = resolved(
            &zone,
            "America/New_York",
            reference(utc(2026, 11, 1, 5, 30), InstanceClock::Utc),
            &mut noise,
        );
        let other_half = resolved(
            &zone,
            "America/New_York",
            reference(utc(2026, 11, 1, 6, 30), InstanceClock::Utc),
            &mut noise,
        );
        let wall_clock_only = resolved(
            &zone,
            "America/New_York",
            reference(key(2026, 11, 1, 1, 30), InstanceClock::Zoned),
            &mut noise,
        );
        let sprang = resolved(
            &zone,
            "America/New_York",
            reference(key(2026, 3, 8, 2, 30), InstanceClock::Zoned),
            &mut noise,
        );

        let current = Revision::new(2, Some(utc(2026, 10, 1, 12, 0)));
        let same = Revision::new(2, Some(utc(2026, 10, 1, 12, 0)));
        let older_sequence = Revision::new(1, Some(utc(2026, 10, 31, 12, 0)));
        let older_stamp = Revision::new(2, Some(utc(2026, 9, 1, 12, 0)));
        let newer = Revision::new(3, None);
        let absent = Revision::read(SequenceRead::Absent, Some(utc(2026, 12, 1, 12, 0))).unwrap();
        let cases = [
            (held, same, Verdict::Act),
            (held, newer, Verdict::Act),
            (held, older_sequence, Verdict::SequenceStale),
            (held, older_stamp, Verdict::DtstampStale),
            (other_half, same, Verdict::NoMatchingInstance),
            (other_half, newer, Verdict::NoMatchingInstance),
            (sprang, newer, Verdict::NoMatchingInstance),
            (wall_clock_only, newer, Verdict::NoMatchingInstance),
            (held, absent, Verdict::SequenceStale),
        ];
        for (message, offered, expected) in cases {
            assert_eq!(
                verdict(message, held, offered, current),
                expected,
                "{:?} at {:?}",
                message.side(),
                offered.sequence()
            );
        }

        // Both sides written as wall clocks is the case a zone cannot rescue: one key, two
        // meetings, and nothing in either file says which. It stays a refusal.
        assert_eq!(
            verdict(wall_clock_only, wall_clock_only, newer, current),
            Verdict::AmbiguousInstance
        );
        assert_eq!(
            verdict(held, held, same, current),
            Verdict::Act,
            "the same half of the fold, answered on the revision it was sent"
        );
    }

    /// An absent `SEQUENCE` is zero and not "unknown", so a message carrying none is judged
    /// against what the caller holds rather than waved through.
    #[test]
    fn an_absent_sequence_is_zero_at_a_resolved_instance() {
        let zone = new_york();
        let mut noise = Vec::new();
        let here = resolved(
            &zone,
            "America/New_York",
            reference(utc(2026, 11, 1, 5, 30), InstanceClock::Utc),
            &mut noise,
        );
        let none = Revision::read(SequenceRead::Absent, None).unwrap();
        assert_eq!(none.sequence(), 0);
        assert_eq!(
            verdict(here, here, none, Revision::new(1, None)),
            Verdict::SequenceStale
        );
        assert_eq!(
            verdict(here, here, none, none),
            Verdict::Act,
            "two messages at revision zero are not stale against each other"
        );
        assert!(noise.is_empty());
    }
}
