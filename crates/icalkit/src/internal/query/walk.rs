// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 4 — the component filter tree walk, and the crate's front door. RFC 4791 section 9.7.1.
//!
//! # What this unit owns
//!
//! The recursive shape of a `CALDAV:filter`, and the one public entry point every caller
//! reaches this crate through: a `CompFilter`, a parsed calendar, a [`crate::internal::query::Zones`] and a
//! [`crate::internal::query::Budget`] in, a [`crate::internal::query::Match`] out.
//!
//! - The root `comp-filter` names `VCALENDAR` and the tests below it apply to the calendar
//!   object. A root naming anything else matches nothing, which is section 9.7.1's own reading.
//! - A `comp-filter` matches a component when the component's name matches **and** every test
//!   inside the filter is satisfied: its `time-range` through `overlap`, each `prop-filter`
//!   through `prop`, and each nested `comp-filter` against the component's own subcomponents.
//!   Every test inside one filter has to hold, and a filter is satisfied by **any one**
//!   subcomponent that satisfies it — which is where the conjunction over tests and the
//!   disjunction over candidates meet, and where implementations get it wrong.
//! - `CALDAV:is-not-defined` matches when no subcomponent of that name exists, and is exclusive
//!   with every other test. A filter `ical-dav` reports as contradictory is
//!   [`crate::internal::query::QueryError::Contradictory`] here rather than an answer.
//! - Composition is [`crate::internal::query::Match::and`] and [`crate::internal::query::Match::or`] and never `bool`, so an
//!   undecidable subtree stays undecidable through the whole walk instead of being flattened
//!   into "no match" at the first `&&`.
//!
//! # It must not recurse on the caller's stack
//!
//! `ical-dav` bounds a `CompFilter`'s height at construction against `Limits::max_xml_depth`, so
//! a filter that exists is one that could be walked recursively — but the *calendar* it is
//! walked against is bounded by `Limits::max_component_depth`, which is a different number, and
//! the walk is over the product of the two. Charge `Meter::try_enter_element` per level and
//! refuse at the bound, the way `ical-dav`'s own reader does.
//!
//! # The prefilter seam
//!
//! Before expanding anything for a `time-range`, this unit calls `prefilter`. `docs/adr/0012`
//! fixes that as the shape the crate ships whether or not the measurement has run: the prefilter
//! defaults to "cannot exclude", so the failing branch of the measurement is an implementation
//! of one function rather than a rewrite of this one.
//!
//! It is asked once per filter node rather than once per candidate, because what it answers is a
//! fact about the resource and the filter — `prefilter` reads the whole calendar, since a
//! `RECURRENCE-ID` override lives in a sibling of the master it moves. The answer is kept on the
//! frame and read for every candidate that filter is tried against.
//!
//! # How the four bullets of section 9.7.1 became one rule
//!
//! The section states four alternatives — an empty filter, an `is-not-defined` filter, a filter
//! with a `time-range`, and a filter with only children — and writing them as four branches is
//! how the conjunction and the disjunction get crossed. They are one rule with two quantifiers,
//! and this file writes it that way:
//!
//! > *some* component of the scope named by the filter satisfies *every* test the filter states.
//!
//! The first bullet is that rule with no tests (a conjunction over nothing is satisfied, so the
//! question is only whether a candidate exists). The third and the fourth are the same rule with
//! the tests present, which is why the section repeats "also match the targeted calendar
//! component" in both: the target is singular, and two components each satisfying half of the
//! filter satisfy none of it. The second bullet is the only one that is not that rule, because
//! it asks about the *absence* of a candidate rather than about any candidate's properties.
//!
//! # What the walk holds and what it delegates
//!
//! Four questions belong to other units and reach them through one private seam, so the walk
//! can be read — and tested — without any of them: whether a filter's window can be refused for
//! a resource without expanding it (`prefilter`), whether any recurrence instance of a component
//! overlaps a window (`expand` over `overlap`), and whether a `prop-filter` matches a component
//! (`prop`). The fourth is the join of the latter two for a recurring component: its time range
//! and property filters must be true of the same effective instance after range anchors and an
//! exact override are composed. Everything else on this page is the shape of the tree.
//!
//! # Two refusals are about the query and not about the resource
//!
//! A contradictory filter (section 9.7.1 makes `is-not-defined` exclusive with every other test)
//! and a collation this crate cannot compare with (section 7.5.1) are properties of the request.
//! Both are therefore settled by a pass over the whole filter *before* the first component is
//! looked at, so the same query cannot be refused for one resource in a collection and answered
//! for the next — which is what a server turns into a `CALDAV:supported-collation` precondition
//! on the `REPORT`, not into a per-resource answer.

use core::mem;

use alloc::vec::Vec;

use crate::internal::core::{
    Component, Document, IgnoreDiagnostics, Item, LimitExceeded, PropertyId,
};
use crate::internal::dav::{CompFilter, PropFilter, TextMatch, TimeRange};

use crate::internal::query::prefilter::Exclusion;
use crate::internal::query::{Budget, Match, QueryError, Zones};

/// What the component filter tree walk is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
pub const COMPONENT_FILTER_SECTIONS: &[&str] = &[
    "RFC 4791 section 9.7, CALDAV:filter",
    "RFC 4791 section 9.7.1, CALDAV:comp-filter",
    "RFC 4791 section 7.5.1, an unsupported collation is refused and never defaulted",
    "RFC 5545 section 3.1, a component name is compared without regard to case",
    "docs/adr/0012, the prefilter runs before anything is expanded",
];

/// The component a `CALDAV:filter`'s one child names, RFC 4791 section 9.7.1.
///
/// "The scope of the `CALDAV:comp-filter` XML element is the calendar object when used as a
/// child of the `CALDAV:filter` XML element", and RFC 5545 section 3.4 gives the calendar object
/// exactly one type.
const CALENDAR_OBJECT: &[u8] = b"VCALENDAR";

/// Whether `calendar` satisfies `filter`, RFC 4791 sections 9.7 and 9.7.1.
///
/// The door into this crate. `filter` is the single child of a `CALDAV:filter` element, and it
/// names the calendar object; `calendar` is one calendar object resource as `ical-core` parsed
/// it; `zones` is the caller's zone source and the zone the query's `CALDAV:timezone` stated;
/// `budget` bounds the work, because the filter came off the wire and the resource came out of a
/// store somebody else writes to.
///
/// The answer has three values and the third one is not a failure: a resource whose comparison
/// needed a zone nothing supplied is [`Match::Undecided`], which is a different claim from
/// [`Match::Unmatched`] and reaches the caller as one (`docs/adr/0003`).
///
/// # Errors
///
/// [`QueryError::Contradictory`] for a filter that states a condition and its own negation,
/// [`QueryError::UnsupportedCollation`] for a `text-match` naming a collation section 7.5 does
/// not define here, and [`QueryError::Limit`] when the walk crosses one of the caller's bounds.
pub fn matches(
    filter: &CompFilter,
    calendar: &Document,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    evaluate(&Units, filter, calendar, zones, budget)
}

/// [`matches`], with the three leaf tests supplied rather than taken from the sibling units.
///
/// The walk and the leaves are separable because they fail differently: a defect in the tree
/// shape returns the wrong set of resources for every query, and a defect in a leaf returns the
/// wrong answer for one kind of test. Splitting them here is what lets this unit's own tests
/// state a leaf's answer and check only what section 9.7.1 does with it.
fn evaluate<L: Leaves>(
    leaves: &L,
    filter: &CompFilter,
    calendar: &Document,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    refuse_impossible(filter)?;
    // Section 9.7.1's scope rule read literally: at the root the scope *is* the calendar object,
    // so a root naming anything else is a filter about an object this resource is not. Answering
    // its `is-not-defined` form with "matched" would make every resource in a collection match a
    // filter whose name attribute was a typo.
    if !filter.name().eq_ignore_ascii_case(CALENDAR_OBJECT) {
        return Ok(Match::Unmatched);
    }
    walk(leaves, calendar, filter, zones, budget)
}

/// The four questions this unit asks and does not answer.
///
/// A trait rather than three direct calls, so the tree walk above has exactly one seam with the
/// rest of the crate and this file's tests can drive it without depending on units that are not
/// this one. [`Units`] is the only implementation that ships.
trait Leaves {
    /// Whether `filter`'s `time-range` can be refused for `calendar` without expanding anything.
    ///
    /// Asked about the whole resource and about the filter rather than about one component,
    /// because a `RECURRENCE-ID` override lives in a sibling of the master it moves and no
    /// bound read off the targeted component alone accounts for it. `false` is always a sound
    /// answer and is the default `docs/adr/0012` fixes, so a prefilter that decides nothing
    /// costs an expansion and never an answer.
    fn excluded(
        &self,
        calendar: &Document,
        filter: &CompFilter,
        zones: Zones<'_>,
        budget: &mut Budget<'_>,
    ) -> bool;

    /// Whether at least one recurrence instance of `component` overlaps `range`.
    ///
    /// RFC 4791 section 9.9: "Time range tests MUST consider every recurrence instance when
    /// testing the time range condition; if any one instance matches, then the test returns
    /// true." `siblings` is the enclosing scope's entries, because a `RECURRENCE-ID` override is
    /// a sibling component and a series read without its overrides is a different series.
    fn overlaps(
        &self,
        component: &Component,
        siblings: &[Item],
        range: TimeRange,
        zones: Zones<'_>,
        budget: &mut Budget<'_>,
    ) -> Result<Match, QueryError>;

    /// Whether `filter` matches `component`, RFC 4791 section 9.7.2.
    fn property(
        &self,
        component: &Component,
        filter: &PropFilter,
        zones: Zones<'_>,
        budget: &mut Budget<'_>,
    ) -> Result<Match, QueryError>;

    /// Whether one effective recurrence instance satisfies a time range and every property
    /// filter together. `None` leaves non-recurring components on the ordinary leaf path.
    fn recurring_properties(
        &self,
        _component: &Component,
        _siblings: &[Item],
        _range: TimeRange,
        _filters: &[PropFilter],
        _zones: Zones<'_>,
        _budget: &mut Budget<'_>,
    ) -> Result<Option<Match>, QueryError> {
        Ok(None)
    }
}

/// The leaves as the sibling units of this crate answer them.
///
/// The only place in this file that names another unit, kept to small delegating bodies on
/// purpose: the tree walk above is the part of this crate that a mistake in returns a wrong
/// *set* of resources, and it is worth being able to read it without following a call into
/// `prefilter`, `expand` or `prop`.
#[derive(Debug)]
struct Units;

impl Leaves for Units {
    fn excluded(
        &self,
        calendar: &Document,
        filter: &CompFilter,
        zones: Zones<'_>,
        budget: &mut Budget<'_>,
    ) -> bool {
        matches!(
            crate::internal::query::prefilter::excludes(calendar, filter, zones, budget),
            Exclusion::Excluded
        )
    }

    fn overlaps(
        &self,
        component: &Component,
        siblings: &[Item],
        range: TimeRange,
        zones: Zones<'_>,
        budget: &mut Budget<'_>,
    ) -> Result<Match, QueryError> {
        // `expand` owns the assembly of a component's recurrence set as well as the search over
        // it, which is what its own documentation promises: everything above it "asks 'does this
        // component occupy a period overlapping that range' and gets an answer; everything about
        // how that answer is obtained is here". Diagnostics are dropped rather than routed,
        // because this crate's front door takes no sink — see the note on [`matches`].
        crate::internal::query::expand::component_overlaps(
            component,
            siblings,
            range,
            zones,
            budget,
            &mut IgnoreDiagnostics,
        )
    }

    fn property(
        &self,
        component: &Component,
        filter: &PropFilter,
        zones: Zones<'_>,
        budget: &mut Budget<'_>,
    ) -> Result<Match, QueryError> {
        crate::internal::query::prop::matches_prop_filter(component, filter, zones, budget)
    }

    fn recurring_properties(
        &self,
        component: &Component,
        siblings: &[Item],
        range: TimeRange,
        filters: &[PropFilter],
        zones: Zones<'_>,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Match>, QueryError> {
        use crate::internal::query::{expand, subset};

        if subset::is_override(component) {
            return Ok(Some(Match::Unmatched));
        }
        let related: Vec<&Component> = siblings
            .iter()
            .filter_map(Item::as_component)
            .filter(|candidate| subset::is_override_of(candidate, component))
            .collect();
        let recurring = component
            .properties_named(&PropertyId::RRULE)
            .next()
            .is_some()
            || component
                .properties_named(&PropertyId::RDATE)
                .next()
                .is_some()
            || !related.is_empty();
        if !recurring {
            return Ok(None);
        }

        let expanded = expand::expand_component(component, siblings, range, zones, budget)?;
        let mut any = Match::Unmatched;
        for instance in expanded.instances() {
            let effective = subset::effective_template(component, &related, *instance, zones)?;
            let mut all = Match::Matched;
            for filter in filters {
                all = all.and(self.property(&effective, filter, zones, budget)?);
                if all == Match::Unmatched {
                    break;
                }
            }
            any = any.or(all);
            if any.is_matched() {
                break;
            }
        }
        if let Some(reason) = expanded.incomplete() {
            any = any.or(Match::Undecided(reason));
        }
        Ok(Some(any))
    }
}

/// One `comp-filter` being evaluated against one scope, and how far that has got.
///
/// The state a recursive walk would have kept in a stack frame, made explicit because the walk
/// is over the product of two trees whose depths are two different caller-stated bounds, and a
/// process that overflows its stack aborts rather than unwinds.
#[derive(Debug)]
struct Frame<'a> {
    /// The filter this frame answers.
    filter: &'a CompFilter,
    /// The entries of the enclosing scope, which the candidates are drawn from.
    scope: &'a [Item],
    /// The next entry of `scope` to consider.
    next: usize,
    /// The candidate under test, once one has been found.
    current: Option<&'a Component>,
    /// The next child filter to test against `current`.
    child: usize,
    /// The disjunction over the candidates settled so far.
    any: Match,
    /// The conjunction over the tests answered for `current` so far.
    all: Match,
    /// Whether the prefilter refused this filter's `time-range` for the whole resource.
    excluded: bool,
}

/// Open a frame for `filter` over `scope`, asking the prefilter once for the whole frame.
///
/// `docs/adr/0012` puts the expansion-free bounds in front of the expansion, and the answer is
/// about the resource and the filter rather than about one candidate — so it is taken once here
/// and read from the frame for every candidate the filter is then tried against, instead of
/// being asked again per component. A filter with no `time-range` is not asked at all: there is
/// no window to be outside of, and the prefilter would answer "cannot exclude" for it anyway.
///
/// `any` starts unmatched because a disjunction over no candidate is unmatched, which is section
/// 9.7.1's first bullet read backwards: a name with no component in scope does not exist there.
fn opened<'a, L: Leaves>(
    leaves: &L,
    calendar: &Document,
    filter: &'a CompFilter,
    scope: &'a [Item],
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Frame<'a> {
    let excluded = filter.time_range.is_some() && leaves.excluded(calendar, filter, zones, budget);
    Frame {
        filter,
        scope,
        next: 0,
        current: None,
        child: 0,
        any: Match::Unmatched,
        all: Match::Unmatched,
        excluded,
    }
}

/// What one step of the walk did.
#[derive(Debug)]
enum Advance<'a> {
    /// Answer this filter against these entries before going further.
    Descend(&'a CompFilter, &'a [Item]),
    /// The frame's filter is answered.
    Settled(Match),
    /// The frame moved and has more to do.
    Continue,
}

/// Answer `root` against `calendar`'s own entries, over an explicit stack.
fn walk<'a, L: Leaves>(
    leaves: &L,
    calendar: &'a Document,
    root: &'a CompFilter,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    budget.meter.try_enter_element()?;
    let mut top = opened(leaves, calendar, root, calendar.items(), zones, budget);
    let mut outer: Vec<Frame<'a>> = Vec::new();
    loop {
        match advance(leaves, &mut top, zones, budget)? {
            Advance::Descend(child, inside) => {
                budget.meter.try_enter_element()?;
                outer.try_reserve(1).map_err(|_| LimitExceeded::Depth)?;
                let below = opened(leaves, calendar, child, inside, zones, budget);
                outer.push(mem::replace(&mut top, below));
            },
            Advance::Settled(answer) => {
                budget.meter.leave_element();
                let Some(parent) = outer.pop() else {
                    return Ok(answer);
                };
                top = parent;
                // The child filter is one of the tests the parent's current candidate has to
                // satisfy, so it joins the conjunction — `and`, never `&&`, because an
                // undecidable child has to survive to the top.
                top.all = top.all.and(answer);
            },
            Advance::Continue => {},
        }
    }
}

/// Move `frame` one step: open a candidate, descend into a child filter, or settle.
fn advance<'a, L: Leaves>(
    leaves: &L,
    frame: &mut Frame<'a>,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Advance<'a>, QueryError> {
    let Some(candidate) = frame.current else {
        return open(leaves, frame, zones, budget);
    };
    // Taken out of the frame before the arms write back into it: the child filters live in the
    // filter tree rather than in the frame, so nothing here borrows what the arms mutate.
    let stated = frame.filter;
    match stated.comps().get(frame.child) {
        // An already-unmatched conjunction cannot be rescued by anything below it, so the
        // remaining child filters are work an attacker would otherwise be choosing for us.
        Some(child) if frame.all != Match::Unmatched => {
            frame.child = frame.child.saturating_add(1);
            Ok(Advance::Descend(child, candidate.items()))
        },
        _ => {
            frame.any = frame.any.or(frame.all);
            // Section 9.7.1 is satisfied by any one component in scope, so a matched candidate
            // decides the filter and the rest of the scope is not looked at.
            if frame.any.is_matched() {
                return Ok(Advance::Settled(Match::Matched));
            }
            frame.current = None;
            Ok(Advance::Continue)
        },
    }
}

/// Find the next candidate and answer the tests the filter states about the component itself.
fn open<'a, L: Leaves>(
    leaves: &L,
    frame: &mut Frame<'a>,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Advance<'a>, QueryError> {
    let Some(candidate) = next_candidate(frame, budget)? else {
        // Section 9.7.1's second bullet: `is-not-defined` matches exactly when no component of
        // the name exists in scope, and the scope has now been read to the end without one.
        // Otherwise the answer is the disjunction, which for no candidate at all is unmatched.
        let exhausted = if frame.filter.is_not_defined {
            Match::Matched
        } else {
            frame.any
        };
        return Ok(Advance::Settled(exhausted));
    };
    if frame.filter.is_not_defined {
        return Ok(Advance::Settled(Match::Unmatched));
    }
    // Answered before the frame is written to, so that the tests read a frame that still
    // describes the candidate they are about.
    let held = own_tests(leaves, frame, candidate, zones, budget)?;
    frame.current = Some(candidate);
    frame.child = 0;
    frame.all = held;
    Ok(Advance::Continue)
}

/// The next component of the frame's scope carrying the filter's name.
///
/// Every component the walk inspects is charged for by name, which is the charge `ical-dav`
/// makes for the same name on the way in. The depth of the walk is bounded by the element depth;
/// what this bounds is its *width* times its depth, which is the product the filter tree and the
/// calendar tree make and the only dimension in which a small filter and a small calendar are
/// still expensive together.
fn next_candidate<'a>(
    frame: &mut Frame<'a>,
    budget: &mut Budget<'_>,
) -> Result<Option<&'a Component>, QueryError> {
    let scope = frame.scope;
    while let Some(item) = scope.get(frame.next) {
        frame.next = frame.next.saturating_add(1);
        let Some(component) = item.as_component() else {
            continue;
        };
        let name = component.name();
        budget
            .meter
            .try_charge_bytes(u64::try_from(name.len()).unwrap_or(u64::MAX))?;
        // RFC 5545 section 3.1 compares a component name without regard to case, which is what
        // `Component::is_named` does; the filter's name arrived as XML character data and is
        // compared as the octets the peer wrote.
        if component.is_named(frame.filter.name()) {
            return Ok(Some(component));
        }
    }
    Ok(None)
}

/// The tests one filter states about the candidate itself, conjoined.
///
/// The child `comp-filter`s are not here: they are answered by frames of their own, because
/// each of them opens a new scope and the walk may not recurse to do that.
///
/// The `time-range` goes first and the `prop-filter`s follow, so that the one test that expands
/// a series is the one an already-unmatched conjunction skips. `docs/adr/0012`'s ordering is
/// upstream of both: an excluded frame answers the window without expanding anything at all,
/// and an exclusion is [`Match::Unmatched`] rather than undecidable, because the prefilter is
/// required to be sound — it says "excluded" only where the walk would have said unmatched.
fn own_tests<L: Leaves>(
    leaves: &L,
    frame: &Frame<'_>,
    candidate: &Component,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    let mut held = Match::Matched;
    if let Some(range) = frame.filter.time_range {
        if !frame.excluded && !frame.filter.props().is_empty() {
            if let Some(joined) = leaves.recurring_properties(
                candidate,
                frame.scope,
                range,
                frame.filter.props(),
                zones,
                budget,
            )? {
                return Ok(joined);
            }
        }
        held = held.and(if frame.excluded {
            Match::Unmatched
        } else {
            leaves.overlaps(candidate, frame.scope, range, zones, budget)?
        });
    }
    for wanted in frame.filter.props() {
        if held == Match::Unmatched {
            break;
        }
        held = held.and(leaves.property(candidate, wanted, zones, budget)?);
    }
    Ok(held)
}

/// Refuse a filter no resource could answer, before any resource is looked at.
///
/// A pass over the whole filter tree rather than a check at each frame, because both refusals
/// are facts about the request: a walk that only reached a contradictory subtree for calendars
/// that happen to hold the enclosing component would refuse the query for some resources of a
/// collection and answer it for the rest, and a `REPORT` cannot be half a precondition.
fn refuse_impossible(root: &CompFilter) -> Result<(), QueryError> {
    let mut pending: Vec<&CompFilter> = Vec::new();
    pending.try_reserve(1).map_err(|_| LimitExceeded::Depth)?;
    pending.push(root);
    while let Some(filter) = pending.pop() {
        // RFC 4791 section 9.7.1 makes `is-not-defined` exclusive with every other test: a
        // component that is not there has no time range and no properties.
        if filter.is_contradictory() {
            return Err(QueryError::Contradictory);
        }
        for wanted in filter.props() {
            refuse_impossible_property(wanted)?;
        }
        pending
            .try_reserve(filter.comps().len())
            .map_err(|_| LimitExceeded::Depth)?;
        pending.extend(filter.comps());
    }
    Ok(())
}

/// The same two refusals for one `prop-filter` and the `param-filter`s under it.
fn refuse_impossible_property(filter: &PropFilter) -> Result<(), QueryError> {
    if filter.is_contradictory() {
        return Err(QueryError::Contradictory);
    }
    refuse_unsupported_collation(filter.text_match.as_ref())?;
    for wanted in filter.params() {
        if wanted.is_contradictory() {
            return Err(QueryError::Contradictory);
        }
        refuse_unsupported_collation(wanted.text_match.as_ref())?;
    }
    Ok(())
}

/// Refuse a `text-match` naming a collation this crate does not compare with.
///
/// Through `collate`, which owns the mapping including RFC 4791 section 7.5's rule that the
/// reserved `default` identifier means `i;ascii-casemap`; a second reading of it here would be a
/// second place for that rule to drift. Section 7.5.1 gives a server the
/// `CALDAV:supported-collation` precondition for exactly this refusal, so that it does not run
/// the test under a collation the client did not ask for — which returns a different set of
/// resources and says nothing about having done so.
fn refuse_unsupported_collation(text_match: Option<&TextMatch>) -> Result<(), QueryError> {
    match text_match {
        Some(test) => crate::internal::query::collate::collator_of(&test.collation).map(|_| ()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use alloc::vec::Vec;

    use crate::internal::core::{
        Component, Document, IgnoreDiagnostics, Instant, Item, LimitExceeded, Limits, Meter,
        UtcOffset,
    };
    use crate::internal::dav::{
        Collation, CompFilter, ParamFilter, PropFilter, TextMatch, TimeRange,
    };
    use crate::internal::tz::FixedOffsetSource;

    use super::{Leaves, evaluate};
    use crate::internal::query::{Budget, Match, QueryError, Undecided, Zones};

    /// A calendar object holding one `VEVENT` and nothing else.
    const WITH_EVENT: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VEVENT\r\nUID:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// A calendar object holding one `VTODO` and no `VEVENT`.
    const WITH_TODO: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VTODO\r\nUID:1\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// A `VTODO` carrying a `VALARM`, the shape RFC 4791 section 7.8.5 queries.
    const WITH_ALARM: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VTODO\r\nUID:1\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nEND:VALARM\r\n\
        END:VTODO\r\nEND:VCALENDAR\r\n";

    /// Two `VEVENT`s, each carrying one of the two properties a filter may ask for together.
    const SPLIT_EVENTS: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VEVENT\r\nUID:1\r\nX-ONE:a\r\nEND:VEVENT\r\n\
        BEGIN:VEVENT\r\nUID:2\r\nX-TWO:b\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// One `VEVENT` carrying both of those properties.
    const JOINED_EVENT: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VEVENT\r\nUID:1\r\nX-ONE:a\r\nX-TWO:b\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// Two `VEVENT`s, the first of which the scripted overlap cannot place on a timeline.
    const FLOATING_THEN_FIXED: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VEVENT\r\nUID:1\r\nX-FLOATING:y\r\nEND:VEVENT\r\n\
        BEGIN:VEVENT\r\nUID:2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// One `VEVENT`, which the scripted overlap cannot place on a timeline at all.
    const FLOATING_ONLY: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VEVENT\r\nUID:1\r\nX-FLOATING:y\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// The first instant of 2026, as a `time-range` start.
    const WINDOW_START: i64 = 1_767_225_600;

    /// A week later, as a `time-range` end.
    const WINDOW_END: i64 = 1_767_830_400;

    /// How deep the product walk is driven, inside both caller-stated depth bounds.
    const DEEP: usize = 20;

    /// The leaves, scripted, so that a row states what a leaf answered rather than computing it.
    ///
    /// `property` is the one leaf rule this stub really implements, and it implements only RFC
    /// 4791 section 9.7.2's first bullet — an empty `prop-filter` matches when a property of the
    /// name exists — because that is the whole of what section 9.7.1's own rules need in order
    /// to be exercised. Everything else is a value the row supplies.
    #[derive(Debug)]
    struct Scripted {
        /// What an overlap test answers for a component with no `X-FLOATING` property.
        range: Match,
        /// Whether the prefilter excludes every component it is asked about.
        excludes: bool,
        /// How many times the prefilter was asked.
        asked: Cell<u32>,
        /// How many times an expansion was reached.
        expanded: Cell<u32>,
    }

    impl Scripted {
        /// A stub whose overlap test answers `range` and whose prefilter excludes nothing.
        const fn answering(range: Match) -> Self {
            Self {
                range,
                excludes: false,
                asked: Cell::new(0),
                expanded: Cell::new(0),
            }
        }

        /// A stub whose prefilter excludes everything it is asked about.
        const fn excluding() -> Self {
            Self {
                range: Match::Matched,
                excludes: true,
                asked: Cell::new(0),
                expanded: Cell::new(0),
            }
        }
    }

    impl Leaves for Scripted {
        fn excluded(
            &self,
            _calendar: &Document,
            _filter: &CompFilter,
            _zones: Zones<'_>,
            _budget: &mut Budget<'_>,
        ) -> bool {
            self.asked.set(self.asked.get().saturating_add(1));
            self.excludes
        }

        fn overlaps(
            &self,
            component: &Component,
            _siblings: &[Item],
            _range: TimeRange,
            _zones: Zones<'_>,
            _budget: &mut Budget<'_>,
        ) -> Result<Match, QueryError> {
            self.expanded.set(self.expanded.get().saturating_add(1));
            if carries(component, b"X-FLOATING") {
                return Ok(Match::Undecided(Undecided::ZoneUnstated));
            }
            Ok(self.range)
        }

        fn property(
            &self,
            component: &Component,
            filter: &PropFilter,
            _zones: Zones<'_>,
            _budget: &mut Budget<'_>,
        ) -> Result<Match, QueryError> {
            Ok(Match::of(carries(component, filter.name())))
        }
    }

    /// Whether `component` carries a property named `name`.
    fn carries(component: &Component, name: &[u8]) -> bool {
        component.properties().any(|line| line.is_named(name))
    }

    /// The calendar object `text` spells.
    fn calendar_of(text: &[u8]) -> Document {
        Document::parse(text, Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
    }

    /// A filter naming one component and stating nothing about it.
    fn named(name: &[u8]) -> CompFilter {
        let mut scratch = Meter::new(Limits::DEFAULT);
        CompFilter::new(name, Limits::DEFAULT, &mut scratch).unwrap()
    }

    /// `outer` with `inner` nested inside it.
    fn nest(mut outer: CompFilter, inner: CompFilter) -> CompFilter {
        let mut scratch = Meter::new(Limits::DEFAULT);
        outer
            .push_comp(inner, Limits::DEFAULT, &mut scratch)
            .unwrap();
        outer
    }

    /// `filter` with an empty `prop-filter` naming `name` added.
    fn with_prop(mut filter: CompFilter, name: &[u8]) -> CompFilter {
        let mut scratch = Meter::new(Limits::DEFAULT);
        let wanted = PropFilter::new(name, Limits::DEFAULT, &mut scratch).unwrap();
        filter.push_prop(wanted, &mut scratch).unwrap();
        filter
    }

    /// `filter` with a `time-range` over the first week of 2026.
    fn with_range(mut filter: CompFilter) -> CompFilter {
        filter.time_range = Some(
            TimeRange::new(
                Some(Instant::from_unix_seconds(WINDOW_START)),
                Some(Instant::from_unix_seconds(WINDOW_END)),
            )
            .unwrap(),
        );
        filter
    }

    /// `filter` with `CALDAV:is-not-defined` set.
    fn undefined(mut filter: CompFilter) -> CompFilter {
        filter.is_not_defined = true;
        filter
    }

    /// Run `filter` against `text` with the scripted leaves and the default policy.
    fn answer(filter: &CompFilter, text: &[u8], leaves: &Scripted) -> Result<Match, QueryError> {
        let calendar = calendar_of(text);
        let source = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
        let mut ledger = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut ledger);
        evaluate(leaves, filter, &calendar, Zones::new(&source), &mut budget)
    }

    /// One row of a table, with its expectation taken from the passage `reason` names.
    struct Case {
        /// The passage of RFC 4791 the row transcribes.
        reason: &'static str,
        /// The calendar object under test.
        calendar: &'static [u8],
        /// The filter, built here because a filter is built rather than spelled.
        build: fn() -> CompFilter,
        /// What the scripted overlap test answers for an ordinary component.
        overlap: Match,
        /// What the named passage says the answer is.
        expected: Match,
    }

    /// Run every row and report the passage rather than the row index on a failure.
    fn check(table: &[Case]) {
        for case in table {
            let leaves = Scripted::answering(case.overlap);
            let got = answer(&(case.build)(), case.calendar, &leaves);
            assert_eq!(got, Ok(case.expected), "{}", case.reason);
        }
    }

    /// The filter both `is-not-defined` rows use, kept out of the rows themselves.
    fn alarm_absent() -> CompFilter {
        let todo = nest(named(b"VTODO"), undefined(named(b"VALARM")));
        nest(named(b"VCALENDAR"), todo)
    }

    /// A `VEVENT` filter asking for two properties at once.
    fn wants_both() -> CompFilter {
        let event = with_prop(with_prop(named(b"VEVENT"), b"X-ONE"), b"X-TWO");
        nest(named(b"VCALENDAR"), event)
    }

    #[test]
    fn an_empty_comp_filter_asks_only_whether_the_named_component_is_in_scope() {
        check(&[
            Case {
                reason: "9.7.1 first bullet: the named component exists in scope",
                calendar: WITH_EVENT,
                build: || nest(named(b"VCALENDAR"), named(b"VEVENT")),
                overlap: Match::Matched,
                expected: Match::Matched,
            },
            Case {
                reason: "9.7.1 first bullet: and here it does not",
                calendar: WITH_TODO,
                build: || nest(named(b"VCALENDAR"), named(b"VEVENT")),
                overlap: Match::Matched,
                expected: Match::Unmatched,
            },
            Case {
                reason: "9.7.1 first bullet, section 9.5's own example: the root alone",
                calendar: WITH_TODO,
                build: || named(b"VCALENDAR"),
                overlap: Match::Matched,
                expected: Match::Matched,
            },
        ]);
    }

    #[test]
    fn is_not_defined_asks_whether_the_scope_holds_no_such_component() {
        check(&[
            Case {
                reason: "9.7.1 second bullet: no VALARM in the enclosing VTODO",
                calendar: WITH_TODO,
                build: alarm_absent,
                overlap: Match::Matched,
                expected: Match::Matched,
            },
            Case {
                reason: "9.7.1 second bullet: and here one is there",
                calendar: WITH_ALARM,
                build: alarm_absent,
                overlap: Match::Matched,
                expected: Match::Unmatched,
            },
            Case {
                reason: "9.7.1 second bullet: the calendar object itself is there",
                calendar: WITH_EVENT,
                build: || undefined(named(b"VCALENDAR")),
                overlap: Match::Matched,
                expected: Match::Unmatched,
            },
        ]);
    }

    #[test]
    fn a_time_range_asks_whether_any_one_instance_overlaps_the_window() {
        check(&[
            Case {
                reason: "9.7.1 third bullet: an instance overlaps",
                calendar: WITH_EVENT,
                build: || nest(named(b"VCALENDAR"), with_range(named(b"VEVENT"))),
                overlap: Match::Matched,
                expected: Match::Matched,
            },
            Case {
                reason: "9.7.1 third bullet: no instance overlaps",
                calendar: WITH_EVENT,
                build: || nest(named(b"VCALENDAR"), with_range(named(b"VEVENT"))),
                overlap: Match::Unmatched,
                expected: Match::Unmatched,
            },
            Case {
                reason: "9.9 with docs/adr/0003: an overlap with no timeline is undecided",
                calendar: FLOATING_ONLY,
                build: || nest(named(b"VCALENDAR"), with_range(named(b"VEVENT"))),
                overlap: Match::Matched,
                expected: Match::Undecided(Undecided::ZoneUnstated),
            },
            Case {
                reason: "9.7.1 third bullet: one matched candidate decides it for the rest",
                calendar: FLOATING_THEN_FIXED,
                build: || nest(named(b"VCALENDAR"), with_range(named(b"VEVENT"))),
                overlap: Match::Matched,
                expected: Match::Matched,
            },
        ]);
    }

    #[test]
    fn every_child_filter_must_match_the_one_targeted_component() {
        check(&[
            Case {
                reason: "9.7.1 fourth bullet: two components each satisfying half satisfy none",
                calendar: SPLIT_EVENTS,
                build: wants_both,
                overlap: Match::Matched,
                expected: Match::Unmatched,
            },
            Case {
                reason: "9.7.1 fourth bullet: and one component satisfying both does",
                calendar: JOINED_EVENT,
                build: wants_both,
                overlap: Match::Matched,
                expected: Match::Matched,
            },
            Case {
                reason: "9.7.1 fourth bullet: any one subcomponent is enough, whichever it is",
                calendar: SPLIT_EVENTS,
                build: || nest(named(b"VCALENDAR"), with_prop(named(b"VEVENT"), b"X-TWO")),
                overlap: Match::Matched,
                expected: Match::Matched,
            },
        ]);
    }

    #[test]
    fn the_root_names_the_calendar_object_and_names_are_compared_without_case() {
        check(&[
            Case {
                reason: "9.7.1 scope rule: the root's scope is the calendar object",
                calendar: WITH_EVENT,
                build: || nest(named(b"VEVENT"), named(b"VALARM")),
                overlap: Match::Matched,
                expected: Match::Unmatched,
            },
            Case {
                reason: "RFC 5545 section 3.1: a component name is compared without case",
                calendar: WITH_EVENT,
                build: || nest(named(b"vcalendar"), named(b"vevent")),
                overlap: Match::Matched,
                expected: Match::Matched,
            },
        ]);
    }

    #[test]
    fn a_filter_stating_a_condition_and_its_own_negation_is_refused() {
        // RFC 4791 section 9.7.1 writes `(is-not-defined | (time-range?, prop-filter*,
        // comp-filter*))`, so the two halves are alternatives and a value holding both is one no
        // body expresses.
        let nested = nest(named(b"VCALENDAR"), with_range(undefined(named(b"VEVENT"))));
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&nested, WITH_EVENT, &leaves),
            Err(QueryError::Contradictory)
        );
    }

    #[test]
    fn a_contradiction_is_refused_for_a_resource_that_never_reaches_it() {
        // The refusal is a fact about the request. A walk that only found the contradiction
        // under components a particular resource happens to carry would answer the same query
        // for half a collection and refuse it for the other half.
        let todo = nest(named(b"VTODO"), with_range(undefined(named(b"VALARM"))));
        let filter = nest(named(b"VCALENDAR"), todo);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&filter, WITH_EVENT, &leaves),
            Err(QueryError::Contradictory)
        );
    }

    #[test]
    fn a_contradictory_property_filter_is_refused_with_the_filter_that_holds_it() {
        let mut scratch = Meter::new(Limits::DEFAULT);
        let mut wanted = PropFilter::new(b"SUMMARY", Limits::DEFAULT, &mut scratch).unwrap();
        wanted.is_not_defined = true;
        wanted.text_match = Some(TextMatch::new(b"meeting", &mut scratch).unwrap());
        let mut event = named(b"VEVENT");
        event.push_prop(wanted, &mut scratch).unwrap();
        let filter = nest(named(b"VCALENDAR"), event);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&filter, WITH_EVENT, &leaves),
            Err(QueryError::Contradictory)
        );
    }

    #[test]
    fn a_collation_this_crate_does_not_compare_with_is_refused_and_never_defaulted() {
        // RFC 4791 section 7.5.1: the server answers with the CALDAV:supported-collation
        // precondition rather than running the test under a collation nobody asked for.
        let mut scratch = Meter::new(Limits::DEFAULT);
        let mut wanted = PropFilter::new(b"SUMMARY", Limits::DEFAULT, &mut scratch).unwrap();
        let mut test = TextMatch::new(b"meeting", &mut scratch).unwrap();
        test.collation = Collation::parse(b"i;unicode-casemap").unwrap();
        wanted.text_match = Some(test);
        let mut event = named(b"VEVENT");
        event.push_prop(wanted, &mut scratch).unwrap();
        let filter = nest(named(b"VCALENDAR"), event);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&filter, WITH_EVENT, &leaves),
            Err(QueryError::UnsupportedCollation)
        );
    }

    #[test]
    fn an_unsupported_collation_on_a_parameter_filter_is_refused_too() {
        let mut scratch = Meter::new(Limits::DEFAULT);
        let mut parameter = ParamFilter::new(b"PARTSTAT", &mut scratch).unwrap();
        let mut test = TextMatch::new(b"ACCEPTED", &mut scratch).unwrap();
        test.collation = Collation::parse(b"i;unicode-casemap").unwrap();
        parameter.text_match = Some(test);
        let mut wanted = PropFilter::new(b"ATTENDEE", Limits::DEFAULT, &mut scratch).unwrap();
        wanted.push_param(parameter, &mut scratch).unwrap();
        let mut event = named(b"VEVENT");
        event.push_prop(wanted, &mut scratch).unwrap();
        let filter = nest(named(b"VCALENDAR"), event);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&filter, WITH_EVENT, &leaves),
            Err(QueryError::UnsupportedCollation)
        );
    }

    #[test]
    fn nothing_is_expanded_before_the_prefilter_has_been_asked() {
        // docs/adr/0012 fixes the ordering, and an exclusion is unmatched rather than an
        // expansion that happened to find nothing.
        let filter = nest(named(b"VCALENDAR"), with_range(named(b"VEVENT")));
        let leaves = Scripted::excluding();
        assert_eq!(answer(&filter, WITH_EVENT, &leaves), Ok(Match::Unmatched));
        assert_eq!(leaves.asked.get(), 1);
        assert_eq!(leaves.expanded.get(), 0);
    }

    #[test]
    fn a_prefilter_that_excludes_nothing_costs_an_expansion_and_no_answer() {
        let filter = nest(named(b"VCALENDAR"), with_range(named(b"VEVENT")));
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(answer(&filter, WITH_EVENT, &leaves), Ok(Match::Matched));
        assert_eq!(leaves.asked.get(), 1);
        assert_eq!(leaves.expanded.get(), 1);
    }

    #[test]
    fn the_walk_refuses_at_the_element_depth_the_caller_stated() {
        // The filter was built under one policy and is evaluated under another, which is the
        // ordinary case: the filter came off the wire and the meter is this caller's.
        let filter = nest(named(b"VCALENDAR"), named(b"VEVENT"));
        let tight = Limits::DEFAULT.with_max_xml_depth(1);
        let calendar = calendar_of(WITH_EVENT);
        let source = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
        let mut ledger = Meter::new(tight);
        let mut budget = Budget::new(tight, &mut ledger);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            evaluate(
                &leaves,
                &filter,
                &calendar,
                Zones::new(&source),
                &mut budget
            ),
            Err(QueryError::Limit(LimitExceeded::Depth))
        );
    }

    #[test]
    fn the_walk_refuses_at_the_octet_budget_the_caller_stated() {
        // The product of the two trees is charged by the name of every component inspected, so a
        // budget smaller than one component name refuses before the first candidate is decided.
        let filter = nest(named(b"VCALENDAR"), named(b"VEVENT"));
        let calendar = calendar_of(WITH_EVENT);
        let source = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
        let mut ledger = Meter::with_budget(Limits::DEFAULT, 4);
        let mut budget = Budget::new(Limits::DEFAULT, &mut ledger);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            evaluate(
                &leaves,
                &filter,
                &calendar,
                Zones::new(&source),
                &mut budget
            ),
            Err(QueryError::Limit(LimitExceeded::Budget))
        );
        assert!(budget.is_exhausted());
    }

    #[test]
    fn the_product_of_two_deep_trees_is_walked_without_the_callers_stack() {
        let mut text = Vec::new();
        text.extend_from_slice(b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
        for _ in 0..DEEP {
            text.extend_from_slice(b"BEGIN:X-LEVEL\r\n");
        }
        for _ in 0..DEEP {
            text.extend_from_slice(b"END:X-LEVEL\r\n");
        }
        text.extend_from_slice(b"END:VCALENDAR\r\n");

        let mut filter = named(b"X-LEVEL");
        for _ in 1..DEEP {
            filter = nest(named(b"X-LEVEL"), filter);
        }
        let root = nest(named(b"VCALENDAR"), filter);

        let calendar = calendar_of(&text);
        let source = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
        let mut ledger = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut ledger);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            evaluate(&leaves, &root, &calendar, Zones::new(&source), &mut budget),
            Ok(Match::Matched)
        );
    }

    #[test]
    fn an_undecided_child_survives_a_matched_sibling_test() {
        // Kleene, not `&&`: the conjunction of a matched property filter and an undecidable
        // time range is undecidable, and a caller that saw "unmatched" would drop a resource it
        // was never established to be outside the window.
        let event = with_prop(with_range(named(b"VEVENT")), b"X-FLOATING");
        let filter = nest(named(b"VCALENDAR"), event);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&filter, FLOATING_THEN_FIXED, &leaves),
            Ok(Match::Undecided(Undecided::ZoneUnstated))
        );
    }

    #[test]
    fn an_unmatched_test_beats_an_undecided_one_in_the_same_filter() {
        // The other half of the same rule: no reading of the undecidable operand could have
        // rescued a conjunction whose other operand is a fact.
        let event = with_prop(with_range(named(b"VEVENT")), b"X-ABSENT");
        let filter = nest(named(b"VCALENDAR"), event);
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&filter, FLOATING_THEN_FIXED, &leaves),
            Ok(Match::Unmatched)
        );
    }

    #[test]
    fn an_undecided_subtree_reaches_the_root_through_a_nested_filter() {
        let alarm = with_range(named(b"VALARM"));
        let event = nest(named(b"VEVENT"), alarm);
        let filter = nest(named(b"VCALENDAR"), event);
        let text: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
            BEGIN:VEVENT\r\nUID:1\r\nBEGIN:VALARM\r\nX-FLOATING:y\r\nEND:VALARM\r\n\
            END:VEVENT\r\nEND:VCALENDAR\r\n";
        let leaves = Scripted::answering(Match::Matched);
        assert_eq!(
            answer(&filter, text, &leaves),
            Ok(Match::Undecided(Undecided::ZoneUnstated))
        );
    }
}
