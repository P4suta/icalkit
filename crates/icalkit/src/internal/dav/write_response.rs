// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writing a multistatus: the whole body at once, or one response at a time.
//!
//! The same types a client reads through [`crate::internal::dav::ResponseSource`] are the ones a server writes
//! through here, and the direction shows up in which trait is called and in the `Limits` the
//! caller passes — never in which fields exist. A `getetag` at `200` beside a `displayname` at
//! `404` is one [`DavResponse`] with two [`PropStat`]s on either side of the wire.
//!
//! # A document and a fragment are different things
//!
//! [`MultiStatus`] and [`MultiStatusWriter`] write a *document*: the XML declaration, the root
//! element, and the three namespace declarations everything under it resolves against. Every
//! other `WriteXml` implementation in this module writes a *fragment* that is well-formed only
//! inside that root, because the prefixes it uses are declared there. That is not a gap: a
//! fragment is what a server appends to a body it is already streaming, and repeating the
//! declarations on every response would be three hundred octets per resource on a body that may
//! carry forty thousand of them.
//!
//! The prefixes `D:`, `C:` and `CS:` are an output choice and never an input assumption. A peer
//! that binds `DAV:` to `d:`, to `ns0:`, or to no prefix at all through a default declaration is
//! naming the same elements; identity is the (namespace, local name) pair
//! [`crate::internal::dav::ElementName::resolve`] takes, and nothing here writes or reads a prefix as though it
//! meant something.
//!
//! # Nothing here is unbounded
//!
//! A server answering a forty-thousand-resource collection cannot be asked to hold the answer,
//! which is why [`MultiStatusWriter`] exists and why the owned [`MultiStatus`] drives it rather
//! than duplicating it — the same relationship [`MultiStatus::read`] has with the streaming
//! reader, so the two cannot drift apart. Each door charges what it costs against the caller's
//! ledger: one element and one level of nesting per element written, one response per response,
//! and the octets of every `href`, every character run and every kept fragment. The cardinality
//! bounds are the ones the read side charges — `max_responses` for the responses,
//! `max_props_per_response` for the property groups and their properties — because a body this
//! policy admits writing is a body the same policy admits reading back, and a pair of numbers
//! that disagreed would describe an exchange nobody can complete.
//!
//! # Two values are written back rather than re-rendered
//!
//! A `CALDAV:calendar-data` payload goes through the shared writer's text escaping, so each `CR` leaves as
//! `&#13;` — the one construct XML 1.0 section 2.11 does not fold, which is what lets any
//! conformant reader reconstruct the octets the server stored. No `CDATA` section is written,
//! ever, so a literal `]]>` inside a `DESCRIPTION` is not an escaping bug waiting to happen.
//!
//! A property this crate has no model for leaves in one of two shapes, and which one it is
//! decides how it is written. [`PropValue::Unmodeled`] is *character data* and is escaped like
//! any other run: what a peer sent as a string leaves here as a string. That is not tidiness.
//! A peer writing `&lt;D:href&gt;/calendars/ann/private/secret.ics&lt;/D:href&gt;` inside its
//! own extension property is writing a string, and a proxying server that copied the decoded
//! octets out unescaped would put a `DAV:href` element into its own multistatus that the peer
//! never sent — an element RFC 4918 gives meaning to, chosen by the peer.
//!
//! [`PropValue::Markup`] is the other shape: elements a peer really did send, re-serialized by
//! the reader in this crate's own prefixes with every text run already escaped. Those octets go
//! to the sink as they stand, which is what makes a proxy lossless over a peer's structure. A
//! caller may also build one by hand, so they are screened first: the tags must balance, so a
//! kept value cannot close an element this writer opened; there is no `DOCTYPE`, comment,
//! `CDATA` section or processing instruction, because this crate's own reader refuses those and
//! a body it cannot read back is a body it must not write; every `&` must begin a reference a
//! reader would resolve, so the emitted document is XML; and the octets must be UTF-8, because
//! the document declares that it is. That guard is a refusal filter and not a parser: what
//! passes it is well-formed enough not to forge structure, which is the property that matters.

use core::fmt::{self, Debug, Formatter};

use crate::internal::core::{LimitExceeded, Limits, Meter};

use crate::internal::dav::codec::WriteXml;
use crate::internal::dav::element::ElementName;
use crate::internal::dav::failure::{DavError, SyntaxError, ValueError};
use crate::internal::dav::request::PropName;
use crate::internal::dav::response::{
    CalendarPayload, DavProperty, DavResponse, ErrorBody, MultiStatus, PropStat, PropValue,
    ResponseBody,
};
use crate::internal::dav::sink::ByteSink;
use crate::internal::dav::value::{ETag, Href, ResourceType, Status, SyncToken, bounded_cap};
use crate::internal::dav::writer::XmlWriter;

/// How deep the deepest element of a multistatus sits.
///
/// `multistatus` > `response` > `propstat` > `prop` > one property > that property's own child,
/// which is where a `DAV:href` inside an `owner` lands. A caller whose policy refuses a document
/// this deep could not read back what it wrote, so the refusal comes at the root rather than
/// halfway through the body.
const MULTISTATUS_DEPTH: u16 = 6;

/// How deep one property element sits, which is what a kept fragment nests below.
const PROPERTY_DEPTH: u16 = 5;

/// The most octets a reference may occupy, `&` and `;` included.
///
/// The ceiling `crate::internal::dav::text` scans a reference under, restated here because that constant is
/// private to the module that reads: a `&` followed by megabytes of digits is not a reference
/// anybody wrote, and it must not be written either.
const MAX_REFERENCE_BYTES: usize = 12;

/// A multistatus written one response at a time.
///
/// The encoding primitive, of which [`MultiStatus`]'s own [`WriteXml`] is one consumer. A server
/// enumerating a collection it cannot hold pushes each response as it is produced and retains
/// one, which is the only answer that works when the collection is larger than the machine —
/// RFC 4791 gives a `REPORT` no pagination, so the alternative is not a smaller body but no
/// answer at all.
///
/// [`MultiStatusWriter::finish`] closes the root and must be called: an encoder cannot close it
/// from `Drop`, because writing can fail and a failing `Drop` has nowhere to report. A writer
/// dropped without it leaves an unterminated document, which is the honest outcome — the octets
/// already handed to the sink are gone, and pretending otherwise would mean buffering the body
/// this type exists not to buffer.
pub struct MultiStatusWriter<'a> {
    /// The stack-balanced document writer retained across streamed responses.
    writer: XmlWriter<'a>,
    /// The bounds every response is written under.
    limits: Limits,
    /// How many responses have been written.
    written: u32,
}

impl<'a> MultiStatusWriter<'a> {
    /// Open a multistatus document: the declaration, the root, and its three declarations.
    pub fn new(
        out: &'a mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<Self, DavError> {
        if limits.max_xml_depth() < MULTISTATUS_DEPTH {
            return Err(DavError::Limit(LimitExceeded::Depth));
        }
        let mut writer = XmlWriter::new(out);
        writer.open(ElementName::Multistatus, meter)?;
        writer.begin_content(meter)?;
        Ok(Self {
            writer,
            limits,
            written: 0,
        })
    }

    /// Write one more response.
    pub fn push(&mut self, response: &DavResponse, meter: &mut Meter) -> Result<(), DavError> {
        write_dav_response(&mut self.writer, self.limits, meter, response)?;
        self.written = self.written.saturating_add(1);
        Ok(())
    }

    /// How many responses have been written.
    #[must_use]
    pub const fn written(&self) -> u32 {
        self.written
    }

    /// Close the document, carrying the RFC 6578 token after the responses if there is one.
    ///
    /// The token goes last because RFC 6578 section 3 puts it last, which is also what lets a
    /// reader answer [`crate::internal::dav::ResponseSource::sync_token`] only once it has been drained.
    pub fn finish(
        mut self,
        sync_token: Option<&SyncToken>,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        if let Some(token) = sync_token {
            let octets = token.as_bytes();
            check_text_bound(octets, meter)?;
            self.writer
                .element_text(ElementName::SyncToken, octets, meter)?;
        }
        self.writer.close(meter)
    }

    /// Close the document by naming the collection itself insufficient, RFC 4918 section 11.5.
    ///
    /// The answer a server has when the collection is larger than what it can encode. It is not
    /// a good answer and this crate does not pretend otherwise: RFC 4791 gives a `REPORT` no
    /// pagination and no way to say "there was more", so a truncated enumeration cannot be
    /// signaled as truncated. What `507` on the collection's own `href` does say — truthfully,
    /// in the grammar the client is already reading — is that this resource has an answer the
    /// server could not produce, which is strictly better than a body that looks complete.
    ///
    /// The refusal is not charged against the response count, because it is what a server sends
    /// *when* that count is reached and a bound that swallowed its own escape hatch would leave
    /// the server with nothing to say.
    pub fn finish_insufficient_storage(
        mut self,
        collection: &Href,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let limits = self.limits;
        self.writer.open(ElementName::Response, meter)?;
        write_href(&mut self.writer, limits, meter, collection)?;
        write_status(&mut self.writer, meter, Status::INSUFFICIENT_STORAGE)?;
        self.writer.close(meter)?;
        self.writer.close(meter)
    }
}

impl Debug for MultiStatusWriter<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        // The sink is a trait object with no `Debug` bound, and adding one would forbid a
        // caller's own sink from being anything it likes.
        formatter
            .debug_struct("MultiStatusWriter")
            .field("limits", &self.limits)
            .field("written", &self.written)
            .finish_non_exhaustive()
    }
}

impl WriteXml for MultiStatus {
    /// Write the whole body, through the incremental encoder rather than beside it.
    ///
    /// One path, for the reason [`MultiStatus::read`] has one: a private fast path is a second
    /// implementation, and two implementations of one wire format drift.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut writer = MultiStatusWriter::new(out, limits, meter)?;
        for response in self.responses() {
            writer.push(response, meter)?;
        }
        writer.finish(self.sync_token.as_ref(), meter)
    }
}

impl WriteXml for DavResponse {
    /// Write one `DAV:response`, RFC 4918 section 14.24.
    ///
    /// A fragment: the prefixes it uses are the ones the root declared.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut writer = XmlWriter::fragment(out);
        write_dav_response(&mut writer, limits, meter, self)
    }
}

impl WriteXml for ResponseBody {
    /// Write either the one status the resource has or the groups its properties have.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut writer = XmlWriter::fragment(out);
        write_response_body(&mut writer, limits, meter, self)
    }
}

impl WriteXml for PropStat {
    /// Write one `DAV:propstat`: the properties, then the status they came back under.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut writer = XmlWriter::fragment(out);
        write_propstat(&mut writer, limits, meter, self)
    }
}

impl WriteXml for DavProperty {
    /// Write one property element, empty when its value is.
    ///
    /// An empty element rather than an open-and-close pair, because that is what a `propname`
    /// answer and a `404` group carry and what every server writes for them.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut writer = XmlWriter::fragment(out);
        write_dav_property(&mut writer, limits, meter, self)
    }
}

impl WriteXml for PropValue {
    /// Write what is *inside* a property element, which is what differs between the shapes.
    ///
    /// The element around it belongs to [`DavProperty`], because only the name knows what to
    /// call it.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut writer = XmlWriter::fragment(out);
        write_prop_value(&mut writer, limits, meter, self)
    }
}

impl WriteXml for ErrorBody {
    /// Write a `DAV:error` and the preconditions it names, as the empty elements they are.
    ///
    /// RFC 4791 section 5.3.2.1 and RFC 6638 section 3.2.1 define their refusals as elements
    /// with no content, so this is the whole of the vocabulary — including
    /// `CALDAV:allowed-organizer-scheduling-object-change`, which is how a server says that a
    /// stored copy's `ORGANIZER` moved. That comparison belongs to a server holding both copies
    /// and the wire form belongs here.
    fn write_xml(
        &self,
        out: &mut dyn ByteSink,
        limits: Limits,
        meter: &mut Meter,
    ) -> Result<(), DavError> {
        let mut writer = XmlWriter::fragment(out);
        write_error_body(&mut writer, limits, meter, self)
    }
}

/// Write one `DAV:response` into an existing document or fragment writer.
fn write_dav_response(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    response: &DavResponse,
) -> Result<(), DavError> {
    meter.try_charge_response()?;
    writer.open(ElementName::Response, meter)?;
    write_href(writer, limits, meter, &response.href)?;
    write_response_body(writer, limits, meter, &response.body)?;
    if let Some(error) = response.error.as_ref() {
        write_error_body(writer, limits, meter, error)?;
    }
    writer.close(meter)
}

/// Write the status or property groups that form a response body.
fn write_response_body(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    body: &ResponseBody,
) -> Result<(), DavError> {
    match body {
        ResponseBody::Status(status) => write_status(writer, meter, *status),
        ResponseBody::PropStats(groups) => {
            check_cardinality(
                groups.len(),
                limits.max_props_per_response(),
                LimitExceeded::Properties,
            )?;
            for group in groups {
                write_propstat(writer, limits, meter, group)?;
            }
            Ok(())
        },
    }
}

/// Write one property-status group.
fn write_propstat(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    group: &PropStat,
) -> Result<(), DavError> {
    check_cardinality(
        group.props().len(),
        limits.max_props_per_response(),
        LimitExceeded::Properties,
    )?;
    writer.open(ElementName::Propstat, meter)?;
    writer.open(ElementName::Prop, meter)?;
    for property in group.props() {
        write_dav_property(writer, limits, meter, property)?;
    }
    writer.close(meter)?;
    write_status(writer, meter, group.status)?;
    if let Some(error) = group.error.as_ref() {
        write_error_body(writer, limits, meter, error)?;
    }
    writer.close(meter)
}

/// Write one named property and its value.
fn write_dav_property(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    property: &DavProperty,
) -> Result<(), DavError> {
    let vacant = matches!(property.value, PropValue::Empty);
    match &property.name {
        PropName::Known(name) if vacant => writer.empty(*name, meter),
        PropName::Known(name) => {
            writer.open(*name, meter)?;
            write_prop_value(writer, limits, meter, &property.value)?;
            writer.close(meter)
        },
        PropName::Extension(name) if vacant => writer.empty_extension(name, meter),
        PropName::Extension(name) => {
            writer.open_extension(name, meter)?;
            write_prop_value(writer, limits, meter, &property.value)?;
            writer.close(meter)
        },
    }
}

/// Write the content held by one property.
fn write_prop_value(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    value: &PropValue,
) -> Result<(), DavError> {
    match value {
        PropValue::Empty => Ok(()),
        PropValue::Text(octets) | PropValue::Unmodeled(octets) => {
            check_text_bound(octets, meter)?;
            check_text_is_utf8(octets)?;
            writer.text(octets, meter)
        },
        PropValue::Reference(target) => write_href(writer, limits, meter, target),
        PropValue::Resource(claimed) => write_resource_type(writer, meter, claimed),
        PropValue::Entity(tag) => write_entity_tag(writer, meter, tag),
        PropValue::CalendarData(payload) => write_calendar_data(writer, meter, payload),
        PropValue::Markup(octets) => write_kept(writer, limits, meter, octets),
    }
}

/// Write a `DAV:error` and its empty precondition elements.
fn write_error_body(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    error: &ErrorBody,
) -> Result<(), DavError> {
    check_cardinality(
        error.conditions().len(),
        limits.max_props_per_response(),
        LimitExceeded::Properties,
    )?;
    writer.open(ElementName::Error, meter)?;
    for condition in error.conditions() {
        match condition {
            PropName::Known(name) => writer.empty(*name, meter)?,
            PropName::Extension(name) => writer.empty_extension(name, meter)?,
        }
    }
    writer.close(meter)
}

/// Write a `DAV:status`, which carries a whole status line and means only its code.
fn write_status(
    writer: &mut XmlWriter<'_>,
    meter: &mut Meter,
    status: Status,
) -> Result<(), DavError> {
    let code = status.code();
    let mut line = *b"HTTP/1.1 000 ";
    line[9] = b'0'.saturating_add(u8::try_from(code / 100).unwrap_or(0));
    line[10] = b'0'.saturating_add(u8::try_from((code / 10) % 10).unwrap_or(0));
    line[11] = b'0'.saturating_add(u8::try_from(code % 10).unwrap_or(0));
    writer.element_text(ElementName::Status, &line, meter)
}

/// Write a `DAV:href`, charged and bounded exactly as reading one is.
fn write_href(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    href: &Href,
) -> Result<(), DavError> {
    let octets = href.as_bytes();
    let length = u32::try_from(octets.len()).map_err(|_| LimitExceeded::Href)?;
    if length > limits.max_href_bytes() {
        return Err(DavError::Limit(LimitExceeded::Href));
    }
    writer.element_text(ElementName::Href, octets, meter)
}

/// Write a `DAV:resourcetype`: the claims this crate models, then the ones it kept by name.
fn write_resource_type(
    writer: &mut XmlWriter<'_>,
    meter: &mut Meter,
    claimed: &ResourceType,
) -> Result<(), DavError> {
    if claimed.collection {
        writer.empty(ElementName::Collection, meter)?;
    }
    if claimed.calendar {
        writer.empty(ElementName::Calendar, meter)?;
    }
    if claimed.principal {
        writer.empty(ElementName::Principal, meter)?;
    }
    for other in claimed.others() {
        writer.empty_extension(other, meter)?;
    }
    Ok(())
}

/// Write an entity tag as element content, escaped.
///
/// Escaped rather than handed to [`ETag::write_value`], because the octets between the quotes
/// are the peer's and an `&` among them would otherwise be markup this crate did not intend.
/// A reader resolves the reference and [`ETag::parse`] sees what the server wrote.
fn write_entity_tag(
    writer: &mut XmlWriter<'_>,
    meter: &mut Meter,
    tag: &ETag,
) -> Result<(), DavError> {
    let octets = tag.as_bytes();
    let weakness = if tag.is_weak() { 2 } else { 0 };
    let width = octets.len().saturating_add(2).saturating_add(weakness);
    check_text_width(width, meter)?;
    if tag.is_weak() {
        writer.text(b"W/", meter)?;
    }
    writer.text(b"\"", meter)?;
    writer.text(octets, meter)?;
    writer.text(b"\"", meter)?;
    Ok(())
}

/// Write a `CALDAV:calendar-data` payload so that a conformant reader recovers its octets.
///
/// Every `CR` leaves as `&#13;`, which XML 1.0 section 2.11 does not fold because a reference is
/// resolved after normalization rather than before it. This side of the exchange therefore needs
/// no departure from the specification at all: what a server stored is what a client that
/// resolves references gets back, whichever line endings it stored.
fn write_calendar_data(
    writer: &mut XmlWriter<'_>,
    meter: &mut Meter,
    payload: &CalendarPayload,
) -> Result<(), DavError> {
    let octets = payload.as_bytes();
    check_text_bound(octets, meter)?;
    check_text_is_utf8(octets)?;
    writer.text(octets, meter)
}

/// Refuse octets no XML document can carry, at the door that would otherwise emit them.
///
/// An XML document declares an encoding and this crate's declares UTF-8, so a payload that is
/// not UTF-8 makes the *whole* multistatus unreadable to any conformant processor — not one
/// property, the entire response, and with nothing on the wire to say why. There is no
/// escaping that helps: a character reference names a code point and these octets are not one.
///
/// Two real resources land here and are refused rather than mangled. A `.ics` a store holds in
/// some other encoding is one. The other is a resource `docs/adr/0001` guarantees this
/// workspace round-trips byte for byte: an RFC 5545 fold that falls between the lead octet of
/// a multi-octet character and its continuations. That file is a file this workspace reads and
/// writes losslessly and has **no CalDAV representation at all**, which is a fact about the
/// envelope rather than about the file. A refusal a caller can see is the only honest answer;
/// the other one was a body the peer discards whole.
fn check_text_is_utf8(octets: &[u8]) -> Result<(), DavError> {
    if core::str::from_utf8(octets).is_ok() {
        Ok(())
    } else {
        Err(DavError::Invalid(ValueError::NotUtf8))
    }
}

/// Write the octets of a property whose value is markup this crate has no model for.
fn write_kept(
    writer: &mut XmlWriter<'_>,
    limits: Limits,
    meter: &mut Meter,
    octets: &[u8],
) -> Result<(), DavError> {
    check_text_bound(octets, meter)?;
    check_text_is_utf8(octets)?;
    check_fragment(
        octets,
        limits.max_xml_depth().saturating_sub(PROPERTY_DEPTH),
    )?;
    writer.validated_markup(octets, meter)
}

/// Check one character run against the per-element ceiling.
///
/// The shared writer separately charges every escaped output octet against the aggregate work
/// budget, so this check must not charge the input a second time.
fn check_text_bound(octets: &[u8], meter: &Meter) -> Result<(), DavError> {
    check_text_width(octets.len(), meter)
}

/// Check a computed character-data width against the per-element ceiling.
fn check_text_width(width: usize, meter: &Meter) -> Result<(), DavError> {
    let length = u32::try_from(width).map_err(|_| LimitExceeded::Text)?;
    if length > meter.limits().max_xml_text_bytes() {
        return Err(DavError::Limit(LimitExceeded::Text));
    }
    Ok(())
}

/// Refuse a collection larger than the policy that will read it back admits.
fn check_cardinality(count: usize, cap: u32, dimension: LimitExceeded) -> Result<(), DavError> {
    if count > bounded_cap(cap) {
        return Err(DavError::Limit(dimension));
    }
    Ok(())
}

/// What one tag of a kept fragment does to the nesting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagShape {
    /// `<name>` — one level deeper.
    Open,
    /// `</name>` — one level back.
    Close,
    /// `<name/>` — neither.
    Empty,
}

/// Refuse kept octets that would forge structure this writer did not open.
///
/// A refusal filter rather than a parser: it establishes that the tags balance, that nothing
/// closes an element it did not open, that no tag runs off the end, that the nesting stays
/// inside the caller's depth bound, and that nothing declares, comments, or instructs. It does
/// not establish that the fragment is a valid document, and it is not the place to try.
fn check_fragment(octets: &[u8], depth_budget: u16) -> Result<(), DavError> {
    let mut at = 0;
    let mut depth: u16 = 0;
    while let Some(&byte) = octets.get(at) {
        if byte == b'&' {
            check_reference(octets, at)?;
            at = at.saturating_add(1);
            continue;
        }
        if byte != b'<' {
            at = at.saturating_add(1);
            continue;
        }
        let rest = octets
            .get(at..)
            .ok_or(DavError::Syntax(SyntaxError::Malformed))?;
        let (end, shape) = scan_tag(rest)?;
        depth = apply_shape(shape, depth, depth_budget)?;
        at = at.saturating_add(end).saturating_add(1);
    }
    if depth == 0 {
        Ok(())
    } else {
        Err(DavError::Syntax(SyntaxError::Truncated))
    }
}

/// Find where one tag ends and say what shape it is.
///
/// Quotes are honored, because a `>` inside an attribute value is ordinary and a scan that
/// stopped at it would report a balanced fragment as unbalanced.
fn scan_tag(rest: &[u8]) -> Result<(usize, TagShape), DavError> {
    if rest.starts_with(b"<!".as_slice()) || rest.starts_with(b"<?".as_slice()) {
        // A DOCTYPE, a comment, a CDATA section or a processing instruction. This crate's own
        // reader refuses the first and the last outright, and a body it cannot read back is a
        // body it must not write.
        return Err(DavError::Syntax(SyntaxError::Malformed));
    }
    let mut at = 1;
    let mut quote: Option<u8> = None;
    while let Some(&byte) = rest.get(at) {
        match (quote, byte) {
            (None, b'"' | b'\'') => quote = Some(byte),
            (Some(open), _) if open == byte => quote = None,
            (None, b'>') => return Ok((at, shape_of(rest, at)?)),
            _ => {},
        }
        at = at.saturating_add(1);
    }
    Err(DavError::Syntax(SyntaxError::Truncated))
}

/// Classify a tag that runs from the start of `rest` to the `>` at `end`.
fn shape_of(rest: &[u8], end: usize) -> Result<TagShape, DavError> {
    let inner = rest
        .get(1..end)
        .ok_or(DavError::Syntax(SyntaxError::Malformed))?;
    if inner.is_empty() {
        return Err(DavError::Syntax(SyntaxError::Malformed));
    }
    if inner.starts_with(b"/".as_slice()) {
        Ok(TagShape::Close)
    } else if inner.ends_with(b"/".as_slice()) {
        Ok(TagShape::Empty)
    } else {
        Ok(TagShape::Open)
    }
}

/// Apply one tag to the running nesting, refusing what would leave or exceed it.
fn apply_shape(shape: TagShape, depth: u16, budget: u16) -> Result<u16, DavError> {
    match shape {
        TagShape::Open => {
            let grown = depth
                .checked_add(1)
                .ok_or(DavError::Limit(LimitExceeded::Depth))?;
            if grown > budget {
                return Err(DavError::Limit(LimitExceeded::Depth));
            }
            Ok(grown)
        },
        TagShape::Close => depth
            .checked_sub(1)
            .ok_or(DavError::Syntax(SyntaxError::Malformed)),
        TagShape::Empty => Ok(depth),
    }
}

/// Refuse a `&` that does not begin a reference this crate's own reader would resolve.
///
/// Asking only whether a `;` appears within the scanning ceiling was two bugs at once. A value
/// holding `AT&T` has no `;` nearby and was refused outright, so a response this crate had
/// read could not be written at all; a value holding `a & b; c` has one, so a bare `&` reached
/// the sink and the emitted document was not well-formed XML — this crate's own reader
/// answered `SyntaxError::UndefinedEntity` on it, and so would any other. What has to hold is
/// the actual property: every `&` in a fragment is the start of a reference a reader resolves.
fn check_reference(octets: &[u8], start: usize) -> Result<(), DavError> {
    let window = octets
        .get(start..)
        .map(|rest| {
            rest.get(..MAX_REFERENCE_BYTES.min(rest.len()))
                .unwrap_or(rest)
        })
        .ok_or(DavError::Syntax(SyntaxError::Malformed))?;
    let end = window
        .iter()
        .position(|byte| *byte == b';')
        .ok_or(DavError::Syntax(SyntaxError::Malformed))?;
    let name = window
        .get(1..end)
        .ok_or(DavError::Syntax(SyntaxError::Malformed))?;
    if matches!(name, b"amp" | b"lt" | b"gt" | b"quot" | b"apos") {
        return Ok(());
    }
    // A numeric character reference is resolvable exactly when the XML layer would resolve it,
    // so the two doors agree on what a document may hold rather than on what it may say.
    let mut resolved: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    crate::internal::dav::xml::chars::push_reference(octets, start, &mut resolved)
        .map(|_| ())
        .map_err(DavError::from)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use crate::internal::core::{LimitExceeded, Limits, Meter};

    use super::MultiStatusWriter;
    use crate::internal::dav::codec::WriteXml;
    use crate::internal::dav::element::{ElementName, Namespace};
    use crate::internal::dav::failure::{DavError, SyntaxError};
    use crate::internal::dav::request::PropName;
    use crate::internal::dav::response::{
        CalendarPayload, DavProperty, DavResponse, ErrorBody, MultiStatus, PropStat, PropValue,
    };
    use crate::internal::dav::sink::SliceSink;
    use crate::internal::dav::text::{TextMode, decode_text};
    use crate::internal::dav::value::{ETag, ExtensionName, Href, ResourceType, Status, SyncToken};

    /// The `.ics` the three recorded server exchanges are all carrying, byte for byte.
    ///
    /// The same fixture `tests/calendar_data_collision.rs` proves the read against, so the two
    /// halves of the round trip are asserted over one payload rather than two that might drift.
    const PAYLOAD: &[u8] = include_bytes!("fixtures/calendar-data-payload.ics");

    /// What every document this module writes begins with.
    const PROLOGUE: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" \
xmlns:CS=\"http://calendarserver.org/ns/\">";

    /// What every one of them ends with.
    const EPILOGUE: &[u8] = b"</D:multistatus>";

    /// A multistatus a server builds, and the octets it must send.
    type Case = (
        &'static str,
        fn(Limits, &mut Meter) -> MultiStatus,
        &'static [u8],
    );

    /// A namespace URI, a local name as one deployment spells it, and the row it lands on.
    type Resolution = (
        &'static str,
        &'static [u8],
        &'static [u8],
        Option<ElementName>,
    );

    fn href(path: &[u8], meter: &mut Meter) -> Href {
        Href::new(path, Limits::DEFAULT, meter).unwrap()
    }

    fn encode(body: &MultiStatus, limits: Limits, meter: &mut Meter) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        body.write_xml(&mut out, limits, meter).unwrap();
        out
    }

    /// The octets between an element's start tag and its end tag, or nothing.
    fn span<'a>(body: &'a [u8], open: &[u8], shut: &[u8]) -> Option<&'a [u8]> {
        let start = find(body, open)?.checked_add(open.len())?;
        let rest = body.get(start..)?;
        let end = find(rest, shut)?;
        rest.get(..end)
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// One `href` whose `getetag` came back at `200` and whose `displayname` came back at `404`.
    ///
    /// The ordinary case rather than the exotic one, and the reason there is no flat status: a
    /// client reading this must not show the second as though the server had answered it.
    fn divergent_statuses(limits: Limits, meter: &mut Meter) -> MultiStatus {
        let mut body = MultiStatus::new(limits);
        let mut response =
            DavResponse::with_propstats(href(b"/calendars/ann/work/1.ics", meter), limits);

        let mut found = PropStat::new(Status::OK, limits);
        found
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::Getetag),
                    value: PropValue::Entity(ETag::parse(b"\"5f2b8c1e9a04\"").unwrap()),
                },
                meter,
            )
            .unwrap();
        let mut absent = PropStat::new(Status::NOT_FOUND, limits);
        absent
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::Displayname),
                    value: PropValue::Empty,
                },
                meter,
            )
            .unwrap();
        response.push_propstat(found, meter).unwrap();
        response.push_propstat(absent, meter).unwrap();
        body.push(response, meter).unwrap();
        body
    }

    /// The bare `404` a multiget sends for an `href` the client asked about and nothing holds.
    fn bare_not_found(limits: Limits, meter: &mut Meter) -> MultiStatus {
        let mut body = MultiStatus::new(limits);
        let gone = href(b"/calendars/ann/work/gone.ics", meter);
        body.push(DavResponse::with_status(gone, Status::NOT_FOUND), meter)
            .unwrap();
        body
    }

    /// A collection reporting what it is, RFC 4791 section 4.2.
    fn calendar_collection(limits: Limits, meter: &mut Meter) -> MultiStatus {
        let mut body = MultiStatus::new(limits);
        let mut response =
            DavResponse::with_propstats(href(b"/calendars/ann/work/", meter), limits);
        let mut group = PropStat::new(Status::OK, limits);
        let mut claimed = ResourceType::new(limits);
        claimed.collection = true;
        claimed.calendar = true;
        group
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::Resourcetype),
                    value: PropValue::Resource(claimed),
                },
                meter,
            )
            .unwrap();
        response.push_propstat(group, meter).unwrap();
        body.push(response, meter).unwrap();
        body
    }

    /// An RFC 6578 answer: the changes, then the token, which round-trips uninterpreted.
    fn synchronized(limits: Limits, meter: &mut Meter) -> MultiStatus {
        let mut body = bare_not_found(limits, meter);
        body.sync_token =
            Some(SyncToken::new(b"http://example.invalid/ns/sync/42", limits, meter).unwrap());
        body
    }

    /// The refusal a server writes when a stored copy's `ORGANIZER` moved, RFC 6638 section
    /// 3.2.1 — the one defense a file-level scheduling gate cannot supply.
    fn organizer_refused(limits: Limits, meter: &mut Meter) -> MultiStatus {
        let mut body = MultiStatus::new(limits);
        let mut response =
            DavResponse::with_status(href(b"/calendars/ann/work/1.ics", meter), Status::FORBIDDEN);
        let mut refusal = ErrorBody::new(limits);
        refusal
            .push(
                PropName::Known(ElementName::AllowedOrganizerSchedulingObjectChange),
                meter,
            )
            .unwrap();
        response.error = Some(refusal);
        body.push(response, meter).unwrap();
        body
    }

    /// A property this crate has no model for, kept and handed back on.
    fn proxied_property(limits: Limits, meter: &mut Meter) -> MultiStatus {
        let mut body = MultiStatus::new(limits);
        let mut response =
            DavResponse::with_propstats(href(b"/calendars/ann/work/", meter), limits);
        let mut group = PropStat::new(Status::OK, limits);
        group
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::SupportedReportSet),
                    value: PropValue::Markup(
                        b"<D:supported-report><D:report><C:calendar-multiget/></D:report>\
</D:supported-report>"
                            .to_vec()
                            .into_boxed_slice(),
                    ),
                },
                meter,
            )
            .unwrap();
        response.push_propstat(group, meter).unwrap();
        body.push(response, meter).unwrap();
        body
    }

    /// A vendor property outside the vocabulary, which Apple's clients really do read.
    fn extension_property(limits: Limits, meter: &mut Meter) -> MultiStatus {
        let mut body = MultiStatus::new(limits);
        let mut response =
            DavResponse::with_propstats(href(b"/calendars/ann/work/", meter), limits);
        let mut group = PropStat::new(Status::OK, limits);
        let name =
            ExtensionName::new(b"http://apple.com/ns/ical/", b"calendar-color", meter).unwrap();
        group
            .push(
                DavProperty {
                    name: PropName::Extension(name),
                    value: PropValue::Text(b"#711A76FF".to_vec().into_boxed_slice()),
                },
                meter,
            )
            .unwrap();
        response.push_propstat(group, meter).unwrap();
        body.push(response, meter).unwrap();
        body
    }

    #[test]
    fn a_multistatus_is_written_as_the_body_a_server_sends() {
        let cases: &[Case] = &[
            (
                "one href, getetag at 200 beside displayname at 404",
                divergent_statuses,
                b"<D:response><D:href>/calendars/ann/work/1.ics</D:href>\
<D:propstat><D:prop><D:getetag>\"5f2b8c1e9a04\"</D:getetag></D:prop>\
<D:status>HTTP/1.1 200 </D:status></D:propstat>\
<D:propstat><D:prop><D:displayname/></D:prop>\
<D:status>HTTP/1.1 404 </D:status></D:propstat></D:response>",
            ),
            (
                "an href the multiget asked about and nothing holds",
                bare_not_found,
                b"<D:response><D:href>/calendars/ann/work/gone.ics</D:href>\
<D:status>HTTP/1.1 404 </D:status></D:response>",
            ),
            (
                "a calendar collection reporting what it is",
                calendar_collection,
                b"<D:response><D:href>/calendars/ann/work/</D:href><D:propstat><D:prop>\
<D:resourcetype><D:collection/><C:calendar/></D:resourcetype></D:prop>\
<D:status>HTTP/1.1 200 </D:status></D:propstat></D:response>",
            ),
            (
                "the sync token after the responses, RFC 6578 section 3",
                synchronized,
                b"<D:response><D:href>/calendars/ann/work/gone.ics</D:href>\
<D:status>HTTP/1.1 404 </D:status></D:response>\
<D:sync-token>http://example.invalid/ns/sync/42</D:sync-token>",
            ),
            (
                "an ORGANIZER change refused as the empty element RFC 6638 defines",
                organizer_refused,
                b"<D:response><D:href>/calendars/ann/work/1.ics</D:href>\
<D:status>HTTP/1.1 403 </D:status>\
<D:error><C:allowed-organizer-scheduling-object-change/></D:error></D:response>",
            ),
            (
                "a property whose value is markup, handed back as the markup it kept",
                proxied_property,
                b"<D:response><D:href>/calendars/ann/work/</D:href><D:propstat><D:prop>\
<D:supported-report-set><D:supported-report><D:report><C:calendar-multiget/></D:report>\
</D:supported-report></D:supported-report-set></D:prop>\
<D:status>HTTP/1.1 200 </D:status></D:propstat></D:response>",
            ),
            (
                "a vendor property, its namespace declared on itself",
                extension_property,
                b"<D:response><D:href>/calendars/ann/work/</D:href><D:propstat><D:prop>\
<X:calendar-color xmlns:X=\"http://apple.com/ns/ical/\">#711A76FF</X:calendar-color>\
</D:prop><D:status>HTTP/1.1 200 </D:status></D:propstat></D:response>",
            ),
        ];

        for (what, build, expected) in cases {
            let limits = Limits::DEFAULT;
            let mut meter = Meter::new(limits);
            let body = build(limits, &mut meter);
            let written = encode(&body, limits, &mut meter);
            let mut wanted: Vec<u8> = Vec::new();
            wanted.extend_from_slice(PROLOGUE);
            wanted.extend_from_slice(expected);
            wanted.extend_from_slice(EPILOGUE);
            assert_eq!(written, wanted, "{what}");
        }
    }

    #[test]
    fn the_incremental_encoder_and_the_owned_one_write_the_same_octets() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let body = divergent_statuses(limits, &mut meter);
        let whole = encode(&body, limits, &mut meter);

        // What a server with no room for the collection does instead: one response at a time,
        // holding one, into a sink it owns.
        let mut streamed: Vec<u8> = Vec::new();
        let mut writer = MultiStatusWriter::new(&mut streamed, limits, &mut meter).unwrap();
        for response in body.responses() {
            writer.push(response, &mut meter).unwrap();
        }
        assert_eq!(writer.written(), 1);
        writer.finish(None, &mut meter).unwrap();
        assert_eq!(streamed, whole);
    }

    #[test]
    fn a_payload_leaves_with_the_carriage_returns_a_conformant_reader_recovers() {
        // The fixture is checked in with its CRs (`.gitattributes` keeps them), and a mangled
        // checkout must fail here rather than pass vacuously.
        assert!(
            PAYLOAD.contains(&b'\r'),
            "the fixture lost its carriage returns"
        );

        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut body = MultiStatus::new(limits);
        let mut response =
            DavResponse::with_propstats(href(b"/calendars/ann/work/1.ics", &mut meter), limits);
        let mut group = PropStat::new(Status::OK, limits);
        group
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::CalendarData(
                        CalendarPayload::from_octets(PAYLOAD, limits, &mut meter).unwrap(),
                    ),
                },
                &mut meter,
            )
            .unwrap();
        response.push_propstat(group, &mut meter).unwrap();
        body.push(response, &mut meter).unwrap();
        let written = encode(&body, limits, &mut meter);

        // Not one literal CR on the wire: section 2.11 would fold every one of them away
        // before any reader saw it, which is exactly what a reference survives.
        let carried = span(&written, b"<C:calendar-data>", b"</C:calendar-data>").unwrap();
        assert!(!carried.contains(&b'\r'));
        assert!(find(carried, b"&#13;").is_some());

        // And read back by this crate's own character-data rules it is the payload, octet for
        // octet, fold included.
        let mut sink = crate::internal::core::IgnoreDiagnostics;
        let recovered = decode_text(carried, TextMode::Verbatim, 0, &mut meter, &mut sink).unwrap();
        assert_eq!(recovered.run.as_bytes(), PAYLOAD);
        assert!(recovered.run.as_bytes().windows(3).any(|at| at == b"\r\n "));
    }

    #[test]
    fn the_written_status_lines_read_back_as_the_codes_they_name() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let body = divergent_statuses(limits, &mut meter);
        let written = encode(&body, limits, &mut meter);

        // The reading direction of the same octets, through the parser a client uses.
        let mut rest: &[u8] = &written;
        let mut codes: Vec<u16> = Vec::new();
        while let Some(line) = span(rest, b"<D:status>", b"</D:status>") {
            codes.push(Status::parse_status_line(line).unwrap().code());
            let consumed = find(rest, b"</D:status>").unwrap().saturating_add(11);
            rest = rest.get(consumed..).unwrap();
        }
        assert_eq!(codes, [200_u16, 404]);

        let tag = span(&written, b"<D:getetag>", b"</D:getetag>").unwrap();
        let read = ETag::parse(tag).unwrap();
        assert_eq!(read.as_bytes(), b"5f2b8c1e9a04");
        assert!(!read.is_weak());
    }

    #[test]
    fn the_fixed_output_prefix_is_not_what_identity_is() {
        // The prefixes three deployed servers write, against the prefix this crate writes. All
        // four spellings are one element; the fifth is a different element however familiar its
        // prefix looks.
        let cases: &[Resolution] = &[
            (
                "this crate",
                b"DAV:",
                b"multistatus",
                Some(ElementName::Multistatus),
            ),
            (
                "SabreDAV's d:",
                b"DAV:",
                b"multistatus",
                Some(ElementName::Multistatus),
            ),
            (
                "Radicale's ns0:",
                b"DAV:",
                b"response",
                Some(ElementName::Response),
            ),
            (
                "Calendar Server's default declaration",
                b"DAV:",
                b"href",
                Some(ElementName::Href),
            ),
            (
                "Radicale's ns1:",
                b"urn:ietf:params:xml:ns:caldav",
                b"calendar-data",
                Some(ElementName::CalendarData),
            ),
            (
                "a familiar prefix over a hostile URI",
                b"http://evil.example/not-dav",
                b"multistatus",
                None,
            ),
        ];
        for (what, uri, local, wanted) in cases {
            assert_eq!(
                ElementName::resolve(Namespace::from_uri(uri), local),
                *wanted,
                "{what}"
            );
        }

        // And what this crate emits for those rows is the prefix it declared for them.
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let written = encode(&bare_not_found(limits, &mut meter), limits, &mut meter);
        assert!(find(&written, b"<D:multistatus xmlns:D=\"DAV:\"").is_some());
    }

    #[test]
    fn a_kept_property_that_would_forge_structure_is_refused() {
        let cases: &[(&str, &[u8], DavError)] = &[
            (
                "closing an element it never opened",
                b"</D:prop></D:response><D:response><D:href>/evil</D:href>",
                DavError::Syntax(SyntaxError::Malformed),
            ),
            (
                "a start tag it never closes",
                b"<D:supported-report>",
                DavError::Syntax(SyntaxError::Truncated),
            ),
            (
                "a tag that runs off the end of the octets",
                b"<D:report",
                DavError::Syntax(SyntaxError::Truncated),
            ),
            (
                "a DOCTYPE, which the reader refuses outright",
                b"<!DOCTYPE x [<!ENTITY a \"b\">]>",
                DavError::Syntax(SyntaxError::Malformed),
            ),
            (
                "a processing instruction",
                b"<?xml-stylesheet href=\"x\"?>",
                DavError::Syntax(SyntaxError::Malformed),
            ),
            (
                "a reference no semicolon terminates",
                b"&aaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                DavError::Syntax(SyntaxError::Malformed),
            ),
        ];
        for (what, octets, wanted) in cases {
            let limits = Limits::DEFAULT;
            let mut meter = Meter::new(limits);
            let value = PropValue::Markup((*octets).to_vec().into_boxed_slice());
            let mut out: Vec<u8> = Vec::new();
            assert_eq!(
                value.write_xml(&mut out, limits, &mut meter),
                Err(*wanted),
                "{what}"
            );
        }
    }

    #[test]
    fn a_kept_fragment_nests_no_deeper_than_the_caller_admits() {
        // Six is the depth of the multistatus itself, so a property's own children get one
        // level under a policy that admits exactly six, and the fragment below wants two.
        let limits = Limits::DEFAULT.with_max_xml_depth(6);
        let mut meter = Meter::new(limits);
        let value = PropValue::Markup(
            b"<D:supported-report><D:report><C:calendar-multiget/></D:report>\
</D:supported-report>"
                .to_vec()
                .into_boxed_slice(),
        );
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            value.write_xml(&mut out, limits, &mut meter),
            Err(DavError::Limit(LimitExceeded::Depth))
        );
    }

    #[test]
    fn a_property_name_that_is_not_a_name_is_refused_rather_than_written() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        // A peer's property name is octets a proxying server would otherwise paste into its own
        // body. There is no escaping for a name, so there is no writing this one.
        let forged = ExtensionName::new(
            b"http://x.invalid/ns",
            b"x/><D:href>/evil</D:href><D:x",
            &mut meter,
        )
        .unwrap();
        let property = DavProperty {
            name: PropName::Extension(forged),
            value: PropValue::Empty,
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            property.write_xml(&mut out, limits, &mut meter),
            Err(DavError::Syntax(SyntaxError::Malformed))
        );
        assert!(out.is_empty());
    }

    #[test]
    fn the_response_count_is_bounded_on_the_way_out_as_it_is_on_the_way_in() {
        // One number for both directions: a policy that admits writing three responses and
        // refuses reading them back describes an exchange nobody can complete.
        let limits = Limits::DEFAULT.with_max_responses(2);
        let mut meter = Meter::new(limits);
        let mut out: Vec<u8> = Vec::new();
        let mut writer = MultiStatusWriter::new(&mut out, limits, &mut meter).unwrap();
        for index in 0..2_u8 {
            let path = alloc::format!("/calendars/ann/work/{index}.ics");
            let response =
                DavResponse::with_status(href(path.as_bytes(), &mut meter), Status::NOT_FOUND);
            writer.push(&response, &mut meter).unwrap();
        }
        let extra = DavResponse::with_status(
            href(b"/calendars/ann/work/3.ics", &mut meter),
            Status::NOT_FOUND,
        );
        assert_eq!(
            writer.push(&extra, &mut meter),
            Err(DavError::Limit(LimitExceeded::Responses))
        );
    }

    #[test]
    fn a_collection_a_server_cannot_afford_is_closed_with_the_507_it_can_name() {
        let limits = Limits::DEFAULT.with_max_responses(1);
        let mut meter = Meter::new(limits);
        let mut out: Vec<u8> = Vec::new();
        let mut writer = MultiStatusWriter::new(&mut out, limits, &mut meter).unwrap();
        let first =
            DavResponse::with_status(href(b"/calendars/ann/work/1.ics", &mut meter), Status::OK);
        writer.push(&first, &mut meter).unwrap();

        let collection = href(b"/calendars/ann/work/", &mut meter);
        let refused = DavResponse::with_status(collection.clone(), Status::OK);
        assert_eq!(
            writer.push(&refused, &mut meter),
            Err(DavError::Limit(LimitExceeded::Responses))
        );
        // The escape hatch is not charged against the count that closed: a server with nothing
        // left to say would have nothing to send.
        writer
            .finish_insufficient_storage(&collection, &mut meter)
            .unwrap();
        let tail = b"<D:response><D:href>/calendars/ann/work/</D:href>\
<D:status>HTTP/1.1 507 </D:status></D:response></D:multistatus>";
        assert!(out.ends_with(tail));
    }

    #[test]
    fn a_property_count_past_the_bound_is_refused_on_the_way_out() {
        let generous = Limits::DEFAULT;
        let mut meter = Meter::new(generous);
        let body = divergent_statuses(generous, &mut meter);
        // The same value, written under a policy that will not read two property groups back.
        let strict = Limits::DEFAULT.with_max_props_per_response(1);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            body.write_xml(&mut out, strict, &mut meter),
            Err(DavError::Limit(LimitExceeded::Properties))
        );
    }

    #[test]
    fn an_href_past_the_bound_is_refused_on_the_way_out() {
        let generous = Limits::DEFAULT;
        let mut meter = Meter::new(generous);
        let body = bare_not_found(generous, &mut meter);
        let strict = Limits::DEFAULT.with_max_href_bytes(8);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            body.write_xml(&mut out, strict, &mut meter),
            Err(DavError::Limit(LimitExceeded::Href))
        );
    }

    #[test]
    fn a_document_deeper_than_the_policy_admits_is_refused_at_the_root() {
        let limits = Limits::DEFAULT.with_max_xml_depth(4);
        let mut meter = Meter::new(limits);
        let mut out: Vec<u8> = Vec::new();
        assert!(matches!(
            MultiStatusWriter::new(&mut out, limits, &mut meter),
            Err(DavError::Limit(LimitExceeded::Depth))
        ));
        assert!(out.is_empty());
    }

    #[test]
    fn fixed_response_markup_is_charged_against_the_wire_budget() {
        let limits = Limits::DEFAULT;
        let mut build_meter = Meter::new(limits);
        let mut body = MultiStatus::new(limits);
        body.push(
            DavResponse::with_status(href(b"/", &mut build_meter), Status::OK),
            &mut build_meter,
        )
        .unwrap();

        // The href alone fits. A writer charging the complete wire representation must refuse
        // before emitting the declaration and root around it.
        let mut meter = Meter::with_budget(limits, 1);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            body.write_xml(&mut out, limits, &mut meter),
            Err(DavError::Limit(LimitExceeded::Budget))
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_sink_with_no_room_reports_it_rather_than_writing_a_prefix() {
        // The shape a device with 64 KB has: a caller-owned buffer and no allocator at all.
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let body = bare_not_found(limits, &mut meter);
        let mut buffer = [0_u8; 64];
        let mut sink = SliceSink::new(&mut buffer);
        assert!(matches!(
            body.write_xml(&mut sink, limits, &mut meter),
            Err(DavError::Output(_))
        ));
    }

    #[test]
    fn a_weak_entity_tag_keeps_its_weakness_across_the_wire() {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut out: Vec<u8> = Vec::new();
        let value = PropValue::Entity(ETag::parse(b"W/\"5f2b8c1e9a04\"").unwrap());
        value.write_xml(&mut out, limits, &mut meter).unwrap();
        assert_eq!(out, b"W/\"5f2b8c1e9a04\"");
        // RFC 9110 section 8.8.3.2's two comparisons are different questions, and the one a
        // conditional PUT asks must still fail after the round trip.
        let read = ETag::parse(&out).unwrap();
        assert!(read.is_weak());
        assert!(!read.strongly_matches(&ETag::parse(b"\"5f2b8c1e9a04\"").unwrap()));
    }
}
