// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 3 — a fixed offset as a source, and two sources with the disagreement kept.
//!
//! `docs/adr/0003` is what this unit implements literally: where the embedded `VTIMEZONE` and
//! the caller's database disagree, that is reported rather than silently settled.
//!
//! The two types are declared here with their constructors and accessors, so the crate root
//! needs no edit when the behavior lands. What this unit adds to them, and no other unit does:
//!
//! ```text
//! impl ZoneSource for FixedOffsetSource { .. }
//! impl<'a, E: ZoneSource + ?Sized, F: ZoneSource + ?Sized> CombinedZoneSource<'a, E, F> {
//!     pub fn resolve(&self, tzid: &str, local: CivilDateTime) -> PolicyOutcome<ZoneAnswer>;
//!     pub fn offset_at(&self, tzid: &str, instant: Instant) -> PolicyOutcome<OffsetAnswer>;
//!     pub fn report<D: DiagnosticSink + ?Sized>(
//!         &self, outcome: PolicyOutcome<OffsetAnswer>, at: Instant,
//!         meter: &mut Meter, sink: &mut D,
//!     );
//! }
//! ```
//!
//! Both sources are queried on every call, unconditionally. No short circuit, no operand order
//! that skips work: a fallback chain is the thing the ADR refuses, and a combinator that
//! stopped early would reintroduce one under a different name. The cost is two lookups where a
//! caller wanting one uses one source directly, which is the correct trade and not a hidden one.
//!
//! [`CombinedZoneSource`] deliberately does **not** implement [`ZoneSource`]. Implementing it
//! would force the type to collapse a disagreement into a single answer somewhere inside
//! itself, and that decision belongs to whoever is going to show it to a person.
//!
//! Codes this unit owns, and no other unit may emit: `time-zone-source-disagreement`, from
//! [`PolicyOutcome::diagnostic_code`], and `unknown-time-zone` for [`PolicyOutcome::Neither`],
//! both against the caller's meter and sink rather than inside the trait. `Neither` is now
//! narrower than it was, and that is what M2 corrected here: a calendar declaring a zone with
//! no observance supplies the identifier and no data, and reporting it as a `TZID` nobody
//! supplied is a violation-level claim about a file that plainly wrote it down. Where either
//! source recognizes the identifier the outcome is [`PolicyOutcome::Undetermined`] and this
//! unit says nothing, because what is wrong with such a file was already reported by whoever
//! read it. The third fact an
//! outcome carries — that one side answered past the end of what it knows — travels on
//! [`AnswerBasis`] and is reported by whoever consumes the answer, for the reason
//! `answer.rs` gives: a source implementable by a caller who has never heard of a `Meter`
//! cannot also be the thing that charges one.
//!
//! [`PolicyOutcome::diagnostic_code`]: crate::PolicyOutcome::diagnostic_code
//! [`PolicyOutcome::Neither`]: crate::PolicyOutcome::Neither

use core::fmt::{self, Debug, Formatter};

use ical_core::{
    CivilDateTime, Diagnostic, DiagnosticCode, DiagnosticSink, Instant, Meter, Severity, UtcOffset,
    report_diagnostic,
};

use crate::answer::{
    AnswerBasis, LocalResolution, OffsetAnswer, PolicyOutcome, Reading, ZoneAnswer, ZoneProvenance,
    ZoneSource,
};

/// One identifier answered with one offset, forever.
///
/// Not a time zone, and the type says so: a fixed offset cannot state when a transition
/// happens, so it can never produce an ambiguous or a nonexistent reading and nothing above may
/// take it as a substitute for a zone whose definition is missing. What it is for is a
/// `Z`-terminated value and a caller that genuinely has one offset, expressible with the
/// `vtimezone` feature switched off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FixedOffsetSource {
    /// The identifier this source answers to, compared by exact bytes.
    tzid: &'static str,
    /// The offset it reports for every instant.
    offset: UtcOffset,
    /// Whether it reports that offset as a daylight one.
    daylight: bool,
}

impl FixedOffsetSource {
    /// A source answering `tzid` with `offset`.
    #[must_use]
    pub const fn new(tzid: &'static str, offset: UtcOffset, daylight: bool) -> Self {
        Self {
            tzid,
            offset,
            daylight,
        }
    }

    /// The identifier this source answers to.
    #[must_use]
    pub const fn tzid(self) -> &'static str {
        self.tzid
    }

    /// The offset it reports.
    #[must_use]
    pub const fn offset(self) -> UtcOffset {
        self.offset
    }

    /// Whether it reports that offset as a daylight one.
    #[must_use]
    pub const fn daylight(self) -> bool {
        self.daylight
    }
}

impl ZoneSource for FixedOffsetSource {
    /// The one instant the wall clock names under the one offset.
    ///
    /// Always [`LocalResolution::Unique`], always [`ZoneProvenance::FixedOffset`], always
    /// [`AnswerBasis::Computed`]. A source holding a single offset has no transition for a
    /// local time to fall either side of, so it can neither name two instants nor fail to name
    /// one, and it has no table to run out of. That is precisely why it is not a stand-in for
    /// a zone whose definition is missing: it answers every question confidently, including
    /// the two an honest zone answers awkwardly.
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        // Compared by exact bytes. Folding case or stripping a vendor prefix here is the
        // identifier aliasing `docs/adr/0003` keeps as the caller's own visible step, and a
        // source that quietly did it would be the buried fallback chain that ADR refuses.
        if tzid != self.tzid {
            return None;
        }
        // `None` from `at_offset` is unreachable for values that exist — every `CivilDate` has
        // a day count and no offset under a day can push the sum off an `i64` — and is checked
        // anyway, for the reason `ical-core` gives at every other such site: a bound that
        // holds today is not a bound the compiler checks.
        let instant = local.at_offset(self.offset)?;
        let reading = Reading::new(instant, self.offset, self.daylight);
        Some(ZoneAnswer::new(
            LocalResolution::Unique { reading },
            ZoneProvenance::FixedOffset,
            AnswerBasis::Computed,
        ))
    }

    /// The one offset, whatever the instant.
    fn offset_at(&self, tzid: &str, _instant: Instant) -> Option<OffsetAnswer> {
        if tzid != self.tzid {
            return None;
        }
        Some(OffsetAnswer::new(
            self.offset,
            self.daylight,
            ZoneProvenance::FixedOffset,
            AnswerBasis::Computed,
        ))
    }
}

/// Two sources queried together, with neither preferred and nothing collapsed.
///
/// Generic and dyn-capable rather than dyn-mandating: it monomorphizes over two concrete
/// sources and accepts two `&dyn ZoneSource` unchanged, which is what the `?Sized` bounds buy.
pub struct CombinedZoneSource<'a, E: ?Sized, F: ?Sized> {
    /// The calendar's own definitions.
    embedded: &'a E,
    /// The database the caller wired in.
    fallback: &'a F,
}

impl<'a, E: ZoneSource + ?Sized, F: ZoneSource + ?Sized> CombinedZoneSource<'a, E, F> {
    /// The pair, in the order their names are reported under.
    #[must_use]
    pub const fn new(embedded: &'a E, fallback: &'a F) -> Self {
        Self { embedded, fallback }
    }

    /// The calendar's own definitions.
    #[must_use]
    pub const fn embedded(&self) -> &'a E {
        self.embedded
    }

    /// The database the caller wired in.
    #[must_use]
    pub const fn fallback(&self) -> &'a F {
        self.fallback
    }

    /// What both sources say the wall clock `local` names under `tzid`.
    ///
    /// Both are asked before anything is compared, and the two bindings below are the whole
    /// of that guarantee: there is no `or_else`, no `?` between them and no arm that reaches
    /// the second source only when the first said nothing.
    ///
    /// Agreement is equality of the two [`LocalResolution`]s and nothing else. Two sources
    /// that name the same instants under the same offsets have agreed, and which of them was
    /// continuing its last observance is the *other* fact the outcome carries — on each
    /// answer's own [`AnswerBasis`], which [`PolicyOutcome::Agreed`] keeps both of rather than
    /// collapsing to one. Provenance is excluded from the comparison for the same reason
    /// [`OffsetAnswer::agrees_with`] excludes it: who spoke is not what was said.
    #[must_use]
    pub fn resolve(&self, tzid: &str, local: CivilDateTime) -> PolicyOutcome<ZoneAnswer> {
        let embedded = self.embedded.resolve(tzid, local);
        let fallback = self.fallback.resolve(tzid, local).map(named_by_its_role);
        outcome(
            embedded,
            fallback,
            || self.recognized(tzid),
            |one, other| one.resolution == other.resolution,
        )
    }

    /// Whether either source recognizes `tzid` at all.
    ///
    /// Asked only where both sources answered nothing, and asked of both for the reason every
    /// other question here is: a source that recognizes an identifier it cannot answer about is
    /// not a source that was skipped.
    fn recognized(&self, tzid: &str) -> bool {
        let embedded = self.embedded.recognizes(tzid);
        let fallback = self.fallback.recognizes(tzid);
        embedded || fallback
    }

    /// What both sources say the zone `tzid` was running at `instant`.
    ///
    /// The other direction, formed the same way and with the same refusal to short-circuit.
    /// Agreement here is [`OffsetAnswer::agrees_with`], which is the offset and the daylight
    /// flag together: a zone whose daylight offset is the smaller of its two exists, so a
    /// comparison of offsets alone would call two different observances the same answer.
    #[must_use]
    pub fn offset_at(&self, tzid: &str, instant: Instant) -> PolicyOutcome<OffsetAnswer> {
        let embedded = self.embedded.offset_at(tzid, instant);
        let fallback = self
            .fallback
            .offset_at(tzid, instant)
            .map(offset_named_by_its_role);
        outcome(
            embedded,
            fallback,
            || self.recognized(tzid),
            OffsetAnswer::agrees_with,
        )
    }

    /// Put what the pair found on the caller's sink, charging a refusal to the caller's meter.
    ///
    /// Two codes and no others. `time-zone-source-disagreement` is a note, because two
    /// defensible readings of one instant is not a defect in the file; `unknown-time-zone` is
    /// a violation, because a `TZID` nobody supplied a definition for is one. Everything else
    /// an outcome carries — an answer that continued past the end of its table, an awkward
    /// local time — belongs to the unit that consumes the answer, and emitting it here as well
    /// would put one code on two emitters, which is the thing the golden list exists to stop.
    ///
    /// Separate from [`CombinedZoneSource::offset_at`] rather than folded into it, because a
    /// caller resolving a thousand occurrences of one series against one zone wants one
    /// diagnostic and not a thousand, and only the caller knows where that line is.
    pub fn report<D: DiagnosticSink + ?Sized>(
        &self,
        outcome: PolicyOutcome<OffsetAnswer>,
        at: Instant,
        meter: &mut Meter,
        sink: &mut D,
    ) {
        // The pair is bound and not read. A `Diagnostic` carries a code, a severity and an
        // instant, and has no field naming which sources were asked, so `embedded` and
        // `fallback` have nothing to put in one. Reporting stays a method all the same: it is
        // the second half of `offset_at`, and a caller should not have to change call syntax
        // halfway through the sentence.
        let Self { .. } = self;
        // The code comes from the outcome rather than from a literal here, so that a second
        // emitter cannot pick a different one for the same state.
        if let Some(code) = outcome.diagnostic_code() {
            report_diagnostic(
                &mut *sink,
                &mut *meter,
                Diagnostic::at_instant(code, Severity::Note, at),
            );
        }
        // The one outcome with no code of its own, because there is no reading to note: both
        // sources were asked and neither recognized the identifier. What is wrong is the
        // calendar's `TZID`, or the wiring that was supposed to answer it.
        //
        // `PolicyOutcome::Undetermined` is deliberately not this. There the identifier *was*
        // supplied and the data behind it was not, which is a different claim and one whoever
        // read the definition has already reported under `vtimezone-without-observance`.
        if matches!(outcome, PolicyOutcome::Neither) {
            report_diagnostic(
                &mut *sink,
                &mut *meter,
                Diagnostic::at_instant(DiagnosticCode::UnknownTimeZone, Severity::Violation, at),
            );
        }
    }
}

/// The five outcomes, over two answers already in hand and a statement of what agreement means
/// for them.
///
/// Written once because both directions form an outcome identically and differ only in that
/// statement, and two spellings of one five-armed match are two chances to spell it
/// differently. Taking the answers as `Option`s rather than as sources is deliberate: by the
/// time this is called both lookups have happened, so there is nothing left here that *could*
/// short-circuit, whatever a later edit does to the arms.
fn outcome<A: Copy>(
    embedded: Option<A>,
    fallback: Option<A>,
    recognized: impl FnOnce() -> bool,
    agrees: fn(A, A) -> bool,
) -> PolicyOutcome<A> {
    match (embedded, fallback) {
        (Some(embedded), Some(fallback)) => {
            if agrees(embedded, fallback) {
                PolicyOutcome::Agreed { embedded, fallback }
            } else {
                PolicyOutcome::Disagreed { embedded, fallback }
            }
        },
        // A `None` is "this source does not know this identifier" and never "this source
        // disagreed", which is why these two arms exist rather than a disagreement with a
        // missing half.
        (Some(embedded), None) => PolicyOutcome::OnlyEmbedded(embedded),
        (None, Some(fallback)) => PolicyOutcome::OnlyFallback(fallback),
        // Nobody answered, which is two facts and not one. A file declaring a zone with no
        // observance supplies the identifier and nothing behind it, and reporting that as a
        // `TZID` nobody supplied is a violation-level claim about a file that plainly wrote it.
        // The recognition question is asked here and only here, after both lookups have
        // happened, so it can add no fallback chain to a pair that already asked both.
        (None, None) if recognized() => PolicyOutcome::Undetermined,
        (None, None) => PolicyOutcome::Neither,
    }
}

/// The answer a source in the fallback role gives, named by the role it was wired into.
///
/// `docs/adr/0003` calls the second source "the database the caller wired in", and a caller's
/// database of zones is very often a set of `VTIMEZONE` definitions — that is what RFC 7808's
/// time zone service distributes and what a CalDAV server stores. A [`TransitionTable`] cannot
/// know which of the two roles it was handed to, so it names the only source a `VTIMEZONE` can
/// come from and this is where the wiring corrects it. Without the correction a disagreement
/// between a file and a database had both halves naming the file.
///
/// Only that one provenance is rewritten. A [`FixedOffsetSource`] in the fallback role is still
/// [`ZoneProvenance::FixedOffset`], because "this is not a zone at all" is a fact about the
/// source rather than about its role, and a hand-written source already says which it is.
///
/// [`TransitionTable`]: crate::TransitionTable
fn named_by_its_role(answer: ZoneAnswer) -> ZoneAnswer {
    match answer.source {
        ZoneProvenance::EmbeddedVtimezone => ZoneAnswer::new(
            answer.resolution,
            ZoneProvenance::CallerDatabase,
            answer.basis,
        ),
        _ => answer,
    }
}

/// [`named_by_its_role`] for the other direction's answer.
fn offset_named_by_its_role(answer: OffsetAnswer) -> OffsetAnswer {
    match answer.source {
        ZoneProvenance::EmbeddedVtimezone => OffsetAnswer::new(
            answer.offset,
            answer.daylight,
            ZoneProvenance::CallerDatabase,
            answer.basis,
        ),
        _ => answer,
    }
}

impl<E: ?Sized, F: ?Sized> Debug for CombinedZoneSource<'_, E, F> {
    /// Written by hand rather than derived, because a derived implementation would hold `E` and
    /// `F` to `Debug` and the whole point of the type is that either may be `dyn ZoneSource`.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("CombinedZoneSource { embedded, fallback }")
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::cell::Cell;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, IgnoreDiagnostics,
        Instant, Limits, Meter, Severity, UtcOffset,
    };

    use super::{CombinedZoneSource, FixedOffsetSource};
    use crate::answer::{
        AnswerBasis, LocalResolution, OffsetAnswer, PolicyOutcome, Reading, ZoneAnswer,
        ZoneProvenance, ZoneSource,
    };

    /// The last Sunday in March and the last Sunday in October, per year, which is the rule
    /// the European Union writes and the day the tz database lands it on.
    ///
    /// Spelled out rather than computed, because a table this file derived from a rule this
    /// file also implements would check the derivation and not the rule.
    const BERLIN_DAYS: [(u16, u8, u8); 10] = [
        (2026, 29, 25),
        (2027, 28, 31),
        (2028, 26, 29),
        (2029, 25, 28),
        (2030, 31, 27),
        (2031, 30, 26),
        (2032, 28, 31),
        (2033, 27, 30),
        (2034, 26, 29),
        (2035, 25, 28),
    ];

    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap()
    }

    /// A wall clock, in the fields a `DATE-TIME` writes one.
    fn stamp(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> CivilDateTime {
        CivilDateTime::new(
            CivilDate::from_ymd(year, month, day).unwrap(),
            CivilTime::from_hms(hour, minute, 0).unwrap(),
        )
    }

    /// The instant a clock running `running` from UTC shows `local` at.
    fn at(local: CivilDateTime, running: UtcOffset) -> Instant {
        local.at_offset(running).unwrap()
    }

    /// One transition, as a zone database records one.
    #[derive(Clone, Copy, Debug)]
    struct Shift {
        /// The UTC instant the clocks moved.
        moment: Instant,
        /// The offset they left.
        vacated: UtcOffset,
        /// The offset they took.
        adopted: UtcOffset,
        /// Whether the offset they took is the zone's daylight one.
        daylight: bool,
    }

    /// A transition at `utc`, written as the UTC wall clock a database publishes it at.
    fn shift(utc: CivilDateTime, vacated: i32, adopted: i32, daylight: bool) -> Shift {
        Shift {
            moment: at(utc, UtcOffset::UTC),
            vacated: offset(vacated),
            adopted: offset(adopted),
            daylight,
        }
    }

    /// Whether `moved` sprang over `local`, that is, whether the wall clock jumped past it.
    fn sprang_over(moved: Shift, local: CivilDateTime) -> bool {
        let opened = CivilDateTime::from_instant(moved.moment, moved.vacated);
        let closed = CivilDateTime::from_instant(moved.moment, moved.adopted);
        matches!((opened, closed), (Some(from), Some(to)) if from <= local && local < to)
    }

    /// A zone as the finite table of transitions a `VTIMEZONE` spells out.
    ///
    /// Written here rather than taken from the unit that reads a real `VTIMEZONE`, because
    /// what is under test is the combinator and a test reaching into another unit's file would
    /// be testing that unit too. Every table below opens in standard time and carries the tz
    /// database's own transitions for the zone it names.
    #[derive(Clone, Debug)]
    struct Zone {
        /// The identifier the file wrote, whatever it happens to be.
        tzid: &'static str,
        /// The offset in force before the first transition.
        standing: UtcOffset,
        /// The transitions, in order.
        shifts: Vec<Shift>,
        /// The last date the table has real data for, absent when its rules run on.
        known_through: Option<CivilDate>,
        /// Which source this table stands for.
        provenance: ZoneProvenance,
    }

    impl Zone {
        /// The same table wired in as something else — the caller's database rather than the
        /// calendar's own definitions, or the other way about.
        fn sourced_from(mut self, provenance: ZoneProvenance) -> Self {
            self.provenance = provenance;
            self
        }

        /// The offset in force at `moment`, and whether it is the daylight one.
        fn running_at(&self, moment: Instant) -> (UtcOffset, bool) {
            let mut current = (self.standing, false);
            for moved in &self.shifts {
                if moved.moment <= moment {
                    current = (moved.adopted, moved.daylight);
                }
            }
            current
        }

        /// Every offset this table ever runs at, with the daylight flag it carries there.
        fn candidates(&self) -> Vec<(UtcOffset, bool)> {
            let mut all = alloc::vec![(self.standing, false)];
            all.extend(
                self.shifts
                    .iter()
                    .map(|moved| (moved.adopted, moved.daylight)),
            );
            all
        }

        /// Every reading of `local` the table admits, earliest first.
        ///
        /// The standard algorithm and the only honest one: a candidate offset names a real
        /// instant only if the offset in force at that instant is the candidate itself. None
        /// is a gap, one is an ordinary day, two is a fold.
        fn readings(&self, local: CivilDateTime) -> Vec<Reading> {
            let mut found: Vec<Reading> = Vec::new();
            for (candidate, daylight) in self.candidates() {
                let Some(moment) = local.at_offset(candidate) else {
                    continue;
                };
                if self.running_at(moment) != (candidate, daylight) {
                    continue;
                }
                if found.iter().any(|reading| reading.instant == moment) {
                    continue;
                }
                found.push(Reading::new(moment, candidate, daylight));
            }
            found.sort_unstable();
            found
        }

        /// The gap `local` fell into, when it fell into one.
        fn gap(&self, local: CivilDateTime) -> Option<LocalResolution> {
            let moved = self
                .shifts
                .iter()
                .copied()
                .find(|candidate| sprang_over(*candidate, local))?;
            Some(nonexistent(
                local,
                moved.moment,
                moved.vacated,
                moved.adopted,
            ))
        }

        /// `Computed` while the table has data, and the date it ran out on past that.
        fn basis(&self, asked: CivilDate) -> AnswerBasis {
            match self.known_through {
                Some(last) if asked > last => AnswerBasis::BeyondKnownTransitions(last),
                _ => AnswerBasis::Computed,
            }
        }
    }

    impl ZoneSource for Zone {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            if tzid != self.tzid {
                return None;
            }
            let readings = self.readings(local);
            let resolution = match readings.as_slice() {
                // No reading means a transition sprang over the wall clock, which is the only
                // way a table can name no instant at all.
                [] => self.gap(local).unwrap(),
                [only] => LocalResolution::Unique { reading: *only },
                [earlier, later, ..] => LocalResolution::Ambiguous {
                    earlier: *earlier,
                    later: *later,
                },
            };
            Some(ZoneAnswer::new(
                resolution,
                self.provenance,
                self.basis(local.date()),
            ))
        }

        fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
            if tzid != self.tzid {
                return None;
            }
            let (running, daylight) = self.running_at(instant);
            let asked = CivilDateTime::from_instant(instant, running).unwrap();
            Some(OffsetAnswer::new(
                running,
                daylight,
                self.provenance,
                self.basis(asked.date()),
            ))
        }
    }

    /// A source that records how often it was asked.
    ///
    /// The only way to see from outside that both sources were queried: an outcome of
    /// `OnlyEmbedded` looks the same whether the other source was asked and knew nothing or
    /// was never asked at all.
    #[derive(Debug)]
    struct Counting {
        /// The zone underneath.
        zone: Zone,
        /// How often either method has been called.
        calls: Cell<u32>,
    }

    impl Counting {
        fn new(zone: Zone) -> Self {
            Self {
                zone,
                calls: Cell::new(0),
            }
        }

        fn calls(&self) -> u32 {
            self.calls.get()
        }

        /// Record one call, saturating rather than wrapping.
        fn note(&self) {
            self.calls.set(self.calls.get().saturating_add(1));
        }
    }

    impl ZoneSource for Counting {
        fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
            self.note();
            self.zone.resolve(tzid, local)
        }

        fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
            self.note();
            self.zone.offset_at(tzid, instant)
        }
    }

    /// The nonexistent reading a spring-forward at `moment` gives `local`.
    ///
    /// In UTC a gap has no width at all — it is the wall clock that jumps — so the last
    /// instant under the old offset and the first under the new one are one second apart,
    /// which is what `gap_start < gap_end` asks for and what makes
    /// `GapPolicy::ClampToTransition` land on the instant the clocks moved. `shifted` is RFC
    /// 5545 section 3.3.5's reading of the queried wall clock and nothing else.
    fn nonexistent(
        local: CivilDateTime,
        moment: Instant,
        vacated: UtcOffset,
        adopted: UtcOffset,
    ) -> LocalResolution {
        LocalResolution::Nonexistent {
            gap_start: moment.checked_add_seconds(-1).unwrap(),
            gap_end: moment,
            offset_before: vacated,
            offset_after: adopted,
            shifted: at(local, vacated),
        }
    }

    /// What a zone's own rule says a wall clock names, stated the way the rule is stated
    /// rather than as instants this file computed.
    #[derive(Clone, Copy, Debug)]
    enum Rule {
        /// An ordinary day: one offset, and whether it is the daylight one.
        Plain {
            /// The offset in force, in seconds east of UTC.
            running: i32,
            /// Whether that is the zone's daylight offset.
            daylight: bool,
        },
        /// A fall-back: the daylight offset left and the standard one taken.
        Fold {
            /// The offset in force before the transition.
            leaving: i32,
            /// The offset in force after it.
            taking: i32,
        },
        /// A spring-forward: the UTC wall clock the zone moved at, and the two offsets.
        Gap {
            /// The instant of the transition, as a UTC wall clock.
            moved: CivilDateTime,
            /// The offset in force before it.
            leaving: i32,
            /// The offset in force after it.
            taking: i32,
        },
    }

    impl Rule {
        /// The resolution this rule says `local` has.
        fn resolution(self, local: CivilDateTime) -> LocalResolution {
            match self {
                Self::Plain { running, daylight } => LocalResolution::Unique {
                    reading: Reading::new(at(local, offset(running)), offset(running), daylight),
                },
                // Every fall-back below leaves a daylight offset for a standard one, and the
                // daylight offset is the larger of the two, so reading the wall clock at the
                // offset being left names the earlier of the two instants.
                Self::Fold { leaving, taking } => LocalResolution::Ambiguous {
                    earlier: Reading::new(at(local, offset(leaving)), offset(leaving), true),
                    later: Reading::new(at(local, offset(taking)), offset(taking), false),
                },
                Self::Gap {
                    moved,
                    leaving,
                    taking,
                } => nonexistent(
                    local,
                    at(moved, UtcOffset::UTC),
                    offset(leaving),
                    offset(taking),
                ),
            }
        }
    }

    /// Which of the five outcomes this is, with the answers left out.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Shape {
        Agreed,
        Disagreed,
        OnlyEmbedded,
        OnlyFallback,
        Neither,
        Undetermined,
    }

    fn shape<A: Copy>(outcome: PolicyOutcome<A>) -> Shape {
        match outcome {
            PolicyOutcome::Agreed { .. } => Shape::Agreed,
            PolicyOutcome::Disagreed { .. } => Shape::Disagreed,
            PolicyOutcome::OnlyEmbedded(_) => Shape::OnlyEmbedded,
            PolicyOutcome::OnlyFallback(_) => Shape::OnlyFallback,
            PolicyOutcome::Neither => Shape::Neither,
            _ => Shape::Undetermined,
        }
    }

    /// `America/New_York` as the tz database has had it since 2007: daylight saving from 02:00
    /// on the second Sunday in March to 02:00 on the first Sunday in November.
    fn new_york() -> Zone {
        Zone {
            tzid: "America/New_York",
            standing: offset(-18_000),
            shifts: alloc::vec![
                shift(stamp(2026, 3, 8, 7, 0), -18_000, -14_400, true),
                shift(stamp(2026, 11, 1, 6, 0), -14_400, -18_000, false),
            ],
            known_through: None,
            provenance: ZoneProvenance::EmbeddedVtimezone,
        }
    }

    /// The same zone as a client wrote it into a file before 2007: daylight saving from the
    /// first Sunday in April to the last Sunday in October, which is what the United States
    /// ran until the Energy Policy Act moved both dates.
    fn new_york_before_2007() -> Zone {
        Zone {
            shifts: alloc::vec![
                shift(stamp(2026, 4, 5, 7, 0), -18_000, -14_400, true),
                shift(stamp(2026, 10, 25, 6, 0), -14_400, -18_000, false),
            ],
            ..new_york()
        }
    }

    /// The European Union's transitions for `years` of [`BERLIN_DAYS`], at 01:00 UTC.
    fn berlin_shifts(years: usize) -> Vec<Shift> {
        let mut shifts = Vec::new();
        for (year, spring, autumn) in BERLIN_DAYS.iter().copied().take(years) {
            shifts.push(shift(stamp(year, 3, spring, 1, 0), 3_600, 7_200, true));
            shifts.push(shift(stamp(year, 10, autumn, 1, 0), 7_200, 3_600, false));
        }
        shifts
    }

    /// Europe/Berlin as the caller's database has it, moving at 01:00 UTC rather than at a
    /// local hour, which is what makes its fold 02:00 to 03:00 local rather than 01:00 to
    /// 02:00.
    fn berlin() -> Zone {
        Zone {
            tzid: "Europe/Berlin",
            standing: offset(3_600),
            shifts: berlin_shifts(BERLIN_DAYS.len()),
            known_through: None,
            provenance: ZoneProvenance::CallerDatabase,
        }
    }

    /// The same zone as a file that spelled its transitions out as `RDATE` lines through 2029
    /// and then stopped.
    ///
    /// The input this milestone's brief names: asked about 2035 it continues its final
    /// observance, which is the defensible answer, and says so on every answer's basis.
    fn berlin_through_2029() -> Zone {
        Zone {
            shifts: berlin_shifts(4),
            known_through: CivilDate::from_ymd(2029, 10, 28),
            ..berlin().sourced_from(ZoneProvenance::EmbeddedVtimezone)
        }
    }

    /// `Australia/Lord_Howe`, whose daylight saving is half an hour rather than a whole one:
    /// `+10:30` standard and `+11:00` in daylight, moving at 02:00 local on the first Sunday
    /// in October and the first Sunday in April.
    fn lord_howe() -> Zone {
        Zone {
            tzid: "Australia/Lord_Howe",
            standing: offset(37_800),
            shifts: alloc::vec![
                shift(stamp(2025, 10, 4, 15, 30), 37_800, 39_600, true),
                shift(stamp(2026, 4, 4, 15, 0), 39_600, 37_800, false),
                shift(stamp(2026, 10, 3, 15, 30), 37_800, 39_600, true),
            ],
            known_through: None,
            provenance: ZoneProvenance::EmbeddedVtimezone,
        }
    }

    /// Berlin's rules under whatever identifier a file happened to write.
    ///
    /// A `TZID` is a string this crate may not parse: `/mozilla.org/20050126_1/Europe/Berlin`
    /// from Lightning, `W. Europe Standard Time` from Exchange and `Customized Time Zone` from
    /// a user's own dialog all name a zone, and none of them is an IANA key.
    fn named(tzid: &'static str) -> Zone {
        Zone {
            tzid,
            ..berlin().sourced_from(ZoneProvenance::EmbeddedVtimezone)
        }
    }

    /// A fixed offset resolves the two hours a real zone cannot, which is exactly what it is
    /// for and exactly why it is not a substitute for a zone.
    #[test]
    fn a_fixed_offset_names_one_instant_at_every_hour_including_the_awkward_ones() {
        let pinned = FixedOffsetSource::new("W. Europe Standard Time", offset(3_600), false);
        let cases = [
            (
                "the hour America/New_York repeats",
                stamp(2026, 11, 1, 1, 30),
            ),
            ("the hour it skips", stamp(2026, 3, 8, 2, 30)),
            ("the half hour Lord Howe skips", stamp(2026, 10, 4, 2, 15)),
            ("an ordinary noon", stamp(2026, 7, 1, 12, 0)),
        ];
        for (hour, local) in cases {
            let answer = pinned.resolve("W. Europe Standard Time", local).unwrap();
            let expected = Rule::Plain {
                running: 3_600,
                daylight: false,
            };
            assert_eq!(answer.resolution, expected.resolution(local), "{hour}");
            assert!(!answer.resolution.is_ambiguous(), "{hour}");
            assert!(!answer.resolution.is_nonexistent(), "{hour}");
            assert_eq!(answer.resolution.diagnostic_code(), None, "{hour}");
            assert_eq!(answer.source, ZoneProvenance::FixedOffset, "{hour}");
            assert_eq!(answer.basis, AnswerBasis::Computed, "{hour}");
        }
    }

    /// An identifier it does not answer to is unrecognized, and never a default.
    #[test]
    fn a_fixed_offset_answers_its_own_identifier_and_no_other() {
        let pinned = FixedOffsetSource::new("W. Europe Standard Time", offset(3_600), false);
        assert_eq!(pinned.tzid(), "W. Europe Standard Time");
        assert_eq!(pinned.offset(), offset(3_600));
        assert!(!pinned.daylight());
        assert_eq!(
            pinned.resolve("Europe/Berlin", stamp(2026, 7, 1, 12, 0)),
            None
        );
        assert_eq!(pinned.offset_at("Europe/Berlin", Instant::EPOCH), None);
        assert_eq!(
            pinned.offset_at("W. Europe Standard Time", Instant::EPOCH),
            Some(OffsetAnswer::new(
                offset(3_600),
                false,
                ZoneProvenance::FixedOffset,
                AnswerBasis::Computed,
            ))
        );
    }

    /// Real folds and real gaps, from three zones with three different transition shapes, and
    /// both answers keep the source that produced them.
    #[test]
    fn a_fold_and_a_gap_survive_the_pair_and_each_answer_names_its_source() {
        // Spelled out in UTC once, because a table checked only against another expression of
        // the same table proves less than it looks: 01:30 on 2026-11-01 in New York is 05:30Z
        // under EDT and 06:30Z under EST.
        let repeated = stamp(2026, 11, 1, 1, 30);
        let told = new_york().resolve("America/New_York", repeated).unwrap();
        assert_eq!(
            told.resolution,
            LocalResolution::Ambiguous {
                earlier: Reading::new(
                    at(stamp(2026, 11, 1, 5, 30), UtcOffset::UTC),
                    offset(-14_400),
                    true,
                ),
                later: Reading::new(
                    at(stamp(2026, 11, 1, 6, 30), UtcOffset::UTC),
                    offset(-18_000),
                    false,
                ),
            }
        );

        for (rule_of, zone, local, rule) in awkward_hours() {
            let mirror = zone.clone().sourced_from(ZoneProvenance::CallerDatabase);
            let combined = CombinedZoneSource::new(&zone, &mirror);
            let outcome = combined.resolve(zone.tzid, local);
            let PolicyOutcome::Agreed { embedded, fallback } = outcome else {
                panic!("{rule_of}: two readings of one table are one answer");
            };
            assert_eq!(embedded.resolution, rule.resolution(local), "{rule_of}");
            assert_eq!(fallback.resolution, embedded.resolution, "{rule_of}");
            assert_eq!(
                embedded.source,
                ZoneProvenance::EmbeddedVtimezone,
                "{rule_of}"
            );
            assert_eq!(fallback.source, ZoneProvenance::CallerDatabase, "{rule_of}");
            assert_eq!(outcome.diagnostic_code(), None, "{rule_of}");
        }
    }

    /// The zone, the wall clock asked about, and what that zone's own rule says it names.
    ///
    /// Every rule is the tz database's, worked out by hand rather than read back from what
    /// this file answered.
    fn awkward_hours() -> [(&'static str, Zone, CivilDateTime, Rule); 6] {
        [
            (
                "New York falls back at 02:00 EDT on the first Sunday in November",
                new_york(),
                stamp(2026, 11, 1, 1, 30),
                Rule::Fold {
                    leaving: -14_400,
                    taking: -18_000,
                },
            ),
            (
                "and springs forward at 02:00 EST on the second Sunday in March",
                new_york(),
                stamp(2026, 3, 8, 2, 30),
                Rule::Gap {
                    moved: stamp(2026, 3, 8, 7, 0),
                    leaving: -18_000,
                    taking: -14_400,
                },
            ),
            (
                "Berlin moves at 01:00 UTC, so its fold is 02:00 to 03:00 local",
                named("Europe/Berlin"),
                stamp(2026, 10, 25, 2, 30),
                Rule::Fold {
                    leaving: 7_200,
                    taking: 3_600,
                },
            ),
            (
                "and an ordinary summer noon there names one instant",
                named("Europe/Berlin"),
                stamp(2026, 7, 1, 12, 0),
                Rule::Plain {
                    running: 7_200,
                    daylight: true,
                },
            ),
            (
                "Lord Howe repeats half an hour rather than a whole one",
                lord_howe(),
                stamp(2026, 4, 5, 1, 45),
                Rule::Fold {
                    leaving: 39_600,
                    taking: 37_800,
                },
            ),
            (
                "and skips half of one: 02:00 becomes 02:30 in October",
                lord_howe(),
                stamp(2026, 10, 4, 2, 15),
                Rule::Gap {
                    moved: stamp(2026, 10, 3, 15, 30),
                    leaving: 37_800,
                    taking: 39_600,
                },
            ),
        ]
    }

    /// The disagreement this crate exists for, on the input `docs/adr/0003` describes.
    ///
    /// A file written by a client in 2006 carries 2006's rules; the database the caller wired
    /// in carries the ones the Energy Policy Act moved in 2007. Mid-March is where they
    /// differ, and each is right about the question it answers.
    #[test]
    fn rules_that_changed_after_2007_disagree_and_the_disagreement_is_the_answer() {
        let written = new_york_before_2007();
        let today = new_york().sourced_from(ZoneProvenance::CallerDatabase);
        let combined = CombinedZoneSource::new(&written, &today);
        let local = stamp(2026, 3, 15, 12, 0);
        let outcome = combined.resolve("America/New_York", local);
        let PolicyOutcome::Disagreed {
            embedded: older,
            fallback: newer,
        } = outcome
        else {
            panic!("2006's rules and today's do not agree about the 15th of March");
        };

        let standard = Rule::Plain {
            running: -18_000,
            daylight: false,
        };
        let daylight = Rule::Plain {
            running: -14_400,
            daylight: true,
        };
        assert_eq!(
            older.resolution,
            standard.resolution(local),
            "EST until April"
        );
        assert_eq!(
            newer.resolution,
            daylight.resolution(local),
            "EDT since March"
        );
        assert_eq!(older.source, ZoneProvenance::EmbeddedVtimezone);
        assert_eq!(newer.source, ZoneProvenance::CallerDatabase);
        assert!(outcome.is_disagreement());
        assert_eq!(
            outcome.diagnostic_code(),
            Some(DiagnosticCode::TimeZoneSourceDisagreement)
        );
        assert_eq!(outcome.embedded_first(), Some(older));
        // Both tables computed their answers from rules they hold. Staleness is the case
        // `docs/adr/0003` names as the one `basis` does not close, and this is it: nothing in
        // either answer says one side's rules are twenty years old.
        assert_eq!(older.basis, AnswerBasis::Computed);
        assert_eq!(newer.basis, AnswerBasis::Computed);
    }

    /// A table that ran out stays distinguishable from one that did not, agreement and all.
    #[test]
    fn a_table_that_ends_before_the_question_says_so_on_every_answer_it_gives() {
        let embedded = berlin_through_2029();
        let database = berlin();
        let combined = CombinedZoneSource::new(&embedded, &database);
        let ran_out =
            AnswerBasis::BeyondKnownTransitions(CivilDate::from_ymd(2029, 10, 28).unwrap());

        // January 2035: both say `+01:00` standard, and one of them says it by continuing an
        // observance it last had data for in 2029. `Agreed` keeps both answers so that
        // difference survives the agreement, which is the whole reason it does.
        let winter = combined.resolve("Europe/Berlin", stamp(2035, 1, 15, 12, 0));
        let PolicyOutcome::Agreed {
            embedded: stale,
            fallback: fresh,
        } = winter
        else {
            panic!("both readings of a January noon in Berlin are +01:00");
        };
        assert_eq!(stale.resolution, fresh.resolution);
        assert_eq!(stale.basis, ran_out);
        assert_eq!(fresh.basis, AnswerBasis::Computed);
        assert_eq!(
            winter.diagnostic_code(),
            None,
            "agreement is not disagreement"
        );
        assert_eq!(
            stale.basis.diagnostic_code(),
            Some(DiagnosticCode::TimeZoneCoverageExhausted),
            "the coverage fact travels on the answer, for its consumer to report"
        );

        // July 2035: the continued observance is simply wrong, because the zone does move.
        // The two facts arrive separately rather than one hiding the other.
        let summer = combined.resolve("Europe/Berlin", stamp(2035, 7, 1, 12, 0));
        assert!(summer.is_disagreement());
        let PolicyOutcome::Disagreed {
            embedded: guessed,
            fallback: known,
        } = summer
        else {
            panic!("a table that stopped in 2029 has no summer of 2035");
        };
        assert_eq!(guessed.basis, ran_out);
        assert_eq!(known.basis, AnswerBasis::Computed);
    }

    /// A `TZID` is not an IANA identifier, and neither side may pretend otherwise.
    #[test]
    fn an_identifier_that_is_not_an_iana_name_is_answered_or_refused_by_name() {
        let database = berlin();
        // What the file declared, what a property asked for, and who ends up answering.
        let cases = [
            (
                "a Lightning prefix is an identifier and not a path",
                "/mozilla.org/20050126_1/Europe/Berlin",
                "/mozilla.org/20050126_1/Europe/Berlin",
                Shape::OnlyEmbedded,
            ),
            (
                "an Exchange zone name is an identifier and not an IANA key",
                "W. Europe Standard Time",
                "W. Europe Standard Time",
                Shape::OnlyEmbedded,
            ),
            (
                "a name a user typed into a dialog is still an identifier",
                "Customized Time Zone",
                "Customized Time Zone",
                Shape::OnlyEmbedded,
            ),
            (
                "the database knows the IANA key the file did not use",
                "W. Europe Standard Time",
                "Europe/Berlin",
                Shape::OnlyFallback,
            ),
            (
                "casing is not folded, because aliasing is the caller's visible step",
                "W. Europe Standard Time",
                "europe/berlin",
                Shape::Neither,
            ),
            (
                "and a zone nobody supplied is reported rather than defaulted to UTC",
                "W. Europe Standard Time",
                "Pacific/Kiritimati",
                Shape::Neither,
            ),
        ];
        for (spelling, declared, asked, expected) in cases {
            let embedded = named(declared);
            let combined = CombinedZoneSource::new(&embedded, &database);
            let outcome = combined.resolve(asked, stamp(2026, 7, 1, 12, 0));
            assert_eq!(shape(outcome), expected, "{spelling}");
            assert_eq!(
                outcome.embedded_first().is_some(),
                expected != Shape::Neither,
                "{spelling}"
            );
        }
    }

    /// `docs/adr/0003` refuses a fallback chain, and a combinator that stopped as soon as one
    /// source answered would be one under another name.
    ///
    /// Counting the calls is the only way to see it from outside: an outcome of
    /// `OnlyEmbedded` is the same value either way.
    #[test]
    fn both_sources_are_queried_on_every_call_whichever_of_them_answers() {
        let iana = Counting::new(named("Europe/Berlin"));
        let windows = Counting::new(named("W. Europe Standard Time"));
        let combined = CombinedZoneSource::new(&iana, &windows);
        let local = stamp(2026, 7, 1, 12, 0);

        assert_eq!(
            shape(combined.resolve("Europe/Berlin", local)),
            Shape::OnlyEmbedded
        );
        assert_eq!(
            (iana.calls(), windows.calls()),
            (1, 1),
            "the source that could not have helped was asked anyway"
        );
        assert_eq!(
            shape(combined.resolve("Pacific/Kiritimati", local)),
            Shape::Neither
        );
        assert_eq!(
            (iana.calls(), windows.calls()),
            (4, 4),
            "an identifier neither answered is asked once more of each: the recognition              question, which is what tells a zone nobody supplied from a zone supplied with no              transitions. It is asked only in that arm, and only after both lookups happened."
        );
        assert_eq!(
            shape(combined.offset_at("Europe/Berlin", at(local, UtcOffset::UTC))),
            Shape::OnlyEmbedded
        );
        assert_eq!(
            (iana.calls(), windows.calls()),
            (5, 5),
            "offset_at short circuits no more than resolve does"
        );
    }

    /// A fixed offset cannot corroborate a fold, and the pair says so rather than agreeing.
    #[test]
    fn a_fixed_offset_beside_a_zone_disagrees_at_a_fold_rather_than_settling_it() {
        let zone = new_york();
        let pinned = FixedOffsetSource::new("America/New_York", offset(-18_000), false);
        let combined = CombinedZoneSource::new(&zone, &pinned);
        let local = stamp(2026, 11, 1, 1, 30);
        let outcome = combined.resolve("America/New_York", local);
        let PolicyOutcome::Disagreed {
            embedded: zoned,
            fallback: flat,
        } = outcome
        else {
            panic!("one instant against two is a disagreement");
        };
        assert!(zoned.resolution.is_ambiguous());
        assert_eq!(
            zoned.resolution.diagnostic_code(),
            Some(DiagnosticCode::AmbiguousLocalTime),
            "the fold is the answer's own code, not this unit's to emit"
        );
        assert_eq!(flat.source, ZoneProvenance::FixedOffset);
        assert_eq!(
            flat.resolution.unambiguous(),
            Some(at(local, offset(-18_000)))
        );
    }

    /// The two codes this unit owns reach the sink, and nothing else does.
    #[test]
    fn reporting_puts_the_two_codes_this_unit_owns_on_the_sink_and_no_others() {
        let embedded = berlin_through_2029();
        let database = berlin();
        let combined = CombinedZoneSource::new(&embedded, &database);
        // The identifier, the instant, and what reporting that offset outcome should leave in
        // the caller's sink. July 2035 is past the embedded table's last transition and inside
        // the database's rules, which is where the two part company.
        let cases = [
            (
                "two sources, two answers, one instant",
                "Europe/Berlin",
                stamp(2035, 7, 1, 12, 0),
                Some((DiagnosticCode::TimeZoneSourceDisagreement, Severity::Note)),
            ),
            (
                "agreement is silent, whatever stood behind it",
                "Europe/Berlin",
                stamp(2035, 1, 15, 12, 0),
                None,
            ),
            (
                "nobody supplied this one",
                "Customized Time Zone",
                stamp(2035, 7, 1, 12, 0),
                Some((DiagnosticCode::UnknownTimeZone, Severity::Violation)),
            ),
        ];
        for (case, tzid, local, expected) in cases {
            let moment = at(local, UtcOffset::UTC);
            let outcome = combined.offset_at(tzid, moment);
            let mut kept: Vec<Diagnostic> = Vec::new();
            let mut meter = Meter::new(Limits::DEFAULT);
            combined.report(outcome, moment, &mut meter, &mut kept);
            let seen: Vec<(DiagnosticCode, Severity)> = kept
                .iter()
                .map(|entry| (entry.code(), entry.severity()))
                .collect();
            let wanted: Vec<(DiagnosticCode, Severity)> = expected.into_iter().collect();
            assert_eq!(seen, wanted, "{case}");
            assert!(
                kept.iter().all(|entry| entry.instant() == Some(moment)),
                "{case}"
            );
            assert_eq!(meter.diagnostics_dropped(), 0, "{case}");
        }
    }

    /// A sink that keeps nothing still counts, because reporting goes through
    /// `report_diagnostic` rather than through `DiagnosticSink::push`.
    #[test]
    fn a_refused_diagnostic_is_charged_to_the_caller_s_meter() {
        let embedded = berlin_through_2029();
        let database = berlin();
        let combined = CombinedZoneSource::new(&embedded, &database);
        let moment = at(stamp(2035, 7, 1, 12, 0), UtcOffset::UTC);
        let mut meter = Meter::new(Limits::DEFAULT);
        let split = combined.offset_at("Europe/Berlin", moment);
        combined.report(split, moment, &mut meter, &mut IgnoreDiagnostics);
        let nobody = combined.offset_at("Customized Time Zone", moment);
        combined.report(nobody, moment, &mut meter, &mut IgnoreDiagnostics);
        assert_eq!(meter.diagnostics_dropped(), 2);
    }
}
