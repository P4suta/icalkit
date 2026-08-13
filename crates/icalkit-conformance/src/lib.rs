// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unpublished subject internals used by the in-tree adversarial corpus.
//!
//! The versioned JSONL protocol remains the interoperability boundary. This library is an
//! isolation helper for repository tests that deliberately exercise implementation invariants
//! below the public `icalkit` API; it is never part of the production or release graph.

#![forbid(unsafe_code)]

extern crate alloc;
extern crate self as ical_core;

#[path = "../../icalkit/src/internal/mod.rs"]
pub mod internal;

// The isolated XML layer is compiled in three roots and names only these metering primitives.
// Re-export them at this private tool's root so its stable `icalkit_conformance::internal::core::` spelling still resolves.
pub use internal::core::{LimitExceeded, Limits, Meter};
