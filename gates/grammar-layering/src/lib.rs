// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The grammar layer, compiled alone. A path from it into the model above it resolves in
//! `icalkit` and does not resolve here, which is what makes the layer a fact (ADR 0004).

// The grammar names `alloc` and `alloc` is not in a std crate's extern prelude, so the
// declaration is needed here even though nothing in this file uses it. Deliberately not
// `#![no_std]`: that attribute is what `xtask purity` reads to decide a directory holds a core
// crate, and this member is not one.
extern crate alloc;

#[path = "../../../crates/icalkit/src/internal/core/grammar/mod.rs"]
mod grammar;

pub use crate::grammar::*;
