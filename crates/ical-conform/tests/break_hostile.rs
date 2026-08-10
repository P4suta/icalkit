// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! An adversary's pass over the round-trip claim of `docs/adr/0001` and the bounds of
//! `docs/adr/0010`.
//!
//! Every fixture here is what an untrusted `.ics` mail attachment looks like when the sender
//! is not a calendar client: a terminator that never arrives, a quote that never closes, a
//! component that never ends, octets that are not text, and two shapes chosen to find out
//! whether the caller's stated `Limits` bound the work or merely travel beside it.
//!
//! Four properties are asserted separately, because byte identity alone is not the claim.
//! **P1** is `serialize(parse(x)) == x`. **P2** is that a second pass agrees with the first,
//! which is what catches a parser that normalizes once and then disagrees with itself. **P3**
//! is that writing one property's value moves no octet outside that property's own line, which
//! `docs/adr/0001` states and which nothing else here would notice. **P4** is that an input
//! carrying a diagnostic still satisfies P1, because a parser can always make a violation go
//! away by dropping the line that carried it.
//!
//! The last two cases in this file were findings left failing on purpose, and the shape they
//! found is that `Limits` had no field a fold was counted against. The third break lives in
//! `break_hostile_stack_overflow.rs`, which aborted its process rather than failing an
//! assertion, and is now the regression test for a serializer and a teardown that walk nesting
//! without recursing.

use std::fs;
use std::path::PathBuf;

use ical_core::{
    ContentLineReader, Diagnostic, Document, Item, Limits, Meter, ParseError, PropertyId, TextValue,
};

/// The octets of one fixture in this attacker's directory.
///
/// Read from disk rather than written inline, because `.gitattributes` marks the directory
/// `-text` and the terminators are the thing under test: a fixture Git was free to rewrite
/// would assert nothing.
fn fixture(name: &str) -> Vec<u8> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push("break_hostile");
    path.push(name);
    // `assert!` rather than an unwrap, because a helper outside a test function is production
    // code as far as the workspace lint profile is concerned.
    let read = fs::read(&path);
    assert!(read.is_ok(), "reading {}: {:?}", path.display(), read.err());
    read.unwrap_or_default()
}

/// Parse under `limits`, keeping every diagnostic the reader reported.
fn read_with(octets: &[u8], limits: Limits) -> (Result<Document, ParseError>, Vec<Diagnostic>) {
    let mut kept: Vec<Diagnostic> = Vec::new();
    let outcome = Document::parse(octets, limits, &mut kept);
    (outcome, kept)
}

/// Parse under the default policy and hand back the octets written out again.
fn round_trip(octets: &[u8]) -> (Vec<u8>, Vec<Diagnostic>) {
    let (outcome, kept) = read_with(octets, Limits::DEFAULT);
    assert!(
        outcome.is_ok(),
        "the default policy refused: {:?}",
        outcome.as_ref().err()
    );
    (
        outcome
            .map(|document| document.to_bytes())
            .unwrap_or_default(),
        kept,
    )
}

/// Every fixture whose octets a parse then a serialize has to reproduce exactly.
const SURVIVES: &[&str] = &[
    "byte_order_mark.ics",
    "nul_and_invalid_utf8.ics",
    "bare_carriage_returns.ics",
    "colonless_and_blank_lines.ics",
    "end_without_begin.ics",
    "begin_without_end.ics",
    "unterminated_quoted_parameter.ics",
    "fold_splits_a_codepoint.ics",
    "mixed_terminators.ics",
    "vendor_decorated_event.ics",
];

#[test]
fn p1_every_hostile_fixture_is_written_back_octet_for_octet() {
    for name in SURVIVES {
        let original = fixture(name);
        let (written, _) = round_trip(&original);
        assert_eq!(written, original, "P1 failed on {name}");
    }
}

#[test]
fn p2_a_second_pass_agrees_with_the_first() {
    for name in SURVIVES {
        let original = fixture(name);
        let (once, _) = round_trip(&original);
        let (twice, _) = round_trip(&once);
        assert_eq!(twice, once, "P2 failed on {name}");
    }
}

#[test]
fn p4_a_fixture_that_earns_a_diagnostic_still_round_trips() {
    // Each of these violates RFC 5545 somewhere, so each must produce at least one
    // diagnostic *and* come back out unchanged. Accepting a violation by quietly dropping the
    // line that carried it would satisfy the first half of that and fail the second.
    let diagnosed = [
        "bare_carriage_returns.ics",
        "colonless_and_blank_lines.ics",
        "end_without_begin.ics",
        "begin_without_end.ics",
        "mixed_terminators.ics",
        "vendor_decorated_event.ics",
    ];
    for name in diagnosed {
        let original = fixture(name);
        let (written, kept) = round_trip(&original);
        assert!(!kept.is_empty(), "{name} produced no diagnostic at all");
        assert_eq!(written, original, "P4 failed on {name}");
    }
}

/// How many leading octets the two agree on.
fn shared_head(before: &[u8], after: &[u8]) -> usize {
    before
        .iter()
        .zip(after.iter())
        .take_while(|(one, two)| one == two)
        .count()
}

/// How many trailing octets the two agree on.
fn shared_tail(before: &[u8], after: &[u8]) -> usize {
    before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take_while(|(one, two)| one == two)
        .count()
}

/// Where `needle` begins in `haystack`, if it is there at all.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn p3_writing_one_value_moves_no_octet_outside_that_property() {
    let original = fixture("vendor_decorated_event.ics");
    let (outcome, _) = read_with(&original, Limits::DEFAULT);
    let mut document = outcome.unwrap_or_else(|error| panic!("the fixture parsed: {error:?}"));

    let mut written = false;
    for calendar in document.components_mut() {
        for event in calendar.components_mut() {
            if let Some(mut guard) = event.get_mut::<TextValue<'_>>(&PropertyId::SUMMARY) {
                guard.set_raw(b"EDITED").unwrap_or_else(|error| {
                    panic!("a value with no control character is writable: {error:?}");
                });
                written = true;
            }
        }
    }
    assert!(written, "the fixture carries a SUMMARY to edit");

    let after = document.to_bytes();
    let head = shared_head(&original, &after);
    let tail = shared_tail(&original, &after);
    let start = find(&original, b"SUMMARY;LANGUAGE=en-GB:")
        .unwrap_or_else(|| panic!("the fixture carries the edited line"));
    // The edited line runs to its own terminator, and every octet of the divergence has to
    // sit inside it: `docs/adr/0001` promises the vendor property, the fold beside it and the
    // bare `CR` two lines down are untouched by an edit that named none of them.
    let end = find(&original, b"\r\nJUNK").map_or(original.len(), |at| at.saturating_add(2));
    assert!(head >= start, "an octet before the edited line changed");
    assert!(
        original.len().saturating_sub(tail) <= end,
        "an octet after the edited line changed"
    );
    assert!(
        find(&after, b"X-Q=\"a;b\":FREE").is_some(),
        "the vendor property survived"
    );
    assert!(
        find(&after, b"hello wo\r\n rld").is_some(),
        "the untouched fold survived"
    );
}

#[test]
fn a_header_of_ten_thousand_octets_is_refused_rather_than_buffered() {
    let original = fixture("header_10000_octets.ics");
    let (outcome, _) = read_with(&original, Limits::DEFAULT);
    assert_eq!(outcome, Err(ParseError::HeaderTooLarge { limit: 4096 }));
}

#[test]
fn a_value_of_several_megabytes_is_refused_rather_than_truncated() {
    // Built here rather than committed: the point is the ceiling, and eight megabytes of `a`
    // in the repository would assert nothing the ceiling does not.
    let mut oversized = Vec::from(&b"X-BOMB:"[..]);
    oversized.extend(std::iter::repeat_n(b'a', 8 * 1024 * 1024));
    oversized.extend_from_slice(b"\r\n");
    let (outcome, _) = read_with(&oversized, Limits::DEFAULT);
    assert_eq!(
        outcome,
        Err(ParseError::ValueTooLarge { limit: 1024 * 1024 })
    );
}

#[test]
fn nesting_ten_thousand_deep_is_refused_under_the_default_policy() {
    let original = fixture("deep_nesting_16000.ics");
    let (outcome, _) = read_with(&original, Limits::DEFAULT);
    assert_eq!(outcome, Err(ParseError::TooDeep { limit: 32 }));
}

/// One legal property whose value arrives as fifty thousand fold continuations.
///
/// `docs/adr/0010` says a hostile-input entry point takes the caller's policy and its running
/// ledger, and that the ledger is what makes bounded calls bounded in aggregate. Here the
/// caller states an input budget of sixty-four octets, and the reader used to consume a
/// hundred thousand of them without charging one: a continuation is a `FoldPoint` pushed onto
/// a vector and a `Diagnostic` pushed onto the sink, and neither is a name, a parameter or a
/// value, which was all `Meter::charge_bytes` ever saw. `max_items` does not bind either —
/// this is one item — and `max_value_bytes` does not, because the value is one octet long.
///
/// Two bounds close it, because one of them alone would not. `GrammarLimits::max_folds_per_line`
/// bounds what a single line may retain, refused at the fold that crosses it rather than after
/// the line is resident; charging each fold's octets against the shared ledger is what makes
/// the same bound hold across a document of many lines.
#[test]
fn a_stated_input_budget_bounds_a_line_of_fold_continuations() {
    let original = fixture("fold_bomb_50000.ics");
    let limits = Limits::DEFAULT.with_max_input_bytes(64);
    let mut meter = Meter::new(limits);
    let mut reader = ContentLineReader::new(&original, limits.grammar());
    let mut kept: Vec<Diagnostic> = Vec::new();
    let outcome = Document::from_tokens(&mut reader, &mut meter, &mut kept);

    let retained = match outcome.as_ref().map(Document::items) {
        Ok([Item::Property(property)]) => property.layout().folds().len(),
        _ => 0,
    };
    assert!(
        outcome.is_err() || meter.is_exhausted(),
        "a {} octet input was read whole under a {} octet budget: spent={}, items={}, \
         continuations retained={retained}, diagnostics={}",
        original.len(),
        meter.budget(),
        meter.spent(),
        meter.items(),
        kept.len(),
    );
}

/// The same shape scaled, so what is retained is measured rather than inferred.
///
/// The committed fixture is one size; this repeats its continuation run at two, because the
/// finding was that what the parse retained tracked the input and never the budget — 400,000
/// continuations retaining eight times what 50,000 did, under one unchanged sixty-four octet
/// ceiling. The assertion this file was left with said `outcome.is_ok()` and named the fix in
/// its own failure message: "the reader refused, which would be the fix". It refuses now, so
/// that is what is asserted, together with the measurement the finding was actually about —
/// what the sink was handed, against the budget the caller stated.
#[test]
fn what_a_fold_continuation_costs_is_bounded_by_the_stated_budget() {
    let limits = Limits::DEFAULT.with_max_input_bytes(64);
    let mut worst = 0_u64;
    for continuations in [50_000_usize, 400_000] {
        let mut bomb = Vec::from(&b"X:"[..]);
        for _ in 0..continuations {
            bomb.extend_from_slice(b"\n ");
        }
        bomb.extend_from_slice(b"v\r\n");

        let mut meter = Meter::new(limits);
        let mut reader = ContentLineReader::new(&bomb, limits.grammar());
        let mut kept: Vec<Diagnostic> = Vec::new();
        let outcome = Document::from_tokens(&mut reader, &mut meter, &mut kept);
        assert!(
            outcome.is_err(),
            "{continuations} continuations were read whole under a {} octet budget",
            limits.max_input_bytes()
        );
        worst = worst.max(u64::try_from(kept.len()).unwrap_or(u64::MAX));
    }
    assert!(
        worst <= limits.max_input_bytes(),
        "{worst} diagnostics were retained under a {} octet input budget",
        limits.max_input_bytes()
    );
}
