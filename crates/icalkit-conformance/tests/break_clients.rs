// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Four separable properties of the round trip, attacked with what real clients emit.
//!
//! `ADR-0001` claims one sentence and this file takes it apart into the four claims that
//! sentence is actually made of, because only the first of them is obvious and the third is
//! the one nobody tests.
//!
//! - **P1**, round trip: `serialize(parse(x))` is `x`, octet for octet.
//! - **P2**, fixed point: parsing and serializing the output of a parse and a serialize
//!   changes nothing further. This is what catches a parser that normalizes on the first pass
//!   and then disagrees with itself.
//! - **P3**, mutation locality: writing one property's value changes that property's octets
//!   and no others. `ADR-0001` states this explicitly and the crate's own corpus test for it
//!   does not exist yet.
//! - **P4**, diagnostics preserve: an input that earns a diagnostic still satisfies P1, so a
//!   violation is never "accepted" by quietly dropping the thing that violated.
//!
//! The fixtures beside this file stand in for the corpus that milestone M5 will collect. Each
//! is synthetic and each is shaped like something a named client actually writes: a fold
//! landing inside a quoted `X-APPLE-STRUCTURED-LOCATION` parameter that carries `=` and `,`,
//! the `X-MICROSOFT-CDO-` family, a `/mozilla.org/`-prefixed `TZID` in a file terminated with
//! bare line feeds, a Windows zone name with spaces, dots and parentheses both quoted and
//! bare, an unfolded line several hundred octets long, unknown components nested three deep,
//! an unknown property carrying an unknown parameter, and `CP1252` octets in a file that
//! declares no charset — including the well-formed-`UTF-8`-wrong-codepoint case that neither
//! failure channel can see.

use std::collections::BTreeSet;

use ical_core::{
    Boundary, Component, ContentLineReader, Diagnostic, DiagnosticCode, Document, Item, Limits,
    Meter, MutationError, ParseError, Property, PropertyId, TextValue,
};

/// One fixture: the octets on disk, and a name for the assertion message.
#[derive(Clone, Copy, Debug)]
struct Fixture {
    /// The file name, relative to this file's fixture directory.
    name: &'static str,
    /// The octets exactly as committed. `.gitattributes` marks these `-text`, so the bytes
    /// here are the bytes a client would have written.
    octets: &'static [u8],
}

/// Every fixture, embedded rather than read, so a case cannot pass by not being found.
const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "apple_structured_location.ics",
        octets: include_bytes!("fixtures/break_clients/apple_structured_location.ics"),
    },
    Fixture {
        name: "microsoft_cdo.ics",
        octets: include_bytes!("fixtures/break_clients/microsoft_cdo.ics"),
    },
    Fixture {
        name: "mozilla_tzid_lf.ics",
        octets: include_bytes!("fixtures/break_clients/mozilla_tzid_lf.ics"),
    },
    Fixture {
        name: "windows_tzid_spaces.ics",
        octets: include_bytes!("fixtures/break_clients/windows_tzid_spaces.ics"),
    },
    Fixture {
        name: "long_unfolded_line.ics",
        octets: include_bytes!("fixtures/break_clients/long_unfolded_line.ics"),
    },
    Fixture {
        name: "nested_unknown_components.ics",
        octets: include_bytes!("fixtures/break_clients/nested_unknown_components.ics"),
    },
    Fixture {
        name: "unknown_property_unknown_param.ics",
        octets: include_bytes!("fixtures/break_clients/unknown_property_unknown_param.ics"),
    },
    Fixture {
        name: "cp1252_summary.ics",
        octets: include_bytes!("fixtures/break_clients/cp1252_summary.ics"),
    },
    Fixture {
        name: "mixed_terminators.ics",
        octets: include_bytes!("fixtures/break_clients/mixed_terminators.ics"),
    },
    Fixture {
        name: "outlook_folded_header.ics",
        octets: include_bytes!("fixtures/break_clients/outlook_folded_header.ics"),
    },
    Fixture {
        name: "fold_splits_codepoint.ics",
        octets: include_bytes!("fixtures/break_clients/fold_splits_codepoint.ics"),
    },
    Fixture {
        name: "web_form_summary.ics",
        octets: include_bytes!("fixtures/break_clients/web_form_summary.ics"),
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

/// The octets one property occupies on its own, folds and terminator included.
///
/// A line's serialization depends on nothing outside its own node — the writer's octet
/// counter starts at the first octet of the name for every line — so writing one node alone
/// yields exactly the octets that node contributes to the whole document.
fn property_octets(property: &Property) -> Vec<u8> {
    Document::new(vec![Item::Property(property.clone())]).to_bytes()
}

/// The octets one `BEGIN` or `END` line occupies on its own.
fn boundary_octets(boundary: &Boundary) -> Vec<u8> {
    let alone = Component::new(boundary.clone(), Vec::new(), None);
    Document::new(vec![Item::Component(alone)]).to_bytes()
}

/// Every line-bearing node of the tree, in document order, each as its own octets.
///
/// Concatenating these is the whole document, which the P3 case asserts before it compares
/// anything — a decomposition that did not add back up would make the locality claim vacuous.
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

/// Write `text` over the first property named `id`, anywhere in the tree.
///
/// Depth first and left to right, so the property found is the first one a reader walking the
/// document in order would reach.
fn write_first(component: &mut Component, id: &PropertyId, text: &[u8]) -> bool {
    if component.properties_named(id).next().is_some() {
        if let Some(mut guard) = component.get_mut::<TextValue<'_>>(id) {
            return guard.set_raw(text).is_ok();
        }
    }
    for nested in component.components_mut() {
        if write_first(nested, id, text) {
            return true;
        }
    }
    false
}

/// Write `text` over the first property named `id` anywhere in `document`.
fn write_somewhere(document: &mut Document, id: &PropertyId, text: &[u8]) -> bool {
    for top in document.components_mut() {
        if write_first(top, id, text) {
            return true;
        }
    }
    false
}

/// The identities a fixture is edited through, tried in order.
///
/// Every fixture carries at least one of these, and each is a scalar text property a client
/// would routinely rewrite.
const EDITED: &[PropertyId] = &[
    PropertyId::SUMMARY,
    PropertyId::DESCRIPTION,
    PropertyId::UID,
];

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

/// P1: parse then serialize is the input, octet for octet.
#[test]
fn p1_parse_then_serialize_is_the_input() {
    for case in FIXTURES {
        let outcome = parse(case.octets);
        let document = outcome.document.unwrap_or_else(|error| {
            panic!("{}: parse refused the file: {error}", case.name);
        });
        assert_eq!(
            document.to_bytes(),
            case.octets,
            "{}: parse then serialize is not the input",
            case.name
        );
    }
}

/// P2: a second parse and serialize changes nothing further.
#[test]
fn p2_a_second_round_trip_is_a_fixed_point() {
    for case in FIXTURES {
        let Ok(first) = parse(case.octets).document else {
            continue;
        };
        let once = first.to_bytes();
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

/// P3: writing one property's value leaves every other line's octets alone.
#[test]
fn p3_one_write_moves_one_lines_octets_and_no_others() {
    for case in FIXTURES {
        let Ok(mut document) = parse(case.octets).document else {
            continue;
        };
        let before = lines(&document);
        assert_eq!(
            before.concat(),
            document.to_bytes(),
            "{}: the line decomposition does not add up to the document",
            case.name
        );

        let Some(edited) = EDITED
            .iter()
            .find(|id| write_somewhere(&mut document, id, b"Rewritten by the corpus"))
        else {
            panic!("{}: no scalar text property to write through", case.name);
        };

        let after = lines(&document);
        let moved = differing(&before, &after);
        assert_eq!(
            moved.len(),
            1,
            "{}: writing {:?} moved {} lines rather than one",
            case.name,
            core::str::from_utf8(edited.as_bytes()),
            moved.len()
        );
    }
}

/// P4: an input that earns a diagnostic still satisfies P1, and no diagnostic is lost.
#[test]
fn p4_a_diagnosed_input_still_round_trips() {
    for case in FIXTURES {
        let outcome = parse(case.octets);
        assert_eq!(
            outcome.dropped, 0,
            "{}: a growable sink refused a diagnostic",
            case.name
        );
        let Ok(document) = outcome.document else {
            continue;
        };
        if outcome.reported.is_empty() {
            continue;
        }
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

/// P2 as two clients experience it: five rounds of open, edit, save, hand over.
///
/// Byte identity on one pass is the weakest form of the claim. What a corpus has to survive is
/// the loop the format exists inside — one client saves, another opens the file it wrote, edits
/// something else, and saves again — because that is where a normalization applied on the first
/// pass would compound instead of cancelling.
#[test]
fn p2_five_generations_of_open_edit_and_save() {
    for case in FIXTURES {
        let mut current: Vec<u8> = Vec::from(case.octets);
        for generation in 0_u32..5 {
            let Ok(mut document) = parse(&current).document else {
                panic!("{}: generation {generation} refused the file", case.name);
            };
            assert_eq!(
                document.to_bytes(),
                current,
                "{}: generation {generation} is not a fixed point",
                case.name
            );
            // Alternating lengths, so that one generation is short enough to sit on one
            // physical line and the next is long enough that the crate folds the line itself.
            let payload: &[u8] = if generation % 2 == 0 {
                b"a value long enough that this crate has to fold the line itself rather than \
                  reuse any fold position the producer chose for the text it replaced"
            } else {
                b"short"
            };
            for id in EDITED {
                if write_somewhere(&mut document, id, payload) {
                    break;
                }
            }
            let saved = document.to_bytes();
            let Ok(reread) = parse(&saved).document else {
                panic!(
                    "{}: generation {generation} wrote a file it cannot read",
                    case.name
                );
            };
            assert_eq!(
                reread.to_bytes(),
                saved,
                "{}: generation {generation} does not survive being reopened",
                case.name
            );
            current = saved;
        }
    }
}

/// P3 across a save and a reload, which is the only way a second client sees an edit.
///
/// The refusal that makes a scoped write safe lives on [`ical_core::PropertyMut::set_raw`], and
/// the design calls it "the one place the crate rejects caller input outright". That sentence is
/// a claim about the *only* door, and it was once false: `Property::set_value_text` was public,
/// reachable through [`ical_core::Component::items_mut`], and checked nothing, so a `SUMMARY`
/// taken from a web form carried its own terminator into the file, serialization duly wrote it,
/// and the second client read back a component with an `ATTENDEE` nobody added — one write
/// moving six of twelve lines once the file was saved and reopened.
///
/// The unchecked setters are crate-private now, which is what makes the sentence true rather
/// than customary: a check repeated on each of them would have closed `set_value_text` and
/// `set_name` and left `edit_parameters` open, because a `&mut Vec` handed out is a door no
/// check can stand in front of. So this case asserts the two halves of what is left. The
/// injection is refused by the only door there is, and a refused write moves nothing at all;
/// an ordinary value goes through the same door and moves exactly one line.
#[test]
fn p3_a_value_written_through_the_public_setter_stays_one_property() {
    let Some(case) = FIXTURES
        .iter()
        .find(|entry| entry.name == "web_form_summary.ics")
    else {
        panic!("the web form fixture is missing");
    };
    let Ok(mut document) = parse(case.octets).document else {
        panic!("{}: the fixture does not parse", case.name);
    };
    let before = lines(&document);

    // Exactly what a booking form yields when the visitor pastes a two-line subject.
    let injected: &[u8] = b"Booking\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:eve@example.test";
    let refusal = write_summary(&mut document, injected);
    assert_eq!(
        refusal,
        Some(Err(MutationError::IllegalControlCharacter)),
        "{}: the only door into a value refuses a value carrying a terminator",
        case.name
    );
    assert_eq!(
        document.to_bytes(),
        case.octets,
        "{}: a refused write leaves the file exactly as it was",
        case.name
    );

    // The same door, with octets a booking form could legitimately produce.
    assert_eq!(
        write_summary(&mut document, b"Booking for the other room"),
        Some(Ok(())),
        "{}: an ordinary value is writable",
        case.name
    );
    let saved = document.to_bytes();
    let Ok(reloaded) = parse(&saved).document else {
        panic!("{}: the saved file does not parse", case.name);
    };
    let moved = differing(&before, &lines(&reloaded));
    assert_eq!(
        moved.len(),
        1,
        "{}: one write moved {} lines once the file was saved and reopened",
        case.name,
        moved.len()
    );
}

/// Write `text` over every `SUMMARY` in `document`, through the one door there is.
///
/// `None` when the document carried no `SUMMARY` at all, so a fixture that quietly stopped
/// having one fails the case rather than passing it vacuously.
fn write_summary(document: &mut Document, text: &[u8]) -> Option<Result<(), MutationError>> {
    let mut outcome = None;
    for calendar in document.components_mut() {
        for event in calendar.components_mut() {
            let Some(mut guard) = event.get_mut::<TextValue<'_>>(&PropertyId::SUMMARY) else {
                continue;
            };
            outcome = Some(guard.set_raw(text));
        }
    }
    outcome
}

/// The two fixtures that must earn a diagnostic do, so P4 is not passing vacuously.
#[test]
fn p4_the_violating_fixtures_are_actually_diagnosed() {
    let expected: &[(&str, DiagnosticCode)] = &[
        ("mozilla_tzid_lf.ics", DiagnosticCode::BareLineFeed),
        ("mixed_terminators.ics", DiagnosticCode::BareLineFeed),
        (
            "mixed_terminators.ics",
            DiagnosticCode::MissingFinalLineBreak,
        ),
        (
            "mixed_terminators.ics",
            DiagnosticCode::MissingValueSeparator,
        ),
        ("mixed_terminators.ics", DiagnosticCode::EmptyPropertyName),
    ];
    for (name, wanted) in expected {
        let Some(case) = FIXTURES.iter().find(|entry| entry.name == *name) else {
            panic!("no fixture named {name}");
        };
        let reported = parse(case.octets).reported;
        let codes: Vec<DiagnosticCode> = reported.iter().copied().map(Diagnostic::code).collect();
        assert!(
            codes.contains(wanted),
            "{name}: expected {wanted}, got {codes:?}"
        );
    }
}
