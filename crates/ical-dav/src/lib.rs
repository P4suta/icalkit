// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV (RFC 4791) as values: requests to build, responses to interpret, no transport.
//!
//! Specification: RFC 4791, "Calendaring Extensions to WebDAV (CalDAV)"
//! <https://www.rfc-editor.org/rfc/rfc4791>, over RFC 4918 (WebDAV), with RFC 6578 for
//! collection synchronization and RFC 6638 for the scheduling properties beside it.
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
//! # Three things a reader should know before using it
//!
//! **The XML is this crate's own, and it refuses more than it accepts.** A hand-rolled
//! tokenizer over the closed `DAV:`, CalDAV and `CalendarServer` vocabulary, resolving
//! namespaces rather than matching prefixes. There is no `DOCTYPE`, no entity declaration, no
//! external entity, no processing instruction beyond the XML declaration, no encoding other
//! than UTF-8, and no entity reference beyond the five XML 1.0 predefines. Each of those is a
//! refusal rather than a gap, because a reader that is merely incomplete is safer than one
//! that is accidentally complete (`SECURITY.md`, `docs/adr/0004`).
//!
//! **`calendar-data` keeps its line endings, and that is a stated departure from XML 1.0.**
//! Section 2.11 of XML 1.0 requires every `CRLF` to be folded to `LF` before parsing, and RFC
//! 5545 makes that same `CRLF` the syntax of a content line. Inside `CALDAV:calendar-data` and
//! nowhere else, this reader hands back the octets as they arrived. The reasoning, the cost,
//! and the caller's way out — [`TextPolicy`] — are on that type and in `docs/adr/0004`.
//!
//! **Nothing here is unbounded.** Every reading door takes the caller's `Limits` and a
//! `&mut Meter`, every collection is a [`Bounded`] whose growth is a charged push, and the
//! dimensions include the ones a body's size does not reach: response count, property count
//! per response, `href` length, one element's character data, and the namespace bindings live
//! at once (`docs/adr/0010`).
//!
//! # Status
//!
//! The shared foundation is landed and tested: the failure channels, the byte sinks, the
//! element vocabulary, the bounded collection, the character-data rules that resolve the
//! line-ending collision, the protocol values, the request and response shapes, and the four
//! codec traits. The tokenizer, the element writer, the per-body readers and writers, and the
//! conditional-write binding are the units built on top of it; see `ROADMAP.md` (M4).

#![no_std]

extern crate alloc;

mod bound;
mod codec;
mod element;
mod failure;
mod freshness;
mod policy;
mod read_request;
mod read_response;
mod reader;
mod request;
mod response;
mod sink;
mod text;
mod value;
mod write_request;
mod write_response;
mod writer;

// The seven units built on the foundation above. Each creates exactly one file, declares it
// here, and adds its own line to the re-export block at the end; nothing else in this file is
// a shared edit. The files are absent rather than present and empty because `cargo shear`
// refuses a module with no items, and a placeholder item to satisfy it would be padding.
//
// `reader.rs`         `XmlPull` over one body: an iterative state machine with an explicit
//                     stack, the scoped prefix-binding stack `Namespace` resolution needs
//                     charged through `Meter::try_bind_prefix`, and the refusals — DOCTYPE,
//                     entity declaration, external entity, processing instruction, non-UTF-8
//                     encoding, unbound prefix, mismatched tag, duplicate attribute,
//                     truncation. Character data goes to `decode_text` under the mode
//                     `TextMode::of` gives it, which this unit never chooses for itself.
// `writer.rs`         The element writer: the XML declaration, the root's fixed `D:`, `C:` and
//                     `CS:` declarations, `Namespace::write_prefix` plus a local name for every
//                     element, `write_escaped_text` and `write_escaped_attribute` for content,
//                     no CDATA ever, and an open-element stack so an unbalanced write cannot
//                     be expressed.
// `read_request.rs`   `ReadXml` for `PropFind`, `CalendarQuery`, `CalendarMultiget`,
//                     `FreeBusyQuery`, `SyncCollection`, and the filter tree under them —
//                     through the constructors those types already have, so a server decoding
//                     a REPORT meets the refusals a client building one would.
// `write_request.rs`  `WriteXml` for the same, plus the `YYYYMMDDTHHMMSSZ` a `time-range`
//                     attribute carries, reported as `ValueError::TimeUnrepresentable` where
//                     an instant has no such spelling rather than clamped to one that does.
// `read_response.rs`  `MultiStatusReader` and `ResponseSource`: one `DavResponse` at a time,
//                     each property into the `PropValue` its element earns, `CalendarPayload`
//                     carrying the line-ending witness the decode produced, and
//                     `PropValue::Unmodeled` rather than a discard for a value with no model.
// `write_response.rs` `WriteXml` for `MultiStatus` and everything under it, plus the
//                     incremental encoder a server pushes one response into at a time.
// `freshness.rs`      `Revision`: the `ETag`, `schedule-tag` and sync token a caller read, and
//                     the `Precondition` a conditional write must carry to land on that
//                     revision. It holds no authority of its own; the freshness a caller gets
//                     is the freshness the server enforces when it compares the `If-Match`.

pub use crate::bound::Bounded;
pub use crate::codec::{ReadXml, ResponseSource, WriteXml, XmlEvent, XmlPull};
pub use crate::element::{ElementName, ElementSpec, Namespace, QName};
pub use crate::failure::{DavError, SinkFull, SyntaxError, ValueError};
pub use crate::policy::{DecodeContext, UnknownPolicy};
#[cfg(feature = "sync-collection")]
pub use crate::request::SyncCollection;
pub use crate::request::{
    CalendarDataRequest, CalendarMultiget, CalendarQuery, Collation, CompFilter, CompSelection,
    FreeBusyQuery, ParamFilter, PropFilter, PropFind, PropName, PropRequest, SyncLevel, TextMatch,
    TimeRange,
};
pub use crate::response::{
    CalendarPayload, DavProperty, DavResponse, ErrorBody, MultiStatus, PropStat, PropValue,
    ResponseBody,
};
pub use crate::sink::{ByteSink, SliceSink};
pub use crate::text::{
    DecodedText, LineEndings, TextMode, TextPolicy, TextRun, decode_text, write_escaped_attribute,
    write_escaped_text,
};
pub use crate::value::{
    Depth, ETag, ExtensionName, Href, Precondition, Prefer, ResourceType, Status, SyncToken,
};

// Unit re-exports. One line per unit, appended by that unit and by nothing else.
pub use crate::freshness::{
    IF_SCHEDULE_TAG_MATCH, Presence, Revision, write_depth_value, write_prefer_value,
};
pub use crate::read_request::RequestBody;
pub use crate::read_response::{MultiStatusReader, UNREADABLE_STATUS};
pub use crate::reader::XmlReader;
pub use crate::write_request::write_utc_date_time;
pub use crate::write_response::MultiStatusWriter;
pub use crate::writer::XmlWriter;
