// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Three debts that were closed at the octets they were measured with and open one spelling
//! over, and the doors that now close them.
//!
//! **What changed.** Each case below was a break when it was written and is a conformance case
//! now: the construction and mutation doors refuse a line named `BEGIN` or `END`, the
//! serializer writes the terminator a line owes once something is written after it, and the
//! parameter door spells its value the way RFC 6868 section 2 says a producer must. The prose
//! under each case is the argument that was made against the old answer, kept because the
//! fixtures are the octets that answer used to write.
//!
//! The construction debt was recorded as one sentence:
//! `Property::new(b"SUMMARY", [], b"a\r\nATTENDEE:mailto:eve@example.test", ..)` pushed through
//! `Component::items_mut` serialized as two content lines and read back as a property nobody
//! added. `break_construction.rs` records that those octets are now refused, and they are. What
//! the refusal is *about* is stated in `mutate.rs` as the mechanical rule — a line the reader
//! would not hand back as the thing that was stored — and two other spellings of that same
//! defect go through the same doors untouched.
//!
//! The first needs no file at all. `Property::create(b"END", [], b"VEVENT")` is accepted by the
//! checked door, because `property_name_is_representable` asks only whether a name reads back
//! whole, and `END` does. The line it writes is a component boundary, so a `SUMMARY` sitting
//! after it inside a `VEVENT` is read back inside the `VCALENDAR` instead: one property leaves
//! the event it was written into, and `Property::create(b"BEGIN", ..)` puts one inside an alarm
//! that nobody added. Every octet of both is authored by this crate through `Property::create`,
//! `Component::create` and `Document::new`, with nothing parsed anywhere.
//!
//! The second is the debt's own door. A property read from a file whose last line carried no
//! terminator holds a layout with no terminator, and `items_mut` will put it anywhere. Placed
//! above another line it writes no separator between them, so two content lines go in and one
//! comes out — the debt's defect with the sign reversed, and this one is silent: the reader
//! reports nothing at all about the line that disappeared.
//!
//! The third is RFC 6868. Both directions landed, and `Parameter::create` — the one door in
//! `ical-core` that authors a parameter value — was not taught to consult either. A caller
//! that hands it the display name `Ann ^n Marie` gets those octets on the wire verbatim, and
//! `decode_caret`, which is this crate's own reading of them, answers `Ann \n Marie`. The
//! caret module states the obligation in the other direction and in these words: "Both
//! directions or neither: a value written `^'` and read back as two octets is a round trip the
//! crate fails against itself." This is that sentence with the roles swapped, and nothing
//! reports it, because `^n` is a pair RFC 6868 defines.
//!
//! What is not claimed here. None of these costs an octet: every case round-trips byte for
//! byte, and each asserts that first, so the break is never confused with P1. The traversal
//! debt was pushed instead of doubted, and the last two cases in this file are that push.

use std::cmp::Ordering;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use ical_core::{
    Component, Diagnostic, DiagnosticCode, Document, Item, Limits, MutationError, Parameter,
    Property, decode_caret, encode_caret,
};

/// One committed fixture of this attack's own directory.
///
/// `assert!` rather than an unwrap, because a helper outside a test function is production code
/// as far as the workspace lint profile is concerned.
fn fixture(name: &str) -> Vec<u8> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push("break_debts");
    path.push(name);
    let read = fs::read(&path);
    assert!(read.is_ok(), "reading {}: {:?}", path.display(), read.err());
    read.unwrap_or_default()
}

/// Parse under the default policy, and hand back what was built beside what was said about it.
fn read(octets: &[u8]) -> (Document, Vec<DiagnosticCode>) {
    let mut kept: Vec<Diagnostic> = Vec::new();
    let outcome = Document::parse(octets, Limits::DEFAULT, &mut kept);
    assert!(outcome.is_ok(), "no bound was crossed: {:?}", outcome.err());
    let reported = kept.iter().map(|held| held.code()).collect();
    (outcome.unwrap_or_default(), reported)
}

/// Every component of a document in document order, as its name and what it directly holds.
///
/// The shape rather than the whole tree, because the claim under test is about which component
/// a line ended up inside, and a whole-tree comparison would also fail on the line layouts a
/// parse records and a construction does not. Walked on an explicit stack for the reason every
/// other traversal in this workspace uses one.
fn shape(document: &Document) -> Vec<(Vec<u8>, usize, usize)> {
    let mut described = Vec::new();
    let mut pending: Vec<&Component> = document.components().collect();
    pending.reverse();
    while let Some(component) = pending.pop() {
        described.push((
            component.name().as_bytes().to_vec(),
            component.properties().count(),
            component.components().count(),
        ));
        let mut nested: Vec<&Component> = component.components().collect();
        nested.reverse();
        pending.extend(nested);
    }
    described
}

/// A `VEVENT` holding a `UID`, then the line under test, then a `SUMMARY` carrying `trailing`.
///
/// The order is the point: the injected line sits between two ordinary ones, so what it does to
/// the line below it is visible as that line changing components.
fn event_holding(name: &[u8], value: &[u8], trailing: &[u8]) -> Result<Component, MutationError> {
    Component::create(
        b"VEVENT",
        vec![
            Item::Property(Property::create(b"UID", Vec::new(), b"1@example.test")?),
            Item::Property(Property::create(name, Vec::new(), value)?),
            Item::Property(Property::create(b"SUMMARY", Vec::new(), trailing)?),
        ],
    )
}

/// The last property of the innermost last component: the line a truncated export cut off.
fn deepest_last_property(document: &Document) -> Option<Property> {
    let mut current = document.components().last()?;
    while let Some(nested) = current.components().last() {
        current = nested;
    }
    current.properties().last().cloned()
}

/// A ladder of `depth` `BEGIN:X` lines and the `END:X` lines that close them.
fn ladder_of(depth: usize) -> Vec<u8> {
    let mut octets = Vec::new();
    for _ in 0..depth {
        octets.extend_from_slice(b"BEGIN:X\r\n");
    }
    for _ in 0..depth {
        octets.extend_from_slice(b"END:X\r\n");
    }
    octets
}

/// What a `BTreeMap` key would compute, which is the traversal `Hash` performs.
fn hashed(document: &Document) -> u64 {
    let mut state = DefaultHasher::new();
    document.hash(&mut state);
    state.finish()
}

/// RFC 5545 section 3.6: a line named `END` closes a component, so no door authors one.
///
/// The debt's rule — "a name the reader would not hand back whole" — is a claim about the
/// grammar underneath, and `END` satisfies it: the reader hands back `END`. What it hands the
/// *line* to is a component model, and that is the layer the refusal had to be stated at. The
/// fixture is the file the door used to write, kept because what it means is the whole
/// argument: the `SUMMARY` written inside the event is read back inside the calendar, and this
/// crate's own reader reports a violation against a file this crate authored.
#[test]
fn rfc5545_3_6_a_property_named_end_closes_the_component_it_was_built_inside() {
    assert_eq!(
        event_holding(b"END", b"VEVENT", b"moved out of the event").err(),
        Some(MutationError::ComponentBoundary),
        "the checked door authored a line the next reader closes a component on"
    );

    // What that door used to write, read as any reader reads it. The tree the caller stated —
    // one event holding three properties — is not the tree these octets are.
    let octets = fixture("authored_end_injection.ics");
    let (reread, reported) = read(&octets);
    assert_eq!(
        reread.to_bytes(),
        octets,
        "P1 holds over the file either way"
    );
    assert_eq!(
        shape(&reread),
        vec![(b"VCALENDAR".to_vec(), 2, 1), (b"VEVENT".to_vec(), 1, 0)],
        "the SUMMARY written into the event is read back inside the calendar"
    );
    assert!(
        reported.contains(&DiagnosticCode::MismatchedEndName),
        "and the file earns a violation: {reported:?}"
    );
}

/// RFC 5545 section 3.6: a line named `BEGIN` opens one, with the same answer.
#[test]
fn rfc5545_3_6_a_property_named_begin_opens_a_component_nobody_added() {
    assert_eq!(
        event_holding(b"BEGIN", b"VALARM", b"swallowed by an alarm").err(),
        Some(MutationError::ComponentBoundary),
        "the checked door authored a line the next reader opens a component on"
    );

    let octets = fixture("authored_begin_injection.ics");
    let (reread, reported) = read(&octets);
    assert_eq!(
        reread.to_bytes(),
        octets,
        "P1 holds over the file either way"
    );
    assert_eq!(
        shape(&reread),
        vec![(b"VEVENT".to_vec(), 1, 1), (b"VALARM".to_vec(), 2, 0)],
        "a VALARM nobody added, holding a property that was the event's"
    );
    assert!(
        reported.contains(&DiagnosticCode::MismatchedEndName),
        "and the file earns violations: {reported:?}"
    );

    // Case is not a way past it, because section 3.1 compares a name without case.
    for spelling in [&b"begin"[..], b"bEgIn", b"end", b"End"] {
        assert_eq!(
            Property::create(spelling, Vec::new(), b"VALARM").err(),
            Some(MutationError::ComponentBoundary),
            "{spelling:?}"
        );
    }
}

/// The debt's own door, with a property whose layout only a reader can produce.
///
/// `Property::create` always writes a terminated layout, so this defect cannot be assembled out
/// of nothing — it needs one line read from a file that stopped without a terminator, which is
/// what a truncated download and what most hand-written exports look like. A merge tool copying
/// a property from one calendar into another is the ordinary case, not the attack.
#[test]
fn rfc5545_3_1_a_property_with_no_terminator_swallows_the_line_placed_after_it() {
    let (truncated, reported) = read(&fixture("truncated_last_line.ics"));
    assert!(
        reported.contains(&DiagnosticCode::MissingFinalLineBreak),
        "the fixture is the file this defect needs: {reported:?}"
    );
    let copied = deepest_last_property(&truncated)
        .unwrap_or_else(|| panic!("the truncated export ends on a property"));

    let addition = Property::create(b"UID", Vec::new(), b"2@example.test")
        .unwrap_or_else(|error| panic!("UID: {error:?}"));
    let assembled = Document::new(vec![Item::Property(copied), Item::Property(addition)]);

    let octets = assembled.to_bytes();
    let (reread, said) = read(&octets);
    assert_eq!(reread.to_bytes(), octets, "P1 holds over what was authored");

    assert_eq!(
        reread.items().len(),
        assembled.items().len(),
        "two content lines were stored and {:?} came back",
        String::from_utf8_lossy(&octets)
    );
    assert!(
        said.is_empty(),
        "nothing is wrong with the file, so nothing is reported: {said:?}"
    );

    // The terminator is the one section 3.1 requires, and it is written between the two lines
    // rather than after the last one: the property that is still last is still unterminated,
    // which is the octet the file it came from did not have.
    assert_eq!(
        octets,
        b"SUMMARY:cut off in transit\r\nUID:2@example.test\r\n".to_vec()
    );
    let alone = Document::new(vec![Item::Property(
        deepest_last_property(&truncated).unwrap_or_else(|| panic!("still there")),
    )]);
    assert_eq!(
        alone.to_bytes(),
        b"SUMMARY:cut off in transit".to_vec(),
        "a line that is still last still ends the way its producer left it"
    );
}

/// RFC 6868, in the direction the write door does not take.
///
/// `Ann ^n Marie` is the display name `caret.rs`'s own fixture table carries, beside the
/// meaning it gives those octets. A caller who means the four characters is handed no way to
/// say so: `Parameter::create` writes what it is given, and this crate's reading of what it
/// wrote is a newline.
#[test]
fn rfc6868_2_a_caret_handed_to_the_write_door_reads_back_as_an_encoding() {
    const MEANT: &[u8] = b"Ann ^n Marie";

    let parameter = Parameter::create(b"CN", MEANT)
        .unwrap_or_else(|error| panic!("no DQUOTE and no control character: {error:?}"));
    let organizer = Property::create(b"ORGANIZER", vec![parameter], b"mailto:ann@example.test")
        .unwrap_or_else(|error| panic!("ORGANIZER: {error:?}"));
    let uid = Property::create(b"UID", Vec::new(), b"1@example.test")
        .unwrap_or_else(|error| panic!("UID: {error:?}"));
    let event = Component::create(
        b"VEVENT",
        vec![Item::Property(uid), Item::Property(organizer)],
    )
    .unwrap_or_else(|error| panic!("VEVENT: {error:?}"));

    let octets = Document::new(vec![Item::Component(event)]).to_bytes();
    assert_ne!(
        octets,
        fixture("authored_caret_parameter.ics"),
        "the fixture is what the door used to write, which is the octets under attack"
    );

    let (reread, reported) = read(&octets);
    assert_eq!(reread.to_bytes(), octets, "P1 holds over what was authored");
    assert!(reported.is_empty(), "nothing is reported: {reported:?}");

    // What the door used to write, still read as RFC 6868 reads it: the caller's four
    // characters arriving as a newline is the defect, and the fixture keeps the evidence.
    let (was_written, said) = read(&fixture("authored_caret_parameter.ics"));
    assert!(said.is_empty(), "and it earned nothing either: {said:?}");
    assert_eq!(
        was_written
            .components()
            .flat_map(Component::properties)
            .find_map(|property| property.parameters_named(b"CN").next())
            .map(|held| decode_caret(held.unquoted()).into_owned()),
        Some(b"Ann \n Marie".to_vec()),
        "the octets the door used to write mean a newline to this crate's own codec"
    );

    let held = reread
        .components()
        .flat_map(Component::properties)
        .find_map(|property| property.parameters_named(b"CN").next())
        .unwrap_or_else(|| panic!("the CN this crate wrote is on the line it wrote"));
    assert_eq!(
        decode_caret(held.unquoted()).as_ref(),
        MEANT,
        "what the write door was handed is not what this crate's own codec reads back"
    );
}

/// The `DQUOTE` half of the same encoding, through the same door.
///
/// The door takes the value and picks the spelling, which is what it already did for quoting,
/// so a caller states `Doe, "Jack"` and gets it back. A caller that spelled the value itself
/// would be encoding text that is already encoded — `^'` would be written `^^'` — which is why
/// the contract is stated on the value rather than left to whoever calls first.
#[test]
fn rfc6868_2_a_dquote_handed_to_the_write_door_reads_back_as_a_dquote() {
    const MEANT: &[u8] = b"Doe, \"Jack\"";

    assert_eq!(
        encode_caret(MEANT).as_ref(),
        b"Doe, ^'Jack^'",
        "the spelling RFC 6868 gives, which is what the door now writes for the caller"
    );

    let parameter = Parameter::create(b"CN", MEANT)
        .unwrap_or_else(|error| panic!("the door spells what it is handed: {error:?}"));
    let organizer = Property::create(b"ORGANIZER", vec![parameter], b"mailto:jack@example.test")
        .unwrap_or_else(|error| panic!("ORGANIZER: {error:?}"));

    let octets = Document::new(vec![Item::Property(organizer)]).to_bytes();
    let (reread, reported) = read(&octets);
    assert_eq!(reread.to_bytes(), octets, "P1 over the spelled form");
    assert!(
        reported.is_empty(),
        "a spelled pair earns nothing: {reported:?}"
    );

    let held = reread
        .items()
        .iter()
        .filter_map(Item::as_property)
        .find_map(|property| property.parameters_named(b"CN").next())
        .unwrap_or_else(|| panic!("the CN is on the line this crate wrote"));
    assert_eq!(decode_caret(held.unquoted()).as_ref(), MEANT);
}

/// The traversal debt at the ceiling its own policy field can be raised to.
///
/// The debt named twenty thousand. `max_component_depth` is a `u16`, so `u16::MAX` is the
/// deepest tree a file can carry a caller to, and every one of the five traversals that used to
/// recurse is exercised there. The test asserts the whole of its claim by returning: a stack
/// overflow is an abort, so an assertion after one of these calls is not what reports it.
#[test]
fn every_derived_traversal_of_a_sixty_five_thousand_deep_document_returns() {
    let octets = ladder_of(usize::from(u16::MAX));
    let limits = Limits::DEFAULT.with_max_component_depth(u16::MAX);
    let mut kept: Vec<Diagnostic> = Vec::new();
    let document = Document::parse(&octets, limits, &mut kept)
        .unwrap_or_else(|error| panic!("the raised policy accepted the depth: {error:?}"));
    assert!(kept.is_empty(), "a conforming calendar earns no diagnostic");

    let copy = document.clone();
    assert_eq!(
        copy, document,
        "a copy differs from what it was copied from"
    );
    assert_eq!(copy.cmp(&document), Ordering::Equal);
    assert_eq!(hashed(&copy), hashed(&document), "equal, hashed unequally");
    assert!(!format!("{document:?}").is_empty());
    assert_eq!(copy.to_bytes(), octets, "P1 at the depth ceiling");

    drop(copy);
    drop(document);
}

/// Past the ceiling a file can reach, through the door that has no ceiling.
///
/// `Component::create` takes a tree and states no depth, so a caller assembles as deep a
/// document as it has memory for and nothing consults `max_component_depth` on the way. A
/// quarter of a million is twelve times what the debt measured.
#[test]
fn a_tree_built_far_past_the_policy_ceiling_is_copied_compared_and_dropped() {
    const DEPTH: u32 = 250_000;

    let mut nested = Component::create(b"X", Vec::new())
        .unwrap_or_else(|error| panic!("the innermost component: {error:?}"));
    for _ in 0..DEPTH {
        nested = Component::create(b"X", vec![Item::Component(nested)])
            .unwrap_or_else(|error| panic!("one more level: {error:?}"));
    }
    let document = Document::new(vec![Item::Component(nested)]);

    let copy = document.clone();
    assert_eq!(
        copy, document,
        "a copy differs from what it was copied from"
    );
    assert_eq!(copy.cmp(&document), Ordering::Equal);
    assert_eq!(hashed(&copy), hashed(&document), "equal, hashed unequally");
    assert!(!copy.to_bytes().is_empty());

    drop(copy);
    drop(document);
}
