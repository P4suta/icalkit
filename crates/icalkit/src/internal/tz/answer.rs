// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The interface a zone answer arrives through, and everything an answer carries with it.
//!
//! Specification: RFC 5545 section 3.2.19 (`TZID`), section 3.3.5 (`DATE-TIME` form 3), and
//! section 3.6.5 (`VTIMEZONE`). `docs/adr/0003` is the decision this module implements.
//!
//! Three claims are made here and nowhere else in the crate, because every unit above depends
//! on all three holding.
//!
//! **A source that does not know an identifier says so, and says nothing else.**
//! [`ZoneSource::resolve`] returns `None` for exactly one condition: this source does not
//! recognize this `TZID`. It never means "recognized, but I have no data for that year" —
//! that is [`AnswerBasis`] — nor "recognized, and I hold no transition at all", which is
//! [`LocalResolution::Undetermined`] on an answer that exists, and it never licenses an
//! implementation to invent an answer. A
//! source handed `W. Europe Standard Time` with no alias table returns `None` and lets the
//! hole stay visible, which is what stops identifier mapping from becoming a fallback chain
//! buried inside somebody's `impl`.
//!
//! The one direction that claim cannot be made in is [`ZoneSource::offset_at`], because an
//! [`OffsetAnswer`] has no way to spell "no offset" and the only number available to fill the
//! field with is UTC. So recognition is asked directly, through [`ZoneSource::recognizes`], and
//! that is what tells a `TZID` nobody supplied from a `TZID` supplied with nothing behind it.
//!
//! **An awkward local time is a value.** At a fall-back an hour repeats and a local time names
//! two instants; at a spring-forward an hour does not exist and a local time names none. Real
//! calendars contain a 02:30 meeting on the morning the clocks move. [`LocalResolution`] has a
//! variant for each and an error for neither.
//!
//! **Every answer names its source and how much of that source stood behind it.** Those are
//! two different facts. [`ZoneProvenance`] is which source answered; [`AnswerBasis`] is whether
//! the source held data covering the question or continued its last observance past the end of
//! what it knew. Agreement between two computed answers and agreement between a computed one
//! and a continued one are different facts, and a type that reported them alike would turn a
//! coincidence into confident corroboration by two independent sources.

use crate::internal::core::{CivilDate, CivilDateTime, DiagnosticCode, Instant, UtcOffset};

/// One reading of a wall clock: the instant it names, the offset that made it so, and whether
/// that offset is the zone's daylight one.
///
/// A struct rather than three fields repeated per variant of [`LocalResolution`], because a
/// fold has two of these and they must carry the same information as the single reading a
/// caller gets on an ordinary day. `daylight` is the observance's own classification —
/// `DAYLIGHT` against `STANDARD` in RFC 5545 section 3.6.5 — and not a comparison of offsets:
/// a zone whose daylight offset is the smaller of the two exists, and inferring the flag from
/// arithmetic would get it backwards there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reading {
    /// The instant the wall clock named.
    pub instant: Instant,
    /// The offset from UTC in force when it did.
    pub offset: UtcOffset,
    /// Whether the observance in force is the zone's daylight one.
    pub daylight: bool,
}

impl Reading {
    /// A reading of `instant` taken at `offset`.
    #[must_use]
    pub const fn new(instant: Instant, offset: UtcOffset, daylight: bool) -> Self {
        Self {
            instant,
            offset,
            daylight,
        }
    }
}

/// What a wall-clock time turned out to name under a zone.
///
/// The three states a local time can be in, and the whole reason this crate exists: an
/// implementation returning an error for two of them has discarded input the caller could have
/// shown a person.
///
/// Invariants every implementation of [`ZoneSource`] owes:
///
/// - [`LocalResolution::Ambiguous`] has `earlier.instant < later.instant` and
///   `earlier.offset != later.offset`.
/// - [`LocalResolution::Nonexistent`] has `gap_start < gap_end` and
///   `offset_before != offset_after`, and `shifted` is the instant the queried wall clock
///   names when it is read with `offset_before`.
///
/// `shifted` is this crate's answer to a contradiction inside RFC 5545 that a library is not
/// entitled to settle. Section 3.3.10 says a recurrence instance falling on a nonexistent
/// local time MUST be ignored; section 3.3.5 says an explicit `DATE-TIME` in a gap is read
/// with the offset in force before it, which is what Google and Apple do. The variant hands a
/// caller the material for both readings and picks neither, because deciding for the caller is
/// how one participant's meeting moves an hour and another's does not. [`GapPolicy`] is where
/// a caller states which it wants.
///
/// [`GapPolicy`]: crate::internal::tz::GapPolicy
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum LocalResolution {
    /// The wall clock names exactly one instant, which is every day of the year but two.
    Unique {
        /// The one reading.
        reading: Reading,
    },
    /// The wall clock names two instants, because the zone fell back through it.
    Ambiguous {
        /// The first of the two, under the offset in force before the transition.
        earlier: Reading,
        /// The second, under the offset in force after it.
        later: Reading,
    },
    /// The source recognizes the zone and holds no transition for it, so it names no instant.
    ///
    /// Not a fourth state of a wall clock: a wall clock under a zone is unique, repeated or
    /// missing and there is no fourth. This is a state of the *source*, and it exists because
    /// the alternative was worse. A `VTIMEZONE` carrying no usable observance is a file half the
    /// exporters in the world produce; a table built from one has no offset to report and used
    /// to say so by answering `None`, which is the one value [`ZoneSource::resolve`] reserves
    /// for "I have never heard of this identifier". Two facts a caller acts on differently —
    /// the zone is undefined, and the zone is defined and empty — arrived as one.
    ///
    /// It carries nothing, because there is nothing: no offset, no reading, no transition
    /// either side. [`LocalResolution::pick`] answers `None` under every policy, and
    /// [`DiagnosticCode::TimeZoneWithoutTransitions`] is what travels.
    Undetermined,
    /// The wall clock names no instant, because the zone sprang forward over it.
    Nonexistent {
        /// The last instant before the gap opened.
        gap_start: Instant,
        /// The first instant after it closed.
        gap_end: Instant,
        /// The offset in force before the transition.
        offset_before: UtcOffset,
        /// The offset in force after it.
        offset_after: UtcOffset,
        /// The instant the queried wall clock names read with `offset_before`.
        ///
        /// RFC 5545 section 3.3.5's reading, offered so a caller obeying it does not have to
        /// re-derive it from a second query against a source that may not agree with the first.
        shifted: Instant,
    },
}

impl LocalResolution {
    /// The one instant, present only when there is exactly one.
    ///
    /// Deliberately `None` for both awkward states. A caller that wants an instant regardless
    /// states a policy and uses [`LocalResolution::pick`], where the choice is visible.
    #[must_use]
    pub const fn unambiguous(self) -> Option<Instant> {
        match self {
            Self::Unique { reading } => Some(reading.instant),
            Self::Ambiguous { .. } | Self::Nonexistent { .. } | Self::Undetermined => None,
        }
    }

    /// The earliest instant this resolution names, `None` in a gap, which names none.
    #[must_use]
    pub const fn earliest(self) -> Option<Instant> {
        match self {
            Self::Unique { reading }
            | Self::Ambiguous {
                earlier: reading, ..
            } => Some(reading.instant),
            Self::Nonexistent { .. } | Self::Undetermined => None,
        }
    }

    /// The instant this resolution names under a stated policy for the two awkward hours.
    ///
    /// `None` only where the policy says to skip. Every collapse of three states into one
    /// instant in this workspace goes through here, so that a caller reading two call sites
    /// sees one rule rather than two conventions.
    #[must_use]
    pub const fn pick(self, gaps: GapPolicy, folds: FoldPolicy) -> Option<Instant> {
        match self {
            Self::Unique { reading } => Some(reading.instant),
            Self::Ambiguous { earlier, later } => match folds {
                FoldPolicy::Earlier => Some(earlier.instant),
                FoldPolicy::Later => Some(later.instant),
            },
            Self::Nonexistent {
                shifted, gap_end, ..
            } => match gaps {
                GapPolicy::Skip => None,
                GapPolicy::ShiftForward => Some(shifted),
                GapPolicy::ClampToTransition => Some(gap_end),
            },
            // No policy applies. A gap policy states what to do with a wall clock a zone
            // sprang over, and this is a source with no zone data at all: there is no offset
            // to read the clock with and nothing for a caller to choose between.
            Self::Undetermined => None,
        }
    }

    /// Whether the wall clock named two instants.
    #[must_use]
    pub const fn is_ambiguous(self) -> bool {
        matches!(self, Self::Ambiguous { .. })
    }

    /// Whether the wall clock named none.
    #[must_use]
    pub const fn is_nonexistent(self) -> bool {
        matches!(self, Self::Nonexistent { .. })
    }

    /// The code an emitter reports this resolution under, absent for the ordinary case.
    ///
    /// The mapping lives here rather than at each emission site so that two units cannot
    /// disagree about which state carries which code. Where a diagnostic is emitted, and
    /// against which meter, stays the emitter's business.
    #[must_use]
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode> {
        match self {
            Self::Unique { .. } => None,
            Self::Ambiguous { .. } => Some(DiagnosticCode::AmbiguousLocalTime),
            Self::Nonexistent { .. } => Some(DiagnosticCode::NonexistentLocalTime),
            Self::Undetermined => Some(DiagnosticCode::TimeZoneWithoutTransitions),
        }
    }

    /// Whether the source had no transition to answer with at all.
    #[must_use]
    pub const fn is_undetermined(self) -> bool {
        matches!(self, Self::Undetermined)
    }
}

/// What a caller wants done with a wall-clock time the zone sprang over.
///
/// RFC 5545 says two things about this and they do not agree, so the crate states both and
/// makes the caller choose; see [`LocalResolution`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum GapPolicy {
    /// Drop the value. RFC 5545 section 3.3.10's MUST for a recurrence instance.
    #[default]
    Skip,
    /// Read it with the offset in force before the gap. RFC 5545 section 3.3.5's reading, and
    /// what Google and Apple are reported to do with an explicit `DATE-TIME`.
    ShiftForward,
    /// Move it to the instant the gap closed, so the event happens as soon as it can.
    ///
    /// Neither section's reading, and offered because it is what a scheduling caller often
    /// wants and the alternative is that caller reimplementing it from `gap_end` at every call
    /// site. Named rather than defaulted.
    ClampToTransition,
}

/// Which instant a caller wants from a wall-clock time the zone fell back through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FoldPolicy {
    /// The first of the two, which is what a calendar shows.
    #[default]
    Earlier,
    /// The second of the two.
    Later,
}

/// Which source produced an answer.
///
/// `docs/adr/0003`'s "every result says which source produced it", as a value rather than as a
/// convention. It is an ordinary enum with no payload because a caller wiring two sources
/// already holds both and needs only to know which one spoke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ZoneProvenance {
    /// A `VTIMEZONE` carried by the calendar that is being read.
    EmbeddedVtimezone,
    /// A zone database the caller wired in.
    CallerDatabase,
    /// A fixed offset, which is not a zone and cannot say when a transition happens.
    FixedOffset,
}

/// How much of a source's data actually stood behind an answer.
///
/// `docs/adr/0003`'s third field. A `VTIMEZONE` whose transitions are three `RDATE` lines
/// through 2029, asked about 2035, has no data for 2035; continuing its last observance is the
/// defensible thing for it to do and a dishonest thing to do quietly. This is what keeps that
/// answer distinguishable from one a rule computed, and it is what lets a caller tell
/// agreement between two computed answers from agreement between a computed one and a
/// continued one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AnswerBasis {
    /// The source held a transition at or before the question and data covering it.
    Computed,
    /// The question lies past the last transition the source knows, whose date this carries.
    ///
    /// The answer continues the final observance recorded at that date. How far past is the
    /// caller's own arithmetic: a source that continued one day and one that continued six
    /// years are both this variant, and the date is what tells them apart.
    BeyondKnownTransitions(CivilDate),
    /// The question lies before the first transition the source knows, whose date this carries.
    ///
    /// A table has two ends and this milestone found the claim stated for one of them. A
    /// `VTIMEZONE` whose `RDATE` lines run from 2027 answers July 2020 by extending its
    /// earliest observance's `TZOFFSETFROM` backwards forever, which is the whole of what the
    /// file states about that era and is very often wrong — `America/New_York` was on `-04:00`
    /// that July and such a table says `-05:00`. Continuing backwards is still the defensible
    /// thing to do; doing it in a value indistinguishable from a computed answer is not.
    BeforeKnownTransitions(CivilDate),
}

impl AnswerBasis {
    /// Whether the answer continued past the end of what its source knows.
    #[must_use]
    pub const fn is_beyond_known_transitions(self) -> bool {
        matches!(self, Self::BeyondKnownTransitions(_))
    }

    /// Whether the answer reached back before the first transition its source knows.
    #[must_use]
    pub const fn is_before_known_transitions(self) -> bool {
        matches!(self, Self::BeforeKnownTransitions(_))
    }

    /// Whether the source held data covering the question at all.
    #[must_use]
    pub const fn is_computed(self) -> bool {
        matches!(self, Self::Computed)
    }

    /// The nearest date the source has real data for, absent when it had data for the question.
    ///
    /// The last such date for [`AnswerBasis::BeyondKnownTransitions`] and the first for
    /// [`AnswerBasis::BeforeKnownTransitions`]; in both cases the edge of the source's
    /// knowledge that the answer was continued from.
    #[must_use]
    pub const fn nearest_known(self) -> Option<CivilDate> {
        match self {
            Self::Computed => None,
            Self::BeyondKnownTransitions(date) | Self::BeforeKnownTransitions(date) => Some(date),
        }
    }

    /// The code an emitter reports this basis under, absent when the source had the data.
    #[must_use]
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode> {
        match self {
            Self::Computed => None,
            Self::BeyondKnownTransitions(_) => Some(DiagnosticCode::TimeZoneCoverageExhausted),
            Self::BeforeKnownTransitions(_) => Some(DiagnosticCode::TimeZoneBeforeKnownTransitions),
        }
    }
}

/// What a wall clock named, who says so, and how much they knew.
///
/// Public fields and not `#[non_exhaustive]`, because callers implement [`ZoneSource`] and
/// have to be able to construct one. The three enums it holds are `#[non_exhaustive]`
/// instead, so a new source kind, a new basis or a new resolution state is not a breaking
/// change for the caller that matches on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZoneAnswer {
    /// What the wall clock named.
    pub resolution: LocalResolution,
    /// Which source said so.
    pub source: ZoneProvenance,
    /// How much of that source's data stood behind it.
    pub basis: AnswerBasis,
}

impl ZoneAnswer {
    /// An answer of `resolution` from `source` on `basis`.
    #[must_use]
    pub const fn new(
        resolution: LocalResolution,
        source: ZoneProvenance,
        basis: AnswerBasis,
    ) -> Self {
        Self {
            resolution,
            source,
            basis,
        }
    }
}

/// What offset a zone was running at an instant, who says so, and how much they knew.
///
/// The other direction from [`ZoneAnswer`], and the one a caller needs to turn an instant back
/// into a wall clock. It has no ambiguity to represent: every instant has exactly one offset
/// under a zone, which is precisely the asymmetry that makes the local direction hard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OffsetAnswer {
    /// The offset in force.
    pub offset: UtcOffset,
    /// Whether the observance in force is the zone's daylight one.
    pub daylight: bool,
    /// Which source said so.
    pub source: ZoneProvenance,
    /// How much of that source's data stood behind it.
    pub basis: AnswerBasis,
}

impl OffsetAnswer {
    /// An answer of `offset` from `source` on `basis`.
    #[must_use]
    pub const fn new(
        offset: UtcOffset,
        daylight: bool,
        source: ZoneProvenance,
        basis: AnswerBasis,
    ) -> Self {
        Self {
            offset,
            daylight,
            source,
            basis,
        }
    }

    /// Whether two answers agree about the zone, ignoring who said so and how they knew.
    ///
    /// The comparison [`PolicyOutcome`] is formed on. Provenance and basis are deliberately
    /// excluded: two sources that agree about the offset have agreed, and the fact that one of
    /// them was extrapolating is the *other* thing the caller is told.
    #[must_use]
    pub const fn agrees_with(self, other: Self) -> bool {
        self.offset.seconds() == other.offset.seconds() && self.daylight == other.daylight
    }
}

/// Where a zone answer comes from.
///
/// Object-safe by construction — `&self` in, an owned answer out, no generic parameter and no
/// associated type — because combining an embedded `VTIMEZONE` with a database the caller
/// already has is a runtime wiring choice made once, not a compile-time one. `docs/adr/0003`
/// fixes that, and the `dyn`-compatible shape is asserted by a test in this module rather than
/// left as a claim.
///
/// No `Send` or `Sync` bound. A server whose concrete source is both still gets
/// `Arc<dyn ZoneSource + Send + Sync>` for free, and a caller on a target where a vtable hop
/// per lookup matters holds the concrete type instead: the trait permits `dyn`, it does not
/// mandate it.
///
/// No meter and no sink either, and that is the deliberate half of the signature. The bounded
/// work is admitting transitions into a table, which happens once where untrusted input is
/// read; a lookup against a table already built is a binary search and a closed-form rule
/// evaluation, so there is nothing here for a budget to refuse. An implementation that would
/// need one is doing work on this path that does not belong on it. Diagnostics about an answer
/// are emitted by whoever consumes it, from [`LocalResolution::diagnostic_code`] and
/// [`AnswerBasis::diagnostic_code`], which is also what keeps this trait implementable by a
/// caller who has never heard of a `Meter`.
pub trait ZoneSource {
    /// What `local` names under the zone `tzid` identifies, or `None` if that is not a zone
    /// this source knows.
    ///
    /// `None` means unrecognized and nothing else. "Recognized but out of data" is
    /// [`AnswerBasis::BeyondKnownTransitions`] on an answer that exists.
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer>;

    /// What offset the zone `tzid` identifies was running at `instant`, or `None` if that is
    /// not a zone this source knows.
    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer>;

    /// Whether this source recognizes `tzid` at all, whatever it can say about any instant.
    ///
    /// The question [`ZoneSource::offset_at`] cannot answer and [`ZoneSource::resolve`] answers
    /// only in one direction. Every instant has exactly one offset under a zone, so an
    /// [`OffsetAnswer`] has nowhere to record "recognized, and I hold nothing" — the offset
    /// field would have to be filled with a number, and the only candidate is UTC, which is the
    /// invention `docs/adr/0003` refuses. So the recognition is asked directly.
    ///
    /// The provided implementation asks the two answering methods and takes either of them
    /// speaking as recognition, which is right for every source whose `None` means what the
    /// trait says it means. A source with an identifier table overrides it with a lookup, which
    /// is cheaper and is what [`TransitionTable`] does.
    ///
    /// [`TransitionTable`]: crate::internal::tz::TransitionTable
    fn recognizes(&self, tzid: &str) -> bool {
        if self.offset_at(tzid, Instant::EPOCH).is_some() {
            return true;
        }
        CivilDateTime::from_instant(Instant::EPOCH, UtcOffset::UTC)
            .is_some_and(|clock| self.resolve(tzid, clock).is_some())
    }
}

impl<S: ZoneSource + ?Sized> ZoneSource for &S {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        (**self).resolve(tzid, local)
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        (**self).offset_at(tzid, instant)
    }

    fn recognizes(&self, tzid: &str) -> bool {
        (**self).recognizes(tzid)
    }
}

/// What two sources said, with nothing collapsed.
///
/// `docs/adr/0003`'s policy type. Both sources are queried on every call, unconditionally,
/// before an outcome is formed: there is no short circuit and no operand order that skips
/// work, because a fallback chain is exactly what the ADR refuses.
///
/// [`PolicyOutcome::Agreed`] keeps both answers rather than one. The ADR's own argument for
/// carrying a basis requires it — agreement between two computed answers is a different fact
/// from agreement between a computed one and a continued one, and collapsing the pair throws
/// away the asymmetry the field exists to surface. The `Only` variants mean the other source
/// returned `None`, that is, did not recognize the identifier; they never mean it disagreed.
///
/// The type parameter defaults to [`ZoneAnswer`] so the same five variants serve
/// [`ZoneSource::offset_at`] with [`OffsetAnswer`] in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum PolicyOutcome<A = ZoneAnswer> {
    /// Both sources answered and their answers matched.
    Agreed {
        /// What the embedded source said.
        embedded: A,
        /// What the fallback source said.
        fallback: A,
    },
    /// Both sources answered and their answers did not match.
    Disagreed {
        /// What the embedded source said.
        embedded: A,
        /// What the fallback source said.
        fallback: A,
    },
    /// Only the embedded source recognized the identifier.
    OnlyEmbedded(A),
    /// Only the fallback source recognized the identifier.
    OnlyFallback(A),
    /// Neither source recognized the identifier.
    Neither,
    /// A source recognizes the identifier and neither had an answer to this question.
    ///
    /// The distinction [`PolicyOutcome::Neither`] used to swallow. A calendar declaring
    /// `Europe/Berlin` with no observance supplies the identifier and no data, so reporting
    /// `unknown-time-zone` about it — "a `TZID` named a zone no supplied source could resolve"
    /// — states something false about the file at [`Severity::Violation`]. What is wrong with
    /// such a file is `vtimezone-without-observance`, which whoever read it already reported.
    ///
    /// [`Severity::Violation`]: crate::internal::core::Severity::Violation
    Undetermined,
}

impl<A: Copy> PolicyOutcome<A> {
    /// The answer a caller would act on, preferring the embedded source where both spoke.
    ///
    /// A convenience with its preference written on it, and deliberately not the only way to
    /// read the outcome: a caller that wants to warn a user about a disagreement matches on
    /// the variant instead. `None` only for [`PolicyOutcome::Neither`].
    #[must_use]
    pub const fn embedded_first(self) -> Option<A> {
        match self {
            Self::Agreed { embedded, .. }
            | Self::Disagreed { embedded, .. }
            | Self::OnlyEmbedded(embedded) => Some(embedded),
            Self::OnlyFallback(answer) => Some(answer),
            Self::Neither | Self::Undetermined => None,
        }
    }

    /// Whether the two sources answered and did not agree.
    #[must_use]
    pub const fn is_disagreement(self) -> bool {
        matches!(self, Self::Disagreed { .. })
    }

    /// The code an emitter reports this outcome under, absent unless the sources disagreed.
    #[must_use]
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode> {
        match self {
            Self::Disagreed { .. } => Some(DiagnosticCode::TimeZoneSourceDisagreement),
            Self::Agreed { .. }
            | Self::OnlyEmbedded(_)
            | Self::OnlyFallback(_)
            | Self::Neither
            | Self::Undetermined => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::internal::core::{
        CivilDate, CivilDateTime, CivilTime, DiagnosticCode, Instant, UtcOffset,
    };

    use super::{
        AnswerBasis, FoldPolicy, GapPolicy, LocalResolution, OffsetAnswer, PolicyOutcome, Reading,
        ZoneAnswer, ZoneProvenance, ZoneSource,
    };

    /// A source that answers one identifier with one offset and knows nothing else.
    ///
    /// Written here rather than reached for from the unit that owns the real fixed-offset
    /// source, because what is under test is the trait's shape and a test that depended on
    /// another unit's file would be testing that unit too.
    #[derive(Debug)]
    struct OneZone {
        /// The identifier this source recognizes, compared by exact bytes.
        tzid: &'static str,
        /// The offset it reports.
        offset: UtcOffset,
    }

    impl ZoneSource for OneZone {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            let instant = (tzid == self.tzid).then(|| local.at_offset(self.offset))??;
            let reading = Reading::new(instant, self.offset, false);
            Some(ZoneAnswer::new(
                LocalResolution::Unique { reading },
                ZoneProvenance::CallerDatabase,
                AnswerBasis::Computed,
            ))
        }

        fn offset_at(&self, tzid: &str, _instant: Instant) -> Option<OffsetAnswer> {
            (tzid == self.tzid).then(|| {
                OffsetAnswer::new(
                    self.offset,
                    false,
                    ZoneProvenance::CallerDatabase,
                    AnswerBasis::Computed,
                )
            })
        }
    }

    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap()
    }

    fn noon() -> CivilDateTime {
        let date = CivilDate::from_ymd(2026, 8, 10).unwrap();
        CivilDateTime::new(date, CivilTime::from_hms(12, 0, 0).unwrap())
    }

    /// The claim the whole wiring story rests on: a source is usable behind a trait object.
    ///
    /// `docs/adr/0003` says combining an embedded `VTIMEZONE` with the caller's database is a
    /// runtime choice, which is only true while this compiles.
    #[test]
    fn a_source_is_usable_behind_a_trait_object() {
        let concrete = OneZone {
            tzid: "Europe/Berlin",
            offset: offset(7200),
        };
        let source: &dyn ZoneSource = &concrete;
        assert!(source.resolve("Europe/Berlin", noon()).is_some());
        assert_eq!(
            source.resolve("W. Europe Standard Time", noon()),
            None,
            "an identifier the source does not know is None and never a default"
        );
        assert!(source.offset_at("Europe/Berlin", at(0)).is_some());
    }

    /// Neither awkward state collapses into an instant by itself.
    #[test]
    fn an_awkward_hour_yields_no_instant_until_a_policy_says_which() {
        let earlier = Reading::new(at(100), offset(7200), true);
        let later = Reading::new(at(3700), offset(3600), false);
        let fold = LocalResolution::Ambiguous { earlier, later };
        assert_eq!(fold.unambiguous(), None);
        assert_eq!(fold.earliest(), Some(at(100)));
        assert_eq!(
            fold.pick(GapPolicy::Skip, FoldPolicy::Earlier),
            Some(at(100))
        );
        assert_eq!(
            fold.pick(GapPolicy::Skip, FoldPolicy::Later),
            Some(at(3700))
        );

        let gap = LocalResolution::Nonexistent {
            gap_start: at(100),
            gap_end: at(3700),
            offset_before: offset(3600),
            offset_after: offset(7200),
            shifted: at(3800),
        };
        assert_eq!(gap.earliest(), None);
        assert_eq!(gap.pick(GapPolicy::Skip, FoldPolicy::Earlier), None);
        assert_eq!(
            gap.pick(GapPolicy::ShiftForward, FoldPolicy::Earlier),
            Some(at(3800)),
            "RFC 5545 section 3.3.5 reads a gap with the offset before it"
        );
        assert_eq!(
            gap.pick(GapPolicy::ClampToTransition, FoldPolicy::Earlier),
            Some(at(3700))
        );
    }

    /// One state, one code, decided once so two emitters cannot pick differently.
    #[test]
    fn each_state_carries_the_code_it_is_reported_under() {
        let reading = Reading::new(at(0), UtcOffset::UTC, false);
        let ordinary = LocalResolution::Unique { reading };
        assert_eq!(ordinary.diagnostic_code(), None);
        let fold = LocalResolution::Ambiguous {
            earlier: reading,
            later: Reading::new(at(3600), UtcOffset::UTC, false),
        };
        assert_eq!(
            fold.diagnostic_code(),
            Some(DiagnosticCode::AmbiguousLocalTime)
        );
        assert_eq!(AnswerBasis::Computed.diagnostic_code(), None);
        let ran_out =
            AnswerBasis::BeyondKnownTransitions(CivilDate::from_ymd(2029, 12, 31).unwrap());
        assert_eq!(
            ran_out.diagnostic_code(),
            Some(DiagnosticCode::TimeZoneCoverageExhausted)
        );
        assert!(ran_out.is_beyond_known_transitions());
    }

    /// Two offsets agree or they do not; who said so and how much they knew is the other fact.
    #[test]
    fn agreement_is_about_the_zone_and_not_about_who_answered() {
        let known = CivilDate::from_ymd(2029, 12, 31).unwrap();
        let embedded = OffsetAnswer::new(
            offset(3600),
            false,
            ZoneProvenance::EmbeddedVtimezone,
            AnswerBasis::BeyondKnownTransitions(known),
        );
        let fallback = OffsetAnswer::new(
            offset(3600),
            false,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        );
        assert!(embedded.agrees_with(fallback));
        let moved = OffsetAnswer::new(
            offset(7200),
            true,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        );
        assert!(!embedded.agrees_with(moved));

        let agreed = PolicyOutcome::Agreed { embedded, fallback };
        assert_eq!(agreed.diagnostic_code(), None);
        assert_eq!(agreed.embedded_first(), Some(embedded));
        let split = PolicyOutcome::Disagreed {
            embedded,
            fallback: moved,
        };
        assert!(split.is_disagreement());
        assert_eq!(
            split.diagnostic_code(),
            Some(DiagnosticCode::TimeZoneSourceDisagreement)
        );
        let nobody: PolicyOutcome<OffsetAnswer> = PolicyOutcome::Neither;
        assert_eq!(nobody.embedded_first(), None);
    }
}
