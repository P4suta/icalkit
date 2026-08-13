// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The RFC 5545 content line grammar, with no object model attached.
//!
//! Specification: RFC 5545 section 3.1, "Content Lines"
//! <https://www.rfc-editor.org/rfc/rfc5545#section-3.1>.
//!
//! Unfolding, lexing, escaping, and the structure of parameters live here; so does the
//! diagnostic vocabulary the whole workspace reports through. The crate root re-exports every
//! item of this module unchanged, so `crate::internal::core::Token` is the one spelling of that type and
//! no caller ever writes this module's path (see `docs/adr/0004`).
//!
//! This was a crate of its own until D-0003. It was insurance against a caller that wanted the
//! grammar without the model, `docs/adr/0004` said out loud that the honest move was to fold it
//! back if no such caller appeared, and none did. What survives the fold is the layer, and the
//! rule that keeps it one: nothing here names anything above this directory. Not `crate::`, not
//! `super::` from this file, not `super::super::` from the files beside it, not `crate::internal::core::`,
//! and no `extern crate` to make that spelling available. The tree stays flat, `#[path]` is
//! refused, and every `.rs` file beside this one is declared by it, so that the directory this
//! rule reads and the module tree the compiler reads are the same set of files.
//! `gates/grammar-layering` compiles these sources in a crate that has no model, which turns
//! naming a model item into a compile error; it cannot see a `crate::X` that the crate root
//! re-exports from here, so the second rule of `just purity` reads this directory for one
//! textually. That is hygiene about not routing a lateral import through the parent crate's
//! public surface, and no compiler enforces it.
//!
//! Diagnostics did not stay above this layer and were never going to. A violation of the
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
//! # Status
//!
//! The whole surface is implemented and tested. Alongside the shared vocabulary — spans,
//! diagnostics and their sink, the two failure channels, the limits policy and its meter, the
//! recorded line syntax, the token shape — [`ContentLineReader`] now unfolds and lexes in one
//! pass, and section 3.3.11's escaping, section 3.2's quoting and RFC 6868's caret encoding
//! are readable and writable in both directions.
//!
//! Two readings this layer had to choose are permissive, and both are now pinned by a corpus
//! case rather than owed one. A bare `LF` or a bare `CR` followed by whitespace is lexed as a
//! fold, recording which terminator arrived rather than refusing it, because [`FoldPoint`] exists
//! to carry exactly that. And a `DQUOTE` opens a quoted parameter value only where a value may
//! begin, so one unbalanced quote inside a `CN` cannot swallow the rest of the line. Both readings
//! round-trip; they disagree with a stricter one about where the header ends. RFC 6868's
//! caret encoding is a codec rather than a storage rule: storage keeps the octets a producer
//! wrote, so a `DQUOTE` written `^'` stays `^'` on the wire and is a `"` only in the decoded
//! view. See `ROADMAP.md` and `ical-conform`.

mod budget;
mod caret;
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
    Diagnostic, DiagnosticCode, DiagnosticSink, IgnoreDiagnostics, Severity, SinkOutcome, Subject,
    report_diagnostic,
};
pub use syntax::{FoldPoint, LineEnding, LineLayout};
pub use token::{ContentLineSource, Token};

// `caret`, `escape` and `lexer` are re-exported wholesale rather than item by item. Their
// contents arrive with the milestone that implements them, and a glob keeps that from turning
// this file into a place two separate pieces of work both have to edit.
pub use caret::*;
pub use escape::*;
pub use lexer::*;
