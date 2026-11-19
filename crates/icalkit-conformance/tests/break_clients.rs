// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The fixtures exercise the four preservation and mutation contracts in `ADR-0001`.
//!
//! P1 compares the serialized document with every input octet.
//! P2 checks that a second parse and serialization yield the same octets as the first.
//! P3 checks that editing a property changes only its content line.
//! P4 checks that the reader preserves the input octets even when it reports a diagnostic.
//!
//! The synthetic fixtures include folded `X-APPLE-STRUCTURED-LOCATION` parameters and Microsoft extension properties.
//! They cover `/mozilla.org/` identifiers, bare line feeds, and quoted and unquoted Windows zone names.
//! Further fixtures contain long physical lines, nested unknown components, extension parameters, and `CP1252` bytes.
//! One encoding case uses valid UTF-8 for an unintended code point.

use std::collections::BTreeSet;

use icalkit_conformance::internal::core::{
    Boundary, Component, ContentLineReader, Diagnostic, DiagnosticCode, Document, Item, Limits,
    Meter, MutationError, ParseError, Property, PropertyId, TextValue,
};

/// One fixture: the octets on disk, and a name for the assertion message.
#[derive(Clone, Copy, Debug)]
struct Fixture {
    /// The filename relative to the fixture directory.
    name: &'static str,
    /// The octets exactly as committed.
    /// `.gitattributes` marks these `-text`, so the bytes here are the bytes a client would have written.
    octets: &'static [u8],
}

/// Each fixture must exist at compile time.
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

/// The parse result and the diagnostics from one run.
#[derive(Debug)]
struct Parsed {
    /// The document or its parse error.
    document: Result<Document, ParseError>,
    /// Diagnostics in the order the reader reported them.
    reported: Vec<Diagnostic>,
    /// Diagnostics the sink refused.
    /// A `Vec` never refuses, so this must stay zero.
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

/// Serialize one property with its folds and terminator.
/// The writer starts the octet count at the property name on each line.
fn property_octets(property: &Property) -> Vec<u8> {
    Document::new(vec![Item::Property(property.clone())]).to_bytes()
}

/// The octets one `BEGIN` or `END` line occupies on its own.
fn boundary_octets(boundary: &Boundary) -> Vec<u8> {
    let alone = Component::new(boundary.clone(), Vec::new(), None);
    Document::new(vec![Item::Component(alone)]).to_bytes()
}

/// Return the octets of each line in document order.
/// P3 checks that the lines concatenate to the complete document before comparing edits.
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

/// Write the first direct match before searching nested components.
fn write_first(items: &mut [Item], id: &PropertyId, text: &[u8], path: &mut Vec<usize>) -> bool {
    for (index, item) in items.iter_mut().enumerate() {
        let Some(component) = item.as_component_mut() else {
            continue;
        };
        path.push(index);
        if let Some(position) = component.items().iter().position(|entry| {
            entry
                .as_property()
                .is_some_and(|property| property.has_id(id))
        }) {
            if component
                .get_mut::<TextValue<'_>>(id)
                .is_some_and(|mut guard| guard.set_raw(text).is_ok())
            {
                path.push(position);
                return true;
            }
        } else if write_first(component.items_mut(), id, text, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// Return the item path of the property that accepted the write.
fn write_somewhere(document: &mut Document, id: &PropertyId, text: &[u8]) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    write_first(document.items_mut(), id, text, &mut path).then_some(path)
}

fn property_at<'a>(document: &'a Document, path: &[usize]) -> Option<&'a Property> {
    let (position, parents) = path.split_last()?;
    let mut items = document.items();
    for &parent in parents {
        items = items.get(parent)?.as_component()?.items();
    }
    items.get(*position)?.as_property()
}

/// Try the scalar text properties in order until a fixture accepts a write.
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
            .find(|id| write_somewhere(&mut document, id, b"Rewritten by the corpus").is_some())
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

/// P4 checks that parsing preserves the bytes and reports every diagnostic.
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

/// Each generation checks the saved value at the edited position and the bytes after reopening.
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
            // Alternate between one physical line and a value that needs folding.
            let payload: &[u8] = if generation % 2 == 0 {
                b"a value long enough that this crate has to fold the line itself rather than \
                  reuse any fold position the producer chose for the text it replaced"
            } else {
                b"short"
            };
            let (id, path) = EDITED
                .iter()
                .find_map(|id| write_somewhere(&mut document, id, payload).map(|path| (id, path)))
                .unwrap_or_else(|| panic!("{}: generation {generation} made no edit", case.name));
            let saved = document.to_bytes();
            let Ok(reread) = parse(&saved).document else {
                panic!(
                    "{}: generation {generation} wrote a file it cannot read",
                    case.name
                );
            };
            let property = property_at(&reread, &path).unwrap_or_else(|| {
                panic!(
                    "{}: generation {generation} lost the property at {path:?}",
                    case.name
                )
            });
            assert!(property.has_id(id));
            assert_eq!(
                property.value_text().as_bytes(),
                payload,
                "{}: generation {generation}",
                case.name
            );
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

/// P3 checks mutation locality after a save and reload.
///
/// Unchecked setters once let callers change a value through `Component::items_mut`.
/// The setters now have crate visibility.
/// The public `PropertyMut::set_raw` rejects line terminators and preserves every octet on refusal.
/// An ordinary replacement changes one content line after reopening.
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

/// Write the first direct `SUMMARY` of each event and return the last result.
/// Return `None` if no event supplied a summary.
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

/// Both designated fixtures must produce a diagnostic for the P4 checks.
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
