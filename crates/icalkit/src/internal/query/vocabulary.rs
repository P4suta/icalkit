// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The types every unit of this crate shares, frozen before any of them was written.
//!
//! Nothing here evaluates anything. What it owns is the vocabulary the evaluation is stated in:
//! the three-valued answer a filter produces, the refusal that ends an evaluation, the
//! sub-selection result and the witness that says it is not the resource the server stored, the
//! free-busy report and its bound, and the seam by which a caller hands in a zone source.
//!
//! # Why the answer has three values
//!
//! [`ADR 0003`](https://github.com/P4suta/icalkit/blob/main/docs/adr/0003-caller-supplied-time-zones.md)
//! forbids inventing a zone. A floating `DTSTART` compared against a `time-range` has no place
//! on the timeline until something says which zone to read it in, and if the query carried no
//! `CALDAV:timezone` and no source recognizes the `TZID`, nothing does. An evaluator that
//! answered "no match" there would be reporting an absence it never established — a resource
//! that is in the window would be missing from the `REPORT`, and the client would have no way
//! to tell that from a resource that is genuinely outside it.
//!
//! So [`Match::Undecided`] is a value, it composes through the boolean operators by Kleene's
//! rules, and it reaches the caller. A server that wants the two-valued answer takes it by
//! deciding what an undecided resource means to *it*, which is a policy and not a fact.

use core::error::Error;
use core::fmt::{self, Debug, Display, Formatter};

use alloc::vec::Vec;

use ical_core::{CivilDateTime, DiagnosticCode, Document, Instant, LimitExceeded, Limits, Meter};
use ical_dav::Collation;
use ical_recur::SearchOutcome;
use ical_tz::{OffsetAnswer, ResolutionPolicy, ZoneAnswer, ZoneSource};

/// Whether a resource satisfies a filter, and the answer an evaluator may not invent.
///
/// Three values rather than two, for the reason this module's own documentation gives. The
/// combinators below are Kleene's strong three-valued logic, which is the reading that makes
/// "undecided" behave like "one of matched or unmatched and I do not know which": a conjunction
/// with an unmatched operand is unmatched however undecided the other one was, because no
/// reading of the undecided operand could have rescued it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Match {
    /// The filter is satisfied.
    Matched,
    /// The filter is not satisfied.
    Unmatched,
    /// The filter could not be decided, and this says why.
    Undecided(Undecided),
}

impl Match {
    /// The answer a decidable test produced.
    #[must_use]
    pub const fn of(matched: bool) -> Self {
        if matched {
            Self::Matched
        } else {
            Self::Unmatched
        }
    }

    /// Whether the filter is satisfied. `false` for an undecided answer, which is why no
    /// caller should route a decision through it without reading [`Match::undecided`] first.
    #[must_use]
    pub const fn is_matched(self) -> bool {
        matches!(self, Self::Matched)
    }

    /// Why the answer could not be decided, or `None` when it was.
    #[must_use]
    pub const fn undecided(self) -> Option<Undecided> {
        match self {
            Self::Undecided(reason) => Some(reason),
            Self::Matched | Self::Unmatched => None,
        }
    }

    /// Both, as RFC 4791 section 9.7.1 composes the tests inside one filter.
    ///
    /// An unmatched operand decides the conjunction whatever the other one was: every reading
    /// of an undecided operand leaves the conjunction unmatched, so the answer is a fact rather
    /// than a guess. Two undecided operands keep the first one's reason, because a caller
    /// reading one reason acts the same way on either.
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unmatched, _) | (_, Self::Unmatched) => Self::Unmatched,
            (Self::Undecided(reason), _) | (Self::Matched, Self::Undecided(reason)) => {
                Self::Undecided(reason)
            },
            (Self::Matched, Self::Matched) => Self::Matched,
        }
    }

    /// Either, as RFC 4791 section 9.7.1 composes the sibling filters of one component.
    ///
    /// The dual of [`Match::and`]: a matched operand decides the disjunction whatever the other
    /// one was.
    #[must_use]
    pub const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Matched, _) | (_, Self::Matched) => Self::Matched,
            (Self::Undecided(reason), _) | (Self::Unmatched, Self::Undecided(reason)) => {
                Self::Undecided(reason)
            },
            (Self::Unmatched, Self::Unmatched) => Self::Unmatched,
        }
    }

    /// The negation `CALDAV:is-not-defined` and a `text-match`'s `negate-condition` take.
    ///
    /// An undecided answer negates to itself. Negating it to "matched" would be the same
    /// invention the third value exists to refuse, one operator further along.
    #[must_use]
    pub const fn negate(self) -> Self {
        match self {
            Self::Matched => Self::Unmatched,
            Self::Unmatched => Self::Matched,
            Self::Undecided(reason) => Self::Undecided(reason),
        }
    }
}

/// Why a filter could not be decided.
///
/// Every variant is a question the evaluator was asked and could not answer without inventing
/// something — a zone, a value, or work past the caller's budget. None of them is a fault in
/// the resource or in the query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Undecided {
    /// A floating value had to be placed on a timeline and the query stated no zone.
    ///
    /// RFC 4791 section 9.9 makes `CALDAV:timezone` the zone a floating `time-range` comparison
    /// is made in, and permits a query to carry none.
    ZoneUnstated,
    /// A value named a `TZID` that no source the caller supplied recognizes.
    ZoneUnknown,
    /// A wall clock fell in a gap or a fold and the caller's policy dropped it.
    ///
    /// Distinct from an unknown zone: the zone answered, and what it answered was "this local
    /// time names no instant" or "it names two". Which one to take is a caller's policy
    /// ([`ResolutionPolicy`]), and a policy that declines to choose leaves the comparison with
    /// no instant to make.
    ZoneAmbiguous,
    /// A recurrence search reached its budget before it reached the end of the window.
    ///
    /// The resource may or may not have an occurrence inside the window; the search stopped
    /// before it could say. `docs/adr/0002` makes this a reported outcome rather than a
    /// truncated answer, and this is that outcome one layer up.
    SearchExhausted,
    /// A value the comparison needs was present and did not decode.
    ValueUnreadable,
    /// RFC 4791 section 9.9 states no overlap rule for the component this filter named.
    ///
    /// The table is per component type and closed. A `time-range` on a component outside it —
    /// a `VTIMEZONE`, or a component type this workspace has no reading for — is a question
    /// section 9.9 does not define an answer to, and guessing one would make this crate
    /// disagree with a conformant server about which resources a query returns.
    OverlapUndefined,
}

impl Undecided {
    /// The code every reason travels to a diagnostic sink under.
    ///
    /// One code for all six: a caller acts on "the answer is not a fact" and the reason is prose
    /// beside it. Splitting it into six codes would freeze six meanings (`docs/adr/0009`) to say
    /// one thing. An associated constant rather than a method, because it does not depend on
    /// which reason it is and a method taking `self` would suggest that it might.
    pub const CODE: DiagnosticCode = DiagnosticCode::QueryFilterUndecidable;

    /// The reason an incomplete recurrence search gives, or `None` when it was complete.
    ///
    /// The one place `ical-recur`'s terminal state crosses into this crate's answer. A search
    /// that stopped at its budget did not establish that the resource has no occurrence in the
    /// window; it established that it did not find one before it ran out, and those are
    /// different claims (`docs/adr/0002`).
    #[must_use]
    pub const fn of_search(outcome: SearchOutcome) -> Option<Self> {
        if outcome.is_complete() {
            None
        } else {
            Some(Self::SearchExhausted)
        }
    }

    /// What could not be answered, as one clause of prose.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::ZoneUnstated => "a floating value and no zone stated by the query",
            Self::ZoneUnknown => "a TZID no supplied source recognizes",
            Self::ZoneAmbiguous => "a wall clock the zone repeats or does not show",
            Self::SearchExhausted => "a recurrence search stopped at its budget",
            Self::ValueUnreadable => "a value that is present and does not decode",
            Self::OverlapUndefined => "a component RFC 4791 section 9.9 states no rule for",
        }
    }
}

impl Display for Undecided {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "the filter is undecidable: {}", self.reason())
    }
}

/// A refusal that ends an evaluation.
///
/// Nothing here is recoverable by ignoring it, which is the split `docs/adr/0009` draws and
/// `ical-dav`'s `DavError` draws one layer down. An undecidable comparison is *not* an error:
/// it is [`Match::Undecided`], and it travels as an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum QueryError {
    /// A caller-stated bound was crossed, and the dimension says which one.
    Limit(LimitExceeded),
    /// A filter states a condition and its own negation, RFC 4791 section 9.7.1.
    ///
    /// `ical-dav` can represent such a filter and reports it through `is_contradictory`; this
    /// crate refuses to evaluate one, because a component that is not defined has no time range
    /// and no properties and there is no answer to give.
    Contradictory,
    /// The filter names a collation this crate does not implement, RFC 4791 section 7.5.
    ///
    /// A server answers this with the `CALDAV:supported-collation` precondition rather than
    /// falling back to a collation the client did not ask for, because a substring test under
    /// the wrong collation silently returns the wrong resources.
    UnsupportedCollation,
    /// A `CALDAV:comp` selection states "every property" and names properties beside it.
    ///
    /// RFC 4791 section 9.6.1 writes `comp ((allprop | prop*), (allcomp | comp*))`, so the two
    /// halves of each pair are alternatives and a value holding both is one no body expresses.
    SelectionContradiction,
    /// A calendar this crate had to build could not be written back as iCalendar.
    ///
    /// Sub-selection and expansion both construct a calendar rather than copying one, and
    /// `docs/adr/0001` requires what comes out to be a document that serializes. A value that
    /// cannot be spelled — an instant outside the four-digit years RFC 5545 section 3.3.5
    /// writes, say — is refused rather than clamped to one that can.
    Unrepresentable,
}

impl From<LimitExceeded> for QueryError {
    fn from(exceeded: LimitExceeded) -> Self {
        Self::Limit(exceeded)
    }
}

impl Display for QueryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Limit(exceeded) => write!(formatter, "{exceeded}"),
            Self::Contradictory => {
                formatter.write_str("the filter states a condition and its own negation")
            },
            Self::UnsupportedCollation => {
                formatter.write_str("the filter names a collation this crate does not implement")
            },
            Self::SelectionContradiction => formatter.write_str(
                "the selection names properties beside allprop, or components beside \
                            allcomp",
            ),
            Self::Unrepresentable => {
                formatter.write_str("the calendar to return holds a value iCalendar cannot write")
            },
        }
    }
}

impl Error for QueryError {}

/// The comparison one `CALDAV:text-match` is made under, RFC 4791 section 7.5.
///
/// A crate-owned classification of `ical-dav`'s [`Collation`] rather than that type re-used,
/// because the two answer different questions. `Collation` records what the peer wrote,
/// extension names included, so a request survives a read and a re-encode. This records what
/// this crate can actually compare with, and the mapping between them is total in one direction
/// only: a collation with no row here is [`QueryError::UnsupportedCollation`], which is the
/// answer RFC 4791 section 7.5.1 gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Collator {
    /// `i;ascii-casemap`, the default: case-folded over ASCII and octet-exact elsewhere.
    #[default]
    AsciiCasemap,
    /// `i;octet`: octet-exact everywhere.
    Octet,
}

impl Collator {
    /// The comparison this crate makes for `collation`, or `None` for one it does not make.
    ///
    /// `None` and never a fallback. A substring test run under a collation the client did not
    /// ask for returns a different set of resources, silently, and RFC 4791 section 7.5.1 gives
    /// a server the `CALDAV:supported-collation` precondition precisely so it does not have to
    /// guess.
    #[must_use]
    pub fn of(collation: &Collation) -> Option<Self> {
        match *collation {
            Collation::AsciiCasemap => Some(Self::AsciiCasemap),
            Collation::Octet => Some(Self::Octet),
            Collation::Other(_) => None,
        }
    }

    /// The collation name this comparison is written as.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::AsciiCasemap => b"i;ascii-casemap",
            Self::Octet => b"i;octet",
        }
    }
}

/// What a sub-selection or an expansion left out of the resource it was taken from.
///
/// The witness that `docs/adr/0001`'s round trip is broken on purpose. Nothing about the octets
/// of a reduced calendar says they are a reduction — they are well-formed iCalendar — so a
/// caller that wrote them back would delete whatever was left out. This value travels beside
/// them, and [`DiagnosticCode::QueryCalendarDataReduced`] travels to the sink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reduction {
    /// `CALDAV:comp` or `CALDAV:prop` kept a subtree rather than the whole object.
    pub components_dropped: bool,
    /// `CALDAV:limit-recurrence-set` or `CALDAV:limit-freebusy-set` kept part of a series.
    pub instances_dropped: bool,
    /// `CALDAV:expand` replaced the recurrence rule with the instances it generates.
    pub expanded: bool,
}

impl Reduction {
    /// Nothing was left out: what came back is what the server stored.
    pub const FAITHFUL: Self = Self {
        components_dropped: false,
        instances_dropped: false,
        expanded: false,
    };

    /// Whether the calendar is the one the server stored, octet differences aside.
    #[must_use]
    pub const fn is_faithful(self) -> bool {
        !self.components_dropped && !self.instances_dropped && !self.expanded
    }

    /// The code a reduction is reported under, or `None` when there was none.
    #[must_use]
    pub const fn code(self) -> Option<DiagnosticCode> {
        if self.is_faithful() {
            None
        } else {
            Some(DiagnosticCode::QueryCalendarDataReduced)
        }
    }
}

/// The calendar a `CALDAV:calendar-data` request asks to be returned, and what it is not.
///
/// The two are one value because separating them is how the second gets dropped. A function
/// answering a bare `Document` would hand a caller something indistinguishable from the stored
/// resource, and the caller would write it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The calendar to return.
    calendar: Document,
    /// What was left out of the resource it was taken from.
    reduction: Reduction,
}

impl Selection {
    /// A selection of `calendar` that left out what `reduction` says.
    #[must_use]
    pub const fn new(calendar: Document, reduction: Reduction) -> Self {
        Self {
            calendar,
            reduction,
        }
    }

    /// The calendar to return.
    #[must_use]
    pub const fn calendar(&self) -> &Document {
        &self.calendar
    }

    /// What was left out.
    #[must_use]
    pub const fn reduction(&self) -> Reduction {
        self.reduction
    }

    /// The calendar, given up along with the witness.
    ///
    /// Named rather than a `From` impl, so that discarding the reduction is something a reader
    /// of the call site sees happening.
    #[must_use]
    pub fn into_calendar(self) -> Document {
        self.calendar
    }
}

/// Which `FBTYPE` a busy period carries, RFC 5545 section 3.2.9.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum BusyType {
    /// `BUSY`, the default the parameter's own definition gives.
    #[default]
    Busy,
    /// `BUSY-UNAVAILABLE`.
    Unavailable,
    /// `BUSY-TENTATIVE`.
    Tentative,
    /// `FREE`, which a `VFREEBUSY` may state and a report generated from events does not.
    Free,
}

impl BusyType {
    /// The parameter value this type is written as.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Busy => b"BUSY",
            Self::Unavailable => b"BUSY-UNAVAILABLE",
            Self::Tentative => b"BUSY-TENTATIVE",
            Self::Free => b"FREE",
        }
    }

    /// Classify an `FBTYPE` value, or `None` for one section 3.2.9 does not define.
    ///
    /// `None` rather than a default, because an `FBTYPE` this crate has no row for is an
    /// extension whose meaning is the extender's, and reading it as `BUSY` would report time as
    /// unavailable on a guess.
    #[must_use]
    pub fn parse(value: &[u8]) -> Option<Self> {
        [Self::Busy, Self::Unavailable, Self::Tentative, Self::Free]
            .into_iter()
            .find(|candidate| candidate.as_bytes().eq_ignore_ascii_case(value))
    }
}

/// One period of a free-busy report, half-open like every window in this workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BusyPeriod {
    /// The first instant inside the period.
    pub start: Instant,
    /// The first instant after it.
    pub end: Instant,
    /// What the time is, RFC 5545 section 3.2.9.
    pub kind: BusyType,
}

impl BusyPeriod {
    /// A period of `kind` running from `start` up to but not including `end`.
    #[must_use]
    pub const fn new(start: Instant, end: Instant, kind: BusyType) -> Self {
        Self { start, end, kind }
    }

    /// Whether the period states no time at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.end.unix_seconds() <= self.start.unix_seconds()
    }
}

/// The answer to a `CALDAV:free-busy-query`, RFC 4791 section 7.10.
///
/// Bounded by `Limits::max_freebusy_periods`, which is charged on the way in rather than
/// checked at the end: a report is generated from a whole collection, so the number of periods
/// is chosen by the data and not by the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeBusyReport {
    /// The periods, in the order they were pushed.
    periods: Vec<BusyPeriod>,
    /// The window the report is about.
    window: (Option<Instant>, Option<Instant>),
    /// Whether a bound stopped the report before the collection ran out.
    truncated: bool,
}

impl FreeBusyReport {
    /// An empty report over the window `start ..< end`, either bound of which may be open.
    #[must_use]
    pub const fn new(start: Option<Instant>, end: Option<Instant>) -> Self {
        Self {
            periods: Vec::new(),
            window: (start, end),
            truncated: false,
        }
    }

    /// The window this report is about.
    #[must_use]
    pub const fn window(&self) -> (Option<Instant>, Option<Instant>) {
        self.window
    }

    /// Record one more busy period, charging the caller's ledger for holding it.
    ///
    /// An empty period is dropped rather than charged: RFC 5545 section 3.8.2.6 writes a
    /// `FREEBUSY` value as a period with a positive duration, and a zero-width one states no
    /// busy time for anybody to read.
    pub fn push(&mut self, period: BusyPeriod, meter: &mut Meter) -> Result<(), QueryError> {
        if period.is_empty() {
            return Ok(());
        }
        meter.try_charge_freebusy_period()?;
        self.periods
            .try_reserve(1)
            .map_err(|_| LimitExceeded::FreeBusyPeriods)?;
        self.periods.push(period);
        Ok(())
    }

    /// The periods recorded so far.
    #[must_use]
    pub fn periods(&self) -> &[BusyPeriod] {
        &self.periods
    }

    /// Record that a bound stopped this report short of the collection.
    pub const fn note_truncated(&mut self) {
        self.truncated = true;
    }

    /// Whether a bound stopped this report short of the collection.
    ///
    /// A truncated report states less busy time than the collection holds, and a caller reading
    /// the gap as free would double-book somebody. It is a field rather than an inference,
    /// because a list of periods cannot say what is missing from it.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }
}

/// The zone answers an evaluation may use, and the one door it may ask through.
///
/// `docs/adr/0003` puts the zone data outside this workspace and the resolution policy in the
/// caller's hands. This is the seam that carries both into an evaluation: the source, the zone
/// a `CALDAV:timezone` on the query stated (RFC 4791 section 9.5), and the policy for the two
/// wall clocks a zone does not answer uniquely.
///
/// It deliberately has no fallback. There is no "assume UTC", no "use the system zone" and no
/// constructor that omits the source, because every one of those is the invention ADR 0003
/// exists to refuse. A question this cannot answer comes back as an [`Undecided`], which the
/// caller sees.
#[derive(Clone, Copy)]
pub struct Zones<'a> {
    /// Where zone answers come from.
    source: &'a dyn ZoneSource,
    /// The `TZID` the query's own `CALDAV:timezone` declared, if it carried one.
    query_zone: Option<&'a str>,
    /// What to do with the two wall clocks a zone does not name one instant for.
    policy: ResolutionPolicy,
}

impl<'a> Zones<'a> {
    /// A seam over `source`, with no query zone and the conservative policy.
    #[must_use]
    pub const fn new(source: &'a dyn ZoneSource) -> Self {
        Self {
            source,
            query_zone: None,
            policy: ResolutionPolicy::DEFAULT,
        }
    }

    /// The same seam with the zone a `calendar-query`'s `CALDAV:timezone` stated.
    ///
    /// The identifier rather than the `VTIMEZONE`: reading that component into a source is
    /// `ical-tz`'s job and the caller's call, because a query's inline zone and the server's own
    /// database legitimately disagree and choosing between them is the policy ADR 0003 leaves
    /// with the caller.
    #[must_use]
    pub const fn with_query_zone(self, tzid: &'a str) -> Self {
        Self {
            query_zone: Some(tzid),
            ..self
        }
    }

    /// The same seam under a different reading of gaps and folds.
    #[must_use]
    pub const fn with_policy(self, policy: ResolutionPolicy) -> Self {
        Self { policy, ..self }
    }

    /// Where zone answers come from.
    #[must_use]
    pub const fn source(self) -> &'a dyn ZoneSource {
        self.source
    }

    /// The zone the query stated, if it stated one.
    #[must_use]
    pub const fn query_zone(self) -> Option<&'a str> {
        self.query_zone
    }

    /// What to do with the two wall clocks a zone does not name one instant for.
    #[must_use]
    pub const fn policy(self) -> ResolutionPolicy {
        self.policy
    }

    /// Which zone a value written with `tzid` is read in.
    ///
    /// A value that named one is read in that one. A floating value is read in the zone the
    /// query stated, and if the query stated none there is no answer at all — which is
    /// [`Undecided::ZoneUnstated`] and never UTC.
    pub const fn zone_for(self, tzid: Option<&'a str>) -> Result<&'a str, Undecided> {
        match tzid {
            Some(named) => Ok(named),
            None => match self.query_zone {
                Some(stated) => Ok(stated),
                None => Err(Undecided::ZoneUnstated),
            },
        }
    }

    /// What a wall clock names under the zone a value written with `tzid` is read in.
    ///
    /// The whole of the floating-value question in one call: which zone applies, whether the
    /// source recognizes it, and what it answered. Turning the answer into an instant is the
    /// caller's, because a gap and a fold are read differently by different units and
    /// [`Zones::policy`] is what they read them with.
    pub fn resolve(
        self,
        tzid: Option<&'a str>,
        local: CivilDateTime,
    ) -> Result<ZoneAnswer, Undecided> {
        let zone = self.zone_for(tzid)?;
        self.source
            .resolve(zone, local)
            .ok_or(Undecided::ZoneUnknown)
    }

    /// What offset the zone a value written with `tzid` is read in was running at `instant`.
    pub fn offset_at(
        self,
        tzid: Option<&'a str>,
        instant: Instant,
    ) -> Result<OffsetAnswer, Undecided> {
        let zone = self.zone_for(tzid)?;
        self.source
            .offset_at(zone, instant)
            .ok_or(Undecided::ZoneUnknown)
    }
}

impl Debug for Zones<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        // The source is a caller's trait object with no `Debug` bound on it, and adding one
        // would exclude every source that does not carry it. What a reader debugging an
        // evaluation wants is which zone the query stated and how gaps and folds are read.
        formatter
            .debug_struct("Zones")
            .field("query_zone", &self.query_zone)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// The bounds and the ledger every entry point of this crate takes, in one value.
///
/// `docs/adr/0010` requires both at every hostile-input door, and an evaluation is a door: the
/// filter came off the wire, the resource came out of a store somebody else writes to, and the
/// zone source is the caller's. Passing them as one borrow rather than as two arguments is what
/// keeps a unit from acquiring a `Limits` of its own halfway down a call chain.
#[derive(Debug)]
pub struct Budget<'a> {
    /// The policy this evaluation is bounded by.
    pub limits: Limits,
    /// The ledger of work already done under it.
    pub meter: &'a mut Meter,
}

impl<'a> Budget<'a> {
    /// A budget over `limits`, keeping its ledger in `meter`.
    #[must_use]
    pub fn new(limits: Limits, meter: &'a mut Meter) -> Self {
        Self { limits, meter }
    }

    /// Whether the ledger has already refused a charge.
    ///
    /// Exhaustion latches, so an evaluation that reads `true` here has an answer that is at best
    /// truncated and should be reported as [`Undecided::SearchExhausted`] rather than as a
    /// resource that does not match.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.meter.is_exhausted()
    }
}
