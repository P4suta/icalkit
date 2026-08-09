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
//! at the seam.
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
//! Bootstrap. Nothing is implemented yet; see `ROADMAP.md` (M0).

#![no_std]

extern crate alloc;
