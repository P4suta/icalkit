// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `VTIMEZONE` as a finite table of transitions, and the rules that generate them.
//!
//! Specification: RFC 5545 section 3.6.5, the `VTIMEZONE` component, with section 3.8.3.3 and
//! section 3.8.3.4 for `TZOFFSETFROM` and `TZOFFSETTO`, section 3.8.5.2 for `RDATE` and
//! section 3.8.5.3 for `RRULE` <https://www.rfc-editor.org/rfc/rfc5545#section-3.6.5>.
//!
//! A `VTIMEZONE` carries its transitions two ways and often both at once. The `RRULE` form
//! says "the last Sunday in March, every year", which is a rule that runs on forever. The
//! `RDATE` form is a finite table of dates that simply stops: a zone written in 2019 with
//! explicit transitions through 2029, referenced by an event in 2035, has no data for 2035.
//!
//! Both are modeled here, and the second is the one that needed a decision. Clamping to the
//! last known state and extrapolating a guessed rule are both silent lies, so this crate does
//! the defensible thing — continues the nearest observance — and refuses to do it quietly:
//! [`TransitionTable::coverage_end`] is the date past which every answer carries
//! [`AnswerBasis::BeyondKnownTransitions`], and [`TransitionTable::coverage_start`] is the date
//! before which it carries [`AnswerBasis::BeforeKnownTransitions`]. Both are different values
//! from the one a rule computed and neither can be mistaken for it. A table has two ends, which
//! M2 found stated for one of them: a definition whose `RDATE` lines begin in 2027 answers July
//! 2020 by extending its earliest `TZOFFSETFROM` backwards forever, and `America/New_York` was
//! not on `-05:00` that July.
//!
//! [`AnswerBasis::BeforeKnownTransitions`]: crate::AnswerBasis::BeforeKnownTransitions
//!
//! # Why the rules are a closed form and not a recurrence search
//!
//! [`YearlyRule`] is deliberately narrower than section 3.3.10's `RECUR`. It says a month, a
//! way of naming a day inside it, a time, and an optional last year, and
//! `YearlyRule::occurrence_in` answers with arithmetic over the weekday of the first of the
//! month: no loop, no candidate set, no budget, and therefore no way to make a zone lookup do
//! unbounded work. That is what keeps `docs/adr/0010`'s argument satisfied structurally on the
//! resolution path instead of by a meter nobody can charge from inside
//! [`ZoneSource::resolve`].
//!
//! The four day forms cover every rule the tz database's own generator and the major producers
//! emit: a fixed day of the month, the *n*th or last weekday of it, and the two "first weekday
//! on or after this date" shapes Exchange and older tzdata releases write as a `BYDAY` paired
//! with a run of `BYMONTHDAY` values. A rule outside them is
//! [`DiagnosticCode::VtimezoneRuleUnsupported`] and the observance's own `DTSTART` still stands
//! as one transition, which is a smaller answer rather than a wrong one.
//!
//! This is also why this crate does not depend on `ical-recur`. It could; `just purity` would
//! allow the path and no cycle would appear. What it would buy is generality this subject does
//! not have, and what it would cost is turning an O(1) lookup into a bounded search with a
//! meter threaded through the one trait `docs/adr/0003` needs a caller to be able to implement.
//!
//! [`AnswerBasis::BeyondKnownTransitions`]: crate::AnswerBasis::BeyondKnownTransitions
//! [`ZoneSource::resolve`]: crate::ZoneSource::resolve
//! [`DiagnosticCode::VtimezoneRuleUnsupported`]: ical_core::DiagnosticCode::VtimezoneRuleUnsupported

use alloc::boxed::Box;
use alloc::vec::Vec;

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, DiagnosticSink, Instant,
    LimitExceeded, Location, Meter, Severity, UtcOffset, Weekday, report_diagnostic,
};

use crate::ident::Tzid;

/// Which occurrence of a weekday inside a month a rule means.
///
/// [`NthWeek::Fifth`] and [`NthWeek::Last`] are both here because they are different rules
/// that agree most of the time: `BYDAY=5SU` names nothing in a month with four Sundays, and
/// `BYDAY=-1SU` always names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NthWeek {
    /// The first such weekday in the month.
    First,
    /// The second.
    Second,
    /// The third.
    Third,
    /// The fourth.
    Fourth,
    /// The fifth, which most months do not have.
    Fifth,
    /// The last, whichever ordinal that turns out to be.
    Last,
}

/// How a rule names the day inside its month.
///
/// `#[non_exhaustive]` so that a fifth form found in the wild is not a breaking change; the
/// variants themselves stay constructible, because a caller building a table by hand from a
/// database it already has is the ordinary case this crate is designed around.
///
/// Every form is evaluated by arithmetic. A value outside the month — day 40, or a fifth
/// Sunday in a month with four — is `None` from the evaluation rather than a refusal at
/// construction, because a rule that names nothing in one year may name something in the next
/// and only the year makes the question answerable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RuleDay {
    /// A fixed day of the month, as `BYMONTHDAY=n` writes one.
    DayOfMonth(u8),
    /// The last day of the month, as `BYMONTHDAY=-1` writes it.
    LastDayOfMonth,
    /// The *n*th or last given weekday, as `BYDAY=2SU` or `BYDAY=-1SU` writes one.
    Nth {
        /// The weekday.
        weekday: Weekday,
        /// Which occurrence of it.
        week: NthWeek,
    },
    /// The first given weekday falling on or after a day, as `BYDAY=SU;BYMONTHDAY=8,..,14`
    /// writes one.
    OnOrAfter {
        /// The weekday.
        weekday: Weekday,
        /// The earliest day of the month it may fall on.
        day: u8,
    },
    /// The last given weekday falling on or before a day, as `BYDAY=SU;BYMONTHDAY=25,..,31`
    /// writes one.
    OnOrBefore {
        /// The weekday.
        weekday: Weekday,
        /// The latest day of the month it may fall on.
        day: u8,
    },
}

/// A yearly transition rule, restricted to the forms a zone definition actually uses.
///
/// The wall clock in `at` is read against the observance's `TZOFFSETFROM`, which is what RFC
/// 5545 section 3.6.5 means by the observance's own `DTSTART`: the transition happens when the
/// clock that is still running reaches that time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearlyRule {
    /// The month, `1` through `12`.
    month: u8,
    /// How the day inside it is named.
    day: RuleDay,
    /// The wall clock the transition happens at, read against `TZOFFSETFROM`.
    at: CivilTime,
    /// The last date the rule applies to, from `UNTIL`; absent when the rule runs on.
    through: Option<CivilDate>,
}

impl YearlyRule {
    /// A rule for `month`, or `None` when there is no such month.
    #[must_use]
    pub const fn new(
        month: u8,
        day: RuleDay,
        at: CivilTime,
        through: Option<CivilDate>,
    ) -> Option<Self> {
        if month == 0 || month > 12 {
            return None;
        }
        Some(Self {
            month,
            day,
            at,
            through,
        })
    }

    /// The month, `1` through `12`.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    /// How the day inside the month is named.
    #[must_use]
    pub const fn day(self) -> RuleDay {
        self.day
    }

    /// The wall clock the transition happens at.
    #[must_use]
    pub const fn at(self) -> CivilTime {
        self.at
    }

    /// The last date the rule applies to, absent when it runs on forever.
    ///
    /// A rule with no end is what makes [`TransitionTable::coverage_end`] `None`: such a zone
    /// knows the future, and no answer it gives is ever an extrapolation.
    #[must_use]
    pub const fn through(self) -> Option<CivilDate> {
        self.through
    }
}

/// One `STANDARD` or `DAYLIGHT` subcomponent, or one date out of an `RDATE` inside one.
///
/// An `RDATE`-driven definition becomes one of these per date, sharing the offsets its
/// subcomponent declared and carrying no rule. That is what makes a table a flat sorted list
/// rather than a tree, and it is why running out of `RDATE`s is visible as the list simply
/// ending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Observance {
    /// When this observance begins, as a wall clock read against `offset_from`.
    start: CivilDateTime,
    /// The offset in force before it, from `TZOFFSETFROM`.
    offset_from: UtcOffset,
    /// The offset in force from it, from `TZOFFSETTO`.
    offset_to: UtcOffset,
    /// Whether this is the zone's daylight observance.
    daylight: bool,
    /// The rule repeating it, absent for a date-driven transition.
    rule: Option<YearlyRule>,
}

impl Observance {
    /// An observance beginning at `start`.
    #[must_use]
    pub const fn new(
        start: CivilDateTime,
        offset_from: UtcOffset,
        offset_to: UtcOffset,
        daylight: bool,
        rule: Option<YearlyRule>,
    ) -> Self {
        Self {
            start,
            offset_from,
            offset_to,
            daylight,
            rule,
        }
    }

    /// When this observance begins, as a wall clock read against `offset_from`.
    #[must_use]
    pub const fn start(self) -> CivilDateTime {
        self.start
    }

    /// The offset in force before it.
    #[must_use]
    pub const fn offset_from(self) -> UtcOffset {
        self.offset_from
    }

    /// The offset in force from it.
    #[must_use]
    pub const fn offset_to(self) -> UtcOffset {
        self.offset_to
    }

    /// Whether this is the zone's daylight observance.
    #[must_use]
    pub const fn daylight(self) -> bool {
        self.daylight
    }

    /// The rule repeating it, absent for a date-driven transition.
    #[must_use]
    pub const fn rule(self) -> Option<YearlyRule> {
        self.rule
    }

    /// Whether the offsets on either side of this observance differ.
    ///
    /// A transition that moves no clock is legal, common at the head of a table where the
    /// first observance states the zone's base offset against itself, and not a moment any
    /// wall clock is ambiguous or missing at.
    #[must_use]
    pub const fn moves_the_clock(self) -> bool {
        self.offset_from.seconds() != self.offset_to.seconds()
    }

    /// The last date this observance has real data for, absent when its rule runs on forever.
    ///
    /// A date-driven observance covers its own date and nothing after it; a rule-driven one
    /// covers up to its `UNTIL`, or the whole future when it has none.
    #[must_use]
    pub fn covered_through(self) -> Option<CivilDate> {
        match self.rule {
            None => Some(self.start.date()),
            Some(rule) => rule.through(),
        }
    }
}

/// One zone's transitions, sorted, bounded, and honest about where its data stops.
///
/// Built where untrusted input is read and immutable afterwards, which is what lets
/// [`ZoneSource`] answer without a meter: the only unbounded quantity a `VTIMEZONE` has is how
/// many transitions it declares, and that is charged here, once.
///
/// [`ZoneSource`]: crate::ZoneSource
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionTable {
    /// The identifier this table answers to, compared by exact bytes.
    tzid: Box<str>,
    /// The observances, ascending by the instant they begin.
    observances: Vec<Observance>,
    /// The observances that repeat by a rule, in the same order.
    ///
    /// A second list rather than a filter at every lookup. A rule is in force from its own
    /// `DTSTART` until something later supersedes it, so *every* rule in a definition has to be
    /// asked about a query however many dated transitions were written between the two — which
    /// is what a window over the sorted table got wrong, and what an `RDATE` list restating two
    /// years of a zone that moves twice a year was enough to trigger.
    rules: Vec<Observance>,
    /// Whether observances were dropped to hold the caller's bound.
    truncated: bool,
    /// The last date backed by real data, absent when every kind of observance repeats forever.
    coverage_end: Option<CivilDate>,
    /// The date of the earliest transition this table records, absent when it records none.
    coverage_start: Option<CivilDate>,
}

impl TransitionTable {
    /// A table for `tzid` over `observances`, charged to `meter`.
    ///
    /// The observances are sorted here rather than required sorted, because they arrive from
    /// several properties of several subcomponents and no producer orders them: `RDATE` lines
    /// interleave with rule anchors and neither knows about the other. That is the one place
    /// this crate differs from `ical-recur`, which requires its caller-supplied lists sorted —
    /// there the caller holds one list and sorting it silently would hide a cost the caller
    /// could have avoided, and here nobody holds one list at all.
    ///
    /// Observances past `Limits::max_vtimezone_observances` are dropped from the end, which
    /// leaves the table's coverage ending earlier rather than leaving it with a hole, and
    /// [`DiagnosticCode::VtimezoneObservancesTruncated`] says so.
    ///
    /// The order is the instant each observance *begins*, which is its own `DTSTART` read
    /// against its own `TZOFFSETFROM`, and never the wall clock that `DTSTART` spells. Two
    /// facts follow and both were wrong before. A definition whose `DAYLIGHT` and `STANDARD`
    /// subcomponents declare one wall clock — legal, and written by producers that state a
    /// transition from both sides — sorted by an equal key, so which one the table held first
    /// was `sort_unstable`'s business and the answer changed with the order the producer wrote
    /// them in. And an observance whose `TZOFFSETFROM` is further east than the previous one's
    /// begins *earlier* than a later wall clock suggests, which left the binary search below
    /// placing a query among onsets that did not ascend.
    #[must_use]
    pub fn new<S: DiagnosticSink + ?Sized>(
        tzid: Box<str>,
        observances: Vec<Observance>,
        meter: &mut Meter,
        sink: &mut S,
    ) -> Self {
        let mut kept = observances;
        // The whole observance breaks the tie, so two beginning at one instant are ordered by
        // what they say rather than by where they were written.
        kept.sort_unstable_by_key(|observance| (onset_of(*observance), *observance));
        let admitted = admitted_count(kept.len(), meter);
        let truncated = admitted < kept.len();
        kept.truncate(admitted);
        if truncated {
            report_diagnostic(
                sink,
                meter,
                Diagnostic::new(
                    DiagnosticCode::VtimezoneObservancesTruncated,
                    Severity::LimitReached,
                    Location::NOWHERE,
                ),
            );
        }
        let coverage_end = coverage_end_of(&kept);
        let coverage_start = kept.first().map(|first| first.start().date());
        let rules = kept
            .iter()
            .filter(|observance| observance.rule().is_some())
            .copied()
            .collect();
        Self {
            tzid,
            observances: kept,
            rules,
            truncated,
            coverage_end,
            coverage_start,
        }
    }

    /// The identifier this table answers to.
    #[must_use]
    pub fn tzid(&self) -> Tzid<'_> {
        Tzid::new(&self.tzid)
    }

    /// The observances, ascending by the instant they begin.
    #[must_use]
    pub fn observances(&self) -> &[Observance] {
        &self.observances
    }

    /// The observances that repeat by a rule, in the same order.
    ///
    /// Every one of them is asked about every query, because a rule is in force from its own
    /// `DTSTART` until a later observance supersedes it and no count of dated transitions
    /// written beside it changes that. A definition holds a handful — two for an ordinary zone,
    /// four for one that kept the rules a government replaced — so "every one of them" is a
    /// constant a lookup can afford, which is what `docs/adr/0010` asks of this path.
    #[must_use]
    pub fn rules(&self) -> &[Observance] {
        &self.rules
    }

    /// Whether observances were dropped to hold the caller's bound.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// The last date backed by real data, absent when this zone knows the future.
    ///
    /// `None` means every kind of observance this definition carries repeats by a rule with no
    /// `UNTIL`, so no answer this table gives is ever an extrapolation. A date means every
    /// question after it is answered by continuing the final observance, and carries
    /// [`AnswerBasis::BeyondKnownTransitions`] saying so.
    ///
    /// "Every kind" is `STANDARD` against `DAYLIGHT`, and it is the correction M2 made here. A
    /// single endless rule used to make the whole table claim to know the future, so a
    /// definition whose daylight rule runs forever and whose standard transitions are three
    /// `RDATE` lines ending in 2029 answered midwinter 2031 with permanent summer time and
    /// called it computed. Half of that table ran out; a zone that cannot say when its summer
    /// ends does not know its own future, whatever its other half states.
    ///
    /// [`AnswerBasis::BeyondKnownTransitions`]: crate::AnswerBasis::BeyondKnownTransitions
    #[must_use]
    pub const fn coverage_end(&self) -> Option<CivilDate> {
        self.coverage_end
    }

    /// The date of the earliest transition this table records, absent when it records none.
    ///
    /// A table has two ends. A question before this date is answered by extending the earliest
    /// observance's `TZOFFSETFROM` backwards forever — the whole of what the file states about
    /// that era — and carries [`AnswerBasis::BeforeKnownTransitions`] saying so.
    ///
    /// [`AnswerBasis::BeforeKnownTransitions`]: crate::AnswerBasis::BeforeKnownTransitions
    #[must_use]
    pub const fn coverage_start(&self) -> Option<CivilDate> {
        self.coverage_start
    }

    /// Whether this table declares no observance at all.
    ///
    /// RFC 5545 section 3.6.5 requires at least one, so such a table came from a file that
    /// violated it, and it answers nothing rather than answering UTC.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.observances.is_empty()
    }
}

/// How many of `offered` observances the caller's policy admits.
///
/// Charged one at a time rather than as a block, because the answer wanted here is *where* the
/// bound falls and a block charge only says that it does. The loop is bounded by the offered
/// count, which is itself bounded by the parse that produced it.
fn admitted_count(offered: usize, meter: &mut Meter) -> usize {
    let mut admitted = 0_usize;
    while admitted < offered {
        if meter.try_charge_vtimezone_observances(1).is_err() {
            break;
        }
        admitted = admitted.saturating_add(1);
    }
    admitted
}

/// The instant an observance's own `DTSTART` names, which is the order a table is kept in.
///
/// RFC 5545 section 3.6.5 reads that `DTSTART` against `TZOFFSETFROM`: the transition happens
/// when the clock that is still running reaches that wall time. `None` for a wall clock whose
/// instant the timeline cannot express, which sorts before every observance that has one — the
/// same place a date before the timeline would sort.
fn onset_of(observance: Observance) -> Option<Instant> {
    observance.start().at_offset(observance.offset_from())
}

/// The last date `observances` are backed by real data for.
///
/// `None` only when every *side* of the definition repeats forever, where a side is `STANDARD`
/// against `DAYLIGHT`. A zone knows its own future when it can say both when summer begins and
/// when it ends; a definition with an endless daylight rule and a finite list of standard
/// onsets knows neither past the last of those onsets, because after it the alternation the
/// zone runs on has one half missing.
///
/// Where some side does run out, the answer is the *latest* date any observance states
/// outright, and not the earliest. A flat table of two dated transitions covers everything
/// between them and stops after the second; taking the earlier of the two would report a
/// July inside the table as an extrapolation past its end.
fn coverage_end_of(observances: &[Observance]) -> Option<CivilDate> {
    if sides_of(observances)
        .into_iter()
        .filter(|side| side.present)
        .all(|side| side.endless)
    {
        return None;
    }
    let mut latest: Option<CivilDate> = None;
    for covered in observances.iter().filter_map(|held| held.covered_through()) {
        latest = Some(match latest {
            Some(known) if known >= covered => known,
            _ => covered,
        });
    }
    latest
}

/// What one side of a definition — its `STANDARD` or its `DAYLIGHT` observances — states.
#[derive(Clone, Copy, Debug)]
struct Side {
    /// Whether the definition carries an observance of this kind at all.
    present: bool,
    /// Whether one of them repeats by a rule with no `UNTIL`.
    endless: bool,
}

/// The two sides of `observances`, as [`Side`] reads them.
fn sides_of(observances: &[Observance]) -> [Side; 2] {
    let mut sides = [Side {
        present: false,
        endless: false,
    }; 2];
    for observance in observances {
        let Some(side) = sides.get_mut(usize::from(observance.daylight())) else {
            continue;
        };
        side.present = true;
        side.endless |= observance.covered_through().is_none();
    }
    sides
}

/// Why a `VTIMEZONE` definition was not admitted into a set.
///
/// The rejected table travels inside the error rather than being dropped, so a caller holding
/// one still holds the definition the file wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZoneSetError {
    /// The set already holds as many zones as the caller's policy admits.
    TooMany(TransitionTable, LimitExceeded),
}

impl ZoneSetError {
    /// The definition that was not admitted.
    #[must_use]
    pub const fn table(&self) -> &TransitionTable {
        match self {
            Self::TooMany(table, _) => table,
        }
    }

    /// The code an emitter reports this refusal under.
    ///
    /// The caller's own bound refused a definition the file carries, which is a fact about the
    /// wiring rather than about the file — and one that has to travel, because the identifiers
    /// the refused definition declares are otherwise indistinguishable from identifiers the
    /// calendar never defined. `docs/adr/0003` amendment 6 said this refusal carried no code;
    /// M2 found what the silence cost and gave it one.
    #[must_use]
    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        match self {
            Self::TooMany(_, _) => DiagnosticCode::VtimezoneComponentsTruncated,
        }
    }
}

/// How a definition was taken into a set.
///
/// The answer `VtimezoneSet::insert` gives where it used to hand a second definition back. A
/// calendar declaring one `TZID` twice has two readings of one zone and RFC 5545 section 3.6.5
/// forbids it; dropping either is how a file with two readings acquires one nobody chose, and
/// dropping the *second* meant an empty placeholder written above a full definition erased it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ZoneAdmission {
    /// The only definition of its identifier so far.
    Sole,
    /// A second or later definition of an identifier already in the set. Both stay reachable.
    Repeated,
}

impl ZoneAdmission {
    /// The code an emitter reports this admission under, absent for the ordinary case.
    #[must_use]
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode> {
        match self {
            Self::Sole => None,
            Self::Repeated => Some(DiagnosticCode::DuplicateTimeZoneIdentifier),
        }
    }
}

/// The zone definitions one calendar carries.
///
/// Kept sorted by identifier so that a lookup is a binary search over exact bytes, which is
/// the comparison `docs/adr/0003` requires and the only one that keeps identifier aliasing the
/// caller's visible step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VtimezoneSet {
    /// The tables, ascending by identifier.
    tables: Vec<TransitionTable>,
}

impl VtimezoneSet {
    /// A set holding nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self { tables: Vec::new() }
    }

    /// Admit `table`, charged to `meter`, saying whether its identifier was already here.
    ///
    /// A second definition of one identifier is admitted beside the first, in the order the
    /// file wrote them, and [`ZoneAdmission::Repeated`] carries the code that says so. The
    /// refusal this used to make cost more than it was worth: the file's second reading became
    /// unreachable from the value that came back, and a calendar opening with an empty
    /// placeholder `VTIMEZONE` had its real definition thrown away as the duplicate.
    ///
    /// The only refusal left is the caller's own zone-count bound.
    pub fn insert(
        &mut self,
        table: TransitionTable,
        meter: &mut Meter,
    ) -> Result<ZoneAdmission, ZoneSetError> {
        if let Err(breach) = meter.try_charge_vtimezone_component() {
            return Err(ZoneSetError::TooMany(table, breach));
        }
        let held = self.definitions(table.tzid().as_str()).len();
        let admission = if held == 0 {
            ZoneAdmission::Sole
        } else {
            ZoneAdmission::Repeated
        };
        // Past the last definition of this identifier, so equal keys keep file order and a
        // reader taking the first of them takes the one the producer wrote first.
        let index = self
            .tables
            .partition_point(|held| held.tzid().as_str() <= table.tzid().as_str());
        self.tables.insert(index, table);
        Ok(admission)
    }

    /// The definition for `tzid` a lookup answers with, compared by exact bytes.
    ///
    /// The first definition that carries a transition, and the first definition otherwise. A
    /// file may declare one identifier twice, and where it does the earlier reading is the one
    /// this answers with — except that a definition carrying no observance is not a reading of
    /// a zone at all, and letting an empty placeholder shadow the definition beside it would
    /// hide a zone the file states in full. [`VtimezoneSet::definitions`] is where both stay
    /// reachable, and that is the accessor a caller comparing two readings wants.
    #[must_use]
    pub fn table(&self, tzid: &str) -> Option<&TransitionTable> {
        let held = self.definitions(tzid);
        held.iter()
            .find(|table| !table.is_empty())
            .or_else(|| held.first())
    }

    /// Every definition of `tzid`, in the order the calendar wrote them.
    #[must_use]
    pub fn definitions(&self, tzid: &str) -> &[TransitionTable] {
        let first = self
            .tables
            .partition_point(|table| table.tzid().as_str() < tzid);
        let past = self
            .tables
            .partition_point(|table| table.tzid().as_str() <= tzid);
        self.tables.get(first..past).unwrap_or(&[])
    }

    /// Every definition, ascending by identifier and in file order within one.
    #[must_use]
    pub fn tables(&self) -> &[TransitionTable] {
        &self.tables
    }

    /// How many zones this set holds.
    ///
    /// Identifiers, not definitions: a calendar declaring `Europe/Berlin` twice carries one
    /// zone and two readings of it, and [`VtimezoneSet::tables`] is where the second one is.
    #[must_use]
    pub fn len(&self) -> usize {
        let mut counted = 0_usize;
        let mut previous: Option<&str> = None;
        for table in &self.tables {
            let name = table.tzid().as_str();
            if previous != Some(name) {
                counted = counted.saturating_add(1);
                previous = Some(name);
            }
        }
        counted
    }

    /// Whether this set holds no zone at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

/// Reading one `VTIMEZONE` out of whatever holds it.
///
/// A trait rather than an inherent constructor because the thing that holds a `VTIMEZONE` is
/// `ical_core::Component`, and a caller with its own representation — a database row, a
/// hand-built definition — should be able to feed this crate without going through the model.
/// The identifier comes back separately from the observances because a `VTIMEZONE` with no
/// `TZID` is a component this crate cannot file anywhere, which is a different answer from one
/// that declared a zone with no transitions.
pub trait ObservanceReader {
    /// Append this definition's observances to `out` and answer with its identifier.
    ///
    /// `None` when there is no usable `TZID`. Everything else survivable — an observance whose
    /// rule this crate does not evaluate, a definition with no observance at all — is a
    /// diagnostic on `sink` and a shorter list in `out`.
    fn read_vtimezone(
        &self,
        meter: &mut Meter,
        sink: &mut dyn DiagnosticSink,
        out: &mut Vec<Observance>,
    ) -> Option<Box<str>>;
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use ical_core::{
        CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, IgnoreDiagnostics, Limits,
        Meter, UtcOffset, Weekday,
    };

    use super::{
        NthWeek, Observance, RuleDay, TransitionTable, VtimezoneSet, YearlyRule, ZoneAdmission,
        ZoneSetError,
    };

    fn two_am() -> CivilTime {
        CivilTime::from_hms(2, 0, 0).unwrap()
    }

    fn stamp(year: u16, month: u8, day: u8) -> CivilDateTime {
        CivilDateTime::new(CivilDate::from_ymd(year, month, day).unwrap(), two_am())
    }

    fn offset(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap()
    }

    fn dated(year: u16, month: u8, day: u8) -> Observance {
        Observance::new(
            stamp(year, month, day),
            offset(3600),
            offset(7200),
            true,
            None,
        )
    }

    fn ruled(through: Option<CivilDate>) -> Observance {
        let rule = YearlyRule::new(
            3,
            RuleDay::Nth {
                weekday: Weekday::Sunday,
                week: NthWeek::Last,
            },
            two_am(),
            through,
        )
        .unwrap();
        Observance::new(
            stamp(2007, 3, 25),
            offset(3600),
            offset(7200),
            true,
            Some(rule),
        )
    }

    fn table(observances: Vec<Observance>, meter: &mut Meter) -> TransitionTable {
        TransitionTable::new(
            Box::from("Europe/Berlin"),
            observances,
            meter,
            &mut IgnoreDiagnostics,
        )
    }

    #[test]
    fn a_month_no_year_has_is_not_a_rule() {
        assert_eq!(
            YearlyRule::new(13, RuleDay::LastDayOfMonth, two_am(), None),
            None
        );
        assert_eq!(
            YearlyRule::new(0, RuleDay::LastDayOfMonth, two_am(), None),
            None
        );
        assert!(YearlyRule::new(12, RuleDay::DayOfMonth(31), two_am(), None).is_some());
    }

    /// Observances arrive from several properties of several subcomponents and no producer
    /// orders them, so the table does.
    #[test]
    fn a_table_sorts_what_it_is_handed() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let built = table(
            alloc::vec![dated(2029, 3, 25), dated(2027, 3, 28), dated(2028, 3, 26)],
            &mut meter,
        );
        let starts: Vec<u16> = built
            .observances()
            .iter()
            .map(|observance| observance.start().date().year())
            .collect();
        assert_eq!(starts, alloc::vec![2027, 2028, 2029]);
        assert!(!built.is_truncated());
        assert_eq!(built.tzid().as_str(), "Europe/Berlin");
    }

    /// The `RDATE` table that runs out, which is the input this whole design turns on.
    #[test]
    fn a_date_driven_table_stops_where_its_dates_do_and_a_rule_driven_one_does_not() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let finite = table(
            alloc::vec![dated(2027, 3, 28), dated(2029, 3, 25)],
            &mut meter,
        );
        assert_eq!(
            finite.coverage_end(),
            CivilDate::from_ymd(2029, 3, 25),
            "an explicit table covers its last date and nothing after it"
        );

        let endless = table(alloc::vec![dated(2027, 3, 28), ruled(None)], &mut meter);
        assert_eq!(
            endless.coverage_end(),
            None,
            "one rule with no UNTIL means the zone knows the future"
        );

        let bounded = CivilDate::from_ymd(2035, 12, 31);
        let expiring = table(alloc::vec![ruled(bounded), dated(2027, 3, 28)], &mut meter);
        assert_eq!(expiring.coverage_end(), bounded);
    }

    /// A bound nobody charges is decoration: a million `RDATE` transitions is a file somebody
    /// can write, and the table that reads one says it was cut.
    #[test]
    fn observances_past_the_caller_s_bound_are_dropped_from_the_end_and_reported() {
        let limits = Limits::DEFAULT.with_max_vtimezone_observances(2);
        let mut meter = Meter::new(limits);
        let mut reported: Vec<Diagnostic> = Vec::new();
        let built = TransitionTable::new(
            Box::from("Europe/Berlin"),
            alloc::vec![dated(2029, 3, 25), dated(2027, 3, 28), dated(2028, 3, 26)],
            &mut meter,
            &mut reported,
        );
        assert_eq!(built.observances().len(), 2);
        assert!(built.is_truncated());
        assert_eq!(
            built.coverage_end(),
            CivilDate::from_ymd(2028, 3, 26),
            "coverage ends earlier rather than the table acquiring a hole"
        );
        assert_eq!(
            reported.first().map(|entry| entry.code()),
            Some(DiagnosticCode::VtimezoneObservancesTruncated)
        );
    }

    /// A duplicate identifier is a reported fact and a second reading kept, never a lost one
    /// and never a silently preferred one.
    #[test]
    fn a_second_definition_of_one_zone_is_kept_beside_the_first_and_reported() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut set = VtimezoneSet::new();
        assert!(set.is_empty());
        assert_eq!(
            set.insert(table(Vec::new(), &mut meter), &mut meter),
            Ok(ZoneAdmission::Sole)
        );
        assert_eq!(
            set.insert(table(Vec::new(), &mut meter), &mut meter),
            Ok(ZoneAdmission::Repeated),
            "the second definition is admitted beside the first, not handed back"
        );
        assert_eq!(
            ZoneAdmission::Repeated.diagnostic_code(),
            Some(DiagnosticCode::DuplicateTimeZoneIdentifier)
        );
        assert_eq!(set.len(), 1, "one identifier");
        assert_eq!(
            set.definitions("Europe/Berlin").len(),
            2,
            "two readings of it, both reachable"
        );
        assert_eq!(set.tables().len(), 2);
        assert!(set.table("Europe/Berlin").is_some());
        assert!(
            set.table("europe/berlin").is_none(),
            "lookup is by exact bytes, which is what keeps aliasing the caller's step"
        );
    }

    /// A definition carrying nothing may not shadow one carrying the zone's own rules, whatever
    /// order the file wrote the two in.
    #[test]
    fn an_empty_definition_does_not_shadow_a_definition_that_holds_transitions() {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut set = VtimezoneSet::new();
        assert!(
            set.insert(table(Vec::new(), &mut meter), &mut meter)
                .is_ok()
        );
        let real = table(alloc::vec![dated(2027, 3, 28)], &mut meter);
        assert!(set.insert(real, &mut meter).is_ok());
        assert_eq!(
            set.table("Europe/Berlin").map(TransitionTable::is_empty),
            Some(false),
            "the placeholder was written first and the definition is what answers"
        );
        assert_eq!(set.definitions("Europe/Berlin").len(), 2);
    }

    #[test]
    fn a_zone_count_past_the_caller_s_bound_is_refused_under_its_own_dimension() {
        let limits = Limits::DEFAULT.with_max_vtimezone_components(1);
        let mut meter = Meter::new(limits);
        let mut set = VtimezoneSet::new();
        assert_eq!(
            set.insert(table(Vec::new(), &mut meter), &mut meter),
            Ok(ZoneAdmission::Sole)
        );
        let second = TransitionTable::new(
            Box::from("America/New_York"),
            Vec::new(),
            &mut meter,
            &mut IgnoreDiagnostics,
        );
        let refused = set.insert(second, &mut meter);
        assert!(matches!(refused, Err(ZoneSetError::TooMany(_, _))));
        assert_eq!(set.len(), 1);
    }
}
