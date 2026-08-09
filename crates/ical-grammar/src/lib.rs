// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The RFC 5545 content line grammar, with no object model attached.
//!
//! Specification: RFC 5545 section 3.1, "Content Lines"
//! <https://www.rfc-editor.org/rfc/rfc5545#section-3.1>.
//!
//! Unfolding, lexing, escaping, and the structure of parameters live here; so does the
//! diagnostic vocabulary the whole workspace reports through. A linter, a diff or merge
//! tool, or a fuzz harness depends on this crate alone and never compiles a `CivilDate`, an
//! edit set, or a typed accessor. `ical-core` depends on it and adds the object model, the
//! typed views, and serialization, re-exporting every item here unchanged so that
//! `ical_core::Token` and `ical_grammar::Token` name one type (see `docs/adr/0004`).
//!
//! Diagnostics did not stay above this seam and were never going to. A violation of the
//! grammar is detected by the grammar, and a value cut short at a limit loses its bytes
//! inside the grammar, which `docs/adr/0001` requires be flagged where the bytes are
//! dropped. So `Diagnostic`, its code, its severity, and the sink they are reported into are
//! defined here rather than a layer up, and there is no second diagnostic type to reconcile
//! at the seam. The limits policy and its running meter are here for the same reason
//! (`docs/adr/0010`): the crates that name them do not all depend on each other, and the
//! count of diagnostics a sink refused has to live outside the sink.
//!
//! Everything here is byte-shaped. A fold may legally split a multi-byte codepoint and a
//! CP1252 `SUMMARY` must survive a round trip, so the layer that must never reject a
//! calendar cannot be the layer that demands UTF-8. Decoding happens in the typed view
//! above, where a failure is a diagnostic and the preserved bytes are still written back
//! (see `docs/adr/0008`).
//!
//! This seam is insurance rather than demonstrated demand, and `docs/adr/0004` says so: if
//! no caller ever wants grammar without model, the honest move is to fold this crate back
//! into `ical-core` before 1.0.
//!
//! # Status
//!
//! The whole surface is implemented and tested. Alongside the shared vocabulary — spans,
//! diagnostics and their sink, the two failure channels, the limits policy and its meter, the
//! recorded line syntax, the token shape — [`ContentLineReader`] now unfolds and lexes in one
//! pass, and section 3.3.11's escaping and section 3.2's quoting are readable and writable in
//! both directions.
//!
//! Two readings this crate had to choose are permissive and are the ones a corpus case should
//! pin down. A bare `LF` or a bare `CR` followed by whitespace is lexed as a fold, recording
//! which terminator arrived rather than refusing it, because [`FoldPoint`] exists to carry
//! exactly that. And a `DQUOTE` opens a quoted parameter value only where a value may begin,
//! so one unbalanced quote inside a `CN` cannot swallow the rest of the line. Both readings
//! round-trip; they disagree with a stricter one about where the header ends. RFC 6868's
//! caret encoding is not implemented, so a `DQUOTE` inside a parameter value stays the octets
//! it arrived as. See `ROADMAP.md` and `ical-conform`.

#![no_std]

extern crate alloc;

mod budget;
mod escape;
mod failure;
mod instant;
mod lexer;
mod location;
mod report;
mod syntax;
mod token;

pub use budget::{GrammarLimits, Limits, Meter};
pub use failure::{LimitExceeded, ParseError};
pub use instant::Instant;
pub use location::{Location, Span};
pub use report::{
    Diagnostic, DiagnosticCode, DiagnosticSink, IgnoreDiagnostics, Severity, SinkOutcome,
    report_diagnostic,
};
pub use syntax::{FoldPoint, LineEnding, LineLayout};
pub use token::{ContentLineSource, Token};

// `escape` and `lexer` are re-exported wholesale rather than item by item. Their contents
// arrive with the milestone that implements them, and a glob keeps that from turning this
// file into a place two separate pieces of work both have to edit.
pub use escape::*;
pub use lexer::*;
