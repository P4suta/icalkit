// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit 3 — property and parameter filters. RFC 4791 sections 9.7.2 and 9.7.3.
//!
//! # What this unit owns
//!
//! Given one `ical_core::Component` and one `ical_dav::PropFilter`, answer a [`crate::internal::query::Match`];
//! and the same for a `ParamFilter` against one property. The two are one unit because they
//! share the whole of their structure — a name, an `is-not-defined`, an optional `text-match`,
//! and a list of children — and splitting them would be two copies of one walk.
//!
//! - A property filter matches a component when **any** occurrence of the named property
//!   satisfies every test in the filter. Section 9.7.2 states the match as a property of "a
//!   calendar property" rather than of the component, and section 9.9 says the same thing about
//!   instances outright — "if any one instance matches, then the test returns true" — so a
//!   component with three `ATTENDEE` lines matches if one of them does.
//! - `CALDAV:is-not-defined` matches when the component carries **no** occurrence of the name,
//!   and is exclusive with every other test in the same filter. `ical-dav` reports that
//!   contradiction through `PropFilter::is_contradictory`; this unit refuses one with
//!   [`crate::internal::query::QueryError::Contradictory`] rather than deciding it.
//! - A `time-range` on a property tests the *value* of that property, which is a different
//!   question from the component overlap `overlap` answers. See below: it is defined for seven
//!   named properties and for nothing else.
//! - A `text-match` runs through `collate` against the property's value as `ical-core` preserved
//!   it, with `negate-condition` applied through [`crate::internal::query::Match::negate`].
//! - A parameter filter is the same shape one level down, against the parameters of the
//!   occurrence being tested — not against the parameters of any occurrence of the name.
//!   Section 9.7.2's third and fourth bullets both end "and all specified `CALDAV:param-filter`
//!   child XML elements also match **the targeted property**", so the conjunction is taken per
//!   occurrence and the existential is taken over the conjunctions, never the other way round.
//!
//! # A property `time-range` is a closed list of seven names, not a value-type test
//!
//! This is the one place the shape of this unit was decided by reading section 9.9 rather than
//! by reasoning about values, and the two do not agree. Section 9.9 writes exactly one
//! property-level rule:
//!
//! > The calendar properties `COMPLETED`, `CREATED`, `DTEND`, `DTSTAMP`, `DTSTART`, `DUE`, and
//! > `LAST-MODIFIED` overlap a given time range if the following condition holds:
//! > `(start <= date-time) AND (end > date-time)`
//!
//! and closes the list in the next paragraph but one: "The semantic of `CALDAV:time-range` is
//! not defined for any other calendar components and properties." So the gate is the property
//! *name*, and a `prop-filter` naming `FREEBUSY`, `TRIGGER`, `RECURRENCE-ID` or `SUMMARY` with a
//! `time-range` inside it is [`crate::internal::query::Undecided::OverlapUndefined`] — including `FREEBUSY`,
//! whose periods section 9.9 does compare against a range but only as part of the *`VFREEBUSY`
//! component* rule, which is `overlap`'s row and not this unit's. A `PERIOD`-valued property
//! therefore never reaches the comparison here, because no property that can carry one is on the
//! list.
//!
//! The condition is a point test, so a `DATE` value is placed at the first instant of the day it
//! names, in whichever zone applies to a value that names none. Section 9.9's `+P1D` readings
//! are stated for *components* — a `VEVENT` with a `DATE` `DTSTART` and no `DTEND`, a `VJOURNAL`
//! — and extending them to the property rule would make this crate answer a question the
//! specification wrote a different rule for.
//!
//! # Names are compared case-insensitively and values are not
//!
//! RFC 5545 section 3.1 makes property and parameter names case-insensitive, so `summary` and
//! `SUMMARY` name one property; a value's case is the collation's business and never this
//! unit's. A parameter's value is read with its section 3.2 `DQUOTE` pair removed, because
//! `PARTSTAT="ACCEPTED"` and `PARTSTAT=ACCEPTED` are one value written two ways and a substring
//! test that saw the quotes would answer differently for the two.
//!
//! # What it must not do
//!
//! Decode a value it does not need. `ical-core`'s typed views parse on demand, and a
//! `text-match` runs against preserved octets — a filter that decoded every property of every
//! component to test one of them is the cost this crate is measured on. The tests inside one
//! filter are therefore evaluated cheapest first, which is sound because section 9.7.2 conjoins
//! them and [`crate::internal::query::Match::and`] is decided by an unmatched operand whichever side it arrives
//! on. A value that must be decoded and does not is [`crate::internal::query::Undecided::ValueUnreadable`] and
//! never "does not match".

use core::str::from_utf8;

use ical_core::{
    CivilDateTime, CivilTime, Component, DateTimeValue, DecodeValue, Instant, Property, UtcOffset,
};
use ical_dav::{ParamFilter, PropFilter, TextMatch, TimeRange};

use crate::internal::query::collate;
use crate::internal::query::vocabulary::{Budget, Match, QueryError, Undecided, Zones};

/// What property and parameter filters is reviewed against, one row per passage.
///
/// The transcription manifest for this unit. Every rule in this file comes from one of these
/// passages, and a reviewer checks the file by reading them in this order rather than by
/// reconstructing which specification a branch came from. A rule with no row here is a rule
/// somebody invented, which is the failure this crate is most exposed to: an evaluator that
/// disagrees with a conformant server returns a different set of resources and says nothing.
pub const PROPERTY_FILTER_SECTIONS: &[&str] = &[
    "RFC 4791 section 9.7.2, CALDAV:prop-filter",
    "RFC 4791 section 9.7.3, CALDAV:param-filter",
    "RFC 4791 section 9.7.4, CALDAV:is-not-defined",
    "RFC 4791 section 9.7.5, CALDAV:text-match and negate-condition",
    "RFC 4791 section 9.9, the seven properties a time-range is defined for",
    "RFC 4791 section 9.9, the semantic is not defined for any other property",
    "RFC 4791 section 9.9, an absent bound is -infinity or +infinity",
    "RFC 5545 section 3.1, property and parameter names are case-insensitive",
    "RFC 5545 section 3.2, a parameter value may be quoted or not",
];

/// The seven properties RFC 4791 section 9.9 gives a `time-range` a meaning on.
///
/// Closed, and closed on purpose: the paragraph after the table says "The semantic of
/// `CALDAV:time-range` is not defined for any other calendar components and properties", so a
/// name that is not here is a question the specification declines rather than one this crate is
/// free to answer. Spelled as octets and compared case-insensitively, because that is how RFC
/// 5545 section 3.1 compares a property name and a client is entitled to write `dtstart`.
const TIME_RANGE_PROPERTIES: &[&[u8]] = &[
    b"COMPLETED",
    b"CREATED",
    b"DTEND",
    b"DTSTAMP",
    b"DTSTART",
    b"DUE",
    b"LAST-MODIFIED",
];

/// Whether `component` has a property satisfying `filter`, RFC 4791 section 9.7.2.
///
/// # Errors
///
/// [`QueryError::Contradictory`] for a filter that states `is-not-defined` beside another test,
/// [`QueryError::UnsupportedCollation`] for a `text-match` naming a collation this crate does not
/// implement, and [`QueryError::Limit`] when the caller's ledger refuses the octets.
pub(crate) fn matches_prop_filter(
    component: &Component,
    filter: &PropFilter,
    zones: Zones<'_>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    if filter.is_contradictory() {
        return Err(QueryError::Contradictory);
    }
    let wanted = filter.name();
    if filter.is_not_defined {
        // Section 9.7.2's second bullet. The match is about the *absence* of the name, so the
        // occurrences are counted and none of them is examined — which is also why a filter
        // carrying another test beside this one was refused above rather than decided.
        let present = component.properties().any(|held| held.is_named(wanted));
        return Ok(Match::of(!present));
    }
    let mut answer = Match::Unmatched;
    for occurrence in component.properties().filter(|held| held.is_named(wanted)) {
        answer = answer.or(occurrence_matches(occurrence, filter, zones, budget)?);
        if answer.is_matched() {
            // Section 9.9: "if any one instance matches, then the test returns true". A matched
            // operand decides `Match::or` whatever follows it, so the remaining occurrences
            // cannot change this answer and reading them would be work with no question behind
            // it.
            break;
        }
    }
    Ok(answer)
}

/// Whether this one occurrence satisfies every test in `filter`, RFC 4791 section 9.7.2.
///
/// # Errors
///
/// As [`matches_prop_filter`].
fn occurrence_matches<'a>(
    property: &'a Property,
    filter: &PropFilter,
    zones: Zones<'a>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    // Cheapest first: the parameter filters read octets already in the tree, the substring test
    // scans a value nobody had to decode, and only the time range decodes one and asks a zone
    // source. Section 9.7.2 conjoins the three and an unmatched operand decides a conjunction
    // from either side, so stopping at the first refusal answers the same question for less.
    let mut answer = Match::Matched;
    for param in filter.params() {
        answer = answer.and(matches_param_filter(property, param, budget)?);
        if answer == Match::Unmatched {
            return Ok(answer);
        }
    }
    if let Some(text) = filter.text_match.as_ref() {
        answer = answer.and(text_matches(
            property.value_text().as_bytes(),
            text,
            budget,
        )?);
        if answer == Match::Unmatched {
            return Ok(answer);
        }
    }
    if let Some(window) = filter.time_range {
        answer = answer.and(value_overlaps(property, window, zones, budget)?);
    }
    Ok(answer)
}

/// Whether `property` has a parameter satisfying `filter`, RFC 4791 section 9.7.3.
///
/// The property is the occurrence the enclosing `prop-filter` is testing and not merely one of
/// the name: section 9.7.3 scopes a `param-filter` to "the calendar property being examined".
///
/// # Errors
///
/// As [`matches_prop_filter`].
pub(crate) fn matches_param_filter(
    property: &Property,
    filter: &ParamFilter,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    if filter.is_contradictory() {
        return Err(QueryError::Contradictory);
    }
    let wanted = filter.name();
    if filter.is_not_defined {
        // Section 9.7.3's second bullet.
        return Ok(Match::of(
            property.parameters_named(wanted).next().is_none(),
        ));
    }
    let Some(text) = filter.text_match.as_ref() else {
        // Section 9.7.3's first bullet: an empty `param-filter` asks only that the parameter is
        // there.
        return Ok(Match::of(
            property.parameters_named(wanted).next().is_some(),
        ));
    };
    let mut answer = Match::Unmatched;
    for parameter in property.parameters_named(wanted) {
        // One name may be written more than once on a line, and one occurrence satisfying the
        // test is enough for the same reason it is enough one level up.
        answer = answer.or(text_matches(parameter.unquoted(), text, budget)?);
        if answer.is_matched() {
            break;
        }
    }
    Ok(answer)
}

/// Whether `value` satisfies `text`, RFC 4791 section 9.7.5.
///
/// # Errors
///
/// [`QueryError::UnsupportedCollation`] for a collation with no row in [`Collator`], which is the
/// answer section 7.5.1 gives it, and [`QueryError::Limit`] when the ledger refuses the octets.
fn text_matches(
    value: &[u8],
    text: &TextMatch,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    // Refused rather than downgraded. A substring test run under a collation the client did not
    // ask for returns a different set of resources and says nothing about it (section 7.5.1).
    // Charged before the search, because the octets are the work: a `calendar-query` runs one of
    // these against every property of every component of every resource in a collection, and the
    // caller's ledger is what bounds that (`docs/adr/0010`).
    budget
        .meter
        .try_charge_bytes(u64::try_from(value.len()).unwrap_or(u64::MAX))?;
    let found = Match::of(collate::contains_text(value, text)?);
    // Section 9.7.5: `negate-condition="yes"` returns a match when the text does *not* match. It
    // negates this one comparison and not the filter around it, which is what makes the RFC's own
    // example — components with a `STATUS` not set to `CANCELLED` — mean what it says.
    Ok(if text.negate { found.negate() } else { found })
}

/// Whether this property's value falls inside `window`, RFC 4791 section 9.9.
///
/// # Errors
///
/// [`QueryError::Limit`] when the caller's ledger refuses the octets of the value.
fn value_overlaps<'a>(
    property: &'a Property,
    window: TimeRange,
    zones: Zones<'a>,
    budget: &mut Budget<'_>,
) -> Result<Match, QueryError> {
    if !TIME_RANGE_PROPERTIES
        .iter()
        .copied()
        .any(|known| property.is_named(known))
    {
        return Ok(Match::Undecided(Undecided::OverlapUndefined));
    }
    let written = property.value_text().as_bytes();
    budget
        .meter
        .try_charge_bytes(u64::try_from(written.len()).unwrap_or(u64::MAX))?;
    let Ok(value) = DateTimeValue::decode_property(property) else {
        // Present and not readable. Section 9.9's condition has two sides and this one has no
        // instant to put on it, so "does not match" would report an absence never established.
        return Ok(Match::Undecided(Undecided::ValueUnreadable));
    };
    Ok(match placed(value, zones) {
        Ok(instant) => Match::of(covers(window, instant)),
        Err(reason) => Match::Undecided(reason),
    })
}

/// The instant a property value names, or why it names none.
///
/// # Errors
///
/// A zone reason, or [`Undecided::ValueUnreadable`] for a value that read and names no instant
/// on the representable timeline. None of them is a fault in the resource or in the query.
fn placed<'a>(value: DateTimeValue<'a>, zones: Zones<'a>) -> Result<Instant, Undecided> {
    match value {
        // Already an instant. Handing it to the caller's source instead would make the answer
        // depend on whether that source happens to carry a zone named `UTC`, which is a fallback
        // of exactly the shape `docs/adr/0003` refuses.
        DateTimeValue::Utc(stamp) => stamp
            .at_offset(UtcOffset::UTC)
            .ok_or(Undecided::ValueUnreadable),
        // A `DATE` carries no zone of its own — RFC 5545 section 3.3.4 gives it no place for
        // one — so it is read in the zone a value that names none is read in, and section 9.9's
        // point condition is applied at the first instant of the day.
        DateTimeValue::Date(date) => {
            let midnight = CivilDateTime::new(date, CivilTime::MIDNIGHT);
            resolved(midnight, None, zones)
        },
        DateTimeValue::Local(stamp) => resolved(stamp, None, zones),
        DateTimeValue::Zoned { stamp, tzid } => match from_utf8(tzid) {
            Ok(named) => resolved(stamp, Some(named), zones),
            // A `TZID` that is not UTF-8 is one no `ZoneSource` can even be asked about, the
            // trait taking a `&str`. That is precisely "a TZID no supplied source recognizes",
            // and it is not a resource outside the window.
            Err(_) => Err(Undecided::ZoneUnknown),
        },
    }
}

/// The instant a wall clock names under the zone that applies to it.
///
/// # Errors
///
/// [`Undecided::ZoneUnstated`] for a floating value the query stated no zone for,
/// [`Undecided::ZoneUnknown`] for a zone no supplied source recognizes, and
/// [`Undecided::ZoneAmbiguous`] where the caller's policy names no instant.
fn resolved<'a>(
    local: CivilDateTime,
    tzid: Option<&'a str>,
    zones: Zones<'a>,
) -> Result<Instant, Undecided> {
    let answer = zones.resolve(tzid, local)?;
    let policy = zones.policy();
    // `pick` declines two states: a wall clock the zone sprang over when the policy says to skip
    // it, and a zone the source recognizes and holds no transitions for. Both leave the
    // comparison with no instant to be made on, which is what `ZoneAmbiguous` names — "a wall
    // clock the zone repeats or does not show". A fold always names one under either policy.
    answer
        .resolution
        .pick(policy.gaps(), policy.folds())
        .ok_or(Undecided::ZoneAmbiguous)
}

/// Section 9.9's property condition: `(start <= date-time) AND (end > date-time)`.
///
/// An absent bound is the infinity section 9.9 names for it — "assume '-infinity' and
/// '+infinity' as their value" — so it is a `true` rather than a comparison against a sentinel
/// instant this crate would have had to invent. The start is inclusive and the end is not,
/// which is the half-open window every other bound in this workspace is written as and which
/// section 9.9 states outright for the attributes themselves.
fn covers(window: TimeRange, at: Instant) -> bool {
    let from_start = window
        .start()
        .is_none_or(|start| start.unix_seconds() <= at.unix_seconds());
    let before_end = window
        .end()
        .is_none_or(|end| end.unix_seconds() > at.unix_seconds());
    from_start && before_end
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::internal::tz::FixedOffsetSource;
    use ical_core::{Component, Instant, Item, Limits, Meter, Parameter, Property, UtcOffset};
    use ical_dav::{Collation, ParamFilter, PropFilter, TextMatch, TimeRange};

    use super::{PROPERTY_FILTER_SECTIONS, matches_param_filter, matches_prop_filter};
    use crate::internal::query::vocabulary::{Budget, Match, QueryError, Undecided, Zones};

    type ParameterMatchCase<'a> = (&'a [u8], &'a [u8], &'a [u8], &'a [u8], Match);
    type ZonedValueCase<'a> = (Option<&'a [u8]>, &'a [u8], bool, Match);
    type DateWindowCase<'a> = (&'a [u8], Option<i64>, Option<i64>, Match);

    // Every bound below is written out rather than computed, so that a reader checking a row
    // against RFC 4791 section 9.9 reads one number and not an expression, and so that no test
    // in this file does arithmetic whose overflow behavior would have to be stated.

    /// `2026-03-15T00:00:00Z`, the midnight the `DATE` `20260315` names in UTC.
    const MIDNIGHT: i64 = 1_773_532_800;
    /// One second before that midnight.
    const BEFORE_MIDNIGHT: i64 = 1_773_532_799;
    /// `2026-03-15T11:00:00Z`.
    const ELEVEN: i64 = 1_773_572_400;
    /// `2026-03-15T12:00:00Z`, the instant every time-range row is written around.
    const NOON: i64 = 1_773_576_000;
    /// One second past noon.
    const PAST_NOON: i64 = 1_773_576_001;
    /// `2026-03-15T13:00:00Z`.
    const THIRTEEN: i64 = 1_773_579_600;
    /// `2026-03-16T00:00:00Z`, the midnight the `DATE` `20260316` names in UTC.
    const NEXT_MIDNIGHT: i64 = 1_773_619_200;

    fn property(name: &[u8], parameters: Vec<Parameter>, value: &[u8]) -> Property {
        Property::create(name, parameters, value).unwrap()
    }

    fn component(properties: Vec<Property>) -> Component {
        Component::create(
            b"VEVENT",
            properties.into_iter().map(Item::Property).collect(),
        )
        .unwrap()
    }

    fn prop_filter(name: &[u8]) -> PropFilter {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        PropFilter::new(name, limits, &mut meter).unwrap()
    }

    fn param_filter(name: &[u8]) -> ParamFilter {
        let mut meter = Meter::new(Limits::DEFAULT);
        ParamFilter::new(name, &mut meter).unwrap()
    }

    fn text_match(value: &[u8], negate: bool) -> TextMatch {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut built = TextMatch::new(value, &mut meter).unwrap();
        built.negate = negate;
        built
    }

    fn window(start: Option<i64>, end: Option<i64>) -> TimeRange {
        TimeRange::new(
            start.map(Instant::from_unix_seconds),
            end.map(Instant::from_unix_seconds),
        )
        .unwrap()
    }

    /// A zone source that recognizes one identifier, five hours behind UTC.
    fn eastern() -> FixedOffsetSource {
        FixedOffsetSource::new(
            "America/New_York",
            UtcOffset::from_seconds(-18_000).unwrap(),
            false,
        )
    }

    /// A zone source that recognizes `UTC` and nothing else.
    fn utc() -> FixedOffsetSource {
        FixedOffsetSource::new("UTC", UtcOffset::UTC, false)
    }

    fn decide(held: &Component, filter: &PropFilter, zones: Zones<'_>) -> Match {
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        matches_prop_filter(held, filter, zones, &mut budget).unwrap()
    }

    #[test]
    fn presence_and_absence_are_section_9_7_2s_first_two_bullets() {
        let source = eastern();
        let held = component(vec![property(b"SUMMARY", vec![], b"Birthday party")]);
        // (name the filter carries, is-not-defined, expected). The expectation is the RFC's: an
        // empty `prop-filter` matches when a property of the name exists in the enclosing
        // component, and one carrying `is-not-defined` matches when no such property exists.
        let rows: &[(&[u8], bool, Match)] = &[
            (b"SUMMARY", false, Match::Matched),
            (b"DESCRIPTION", false, Match::Unmatched),
            (b"SUMMARY", true, Match::Unmatched),
            (b"DESCRIPTION", true, Match::Matched),
            // RFC 5545 section 3.1: a property name is compared case-insensitively.
            (b"summary", false, Match::Matched),
        ];
        for &(name, absent, expected) in rows {
            let mut filter = prop_filter(name);
            filter.is_not_defined = absent;
            assert_eq!(
                decide(&held, &filter, Zones::new(&source)),
                expected,
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_text_match_is_section_9_7_5s_substring_test() {
        let source = eastern();
        // (value written on the line, text to look for, negate-condition, expected). The last
        // two rows are section 9.7.5's own example: negate-condition "can be used to match
        // components with a STATUS property not set to CANCELLED".
        let rows: &[(&[u8], &[u8], bool, Match)] = &[
            (b"Birthday party", b"party", false, Match::Matched),
            (b"Birthday party", b"funeral", false, Match::Unmatched),
            // RFC 4790 section 9.2: `i;ascii-casemap` folds A-Z against a-z.
            (b"Birthday party", b"PARTY", false, Match::Matched),
            (b"CANCELLED", b"CANCELLED", true, Match::Unmatched),
            (b"CONFIRMED", b"CANCELLED", true, Match::Matched),
        ];
        for &(value, needle, negate, expected) in rows {
            let held = component(vec![property(b"STATUS", vec![], value)]);
            let mut filter = prop_filter(b"STATUS");
            filter.text_match = Some(text_match(needle, negate));
            assert_eq!(
                decide(&held, &filter, Zones::new(&source)),
                expected,
                "{value:?}"
            );
        }
    }

    /// RFC 4791 section 7.5 maps RFC 4790's reserved `default` identifier to
    /// `i;ascii-casemap` before a property value is compared.
    #[test]
    fn the_reserved_default_collation_reaches_property_comparison() {
        let source = eastern();
        let held = component(vec![property(b"SUMMARY", vec![], b"Birthday party")]);
        let mut filter = prop_filter(b"SUMMARY");
        let mut matcher = text_match(b"PARTY", false);
        matcher.collation = Collation::parse(b"default").unwrap();
        filter.text_match = Some(matcher);

        assert_eq!(decide(&held, &filter, Zones::new(&source)), Match::Matched);
    }

    #[test]
    fn one_occurrence_satisfying_every_test_is_what_matches() {
        let source = eastern();
        let accepted = Parameter::create(b"PARTSTAT", b"ACCEPTED").unwrap();
        let held = component(vec![
            property(b"ATTENDEE", vec![accepted], b"mailto:ann@example.test"),
            property(b"ATTENDEE", vec![], b"mailto:bob@example.test"),
        ]);
        // Section 9.7.2's bullets conjoin the tests against *the targeted property*, so a
        // `text-match` satisfied by one line and a `param-filter` satisfied by the other is not
        // a match: the third and fourth rows are the ones that catch an evaluator which took the
        // existential over each test separately.
        let rows: &[(&[u8], bool, Match)] = &[
            (b"mailto:ann", false, Match::Matched),
            (b"mailto:bob", true, Match::Matched),
            (b"mailto:ann", true, Match::Unmatched),
            (b"mailto:bob", false, Match::Unmatched),
        ];
        for &(needle, undefined, expected) in rows {
            let mut inner = param_filter(b"PARTSTAT");
            inner.is_not_defined = undefined;
            if !undefined {
                inner.text_match = Some(text_match(b"ACCEPTED", false));
            }
            let mut filter = prop_filter(b"ATTENDEE");
            filter.text_match = Some(text_match(needle, false));
            let mut meter = Meter::new(Limits::DEFAULT);
            filter.push_param(inner, &mut meter).unwrap();
            assert_eq!(
                decide(&held, &filter, Zones::new(&source)),
                expected,
                "{needle:?}"
            );
        }
    }

    #[test]
    fn a_parameter_filter_reads_the_value_without_its_quotes() {
        // RFC 5545 section 3.2 puts a value carrying a comma inside a `DQUOTE` pair, so
        // `CN=Doe, John` is written `CN="Doe, John"` and the quotes are syntax rather than
        // value. The last row is what catches a test run against the written octets: a needle
        // carrying the opening quote must not match, because no value has one in it.
        // (parameter name on the line, its value, name the filter carries, needle, expected)
        let rows: &[ParameterMatchCase<'_>] = &[
            (
                b"PARTSTAT",
                b"ACCEPTED",
                b"PARTSTAT",
                b"ACCEPTED",
                Match::Matched,
            ),
            (
                b"PARTSTAT",
                b"DECLINED",
                b"PARTSTAT",
                b"ACCEPTED",
                Match::Unmatched,
            ),
            // RFC 5545 section 3.1: a parameter name is compared case-insensitively too.
            (
                b"PARTSTAT",
                b"ACCEPTED",
                b"partstat",
                b"ACCEPTED",
                Match::Matched,
            ),
            (b"CN", b"Doe, John", b"CN", b"Doe, John", Match::Matched),
            (b"CN", b"Doe, John", b"CN", b"\"Doe", Match::Unmatched),
        ];
        for &(held_name, written, wanted, needle, expected) in rows {
            let line = property(
                b"ATTENDEE",
                vec![Parameter::create(held_name, written).unwrap()],
                b"mailto:ann@example.test",
            );
            let mut filter = param_filter(wanted);
            filter.text_match = Some(text_match(needle, false));
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
            let answer = matches_param_filter(&line, &filter, &mut budget).unwrap();
            assert_eq!(answer, expected, "{written:?} {needle:?}");
        }
    }

    #[test]
    fn a_parameter_filter_with_no_text_match_asks_only_that_it_is_there() {
        // Section 9.7.3's two bullets, which are the whole of what it states.
        let bare = property(b"ATTENDEE", vec![], b"mailto:bob@example.test");
        let dressed = property(
            b"ATTENDEE",
            vec![Parameter::create(b"PARTSTAT", b"DECLINED").unwrap()],
            b"mailto:ann@example.test",
        );
        let rows: &[(bool, bool, Match)] = &[
            (true, false, Match::Matched),
            (false, false, Match::Unmatched),
            (true, true, Match::Unmatched),
            (false, true, Match::Matched),
        ];
        for &(dressed_line, undefined, expected) in rows {
            let line = if dressed_line { &dressed } else { &bare };
            let mut filter = param_filter(b"PARTSTAT");
            filter.is_not_defined = undefined;
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
            let answer = matches_param_filter(line, &filter, &mut budget).unwrap();
            assert_eq!(answer, expected, "{dressed_line} {undefined}");
        }
    }

    #[test]
    fn the_property_time_range_is_section_9_9s_half_open_point_test() {
        let source = eastern();
        let held = component(vec![property(b"DTSTAMP", vec![], b"20260315T120000Z")]);
        // (start, end, expected) against a value at noon, reading section 9.9's condition
        // `(start <= date-time) AND (end > date-time)` and its "assume '-infinity' and
        // '+infinity'" for an absent bound. Rows two and six are the boundary: the start is
        // inclusive and the end is not, and an evaluator that flipped either would move every
        // value sitting exactly on an hour into the neighboring window.
        let rows: &[(Option<i64>, Option<i64>, Match)] = &[
            (Some(NOON), Some(THIRTEEN), Match::Matched),
            (Some(ELEVEN), Some(NOON), Match::Unmatched),
            (Some(PAST_NOON), None, Match::Unmatched),
            (Some(NOON), None, Match::Matched),
            (None, Some(PAST_NOON), Match::Matched),
            (None, Some(NOON), Match::Unmatched),
        ];
        for &(start, end, expected) in rows {
            let mut filter = prop_filter(b"DTSTAMP");
            filter.time_range = Some(window(start, end));
            let answer = decide(&held, &filter, Zones::new(&source));
            assert_eq!(answer, expected, "{start:?} {end:?}");
        }
    }

    #[test]
    fn a_time_range_on_a_property_section_9_9_omits_is_undecided() {
        let source = eastern();
        // Section 9.9 names seven properties and then says the semantic "is not defined for any
        // other calendar components and properties". `FREEBUSY` is the row a reader assumes is
        // an oversight and is not: its periods are compared against a range, but by the
        // *VFREEBUSY component* rule, which is a different filter and a different unit.
        let undefined = Match::Undecided(Undecided::OverlapUndefined);
        let stamp: &[u8] = b"20260315T120000Z";
        let rows: &[(&[u8], &[u8], Match)] = &[
            (b"COMPLETED", stamp, Match::Matched),
            (b"CREATED", stamp, Match::Matched),
            (b"DTEND", stamp, Match::Matched),
            (b"DTSTAMP", stamp, Match::Matched),
            (b"DTSTART", stamp, Match::Matched),
            (b"DUE", stamp, Match::Matched),
            (b"LAST-MODIFIED", stamp, Match::Matched),
            (b"FREEBUSY", b"20260315T120000Z/20260315T130000Z", undefined),
            (b"TRIGGER", stamp, undefined),
            (b"RECURRENCE-ID", stamp, undefined),
            (b"SUMMARY", b"Birthday party", undefined),
            // Present and not readable as a date-time, which is still not "does not match".
            (
                b"DTSTAMP",
                b"whenever",
                Match::Undecided(Undecided::ValueUnreadable),
            ),
        ];
        for &(name, value, expected) in rows {
            let held = component(vec![property(name, vec![], value)]);
            let mut filter = prop_filter(name);
            filter.time_range = Some(window(Some(ELEVEN), Some(THIRTEEN)));
            assert_eq!(
                decide(&held, &filter, Zones::new(&source)),
                expected,
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_value_with_no_timeline_to_sit_on_is_undecided_and_not_unmatched() {
        let source = eastern();
        // (TZID parameter, value, whether the query stated a zone, expected). A floating value
        // and no `CALDAV:timezone` is the case section 9.9 leaves the query to supply and
        // `docs/adr/0003` forbids inventing an answer for; "does not match" would report a
        // resource as outside a window nothing ever compared it against.
        let unstated = Match::Undecided(Undecided::ZoneUnstated);
        let unknown = Match::Undecided(Undecided::ZoneUnknown);
        let rows: &[ZonedValueCase<'_>] = &[
            (None, b"20260315T070000", false, unstated),
            (None, b"20260315T070000", true, Match::Matched),
            (None, b"20260315T090000", true, Match::Unmatched),
            (Some(b"Mars/Olympus"), b"20260315T070000", true, unknown),
            (
                Some(b"America/New_York"),
                b"20260315T070000",
                false,
                Match::Matched,
            ),
        ];
        for &(tzid, value, stated, expected) in rows {
            let parameters = tzid.map_or_else(Vec::new, |named| {
                vec![Parameter::create(b"TZID", named).unwrap()]
            });
            let held = component(vec![property(b"DTSTART", parameters, value)]);
            let mut filter = prop_filter(b"DTSTART");
            filter.time_range = Some(window(Some(ELEVEN), Some(THIRTEEN)));
            let seam = Zones::new(&source);
            let seam = if stated {
                seam.with_query_zone("America/New_York")
            } else {
                seam
            };
            assert_eq!(decide(&held, &filter, seam), expected, "{value:?} {stated}");
        }
    }

    #[test]
    fn one_matching_occurrence_beats_an_undecidable_one() {
        // Section 9.9: "if any one instance matches, then the test returns true". The floating
        // line cannot be placed and the zoned one is inside the window, and Kleene's disjunction
        // makes the pair a match rather than an undecided answer.
        let source = eastern();
        let zoned = property(
            b"DTSTART",
            vec![Parameter::create(b"TZID", b"America/New_York").unwrap()],
            b"20260315T070000",
        );
        let floating = property(b"DTSTART", vec![], b"20260315T070000");
        let held = component(vec![floating.clone(), zoned]);
        let mut filter = prop_filter(b"DTSTART");
        filter.time_range = Some(window(Some(ELEVEN), Some(THIRTEEN)));
        assert_eq!(decide(&held, &filter, Zones::new(&source)), Match::Matched);
        // With only the floating line there is nothing to rescue the answer.
        let alone = component(vec![floating]);
        assert_eq!(
            decide(&alone, &filter, Zones::new(&source)),
            Match::Undecided(Undecided::ZoneUnstated)
        );
    }

    #[test]
    fn a_date_value_is_placed_at_the_first_instant_of_its_day() {
        // Section 9.9's property condition is a point test, and the `+P1D` readings printed
        // beside it are stated for *components*. A `DATE` `DTSTART` therefore sits at the
        // midnight beginning the day it names, read in the zone the query stated, and a window
        // ending at that midnight excludes it.
        let source = utc();
        let rows: &[DateWindowCase<'_>] = &[
            (b"20260315", Some(MIDNIGHT), None, Match::Matched),
            (b"20260315", None, Some(MIDNIGHT), Match::Unmatched),
            (
                b"20260315",
                Some(BEFORE_MIDNIGHT),
                Some(MIDNIGHT),
                Match::Unmatched,
            ),
            (
                b"20260316",
                Some(MIDNIGHT),
                Some(NEXT_MIDNIGHT),
                Match::Unmatched,
            ),
            (b"20260316", Some(NEXT_MIDNIGHT), None, Match::Matched),
        ];
        for &(value, start, end, expected) in rows {
            let held = component(vec![property(b"DTSTART", vec![], value)]);
            let mut filter = prop_filter(b"DTSTART");
            filter.time_range = Some(window(start, end));
            let seam = Zones::new(&source).with_query_zone("UTC");
            assert_eq!(
                decide(&held, &filter, seam),
                expected,
                "{value:?} {start:?}"
            );
        }
    }

    #[test]
    fn a_query_zone_no_source_recognizes_is_undecided_and_never_utc() {
        // `docs/adr/0003` forbids a fallback, so a `CALDAV:timezone` naming a zone the caller's
        // source has never heard of leaves a floating value with no timeline rather than being
        // read as UTC on a guess.
        let source = eastern();
        let held = component(vec![property(b"DTSTART", vec![], b"20260315T070000")]);
        let mut filter = prop_filter(b"DTSTART");
        filter.time_range = Some(window(Some(ELEVEN), None));
        let seam = Zones::new(&source).with_query_zone("Mars/Olympus");
        assert_eq!(
            decide(&held, &filter, seam),
            Match::Undecided(Undecided::ZoneUnknown)
        );
    }

    #[test]
    fn a_filter_that_states_a_condition_and_its_negation_is_refused() {
        // Section 9.7.4's `is-not-defined` is about the absence of the name, so a filter that
        // also tests the value of that name states a condition and its own negation. `ical-dav`
        // can represent one and reports it; this crate declines to decide it.
        let source = eastern();
        let held = component(vec![property(b"SUMMARY", vec![], b"Birthday party")]);
        let mut filter = prop_filter(b"SUMMARY");
        filter.is_not_defined = true;
        filter.text_match = Some(text_match(b"party", false));
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        assert_eq!(
            matches_prop_filter(&held, &filter, Zones::new(&source), &mut budget),
            Err(QueryError::Contradictory)
        );
        let line = property(b"ATTENDEE", vec![], b"mailto:ann@example.test");
        let mut inner = param_filter(b"PARTSTAT");
        inner.is_not_defined = true;
        inner.text_match = Some(text_match(b"ACCEPTED", false));
        assert_eq!(
            matches_param_filter(&line, &inner, &mut budget),
            Err(QueryError::Contradictory)
        );
    }

    #[test]
    fn a_collation_this_crate_does_not_implement_is_refused_not_downgraded() {
        // RFC 4791 section 7.5.1 gives a server the `CALDAV:supported-collation` precondition
        // for exactly this, so falling back to `i;ascii-casemap` would answer a query nobody
        // wrote and would say nothing about having done so.
        let source = eastern();
        let held = component(vec![property(b"SUMMARY", vec![], b"Birthday party")]);
        let mut needle = text_match(b"party", false);
        needle.collation = Collation::Other(b"i;unicode-casemap".as_slice().into());
        let mut filter = prop_filter(b"SUMMARY");
        filter.text_match = Some(needle);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
        assert_eq!(
            matches_prop_filter(&held, &filter, Zones::new(&source), &mut budget),
            Err(QueryError::UnsupportedCollation)
        );
    }

    #[test]
    fn the_manifest_names_every_passage_this_unit_transcribes() {
        for passage in ["9.7.2", "9.7.3", "9.7.4", "9.7.5", "9.9"] {
            assert!(
                PROPERTY_FILTER_SECTIONS
                    .iter()
                    .any(|row| row.contains(passage)),
                "{passage}"
            );
        }
    }
}
