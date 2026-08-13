// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The component layer, attacked through the shapes RFC 5545 section 3.6 has an opinion about.
//!
//! `break_clients.rs` states the four properties this file reuses — P1 round trip, P2 fixed
//! point, P3 mutation locality, P4 diagnostics preserve — and attacks them with what real
//! clients emit. This file attacks the same four with what section 3.6 forbids, because the
//! component layer is where a reading of the specification was just added and a reading is
//! exactly the kind of thing that starts deciding what to keep.
//!
//! The fixtures beside this file are every cardinality accident a component can have: two
//! `DTSTART`s in one `VEVENT`, a `DTEND` beside a `DURATION` and neither of them at all, a
//! `VALARM` at the top of a calendar and nested inside another `VALARM`, a `VTIMEZONE` with no
//! observance, a `STANDARD` with no `VTIMEZONE` around it, an unknown component three deep
//! carrying unknown properties with unknown parameters, a `VCALENDAR` with neither `VERSION`
//! nor `PRODID`, two `VEVENT`s sharing a `UID` and a `RECURRENCE-ID`, an `END` naming a
//! component other than the `BEGIN` it would close, and a component holding nothing but other
//! components. None of those may cost an octet, whether or not it earns a diagnostic.

use std::collections::BTreeSet;

use ical_core::{
    Boundary, Component, ComponentKind, ContentLineReader, Diagnostic, DiagnosticCode, Document,
    Item, Limits, Meter, ParseError, Property, PropertyId, ProposedChange, RawText, TextValue,
};

/// One fixture: the octets on disk, and a name for the assertion message.
#[derive(Clone, Copy, Debug)]
struct Fixture {
    /// The file name, relative to this file's fixture directory.
    name: &'static str,
    /// The octets exactly as committed. `.gitattributes` marks these `-text`, so a fold, a
    /// bare `LF` and a missing final terminator are the octets a producer would have written.
    octets: &'static [u8],
}

/// Every fixture, embedded rather than read, so a case cannot pass by not being found.
const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "two_dtstart.ics",
        octets: include_bytes!("fixtures/break_components/two_dtstart.ics"),
    },
    Fixture {
        name: "dtend_and_duration.ics",
        octets: include_bytes!("fixtures/break_components/dtend_and_duration.ics"),
    },
    Fixture {
        name: "neither_dtend_nor_duration.ics",
        octets: include_bytes!("fixtures/break_components/neither_dtend_nor_duration.ics"),
    },
    Fixture {
        name: "valarm_misplaced.ics",
        octets: include_bytes!("fixtures/break_components/valarm_misplaced.ics"),
    },
    Fixture {
        name: "valarm_without_action_or_trigger.ics",
        octets: include_bytes!("fixtures/break_components/valarm_without_action_or_trigger.ics"),
    },
    Fixture {
        name: "vtimezone_without_observance.ics",
        octets: include_bytes!("fixtures/break_components/vtimezone_without_observance.ics"),
    },
    Fixture {
        name: "standard_outside_vtimezone.ics",
        octets: include_bytes!("fixtures/break_components/standard_outside_vtimezone.ics"),
    },
    Fixture {
        name: "unknown_three_deep.ics",
        octets: include_bytes!("fixtures/break_components/unknown_three_deep.ics"),
    },
    Fixture {
        name: "calendar_without_version_or_prodid.ics",
        octets: include_bytes!("fixtures/break_components/calendar_without_version_or_prodid.ics"),
    },
    Fixture {
        name: "same_uid_same_recurrence_id.ics",
        octets: include_bytes!("fixtures/break_components/same_uid_same_recurrence_id.ics"),
    },
    Fixture {
        name: "end_names_other_component.ics",
        octets: include_bytes!("fixtures/break_components/end_names_other_component.ics"),
    },
    Fixture {
        name: "components_only_no_properties.ics",
        octets: include_bytes!("fixtures/break_components/components_only_no_properties.ics"),
    },
    Fixture {
        name: "components_only_lf.ics",
        octets: include_bytes!("fixtures/break_components/components_only_lf.ics"),
    },
    Fixture {
        name: "folded_boundaries.ics",
        octets: include_bytes!("fixtures/break_components/folded_boundaries.ics"),
    },
    Fixture {
        name: "boundary_name_edges.ics",
        octets: include_bytes!("fixtures/break_components/boundary_name_edges.ics"),
    },
    Fixture {
        name: "boundary_with_parameters.ics",
        octets: include_bytes!("fixtures/break_components/boundary_with_parameters.ics"),
    },
];

/// What one parse produced: the tree, and everything the sink was told.
#[derive(Debug)]
struct Parsed {
    /// The tree, when one could be built at all.
    document: Result<Document, ParseError>,
    /// The diagnostics, in the order they were reported.
    reported: Vec<Diagnostic>,
    /// Diagnostics the sink refused. A `Vec` never refuses, so this must stay zero.
    dropped: u32,
}

/// Parse `octets` under the default policy, keeping every diagnostic.
fn parse(octets: &[u8]) -> Parsed {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut reader = ContentLineReader::new(octets, limits.grammar());
    let mut reported: Vec<Diagnostic> = Vec::new();
    let document = Document::from_tokens(&mut reader, &mut meter, &mut reported);
    Parsed {
        document,
        dropped: meter.diagnostics_dropped(),
        reported,
    }
}

/// The tree, with the fixture that would not parse named in the failure.
///
/// `assert!` rather than an `unwrap`, and the same in every helper below: a function outside a
/// `#[test]` is production code as far as this workspace's lint profile is concerned, so the
/// failure has to be an assertion and the value after it a default nothing reads.
fn tree(case: Fixture) -> Document {
    let outcome = parse(case.octets).document;
    assert!(outcome.is_ok(), "{}: parse refused the file", case.name);
    outcome.unwrap_or_default()
}

/// The octets one property occupies on its own, folds and terminator included.
fn property_octets(property: &Property) -> Vec<u8> {
    Document::new(vec![Item::Property(property.clone())]).to_bytes()
}

/// The octets one `BEGIN` or `END` line occupies on its own.
fn boundary_octets(boundary: &Boundary) -> Vec<u8> {
    let alone = Component::new(boundary.clone(), Vec::new(), None);
    Document::new(vec![Item::Component(alone)]).to_bytes()
}

/// Every line-bearing node of the tree, in document order, each as its own octets.
fn lines(document: &Document) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    collect_lines(document.items(), &mut out);
    out
}

/// Append every line of `items`, walking nested components in place.
fn collect_lines(items: &[Item], out: &mut Vec<Vec<u8>>) {
    for entry in items {
        match entry {
            Item::Property(property) => out.push(property_octets(property)),
            Item::Component(component) => {
                out.push(boundary_octets(component.begin()));
                collect_lines(component.items(), out);
                if let Some(closing) = component.end() {
                    out.push(boundary_octets(closing));
                }
            },
        }
    }
}

/// The indices at which two line decompositions differ.
fn differing(before: &[Vec<u8>], after: &[Vec<u8>]) -> BTreeSet<usize> {
    let mut found = BTreeSet::new();
    let width = before.len().max(after.len());
    for index in 0..width {
        if before.get(index) != after.get(index) {
            found.insert(index);
        }
    }
    found
}

/// Every component of the tree, outermost first, then in document order.
fn components(document: &Document) -> Vec<&Component> {
    let mut out: Vec<&Component> = Vec::new();
    gather(document.items(), &mut out);
    out
}

/// Append every component of `items`, walking nested components in place.
fn gather<'a>(items: &'a [Item], out: &mut Vec<&'a Component>) {
    for entry in items {
        if let Item::Component(component) = entry {
            out.push(component);
            gather(component.items(), out);
        }
    }
}

/// Every diagnostic code one component's audit reports, in the order it reports them.
fn audit_codes(component: &Component) -> Vec<DiagnosticCode> {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut reported: Vec<Diagnostic> = Vec::new();
    component.audit(&mut meter, &mut reported);
    reported.iter().copied().map(Diagnostic::code).collect()
}

/// The path of entry indices from the document's items down to the first component named
/// `name`, in document order, or `None` when the tree holds no such component.
///
/// A path rather than a borrow, because a search that both returns a `&mut` and recurses past
/// it holds two mutable borrows of one entry. Walking the path afterwards holds one.
fn path_to(items: &[Item], name: &[u8], prefix: &mut Vec<usize>) -> bool {
    for (index, entry) in items.iter().enumerate() {
        let Item::Component(component) = entry else {
            continue;
        };
        prefix.push(index);
        if component.is_named(name) || path_to(component.items(), name, prefix) {
            return true;
        }
        prefix.pop();
    }
    false
}

/// The first component anywhere in `document` whose name matches `name`, mutably.
fn find_named<'a>(document: &'a mut Document, name: &[u8]) -> Option<&'a mut Component> {
    let mut path: Vec<usize> = Vec::new();
    if !path_to(document.items(), name, &mut path) {
        return None;
    }
    let mut steps = path.into_iter();
    let first = steps.next()?;
    let mut here = document.items_mut().get_mut(first)?.as_component_mut()?;
    for step in steps {
        here = here.items_mut().get_mut(step)?.as_component_mut()?;
    }
    Some(here)
}

// ---------------------------------------------------------------------------------------
// P1, P2 and P4 over every fixture
// ---------------------------------------------------------------------------------------

/// P1: parse then serialize is the input, octet for octet.
#[test]
fn p1_every_cardinality_accident_round_trips() {
    for &case in FIXTURES {
        assert_eq!(
            tree(case).to_bytes(),
            case.octets,
            "{}: parse then serialize is not the input",
            case.name
        );
    }
}

/// P2: a second parse and serialize changes nothing further.
#[test]
fn p2_a_second_round_trip_is_a_fixed_point() {
    for &case in FIXTURES {
        let once = tree(case).to_bytes();
        let Ok(second) = parse(&once).document else {
            panic!("{}: the serializer's own output did not parse", case.name);
        };
        assert_eq!(
            second.to_bytes(),
            once,
            "{}: the parser disagrees with itself on the second pass",
            case.name
        );
    }
}

/// P4: an input that earns a diagnostic still satisfies P1, and no diagnostic is lost.
#[test]
fn p4_a_diagnosed_component_still_round_trips() {
    for &case in FIXTURES {
        let outcome = parse(case.octets);
        assert_eq!(
            outcome.dropped, 0,
            "{}: a growable sink refused a diagnostic",
            case.name
        );
        let document = outcome
            .document
            .unwrap_or_else(|error| panic!("{}: parse refused the file: {error}", case.name));
        let codes: Vec<DiagnosticCode> = outcome
            .reported
            .iter()
            .copied()
            .map(Diagnostic::code)
            .collect();
        assert_eq!(
            document.to_bytes(),
            case.octets,
            "{}: a file earning {codes:?} did not survive the round trip",
            case.name
        );
    }
}

/// P4 again, with the audit run over every component the file holds.
///
/// The audit is the new reading, and the claim beside it is that it changes nothing. A
/// reading that reordered, dropped or rewrote anything it disapproved of would show up here
/// and nowhere else, because nothing else calls it.
#[test]
fn p4_the_audit_costs_the_document_no_octet() {
    for &case in FIXTURES {
        let document = tree(case);
        let mut meter = Meter::new(Limits::DEFAULT);
        let mut reported: Vec<Diagnostic> = Vec::new();
        for component in components(&document) {
            component.audit(&mut meter, &mut reported);
        }
        assert_eq!(
            document.to_bytes(),
            case.octets,
            "{}: auditing the tree changed what it writes back",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------------------
// The structures themselves, so that P1 is not passing over a tree that lost them
// ---------------------------------------------------------------------------------------

/// The nesting a file states is the nesting the tree holds.
///
/// P1 over a flat comparison of octets can pass while the tree that produced them is wrong:
/// a `BEGIN` degraded to a property writes back the same line. So the shape is asserted
/// separately, per fixture, against what the file says.
#[test]
fn the_tree_holds_the_nesting_the_file_states() {
    let expected: &[(&str, &[&[u8]])] = &[
        (
            "valarm_misplaced.ics",
            &[
                b"VCALENDAR",
                b"VALARM",
                b"VEVENT",
                b"VALARM",
                b"VALARM",
                b"VTIMEZONE",
                b"VALARM",
            ],
        ),
        (
            "unknown_three_deep.ics",
            &[
                b"VCALENDAR",
                b"X-OUTER-CONTAINER",
                b"X-MIDDLE-CONTAINER",
                b"X-INNER-CONTAINER",
            ],
        ),
        (
            "components_only_no_properties.ics",
            &[b"VCALENDAR", b"VEVENT", b"VALARM"],
        ),
        (
            "components_only_lf.ics",
            &[b"VCALENDAR", b"VEVENT", b"VALARM"],
        ),
        (
            "standard_outside_vtimezone.ics",
            &[b"VCALENDAR", b"STANDARD", b"DAYLIGHT"],
        ),
        ("folded_boundaries.ics", &[b"VCALENDAR", b"VEVENT"]),
    ];
    for (name, wanted) in expected {
        let case = fixture(name);
        let document = tree(case);
        let seen: Vec<Vec<u8>> = components(&document)
            .iter()
            .map(|component| component.name().as_bytes().to_ascii_uppercase())
            .collect();
        let wanted: Vec<Vec<u8>> = wanted.iter().map(|entry| entry.to_vec()).collect();
        assert_eq!(seen, wanted, "{name}: the tree is not the file's shape");
    }
}

/// A `BEGIN` folded through its keyword and its name still opens the component it names.
///
/// The fold is a syntax of the line and not of the name, so a reader that unfolds correctly
/// and a reader that does not both write the file back — only one of them has a `VEVENT`.
#[test]
fn a_folded_begin_still_names_its_component() {
    let case = fixture("folded_boundaries.ics");
    let document = tree(case);
    let found = components(&document);
    let Some(event) = found.iter().find(|component| component.is_named(b"VEVENT")) else {
        panic!("{}: the folded BEGIN did not open a VEVENT", case.name);
    };
    assert_eq!(
        event.kind(),
        Some(ComponentKind::Event),
        "{}: the folded BEGIN opened a component with no kind",
        case.name
    );
    assert!(
        event.end().is_some(),
        "{}: the folded END did not close the component the folded BEGIN opened",
        case.name
    );
    assert!(
        parse(case.octets)
            .reported
            .iter()
            .all(|entry| entry.code() != DiagnosticCode::UnclosedComponent),
        "{}: a folded boundary pair was read as unclosed",
        case.name
    );
}

/// An `END` naming another component degrades to a property and closes nothing.
#[test]
fn an_end_naming_another_component_is_a_property() {
    let case = fixture("end_names_other_component.ics");
    let outcome = parse(case.octets);
    let codes: Vec<DiagnosticCode> = outcome
        .reported
        .iter()
        .copied()
        .map(Diagnostic::code)
        .collect();
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == DiagnosticCode::MismatchedEndName)
            .count(),
        3,
        "{}: expected three mismatched ends, got {codes:?}",
        case.name
    );
    let document = outcome
        .document
        .unwrap_or_else(|error| panic!("{}: parse refused the file: {error}", case.name));
    assert_eq!(
        document.to_bytes(),
        case.octets,
        "{}: a crossed END cost the file an octet",
        case.name
    );
}

// ---------------------------------------------------------------------------------------
// The audit, which is the reading this milestone added
// ---------------------------------------------------------------------------------------

/// What section 3.6 says about each fixture's components, as codes.
#[test]
fn the_audit_reports_what_section_3_6_defines() {
    let expected: &[(&str, &[u8], &[DiagnosticCode])] = &[
        // Two `DTSTART`s in a `VEVENT` is the duplicate the table exists to catch.
        (
            "two_dtstart.ics",
            b"VEVENT",
            &[DiagnosticCode::DuplicateProperty],
        ),
        // A `VCALENDAR` with neither required property earns one report per name.
        (
            "calendar_without_version_or_prodid.ics",
            b"VCALENDAR",
            &[
                DiagnosticCode::MissingRequiredProperty,
                DiagnosticCode::MissingRequiredProperty,
            ],
        ),
        // A `VALARM` with neither `ACTION` nor `TRIGGER`.
        (
            "valarm_without_action_or_trigger.ics",
            b"VALARM",
            &[
                DiagnosticCode::MissingRequiredProperty,
                DiagnosticCode::MissingRequiredProperty,
            ],
        ),
    ];
    for (name, component, wanted) in expected {
        let document = tree(fixture(name));
        let found = components(&document);
        let Some(subject) = found.iter().find(|entry| entry.is_named(component)) else {
            panic!("{name}: no component named {component:?}");
        };
        assert_eq!(
            audit_codes(subject),
            *wanted,
            "{name}: the audit of {component:?} is not what section 3.6 says"
        );
    }
}

/// An unknown component earns no report about anything it carries.
#[test]
fn an_unknown_component_is_audited_into_silence() {
    let document = tree(fixture("unknown_three_deep.ics"));
    for component in components(&document) {
        if component.kind().is_some() {
            continue;
        }
        assert!(
            audit_codes(component).is_empty(),
            "{:?}: a component with no schema earned a report",
            component.name().as_bytes()
        );
    }
}

/// The entailment `docs/adr/0001` names: `DTEND` against `DURATION`, in one `VEVENT`.
///
/// ADR-0001 says the audit reports "the known relationships an edit has broken — the all-day
/// CDO pair against `DTSTART`'s value type, `DTEND` against `DURATION`, `RRULE`'s `UNTIL`
/// against `DTSTART`'s form and zone", and `schema.rs` defers the `DTEND`/`DURATION` pair out
/// of its table and onto "the audit `docs/adr/0001` describes". `Component::audit` is the only
/// audit there is.
#[test]
fn the_audit_reports_the_entailment_it_was_deferred_to() {
    let document = tree(fixture("dtend_and_duration.ics"));
    let found = components(&document);
    let Some(event) = found.iter().find(|entry| entry.is_named(b"VEVENT")) else {
        panic!("dtend_and_duration.ics: no VEVENT");
    };
    assert!(
        event.dtend().is_present() && event.duration().is_present(),
        "dtend_and_duration.ics: the fixture no longer carries both"
    );
    assert_eq!(
        audit_codes(event),
        vec![DiagnosticCode::MutuallyExclusiveProperties],
        "dtend_and_duration.ics: a VEVENT carrying both DTEND and DURATION earns no report"
    );
}

/// RFC 5545 section 3.6.1 and section 3.6.2: a pair each section admits singly and forbids
/// together.
///
/// "Either the `DTEND` or the `DURATION` property MAY appear in a `VEVENT`, but `DTEND` and
/// `DURATION` MUST NOT occur in the same `VEVENT`", and section 3.6.2 says the same of `DUE`
/// against `DURATION` in a `VTODO`. Neither is a count of one name, so neither can be read out
/// of a table stated per name: the exclusion is a relation between two, and it is reported as
/// `DiagnosticCode::MutuallyExclusiveProperties`.
///
/// Reported once per pair rather than once per line, and reported about the pair rather than
/// about either half: a component carrying one of the two, or neither, earns nothing, and a
/// component this crate has no schema for earns nothing whatever it carries.
///
/// **What this case does not claim.** ADR-0001 names three entailments and this is the one M0
/// implements. The `X-MICROSOFT-CDO-ALLDAYEVENT` pair against `DTSTART`'s value type and
/// `RRULE`'s `UNTIL` against `DTSTART`'s form both turn on a *value* rather than on a name;
/// the second needs the recurrence grammar M1 owns. That ADR's amendments say so rather than
/// leaving the gap to be discovered here.
#[test]
fn rfc5545_3_6_1_dtend_and_duration_are_admitted_singly_and_forbidden_together() {
    // Both: exactly one report, and it is about the pair.
    let both = tree(fixture("dtend_and_duration.ics"));
    for component in components(&both) {
        let expected = if component.is_named(b"VEVENT") {
            vec![DiagnosticCode::MutuallyExclusiveProperties]
        } else {
            Vec::new()
        };
        assert_eq!(
            audit_codes(component),
            expected,
            "{:?}",
            component.name().as_bytes()
        );
    }

    // Neither: section 3.6.1 admits an event with no end and no length at all, so the silence
    // here is the other half of the claim and not the absence of a reading.
    let neither = tree(fixture("neither_dtend_nor_duration.ics"));
    for component in components(&neither) {
        assert!(
            !audit_codes(component).contains(&DiagnosticCode::MutuallyExclusiveProperties),
            "{:?} earned a report about a pair it does not carry",
            component.name().as_bytes()
        );
    }

    // The same relation in the other component, spelled with the name section 3.6.2 uses.
    let todo: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example Corp//Corpus//EN\r\n\
        BEGIN:VTODO\r\nUID:both@example.test\r\nDTSTAMP:20260810T090000Z\r\n\
        DUE:20260810T110000Z\r\nDURATION:PT2H\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    let parsed = tree(Fixture {
        name: "a VTODO carrying both DUE and DURATION",
        octets: todo,
    });
    let Some(entry) = components(&parsed)
        .into_iter()
        .find(|component| component.is_named(b"VTODO"))
    else {
        panic!("the VTODO is where it was written");
    };
    assert_eq!(
        audit_codes(entry),
        vec![DiagnosticCode::MutuallyExclusiveProperties]
    );
}

// ---------------------------------------------------------------------------------------
// P3, through the component-shaped doors
// ---------------------------------------------------------------------------------------

/// P3: a write through a duplicated identity moves one line.
///
/// Two `DTSTART`s is the case where "the property `id` names" names two properties.
/// `Component::get` refuses to pick between them; the write side has to be at least as
/// careful, because a caller that edited one of two and was told nothing has a file whose
/// meaning changed in a way no diagnostic mentions.
#[test]
fn p3_a_write_into_a_duplicated_identity_moves_one_line() {
    let case = fixture("two_dtstart.ics");
    let mut document = tree(case);
    let before = lines(&document);
    let Some(event) = find_named(&mut document, b"VEVENT") else {
        panic!("{}: no VEVENT", case.name);
    };
    let Some(mut guard) = event.get_mut::<TextValue<'_>>(&PropertyId::SUMMARY) else {
        panic!("{}: no SUMMARY to write through", case.name);
    };
    guard
        .set_raw(b"Rewritten by the corpus")
        .unwrap_or_else(|error| panic!("{}: an ordinary value was refused: {error}", case.name));
    let moved = differing(&before, &lines(&document));
    assert_eq!(
        moved.len(),
        1,
        "{}: one write moved {} lines",
        case.name,
        moved.len()
    );
}

/// P3: removing a duplicated identity removes both occurrences and nothing else.
#[test]
fn p3_removing_a_duplicated_identity_takes_both_and_no_more() {
    let case = fixture("two_dtstart.ics");
    let mut document = tree(case);
    let before = lines(&document);
    let Some(event) = find_named(&mut document, b"VEVENT") else {
        panic!("{}: no VEVENT", case.name);
    };
    event
        .apply(
            &PropertyId::DTSTART,
            &ProposedChange::Remove,
            Limits::DEFAULT,
        )
        .unwrap_or_else(|error| panic!("{}: the removal was refused: {error}", case.name));
    let after = lines(&document);
    assert_eq!(
        before.len().saturating_sub(after.len()),
        2,
        "{}: removing one identity took {} lines",
        case.name,
        before.len().saturating_sub(after.len())
    );
    let kept: Vec<&Vec<u8>> = before
        .iter()
        .filter(|line| !line.starts_with(b"DTSTART"))
        .collect();
    let seen: Vec<&Vec<u8>> = after.iter().collect();
    assert_eq!(
        kept, seen,
        "{}: removing DTSTART moved a line that was not one",
        case.name
    );
}

/// P3: adding a property to a component that holds only components moves nothing else.
///
/// The insertion goes ahead of the nested components, because section 3.6 writes properties
/// first, and the only octet it may put outside its own line is the terminator the line above
/// needs in order to stay a line.
#[test]
fn p3_an_addition_into_a_component_of_components_stays_one_line() {
    for name in [
        "components_only_no_properties.ics",
        "components_only_lf.ics",
    ] {
        let case = fixture(name);
        let mut document = tree(case);
        let before = lines(&document);
        let Some(alarm) = find_named(&mut document, b"VALARM") else {
            panic!("{name}: no VALARM");
        };
        assert_eq!(
            alarm.properties().count(),
            0,
            "{name}: the fixture no longer holds a property-free component"
        );
        alarm
            .apply(
                &PropertyId::from_name(b"X-ADDED"),
                &ProposedChange::Add(RawText::from_bytes(b"X-ADDED:one")),
                Limits::DEFAULT,
            )
            .unwrap_or_else(|error| panic!("{name}: the addition was refused: {error}"));
        let after = lines(&document);
        assert_eq!(
            after.len(),
            before.len().saturating_add(1),
            "{name}: an addition changed the line count by more than one"
        );
        let saved = document.to_bytes();
        let Ok(reloaded) = parse(&saved).document else {
            panic!("{name}: the addition wrote a file it cannot read");
        };
        assert_eq!(
            reloaded.to_bytes(),
            saved,
            "{name}: the file the addition wrote is not a fixed point"
        );
        let mut again = reloaded_document(&saved);
        let Some(round) = find_named(&mut again, b"VALARM") else {
            panic!("{name}: the reloaded file lost its VALARM");
        };
        assert_eq!(
            round.properties().count(),
            1,
            "{name}: the added property is not inside the component it was added to"
        );
    }
}

/// A sink with no room, which is what a caller with no allocator passes.
///
/// `IgnoreDiagnostics` is the crate's own version of this; a local one counts what it refused
/// so the case can check that the refusals were charged rather than swallowed.
#[derive(Debug, Default)]
struct NoRoom {
    /// How many diagnostics were offered.
    offered: u32,
}

impl ical_core::DiagnosticSink for NoRoom {
    fn push(&mut self, _diagnostic: Diagnostic) -> ical_core::SinkOutcome {
        self.offered = self.offered.saturating_add(1);
        ical_core::SinkOutcome::Refused
    }
}

/// P4: a sink that refuses every diagnostic costs the document nothing.
///
/// The promise in `docs/adr/0009` is that no reader may treat a refusal as a reason to stop
/// reading, and the component fixtures are where it bites: several of them earn a diagnostic
/// per line for lines that are still structure. A reader that stopped, or that kept less,
/// would show up as octets.
#[test]
fn p4_a_sink_with_no_room_still_yields_the_whole_file() {
    for &case in FIXTURES {
        let kept = parse(case.octets);
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut reader = ContentLineReader::new(case.octets, limits.grammar());
        let mut refusing = NoRoom::default();
        let document = Document::from_tokens(&mut reader, &mut meter, &mut refusing)
            .unwrap_or_else(|error| {
                panic!("{}: the refusing sink lost the file: {error}", case.name)
            });
        assert_eq!(
            document.to_bytes(),
            case.octets,
            "{}: a refused diagnostic cost the file an octet",
            case.name
        );
        assert_eq!(
            u32::try_from(kept.reported.len()).unwrap_or(u32::MAX),
            refusing.offered,
            "{}: the refusing sink was offered a different set",
            case.name
        );
        assert_eq!(
            meter.diagnostics_dropped(),
            refusing.offered,
            "{}: a refusal was not charged to the meter",
            case.name
        );
    }
}

/// P3: replacing a duplicated identity leaves no occurrence of it unwritten.
///
/// `ProposedChange::Remove` takes every occurrence of the identity it names, and says why:
/// "a caller that asked for a property to be gone and got one of two copies removed has no way
/// to see that it half happened". The same sentence is true of `Replace`, which is one of the
/// four variants of the same vocabulary and is addressed to the same identity.
#[test]
fn p3_replacing_a_duplicated_identity_is_not_half_applied() {
    let case = fixture("two_dtstart.ics");
    let mut document = tree(case);
    let Some(event) = find_named(&mut document, b"VEVENT") else {
        panic!("{}: no VEVENT", case.name);
    };
    assert_eq!(
        event.properties_named(&PropertyId::DTSTART).count(),
        2,
        "{}: the fixture no longer carries two DTSTARTs",
        case.name
    );
    let replacement = ProposedChange::Replace(RawText::from_bytes(b"DTSTART:20270101T000000Z"));
    event
        .apply(&PropertyId::DTSTART, &replacement, Limits::DEFAULT)
        .unwrap_or_else(|error| panic!("{}: the replacement was refused: {error}", case.name));
    let stale: Vec<String> = event
        .properties_named(&PropertyId::DTSTART)
        .map(|property| property.value_text().as_bytes().to_vec())
        .filter(|value| value.as_slice() != b"20270101T000000Z")
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .collect();
    assert!(
        stale.is_empty(),
        "{}: a replacement addressed to DTSTART left {stale:?} behind",
        case.name
    );
}

/// P3: a scoped write may not change the document's nesting.
///
/// `BEGIN` and `END` are ordinary property names to everything below the component layer, and
/// the change vocabulary is addressed to a `PropertyId`. So a caller may address a change to
/// `BEGIN`, hand over one well-formed content line carrying no control character, and have it
/// stored as a property — which is what `docs/design/ical-core-api.md` says a line this crate
/// cannot make sense of degrades to. It is written back as a `BEGIN` line, and the next reader
/// opens a component on it: every entry after the insertion moves inside a component nobody
/// added, and the enclosing `END` closes that one instead of the one it was written for.
///
/// This is the injection `break_clients.rs` records as closed, arriving through the component
/// layer rather than through a value: one write, and the file a second client opens is a
/// different tree.
#[test]
fn p3_a_change_addressed_to_a_boundary_cannot_restructure_the_document() {
    inject("valarm_misplaced.ics", b"VEVENT", b"BEGIN", b"BEGIN:VEVENT");
    inject("two_dtstart.ics", b"VEVENT", b"END", b"END:VEVENT");
}

/// Add `line` to the component named `host` in `name`, and require the nesting to survive.
///
/// The other permitted answer is a refusal, since a line that is a component boundary is not
/// a property this crate can add and keep its own reading of the file. Either is an answer;
/// silently writing one is not.
fn inject(name: &str, host: &[u8], id: &[u8], line: &[u8]) {
    let case = fixture(name);
    let mut document = tree(case);
    let shape = shape_of(&document);
    let before = parse(case.octets).reported.len();
    let found = find_named(&mut document, host);
    assert!(found.is_some(), "{name}: no component named {host:?}");
    let Some(target) = found else { return };
    let change = ProposedChange::Add(RawText::from_bytes(line));
    if target
        .apply(&PropertyId::from_name(id), &change, Limits::DEFAULT)
        .is_err()
    {
        return;
    }
    let saved = document.to_bytes();
    let outcome = parse(&saved);
    assert!(
        outcome.document.is_ok(),
        "{name}: the addition wrote a file it cannot read"
    );
    let reloaded = outcome.document.unwrap_or_default();
    let codes: Vec<DiagnosticCode> = outcome
        .reported
        .iter()
        .copied()
        .map(Diagnostic::code)
        .collect();
    assert_eq!(
        shape_of(&reloaded),
        shape,
        "{name}: adding {line:?} rewrote the nesting, and the reload now says {codes:?}"
    );
    assert_eq!(
        outcome.reported.len(),
        before,
        "{name}: adding {line:?} earned the file {codes:?}"
    );
}

/// The component names of a document, outermost first, then in document order, with the
/// nesting depth beside each, which is the shape a second client reads.
fn shape_of(document: &Document) -> Vec<(usize, Vec<u8>)> {
    let mut out: Vec<(usize, Vec<u8>)> = Vec::new();
    describe(document.items(), 0, &mut out);
    out
}

/// Append `items`' components at `depth`, walking nested components in place.
fn describe(items: &[Item], depth: usize, out: &mut Vec<(usize, Vec<u8>)>) {
    for entry in items {
        if let Item::Component(component) = entry {
            out.push((depth, component.name().as_bytes().to_ascii_uppercase()));
            describe(component.items(), depth.saturating_add(1), out);
        }
    }
}

/// Parse `octets` again, for a case that needs a second owned tree.
fn reloaded_document(octets: &[u8]) -> Document {
    let outcome = parse(octets).document;
    assert!(outcome.is_ok(), "the saved file did not parse");
    outcome.unwrap_or_default()
}

/// P3, over documents nobody chose: an ordinary addition never changes the nesting.
///
/// `sweep.rs` puts generated calendars through parse and serialize; it never writes to one.
/// The write side is where the component layer and the line layer meet — an insertion has to
/// choose a position among entries it did not create and terminate a line it did not author —
/// so the sweep here is the same idea addressed at that seam. Every generated document that
/// parses is edited once per component it holds, and the three things asserted are the three a
/// second client depends on: the file reloads, it is a fixed point, and it nests the way it did
/// before the edit.
#[test]
fn p3_a_swept_addition_never_moves_a_component() {
    let mut stream = Stream::new(SEED);
    let mut edits = 0_u32;
    let mut documents = 0_u32;
    for _ in 0..DOCUMENTS {
        let octets = generated(&mut stream);
        let Ok(document) = parse(&octets).document else {
            panic!("the default policy refused a generated document: {octets:?}");
        };
        documents = documents.saturating_add(1);
        let shape = shape_of(&document);
        let mut paths: Vec<Vec<usize>> = Vec::new();
        every_path(document.items(), &mut Vec::new(), &mut paths);
        for path in &paths {
            edits = edits.saturating_add(1);
            check_one_addition(&document, path, &shape, &octets);
        }
    }
    println!("swept {documents} generated documents and {edits} additions");
}

/// Add one ordinary property at `path` and assert what a second client depends on.
fn check_one_addition(
    document: &Document,
    path: &[usize],
    shape: &[(usize, Vec<u8>)],
    octets: &[u8],
) {
    let mut edited = document.clone();
    let reached = component_at(&mut edited, path);
    assert!(
        reached.is_some(),
        "a path this walk produced does not resolve: {path:?}"
    );
    let Some(target) = reached else { return };
    let added = PropertyId::from_name(b"X-ADDED");
    let change = ProposedChange::Add(RawText::from_bytes(b"X-ADDED:swept"));
    if target.apply(&added, &change, Limits::DEFAULT).is_err() {
        return;
    }
    let saved = edited.to_bytes();
    let read_back = parse(&saved).document;
    assert!(
        read_back.is_ok(),
        "an addition wrote a file it cannot read, from {octets:?}"
    );
    let reloaded = read_back.unwrap_or_default();
    assert_eq!(
        reloaded.to_bytes(),
        saved,
        "an addition into {path:?} of {octets:?} is not a fixed point"
    );
    assert_eq!(
        shape_of(&reloaded),
        shape,
        "an addition into {path:?} of {octets:?} moved a component"
    );
    let mut round = reloaded;
    let landing = component_at(&mut round, path);
    assert!(
        landing.is_some(),
        "the reloaded file lost the component at {path:?}, from {octets:?}"
    );
    let Some(landed) = landing else { return };
    assert_eq!(
        landed.properties_named(&added).count(),
        1,
        "the addition into {path:?} of {octets:?} did not land there"
    );
}

/// Append the index path of every component in `items`, depth first.
fn every_path(items: &[Item], prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    for (index, entry) in items.iter().enumerate() {
        let Item::Component(component) = entry else {
            continue;
        };
        prefix.push(index);
        out.push(prefix.clone());
        every_path(component.items(), prefix, out);
        prefix.pop();
    }
}

/// The component `path` names, mutably.
fn component_at<'a>(document: &'a mut Document, path: &[usize]) -> Option<&'a mut Component> {
    let mut steps = path.iter();
    let first = *steps.next()?;
    let mut here = document.items_mut().get_mut(first)?.as_component_mut()?;
    for step in steps {
        here = here.items_mut().get_mut(*step)?.as_component_mut()?;
    }
    Some(here)
}

/// The seed the sweep in this file starts from, a committed constant for `sweep.rs`'s reason.
const SEED: u64 = 0x0C0F_0FE7_C0D3_5EED;

/// How many documents the sweep draws.
const DOCUMENTS: u32 = 20_000;

/// The line bodies a generated document is built from, terminators excluded.
///
/// Every one of them is something the component layer has to decide about: a boundary that
/// opens, one that closes, one that closes something else, one that carries parameters and so
/// cannot be a boundary at all, one with no name, one with no `:`, and two ordinary properties
/// for the insertion to sit among.
const BODIES: &[&[u8]] = &[
    b"BEGIN:VEVENT",
    b"BEGIN:VALARM",
    b"BEGIN:X-Q",
    b"BEGIN:",
    b"BEGIN;X-P=1:VEVENT",
    b"BEGIN",
    b"END:VEVENT",
    b"END:VALARM",
    b"END:X-Q",
    b"END:",
    b"END;X-P=1:VEVENT",
    b"END",
    b"UID:1",
    b"X-P;A=\"b;c\":v",
    b"",
    b"begin:vevent",
    b"end:vevent",
];

/// The terminators a generated line may carry. The empty one is only ever the last.
const ENDINGS: &[&[u8]] = &[b"\r\n", b"\n", b"\r"];

/// The two octets RFC 5545 section 3.1 lets a fold continue with.
const CONTINUATIONS: &[&[u8]] = &[b" ", b"\t"];

/// One generated document: between two and seven lines, some of them folded.
fn generated(stream: &mut Stream) -> Vec<u8> {
    let count = stream.below(6).saturating_add(2);
    let mut octets: Vec<u8> = Vec::new();
    for index in 0..count {
        let body = stream.pick(BODIES).copied().unwrap_or(b"");
        push_folded(stream, body, &mut octets);
        let last = index.saturating_add(1) == count;
        if last && stream.below(4) == 0 {
            // A file whose last line carries no terminator at all, which is what makes the
            // insertion have to author one.
            break;
        }
        let ending = stream.pick(ENDINGS).copied().unwrap_or(b"\r\n");
        octets.extend_from_slice(ending);
    }
    octets
}

/// Append `body`, folded at one position a quarter of the time.
fn push_folded(stream: &mut Stream, body: &[u8], out: &mut Vec<u8>) {
    if body.is_empty() || stream.below(4) != 0 {
        out.extend_from_slice(body);
        return;
    }
    let at = stream.below(body.len());
    let (head, tail) = body.split_at(at);
    out.extend_from_slice(head);
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(stream.pick(CONTINUATIONS).copied().unwrap_or(b" "));
    out.extend_from_slice(tail);
}

/// A deterministic source of draws, `splitmix64`, spelled as `sweep.rs` spells it.
///
/// Every step is a `wrapping_*` method rather than an operator, because
/// `arithmetic_side_effects` is an error here and a mixing function is the one place where a
/// wrap is the intent.
#[derive(Debug)]
struct Stream {
    /// The whole state, advanced once per draw.
    state: u64,
}

impl Stream {
    /// A stream at `seed`.
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next sixty-four bits.
    fn draw(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mixed = self.state;
        let once = (mixed ^ mixed.wrapping_shr(30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let twice = (once ^ once.wrapping_shr(27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        twice ^ twice.wrapping_shr(31)
    }

    /// A number below `bound`, and zero when `bound` is zero.
    fn below(&mut self, bound: usize) -> usize {
        let drawn = usize::try_from(self.draw() & 0xFFFF_FFFF).unwrap_or(0);
        drawn.checked_rem(bound).unwrap_or(0)
    }

    /// One of `choices`, and nothing when there are none.
    fn pick<'a, T>(&mut self, choices: &'a [T]) -> Option<&'a T> {
        choices.get(self.below(choices.len()))
    }
}

/// A fixture that is not one, so that a lookup failure is an assertion rather than an unwrap.
const MISSING: Fixture = Fixture {
    name: "missing",
    octets: b"",
};

/// The fixture with this name, with a failed lookup named in the assertion.
fn fixture(name: &str) -> Fixture {
    let found = FIXTURES.iter().copied().find(|entry| entry.name == name);
    assert!(found.is_some(), "no fixture named {name}");
    found.unwrap_or(MISSING)
}
