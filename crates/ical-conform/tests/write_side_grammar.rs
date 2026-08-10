// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The grammar rules a *write* has to obey, addressed to the sections they come from.
//!
//! Every other case in this crate reads octets somebody else produced and asserts they come
//! back unchanged. These are the mirror image: this crate is the producer, and the question is
//! whether what it wrote is a content line at all. It is a separate file because the failure
//! mode is different in kind. A read that gets a rule wrong loses fidelity and the corpus says
//! so; a write that gets one wrong emits a file whose *shape* disagrees with the tree it came
//! from, and the damage lands in whichever client opens it next.
//!
//! Three rules, each with an RFC section behind it, each added because a scoped write reached
//! past the property it named:
//!
//! - **Section 3.2, `SAFE-CHAR` and `QSAFE-CHAR`.** A parameter value carrying `:` `;` or `,`
//!   is written inside a `DQUOTE` pair, because those three are excluded from an unquoted
//!   value and each of them ends something. One carrying a `DQUOTE` or a control character is
//!   refused, because `QSAFE-CHAR` excludes both and section 3.2 defines no escape that would
//!   return them: there is no spelling to pick.
//! - **Section 3.2, `param-name`.** A parameter name carrying a delimiter is refused for the
//!   same reason, one level up.
//! - **Section 3.1, content lines.** A line written after another line is a second content
//!   line only if the first one ends. A final line often arrives with no terminator and is
//!   written back with none; the moment something is added after it, section 3.1's `CRLF` is
//!   what makes the two of them two lines.
//!
//! Where the specification permits a choice, both permitted outcomes are recorded here rather
//! than one becoming the answer because it was written first (`docs/adr/0006`).

use ical_core::{
    Component, Diagnostic, Document, Item, Limits, MutationError, ParameterEdit, ParseError,
    Property, PropertyId, ProposedChange, RawText,
};

/// A calendar with one decorated `SUMMARY`, terminated as section 3.1 asks.
const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\r\n\
    BEGIN:VEVENT\r\n\
    UID:1@example.test\r\n\
    SUMMARY;X-STATE=old:Lunch\r\n\
    END:VEVENT\r\n\
    END:VCALENDAR\r\n";

/// The same calendar with its last line cut short, which is a shape producers really emit.
const UNTERMINATED: &[u8] = b"BEGIN:VCALENDAR\r\n\
    BEGIN:VEVENT\r\n\
    UID:1@example.test\r\n\
    SUMMARY:Lunch";

/// Parse under the default policy, or hand back an empty document.
///
/// Total rather than fallible: every input here is well inside the default bounds, and a
/// refusal that slipped past that surfaces as an assertion about a document with nothing in
/// it, which names the case, rather than as a panic inside a helper, which does not.
fn tree_of(input: &[u8]) -> Document {
    let mut kept: Vec<Diagnostic> = Vec::new();
    Document::parse(input, Limits::DEFAULT, &mut kept).unwrap_or_default()
}

/// The first `VEVENT` inside the first calendar, which is where every write below lands.
fn subject(document: &mut Document) -> Option<&mut Component> {
    document
        .components_mut()
        .flat_map(Component::components_mut)
        .find(|nested| nested.is_named(b"VEVENT"))
}

/// Apply `change` to `SUMMARY` in the calendar `input`, and hand back what was written.
fn write_summary(input: &[u8], change: &ProposedChange) -> (Result<(), MutationError>, Vec<u8>) {
    let mut document = tree_of(input);
    let outcome = subject(&mut document).map_or(Err(MutationError::Absent), |event| {
        event.apply(&PropertyId::SUMMARY, change, Limits::DEFAULT)
    });
    let written = document.to_bytes();
    (outcome, written)
}

/// The value text of the first property named `name` directly inside `component`.
fn value_of(component: &Component, name: &[u8]) -> Option<Vec<u8>> {
    component
        .properties()
        .find(|property| property.is_named(name))
        .map(|property| property.value_text().as_bytes().to_vec())
}

/// Every parameter of the first property named `name`, as it was written.
fn parameters_of(component: &Component, name: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    component
        .properties()
        .find(|property| property.is_named(name))
        .map(Property::parameters)
        .unwrap_or_default()
        .iter()
        .map(|held| {
            (
                held.name().as_bytes().to_vec(),
                held.value().as_bytes().to_vec(),
            )
        })
        .collect()
}

/// RFC 5545 section 3.2: `SAFE-CHAR` excludes `:` `;` and `,` from an unquoted parameter
/// value, and `QSAFE-CHAR` admits all three.
///
/// So a written value carrying one of them goes inside a `DQUOTE` pair, and comes back on the
/// next read as the value that was assigned rather than as a truncated one with the rest of
/// the line rearranged around it.
#[test]
fn rfc5545_3_2_a_written_parameter_value_is_quoted_where_the_grammar_forces_it() {
    // The value assigned, and the octets section 3.2 spells it as on the line.
    let cases: &[(&[u8], &[u8])] = &[
        (b"a:b", b"\"a:b\""),
        (b"a;b", b"\"a;b\""),
        (b"a,b", b"\"a,b\""),
        // Nothing section 3.2 excludes, so nothing is added: quoting a value that did not
        // need it would put octets on the wire the caller never asked for.
        (b"busy", b"busy"),
        (b"W. Europe Standard Time", b"W. Europe Standard Time"),
    ];
    for (assigned, spelled) in cases {
        let change = ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", assigned)]);
        let (outcome, written) = write_summary(CALENDAR, &change);
        assert_eq!(outcome, Ok(()), "{assigned:?}");

        let mut line = Vec::from(&b"SUMMARY;X-STATE="[..]);
        line.extend_from_slice(spelled);
        line.extend_from_slice(b":Lunch\r\n");
        assert!(
            written.windows(line.len()).any(|window| window == line),
            "{assigned:?} was not written as {}:\n  {}",
            String::from_utf8_lossy(&line),
            String::from_utf8_lossy(&written)
        );

        // The half a quoting rule is actually for: the assignment survives the round trip,
        // and so does the value text the change promised not to touch.
        let mut reread = tree_of(&written);
        let event = subject(&mut reread).expect("the written calendar still carries one VEVENT");
        assert_eq!(value_of(event, b"SUMMARY").as_deref(), Some(&b"Lunch"[..]));
        assert_eq!(
            parameters_of(event, b"SUMMARY"),
            vec![(b"X-STATE".to_vec(), spelled.to_vec())],
            "{assigned:?}"
        );
    }
}

/// RFC 5545 section 3.2: `QSAFE-CHAR` excludes `DQUOTE` and every `CONTROL` octet, and section
/// 3.2 defines no escape that would bring either back.
///
/// **Where implementations differ.** RFC 6868 defines a caret encoding — `^n` for a newline,
/// `^^` for a caret, `^'` for a `DQUOTE` — which is exactly the missing spelling, and clients
/// that implement it write these values rather than refusing them. `ical-grammar` now reads
/// and writes that encoding in both directions (`decode_caret`/`encode_caret`), but it is a
/// codec a caller opts into rather than a storage rule: a `^'` read out of a file stays the
/// two octets it arrived as, and the mutation door still refuses a `DQUOTE` or a control
/// octet, because `parameter_is_representable` has not been taught to consult `encode_caret`.
/// Both outcomes are permitted, and the two are recorded here:
///
/// - an RFC 6868 implementation writes `X-STATE=^'hi^'` and reads back `"hi"`;
/// - this crate refuses, with `MutationError::NotRepresentable`, and writes nothing.
///
/// Adopting the first is a decision about what `Component::apply` emits — the reading half no
/// longer stands in the way — and it is the write path's own unit to make, not this case's.
#[test]
fn rfc5545_3_2_a_parameter_value_with_no_spelling_is_refused_rather_than_written() {
    let refused: &[&[u8]] = &[
        b"say \"hi\"",
        b"busy\r\nATTENDEE:mailto:eve@example.test",
        b"busy\nmore",
        b"bell\x07",
        b"delete\x7f",
    ];
    for assigned in refused {
        let change = ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", assigned)]);
        let (outcome, written) = write_summary(CALENDAR, &change);
        assert_eq!(
            outcome,
            Err(MutationError::NotRepresentable),
            "{assigned:?} was written rather than refused"
        );
        assert_eq!(written, CALENDAR, "a refused change wrote octets");
    }
}

/// RFC 5545 section 3.2: `param-name` is `iana-token / x-name`, and every octet that ends a
/// name hands the rest of it to something else.
///
/// The refusal here is narrower than the ABNF on purpose, and the case records why. Producers
/// write `_` and `.` in vendor parameter names, this crate reads them back unchanged, and
/// refusing to write a name that survives a round trip would be this crate disagreeing with
/// itself. What is refused is the mechanical claim instead: a name the reader would hand back
/// in pieces, or no name at all.
#[test]
fn rfc5545_3_2_a_parameter_name_that_would_not_read_back_whole_is_refused() {
    let refused: &[&[u8]] = &[b"X-A:B", b"X-A;B", b"X-A=B", b"X-A,B", b"X-A\"B", b""];
    for spelling in refused {
        let change = ProposedChange::SetParameters(vec![ParameterEdit::set(spelling, b"busy")]);
        let (outcome, written) = write_summary(CALENDAR, &change);
        assert_eq!(
            outcome,
            Err(MutationError::NotRepresentable),
            "{spelling:?} was written rather than refused"
        );
        assert_eq!(written, CALENDAR, "a refused change wrote octets");
    }

    // The vendor spellings that do come back are written, because they do come back.
    let accepted =
        ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-VENDOR_FLAG.2", b"busy")]);
    let (outcome, written) = write_summary(CALENDAR, &accepted);
    assert_eq!(outcome, Ok(()));
    let mut reread = tree_of(&written);
    let event = subject(&mut reread).expect("the written calendar still carries one VEVENT");
    assert_eq!(
        parameters_of(event, b"SUMMARY"),
        vec![
            // The parameter the fixture already carried, which the edit did not name.
            (b"X-STATE".to_vec(), b"old".to_vec()),
            (b"X-VENDOR_FLAG.2".to_vec(), b"busy".to_vec()),
        ]
    );
}

/// RFC 5545 section 3.1: content lines are delimited by `CRLF`, so a line with something
/// written after it needs one.
///
/// A final line that arrives without a terminator is written back without one, because adding
/// an octet the file did not have is the same class of change as dropping one. That reasoning
/// holds for exactly as long as the line is last. Two content lines with nothing between them
/// are one content line: the addition would not exist on the next read, and the property above
/// it would come back carrying the addition's octets in its value.
///
/// **Where implementations differ.** The specification does not say what an editor does to an
/// unterminated final line when it appends after it, and both answers keep a file readable:
///
/// - refuse the addition, leaving the caller to terminate the line first;
/// - terminate the line, which is what this crate does, because a refusal here would make
///   "this calendar has an addition" depend on a property of the *file* that the caller did
///   not choose and mostly cannot see.
///
/// The octet written is the only one an addition puts outside the line it added, and the line
/// above keeps its name, its parameters, its value and its position.
#[test]
fn rfc5545_3_1_an_addition_terminates_the_line_that_stopped_being_last() {
    let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
    let (outcome, written) = write_summary_add(UNTERMINATED, &change);
    assert_eq!(outcome, Ok(()));
    assert_eq!(
        written,
        &b"BEGIN:VCALENDAR\r\n\
           BEGIN:VEVENT\r\n\
           UID:1@example.test\r\n\
           SUMMARY:Lunch\r\n\
           COMMENT:added\r\n"[..],
        "the line above gained the delimiter that makes it a line"
    );

    let mut reread = tree_of(&written);
    let event = subject(&mut reread).expect("the written calendar still carries one VEVENT");
    assert_eq!(
        value_of(event, b"SUMMARY").as_deref(),
        Some(&b"Lunch"[..]),
        "the line above kept its value"
    );
    assert_eq!(
        value_of(event, b"COMMENT").as_deref(),
        Some(&b"added"[..]),
        "and the addition is a property of its own"
    );
}

/// A terminator that was already there is left as it was, bare `LF` included.
///
/// Section 3.1 requires `CRLF` and a great many producers write a bare `LF` anyway. Which one
/// arrived is reported as `DiagnosticCode::BareLineFeed` and written back unchanged; an
/// addition elsewhere in the component is not an occasion to correct it.
#[test]
fn rfc5545_3_1_an_addition_corrects_no_terminator_that_was_already_there() {
    let bare_feeds: &[u8] = b"BEGIN:VCALENDAR\n\
        BEGIN:VEVENT\n\
        UID:1@example.test\n\
        SUMMARY:Lunch\n";
    let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
    let (outcome, written) = write_summary_add(bare_feeds, &change);
    assert_eq!(outcome, Ok(()));
    assert_eq!(
        written,
        &b"BEGIN:VCALENDAR\n\
           BEGIN:VEVENT\n\
           UID:1@example.test\n\
           SUMMARY:Lunch\n\
           COMMENT:added\r\n"[..],
        "every bare LF survived an addition that named none of them"
    );
}

/// Apply `change` to `COMMENT` in the calendar `input`, and hand back what was written.
///
/// A second helper rather than a parameter on the first, because an addition names the
/// identity it is adding and the two calls would otherwise differ only in a `PropertyId` a
/// reader has to trace back to work out which case is which.
fn write_summary_add(
    input: &[u8],
    change: &ProposedChange,
) -> (Result<(), MutationError>, Vec<u8>) {
    let mut document = tree_of(input);
    let outcome = subject(&mut document).map_or(Err(MutationError::Absent), |event| {
        event.apply(&PropertyId::COMMENT, change, Limits::DEFAULT)
    });
    let written = document.to_bytes();
    (outcome, written)
}

/// RFC 5545 section 3.1: a line may be folded across as many continuation lines as a producer
/// likes, and this crate states a ceiling on how many it will keep.
///
/// Section 3.1 imposes no bound, and it is right not to: the format has no length field and
/// nothing about a legitimate calendar cares how many times a value was broken. A reader does,
/// because it keeps one `FoldPoint` per fold so the writer can put the fold back, and a line of
/// nothing but continuations is one item, one octet of value and no header — which is to say
/// that no other bound in `Limits` counts it. So the ceiling is a caller-stated policy rather
/// than a rule read out of the specification (`docs/adr/0010`), and this case pins down where
/// the default sits and that it is exact.
///
/// The default is derived from the two numbers beside it: a value at `max_value_bytes` folded
/// at the seventy-five octets section 3.1 asks for needs about fourteen thousand continuations,
/// so sixteen thousand three hundred and eighty-four accepts every legitimate line and refuses
/// a line whose continuations outnumber its content.
#[test]
fn rfc5545_3_1_a_line_may_be_folded_as_far_as_the_stated_policy_and_no_further() {
    let limits = Limits::DEFAULT.with_grammar(Limits::DEFAULT.grammar().with_max_folds_per_line(4));

    // Exactly the bound: read whole, and written back fold for fold.
    let at_the_bound: &[u8] = b"X:a\r\n b\r\n c\r\n d\r\n e\r\n";
    let mut kept: Vec<Diagnostic> = Vec::new();
    let read = Document::parse(at_the_bound, limits, &mut kept);
    assert_eq!(
        read.map(|document| document.to_bytes()).as_deref(),
        Ok(at_the_bound),
        "the widest line the policy allows is preserved rather than merely accepted"
    );

    // One fold past it: refused as a whole document, never truncated to what fitted.
    let past_the_bound: &[u8] = b"X:a\r\n b\r\n c\r\n d\r\n e\r\n f\r\n";
    let mut ignored: Vec<Diagnostic> = Vec::new();
    assert_eq!(
        Document::parse(past_the_bound, limits, &mut ignored),
        Err(ParseError::TooManyFolds { limit: 4 })
    );

    // And the default is the number the policy documents, tested where a caller meets it.
    assert_eq!(Limits::DEFAULT.grammar().max_folds_per_line(), 16_384);
}

/// RFC 5545 section 3.1 again, from the other side: a line the crate did not author keeps the
/// terminator it arrived with, and an addition into a component that has no property yet lands
/// after the `BEGIN` — which is then the line the same rule applies to.
#[test]
fn rfc5545_3_1_an_addition_with_no_property_above_it_terminates_the_begin_line() {
    let opened: &[u8] = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT";
    let change = ProposedChange::Add(RawText::from_bytes(b"COMMENT:added\r\n"));
    let (outcome, written) = write_summary_add(opened, &change);
    assert_eq!(outcome, Ok(()));
    assert_eq!(
        written,
        &b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nCOMMENT:added\r\n"[..]
    );

    // And the addition really is inside the event rather than beside it.
    let mut reread = tree_of(&written);
    let event = subject(&mut reread).expect("the written calendar still carries one VEVENT");
    assert!(
        event
            .items()
            .iter()
            .filter_map(Item::as_property)
            .any(|held| held.is_named(b"COMMENT"))
    );
}
