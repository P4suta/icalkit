// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! BREAK. A calendar assembled from scratch used to be able to write a line nobody added.
//!
//! The scoped-write door was closed first: `PropertyMut::set_raw` refuses a value carrying a
//! terminator, and `write_side_grammar.rs` records what it refuses and why. The tree-building
//! door was not, and it is the same door. `Property::new` was public and unchecked, so
//! `Property::new(b"SUMMARY", [], b"a\r\nATTENDEE:mailto:eve@example.test", ..)` pushed into a
//! component through `Component::items_mut` serialized as two content lines, and the next read
//! of that file found an attendee the caller never wrote. A `SUMMARY` taken from a web form is
//! how those octets arrive.
//!
//! The door had to stay open — a caller assembling a calendar from nothing is the ordinary
//! case, not the attack — so it was not closed but moved. `Property::new`, `Parameter::new` and
//! `Boundary::new` are crate-private, which is why no test in this file can call them, and the
//! public doors are `Property::create`, `Parameter::create` and `Component::create`, each of
//! which refuses exactly the octets RFC 5545 section 3.1 has no way to write back. Refusing
//! costs the round-trip claim of `docs/adr/0001` nothing: octets that were never read from
//! anywhere have no producer's spelling to preserve.
//!
//! So this file states two things about one boundary. What construction refuses, it refuses
//! before anything is stored. What construction accepts, it writes as exactly the lines the
//! caller asked for — which is the claim that makes the refusal worth having, because a door
//! that refused everything would satisfy the first on its own.

use ical_core::{Component, Document, Item, Limits, MutationError, Parameter, Property};

/// The octets the debt was recorded with: a value that ends its own line and starts another.
const INJECTION: &[u8] = b"a\r\nATTENDEE:mailto:eve@example.test";

/// How many content lines `octets` holds, counted as section 3.1 terminates them.
///
/// Counted rather than parsed, because the question is what was *written*: a reader that
/// unfolded or recovered would answer about the tree it built and not about the file.
fn line_count(octets: &[u8]) -> usize {
    octets.windows(2).filter(|pair| *pair == b"\r\n").count()
}

/// The whole of one document as the octets a file would hold.
fn written(items: Vec<Item>) -> Vec<u8> {
    Document::new(items).to_bytes()
}

#[test]
fn rfc5545_3_1_a_constructed_value_cannot_carry_a_second_content_line() {
    let refused = Property::create(b"SUMMARY", Vec::new(), INJECTION);
    assert_eq!(
        refused.err(),
        Some(MutationError::IllegalControlCharacter),
        "the octets the debt was measured with are still constructible"
    );
}

/// Every octet a value has no spelling for, at the door that used to take all of them.
///
/// `CRLF` is the injection; a bare `CR`, a bare `LF` and a `NUL` are the same defect written
/// differently, and section 3.1 excludes all of them from a value. `HTAB` is here because this
/// crate refuses it too, which is stricter than the grammar and is a decision worth pinning
/// rather than inheriting.
#[test]
fn rfc5545_3_1_construction_refuses_every_control_character_rather_than_escaping_one() {
    let refused: &[&[u8]] = &[
        INJECTION, b"a\rb", b"a\nb", b"a\x00b", b"a\x07b", b"a\tb", b"a\x7fb",
    ];
    for value in refused {
        let outcome = Property::create(b"SUMMARY", Vec::new(), value);
        assert_eq!(
            outcome.err(),
            Some(MutationError::IllegalControlCharacter),
            "{value:?} was accepted"
        );
    }
}

/// The same injection through the name and through a parameter, which are lines too.
///
/// A name carrying a `:` ends the header where the caller did not, and a parameter value
/// carrying a `DQUOTE` closes a quote section 3.2 opened; both write octets back as something
/// other than what was handed over, so both are refused at the door rather than escaped past
/// it. The empty name is here because it is the one shape that reads back as nothing at all.
///
/// **What this crate does not refuse, and why.** Section 3.1's `name` is `iana-token / x-name`
/// and admits `ALPHA`, `DIGIT` and `-` only, so `SUM MARY` and `SUM.MARY` are names the
/// grammar has no production for. This door takes them, because the rule it applies is the
/// mechanical one — a name the reader would not hand back whole — and producers really do
/// write `_` and `.` in vendor names. A stricter door would refuse a name that round-trips,
/// which is the trade the case below records rather than argues.
#[test]
fn rfc5545_3_1_a_constructed_name_and_parameter_are_refused_on_the_same_terms() {
    let names: &[&[u8]] = &[b"", b"SUM:MARY", b"SUM;MARY", INJECTION, b"SUM\x00MARY"];
    for name in names {
        assert_eq!(
            Property::create(name, Vec::new(), b"Lunch").err(),
            Some(MutationError::NotRepresentable),
            "property name {name:?} was accepted"
        );
        assert_eq!(
            Component::create(name, Vec::new()).err(),
            Some(MutationError::NotRepresentable),
            "component name {name:?} was accepted"
        );
    }

    let values: &[&[u8]] = &[b"say \"hi\"", INJECTION, b"bell\x07"];
    for value in values {
        assert_eq!(
            Parameter::create(b"CN", value).err(),
            Some(MutationError::NotRepresentable),
            "parameter value {value:?} was accepted"
        );
    }

    // The other side of the reading stated above: a name the ABNF has no place for is taken,
    // and taken because it reads back as itself rather than because nobody looked.
    let odd = Property::create(b"X-SUM MARY.1", Vec::new(), b"Lunch")
        .unwrap_or_else(|error| panic!("a name that round-trips is writable: {error:?}"));
    let octets = written(vec![Item::Property(odd)]);
    assert_eq!(octets, b"X-SUM MARY.1:Lunch\r\n".to_vec());
    assert_eq!(line_count(&octets), 1);
}

/// What the door is for: a calendar built from nothing writes the lines its author stated.
///
/// Six lines asked for and six lines written, with the value that needed quoting quoted and
/// nothing else moved. This is the assertion that keeps the refusals above honest — a
/// constructor that answered `NotRepresentable` to everything would pass every test before
/// this one.
#[test]
fn rfc5545_3_6_a_calendar_assembled_from_nothing_writes_the_lines_it_was_given() {
    let organizer = Parameter::create(b"CN", b"Doe, John")
        .unwrap_or_else(|error| panic!("a display name with a comma is writable: {error:?}"));
    let event = Component::create(
        b"VEVENT",
        vec![
            Item::Property(
                Property::create(b"UID", Vec::new(), b"1@example.test")
                    .unwrap_or_else(|error| panic!("UID: {error:?}")),
            ),
            Item::Property(
                Property::create(b"ORGANIZER", vec![organizer], b"mailto:j@example.test")
                    .unwrap_or_else(|error| panic!("ORGANIZER: {error:?}")),
            ),
        ],
    )
    .unwrap_or_else(|error| panic!("VEVENT: {error:?}"));
    let calendar = Component::create(b"VCALENDAR", vec![Item::Component(event)])
        .unwrap_or_else(|error| panic!("VCALENDAR: {error:?}"));

    let octets = written(vec![Item::Component(calendar)]);
    assert_eq!(
        octets,
        b"BEGIN:VCALENDAR\r\n\
          BEGIN:VEVENT\r\n\
          UID:1@example.test\r\n\
          ORGANIZER;CN=\"Doe, John\":mailto:j@example.test\r\n\
          END:VEVENT\r\n\
          END:VCALENDAR\r\n"
            .to_vec()
    );
    assert_eq!(line_count(&octets), 6, "six lines asked for");
}

/// The property that carries the injection's octets minus the terminator still writes one line.
///
/// The refusal is about the two octets that end a line and not about the shape of the value: a
/// caller whose display text really does hold `ATTENDEE:mailto:...` gets it written, escaped
/// where section 3.3.11 requires an escape by the codec that owns that, and read back as one
/// property. Without this case the refusals above would be consistent with a door that refused
/// anything that looked suspicious.
#[test]
fn rfc5545_3_1_the_same_octets_without_a_terminator_are_one_line_and_are_written() {
    let property = Property::create(
        b"SUMMARY",
        Vec::new(),
        b"a ATTENDEE:mailto:eve@example.test",
    )
    .unwrap_or_else(|error| panic!("no control character, no refusal: {error:?}"));
    let octets = written(vec![Item::Property(property)]);
    assert_eq!(line_count(&octets), 1);

    let mut kept = Vec::new();
    let reread = Document::parse(&octets, Limits::DEFAULT, &mut kept)
        .unwrap_or_else(|error| panic!("what construction wrote is readable: {error:?}"));
    assert!(kept.is_empty(), "a line this crate authored earns nothing");
    assert_eq!(reread.items().len(), 1, "one property, not two");
    assert_eq!(reread.to_bytes(), octets, "P1 over a constructed line");
}
