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
//! - **Section 3.2, `SAFE-CHAR` and `QSAFE-CHAR`, and RFC 6868 section 2.** A parameter value
//!   carrying `:` `;` or `,` is written inside a `DQUOTE` pair, because those three are
//!   excluded from an unquoted value and each of them ends something. One carrying a `DQUOTE`
//!   or a newline is written in the caret pair RFC 6868 gives it, and a `^` is written `^^` so
//!   that those pairs stay unambiguous. One carrying a control character neither of them
//!   spells is refused: there is no spelling to pick.
//! - **Section 3.2, `param-name`.** A parameter name carrying a delimiter is refused for the
//!   same reason, one level up.
//! - **Section 3.1, content lines.** A line written after another line is a second content
//!   line only if the first one ends. A final line often arrives with no terminator and is
//!   written back with none; the moment something is added after it, section 3.1's `CRLF` is
//!   what makes the two of them two lines.
//!
//! Where the specification permits a choice, both permitted outcomes are recorded here rather
//! than one becoming the answer because it was written first (`docs/adr/0006`).

use icalkit_conformance::internal::core::{
    Component, Diagnostic, Document, Item, Limits, MutationError, ParameterEdit, ParseError,
    Property, PropertyId, ProposedChange, RawText, TextValue, decode_caret,
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

/// The value one parameter of one property stands for: the quotes off and the carets resolved.
///
/// Both steps, in the order a reader undoes them, because that is what "the value this crate
/// wrote" means once the write door spells one.
fn decoded_parameter(component: &Component, property: &[u8], parameter: &[u8]) -> Option<Vec<u8>> {
    component
        .properties()
        .find(|held| held.is_named(property))?
        .parameters_named(parameter)
        .next()
        .map(|held| decode_caret(held.unquoted()).into_owned())
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

/// RFC 6868 section 2: the caret encoding supplies the two spellings section 3.2 lacks, and
/// requires a `^` to be written `^^` so that the first two stay unambiguous.
///
/// A write door takes a *value* and picks its spelling — that is what quoting already is — so
/// the encoding belongs to the same step. `say "hi"` is written `say ^'hi^'` and `Ann ^n Marie`
/// is written `Ann ^^n Marie`, and `decode_caret` reads each back as the octets that were
/// handed over. Leaving the caret alone was the defect this case now records the closing of:
/// the door wrote `Ann ^n Marie` verbatim and this crate's own codec read those octets as a
/// value carrying a newline, which is `caret.rs`'s "both directions or neither" failing against
/// itself.
///
/// **Where implementations differ.** RFC 6868 is an extension, and a consumer that has not
/// implemented it reads `^'` as two literal octets. Three outcomes are permitted for a value
/// carrying a `DQUOTE` and all three are in the wild:
///
/// - encode it, which is what this crate does and what RFC 6868 asks for;
/// - refuse it, since section 3.2 alone has no spelling — this crate's own earlier answer, and
///   the one that never emits an octet a non-6868 consumer misreads;
/// - write the `DQUOTE` bare, which produces a line whose value ends early on the next read.
///   That one is a defect rather than a choice, and no case here permits it.
///
/// What has no spelling under either reading is still refused: a `CR`, and every `CONTROL`
/// octet RFC 6868 gives no pair. That is the injection refusal, and it is unchanged — a
/// terminator inside a parameter value is how an assignment becomes a second `ATTENDEE`.
#[test]
fn rfc6868_2_a_parameter_value_is_written_in_the_spelling_the_encoding_gives_it() {
    // The value assigned, and the octets the two encodings together spell it as.
    let spelled: &[(&[u8], &[u8])] = &[
        (b"say \"hi\"", b"say ^'hi^'"),
        (b"Ann ^n Marie", b"Ann ^^n Marie"),
        (b"100^", b"100^^"),
        (b"busy\nmore", b"busy^nmore"),
        // Both encodings at once, and the quoting is decided after the carets are spelled.
        (b"Doe, \"Jack\"", b"\"Doe, ^'Jack^'\""),
    ];
    for (assigned, written_as) in spelled {
        let change = ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", assigned)]);
        let (outcome, written) = write_summary(CALENDAR, &change);
        assert_eq!(outcome, Ok(()), "{assigned:?}");

        let mut reread = tree_of(&written);
        let event = subject(&mut reread).expect("the written calendar still carries one VEVENT");
        assert_eq!(
            parameters_of(event, b"SUMMARY"),
            vec![(b"X-STATE".to_vec(), written_as.to_vec())],
            "{assigned:?}"
        );
        assert_eq!(
            decoded_parameter(event, b"SUMMARY", b"X-STATE").as_deref(),
            Some(*assigned),
            "{assigned:?} is not what this crate's own codec reads back"
        );
    }

    let refused: &[&[u8]] = &[
        b"busy\r\nATTENDEE:mailto:eve@example.test",
        b"busy\rmore",
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

/// RFC 5545 section 3.6: `BEGIN` and `END` delimit a component, so no write may author one.
///
/// The rule the two doors below apply is the same one section 3.1 gives every other name — "a
/// line the reader would not hand back as the thing that was stored" — read at the layer that
/// has a component model. `END` reads back as `END`, which is why the section 3.1 predicate
/// takes it and why the refusal has to be stated here instead: what the *line* reads back as is
/// a component boundary, and a caller that wrote one would be restructuring the file through a
/// door that names one property.
///
/// The reader keeps such a line — a mismatched `END` is section 3.6 recovery's own answer, and
/// the last case below is one — so this is a write-side refusal only, and it costs the
/// round-trip claim nothing.
///
/// **The alternative, and why not.** A door could accept the write and let the file restructure
/// itself, which is what this crate did until the case below was written: one addition moved
/// six of the twelve lines in `valarm_misplaced.ics` into a component nobody added, and the
/// reload reported `MismatchedEndName` and two `UnclosedComponent`s against a file the caller
/// had just saved. No specification forbids emitting those octets. What forbids it is that the
/// change vocabulary is addressed to one property, and this one is not a property.
#[test]
fn rfc5545_3_6_a_write_may_not_author_a_component_boundary() {
    let boundary = PropertyId::from_name(b"BEGIN");
    let changes = [
        ProposedChange::Add(RawText::from_bytes(b"BEGIN:VALARM\r\n")),
        ProposedChange::Replace(RawText::from_bytes(b"BEGIN:VALARM\r\n")),
        ProposedChange::SetParameters(vec![ParameterEdit::set(b"X-STATE", b"busy")]),
    ];
    for change in &changes {
        let mut document = tree_of(CALENDAR);
        let outcome = subject(&mut document).map_or(Err(MutationError::Absent), |event| {
            event.apply(&boundary, change, Limits::DEFAULT)
        });
        assert_eq!(
            outcome,
            Err(MutationError::ComponentBoundary),
            "{change:?} was written rather than refused"
        );
        assert_eq!(
            document.to_bytes(),
            CALENDAR,
            "a refused change wrote octets"
        );
    }

    // The same refusal through the value guard, over the property section 3.6 recovery keeps
    // for an `END` that named the wrong component.
    let recovered: &[u8] = b"BEGIN:VEVENT\r\nSUMMARY:Lunch\r\nEND:VTODO\r\n";
    let mut document = tree_of(recovered);
    let event = document
        .components_mut()
        .next()
        .expect("the recovery kept one component");
    assert_eq!(
        event
            .get_mut::<TextValue<'_>>(&PropertyId::from_name(b"END"))
            .expect("and kept the mismatched END as a property")
            .set_raw(b"VEVENT"),
        Err(MutationError::ComponentBoundary)
    );
    assert_eq!(document.to_bytes(), recovered);

    // What a caller wanting a component uses instead, which writes both lines itself.
    let built = Component::create(b"VALARM", Vec::new()).expect("a well-formed component");
    assert_eq!(
        Document::new(vec![Item::Component(built)]).to_bytes(),
        b"BEGIN:VALARM\r\nEND:VALARM\r\n"
    );
}

/// RFC 5545 section 3.1: a line whose first octet is `SP` or `HTAB` is a continuation, so a
/// property whose name begins with one is written below a fold of its own.
///
/// Section 3.1 folds at octets and says nothing about what they spell, so a fold at octet zero
/// followed by whitespace unfolds to a content line whose name starts with whitespace. That is
/// a property the reader really builds, and a write to it discards the recorded layout — which
/// is where the fold at octet zero was. Written flat, the line would rejoin the line above it
/// and the property would cease to exist; written below a fold, it is the same property it was.
#[test]
fn rfc5545_3_1_a_written_line_whose_name_begins_with_whitespace_keeps_its_own_line() {
    let folded: &[u8] = b"BEGIN:VCALENDAR\r\n\r\n \tSUMMARY:Lunch\r\nEND:VCALENDAR\r\n";
    let mut document = tree_of(folded);
    assert_eq!(document.to_bytes(), folded, "the fixture holds on its own");

    let calendar = document
        .components_mut()
        .next()
        .expect("the file opens one component");
    let identity = PropertyId::from_name(b"\tSUMMARY");
    assert_eq!(
        calendar
            .get_mut::<TextValue<'_>>(&identity)
            .expect("the property the fold at octet zero produced")
            .set_raw(b"written"),
        Ok(())
    );

    let written = document.to_bytes();
    assert_eq!(
        written, b"BEGIN:VCALENDAR\r\n\r\n \tSUMMARY:written\r\nEND:VCALENDAR\r\n",
        "the canonical refold opened the line with the fold its name needs"
    );

    // And the property is still one property, in the component it was in.
    let reread = tree_of(&written);
    assert_eq!(
        reread.to_bytes(),
        written,
        "what was written is a fixed point"
    );
    let held: Vec<Vec<u8>> = reread
        .components()
        .flat_map(Component::properties)
        .map(|property| property.name().as_bytes().to_vec())
        .collect();
    assert_eq!(held, vec![b"\tSUMMARY".to_vec()]);
}

/// RFC 5545 section 3.1: two content lines with nothing between them are one content line.
///
/// The mutation door writes the terminator a line owes when an addition lands after it, and the
/// case above records that. The serializer owes the same octet for a tree nobody added to
/// through that door: a property read out of a truncated export carries a layout with no
/// terminator, `Component::items_mut` will put it anywhere, and two lines stored with nothing
/// between them would be one line read back — the second line's octets glued to the first
/// one's value, with nothing reported.
///
/// The terminator is written between the two and not after the last, so a file that ended
/// without one still does.
#[test]
fn rfc5545_3_1_a_stored_line_that_stopped_being_last_is_written_as_a_line() {
    let mut document = tree_of(UNTERMINATED);
    let event = subject(&mut document).expect("the export ends inside a VEVENT");
    // A copy of the line the export was cut off in the middle of, which is the only way to get
    // a second property whose layout carries no terminator: `Property::create` writes one.
    let copied = event
        .properties()
        .last()
        .cloned()
        .expect("the event holds the truncated line");
    event.items_mut().push(Item::Property(copied));

    let written = document.to_bytes();
    assert_eq!(
        written,
        b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1@example.test\r\nSUMMARY:Lunch\r\n\
          SUMMARY:Lunch"
            .to_vec(),
        "the line above gained the terminator and the line that is last did not"
    );

    let reread = tree_of(&written);
    assert_eq!(
        reread.to_bytes(),
        written,
        "what was written is a fixed point"
    );
    let event = reread
        .components()
        .flat_map(Component::components)
        .find(|nested| nested.is_named(b"VEVENT"))
        .expect("the event survived");
    assert_eq!(
        event.properties().count(),
        3,
        "three lines were stored and the file has to hold three"
    );
}
