// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! RFC 5545 section 3.2 parameter values, and RFC 6868's caret encoding over them.
//!
//! Two readings this workspace made and never wrote down. The first is RFC 6868: `^n`, `^^`
//! and `^'` are the spellings section 3.2 lacks, and a receiver either understands them or
//! shows a user two octets it cannot explain. The second is section 3.2's own `DQUOTE` pair:
//! what a quote does depends on where it appears, and every implementation that has ever
//! shipped has had to decide what an unterminated one costs.
//!
//! Neither is a rule this crate may pick for the ecosystem. What it may do — what
//! `docs/adr/0006` asks of a case — is record every behavior the specifications permit, say
//! which one this project produces and why, and state the `Limits` policy the answer was
//! produced under, because an outcome that depends on a budget is not reproducible without
//! one. The `observed` half of each case below is what the implementations *document*: no
//! bridge has been run against a foreign subject, and refreshing this matrix against one is
//! what `ical-conform`'s bridge exists to do.
//!
//! **RFC 6868, and what each implementation does with it.** An implementation that
//! implements the encoding — libical since 3.0 and ical.js both decode these pairs inside a
//! parameter value — shows a user `"Doe, John"` for a `CN` written `"^'Doe, John^'"`. One
//! that does not shows the carets. Both readings leave the file alone, both are permitted,
//! and neither is corrected here. This project decodes when a caller asks and stores the
//! octets a producer wrote, which is the only answer compatible with `docs/adr/0001`: a
//! reading that rewrote storage would send back a line nobody sent.
//!
//! **Where the specification permits two spellings, both are recorded.** Section 3.2 lets a
//! producer quote a value that needed no quoting, so `CN=Ann` and `CN="Ann"` are two
//! spellings of one value; RFC 6868 forces no encoding on a value carrying none of its three
//! octets, so `CN=Ann` and `CN=^^` are not comparable but `CN=Paris` and nothing else is one
//! spelling. This file asserts that both spellings *read* alike and that each is written back
//! as it arrived. It canonizes neither.
//!
//! **What this file is bounded by.** A parameter value is header octets — the reader
//! reassembles the name and the parameters across folds into one scratch buffer — so a quoted
//! value is bounded by `GrammarLimits::max_header_bytes` and a property value is not. That
//! ceiling is the boundary these cases run against, and it is stated per case rather than
//! assumed (`docs/adr/0010`).
//!
//! Every fixture here is written back octet for octet under the stated policy, including the
//! two that earn a diagnostic. "Diagnosed" and "preserved" are two claims and both hold.

use icalkit_conformance::internal::core::{
    Diagnostic, DiagnosticCode, Document, Item, Limits, Parameter, ParameterQuoting, ParseError,
    Property, caret_needs_encoding, decode_caret, encode_caret, parameter_is_representable,
    parameter_name_is_representable, quote_parameter, undefined_caret_escapes, unescape_text,
    unquote_parameter,
};

/// The policy every case runs under unless it names another.
///
/// Named rather than left implicit: a header ceiling is what decides whether a long quoted
/// parameter is read at all, and a case that did not state one would be asserting against
/// whatever the default happened to be on the day it was written (`docs/adr/0010`).
const POLICY: Limits = Limits::DEFAULT;

/// RFC 6868's three pairs, an undefined one, a caret inside quotes and a caret at the end.
const CARET_ENCODING: &[u8] = include_bytes!("fixtures/break_parameters/caret_encoding.ics");

/// Section 3.2's `DQUOTE`, in each of the four positions that mean different things.
const QUOTED_VALUES: &[u8] = include_bytes!("fixtures/break_parameters/quoted_values.ics");

/// A quote opened on a last line that also carried no terminator.
const UNTERMINATED_TAIL: &[u8] =
    include_bytes!("fixtures/break_parameters/quoted_unterminated_tail.ics");

/// Every fixture in this directory, named beside its octets.
///
/// Read at compile time rather than from the filesystem, because `.gitattributes` marks these
/// files `-text` so that the octets committed are the octets on disk, and a test that read them
/// through a path could still be handed a working tree some tool had normalized.
const FIXTURES: &[(&str, &[u8])] = &[
    ("caret_encoding", CARET_ENCODING),
    ("quoted_values", QUOTED_VALUES),
    ("quoted_unterminated_tail", UNTERMINATED_TAIL),
];

/// One `CN` of `caret_encoding.ics`, addressed by the line it sits on.
///
/// The two spellings are kept apart on purpose. `stored` is what the file holds and what is
/// written back; `inner` is what is left once section 3.2's quotes come off, and it is what
/// RFC 6868 is defined over — the encoding is a reading of a parameter value, not of the
/// delimiters around it.
struct CaretCase {
    /// The value text of the content line this `CN` sits on, which is what names it.
    line: &'static [u8],
    /// The `CN` octets as stored, any `DQUOTE` pair included.
    stored: &'static [u8],
    /// What is left once that pair comes off.
    inner: &'static [u8],
    /// What RFC 6868 section 2 says those octets mean.
    meaning: &'static [u8],
    /// Whether a `^` in them begins a pair the specification gives no meaning.
    undefined: bool,
}

/// The seven readings `caret_encoding.ics` was written to pin down.
const CARET_CASES: &[CaretCase] = &[
    // `^n`: the newline section 3.2 cannot spell at all.
    CaretCase {
        line: b"mailto:a@ex.test",
        stored: b"Ann^nMarie",
        inner: b"Ann^nMarie",
        meaning: b"Ann\nMarie",
        undefined: false,
    },
    // `^^`: the escape hatch that keeps the encoding invertible.
    CaretCase {
        line: b"mailto:b@ex.test",
        stored: b"100^^ tested",
        inner: b"100^^ tested",
        meaning: b"100^ tested",
        undefined: false,
    },
    // `^'`: the `DQUOTE` section 3.2's `QSAFE-CHAR` excludes.
    CaretCase {
        line: b"mailto:c@ex.test",
        stored: b"^'Ann^'",
        inner: b"^'Ann^'",
        meaning: b"\"Ann\"",
        undefined: false,
    },
    // A pair RFC 6868 never defined, which section 2 requires a receiver to leave as it is.
    CaretCase {
        line: b"mailto:d@ex.test",
        stored: b"^x undefined",
        inner: b"^x undefined",
        meaning: b"^x undefined",
        undefined: true,
    },
    // Carets inside a quoted value: the quotes come off first, and the walk is left to right,
    // so the `^^` is an encoded caret and the `^x` after it is still undefined.
    CaretCase {
        line: b"mailto:e@ex.test",
        stored: b"\"^n^^^'^x\"",
        inner: b"^n^^^'^x",
        meaning: b"\n^\"^x",
        undefined: true,
    },
    // A caret at the very end of a value, with nothing left for it to encode.
    CaretCase {
        line: b"mailto:f@ex.test",
        stored: b"ends with a caret^",
        inner: b"ends with a caret^",
        meaning: b"ends with a caret^",
        undefined: false,
    },
    // A `CN` that needs the encoding: its octets are a `DQUOTE` pair and a comma, and section
    // 3.2 has a spelling for the comma and none at all for the quotes.
    CaretCase {
        line: b"mailto:g@ex.test",
        stored: b"\"^'Doe, John^'\"",
        inner: b"^'Doe, John^'",
        meaning: b"\"Doe, John\"",
        undefined: false,
    },
];

/// One parameter of `quoted_values.ics`, addressed the same way.
struct QuotedCase {
    /// The value text of the content line this parameter sits on.
    line: &'static [u8],
    /// The parameter name, as written.
    name: &'static [u8],
    /// The parameter value as stored, quotes included.
    stored: &'static [u8],
    /// What is left once the `DQUOTE` pair comes off.
    inner: &'static [u8],
    /// What the quotes were doing.
    quoting: ParameterQuoting,
}

/// The four positions a `DQUOTE` can occupy, and what each one turns out to mean.
const QUOTED_CASES: &[QuotedCase] = &[
    // A `DQUOTE` where a value may not begin, because the value already began. Section 3.2's
    // `SAFE-CHAR` excludes it and this reader keeps it: the octets are still all there, and
    // the parameter after it is still a parameter rather than the tail of this one.
    QuotedCase {
        line: b"mailto:a@ex.test",
        name: b"CN",
        stored: b"Doe\"John",
        inner: b"Doe\"John",
        quoting: ParameterQuoting::Bare,
    },
    QuotedCase {
        line: b"mailto:a@ex.test",
        name: b"ROLE",
        stored: b"CHAIR",
        inner: b"CHAIR",
        quoting: ParameterQuoting::Bare,
    },
    // A `:` inside quotes, twice, in a multi-valued parameter. The header does not end at
    // either of them, which is the whole reason the reader carries a quoted state.
    QuotedCase {
        line: b"mailto:d@ex.test",
        name: b"DELEGATED-TO",
        stored: b"\"mailto:b@ex.test\",\"mailto:c@ex.test\"",
        inner: b"mailto:b@ex.test\",\"mailto:c@ex.test",
        quoting: ParameterQuoting::Quoted,
    },
    // A fold inside a quoted value. The first `SP` of the continuation is the fold's and is
    // removed; the second is content, so the value unfolds to one quoted pair.
    QuotedCase {
        line: b"mailto:e@ex.test",
        name: b"CN",
        stored: b"\"Ann Marie\"",
        inner: b"Ann Marie",
        quoting: ParameterQuoting::Quoted,
    },
    // A `DQUOTE` inside a parameter *name*, which is the other place no value may begin.
    QuotedCase {
        line: b"value",
        name: b"C\"N",
        stored: b"v",
        inner: b"v",
        quoting: ParameterQuoting::Bare,
    },
];

/// A parameter value this crate could author, the octets RFC 6868 spells it with, and the
/// section 3.2 spelling of the whole once quoting is applied on top of the encoding.
///
/// That order is the only sound one: quoting adds delimiters, and encoding them would turn a
/// `DQUOTE` pair that delimits into two octets that do not.
const AUTHORED: &[(&[u8], &[u8], &[u8])] = &[
    (b"say \"hi\"", b"say ^'hi^'", b"say ^'hi^'"),
    (b"two\nlines", b"two^nlines", b"two^nlines"),
    (b"100^", b"100^^", b"100^^"),
    (b"\"Doe, John\"", b"^'Doe, John^'", b"\"^'Doe, John^'\""),
    (b"Europe/Paris", b"Europe/Paris", b"Europe/Paris"),
];

/// Parse `input` under `limits` and write it back, keeping what was diagnosed.
///
/// The refusal is carried rather than unwrapped so that a fixture crossing a bound fails an
/// assertion naming the bound instead of panicking inside this helper.
fn read(input: &[u8], limits: Limits) -> (Result<Vec<u8>, ParseError>, Vec<Diagnostic>) {
    let mut kept = Vec::new();
    let written = Document::parse(input, limits, &mut kept).map(|tree| tree.to_bytes());
    (written, kept)
}

/// Parse `input` under [`POLICY`], or hand back an empty document.
///
/// Total rather than fallible, because every fixture here is inside the default bounds and
/// [`p1_every_fixture_is_written_back_octet_for_octet`] is the assertion that says so. A
/// refusal that slipped past it surfaces as an assertion about a document with nothing in it,
/// which names the case, rather than as a panic inside a helper, which does not.
fn tree_of(input: &[u8]) -> Document {
    let mut kept: Vec<Diagnostic> = Vec::new();
    Document::parse(input, POLICY, &mut kept).unwrap_or_default()
}

/// [`POLICY`] with the header ceiling moved to `bytes`.
fn header_ceiling(bytes: u32) -> Limits {
    POLICY.with_grammar(POLICY.grammar().with_max_header_bytes(bytes))
}

/// Every property of `document`, at any depth, in the order they were written.
fn properties(document: &Document) -> Vec<&Property> {
    let mut found = Vec::new();
    collect(document.items(), &mut found);
    found
}

/// Append the properties under `items`, depth first.
fn collect<'a>(items: &'a [Item], found: &mut Vec<&'a Property>) {
    for entry in items {
        match entry {
            Item::Property(property) => found.push(property),
            Item::Component(nested) => collect(nested.items(), found),
        }
    }
}

/// The property whose value text is `value`, which is how a case names one line of a fixture.
///
/// The value rather than the name, because a fixture carries seven `ATTENDEE`s and one address
/// per line is what tells them apart — the same thing a human reading the file goes by.
fn line_of<'a>(document: &'a Document, value: &[u8]) -> Option<&'a Property> {
    properties(document)
        .into_iter()
        .find(|property| property.value_text().as_bytes() == value)
}

/// The first parameter of `property` named `name`, as written.
///
/// `name` shares the property's lifetime because `parameters_named` ties the identity it
/// searches for to the iterator it hands back.
fn parameter_of<'a>(property: &'a Property, name: &'a [u8]) -> Option<&'a Parameter> {
    property.parameters_named(name).next()
}

/// Whether `kept` carries a diagnostic with the code `wanted`.
fn reported(kept: &[Diagnostic], wanted: DiagnosticCode) -> bool {
    kept.iter().any(|held| held.code() == wanted)
}

#[test]
fn p1_every_fixture_is_written_back_octet_for_octet() {
    for (name, octets) in FIXTURES {
        let (written, _) = read(octets, POLICY);
        assert_eq!(written.as_deref(), Ok(*octets), "{name}");
    }
}

#[test]
fn p2_what_a_parse_wrote_is_a_fixed_point_of_parsing_it_again() {
    for (name, octets) in FIXTURES {
        let once = read(octets, POLICY).0.expect("a fixture within the bounds");
        let twice = read(&once, POLICY)
            .0
            .expect("what this crate wrote is readable");
        assert_eq!(twice, once, "{name}");
    }
}

/// The empty input, which is a legal calendar file and the degenerate parameter value.
///
/// Both codecs answer for it without inventing anything, which is what keeps a `X-FLAG=` with
/// nothing after the `=` from being a special case anywhere above them.
#[test]
fn an_empty_input_is_an_empty_document_and_both_codecs_answer_for_an_empty_value() {
    let (written, _) = read(b"", POLICY);
    assert_eq!(written.as_deref(), Ok(&[][..]));
    assert!(
        tree_of(b"").items().is_empty(),
        "nothing was built out of nothing"
    );

    assert!(decode_caret(b"").is_empty());
    assert!(encode_caret(b"").is_empty());
    assert!(!undefined_caret_escapes(b""));
    assert!(!caret_needs_encoding(b""));

    let empty = unquote_parameter(b"");
    assert_eq!(empty.value(), b"");
    assert_eq!(empty.quoting(), ParameterQuoting::Bare);
    assert_eq!(empty.diagnostic_code(), None);
}

/// RFC 6868 section 2: a parameter value means what the caret pairs in it say, and the octets
/// that said it are still the octets in the file.
///
/// Both halves are asserted because they are two claims. A reader that decoded into storage
/// would satisfy the first and break `docs/adr/0001`; one that never decoded at all would
/// satisfy the second and leave a user looking at `^'Doe, John^'`.
#[test]
fn rfc6868_2_a_parameter_value_means_what_the_pairs_say_and_keeps_the_octets_it_arrived_as() {
    let document = tree_of(CARET_ENCODING);
    for case in CARET_CASES {
        let property = line_of(&document, case.line).expect("the fixture carries the line");
        let held = parameter_of(property, b"CN").expect("the line carries a CN");
        assert_eq!(held.value().as_bytes(), case.stored, "{:?}", case.line);

        // The quotes come off first: RFC 6868 is defined over a parameter value, and the
        // delimiters around one are section 3.2's rather than part of it.
        let taken = unquote_parameter(case.stored);
        assert_eq!(taken.value(), case.inner, "{:?}", case.line);
        assert_eq!(
            decode_caret(taken.value()).as_ref(),
            case.meaning,
            "{:?}",
            case.line
        );
    }
    assert_eq!(
        read(CARET_ENCODING, POLICY).0.as_deref(),
        Ok(CARET_ENCODING)
    );
}

/// RFC 6868 section 2: a `^` the table gives no meaning is left as it is, and a `^` with
/// nothing after it was followed by no octet at all.
///
/// **Where implementations differ.** Nothing in RFC 6868 says what a receiver tells its user
/// about `^x`. An implementation may show the two octets, may drop the caret, or may report
/// the producer; this project answers the question and repairs nothing, so `undefined` is a
/// note a caller turns into `DiagnosticCode::UndefinedCaretEscape` with the offset and the
/// severity only it holds. Dropping the caret is the one answer that is not permitted here: it
/// would make the text shown disagree with the file that is written back.
#[test]
fn rfc6868_2_an_undefined_pair_is_a_note_the_caller_reports_and_never_a_repair() {
    for case in CARET_CASES {
        assert_eq!(
            undefined_caret_escapes(case.inner),
            case.undefined,
            "{:?}",
            case.line
        );
    }

    // The trailing caret is the boundary between the two: it is not an undefined pair, because
    // the frozen code says a `^` *followed by* an octet and this one is followed by none.
    assert!(!undefined_caret_escapes(b"ends with a caret^"));
    assert!(undefined_caret_escapes(b"^x undefined"));

    // Reported and still unrewritten, which is the half a repair would break.
    assert_eq!(decode_caret(b"^x undefined").as_ref(), b"^x undefined");
    assert_eq!(
        decode_caret(b"ends with a caret^").as_ref(),
        b"ends with a caret^"
    );
}

/// RFC 6868 section 2 defines the encoding for a parameter value and for nothing else.
///
/// So the `^n` in this fixture's `SUMMARY` is two ordinary octets. Section 3.3.11 is the codec
/// a property value goes through, its table has no caret in it, and applying RFC 6868 one
/// level up would invent an encoding no producer agreed to — silently turning a caret somebody
/// typed into a line break.
#[test]
fn rfc6868_2_the_encoding_is_a_parameter_value_reading_and_reaches_no_property_value() {
    let document = tree_of(CARET_ENCODING);
    let summary = line_of(&document, b"Handover ^n not an encoding")
        .expect("the fixture carries the SUMMARY");
    assert!(summary.is_named(b"SUMMARY"));
    assert_eq!(
        unescape_text(summary.value_text().as_bytes()).as_ref(),
        b"Handover ^n not an encoding",
        "section 3.3.11 gives a caret no meaning"
    );

    // The same octets, read as the parameter value they are not, would say something else.
    assert_eq!(decode_caret(b"^n").as_ref(), b"\n");
}

/// RFC 6868 section 2, the other direction: what this crate would author reads back as the
/// octets it was handed.
///
/// This is P1 for text with no producer behind it. Storage keeps what a file said and this
/// composition is what the write side owes instead: encode, then quote where section 3.2
/// forces it, and the two readings undo it exactly.
///
/// **Where implementations differ, and what this project does today.** Writing `^'` into a
/// file is only honest for a producer that also reads it, so adopting the encoding on the
/// write side is a change to both directions at once. `ical-core`'s mutation door refuses a
/// parameter value carrying a `DQUOTE` or a control character with
/// `MutationError::NotRepresentable` — recorded in `write_side_grammar.rs`, and unchanged by
/// this file. The encoding below is available to a caller that wants it and is not applied
/// behind one's back.
#[test]
fn rfc6868_2_what_this_crate_would_author_reads_back_as_the_octets_it_was_handed() {
    for (authored, encoded, spelled) in AUTHORED {
        assert_eq!(encode_caret(authored).as_ref(), *encoded, "{authored:?}");
        assert_eq!(quote_parameter(encoded).as_ref(), *spelled, "{authored:?}");

        let taken = unquote_parameter(spelled);
        assert_eq!(
            decode_caret(taken.value()).as_ref(),
            *authored,
            "{authored:?}"
        );
        assert!(parameter_is_representable(encoded), "{authored:?}");
    }

    // The two the base grammar cannot write at all, which is what the encoding is for.
    assert!(!parameter_is_representable(b"say \"hi\""));
    assert!(!parameter_is_representable(b"two\nlines"));
    assert!(caret_needs_encoding(b"say \"hi\""));
    assert!(!caret_needs_encoding(b"Europe/Paris"));
}

/// The write side and the read side of this file meet on one value.
///
/// The last row of [`AUTHORED`] spells `"Doe, John"` exactly as `caret_encoding.ics` carries
/// it. That is the claim "both directions or neither" reduces to over a committed file: what a
/// producer wrote and what this crate would have written are the same octets, and they decode
/// to the same text.
#[test]
fn rfc6868_2_the_spelling_this_crate_would_write_is_the_spelling_the_fixture_carries() {
    let document = tree_of(CARET_ENCODING);
    let property = line_of(&document, b"mailto:g@ex.test").expect("the fixture carries the line");
    let held = parameter_of(property, b"CN").expect("the line carries a CN");

    let authored: &[u8] = b"\"Doe, John\"";
    let spelled = quote_parameter(encode_caret(authored).as_ref()).into_owned();
    assert_eq!(held.value().as_bytes(), spelled.as_slice());
    assert_eq!(
        decode_caret(unquote_parameter(held.value().as_bytes()).value()).as_ref(),
        authored
    );
}

/// RFC 5545 section 3.2: a `DQUOTE` opens a quoted string only where a value may begin, and
/// everywhere else it is an octet.
///
/// Four positions, four answers, and only one of them is the quoted string the grammar
/// describes. The reading matters twice over: it decides where the header ends, and it decides
/// whether the property after this one exists.
#[test]
fn rfc5545_3_2_a_dquote_is_a_delimiter_only_where_a_value_may_begin() {
    let document = tree_of(QUOTED_VALUES);
    for case in QUOTED_CASES {
        let property = line_of(&document, case.line).expect("the fixture carries the line");
        let held = parameter_of(property, case.name).expect("the line carries the parameter");
        assert_eq!(held.value().as_bytes(), case.stored, "{:?}", case.name);

        let taken = unquote_parameter(case.stored);
        assert_eq!(taken.value(), case.inner, "{:?}", case.name);
        assert_eq!(taken.quoting(), case.quoting, "{:?}", case.name);
        assert_eq!(taken.diagnostic_code(), None, "{:?}", case.name);
    }
    assert_eq!(read(QUOTED_VALUES, POLICY).0.as_deref(), Ok(QUOTED_VALUES));

    // A name carrying a `DQUOTE` is read back whole and is one this crate declines to author,
    // which are two different claims and both are this corpus's business.
    assert!(!parameter_name_is_representable(b"C\"N"));
}

/// RFC 5545 section 3.2: a multi-valued parameter is a list of values, and the quotes belong
/// to each value rather than to the list.
///
/// So `unquote_parameter` is defined over one value and answers for the whole list by taking
/// the outermost pair off — which is *not* two addresses, and a caller that wants them splits
/// on the top-level `,` first. Recorded rather than fixed: a function that split as well would
/// have to allocate, would have to decide what a `,` inside quotes means, and would give a
/// caller reading a single-valued `TZID` a list to unwrap.
#[test]
fn rfc5545_3_2_unquoting_answers_for_one_value_and_a_list_is_the_caller_to_split() {
    let stored: &[u8] = b"\"mailto:b@ex.test\",\"mailto:c@ex.test\"";
    let taken = unquote_parameter(stored);
    assert_eq!(taken.quoting(), ParameterQuoting::Quoted);
    assert_eq!(taken.value(), b"mailto:b@ex.test\",\"mailto:c@ex.test");

    // Split first, then unquote, and each value answers for itself.
    let addresses: Vec<&[u8]> = stored
        .split(|octet| *octet == b',')
        .map(|entry| unquote_parameter(entry).value())
        .collect();
    assert_eq!(
        addresses,
        vec![&b"mailto:b@ex.test"[..], &b"mailto:c@ex.test"[..]]
    );
}

/// RFC 5545 section 3.2 lets a producer quote a value that needed no quoting, so two spellings
/// read as one value.
///
/// Both are permitted and this file canonizes neither. What it asserts is the pair of claims
/// that makes leaving them alone safe: the two spellings read alike, and each is written back
/// as it arrived. `quote_parameter` picks the bare one when this crate authors a value,
/// because adding octets a caller never asked for is the same class of change as dropping
/// some — but that is a rule for text with no producer, not a correction applied to a file.
#[test]
fn rfc5545_3_2_a_quoted_value_and_a_bare_one_are_two_permitted_spellings_of_one_value() {
    assert_eq!(unquote_parameter(b"\"Ann\"").value(), b"Ann");
    assert_eq!(unquote_parameter(b"Ann").value(), b"Ann");
    assert_eq!(
        unquote_parameter(b"\"Ann\"").quoting(),
        ParameterQuoting::Quoted
    );
    assert_eq!(unquote_parameter(b"Ann").quoting(), ParameterQuoting::Bare);

    for spelling in [&b"X;CN=\"Ann\":v\r\n"[..], &b"X;CN=Ann:v\r\n"[..]] {
        assert_eq!(read(spelling, POLICY).0.as_deref(), Ok(spelling));
    }

    // The spelling this crate writes when nothing forces the other one.
    assert_eq!(quote_parameter(b"Ann").as_ref(), b"Ann");
    assert_eq!(quote_parameter(b"Doe, John").as_ref(), b"\"Doe, John\"");
}

/// RFC 5545 section 3.2: an opening `DQUOTE` whose closing one never arrives swallows the `:`
/// that would have ended the header, so the property has no value at all.
///
/// **Where implementations differ.** The specification says a parameter value may be quoted
/// and does not say what a reader does when the quote is not closed, and every answer loses
/// something:
///
/// - refuse the line, which discards a calendar over one octet;
/// - close the quote at the end of the line and hand back a value, which invents an octet;
/// - end the header at the `:` as though no quote had opened, which is what a reader with no
///   quoted state does and what makes `DELEGATED-TO="mailto:a"` unreadable;
/// - keep the octets, report the shape, and let the property be one with no value — which is
///   this project's answer, because it is the only one that writes the file back unchanged.
///
/// The cost is stated rather than hidden: the value is gone from the tree's point of view, and
/// a caller reading this `X-VENDOR` gets nothing. It is a diagnostic and not an error, so the
/// rest of the calendar is still there, and the octets are still there to be written back.
#[test]
fn rfc5545_3_2_an_unterminated_quote_swallows_the_colon_that_would_have_ended_the_header() {
    let (written, kept) = read(QUOTED_VALUES, POLICY);
    assert_eq!(
        written.as_deref(),
        Ok(QUOTED_VALUES),
        "diagnosed and preserved"
    );
    assert!(
        reported(&kept, DiagnosticCode::MissingValueSeparator),
        "a line that never reached its `:` is reported as the shape it is"
    );

    let document = tree_of(QUOTED_VALUES);
    let property = properties(&document)
        .into_iter()
        .find(|held| held.is_named(b"X-VENDOR"))
        .expect("the fixture carries the line");
    assert!(!property.layout().has_separator());
    assert_eq!(property.value_text().as_bytes(), b"");

    let held = parameter_of(property, b"CN").expect("the line carries a CN");
    let taken = unquote_parameter(held.value().as_bytes());
    assert_eq!(taken.value(), b"never closed:still the header");
    assert_eq!(taken.quoting(), ParameterQuoting::Unterminated);
    assert_eq!(
        taken.diagnostic_code(),
        Some(DiagnosticCode::UnterminatedQuotedParameter)
    );
}

/// The same shape on the last line of a file that also carried no terminator, which is where a
/// truncated download and a truncated quote arrive together.
///
/// Two violations, two diagnostics, one file written back octet for octet — and no error,
/// because `ParseError` is for an input no item could be built from and this one built four.
#[test]
fn rfc5545_3_1_an_unterminated_quote_at_the_end_of_a_file_is_diagnosed_and_not_refused() {
    let (written, kept) = read(UNTERMINATED_TAIL, POLICY);
    assert_eq!(written.as_deref(), Ok(UNTERMINATED_TAIL));
    assert!(reported(&kept, DiagnosticCode::MissingFinalLineBreak));
    assert!(reported(&kept, DiagnosticCode::MissingValueSeparator));
    assert!(reported(&kept, DiagnosticCode::UnclosedComponent));

    let document = tree_of(UNTERMINATED_TAIL);
    let property = properties(&document)
        .into_iter()
        .find(|held| held.is_named(b"ATTENDEE"))
        .expect("the fixture carries the line");
    let held = parameter_of(property, b"CN").expect("the line carries a CN");
    let taken = unquote_parameter(held.value().as_bytes());
    assert!(taken.is_unterminated());
    assert_eq!(
        taken.value(),
        b"Ann Marie:mailto:z@ex.test",
        "everything after the quote is the parameter, terminator or no terminator"
    );
}

/// A quoted parameter value is header octets, so the stated header ceiling bounds it — and a
/// property value is not, so the same ceiling does not bound that.
///
/// This is the boundary every case above runs inside. The reader reassembles a name and its
/// parameters across folds into one scratch buffer because a parameter split by a fold would
/// otherwise reach a consumer in pieces; a value is delivered in chunks that borrow the input
/// and are never buffered. Bounding the first and not the second is what makes a 400 MB inline
/// `ATTACH` cheap and a 400 MB `CN` refused (`docs/adr/0008`, `docs/adr/0010`).
#[test]
fn adr0010_a_quoted_parameter_value_is_header_octets_and_the_stated_ceiling_bounds_it() {
    // `X;CN="ab"` is nine octets of header. Everything after the `:` is not header at all.
    let line: &[u8] = b"X;CN=\"ab\":a value far longer than the ceiling this case states\r\n";
    assert_eq!(read(line, header_ceiling(9)).0.as_deref(), Ok(line));
    assert_eq!(
        read(line, header_ceiling(8)).0,
        Err(ParseError::HeaderTooLarge { limit: 8 })
    );

    // Folding is not a way around it: the ceiling counts the reassembled header, and a fold
    // inside a quoted value is exactly the shape that would evade a physical-line bound.
    let folded: &[u8] = b"X;CN=\"a\r\n b\":v\r\n";
    assert_eq!(read(folded, header_ceiling(9)).0.as_deref(), Ok(folded));
    assert_eq!(
        read(folded, header_ceiling(8)).0,
        Err(ParseError::HeaderTooLarge { limit: 8 })
    );

    // The policy the rest of this file states, pinned where a caller meets it.
    assert_eq!(POLICY.grammar().max_header_bytes(), 4096);
}
