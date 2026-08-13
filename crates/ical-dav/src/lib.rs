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
//! **The XML wrapper refuses more than it accepts.** The private `xmlparser` dependency
//! establishes XML 1.0 lexical grammar. This crate then resolves namespaces, checks duplicate
//! attributes and matching tags, enforces budgets, and maps the closed `DAV:`, CalDAV and
//! `CalendarServer` vocabulary. There is no `DOCTYPE`, entity declaration, external entity,
//! processing instruction beyond the XML declaration, non-UTF-8 declaration, or entity
//! reference beyond the five XML 1.0 predefines. Each is an explicit refusal
//! (`SECURITY.md`, `docs/adr/0013`).
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
//! Landed and tested, and the milestone it belongs to is met. Every body RFC 4791 defines for a
//! `PROPFIND` or a `REPORT` reads and writes from both ends over [`XmlReader`], which is this
//! crate's private lexer wrapper — `MKCALENDAR`'s is the one the vocabulary
//! paragraph below names as absent: [`RequestBody`] for the five request roots and the filter
//! tree beneath
//! them, [`MultiStatusReader`] and [`MultiStatusWriter`] for the multistatus one response at a
//! time, [`MultiStatus`] as one consumer of each rather than a second implementation beside
//! them, [`XmlWriter`] as the element writer whose open-element stack makes an unbalanced
//! document unrepresentable, and [`Revision`] for the conditional write that makes a second
//! turn land on the revision the first turn read. `tests/interop.rs` drives the two halves
//! through each other rather than each against a stand-in.
//!
//! **What this crate does not do, and a server needs.** Nothing here evaluates a filter: a
//! `comp-filter`, a `time-range` and a `text-match` are represented and handed back, and
//! deciding which resources match is work a server does by composing them with `ical-recur`
//! and `ical-core`. The vocabulary is CalDAV's and stops there — `MKCALENDAR` has a request
//! body and no row, RFC 3744's ACL vocabulary is absent, and so are `DAV:expand-property` and
//! `DAV:principal-property-search`. The two gaps that sat inside what *is* modeled are closed,
//! by the attack rather than by the plan: `CALDAV:timezone` (the inline `VTIMEZONE` a
//! `calendar-query` may carry, RFC 4791 section 9.5) has a row, a field on [`CalendarQuery`] and
//! the line-ending carve-out its value earns, so the zone a client stated survives a read and a
//! re-encode instead of being dropped as foreign and answering a floating-time `time-range` in a
//! zone nobody asked for; and `DAV:allprop` and `DAV:propname` inside a `calendar-query` are
//! [`QueryShape`] beside `props`, so section 9.5's own production is a body this crate reads and
//! writes rather than one it answers `DavError::Unexpected` to. What sits on that list in their
//! place is `calendar-multiget`, whose grammar admits the same three shapes and which still
//! carries only a property list: nobody is known to send the other two to a multiget, which is a
//! reason to file it rather than to call it closed. RFC 6638's preconditions and tags are
//! modeled and the POST to a scheduling outbox is not.
//!
//! This crate is not RFC-4791-complete and nothing here entitles anyone to say it is. See
//! `ROADMAP.md` (M4) and `docs/design/ical-dav-api.md`.

#![no_std]

extern crate alloc;

#[path = "../../icalkit/src/internal/dav/bound.rs"]
mod bound;
#[path = "../../icalkit/src/internal/dav/codec.rs"]
mod codec;
#[path = "../../icalkit/src/internal/dav/element.rs"]
mod element;
#[path = "../../icalkit/src/internal/dav/failure.rs"]
mod failure;
#[path = "../../icalkit/src/internal/dav/freshness.rs"]
mod freshness;
#[path = "../../icalkit/src/internal/dav/policy.rs"]
mod policy;
#[path = "../../icalkit/src/internal/dav/read_request.rs"]
mod read_request;
#[path = "../../icalkit/src/internal/dav/read_response.rs"]
mod read_response;
#[path = "../../icalkit/src/internal/dav/reader.rs"]
mod reader;
#[path = "../../icalkit/src/internal/dav/request.rs"]
mod request;
#[path = "../../icalkit/src/internal/dav/response.rs"]
mod response;
#[path = "../../icalkit/src/internal/dav/sink.rs"]
mod sink;
#[path = "../../icalkit/src/internal/dav/text.rs"]
mod text;
#[path = "../../icalkit/src/internal/dav/value.rs"]
mod value;
#[path = "../../icalkit/src/internal/dav/write_request.rs"]
mod write_request;
#[path = "../../icalkit/src/internal/dav/write_response.rs"]
mod write_response;
#[path = "../../icalkit/src/internal/dav/writer.rs"]
mod writer;
// The WebDAV XML grammar, kept private and forbidden to name a CalDAV type. Nothing of it is
// re-exported below, deliberately and permanently: `docs/adr/0012` decided that `webdav-core` is
// not published and that the untangling happens anyway, because the harm ADR 0004's ordering bet
// guards against is caused by *exporting* the grammar rather than by leaving it in place. A
// published crate name cannot be withdrawn; an unexported module can. `gates/xml-layering`
// compiles it in a root with no CalDAV vocabulary, and the third rule of `just purity` is what
// keeps that member from being deleted.
#[path = "../../icalkit/src/internal/dav/xml/mod.rs"]
mod xml;

// Stable crate-shaped root for source shared with `icalkit::internal::dav`.
pub(crate) mod internal {
    #[allow(unused_imports)]
    pub(crate) mod dav {
        pub(crate) const SYNC_COLLECTION_ENABLED: bool = cfg!(feature = "sync-collection");

        pub(crate) use crate::request::SyncCollection;
        pub(crate) use crate::{
            Bounded, ByteSink, CalendarDataRequest, CalendarMultiget, CalendarPayload,
            CalendarQuery, Collation, CompFilter, CompSelection, DavError, DavProperty,
            DavResponse, DecodeContext, DecodedText, Depth, ETag, ElementName, ElementSpec,
            ErrorBody, ExtensionName, FreeBusyQuery, Href, IF_SCHEDULE_TAG_MATCH, LineEndings,
            MatchHeader, MultiStatus, MultiStatusReader, MultiStatusWriter, Namespace, ParamFilter,
            Precondition, Prefer, Presence, PropFind, PropName, PropRequest, PropStat, PropValue,
            QName, QueryShape, ReadXml, RequestBody, ResourceType, ResponseBody, ResponseSource,
            Revision, SinkFull, SliceSink, Status, SyncLevel, SyncToken, SyntaxError, TextMatch,
            TextMode, TextPolicy, TextRun, TimeRange, UNREADABLE_STATUS, UnknownPolicy, ValueError,
            WriteXml, XmlEvent, XmlPull, XmlReader, XmlWriter, bound, codec, element, failure,
            freshness, policy, read_request, read_response, reader, request, response, sink, text,
            value, write_request, write_response, writer, xml,
        };
    }
}

// The seven modules above that are not the shared foundation, and what each owns. They were
// written concurrently against the frozen surface and integrated in one pass, which is why
// each is one file with no overlap; the notes stay because they are the division of labor a
// reader needs to know before changing one of them.
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
    FreeBusyQuery, ParamFilter, PropFilter, PropFind, PropName, PropRequest, QueryShape, SyncLevel,
    TextMatch, TimeRange,
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
    Depth, ETag, ExtensionName, Href, MatchHeader, Precondition, Prefer, ResourceType, Status,
    SyncToken,
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
