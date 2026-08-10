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

use std::fs;
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
