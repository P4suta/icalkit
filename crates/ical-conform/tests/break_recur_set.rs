// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The recurrence *set*, attacked where its three mechanisms meet.
//!
//! `ical-recur` does not claim to expand a rule. It claims to expand a rule **and** apply
//! `RDATE`, `EXDATE` and `RECURRENCE-ID` inside the iterator, because — in `docs/adr/0002`'s
//! own words — "a caller that has to reconcile them is a caller that will get it wrong". This
//! file only ever attacks that composition. A rule expanded on its own is
//! `rfc5545_recurrence_examples.rs`'s subject and is not questioned here.
//!
//! # The properties every case below is measured against
//!
//! - **C1 — set composition.** The occurrences a series has are the rule's instances, plus
//!   every `RDATE`, minus every `EXDATE`, with each `RECURRENCE-ID` applied to the instance it
//!   addresses. No mechanism may remove an instance no line of the file names.
//! - **C2 — `EXDATE` wins, scoped to one instant.** `crates/ical-recur/src/merge.rs`
//!   precedence 1 and 2: an instant in both an `EXDATE` list and the override table is dropped
//!   and reported, and the exclusion removes that instance only — never the override object,
//!   whose diff stays in force for every later candidate.
//! - **C3 — `RANGE=THISANDFUTURE` is a property diff, not a time delta.** An anchor that
//!   changes only `LOCATION` relocates every later instance and moves none of them. This is
//!   the bug five of seven bake-off proposals shared, so it is asserted from the outside.
//! - **C4 — a window is not a filter on generated instants.** `docs/adr/0002` amendment 2: a
//!   window admits by cadence key **or** by effective start, so an override that moved an
//!   instance into the window has to appear even though nothing generated a key inside it.
//! - **C5 — a collision has a stated answer.** An `RDATE` naming an instant a rule already
//!   generated yields one occurrence; two overrides naming one instant are refused rather than
//!   silently ranked.
//!
//! # How a fixture becomes an input
//!
//! Every case is a committed `.ics` file, because a corpus entry another implementation can run
//! is worth more than a table of instants only this workspace can read. [`Series::read`] is the
//! caller step `docs/adr/0003` puts outside this crate: it resolves each `DTSTART`, `RDATE`,
//! `EXDATE` and `RECURRENCE-ID` onto one timeline — the wall clock read at UTC, the convention
//! `rfc5545_recurrence_examples.rs` already established — and hands `ical-recur` the sorted
//! slices its surface takes. An override's `DTSTART` becomes `Override::moved_to` and its
//! `LOCATION` and `SUMMARY` become its diff, which is the reading RFC 5545 section 3.8.4.4
//! describes and the shape `PropertyDiff` exists to carry.
//!
//! Every helper below answers `Option` and every unwrap sits inside a `#[test]`, which is the
//! shape `rfc5545_recurrence_examples.rs` uses for the same reason: a fixture that stopped
//! describing what its name says must fail as a named case and never as a panic in a helper
//! shared by fourteen of them.

use ical_core::{
    CivilDate, CivilDateTime, CivilTime, Component, DecodeValue, Diagnostic, DiagnosticCode,
    Document, Instant, Limits, Meter, PropertyId, UtcOffset,
};
use ical_recur::{
    InputError, InputList, Occurrence, Override, OverrideRange, OverrideSet, PropertyChange,
    PropertyDiff, RecurrenceInput, SearchStep, ValueKind, Window, parse_recur,
};

/// The property identities this corpus reads, as statics.
///
/// `Component::properties_named` ties the lifetime of the walk to the borrow of the identity it
/// walks for, so an identity built at the call site would not outlive the values read through
/// it. A `static` is that lifetime, spelled once.
mod ids {
    use ical_core::PropertyId;

    /// `DTSTART`, the instant a series or an override starts at.
    pub(crate) static DTSTART: PropertyId = PropertyId::DTSTART;
    /// `RRULE`, the one rule a component is expanded from.
    pub(crate) static RRULE: PropertyId = PropertyId::RRULE;
    /// `RDATE`, the instants a component adds.
    pub(crate) static RDATE: PropertyId = PropertyId::RDATE;
    /// `EXDATE`, the instants a component removes.
    pub(crate) static EXDATE: PropertyId = PropertyId::EXDATE;
    /// `RECURRENCE-ID`, the instance an override addresses.
    pub(crate) static RECURRENCE_ID: PropertyId = PropertyId::RECURRENCE_ID;
    /// `LOCATION`, the property the `THISANDFUTURE` cases are written around.
    pub(crate) static LOCATION: PropertyId = PropertyId::LOCATION;
    /// `SUMMARY`, the other property an override in this corpus states.
    pub(crate) static SUMMARY: PropertyId = PropertyId::SUMMARY;
}

/// One fixture, embedded rather than read, so a case cannot pass by not being found.
#[derive(Clone, Copy, Debug)]
struct Fixture {
    /// The file name, for the assertion message.
    name: &'static str,
    /// The octets exactly as committed.
    octets: &'static [u8],
}

/// Every case in this file, in the order the file argues them.
const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "exdate_removes_an_rdate_before_the_next_rule_key.ics",
        octets: include_bytes!(
            "fixtures/break_recur_set/exdate_removes_an_rdate_before_the_next_rule_key.ics"
        ),
    },
    Fixture {
        name: "exdate_removes_the_head_of_an_rdate_tail.ics",
        octets: include_bytes!(
            "fixtures/break_recur_set/exdate_removes_the_head_of_an_rdate_tail.ics"
        ),
    },
    Fixture {
        name: "exdate_removes_an_rdate_after_the_rule_ended.ics",
        octets: include_bytes!(
            "fixtures/break_recur_set/exdate_removes_an_rdate_after_the_rule_ended.ics"
        ),
    },
    Fixture {
        name: "exdate_removes_an_rdate_before_dtstart.ics",
        octets: include_bytes!(
            "fixtures/break_recur_set/exdate_removes_an_rdate_before_dtstart.ics"
        ),
    },
    Fixture {
        name: "exdate_and_override_name_one_instant.ics",
        octets: include_bytes!("fixtures/break_recur_set/exdate_and_override_name_one_instant.ics"),
    },
    Fixture {
        name: "exdate_and_this_and_future_anchor_name_one_instant.ics",
        octets: include_bytes!(
            "fixtures/break_recur_set/exdate_and_this_and_future_anchor_name_one_instant.ics"
        ),
    },
    Fixture {
        name: "this_and_future_changes_only_location.ics",
        octets: include_bytes!(
            "fixtures/break_recur_set/this_and_future_changes_only_location.ics"
        ),
    },
    Fixture {
        name: "this_and_future_moves_nine_to_ten.ics",
        octets: include_bytes!("fixtures/break_recur_set/this_and_future_moves_nine_to_ten.ics"),
    },
    Fixture {
        name: "override_moved_into_the_window.ics",
        octets: include_bytes!("fixtures/break_recur_set/override_moved_into_the_window.ics"),
    },
    Fixture {
        name: "rdate_duplicates_a_generated_instant.ics",
        octets: include_bytes!("fixtures/break_recur_set/rdate_duplicates_a_generated_instant.ics"),
    },
    Fixture {
        name: "exdate_value_type_is_date.ics",
        octets: include_bytes!("fixtures/break_recur_set/exdate_value_type_is_date.ics"),
    },
    Fixture {
        name: "two_overrides_name_one_instant.ics",
        octets: include_bytes!("fixtures/break_recur_set/two_overrides_name_one_instant.ics"),
    },
    Fixture {
        name: "override_names_no_generated_instant.ics",
        octets: include_bytes!("fixtures/break_recur_set/override_names_no_generated_instant.ics"),
    },
];

/// One override component, read into the parts `Override::new` takes.
///
/// The changes are owned here rather than borrowed into an `Override`, because `PropertyDiff`
/// borrows a slice and one value cannot hold both a slice and the borrow of it.
#[derive(Debug)]
struct OverrideParts<'a> {
    /// The instant its `RECURRENCE-ID` addresses.
    recurrence_id: Instant,
    /// How far forward its `RANGE` reaches.
    range: OverrideRange,
    /// Its own `DTSTART`, which is where the instance moved to.
    moved_to: Option<Instant>,
    /// What it states about the properties this corpus looks at.
    changes: Vec<PropertyChange<'a>>,
}

/// One fixture's `VEVENT`s, resolved onto the timeline `ical-recur` compares on.
#[derive(Debug)]
struct Series<'a> {
    /// The master component's `DTSTART`.
    dtstart: Instant,
    /// The master component's `RRULE` text, absent for a series that is only `RDATE`s.
    recur: Option<&'a [u8]>,
    /// Every `RDATE` value, ascending.
    rdates: Vec<Instant>,
    /// Every `EXDATE` value, ascending.
    exdates: Vec<Instant>,
    /// Every component carrying a `RECURRENCE-ID`, in the order the file lists them.
    overrides: Vec<OverrideParts<'a>>,
}

impl<'a> Series<'a> {
    /// Read one calendar's single series out of `document`.
    fn read(document: &'a Document) -> Option<Self> {
        let events: Vec<&'a Component> = document
            .components()
            .flat_map(Component::components)
            .filter(|component| component.is_named(b"VEVENT"))
            .collect();
        let master = *events
            .iter()
            .find(|event| !has(event, &ids::RECURRENCE_ID))?;
        let overrides = events
            .iter()
            .filter(|event| has(event, &ids::RECURRENCE_ID))
            .map(|event| override_parts(event))
            .collect::<Option<Vec<OverrideParts<'a>>>>()?;
        Some(Self {
            dtstart: *instants(master, &ids::DTSTART)?.first()?,
            recur: master
                .properties_named(&ids::RRULE)
                .next()
                .map(|rule| rule.value_text().as_bytes()),
            rdates: instants(master, &ids::RDATE)?,
            exdates: instants(master, &ids::EXDATE)?,
            overrides,
        })
    }

    /// The overrides as `ical-recur` takes them, borrowing the changes this value holds.
    fn entries(&'a self) -> Vec<Override<'a>> {
        self.overrides
            .iter()
            .map(|parts| {
                Override::new(
                    parts.recurrence_id,
                    parts.range,
                    parts.moved_to,
                    PropertyDiff::new(&parts.changes),
                )
            })
            .collect()
    }
}

/// Whether `component` carries a property with that identity.
fn has(component: &Component, id: &'static PropertyId) -> bool {
    component.properties_named(id).next().is_some()
}

/// Every instant the properties named `id` carry, in file order.
///
/// `RDATE` and `EXDATE` are comma-separated lists and may repeat, so one property is not one
/// instant. A `VALUE=DATE` entry resolves to midnight, which is what a caller normalizing a
/// date-valued exception through `ical-tz` would hand this crate.
fn instants(component: &Component, id: &'static PropertyId) -> Option<Vec<Instant>> {
    let mut found = Vec::new();
    for property in component.properties_named(id) {
        for value in property.value_text().as_bytes().split(|byte| *byte == b',') {
            found.push(resolve(value)?);
        }
    }
    Some(found)
}

/// One `DATE` or `DATE-TIME` value read onto the UTC timeline.
fn resolve(value: &[u8]) -> Option<Instant> {
    let civil = if let Ok(both) = CivilDateTime::decode_value(value) {
        both
    } else {
        let date = CivilDate::decode_value(value).ok()?;
        CivilDateTime::new(date, CivilTime::from_hms(0, 0, 0)?)
    };
    civil.at_offset(UtcOffset::UTC)
}

/// One override component read into its parts.
fn override_parts(component: &Component) -> Option<OverrideParts<'_>> {
    let addressed = component.properties_named(&ids::RECURRENCE_ID).next()?;
    let onwards = addressed
        .parameters_named(b"RANGE")
        .any(|parameter| parameter.value().as_bytes() == b"THISANDFUTURE");
    let changes = [&ids::LOCATION, &ids::SUMMARY]
        .into_iter()
        .filter_map(|id| component.properties_named(id).next())
        .map(PropertyChange::Set)
        .collect();
    Some(OverrideParts {
        recurrence_id: resolve(addressed.value_text().as_bytes())?,
        range: if onwards {
            OverrideRange::ThisAndFuture
        } else {
            OverrideRange::ThisOnly
        },
        moved_to: instants(component, &ids::DTSTART)?.first().copied(),
        changes,
    })
}

/// The document a fixture holds.
///
/// The fixtures are well-formed iCalendar by construction — what is under attack is the
/// recurrence set they describe, not the octets — so a parse diagnostic here means the fixture
/// is wrong, and the assertion says so before any recurrence claim is measured.
fn parse(name: &str) -> Option<Document> {
    let entry = FIXTURES.iter().find(|candidate| candidate.name == name)?;
    let mut sink: Vec<Diagnostic> = Vec::new();
    let document = Document::parse(entry.octets, Limits::DEFAULT, &mut sink).ok()?;
    assert!(sink.is_empty(), "{name}: the fixture must parse cleanly");
    Some(document)
}

/// What one search produced, flattened past the borrow of the meter.
///
/// The two instant columns are kept as wall-clock text rather than as `Instant`s, because a
/// failure here is read by a person holding the fixture: a missing occurrence has to be legible
/// as the day it was, not as a count of seconds.
#[derive(Debug)]
struct Run {
    /// The effective start of every occurrence emitted, in emission order.
    starts: Vec<String>,
    /// The cadence key of every occurrence emitted, in emission order.
    keys: Vec<String>,
    /// Whether each occurrence carried a `LOCATION` from some override.
    located: Vec<bool>,
    /// Every diagnostic code the search raised.
    codes: Vec<DiagnosticCode>,
}

/// Search `series` over `window`, with the rule the fixture stated.
fn run(series: &Series<'_>, entries: &[Override<'_>], window: Window) -> Option<Run> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut sink: Vec<Diagnostic> = Vec::new();
    let rule = match series.recur {
        Some(text) => Some(parse_recur(text, &mut meter, &mut sink).ok()?),
        None => None,
    };
    let overrides = OverrideSet::new(entries, &mut meter).ok()?;
    let input = RecurrenceInput::new(
        series.dtstart,
        ValueKind::DateTime,
        rule.as_ref(),
        &series.rdates,
        &series.exdates,
        overrides,
        &mut meter,
    )
    .ok()?;
    collect(input, window, &mut meter, &mut sink)
}

/// Drive one search to its end.
fn collect(
    input: RecurrenceInput<'_>,
    window: Window,
    meter: &mut Meter,
    sink: &mut Vec<Diagnostic>,
) -> Option<Run> {
    let mut produced: Vec<Occurrence<'_>> = Vec::new();
    for step in input.search(window, meter, sink) {
        if let SearchStep::Occurrence(occurrence) = step {
            produced.push(occurrence);
        }
    }
    Some(Run {
        starts: produced
            .iter()
            .map(|found| stamp(found.start()))
            .collect::<Option<Vec<String>>>()?,
        keys: produced
            .iter()
            .map(|found| stamp(found.key()))
            .collect::<Option<Vec<String>>>()?,
        located: produced
            .iter()
            .map(|found| found.effective_change(&ids::LOCATION).is_some())
            .collect(),
        codes: sink.iter().map(|found| found.code()).collect(),
    })
}

/// The instant a UTC civil date and time name, built through the checked civil arithmetic.
fn at(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> Option<Instant> {
    let date = CivilDate::from_ymd(year, month, day)?;
    let time = CivilTime::from_hms(hour, minute, 0)?;
    CivilDateTime::new(date, time).at_offset(UtcOffset::UTC)
}

/// One instant as the wall clock the fixture wrote it in.
fn stamp(instant: Instant) -> Option<String> {
    let civil = CivilDateTime::from_instant(instant, UtcOffset::UTC)?;
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}",
        civil.date().year(),
        civil.date().month(),
        civil.date().day(),
        civil.time().hour(),
        civil.time().minute()
    ))
}

/// The wall clock of one March 2026 instant, for an expectation written by hand.
fn text(day: u8, hour: u8, minute: u8) -> Option<String> {
    stamp(at(2026, 3, day, hour, minute)?)
}

/// A window over the first ten days of the month every fixture is written in.
fn march() -> Option<Window> {
    Window::new(at(2026, 3, 1, 0, 0)?, at(2026, 3, 11, 0, 0)?)
}

/// The nine o'clock instances of March 2026 on `days`.
fn nine_am(days: &[u8]) -> Option<Vec<String>> {
    days.iter().map(|day| text(*day, 9, 0)).collect()
}

/// C1. An `EXDATE` that removes an `RDATE` must not also remove a rule instance.
///
/// `FREQ=DAILY;COUNT=5` from March 2 at 09:00, one `RDATE` at March 3 07:00, and an `EXDATE`
/// naming that same 07:00 instant — the shape an exporter produces when a user deletes an extra
/// meeting it had added to a series. The addition sorts *before* the rule's next cadence key,
/// so the merge answers the offered key with the addition and then drops it; the offer is
/// retired anyway and the key it carried is never generated again. Five daily instances are
/// described and March 3 at 09:00 is named by no exclusion in the file.
#[test]
fn an_exdate_on_an_rdate_does_not_remove_the_rule_instance_after_it() {
    let document = parse("exdate_removes_an_rdate_before_the_next_rule_key.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 3, 4, 5, 6]).expect("the expectation"),
        "the exclusion names 07:00 and the rule names 09:00, so every 09:00 survives"
    );
}

/// C1. An `EXDATE` naming the head of the `RDATE` tail must not end the series.
///
/// A series with no `RRULE` at all: three `RDATE`s and one `EXDATE` removing the first of them.
/// `crates/ical-recur/src/merge.rs` states the hazard in its own words — "`Merge::step` answers
/// `None` for a candidate that was excluded as well as for no candidate at all, which is why
/// `Merge::is_drained` is asked first rather than inferred from the answer" — and this case is
/// what happens when a driver infers it.
#[test]
fn an_exdate_on_the_first_rdate_leaves_the_rest_of_the_list_intact() {
    let document = parse("exdate_removes_the_head_of_an_rdate_tail.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[4, 5]).expect("the expectation"),
        "one exclusion removes one addition and says nothing about the other two"
    );
}

/// C1. The same, with the additions sitting past a `COUNT` the rule already spent.
#[test]
fn an_exdate_on_an_addition_after_the_rule_ended_leaves_the_addition_after_it() {
    let document = parse("exdate_removes_an_rdate_after_the_rule_ended.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 3, 7]).expect("the expectation"),
        "the rule's two instances, then the addition the exclusion did not name"
    );
}

/// C1. An excluded `RDATE` before `DTSTART` must not take `DTSTART` with it.
///
/// The sharpest form of the same defect: the instance lost is the first one of the series, so a
/// renderer shows a meeting that starts a day late.
#[test]
fn an_exdate_on_an_addition_before_dtstart_leaves_dtstart_alone() {
    let document = parse("exdate_removes_an_rdate_before_dtstart.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 3, 4]).expect("the expectation"),
        "the addition the exclusion removed sat before DTSTART, and DTSTART is still an instance"
    );
}

/// C2. An instant in both an `EXDATE` and an exact-match override is dropped, and reported.
#[test]
fn an_exdate_beats_an_override_naming_the_same_instant() {
    let document = parse("exdate_and_override_name_one_instant.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 4, 5]).expect("the expectation"),
        "EXDATE wins over the override that moved March 3 to 14:00"
    );
    assert!(
        produced
            .codes
            .contains(&DiagnosticCode::ExdateShadowsOverride),
        "a deletion beating a modification is a policy choice and is said out loud"
    );
}

/// C2. The exclusion is scoped to the instant and never to the override object.
///
/// The `EXDATE` lands on a `RANGE=THISANDFUTURE` anchor's own `RECURRENCE-ID`. One occurrence
/// goes; the anchor's `LOCATION` stays in force on every later one. The other reading turns one
/// duplicated exporter line into the silent reversion of an unbounded tail.
#[test]
fn an_exdate_on_an_anchor_removes_one_instance_and_not_the_anchor() {
    let document =
        parse("exdate_and_this_and_future_anchor_name_one_instant.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 4, 5]).expect("the expectation")
    );
    assert_eq!(
        produced.located,
        vec![false, true, true],
        "the anchor reaches every candidate after the instance the exclusion removed"
    );
}

/// C3. An anchor that changes only `LOCATION` relocates the tail and moves no start.
///
/// The bug five of seven bake-off proposals shared is a scalar time delta, which cannot express
/// this at all. Both halves are asserted: the new `LOCATION` on every later instance, and every
/// later instance still at its own cadence time.
#[test]
fn a_this_and_future_location_edit_moves_nothing_and_relocates_everything_after_it() {
    let document = parse("this_and_future_changes_only_location.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 3, 4, 5]).expect("the expectation")
    );
    assert_eq!(produced.keys, produced.starts, "no instance moved in time");
    assert_eq!(produced.located, vec![false, true, true, true]);
}

/// C4. A window whose lower edge falls between the old time and the new one.
///
/// The series moves from 09:00 to 10:00 for good. A window starting at 09:30 on March 4 holds
/// no cadence key for March 4 — that key is 09:00 — and does hold that instance's effective
/// start. `docs/adr/0002` amendment 2 requires it to appear, which is the half of "a window is
/// not a simple filter on generated instants" that a key-only filter loses.
#[test]
fn an_instance_a_this_and_future_shift_moved_into_the_window_appears() {
    let document = parse("this_and_future_moves_nine_to_ten.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let edge = Window::new(
        at(2026, 3, 4, 9, 30).expect("the lower edge"),
        at(2026, 3, 6, 9, 30).expect("the upper edge"),
    )
    .expect("window");
    let produced = run(&series, &entries, edge).expect("the search runs");
    assert_eq!(
        produced.starts,
        vec![
            text(4, 10, 0).expect("March 4"),
            text(5, 10, 0).expect("March 5"),
            text(6, 10, 0).expect("March 6"),
        ],
        "each later instance keeps its own date and carries the new time"
    );
}

/// C4. An override that moved an instance into a window nothing generated a key in.
///
/// `RECURRENCE-ID:20260310T090000` moved to March 5. A window over March 4 and 5 contains no
/// cadence key of that instance, and the occurrence still has to be reachable.
#[test]
fn an_override_moved_into_the_window_is_emitted_with_its_own_key() {
    let document = parse("override_moved_into_the_window.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let narrow = Window::new(
        at(2026, 3, 4, 0, 0).expect("the lower edge"),
        at(2026, 3, 6, 0, 0).expect("the upper edge"),
    )
    .expect("window");
    let produced = run(&series, &entries, narrow).expect("the search runs");
    assert!(
        produced.keys.contains(&text(10, 9, 0).expect("March 10")),
        "the instance is still addressed by the RECURRENCE-ID the file wrote"
    );
    assert_eq!(
        produced.starts,
        vec![
            text(4, 9, 0).expect("March 4"),
            text(5, 9, 0).expect("March 5"),
            text(5, 9, 0).expect("the instance moved onto March 5"),
        ],
        "two instances start together, which merge.rs precedence 6 decides to keep as two"
    );
}

/// C5. An `RDATE` naming an instant the rule already generated yields one occurrence.
#[test]
fn an_rdate_duplicating_a_generated_instant_is_not_doubled() {
    let document = parse("rdate_duplicates_a_generated_instant.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 3, 4]).expect("the expectation"),
        "the recurrence set is a set"
    );
}

/// An `EXDATE` written as a `DATE` against a `DATE-TIME` `DTSTART` removes nothing.
///
/// RFC 5545 section 3.8.5.1 requires the two value types to agree, and this file breaks that.
/// The resolution `docs/adr/0003` puts at the caller turns the date into midnight, so it names
/// an instant the series does not have. Excluding the whole day is the other reading the RFC's
/// silence permits, and this crate cannot express it — an `EXDATE` arrives as one instant. What
/// is asserted is that the mismatch is inert rather than approximate, which is the only answer a
/// crate that never resolves a zone is entitled to give.
#[test]
fn a_date_valued_exdate_against_a_date_time_series_removes_nothing() {
    let document = parse("exdate_value_type_is_date.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 3, 4]).expect("the expectation")
    );
}

/// C5. Two overrides naming one instant are refused rather than silently ranked.
#[test]
fn two_overrides_on_one_instant_are_refused_by_name() {
    let document = parse("two_overrides_name_one_instant.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let mut meter = Meter::new(Limits::DEFAULT);
    assert_eq!(
        OverrideSet::new(&entries, &mut meter).err(),
        Some(InputError::Duplicated(InputList::Override)),
        "two edits of one instant have no defined precedence, and guessing one silently is the \
         failure this crate exists to prevent"
    );
}

/// An override whose `RECURRENCE-ID` matches no generated instant is inert and silent.
///
/// Real clients emit these — an instance is edited, then the rule is rewritten under it — and
/// nothing in `docs/design/ical-recur-api.md` says what becomes of one. What happens is that it
/// never reaches a candidate and no diagnostic names it, so a file carrying a meeting the user
/// can see in their client produces a series that does not have it. This asserts the shipped
/// behavior rather than a fix, so that the choice is made deliberately rather than by default.
#[test]
fn an_override_addressing_no_generated_instant_is_dropped_without_a_word() {
    let document = parse("override_names_no_generated_instant.ics").expect("fixture");
    let series = Series::read(&document).expect("one master VEVENT");
    let entries = series.entries();
    let produced = run(&series, &entries, march().expect("window")).expect("the search runs");
    assert_eq!(
        produced.starts,
        nine_am(&[2, 3, 4]).expect("the expectation")
    );
    assert!(
        produced.codes.is_empty(),
        "an override that addressed nothing is currently not reported at all"
    );
}

/// A fixture nothing reads is a case nothing attacks.
#[test]
fn every_fixture_in_this_directory_is_readable_as_a_series() {
    for entry in FIXTURES {
        let document = parse(entry.name).expect(entry.name);
        let series = Series::read(&document).expect(entry.name);
        assert!(
            series.recur.is_some() || !series.rdates.is_empty(),
            "{}: a series with neither a rule nor an addition has nothing to expand",
            entry.name
        );
    }
}
