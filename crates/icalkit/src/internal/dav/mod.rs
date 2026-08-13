// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Private WebDAV and CalDAV wire kernel.
//!
//! Files in this module are also compiled by the temporary `ical-dav` conformance harness.
//! The unified crate always includes RFC 6578 sync support; the former package's feature remains
//! only long enough to exercise its legacy compatibility surface.

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
pub(crate) const SYNC_COLLECTION_ENABLED: bool = true;

pub(crate) use bound::Bounded;
pub(crate) use codec::{ReadXml, ResponseSource, WriteXml, XmlEvent, XmlPull};
pub(crate) use element::{ElementName, ElementSpec, Namespace, QName};
pub(crate) use failure::{DavError, SinkFull, SyntaxError, ValueError};
pub(crate) use freshness::{
    IF_SCHEDULE_TAG_MATCH, Presence, Revision, write_depth_value, write_prefer_value,
};
pub(crate) use policy::{DecodeContext, UnknownPolicy};
pub(crate) use read_request::RequestBody;
pub(crate) use read_response::{MultiStatusReader, UNREADABLE_STATUS};
pub(crate) use reader::XmlReader;
pub(crate) use request::{
    CalendarDataRequest, CalendarMultiget, CalendarQuery, Collation, CompFilter, CompSelection,
    FreeBusyQuery, ParamFilter, PropFilter, PropFind, PropName, PropRequest, QueryShape,
    SyncCollection, SyncLevel, TextMatch, TimeRange,
};
pub(crate) use response::{
    CalendarPayload, DavProperty, DavResponse, ErrorBody, MultiStatus, PropStat, PropValue,
    ResponseBody,
};
pub(crate) use sink::{ByteSink, SliceSink};
pub(crate) use text::{
    DecodedText, LineEndings, TextMode, TextPolicy, TextRun, decode_text, write_escaped_attribute,
    write_escaped_text,
};
pub(crate) use value::{
    Depth, ETag, ExtensionName, Href, MatchHeader, Precondition, Prefer, ResourceType, Status,
    SyncToken,
};
pub(crate) use write_request::write_utc_date_time;
pub(crate) use write_response::MultiStatusWriter;
pub(crate) use writer::XmlWriter;
