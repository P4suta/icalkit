// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private WebDAV and CalDAV wire kernel.
//!
//! The unpublished conformance helper also compiles these files to exercise the low-level
//! adversarial corpus. The unified crate always includes RFC 6578 sync support.

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
mod xml;

/// Whether RFC 6578 request bodies are accepted by this compilation root.
pub const SYNC_COLLECTION_ENABLED: bool = true;

pub use bound::Bounded;
pub use codec::{ReadXml, ResponseSource, WriteXml, XmlEvent, XmlPull};
pub use element::{ElementName, ElementSpec, Namespace, QName};
pub use failure::{DavError, SinkFull, SyntaxError, ValueError};
pub use freshness::{
    IF_SCHEDULE_TAG_MATCH, Presence, Revision, write_depth_value, write_prefer_value,
};
pub use policy::{DecodeContext, UnknownPolicy};
pub use read_request::RequestBody;
pub use read_response::{MultiStatusReader, UNREADABLE_STATUS};
pub use reader::XmlReader;
pub use request::{
    CalendarDataRequest, CalendarMultiget, CalendarQuery, Collation, CompFilter, CompSelection,
    FreeBusyQuery, ParamFilter, PropFilter, PropFind, PropName, PropRequest, QueryShape,
    SyncCollection, SyncLevel, TextMatch, TimeRange,
};
pub use response::{
    CalendarPayload, DavProperty, DavResponse, ErrorBody, MultiStatus, PropStat, PropValue,
    ResponseBody,
};
pub use sink::{ByteSink, SliceSink};
pub use text::{
    DecodedText, LineEndings, TextMode, TextPolicy, TextRun, decode_text, write_escaped_attribute,
    write_escaped_text,
};
pub use value::{
    Depth, ETag, ExtensionName, Href, MatchHeader, Precondition, Prefer, ResourceType, Status,
    SyncToken,
};
pub use write_request::write_utc_date_time;
pub use write_response::MultiStatusWriter;
pub use writer::XmlWriter;
