// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The round-trip claim of `docs/adr/0001`, attacked from RFC 5545 section 3.1 and section 3.2.
//!
//! Four properties, separable and asserted separately, because byte identity alone is the
//! weakest of them. P1 is `serialize(parse(x)) == x`. P2 is that the octets `serialize` wrote
//! are themselves a fixed point, which is what catches a reader that normalizes on the first
//! pass and then disagrees with itself. P3 is `docs/adr/0001`'s mutation boundary: a change
//! scoped to one property reaches no other property. P4 is that an input carrying a violation
//! satisfies P1 anyway, so that "diagnosed" and "preserved" stay two claims rather than one.
//!
//! The fixtures are the places the content-line grammar is easiest to get wrong: a fold that
//! splits a multi-octet codepoint, an `HTAB` continuation, a file terminated with bare `LF`
//! and one that mixes all three terminators, a last line with none, a quoted parameter value
//! carrying `:` `;` and `,`, a parameter written twice, RFC 6868's caret escapes including one
//! the specification never defined, both spellings of an escaped line feed, a physical line of
//! exactly 75 octets beside one of 76, and control characters where section 3.1 permits them
//! and where it does not.
//!
//! Three of the assertions below were once failures, and each was the same shape: a write that
//! named one property and, once the document it produced was read back, had produced a
//! different one. Two of them are now refusals — section 3.2 has no spelling for the octets
//! they carried — and the third writes the terminator section 3.1 requires of a line that has
//! stopped being the last one. Each case still names what it caught, so a regression reads as
//! the finding it was rather than as an unexplained assertion.

use icalkit_conformance::internal::core::{
    Component, Diagnostic, Document, Item, Limits, MutationError, ParameterEdit, ParseError,
    PropertyId, ProposedChange, RawText, TextValue,
};

/// The calendar a scoped write is exercised against, and the one line each write names.
const MUTATION_SCOPE: &[u8] = include_bytes!("fixtures/break_grammar/mutation_scope.ics");

/// The line inside [`MUTATION_SCOPE`] that every P3 assertion names.
const SCOPED_LINE: &[u8] = b"SUMMARY;X-STATE=old:Lunch\r\n";

/// A calendar whose last line carries no terminator, which is a shape a producer really emits.
const UNTERMINATED_TAIL: &[u8] =
    include_bytes!("fixtures/break_grammar/mutation_add_after_unterminated.ics");

/// Every fixture in this directory, named beside its octets.
///
/// Read at compile time rather than from the filesystem, because `.gitattributes` marks these
/// files `-text` so that the octets committed are the octets on disk, and a test that read them
/// through a path could still be handed a working tree some tool had normalized.
const FIXTURES: &[(&str, &[u8])] = &[
    (
        "fold_inside_utf8",
        include_bytes!("fixtures/break_grammar/fold_inside_utf8.ics"),
    ),
    (
        "fold_htab_continuation",
        include_bytes!("fixtures/break_grammar/fold_htab_continuation.ics"),
    ),
    (
        "terminators_lf_only",
        include_bytes!("fixtures/break_grammar/terminators_lf_only.ics"),
    ),
    (
        "terminators_mixed",
        include_bytes!("fixtures/break_grammar/terminators_mixed.ics"),
    ),
    (
        "no_final_terminator",
        include_bytes!("fixtures/break_grammar/no_final_terminator.ics"),
    ),
    (
        "parameter_grammar",
        include_bytes!("fixtures/break_grammar/parameter_grammar.ics"),
    ),
    (
        "rfc6868_carets",
        include_bytes!("fixtures/break_grammar/rfc6868_carets.ics"),
    ),
    (
        "text_escapes",
        include_bytes!("fixtures/break_grammar/text_escapes.ics"),
    ),
    (
        "names_and_empty_values",
        include_bytes!("fixtures/break_grammar/names_and_empty_values.ics"),
    ),
    (
        "line_widths_75_and_76",
        include_bytes!("fixtures/break_grammar/line_widths_75_and_76.ics"),
    ),
    (
        "control_characters",
        include_bytes!("fixtures/break_grammar/control_characters.ics"),
    ),
    ("mutation_scope", MUTATION_SCOPE),
    ("mutation_add_after_unterminated", UNTERMINATED_TAIL),
];

/// Parse `input` under the default policy and write it back, keeping what was diagnosed.
///
/// The refusal is carried rather than unwrapped so that a fixture crossing a bound fails an
/// assertion naming the bound instead of panicking inside this helper.
fn read(input: &[u8]) -> (Result<Vec<u8>, ParseError>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let written =
        Document::parse(input, Limits::DEFAULT, &mut diagnostics).map(|tree| tree.to_bytes());
    (written, diagnostics)
}

/// Parse `input` under the default policy, or hand back an empty document.
///
/// Total rather than fallible, because every fixture here is inside the default bounds and
/// [`p1_every_fixture_is_written_back_octet_for_octet`] is the assertion that says so. A
/// refusal that slipped past it surfaces as an assertion about a document with nothing in it,
/// which names the fixture, rather than as a panic inside a helper, which does not.
fn tree_of(input: &[u8]) -> Document {
    let mut diagnostics = Vec::new();
    Document::parse(input, Limits::DEFAULT, &mut diagnostics).unwrap_or_default()
}

/// The first `VEVENT` inside the first calendar, which is where every scoped write lands.
fn subject(document: &mut Document) -> Option<&mut Component> {
    document
        .components_mut()
        .flat_map(Component::components_mut)
        .find(|nested| nested.is_named(b"VEVENT"))
}

/// The octets before `line` in `whole`, and the octets after it.
///
/// This is what "outside that one property" means for P3: a fixture states its target as a
/// whole physical line, so the two ends are exactly the octets a scoped write may not touch.
fn split_around<'a>(whole: &'a [u8], line: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let at = whole
        .windows(line.len())
        .position(|window| window == line)?;
    let after = at.saturating_add(line.len());
    Some((whole.get(..at)?, whole.get(after..)?))
}

/// Every property and component name in document order, so two trees can be compared by shape.
fn shape(document: &Document) -> Vec<String> {
    let mut names = Vec::new();
    walk(document.items(), &mut names);
    names
}

/// Append the names under `items`, depth first, marking which kind each one was.
fn walk(items: &[Item], names: &mut Vec<String>) {
    for entry in items {
        match entry {
            Item::Property(property) => {
                names.push(format!("property {}", text(property.name().as_bytes())));
            },
            Item::Component(nested) => {
                names.push(format!("component {}", text(nested.name().as_bytes())));
                walk(nested.items(), names);
            },
        }
    }
}

/// Octets as something an assertion message can print, without demanding they be text.
fn text(octets: &[u8]) -> String {
    String::from_utf8_lossy(octets).into_owned()
}

/// The value text of the first property named `name` directly inside `component`.
///
/// `None` for a property that is not there, which is one of the outcomes under attack: a write
/// that merged two lines leaves the second one absent rather than wrong.
fn value_of(component: &Component, name: &[u8]) -> Option<Vec<u8>> {
    component
        .properties()
        .find(|property| property.is_named(name))
        .map(|property| property.value_text().as_bytes().to_vec())
}

#[test]
fn p1_every_fixture_is_written_back_octet_for_octet() {
    for (name, octets) in FIXTURES {
        let (written, _) = read(octets);
        assert_eq!(written.as_deref(), Ok(*octets), "{name}");
    }
}

#[test]
fn p2_what_a_parse_wrote_is_a_fixed_point_of_parsing_it_again() {
    for (name, octets) in FIXTURES {
        let once = read(octets).0.expect("a fixture within the bounds");
        let twice = read(&once).0.expect("what this crate wrote is readable");
        assert_eq!(twice, once, "{name}");
    }
}

#[test]
fn p4_a_fixture_that_earns_a_diagnostic_still_round_trips() {
    let mut diagnosed = 0_usize;
    for (name, octets) in FIXTURES {
        let (written, notes) = read(octets);
        if notes.is_empty() {
            continue;
        }
        diagnosed = diagnosed.saturating_add(1);
        assert_eq!(
            written.as_deref(),
            Ok(*octets),
            "{name} is diagnosed and must still be preserved"
        );
    }
    assert!(
        diagnosed >= 4,
        "the corpus has to actually reach the recovery paths, and reached {diagnosed}"
    );
}

#[test]
fn p3_a_value_write_leaves_every_line_around_it_alone() {
    let (before, after) =
        split_around(MUTATION_SCOPE, SCOPED_LINE).expect("the fixture carries the scoped line");
    let mut document = tree_of(MUTATION_SCOPE);
    {
        let event = subject(&mut document).expect("the fixture carries one VEVENT");
        let mut guard = event
            .get_mut::<TextValue<'_>>(&PropertyId::SUMMARY)
            .expect("the fixture carries a SUMMARY");
        guard.set_raw(b"Dinner").expect("a value with no control");
    }
    let written = document.to_bytes();
    assert!(
        written.starts_with(before),
        "the folded UID above the edit moved:\n  {}",
        text(&written)
    );
    assert!(
        written.ends_with(after),
        "the vendor line and its bare LF below the edit moved:\n  {}",
        text(&written)
    );
}

/// A parameter assignment carrying a terminator becomes a second content line.
///
/// `set_raw` refuses every control character precisely so that a value taken from a web form
/// cannot arrive as a second `ATTENDEE`. Nothing refused the same octets on the parameter side,
/// so the same injection traveled through `ProposedChange::SetParameters` — which is the shape
/// `ical-itip` applies an off-the-wire transition with — and the serialized line came back on
/// the next read as a `SUMMARY` that had lost its value beside an `ATTENDEE` nobody named.
///
/// RFC 5545 section 3.2 has no spelling for those octets: `QSAFE-CHAR` excludes a control
/// character, and no escape section 3.2 defines would bring one back. So the write is refused,
/// exactly as `set_raw` refuses it, and the case asserts both halves — the refusal, and that a
/// refused change leaves the file it named octet for octet as it was.
#[test]
fn p3_a_parameter_write_does_not_add_a_property_the_caller_never_named() {
    let expected = shape(&tree_of(MUTATION_SCOPE));
    let mut document = tree_of(MUTATION_SCOPE);
    let injection: &[u8] = b"busy\r\nATTENDEE:mailto:eve@example.test";
    let change = ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", injection)]);
    let outcome = subject(&mut document)
        .expect("the fixture carries one VEVENT")
        .apply(&PropertyId::SUMMARY, &change, Limits::DEFAULT);
    assert_eq!(
        outcome,
        Err(MutationError::NotRepresentable),
        "octets section 3.2 cannot write are refused rather than written"
    );

    let written = document.to_bytes();
    assert_eq!(
        shape(&tree_of(&written)),
        expected,
        "a write naming SUMMARY put a property beside it:\n  {}",
        text(&written)
    );
    assert_eq!(
        written, MUTATION_SCOPE,
        "a refused change writes nothing at all"
    );
}

/// A parameter assignment carrying a `:` moves the value the same change promised not to touch.
///
/// `SetParameters` exists so a `RANGE` edit does not discard the value's preserved text. RFC
/// 5545 section 3.2 requires a parameter value carrying `:` `;` or `,` to be written inside a
/// `DQUOTE` pair; unquoted, the `:` ends the header, and on the next read the value belongs to
/// somewhere else.
#[test]
fn p3_a_parameter_write_leaves_the_value_it_promised_not_to_touch() {
    let mut document = tree_of(MUTATION_SCOPE);
    let change = ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", b"a:b")]);
    subject(&mut document)
        .expect("the fixture carries one VEVENT")
        .apply(&PropertyId::SUMMARY, &change, Limits::DEFAULT)
        .expect("the change applies");

    let written = document.to_bytes();
    let mut reread = tree_of(&written);
    let event = subject(&mut reread).expect("the written calendar still carries one VEVENT");
    assert_eq!(
        value_of(event, b"SUMMARY").as_deref(),
        Some(&b"Lunch"[..]),
        "the value moved to the parameter side:\n  {}",
        text(&written)
    );
}

/// An addition after a line that carried no terminator runs into it.
///
/// The last line of a real export often ends without `CRLF`, and an inserted property is
/// written straight after it, so the two become one content line. On the next read the addition
/// is gone and the property above it has swallowed its octets.
#[test]
fn p3_an_addition_does_not_merge_into_the_line_before_it() {
    let mut document = tree_of(UNTERMINATED_TAIL);
    let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
    subject(&mut document)
        .expect("the fixture carries one VEVENT")
        .apply(&PropertyId::COMMENT, &change, Limits::DEFAULT)
        .expect("the change applies");

    let written = document.to_bytes();
    let mut reread = tree_of(&written);
    let event = subject(&mut reread).expect("the written calendar still carries one VEVENT");
    assert_eq!(
        value_of(event, b"SUMMARY").as_deref(),
        Some(&b"Lunch"[..]),
        "the line above absorbed the addition:\n  {}",
        text(&written)
    );
    assert!(
        event.properties().any(|held| held.is_named(b"COMMENT")),
        "the addition is not a property of its own:\n  {}",
        text(&written)
    );
}
