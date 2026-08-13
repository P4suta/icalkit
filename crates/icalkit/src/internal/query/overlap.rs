// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 2 — the component-type overlap table. RFC 4791 section 9.9.
//!
//! **This is the most error-prone thing in this crate, and nobody in this workspace has
//! transcribed it before.** Read the whole of section 9.9 before touching this file.
//!
//! # It goes in as data
//!
//! A `static` table of rows, one per (component type, which properties are present), the way
//! `ical-itip` holds RFC 5546 section 3 — not as control flow. A reviewer must be able to put the
//! table beside the RFC and diff it row by row, which is impossible against a nest of `if`
//! statements and is the only review this transcription can actually get. A row's `state` lists
//! exactly the `Y`/`N`/`*` columns its table prints, in the printed order, and its `condition` is
//! the "Condition to evaluate" cell. The walk over the table is `overlaps`, which is a dozen
//! lines and holds no rule of its own.
//!
//! # The rows section 9.9 states, which are not one rule with exceptions
//!
//! - **`VEVENT`**, five rows and not four. With `DTEND`; with a `DURATION` greater than zero
//!   seconds; with a `DURATION` that is not, where the period collapses to the instant; with a
//!   `DATE-TIME` `DTSTART` and neither, where it is also the instant; with a `DATE` `DTSTART` and
//!   neither, where the period is the whole day. The RFC's third column — "DURATION property
//!   value is greater than 0 seconds?" — splits what reads like one `DURATION` row into two rows
//!   with different operators, and it is the row a transcription loses.
//! - **`VTODO`**, eight rows. With `DTSTART` and `DURATION`; with `DTSTART` and `DUE`; with
//!   `DTSTART` alone; with `DUE` alone; with neither but with `COMPLETED` and `CREATED`, or with
//!   `COMPLETED` alone, or with `CREATED` alone, or with none of the five. A `VTODO` with none of
//!   them overlaps every range — the cell is the literal `TRUE` — which is the row a reader
//!   assumes is a mistake and is not.
//! - **`VJOURNAL`**, three rows: a `DATE-TIME` `DTSTART`; a `DATE` `DTSTART`; none, whose cell is
//!   the literal `FALSE`.
//! - **`VFREEBUSY`**, three rows: `DTSTART` with `DTEND`; neither, where each `FREEBUSY` period is
//!   compared in turn and any one of them matching decides it; neither and no `FREEBUSY`, which is
//!   `FALSE`. Section 9.9 says a `DURATION` in a `VFREEBUSY` is ignored, so this table never
//!   reads one.
//! - **`VALARM`**, one paragraph rather than a table, and no state columns at all. Its `TRIGGER`
//!   may be relative to the parent's start or end, so the trigger instants cannot be computed
//!   here and the caller hands them in. A repeating alarm occupies every one of its repetitions,
//!   not only the first.
//!
//! # A condition is a conjunction of disjunctions, because five cells need one
//!
//! "The pair of expressions that bound the period" does not survive contact with section 9.9.
//! Three `VTODO` cells print a conjunction whose operands are themselves disjunctions — the
//! `DTSTART`-and-`DURATION` row, the `DTSTART`-and-`DUE` row and the `COMPLETED`-and-`CREATED`
//! row — one prints a single test with no `start` clause at all, and three print a bare `TRUE` or
//! `FALSE`. So a condition here is `&[&[test]]`: the outer slice is the `AND`, each inner slice is
//! one `OR`, and the nesting is the parenthesization the RFC prints. `ALWAYS` is the empty
//! conjunction and `NEVER` is the conjunction of one empty clause, which is how the literal cells
//! are written.
//!
//! # Two things the table cannot decide on its own
//!
//! A floating `DTSTART` has no instant until a zone places it, so every value in `Occupancy` is
//! one the caller has **already resolved** — this unit never touches a `ZoneSource`. That includes
//! `DTSTART+DURATION` and `DTSTART+P1D`: RFC 5545 durations are nominal and the "1 day" section
//! 9.9 gives a `DATE` value is a calendar day, so adding 86400 seconds to a resolved instant would
//! invent a UTC day across every zone transition. A `None` in `Occupancy` therefore means the
//! property is **absent**, never that it could not be resolved; a value that could not be resolved
//! is a [`crate::internal::query::Undecided`] the caller already holds and returns without reaching here.
//!
//! And a component type with no row is [`crate::internal::query::Undecided::OverlapUndefined`]: section 9.9's
//! tables are closed, and inventing a rule for a type — or for a state — they omit would make this
//! crate disagree with a conformant server about which resources a query returns. A `VTODO`
//! carrying `DURATION` but no `DTSTART` has no row, and neither does a `VEVENT` carrying no
//! `DTSTART` at all, which RFC 5545 section 3.6.1 permits when the calendar states a `METHOD`.
//!
//! # The comparison itself
//!
//! Section 9.9's open bounds are handled by reading an absent `start` as minus infinity and an
//! absent `end` as plus infinity, which one enum carries as two variants either side of the
//! timeline so that no comparison needs a branch for them. The operators are transcribed exactly
//! as printed and they are not uniform: a zero-length `VEVENT` is compared with `start <= DTSTART`
//! where a positive-duration one is compared with `start < DTSTART+DURATION`, four `VTODO` cells
//! compare the window's `end` with `>=` rather than `>`, and a `VFREEBUSY` with `DTSTART` and
//! `DTEND` is compared with `start <= DTEND` where the corresponding `VEVENT` row uses
//! `start < DTEND`. Every one of those asymmetries is in the RFC, none of them is a typo here, and
//! normalizing any of them drops or admits resources a conformant server does not.

use crate::internal::core::{ComponentKind, Instant};
use crate::internal::dav::TimeRange;

use crate::internal::query::vocabulary::{BusyPeriod, Match, Undecided};

/// What the component-type overlap table is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
pub const OVERLAP_SECTIONS: &[&str] = &[
    "RFC 4791 section 9.9, CALDAV:time-range",
    "RFC 4791 section 9.9, VEVENT: DTEND, DURATION, a DATE DTSTART, neither",
    "RFC 4791 section 9.9, VTODO: DUE, DTSTART, DURATION, COMPLETED, CREATED, none",
    "RFC 4791 section 9.9, VJOURNAL: a DATE-TIME DTSTART, a DATE DTSTART, none",
    "RFC 4791 section 9.9, VFREEBUSY: DTSTART with DTEND, and the FREEBUSY union",
    "RFC 4791 section 9.9, VALARM: a TRIGGER relative to its parent, and its repetitions",
];

// ---------------------------------------------------------------------------------------
// The values a row is evaluated against
// ---------------------------------------------------------------------------------------

/// The already-resolved values one component offers section 9.9's tables.
///
/// Every instant here is on the timeline already. Placing a floating value there needs a zone and
/// `docs/adr/0003` puts that in the caller's hands, so the caller resolves first and hands the
/// answers down; this unit has no `ZoneSource` and cannot acquire one.
///
/// A field is `None` when the component **does not carry** the property. It is never `None`
/// because a value failed to resolve: that failure is a [`crate::internal::query::Undecided`] the caller already
/// returned. Conflating the two is how a query silently stops returning a resource that is in the
/// window, which is the outcome this crate exists to refuse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Occupancy<'a> {
    /// `DTSTART`, resolved.
    pub(crate) start: Option<Instant>,
    /// Whether that `DTSTART` was written as a `DATE-TIME` rather than as a `DATE`.
    ///
    /// `None` when there is no `DTSTART`, and also what a caller that does not know leaves behind.
    /// Four rows key on this column and the two it chooses between compare different bounds with
    /// different operators, so not knowing must not silently pick one: an unknown answer selects
    /// the `DATE` row, whose condition names `DTSTART+P1D`, which such a caller has not supplied
    /// either — so the answer comes back undecided rather than wrong.
    pub(crate) start_is_date_time: Option<bool>,
    /// `DTEND`, resolved.
    pub(crate) end: Option<Instant>,
    /// `DUE`, resolved.
    pub(crate) due: Option<Instant>,
    /// `COMPLETED`, resolved.
    pub(crate) completed: Option<Instant>,
    /// `CREATED`, resolved.
    pub(crate) created: Option<Instant>,
    /// Whether the component carries a `DURATION` property at all.
    ///
    /// Separate from `duration_end` because section 9.9 keys three rows on the property's
    /// *presence* while their conditions use its *value*, and the two come apart on a component
    /// carrying a `DURATION` with no `DTSTART` for it to be relative to. Reading the presence off
    /// the value would answer such a component with the row written for one that has no
    /// `DURATION` at all.
    pub(crate) has_duration: bool,
    /// `DTSTART+DURATION`, resolved by the caller.
    ///
    /// Resolved rather than computed here: RFC 5545 section 3.3.6 durations are nominal, so a
    /// `P1D` added to a wall clock is not 86400 seconds added to an instant on either side of a
    /// zone transition.
    pub(crate) duration_end: Option<Instant>,
    /// `DTSTART+P1D`, resolved by the caller, for the same reason.
    ///
    /// The "1 day" section 9.9 gives a `DATE` `DTSTART` is a calendar day in the zone the value is
    /// read in, which is 23 or 25 hours twice a year in most of them.
    pub(crate) one_day_end: Option<Instant>,
    /// The periods of the component's `FREEBUSY` properties, in document order.
    ///
    /// Empty when it carries none, which is the state the last `VFREEBUSY` row keys on. Section
    /// 9.9 compares every period irrespective of its `FBTYPE`, so the kind on each is carried for
    /// the caller's benefit and never read here.
    pub(crate) periods: &'a [BusyPeriod],
    /// The instants a `VALARM` triggers at, every repetition included.
    ///
    /// `None` when they could not be computed, which is what a relative `TRIGGER` comes to
    /// whenever the parent component is not to hand. That is [`crate::internal::query::Undecided::ValueUnreadable`]
    /// and not "no trigger overlaps": an alarm whose triggers were never computed has not been
    /// shown to fall outside the window. `Some(&[])` is the different claim that it triggers never.
    pub(crate) triggers: Option<&'a [Instant]>,
}

/// One side of a comparison, with the two infinities section 9.9's open bounds stand for.
///
/// The variant order is the timeline order, so the derived comparison is the comparison the RFC
/// writes and the open-bound cases need no branch of their own: an absent `start` is below every
/// instant and an absent `end` is above every instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bound {
    /// The `-infinity` section 9.9 gives an absent `start` attribute.
    NegativeInfinity,
    /// A value on the timeline.
    At(Instant),
    /// The `+infinity` section 9.9 gives an absent `end` attribute.
    PositiveInfinity,
}

/// The one period or trigger a repeated row is being evaluated against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subject {
    /// The component's own values, which is every row but two.
    Component,
    /// One `FREEBUSY` period of a `VFREEBUSY`.
    Period(BusyPeriod),
    /// One trigger instant of a `VALARM`.
    Trigger(Instant),
}

impl Subject {
    /// The period this subject is, or `None` when it is not one.
    const fn period(self) -> Option<BusyPeriod> {
        match self {
            Self::Period(period) => Some(period),
            Self::Component | Self::Trigger(_) => None,
        }
    }

    /// The trigger instant this subject is, or `None` when it is not one.
    const fn trigger(self) -> Option<Instant> {
        match self {
            Self::Trigger(trigger) => Some(trigger),
            Self::Component | Self::Period(_) => None,
        }
    }
}

/// Everything one printed comparison can name: the component, the window, and the repetition.
#[derive(Clone, Copy, Debug)]
struct Context<'a> {
    /// The component's already-resolved values.
    occupancy: &'a Occupancy<'a>,
    /// The window the `CALDAV:time-range` states.
    range: TimeRange,
    /// The period or trigger being compared, for the two rows that repeat.
    subject: Subject,
}

// ---------------------------------------------------------------------------------------
// The vocabulary one row is written in
// ---------------------------------------------------------------------------------------

/// One of the values section 9.9's conditions name, spelled as the RFC spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Term {
    /// The `start` attribute of `CALDAV:time-range`; absent, it is `-infinity`.
    RangeStart,
    /// The `end` attribute; absent, it is `+infinity`.
    RangeEnd,
    /// `DTSTART`.
    DtStart,
    /// `DTEND`.
    DtEnd,
    /// `DUE`.
    Due,
    /// `COMPLETED`.
    Completed,
    /// `CREATED`.
    Created,
    /// `DTSTART+DURATION`.
    StartPlusDuration,
    /// `DTSTART+P1D`.
    StartPlusOneDay,
    /// `freebusy-period-start`, one period of one `FREEBUSY` property.
    PeriodStart,
    /// `freebusy-period-end`.
    PeriodEnd,
    /// `trigger-time`, one repetition of a `VALARM`.
    TriggerTime,
}

impl Term {
    /// What this term stands for, or `None` when the component carries no such value.
    fn value(self, context: Context<'_>) -> Option<Bound> {
        let held = context.occupancy;
        match self {
            Self::RangeStart => Some(
                context
                    .range
                    .start()
                    .map_or(Bound::NegativeInfinity, Bound::At),
            ),
            Self::RangeEnd => Some(
                context
                    .range
                    .end()
                    .map_or(Bound::PositiveInfinity, Bound::At),
            ),
            Self::DtStart => held.start.map(Bound::At),
            Self::DtEnd => held.end.map(Bound::At),
            Self::Due => held.due.map(Bound::At),
            Self::Completed => held.completed.map(Bound::At),
            Self::Created => held.created.map(Bound::At),
            Self::StartPlusDuration => held.duration_end.map(Bound::At),
            Self::StartPlusOneDay => held.one_day_end.map(Bound::At),
            Self::PeriodStart => context
                .subject
                .period()
                .map(|period| Bound::At(period.start)),
            Self::PeriodEnd => context.subject.period().map(|period| Bound::At(period.end)),
            Self::TriggerTime => context.subject.trigger().map(Bound::At),
        }
    }
}

/// One of the four comparison operators section 9.9 prints.
///
/// All four are printed and they are not interchangeable. Which one a cell uses is the whole of
/// what separates a zero-length component that a window opening on it matches from one it does
/// not, so each is transcribed rather than derived from the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compare {
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
}

impl Compare {
    /// Whether `left OP right` holds.
    fn holds(self, left: Bound, right: Bound) -> bool {
        match self {
            Self::Lt => left < right,
            Self::Le => left <= right,
            Self::Gt => left > right,
            Self::Ge => left >= right,
        }
    }
}

/// One printed comparison: the value on the left, the operator, and the value on its right.
type Test = (Term, Compare, Term);

/// One "Condition to evaluate" cell, as the conjunction of disjunctions the RFC prints.
///
/// The outer slice is the `AND` and each inner slice is one `OR`, so the nesting is the
/// parenthesization section 9.9 prints and nothing has to be re-derived to check a row.
type Condition = &'static [&'static [Test]];

/// The literal `TRUE` cell: a conjunction of no clauses, which every window satisfies.
const ALWAYS: Condition = &[];

/// The literal `FALSE` cell: one clause holding nothing that could satisfy it.
const NEVER: Condition = &[&[]];

/// One column of a state table, quoted as the RFC prints its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    /// "has the DTSTART property?"
    DtStart,
    /// "has the DTEND property?"
    DtEnd,
    /// "has the DURATION property?"
    Duration,
    /// "DURATION property value is greater than 0 seconds?"
    DurationOverZero,
    /// "DTSTART property is a DATE-TIME value?"
    DtStartIsDateTime,
    /// "has the DUE property?"
    Due,
    /// "has the COMPLETED property?"
    Completed,
    /// "has the CREATED property?"
    Created,
    /// "has both the DTSTART and DTEND properties?", which only the `VFREEBUSY` table asks.
    DtStartAndDtEnd,
    /// "has the FREEBUSY property?"
    FreeBusy,
}

impl Column {
    /// The answer this column has for `held`, or `None` when it has none.
    ///
    /// `None` is reachable on one column only: whether a `DURATION` is greater than zero seconds
    /// cannot be answered for a component carrying one with no `DTSTART` to measure it from. A row
    /// keyed `Y` or `N` there does not apply, which leaves such a component with no row at all
    /// rather than with the row written for a component carrying no `DURATION`.
    fn holds(self, held: &Occupancy<'_>) -> Option<bool> {
        match self {
            Self::DtStart => Some(held.start.is_some()),
            Self::DtEnd => Some(held.end.is_some()),
            Self::Duration => Some(held.has_duration),
            Self::DurationOverZero => duration_over_zero(held),
            Self::DtStartIsDateTime => Some(held.start_is_date_time == Some(true)),
            Self::Due => Some(held.due.is_some()),
            Self::Completed => Some(held.completed.is_some()),
            Self::Created => Some(held.created.is_some()),
            Self::DtStartAndDtEnd => Some(held.start.is_some() && held.end.is_some()),
            Self::FreeBusy => Some(!held.periods.is_empty()),
        }
    }
}

/// Whether the component's `DURATION` is greater than zero seconds, if that can be told.
///
/// Compared as resolved instants rather than as a duration value, because a nominal duration is
/// only positive relative to the wall clock it was added to.
fn duration_over_zero(held: &Occupancy<'_>) -> Option<bool> {
    if !held.has_duration {
        return Some(false);
    }
    held.start
        .zip(held.duration_end)
        .map(|(from, until)| until > from)
}

/// One cell of a state table: the `Y`, the `N` and the `*` section 9.9 prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// `Y` — the column must hold.
    Yes,
    /// `N` — it must not.
    No,
    /// `*` — the row applies either way, and applies even where the column has no answer.
    Any,
}

impl State {
    /// Whether this cell admits the answer a column gave.
    const fn admits(self, answer: Option<bool>) -> bool {
        match (self, answer) {
            (Self::Any, _) => true,
            (Self::Yes, Some(held)) => held,
            (Self::No, Some(held)) => !held,
            (Self::Yes | Self::No, None) => false,
        }
    }
}

/// What one row's condition is evaluated over, and how the answers combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repeat {
    /// Once, against the component's own values.
    Once,
    /// Once per `FREEBUSY` period. Section 9.9: "each period in each FREEBUSY property is compared
    /// against the time range", so any one of them matching decides the row.
    EachPeriod,
    /// Once per trigger instant. Section 9.9: a repeating `VALARM` "is said to overlap a given time
    /// range if at least one of its triggers overlaps the time range".
    EachTrigger,
}

// ---------------------------------------------------------------------------------------
// The walk, which holds no rule of its own
// ---------------------------------------------------------------------------------------

/// One row of one of section 9.9's state tables.
#[derive(Clone, Copy, Debug)]
struct Row {
    /// The component type whose table this row is printed in.
    kind: ComponentKind,
    /// The state columns, in the order that table prints them.
    state: &'static [(Column, State)],
    /// What the condition is evaluated over.
    repeat: Repeat,
    /// The "Condition to evaluate" cell.
    condition: Condition,
}

impl Row {
    /// Whether the component is in the state this row's columns describe.
    fn admits(self, held: &Occupancy<'_>) -> bool {
        self.state
            .iter()
            .all(|&(column, state)| state.admits(column.holds(held)))
    }

    /// Whether this row's condition holds, or `None` when it names a value that is not there.
    fn evaluate(self, held: &Occupancy<'_>, range: TimeRange) -> Option<bool> {
        match self.repeat {
            Repeat::Once => self.any_subject(held, range, core::iter::once(Subject::Component)),
            Repeat::EachPeriod => {
                let subjects = held.periods.iter().copied().map(Subject::Period);
                self.any_subject(held, range, subjects)
            },
            Repeat::EachTrigger => {
                let triggers = held.triggers.unwrap_or_default();
                self.any_subject(held, range, triggers.iter().copied().map(Subject::Trigger))
            },
        }
    }

    /// Whether the condition holds for any of `subjects`, which is `false` for none of them.
    fn any_subject(
        self,
        held: &Occupancy<'_>,
        range: TimeRange,
        mut subjects: impl Iterator<Item = Subject>,
    ) -> Option<bool> {
        subjects.try_fold(false, |seen, subject| {
            let context = Context {
                occupancy: held,
                range,
                subject,
            };
            condition_holds(self.condition, context).map(|met| seen || met)
        })
    }
}

/// Whether `condition` holds, or `None` when it names a value the component does not carry.
///
/// Neither the conjunction nor the disjunction short-circuits. A row naming a value that is not
/// there is a row section 9.9's table does not reach, and whether it is answered as such must not
/// depend on which side of an operator the missing value sat or on what the other side said.
fn condition_holds(condition: Condition, context: Context<'_>) -> Option<bool> {
    let mut conjunction = true;
    for clause in condition {
        let mut disjunction = false;
        for &(left, compare, right) in *clause {
            let lower = left.value(context)?;
            let upper = right.value(context)?;
            disjunction = disjunction || compare.holds(lower, upper);
        }
        conjunction = conjunction && disjunction;
    }
    Some(conjunction)
}

/// Whether a component of `kind` holding `held` overlaps `range`, RFC 4791 section 9.9.
///
/// The whole of the rule is `ROWS`. The first row printed for this component type whose state
/// columns admit the component, and whose condition names only values the component carries, is
/// the row that answers. There is no fallback: a component reaching no row is
/// [`crate::internal::query::Undecided::OverlapUndefined`] and never a resource reported as not matching.
///
/// It takes no [`crate::internal::query::Budget`]. Every entry point of this crate does, because the filter came
/// off the wire and the resource came out of somebody else's store — but this is not one. It
/// allocates nothing, searches nothing, and does work bounded by a twenty-row `static` and by two
/// slices the caller built and charged for. The expansion and the resolution that cost happen
/// above it and reach it as the already-resolved values in `Occupancy`.
pub(crate) fn overlaps(kind: ComponentKind, held: &Occupancy<'_>, range: TimeRange) -> Match {
    // The one rule below that no state table states. Section 9.9 evaluates a `VALARM` against
    // `trigger-time`, and a relative `TRIGGER` has no such time until the parent supplies one. A
    // caller that could not compute the triggers hands `None`, and reading that as "no trigger
    // overlaps" would report an absence nothing established.
    if matches!(kind, ComponentKind::Alarm) && held.triggers.is_none() {
        return Match::Undecided(Undecided::ValueUnreadable);
    }
    for row in ROWS {
        if row.kind != kind || !row.admits(held) {
            continue;
        }
        if let Some(met) = row.evaluate(held, range) {
            return Match::of(met);
        }
    }
    Match::Undecided(Undecided::OverlapUndefined)
}

// ---------------------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------------------

/// Section 9.9's four state tables and its `VALARM` paragraph, in the order they are printed.
///
/// Each row's `state` lists exactly the columns its table prints, in the printed order, `*`
/// columns included, so a reviewer reads the `Y`/`N`/`*` down the left of this table and down the
/// left of the RFC's. The comment above each row is that row's key and its printed cell.
static ROWS: &[Row] = &[
    // ---- RFC 4791 section 9.9, the VEVENT table ----
    // Columns: DTEND, DURATION, DURATION > 0 seconds, DTSTART is a DATE-TIME.
    //
    // | Y | N | N | * | (start <  DTEND AND end > DTSTART) |
    Row {
        kind: ComponentKind::Event,
        state: &[
            (Column::DtEnd, State::Yes),
            (Column::Duration, State::No),
            (Column::DurationOverZero, State::No),
            (Column::DtStartIsDateTime, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Lt, Term::DtEnd)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | N | Y | Y | * | (start <  DTSTART+DURATION AND end > DTSTART) |
    Row {
        kind: ComponentKind::Event,
        state: &[
            (Column::DtEnd, State::No),
            (Column::Duration, State::Yes),
            (Column::DurationOverZero, State::Yes),
            (Column::DtStartIsDateTime, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Lt, Term::StartPlusDuration)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | N | Y | N | * | (start <= DTSTART AND end > DTSTART) |
    Row {
        kind: ComponentKind::Event,
        state: &[
            (Column::DtEnd, State::No),
            (Column::Duration, State::Yes),
            (Column::DurationOverZero, State::No),
            (Column::DtStartIsDateTime, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::DtStart)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | N | N | N | Y | (start <= DTSTART AND end > DTSTART) |
    Row {
        kind: ComponentKind::Event,
        state: &[
            (Column::DtEnd, State::No),
            (Column::Duration, State::No),
            (Column::DurationOverZero, State::No),
            (Column::DtStartIsDateTime, State::Yes),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::DtStart)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | N | N | N | N | (start <  DTSTART+P1D AND end > DTSTART) |
    Row {
        kind: ComponentKind::Event,
        state: &[
            (Column::DtEnd, State::No),
            (Column::Duration, State::No),
            (Column::DurationOverZero, State::No),
            (Column::DtStartIsDateTime, State::No),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Lt, Term::StartPlusOneDay)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // ---- RFC 4791 section 9.9, the VTODO table ----
    // Columns: DTSTART, DURATION, DUE, COMPLETED, CREATED.
    //
    // | Y | Y | N | * | * | (start  <= DTSTART+DURATION)  AND             |
    //                     | ((end   >  DTSTART)  OR                       |
    //                     |  (end   >= DTSTART+DURATION))                 |
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::Yes),
            (Column::Duration, State::Yes),
            (Column::Due, State::No),
            (Column::Completed, State::Any),
            (Column::Created, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::StartPlusDuration)],
            &[
                (Term::RangeEnd, Compare::Gt, Term::DtStart),
                (Term::RangeEnd, Compare::Ge, Term::StartPlusDuration),
            ],
        ],
    },
    // | Y | N | Y | * | * | ((start <  DUE)      OR  (start <= DTSTART))  |
    //                     | AND                                           |
    //                     | ((end   >  DTSTART)  OR  (end   >= DUE))      |
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::Yes),
            (Column::Duration, State::No),
            (Column::Due, State::Yes),
            (Column::Completed, State::Any),
            (Column::Created, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[
                (Term::RangeStart, Compare::Lt, Term::Due),
                (Term::RangeStart, Compare::Le, Term::DtStart),
            ],
            &[
                (Term::RangeEnd, Compare::Gt, Term::DtStart),
                (Term::RangeEnd, Compare::Ge, Term::Due),
            ],
        ],
    },
    // | Y | N | N | * | * | (start  <= DTSTART)  AND (end >  DTSTART)     |
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::Yes),
            (Column::Duration, State::No),
            (Column::Due, State::No),
            (Column::Completed, State::Any),
            (Column::Created, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::DtStart)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | N | N | Y | * | * | (start  <  DUE)      AND (end >= DUE)         |
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::No),
            (Column::Duration, State::No),
            (Column::Due, State::Yes),
            (Column::Completed, State::Any),
            (Column::Created, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Lt, Term::Due)],
            &[(Term::RangeEnd, Compare::Ge, Term::Due)],
        ],
    },
    // | N | N | N | Y | Y | ((start <= CREATED)  OR  (start <= COMPLETED))|
    //                     | AND                                           |
    //                     | ((end   >= CREATED)  OR  (end   >= COMPLETED))|
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::No),
            (Column::Duration, State::No),
            (Column::Due, State::No),
            (Column::Completed, State::Yes),
            (Column::Created, State::Yes),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[
                (Term::RangeStart, Compare::Le, Term::Created),
                (Term::RangeStart, Compare::Le, Term::Completed),
            ],
            &[
                (Term::RangeEnd, Compare::Ge, Term::Created),
                (Term::RangeEnd, Compare::Ge, Term::Completed),
            ],
        ],
    },
    // | N | N | N | Y | N | (start  <= COMPLETED) AND (end  >= COMPLETED) |
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::No),
            (Column::Duration, State::No),
            (Column::Due, State::No),
            (Column::Completed, State::Yes),
            (Column::Created, State::No),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::Completed)],
            &[(Term::RangeEnd, Compare::Ge, Term::Completed)],
        ],
    },
    // | N | N | N | N | Y | (end    >  CREATED)                           |
    //
    // One clause, and no `start` clause at all: a task created long before the window still
    // overlaps every window that ends after it was created. That is what section 9.9 prints.
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::No),
            (Column::Duration, State::No),
            (Column::Due, State::No),
            (Column::Completed, State::No),
            (Column::Created, State::Yes),
        ],
        repeat: Repeat::Once,
        condition: &[&[(Term::RangeEnd, Compare::Gt, Term::Created)]],
    },
    // | N | N | N | N | N | TRUE                                          |
    //
    // A `VTODO` with none of the five overlaps every range. It reads like a mistake and it is not:
    // such a task states no time at all, and section 9.9 answers that a `time-range` cannot be
    // what excludes it.
    Row {
        kind: ComponentKind::Todo,
        state: &[
            (Column::DtStart, State::No),
            (Column::Duration, State::No),
            (Column::Due, State::No),
            (Column::Completed, State::No),
            (Column::Created, State::No),
        ],
        repeat: Repeat::Once,
        condition: ALWAYS,
    },
    // ---- RFC 4791 section 9.9, the VJOURNAL table ----
    // Columns: DTSTART, DTSTART is a DATE-TIME.
    //
    // | Y | Y | (start <= DTSTART)     AND (end > DTSTART) |
    Row {
        kind: ComponentKind::Journal,
        state: &[
            (Column::DtStart, State::Yes),
            (Column::DtStartIsDateTime, State::Yes),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::DtStart)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | Y | N | (start <  DTSTART+P1D) AND (end > DTSTART) |
    Row {
        kind: ComponentKind::Journal,
        state: &[
            (Column::DtStart, State::Yes),
            (Column::DtStartIsDateTime, State::No),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Lt, Term::StartPlusOneDay)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | N | * | FALSE                                      |
    Row {
        kind: ComponentKind::Journal,
        state: &[
            (Column::DtStart, State::No),
            (Column::DtStartIsDateTime, State::Any),
        ],
        repeat: Repeat::Once,
        condition: NEVER,
    },
    // ---- RFC 4791 section 9.9, the VFREEBUSY table ----
    // Columns: both DTSTART and DTEND, FREEBUSY.
    //
    // | Y | * | (start <= DTEND) AND (end > DTSTART)         |
    //
    // `start <= DTEND`, where the corresponding `VEVENT` row prints `start < DTEND`. A
    // `VFREEBUSY` ending exactly when a window opens therefore overlaps it and a `VEVENT` does
    // not. The asymmetry is the RFC's and normalizing it changes which resources come back.
    Row {
        kind: ComponentKind::FreeBusy,
        state: &[
            (Column::DtStartAndDtEnd, State::Yes),
            (Column::FreeBusy, State::Any),
        ],
        repeat: Repeat::Once,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::DtEnd)],
            &[(Term::RangeEnd, Compare::Gt, Term::DtStart)],
        ],
    },
    // | N | Y | (start <  freebusy-period-end) AND           |
    //         | (end   >  freebusy-period-start)             |
    Row {
        kind: ComponentKind::FreeBusy,
        state: &[
            (Column::DtStartAndDtEnd, State::No),
            (Column::FreeBusy, State::Yes),
        ],
        repeat: Repeat::EachPeriod,
        condition: &[
            &[(Term::RangeStart, Compare::Lt, Term::PeriodEnd)],
            &[(Term::RangeEnd, Compare::Gt, Term::PeriodStart)],
        ],
    },
    // | N | N | FALSE                                        |
    Row {
        kind: ComponentKind::FreeBusy,
        state: &[
            (Column::DtStartAndDtEnd, State::No),
            (Column::FreeBusy, State::No),
        ],
        repeat: Repeat::Once,
        condition: NEVER,
    },
    // ---- RFC 4791 section 9.9, the VALARM paragraph ----
    //
    // (start <= trigger-time) AND (end > trigger-time), over every trigger. No state columns: the
    // paragraph states one condition and keys it on nothing.
    Row {
        kind: ComponentKind::Alarm,
        state: &[],
        repeat: Repeat::EachTrigger,
        condition: &[
            &[(Term::RangeStart, Compare::Le, Term::TriggerTime)],
            &[(Term::RangeEnd, Compare::Gt, Term::TriggerTime)],
        ],
    },
];

#[cfg(test)]
mod tests {
    use crate::internal::core::{ComponentKind, Instant};
    use crate::internal::dav::TimeRange;

    use super::{Occupancy, ROWS, overlaps};
    use crate::internal::query::vocabulary::{BusyPeriod, BusyType, Match, Undecided};

    /// One expectation, worked out by hand from the cell RFC 4791 section 9.9 prints for the row
    /// the case is in.
    ///
    /// The cell is quoted above each group. No expectation below was taken from what this file
    /// returns: a test whose expectation came from the code under it proves only that the code
    /// agrees with itself, which is precisely the failure a transcription is exposed to.
    struct Case<'a> {
        /// What the case is about, printed when it fails.
        about: &'a str,
        /// The component type.
        kind: ComponentKind,
        /// The resolved values.
        occupancy: Occupancy<'a>,
        /// The window.
        range: TimeRange,
        /// What section 9.9's condition says.
        expect: Match,
    }

    /// The instant `seconds` after the epoch.
    fn at(seconds: i64) -> Instant {
        Instant::from_unix_seconds(seconds)
    }

    /// The window `from ..< until`, as section 9.9's two attributes bound one.
    fn window(from: i64, until: i64) -> TimeRange {
        TimeRange::new(Some(at(from)), Some(at(until))).unwrap()
    }

    /// Run every case and name the one that failed.
    fn check(cases: &[Case<'_>]) {
        for case in cases {
            assert_eq!(
                overlaps(case.kind, &case.occupancy, case.range),
                case.expect,
                "{}",
                case.about
            );
        }
    }

    #[test]
    fn the_table_holds_the_rows_section_9_9_prints_and_no_others() {
        for (kind, printed) in [
            (ComponentKind::Event, 5),
            (ComponentKind::Todo, 8),
            (ComponentKind::Journal, 3),
            (ComponentKind::FreeBusy, 3),
            (ComponentKind::Alarm, 1),
        ] {
            let held = ROWS.iter().filter(|row| row.kind == kind).count();
            assert_eq!(held, printed, "{kind:?}");
        }
        assert_eq!(ROWS.len(), 20);
    }

    #[test]
    fn a_vevent_with_a_dtend_or_a_duration_follows_its_first_three_rows() {
        // | Y | N | N | * | (start < DTEND AND end > DTSTART) |, DTSTART=100, DTEND=200.
        let dated = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            end: Some(at(200)),
            ..Occupancy::default()
        };
        // | N | Y | Y | * | (start < DTSTART+DURATION AND end > DTSTART) |.
        let lasting = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            has_duration: true,
            duration_end: Some(at(200)),
            ..Occupancy::default()
        };
        // | N | Y | N | * | (start <= DTSTART AND end > DTSTART) |, a `DURATION` of `PT0S`.
        let punctual = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            has_duration: true,
            duration_end: Some(at(100)),
            ..Occupancy::default()
        };
        check(&[
            Case {
                about: "an event ending exactly when the window opens: 200 < 200 is false",
                kind: ComponentKind::Event,
                occupancy: dated,
                range: window(200, 300),
                expect: Match::Unmatched,
            },
            Case {
                about: "a window closing exactly on DTSTART: 100 > 100 is false",
                kind: ComponentKind::Event,
                occupancy: dated,
                range: window(50, 100),
                expect: Match::Unmatched,
            },
            Case {
                about: "a window closing one second later: 50 < 200 and 101 > 100",
                kind: ComponentKind::Event,
                occupancy: dated,
                range: window(50, 101),
                expect: Match::Matched,
            },
            Case {
                about: "a positive DURATION is compared with <, so 200 < 200 is false",
                kind: ComponentKind::Event,
                occupancy: lasting,
                range: window(200, 300),
                expect: Match::Unmatched,
            },
            Case {
                about: "a positive DURATION overlapping: 100 < 200 and 101 > 100",
                kind: ComponentKind::Event,
                occupancy: lasting,
                range: window(100, 101),
                expect: Match::Matched,
            },
            Case {
                about: "a zero DURATION is compared with <=, so a window opening on it matches",
                kind: ComponentKind::Event,
                occupancy: punctual,
                range: window(100, 101),
                expect: Match::Matched,
            },
            Case {
                about: "a zero DURATION with the window closing on DTSTART: 100 > 100 is false",
                kind: ComponentKind::Event,
                occupancy: punctual,
                range: window(99, 100),
                expect: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn a_vevent_with_neither_follows_the_two_rows_that_split_on_the_value_type() {
        // | N | N | N | Y | (start <= DTSTART AND end > DTSTART) |.
        let bare = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            ..Occupancy::default()
        };
        // | N | N | N | N | (start < DTSTART+P1D AND end > DTSTART) |, a `DATE` `DTSTART`.
        let whole_day = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(false),
            one_day_end: Some(at(86_500)),
            ..Occupancy::default()
        };
        check(&[
            Case {
                about: "a DATE-TIME DTSTART and neither property is the instant itself",
                kind: ComponentKind::Event,
                occupancy: bare,
                range: window(100, 101),
                expect: Match::Matched,
            },
            Case {
                about: "the same event, window closing on it: 100 > 100 is false",
                kind: ComponentKind::Event,
                occupancy: bare,
                range: window(99, 100),
                expect: Match::Unmatched,
            },
            Case {
                about: "a DATE DTSTART occupies the whole day: 86499 < 86500 and 90000 > 100",
                kind: ComponentKind::Event,
                occupancy: whole_day,
                range: window(86_499, 90_000),
                expect: Match::Matched,
            },
            Case {
                about: "the day is exclusive at its end: 86500 < 86500 is false",
                kind: ComponentKind::Event,
                occupancy: whole_day,
                range: window(86_500, 90_000),
                expect: Match::Unmatched,
            },
            Case {
                about: "a window closing on the day's DTSTART: 100 > 100 is false",
                kind: ComponentKind::Event,
                occupancy: whole_day,
                range: window(0, 100),
                expect: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn a_vtodo_with_a_dtstart_follows_the_first_three_rows_of_its_table() {
        // | Y | Y | N | * | * | (start <= DTSTART+DURATION) AND
        //                       ((end > DTSTART) OR (end >= DTSTART+DURATION)) |
        let scheduled = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            has_duration: true,
            duration_end: Some(at(200)),
            ..Occupancy::default()
        };
        // | Y | N | Y | * | * | ((start < DUE) OR (start <= DTSTART)) AND
        //                       ((end > DTSTART) OR (end >= DUE)) |
        let assigned = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            due: Some(at(200)),
            ..Occupancy::default()
        };
        // | Y | N | N | * | * | (start <= DTSTART) AND (end > DTSTART) |
        let started = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            ..Occupancy::default()
        };
        check(&[
            Case {
                about: "a task with a DURATION uses <=, so a window opening on its end matches",
                kind: ComponentKind::Todo,
                occupancy: scheduled,
                range: window(200, 300),
                expect: Match::Matched,
            },
            Case {
                about: "one second past that end: 201 <= 200 is false",
                kind: ComponentKind::Todo,
                occupancy: scheduled,
                range: window(201, 300),
                expect: Match::Unmatched,
            },
            Case {
                about: "before it: 100 > 100 is false and 100 >= 200 is false",
                kind: ComponentKind::Todo,
                occupancy: scheduled,
                range: window(0, 100),
                expect: Match::Unmatched,
            },
            Case {
                about: "a task with a DUE: 200 < 200 is false and 200 <= 100 is false",
                kind: ComponentKind::Todo,
                occupancy: assigned,
                range: window(200, 300),
                expect: Match::Unmatched,
            },
            Case {
                about: "one second earlier: 199 < 200, and 300 > 100",
                kind: ComponentKind::Todo,
                occupancy: assigned,
                range: window(199, 300),
                expect: Match::Matched,
            },
            Case {
                about: "a window closing on DTSTART: 100 > 100 and 100 >= 200 are both false",
                kind: ComponentKind::Todo,
                occupancy: assigned,
                range: window(0, 100),
                expect: Match::Unmatched,
            },
            Case {
                about: "DTSTART alone, window opening on it: 100 <= 100 and 101 > 100",
                kind: ComponentKind::Todo,
                occupancy: started,
                range: window(100, 101),
                expect: Match::Matched,
            },
            Case {
                about: "DTSTART alone, window after it: 101 <= 100 is false",
                kind: ComponentKind::Todo,
                occupancy: started,
                range: window(101, 200),
                expect: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn a_vtodo_without_a_dtstart_follows_the_last_five_rows_of_its_table() {
        // | N | N | Y | * | * | (start < DUE) AND (end >= DUE) |
        let due_only = Occupancy {
            due: Some(at(200)),
            ..Occupancy::default()
        };
        // | N | N | N | Y | Y | ((start <= CREATED) OR (start <= COMPLETED)) AND
        //                       ((end >= CREATED) OR (end >= COMPLETED)) |
        let recorded = Occupancy {
            completed: Some(at(200)),
            created: Some(at(100)),
            ..Occupancy::default()
        };
        // | N | N | N | Y | N | (start <= COMPLETED) AND (end >= COMPLETED) |
        let finished = Occupancy {
            completed: Some(at(200)),
            ..Occupancy::default()
        };
        // | N | N | N | N | Y | (end > CREATED) |
        let opened = Occupancy {
            created: Some(at(100)),
            ..Occupancy::default()
        };
        check(&[
            Case {
                about: "DUE alone compares the window's end with >=, so 200 >= 200 matches",
                kind: ComponentKind::Todo,
                occupancy: due_only,
                range: window(100, 200),
                expect: Match::Matched,
            },
            Case {
                about: "one second short of it: 199 >= 200 is false",
                kind: ComponentKind::Todo,
                occupancy: due_only,
                range: window(100, 199),
                expect: Match::Unmatched,
            },
            Case {
                about: "a window opening on DUE: 200 < 200 is false",
                kind: ComponentKind::Todo,
                occupancy: due_only,
                range: window(200, 300),
                expect: Match::Unmatched,
            },
            Case {
                about: "COMPLETED and CREATED, window between them: 150 <= 200 and 160 >= 100",
                kind: ComponentKind::Todo,
                occupancy: recorded,
                range: window(150, 160),
                expect: Match::Matched,
            },
            Case {
                about: "a window after both: 300 <= 100 and 300 <= 200 are both false",
                kind: ComponentKind::Todo,
                occupancy: recorded,
                range: window(300, 400),
                expect: Match::Unmatched,
            },
            Case {
                about: "a window before both: 50 >= 100 and 50 >= 200 are both false",
                kind: ComponentKind::Todo,
                occupancy: recorded,
                range: window(0, 50),
                expect: Match::Unmatched,
            },
            Case {
                about: "COMPLETED alone, window closing on it: 0 <= 200 and 200 >= 200",
                kind: ComponentKind::Todo,
                occupancy: finished,
                range: window(0, 200),
                expect: Match::Matched,
            },
            Case {
                about: "COMPLETED alone, one second short: 199 >= 200 is false",
                kind: ComponentKind::Todo,
                occupancy: finished,
                range: window(0, 199),
                expect: Match::Unmatched,
            },
            Case {
                about: "CREATED alone has no start clause, so a much later window still matches",
                kind: ComponentKind::Todo,
                occupancy: opened,
                range: window(500, 600),
                expect: Match::Matched,
            },
            Case {
                about: "CREATED alone, window closing on it: 100 > 100 is false",
                kind: ComponentKind::Todo,
                occupancy: opened,
                range: window(0, 100),
                expect: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn a_vtodo_with_none_of_the_five_properties_overlaps_every_range() {
        // | N | N | N | N | N | TRUE |. The row a reader assumes is a mistake.
        let timeless = Occupancy::default();
        check(&[
            Case {
                about: "TRUE for a window at the epoch",
                kind: ComponentKind::Todo,
                occupancy: timeless,
                range: window(0, 1),
                expect: Match::Matched,
            },
            Case {
                about: "TRUE for a window nowhere near it",
                kind: ComponentKind::Todo,
                occupancy: timeless,
                range: window(-1_000_000_000, -999_999_999),
                expect: Match::Matched,
            },
            Case {
                about: "TRUE for a window open at its end",
                kind: ComponentKind::Todo,
                occupancy: timeless,
                range: TimeRange::starting_at(at(0)),
                expect: Match::Matched,
            },
        ]);
    }

    #[test]
    fn a_vjournal_follows_its_three_rows_and_a_dateless_one_matches_nothing() {
        // | Y | Y | (start <= DTSTART) AND (end > DTSTART) |
        let noted = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            ..Occupancy::default()
        };
        // | Y | N | (start < DTSTART+P1D) AND (end > DTSTART) |
        let day_entry = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(false),
            one_day_end: Some(at(86_500)),
            ..Occupancy::default()
        };
        // | N | * | FALSE |
        let undated = Occupancy::default();
        check(&[
            Case {
                about: "a DATE-TIME entry, window opening on it: 100 <= 100 and 101 > 100",
                kind: ComponentKind::Journal,
                occupancy: noted,
                range: window(100, 101),
                expect: Match::Matched,
            },
            Case {
                about: "a DATE-TIME entry, window closing on it: 100 > 100 is false",
                kind: ComponentKind::Journal,
                occupancy: noted,
                range: window(99, 100),
                expect: Match::Unmatched,
            },
            Case {
                about: "a DATE entry occupies its whole day: 86499 < 86500",
                kind: ComponentKind::Journal,
                occupancy: day_entry,
                range: window(86_499, 90_000),
                expect: Match::Matched,
            },
            Case {
                about: "that day is exclusive at its end: 86500 < 86500 is false",
                kind: ComponentKind::Journal,
                occupancy: day_entry,
                range: window(86_500, 90_000),
                expect: Match::Unmatched,
            },
            Case {
                about: "a VJOURNAL with no DTSTART is FALSE against a window on the epoch",
                kind: ComponentKind::Journal,
                occupancy: undated,
                range: window(0, 1),
                expect: Match::Unmatched,
            },
            Case {
                about: "and FALSE against a window open at its end",
                kind: ComponentKind::Journal,
                occupancy: undated,
                range: TimeRange::starting_at(at(-1_000_000)),
                expect: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn a_vfreebusy_follows_its_three_rows_including_the_freebusy_union() {
        // | Y | * | (start <= DTEND) AND (end > DTSTART) |
        let bounded = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            end: Some(at(200)),
            ..Occupancy::default()
        };
        // | N | Y | (start < freebusy-period-end) AND (end > freebusy-period-start) |
        let periods = [
            BusyPeriod::new(at(300), at(400), BusyType::Busy),
            BusyPeriod::new(at(600), at(700), BusyType::Free),
        ];
        let reported = Occupancy {
            periods: &periods,
            ..Occupancy::default()
        };
        // | N | N | FALSE |
        let empty = Occupancy::default();
        check(&[
            Case {
                about: "DTSTART with DTEND uses <=, unlike the VEVENT row: 200 <= 200 matches",
                kind: ComponentKind::FreeBusy,
                occupancy: bounded,
                range: window(200, 300),
                expect: Match::Matched,
            },
            Case {
                about: "one second past DTEND: 201 <= 200 is false",
                kind: ComponentKind::FreeBusy,
                occupancy: bounded,
                range: window(201, 300),
                expect: Match::Unmatched,
            },
            Case {
                about: "a window closing on DTSTART: 100 > 100 is false",
                kind: ComponentKind::FreeBusy,
                occupancy: bounded,
                range: window(0, 100),
                expect: Match::Unmatched,
            },
            Case {
                about: "the second period decides it, and FREE is compared like every other",
                kind: ComponentKind::FreeBusy,
                occupancy: reported,
                range: window(650, 660),
                expect: Match::Matched,
            },
            Case {
                about: "a window between the two periods matches neither",
                kind: ComponentKind::FreeBusy,
                occupancy: reported,
                range: window(400, 600),
                expect: Match::Unmatched,
            },
            Case {
                about: "a period is exclusive at its end: 400 < 400 is false",
                kind: ComponentKind::FreeBusy,
                occupancy: reported,
                range: window(400, 500),
                expect: Match::Unmatched,
            },
            Case {
                about: "neither DTSTART with DTEND nor any FREEBUSY is FALSE",
                kind: ComponentKind::FreeBusy,
                occupancy: empty,
                range: window(0, 1_000_000),
                expect: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn a_valarm_overlaps_when_any_one_of_its_repetitions_does() {
        // (start <= trigger-time) AND (end > trigger-time), over every trigger.
        let triggers = [at(100), at(200)];
        let repeating = Occupancy {
            triggers: Some(&triggers),
            ..Occupancy::default()
        };
        let silent = Occupancy {
            triggers: Some(&[]),
            ..Occupancy::default()
        };
        check(&[
            Case {
                about: "the second repetition matches where the first does not",
                kind: ComponentKind::Alarm,
                occupancy: repeating,
                range: window(200, 300),
                expect: Match::Matched,
            },
            Case {
                about: "the first repetition matches: 100 <= 100 and 101 > 100",
                kind: ComponentKind::Alarm,
                occupancy: repeating,
                range: window(100, 101),
                expect: Match::Matched,
            },
            Case {
                about: "between them: 150 <= 100 is false, and 200 > 200 is false",
                kind: ComponentKind::Alarm,
                occupancy: repeating,
                range: window(150, 200),
                expect: Match::Unmatched,
            },
            Case {
                about: "an alarm that triggers never overlaps nothing",
                kind: ComponentKind::Alarm,
                occupancy: silent,
                range: window(0, 1_000_000),
                expect: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn an_open_bound_is_the_infinity_section_9_9_gives_it() {
        let dated = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            end: Some(at(200)),
            ..Occupancy::default()
        };
        check(&[
            Case {
                about: "an absent start is below DTEND, and 100 > 100 is still false",
                kind: ComponentKind::Event,
                occupancy: dated,
                range: TimeRange::ending_before(at(100)),
                expect: Match::Unmatched,
            },
            Case {
                about: "an absent start, window closing one second later, matches",
                kind: ComponentKind::Event,
                occupancy: dated,
                range: TimeRange::ending_before(at(101)),
                expect: Match::Matched,
            },
            Case {
                about: "an absent end is above DTSTART, and 200 < 200 is still false",
                kind: ComponentKind::Event,
                occupancy: dated,
                range: TimeRange::starting_at(at(200)),
                expect: Match::Unmatched,
            },
            Case {
                about: "an absent end, window opening one second earlier, matches",
                kind: ComponentKind::Event,
                occupancy: dated,
                range: TimeRange::starting_at(at(199)),
                expect: Match::Matched,
            },
        ]);
    }

    #[test]
    fn a_state_or_a_type_the_table_omits_is_undecided_and_never_unmatched() {
        let undefined = Match::Undecided(Undecided::OverlapUndefined);
        // Section 9.9 prints no table for a VTIMEZONE.
        let observance = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(true),
            ..Occupancy::default()
        };
        // Every VEVENT row names DTSTART, which RFC 5545 section 3.6.1 lets a component omit when
        // the calendar states a METHOD.
        let methodless = Occupancy {
            end: Some(at(200)),
            ..Occupancy::default()
        };
        // The VTODO table prints no row with DTSTART absent and DURATION present.
        let dangling = Occupancy {
            has_duration: true,
            due: Some(at(200)),
            ..Occupancy::default()
        };
        // The DATE row names DTSTART+P1D, and a nominal day cannot be computed here.
        let unresolved_day = Occupancy {
            start: Some(at(100)),
            start_is_date_time: Some(false),
            ..Occupancy::default()
        };
        check(&[
            Case {
                about: "a VTIMEZONE has no row in section 9.9",
                kind: ComponentKind::TimeZone,
                occupancy: observance,
                range: window(0, 1_000_000),
                expect: undefined,
            },
            Case {
                about: "a VEVENT with no DTSTART reaches no row",
                kind: ComponentKind::Event,
                occupancy: methodless,
                range: window(0, 1_000_000),
                expect: undefined,
            },
            Case {
                about: "a VTODO with a DURATION and no DTSTART reaches no row",
                kind: ComponentKind::Todo,
                occupancy: dangling,
                range: window(0, 1_000_000),
                expect: undefined,
            },
            Case {
                about: "a DATE DTSTART whose nominal day the caller did not resolve",
                kind: ComponentKind::Event,
                occupancy: unresolved_day,
                range: window(0, 1_000_000),
                expect: undefined,
            },
            Case {
                about: "a VALARM whose relative TRIGGER could not be placed without its parent",
                kind: ComponentKind::Alarm,
                occupancy: Occupancy::default(),
                range: window(0, 1_000_000),
                expect: Match::Undecided(Undecided::ValueUnreadable),
            },
        ]);
    }
}
