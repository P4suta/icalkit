// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV (RFC 4791) as values: requests to build, responses to interpret, no transport.
//!
//! Specification: RFC 4791, "Calendaring Extensions to WebDAV (CalDAV)"
//! <https://www.rfc-editor.org/rfc/rfc4791>, over RFC 4918 (WebDAV), with RFC 6578 for
//! collection synchronization.
//!
//! CalDAV is WebDAV with calendar semantics: `PROPFIND` and `REPORT` carrying XML bodies,
//! `calendar-query` and `calendar-multiget` for asking which resources matter,
//! `ETag`-conditional writes so two clients do not overwrite each other, and sync tokens so
//! the third refresh does not download the year again. None of that is transport — it is
//! the shape of a request and the reading of a response.
//!
//! This crate builds the one and interprets the other; moving bytes is the caller's job,
//! with the HTTP client, async runtime, and TLS stack it already has. Bundling one would
//! choose all three on behalf of every user, and calendar applications are exactly the
//! programs that made those choices long before they needed a calendar library (see
//! `docs/adr/0004`).
//!
//! Because the layer is data in and data out, one implementation serves both directions: a
//! client builds a `REPORT` and reads the multi-status, a server reads the `REPORT` and
//! builds the multi-status. There is no client-only shape, and the parsing half is the one
//! that does not exist in Rust today. Testing needs no server either — a request is a value
//! and a response is a byte string, so an interoperability case is a recorded exchange.
//!
//! A `time-range` filter is represented here but not evaluated here: deciding which
//! instances of a recurring event fall inside it is recurrence work, which is why this
//! crate depends on the model alone and a server combines it with `ical-recur`.
//!
//! The XML inside those requests and responses is this crate's own, not an outside
//! dependency's: a tokenizer over the closed `DAV:` and CalDAV element vocabulary, resolving
//! namespaces rather than matching prefixes, with no DTD, no external entities, and bounds
//! checked on every event (see `docs/adr/0004`). `calendar-data` stays opaque and is parsed
//! by `ical-core`.
//!
//! Reading a multi-status is incremental and holds one response at a time; writing one is
//! incremental too. A `calendar-multiget` over forty thousand `href`s is a real request from
//! both sides, so materializing the whole thing is one optional consumer of the decoder
//! rather than the only way to read one, and the cap on that belongs to the caller's buffer
//! and not to the wire format.
//!
//! # Status
//!
//! Bootstrap. Nothing is implemented yet; see `ROADMAP.md` (M4). The public surface is
//! designed and compiled; `docs/design/ical-dav-api.md` carries it, including the one part
//! that has never been proved — whether the incremental codec pair is expressible under
//! `no_std` with no allocator on `thumbv7em-none-eabi`.

#![no_std]

extern crate alloc;
