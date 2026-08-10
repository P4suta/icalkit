// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! BREAK. Writing a deeply nested calendar back out overflows the stack and aborts.
//!
//! Its own binary rather than a case in `break_hostile.rs`, because the failure is not an
//! assertion. `write_component` in `crates/ical-core/src/emit.rs` walks nesting by recursion,
//! and the comment above it justifies that with "depth is bounded where the tree is built, by
//! `Limits::max_component_depth`, so a document read through `Document::parse` cannot reach
//! here deep enough to matter". `max_component_depth` is a `u16` the caller sets through a
//! public builder, and every value above roughly five thousand exhausts the stack before the
//! walk finishes. A stack overflow is a process abort, not a panic: no `catch_unwind` sees it,
//! no sibling test in the same binary survives it, and a server parsing an untrusted
//! attachment loses the whole process rather than the request.
//!
//! `Document`'s derived `Drop` is recursive for the same reason, so a caller that parses such
//! a file and never serializes it dies anyway, one scope later.
//!
//! Nextest runs one process per test, so this failure is reported as this test's and does not
//! take the rest of the suite with it. Plain `cargo test` is less forgiving, which is the
//! second reason for the separate file.
//!
//! The serializer and the teardown were given explicit stacks by the milestone that recorded
//! the break above. The five *derived* traversals were not, and the debt said so in a sentence
//! that named its own measurement: `document.clone()` on a twenty-thousand-deep tree overflows
//! the stack. The second test here is that measurement, run from outside `ical-core` through
//! the only door a caller has — `Document::parse` — because a hand-built tree in a unit test
//! proves the traversal iterates and this proves the whole path a caller walks does.

use std::cmp::Ordering;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use ical_core::{Diagnostic, Document, Limits};

/// The nesting ladder: sixteen thousand `BEGIN:X` lines and their sixteen thousand `END`s.
///
/// Sixteen thousand rather than the ten thousand an attacker would reach for first, because
/// the threshold moves with the optimization level — the unoptimized build gives up between
/// four and six thousand, the release build between eight and twelve — and a fixture that
/// reproduces in only one of the two profiles would be reported as flaky rather than as this.
fn ladder() -> Vec<u8> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push("break_hostile");
    path.push("deep_nesting_16000.ics");
    // `assert!` rather than an unwrap, because a helper outside a test function is production
    // code as far as the workspace lint profile is concerned.
    let read = fs::read(&path);
    assert!(read.is_ok(), "reading {}: {:?}", path.display(), read.err());
    read.unwrap_or_default()
}

#[test]
fn a_calendar_nested_sixteen_thousand_deep_is_written_back_octet_for_octet() {
    let original = ladder();
    // The one policy field this attack needs the caller to have raised. Nothing else about
    // the file is irregular: it is `CRLF` throughout, every `BEGIN` has its `END`, and the
    // parse reports no diagnostic at all.
    let limits = Limits::DEFAULT.with_max_component_depth(u16::MAX);
    let mut kept: Vec<Diagnostic> = Vec::new();
    let document = Document::parse(&original, limits, &mut kept)
        .unwrap_or_else(|error| panic!("the raised policy accepted the depth: {error:?}"));
    assert!(kept.is_empty(), "a conforming calendar earns no diagnostic");

    // The process does not return from this call.
    let written = document.to_bytes();
    assert_eq!(
        written, original,
        "P1 failed on a sixteen-thousand-deep tree"
    );
}

/// A ladder of `depth` `BEGIN:X` lines and the `END:X` lines that close them.
///
/// Written here rather than committed as a fixture: the octets are two lines repeated, so a
/// file would add three hundred kilobytes to the repository and say nothing the loop does not.
/// The committed fixture above exists because its terminators are the thing under test; this
/// input's only interesting property is how deep it goes.
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

/// The measurement the debt recorded, from outside the crate that fixed it.
///
/// Twenty thousand because that is the number the milestone measured `clone` aborting at, and
/// the depth is reached the way an attacker reaches it — by handing over a file — rather than
/// by assembling a tree the reader would never build. Every one of the five traversals that
/// used to recurse is exercised, and the test asserts the whole of its claim by returning: a
/// stack overflow is an abort, so an assertion after one of these calls is not what reports it.
#[test]
fn every_derived_traversal_of_a_twenty_thousand_deep_document_returns() {
    const DEPTH: usize = 20_000;

    let octets = ladder_of(DEPTH);
    let limits = Limits::DEFAULT.with_max_component_depth(u16::MAX);
    let mut kept: Vec<Diagnostic> = Vec::new();
    let document = Document::parse(&octets, limits, &mut kept)
        .unwrap_or_else(|error| panic!("the raised policy accepted the depth: {error:?}"));
    assert!(kept.is_empty(), "a conforming calendar earns no diagnostic");

    // `document.clone()` is the call the debt named. The four below it recurse identically,
    // and `Ord` and `Hash` are reached by any caller that puts a component in a `BTreeMap`.
    let copy = document.clone();
    assert_eq!(
        copy, document,
        "a copy differs from what it was copied from"
    );
    assert_eq!(copy.cmp(&document), Ordering::Equal);
    assert_eq!(hashed(&copy), hashed(&document), "equal, hashed unequally");
    assert!(!format!("{document:?}").is_empty());
    assert_eq!(copy.to_bytes(), octets, "P1 failed at twenty thousand deep");

    // And the teardown, which runs twice here and once more when `kept` goes out of scope.
    drop(copy);
    drop(document);
}
