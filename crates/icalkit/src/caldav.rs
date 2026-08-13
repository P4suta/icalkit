// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sans-I/O CalDAV client and server workflow vocabulary.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};

use core::str;

use ical_core::{
    CivilDateTime, Component, ContentLineReader, Diagnostic, Document, Instant, Item, Meter,
    Severity, UtcOffset,
};
use ical_dav::{
    CalendarDataRequest, CalendarPayload, DavProperty, DavResponse, DecodeContext, ETag,
    ElementName, ExtensionName, Href, MultiStatus, MultiStatusReader, Namespace, PropFind,
    PropName, PropRequest, PropStat, PropValue, RequestBody, ResponseBody, Status, SyncCollection,
    SyncToken as DavSyncToken, UnknownPolicy, WriteXml, XmlEvent, XmlPull, XmlReader, XmlWriter,
};
use ical_query::{Budget, Reduction, Selection, Zones};
use ical_tz::{
    AnswerBasis, LocalResolution, OffsetAnswer, Reading, ZoneAnswer, ZoneProvenance, ZoneSource,
};

use crate::scheduling::Message;
use crate::time::{LocalKind, ZoneDatabase};
use crate::{Calendar, Engine, Error, ResourcePolicy, Session};

/// A CalDAV calendar-query with its XML vocabulary kept private.
#[derive(Clone, Debug)]
pub struct Query {
    query: ical_dav::CalendarQuery,
    query_zone: Option<String>,
}

impl Query {
    /// Strictly read one RFC 4791 calendar-query body using secure defaults.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let policy = ResourcePolicy::secure();
        let mut meter = Meter::new(policy.limits);
        Self::parse_with_policy(bytes, policy, &mut meter)
    }

    pub(crate) fn parse_with_policy(
        bytes: &[u8],
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<Self, Error> {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(policy.limits, meter, &mut diagnostics)
            .with_unknown(UnknownPolicy::Reject);
        let mut events = XmlReader::new(bytes);
        let RequestBody::CalendarQuery(query) = RequestBody::read(&mut events, &mut context)
            .map_err(|_| Error::single("icalkit.caldav.query-invalid"))?
        else {
            return Err(Error::single("icalkit.caldav.query-invalid"));
        };
        let query_zone = read_query_zone(&query, policy, context.meter)?;
        Ok(Self { query, query_zone })
    }

    /// Evaluate this filter without collapsing an unknown answer to a non-match.
    pub fn matches(&self, session: &mut Session<'_>, calendar: &Calendar) -> Result<Match, Error> {
        let Some(filter) = self.query.filter.as_ref() else {
            return Ok(Match::Matched);
        };
        let source = ZoneAdapter(session.engine.zone_database());
        let mut zones = Zones::new(&source);
        if let Some(query_zone) = self.query_zone.as_deref() {
            zones = zones.with_query_zone(query_zone);
        }
        let mut budget = Budget::new(session.engine.policy.limits, &mut session.meter);
        ical_query::matches(filter, &calendar.document, zones, &mut budget)
            .map(Match::from_kernel)
            .map_err(|_| Error::single("icalkit.caldav.query-evaluation"))
    }

    /// Apply this query's `calendar-data` selection without turning partial data into a
    /// persistable [`Calendar`].
    pub fn project(
        &self,
        session: &mut Session<'_>,
        calendar: &Calendar,
    ) -> Result<ProjectedCalendar, Error> {
        let request = self
            .query
            .props
            .calendar_data
            .as_ref()
            .ok_or_else(|| Error::single("icalkit.caldav.calendar-data-not-requested"))?;
        if request.expand.is_some() {
            return Err(Error::single("icalkit.caldav.projection-unsupported"));
        }

        let mut source = Selection::new(calendar.document.clone(), Reduction::FAITHFUL);
        let mut budget = Budget::new(session.engine.policy.limits, &mut session.meter);
        if let Some(window) = request.limit_recurrence_set {
            let zone_adapter = ZoneAdapter(session.engine.zone_database());
            let zones = Zones::new(&zone_adapter);
            source =
                ical_query::limit_recurrence_set_in_window(&source, window, zones, &mut budget)
                    .map_err(|_| Error::single("icalkit.caldav.query-projection"))?;
        }
        let mut selected = ical_query::select(&source, request.comp.as_ref(), &mut budget)
            .map_err(|_| Error::single("icalkit.caldav.query-projection"))?;
        if let Some(window) = request.limit_freebusy_set {
            selected = ical_query::limit_freebusy_set(&selected, window, &mut budget)
                .map_err(|_| Error::single("icalkit.caldav.query-projection"))?;
        }
        Ok(ProjectedCalendar {
            bytes: selected.calendar().to_bytes().into_boxed_slice(),
        })
    }
}

fn read_query_zone(
    query: &ical_dav::CalendarQuery,
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<Option<String>, Error> {
    let Some(payload) = query.timezone.as_ref() else {
        return Ok(None);
    };
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut reader = ContentLineReader::new(payload.as_bytes(), policy.limits.grammar());
    let document = Document::from_tokens(&mut reader, meter, &mut diagnostics)
        .map_err(|_| Error::single("icalkit.caldav.query-timezone-invalid"))?;
    if diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.severity(),
            Severity::Violation | Severity::LimitReached
        )
    }) {
        return Err(Error::single("icalkit.caldav.query-timezone-invalid"));
    }
    let mut found: Option<String> = None;
    for component in document.components() {
        collect_query_tzid(component, &mut found)?;
    }
    found
        .ok_or_else(|| Error::single("icalkit.caldav.query-timezone-invalid"))
        .map(Some)
}

fn collect_query_tzid(component: &Component, found: &mut Option<String>) -> Result<(), Error> {
    if component.is_named(b"VTIMEZONE") {
        for property in component
            .properties()
            .filter(|property| property.is_named(b"TZID"))
        {
            if found.is_some() {
                return Err(Error::single("icalkit.caldav.query-timezone-invalid"));
            }
            let tzid = str::from_utf8(property.value_text().as_bytes())
                .map_err(|_| Error::single("icalkit.caldav.query-timezone-invalid"))?;
            if tzid.is_empty() {
                return Err(Error::single("icalkit.caldav.query-timezone-invalid"));
            }
            *found = Some(tzid.into());
        }
    }
    for child in component.items().iter().filter_map(Item::as_component) {
        collect_query_tzid(child, found)?;
    }
    Ok(())
}

/// The three-valued result of evaluating a CalDAV filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Match {
    /// The calendar satisfies the filter.
    Matched,
    /// The calendar does not satisfy the filter.
    Unmatched,
    /// The filter could not be decided without inventing missing information.
    Undecided,
}

impl Match {
    const fn from_kernel(answer: ical_query::Match) -> Self {
        match answer {
            ical_query::Match::Matched => Self::Matched,
            ical_query::Match::Unmatched => Self::Unmatched,
            ical_query::Match::Undecided(_) => Self::Undecided,
        }
    }
}

/// An opaque RFC 6578 synchronization token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncToken {
    bytes: Box<[u8]>,
}

impl SyncToken {
    /// Retain a server-issued token without interpreting it.
    pub fn new(value: impl AsRef<[u8]>) -> Result<Self, Error> {
        let bytes = value.as_ref();
        if bytes.is_empty()
            || bytes.len() > 4096
            || bytes
                .iter()
                .any(|byte| byte.is_ascii_control() || *byte == b' ')
        {
            return Err(Error::single("icalkit.caldav.sync-token-invalid"));
        }
        Ok(Self {
            bytes: bytes.into(),
        })
    }

    /// The exact token octets to return to the issuing server.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// A resource revision suitable for a conditional write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    uri: String,
    etag: Option<String>,
    absent: bool,
}

impl Revision {
    /// A resource known not to exist, represented by If-None-Match star on write.
    pub fn absent(uri: impl Into<String>) -> Result<Self, Error> {
        let uri = uri.into();
        validate_uri(&uri)?;
        Ok(Self {
            uri,
            etag: None,
            absent: true,
        })
    }

    /// A stored resource at a strong entity tag.
    pub fn stored(uri: impl Into<String>, etag: &str) -> Result<Self, Error> {
        let uri = uri.into();
        validate_uri(&uri)?;
        let parsed = ical_dav::ETag::parse(etag.as_bytes())
            .map_err(|_| Error::single("icalkit.caldav.etag-invalid"))?;
        if parsed.is_weak() {
            return Err(Error::single("icalkit.caldav.weak-etag"));
        }
        let tag = str::from_utf8(parsed.as_bytes())
            .map_err(|_| Error::single("icalkit.caldav.etag-invalid"))?;
        Ok(Self {
            uri,
            etag: Some(format!("\"{tag}\"")),
            absent: false,
        })
    }

    /// The resource URI this revision describes.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// The strong entity tag, including its quotes.
    #[must_use]
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }
}

/// Result of CalDAV service discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    principal: String,
    calendar_home: String,
    scheduling_outbox: Option<String>,
}

impl Discovery {
    /// The authenticated principal resource.
    #[must_use]
    pub fn principal_uri(&self) -> &str {
        &self.principal
    }

    /// The discovered calendar home collection.
    #[must_use]
    pub fn calendar_home_uri(&self) -> &str {
        &self.calendar_home
    }

    /// The RFC 6638 scheduling outbox, when advertised.
    #[must_use]
    pub fn scheduling_outbox_uri(&self) -> Option<&str> {
        self.scheduling_outbox.as_deref()
    }
}

/// One changed or removed member in an incremental synchronization.
#[derive(Clone, Debug)]
pub struct SyncChange {
    href: String,
    removed: bool,
    etag: Option<String>,
    calendar: Option<Calendar>,
}

impl SyncChange {
    /// Resource URI named by the report.
    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }

    /// Whether the report says this member no longer exists.
    #[must_use]
    pub const fn is_removed(&self) -> bool {
        self.removed
    }

    /// Entity tag returned for an updated member.
    #[must_use]
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// Strictly validated full calendar data, when the server returned it.
    #[must_use]
    pub const fn calendar(&self) -> Option<&Calendar> {
        self.calendar.as_ref()
    }
}

/// A complete RFC 6578 incremental synchronization result.
#[derive(Clone, Debug)]
pub struct SyncResult {
    token: Option<SyncToken>,
    changes: Vec<SyncChange>,
}

impl SyncResult {
    /// Token that represents the complete returned state.
    #[must_use]
    pub const fn token(&self) -> Option<&SyncToken> {
        self.token.as_ref()
    }

    /// Updated and removed members in response order.
    #[must_use]
    pub fn changes(&self) -> &[SyncChange] {
        &self.changes
    }
}

/// One recipient result from an RFC 6638 scheduling outbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleDelivery {
    recipient: String,
    request_status: String,
}

impl ScheduleDelivery {
    /// Calendar user address this result describes.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    /// The iTIP request-status value returned by the server.
    #[must_use]
    pub fn request_status(&self) -> &str {
        &self.request_status
    }

    /// Whether the request-status belongs to the successful 2.x class.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.request_status.starts_with("2.")
    }
}

/// Typed recipient outcomes from one scheduling outbox POST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleResponse {
    deliveries: Vec<ScheduleDelivery>,
}

impl ScheduleResponse {
    /// Recipient outcomes in server response order.
    #[must_use]
    pub fn deliveries(&self) -> &[ScheduleDelivery] {
        &self.deliveries
    }
}

struct ZoneAdapter<'a>(Option<&'a dyn ZoneDatabase>);

impl ZoneAdapter<'_> {
    fn reading(&self, tzid: &str, timestamp: jiff::Timestamp) -> Option<Reading> {
        let database = self.0?;
        let offset = database.offset_at(tzid, timestamp)?;
        Some(Reading::new(
            Instant::from_unix_seconds(timestamp.as_second()),
            UtcOffset::from_seconds(offset.seconds())?,
            false,
        ))
    }
}

impl ZoneSource for ZoneAdapter<'_> {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        let database = self.0?;
        let date = local.date();
        let time = local.time();
        let local = jiff::civil::DateTime::new(
            i16::try_from(date.year()).ok()?,
            i8::try_from(date.month()).ok()?,
            i8::try_from(date.day()).ok()?,
            i8::try_from(time.hour()).ok()?,
            i8::try_from(time.minute()).ok()?,
            i8::try_from(time.second()).ok()?,
            0,
        )
        .ok()?;
        let answer = database.resolve_local(tzid, local)?;
        let resolution = match answer.kind() {
            LocalKind::Exact => LocalResolution::Unique {
                reading: self.reading(tzid, answer.earlier()?)?,
            },
            LocalKind::Fold => LocalResolution::Ambiguous {
                earlier: self.reading(tzid, answer.earlier()?)?,
                later: self.reading(tzid, answer.later()?)?,
            },
            // The public port deliberately does not invent the transition bounds and
            // before/after offsets that the internal representation requires for a gap.
            LocalKind::Gap => LocalResolution::Undetermined,
        };
        Some(ZoneAnswer::new(
            resolution,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        let database = self.0?;
        let timestamp = jiff::Timestamp::new(instant.unix_seconds(), 0).ok()?;
        let answer = database.offset_at(tzid, timestamp)?;
        Some(OffsetAnswer::new(
            UtcOffset::from_seconds(answer.seconds())?,
            false,
            ZoneProvenance::CallerDatabase,
            AnswerBasis::Computed,
        ))
    }

    fn recognizes(&self, tzid: &str) -> bool {
        self.0
            .and_then(|database| database.offset_at(tzid, jiff::Timestamp::UNIX_EPOCH))
            .is_some()
    }
}

/// One HTTP header without coupling the API to an HTTP implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    name: String,
    value: Vec<u8>,
}

impl Header {
    /// Build an owned header.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Header name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Header value octets.
    #[must_use]
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

fn validate_uri(uri: &str) -> Result<(), Error> {
    if uri.is_empty()
        || uri
            .as_bytes()
            .iter()
            .any(|byte| byte.is_ascii_control() || *byte == b' ')
    {
        return Err(Error::single("icalkit.caldav.uri-invalid"));
    }
    Ok(())
}

fn header_value<'a>(headers: &'a [Header], name: &str) -> Option<&'a [u8]> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_slice())
}

fn encode_xml(
    value: &dyn WriteXml,
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    value
        .write_xml(&mut body, policy.limits, meter)
        .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
    Ok(body)
}

fn propfind_request(
    uri: &str,
    names: &[ElementName],
    policy: ResourcePolicy,
) -> Result<WireRequest, Error> {
    validate_uri(uri)?;
    let mut meter = Meter::new(policy.limits);
    let mut props = PropRequest::new(policy.limits);
    for name in names {
        props
            .push(PropName::Known(*name), &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
    }
    let body = encode_xml(&PropFind::Props(props), policy, &mut meter)?;
    Ok(WireRequest::new(
        "PROPFIND",
        uri,
        vec![
            Header::new("Depth", b"0".to_vec()),
            Header::new("Content-Type", b"application/xml; charset=utf-8".to_vec()),
        ],
        body,
    ))
}

fn read_multistatus(response: &WireResponse, policy: ResourcePolicy) -> Result<MultiStatus, Error> {
    if response.status != 207 {
        return Err(Error::single("icalkit.caldav.multistatus-expected"));
    }
    let mut meter = Meter::new(policy.limits);
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut context = DecodeContext::new(policy.limits, &mut meter, &mut diagnostics);
    let mut events = XmlReader::new(&response.body);
    let mut source = MultiStatusReader::new(&mut events);
    let body = MultiStatus::read(&mut source, &mut context)
        .map_err(|_| Error::single("icalkit.caldav.response-invalid"))?;
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity() == Severity::LimitReached)
    {
        return Err(Error::single("icalkit.caldav.response-truncated"));
    }
    Ok(body)
}

fn reference_property(
    response: &WireResponse,
    name: ElementName,
    policy: ResourcePolicy,
) -> Result<String, Error> {
    reference_from_body(&read_multistatus(response, policy)?, name)?
        .ok_or_else(|| Error::single("icalkit.caldav.discovery-property-missing"))
}

fn reference_from_body(body: &MultiStatus, name: ElementName) -> Result<Option<String>, Error> {
    let wanted = PropName::Known(name);
    for response in body.responses() {
        if let Some(PropValue::Reference(reference)) = response.successful_value(&wanted) {
            return reference
                .as_str()
                .map(str::to_string)
                .map(Some)
                .map_err(|_| Error::single("icalkit.caldav.href-invalid"));
        }
    }
    Ok(None)
}

fn entity_tag(response: &ical_dav::DavResponse) -> Result<Option<String>, Error> {
    let wanted = PropName::Known(ElementName::Getetag);
    let Some(PropValue::Entity(etag)) = response.successful_value(&wanted) else {
        return Ok(None);
    };
    let tag = str::from_utf8(etag.as_bytes())
        .map_err(|_| Error::single("icalkit.caldav.etag-invalid"))?;
    let weak = if etag.is_weak() { "W/" } else { "" };
    Ok(Some(format!("{weak}\"{tag}\"")))
}

fn calendar_data(response: &ical_dav::DavResponse) -> Result<Option<Calendar>, Error> {
    let wanted = PropName::Known(ElementName::CalendarData);
    let Some(PropValue::CalendarData(payload)) = response.successful_value(&wanted) else {
        return Ok(None);
    };
    Calendar::parse(payload.as_bytes()).map(Some)
}

fn decode_sync(response: &WireResponse, policy: ResourcePolicy) -> Result<SyncResult, Error> {
    let body = read_multistatus(response, policy)?;
    let token = body
        .sync_token
        .as_ref()
        .map(|token| SyncToken::new(token.as_bytes()))
        .transpose()?;
    let mut changes = Vec::new();
    for response in body.responses() {
        let href = response
            .href
            .as_str()
            .map(str::to_string)
            .map_err(|_| Error::single("icalkit.caldav.href-invalid"))?;
        let removed = matches!(
            response.body,
            ResponseBody::Status(status) if matches!(status.code(), 404 | 410)
        );
        changes.push(SyncChange {
            href,
            removed,
            etag: if removed { None } else { entity_tag(response)? },
            calendar: if removed {
                None
            } else {
                calendar_data(response)?
            },
        });
    }
    Ok(SyncResult { token, changes })
}

fn mkcalendar_body(
    display_name: &str,
    description: Option<&str>,
    policy: ResourcePolicy,
) -> Result<Vec<u8>, Error> {
    let mut meter = Meter::new(policy.limits);
    let root = ExtensionName::new(Namespace::CALDAV_URI, b"mkcalendar", &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
    let set = ExtensionName::new(Namespace::DAV_URI, b"set", &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
    let mut body = Vec::new();
    {
        let mut writer = XmlWriter::new(&mut body, &mut meter);
        writer
            .open_extension(&root)
            .and_then(|()| writer.open_extension(&set))
            .and_then(|()| writer.open(ElementName::Prop))
            .and_then(|()| writer.open(ElementName::Displayname))
            .and_then(|()| writer.text(display_name.as_bytes()))
            .and_then(|()| writer.close())
            .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
        if let Some(description) = description {
            writer
                .open(ElementName::CalendarDescription)
                .and_then(|()| writer.text(description.as_bytes()))
                .and_then(|()| writer.close())
                .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
        }
        writer
            .close()
            .and_then(|()| writer.close())
            .and_then(|()| writer.close())
            .and_then(|()| writer.finish())
            .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
    }
    Ok(body)
}

fn validate_header_text(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.as_bytes().iter().any(u8::is_ascii_control) {
        return Err(Error::single("icalkit.caldav.header-value-invalid"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ScheduleCapture {
    Recipient,
    RequestStatus,
}

struct ScheduleParser {
    stage: ScheduleStage,
    capture: Option<ScheduleCapture>,
    recipient: Vec<u8>,
    request_status: Vec<u8>,
    fields: u8,
    deliveries: Vec<ScheduleDelivery>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScheduleStage {
    BeforeRoot,
    Root,
    Response,
    Recipient,
    Finished,
}

impl ScheduleParser {
    fn new() -> Self {
        Self {
            stage: ScheduleStage::BeforeRoot,
            capture: None,
            recipient: Vec::new(),
            request_status: Vec::new(),
            fields: 0,
            deliveries: Vec::new(),
        }
    }

    fn event(&mut self, event: XmlEvent<'_>) -> Result<(), Error> {
        match event {
            XmlEvent::Start { name, depth, .. } => self.start(name, depth),
            XmlEvent::Text(text) => {
                match self.capture {
                    Some(ScheduleCapture::Recipient) => {
                        self.recipient.extend_from_slice(text.run.as_bytes());
                    },
                    Some(ScheduleCapture::RequestStatus) => {
                        self.request_status.extend_from_slice(text.run.as_bytes());
                    },
                    None => {},
                }
                Ok(())
            },
            XmlEvent::End { name, depth, .. } => self.end(name, depth),
        }
    }

    fn start(&mut self, name: ical_dav::QName<'_>, depth: u16) -> Result<(), Error> {
        if depth == 1 && is_name(name, Namespace::CalDav, b"schedule-response") {
            if self.stage != ScheduleStage::BeforeRoot {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.stage = ScheduleStage::Root;
        } else if depth == 2 && is_name(name, Namespace::CalDav, b"response") {
            if self.stage != ScheduleStage::Root {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.stage = ScheduleStage::Response;
            self.recipient.clear();
            self.request_status.clear();
            self.fields = 0;
        } else if depth == 3
            && self.stage == ScheduleStage::Response
            && is_name(name, Namespace::CalDav, b"recipient")
        {
            if self.fields & 1 != 0 {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.fields |= 1;
            self.stage = ScheduleStage::Recipient;
        } else if depth == 4
            && self.stage == ScheduleStage::Recipient
            && is_name(name, Namespace::Dav, b"href")
        {
            self.capture = Some(ScheduleCapture::Recipient);
        } else if depth == 3
            && self.stage == ScheduleStage::Response
            && is_name(name, Namespace::CalDav, b"request-status")
        {
            if self.fields & 2 != 0 {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.fields |= 2;
            self.capture = Some(ScheduleCapture::RequestStatus);
        }
        Ok(())
    }

    fn end(&mut self, name: ical_dav::QName<'_>, depth: u16) -> Result<(), Error> {
        if depth == 4 && is_name(name, Namespace::Dav, b"href") {
            self.capture = None;
        } else if depth == 3 && is_name(name, Namespace::CalDav, b"recipient") {
            self.stage = ScheduleStage::Response;
        } else if depth == 3 && is_name(name, Namespace::CalDav, b"request-status") {
            self.capture = None;
        } else if depth == 2 && is_name(name, Namespace::CalDav, b"response") {
            if self.stage != ScheduleStage::Response || self.fields != 3 {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.deliveries.push(ScheduleDelivery {
                recipient: trimmed_utf8(&self.recipient)?.to_string(),
                request_status: trimmed_utf8(&self.request_status)?.to_string(),
            });
            self.stage = ScheduleStage::Root;
        } else if depth == 1 && is_name(name, Namespace::CalDav, b"schedule-response") {
            if self.stage != ScheduleStage::Root {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.stage = ScheduleStage::Finished;
        }
        Ok(())
    }

    fn finish(self) -> Result<ScheduleResponse, Error> {
        if self.stage != ScheduleStage::Finished {
            return Err(Error::single("icalkit.caldav.schedule-invalid"));
        }
        Ok(ScheduleResponse {
            deliveries: self.deliveries,
        })
    }
}

fn is_name(name: ical_dav::QName<'_>, namespace: Namespace<'_>, local: &[u8]) -> bool {
    name.namespace.is(namespace) && name.local_name == local
}

fn trimmed_utf8(bytes: &[u8]) -> Result<&str, Error> {
    let text =
        str::from_utf8(bytes).map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
    let trimmed = text.trim_matches(|character: char| character.is_ascii_whitespace());
    if trimmed.is_empty() {
        return Err(Error::single("icalkit.caldav.schedule-invalid"));
    }
    Ok(trimmed)
}

fn decode_schedule_response(
    response: &WireResponse,
    policy: ResourcePolicy,
) -> Result<ScheduleResponse, Error> {
    if response.status != 200 {
        return Err(Error::single("icalkit.caldav.schedule-refused"));
    }
    let mut meter = Meter::new(policy.limits);
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut context = DecodeContext::new(policy.limits, &mut meter, &mut diagnostics)
        .with_unknown(UnknownPolicy::Reject);
    let mut events = XmlReader::new(&response.body);
    let mut parser = ScheduleParser::new();
    while let Some(event) = events
        .next_event(&mut context)
        .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?
    {
        parser.event(event)?;
    }
    parser.finish()
}

/// An owned sans-I/O HTTP request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireRequest {
    method: String,
    uri: String,
    headers: Vec<Header>,
    body: Vec<u8>,
}

impl WireRequest {
    /// Build an owned request.
    #[must_use]
    pub fn new(
        method: impl Into<String>,
        uri: impl Into<String>,
        headers: Vec<Header>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            method: method.into(),
            uri: uri.into(),
            headers,
            body,
        }
    }

    /// HTTP/WebDAV method.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Request URI.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Request headers.
    #[must_use]
    pub fn headers(&self) -> &[Header] {
        &self.headers
    }

    /// Request body.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// An owned sans-I/O HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireResponse {
    status: u16,
    headers: Vec<Header>,
    body: Vec<u8>,
}

impl WireResponse {
    /// Build an owned response.
    #[must_use]
    pub fn new(status: u16, headers: Vec<Header>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// HTTP status code.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Response headers.
    #[must_use]
    pub fn headers(&self) -> &[Header] {
        &self.headers
    }

    /// Response body.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// A client workflow factory.
#[derive(Clone, Debug)]
pub struct Client {
    policy: ResourcePolicy,
}

type ResponseDecoder<T> = Box<dyn FnOnce(WireResponse) -> Result<ClientStep<T>, crate::Error>>;

enum ClientStep<T> {
    Request(WireRequest, ResponseDecoder<T>),
    Done(T),
}

impl Client {
    /// Create a sans-I/O client workflow factory.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            policy: ResourcePolicy::secure(),
        }
    }

    /// Start a typed operation without coupling it to an HTTP runtime.
    #[must_use]
    pub fn operation<T>(
        &self,
        request: WireRequest,
        decoder: impl FnOnce(WireResponse) -> Result<T, crate::Error> + 'static,
    ) -> Operation<T> {
        Operation::with_policy(request, decoder, self.policy)
    }

    /// Discover the authenticated principal, calendar home, and scheduling outbox.
    pub fn discover(&self, uri: &str) -> Result<Operation<Discovery>, Error> {
        validate_uri(uri)?;
        let policy = self.policy;
        let request = propfind_request(uri, &[ElementName::CurrentUserPrincipal], policy)?;
        Ok(Operation::from_decoder(
            request,
            Box::new(move |response| {
                let principal =
                    reference_property(&response, ElementName::CurrentUserPrincipal, policy)?;
                let request = propfind_request(
                    &principal,
                    &[ElementName::CalendarHomeSet, ElementName::ScheduleOutboxUrl],
                    policy,
                )?;
                Ok(ClientStep::Request(
                    request,
                    Box::new(move |response| {
                        let body = read_multistatus(&response, policy)?;
                        let calendar_home =
                            reference_from_body(&body, ElementName::CalendarHomeSet)?.ok_or_else(
                                || Error::single("icalkit.caldav.calendar-home-missing"),
                            )?;
                        let scheduling_outbox =
                            reference_from_body(&body, ElementName::ScheduleOutboxUrl)?;
                        Ok(ClientStep::Done(Discovery {
                            principal,
                            calendar_home,
                            scheduling_outbox,
                        }))
                    }),
                ))
            }),
            policy,
        ))
    }

    /// Start an initial or incremental RFC 6578 collection synchronization.
    pub fn sync(
        &self,
        uri: &str,
        token: Option<&SyncToken>,
    ) -> Result<Operation<SyncResult>, Error> {
        validate_uri(uri)?;
        let policy = self.policy;
        let mut meter = Meter::new(policy.limits);
        let mut request_body = SyncCollection::new(policy.limits);
        if let Some(token) = token {
            request_body.token = Some(
                DavSyncToken::new(token.as_bytes(), policy.limits, &mut meter)
                    .map_err(|_| Error::single("icalkit.caldav.sync-token-invalid"))?,
            );
        }
        request_body
            .props
            .push(PropName::Known(ElementName::Getetag), &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
        request_body.props.calendar_data = Some(CalendarDataRequest::default());
        let body = encode_xml(&request_body, policy, &mut meter)?;
        let request = WireRequest::new(
            "REPORT",
            uri,
            vec![
                Header::new("Depth", b"1".to_vec()),
                Header::new("Content-Type", b"application/xml; charset=utf-8".to_vec()),
            ],
            body,
        );
        Ok(Operation::new(request, move |response| {
            decode_sync(&response, policy)
        }))
    }

    /// PUT a validated calendar only if the supplied revision still holds.
    pub fn conditional_write(
        &self,
        revision: &Revision,
        calendar: &Calendar,
    ) -> Result<Operation<Revision>, Error> {
        let body = calendar.to_bytes();
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > self.policy.max_input_bytes() {
            return Err(Error::single("icalkit.caldav.request-too-large"));
        }
        let mut headers = vec![Header::new(
            "Content-Type",
            b"text/calendar; charset=utf-8".to_vec(),
        )];
        if revision.absent {
            headers.push(Header::new("If-None-Match", b"*".to_vec()));
        } else {
            let etag = revision
                .etag
                .as_ref()
                .ok_or_else(|| Error::single("icalkit.caldav.revision-unusable"))?;
            headers.push(Header::new("If-Match", etag.as_bytes().to_vec()));
        }
        let uri = revision.uri.clone();
        let request = WireRequest::new("PUT", uri.clone(), headers, body);
        Ok(Operation::new(request, move |response| {
            if !matches!(response.status, 200 | 201 | 204) {
                return Err(Error::single("icalkit.caldav.write-refused"));
            }
            match header_value(&response.headers, "ETag") {
                Some(etag) => {
                    let etag = str::from_utf8(etag)
                        .map_err(|_| Error::single("icalkit.caldav.etag-invalid"))?;
                    Revision::stored(uri, etag)
                },
                None => Ok(Revision {
                    uri,
                    etag: None,
                    absent: false,
                }),
            }
        }))
    }

    /// Create a calendar collection with optional descriptive properties.
    pub fn mkcalendar(
        &self,
        uri: &str,
        display_name: &str,
        description: Option<&str>,
    ) -> Result<Operation<()>, Error> {
        validate_uri(uri)?;
        if display_name.is_empty() {
            return Err(Error::single("icalkit.caldav.display-name-empty"));
        }
        let body = mkcalendar_body(display_name, description, self.policy)?;
        let request = WireRequest::new(
            "MKCALENDAR",
            uri,
            vec![Header::new(
                "Content-Type",
                b"application/xml; charset=utf-8".to_vec(),
            )],
            body,
        );
        Ok(Operation::new(request, |response| {
            if response.status == 201 {
                Ok(())
            } else {
                Err(Error::single("icalkit.caldav.mkcalendar-refused"))
            }
        }))
    }

    /// Submit an iTIP message to an RFC 6638 scheduling outbox.
    pub fn schedule(
        &self,
        outbox_uri: &str,
        originator: &str,
        recipients: &[&str],
        message: &Message,
    ) -> Result<Operation<ScheduleResponse>, Error> {
        validate_uri(outbox_uri)?;
        validate_header_text(originator)?;
        if recipients.is_empty() {
            return Err(Error::single("icalkit.caldav.recipient-missing"));
        }
        let mut recipient_value = Vec::new();
        for (index, recipient) in recipients.iter().enumerate() {
            validate_header_text(recipient)?;
            if index > 0 {
                recipient_value.extend_from_slice(b", ");
            }
            recipient_value.extend_from_slice(recipient.as_bytes());
        }
        let content_type = format!("text/calendar; charset=utf-8; method={}", message.method());
        let request = WireRequest::new(
            "POST",
            outbox_uri,
            vec![
                Header::new("Content-Type", content_type.into_bytes()),
                Header::new("Originator", originator.as_bytes().to_vec()),
                Header::new("Recipient", recipient_value),
            ],
            message.to_bytes(),
        );
        let policy = self.policy;
        Ok(Operation::new(request, move |response| {
            decode_schedule_response(&response, policy)
        }))
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// A typed client operation driven by requests and supplied responses.
pub struct Operation<T> {
    request: Option<WireRequest>,
    result: Option<Result<T, crate::Error>>,
    decoder: Option<ResponseDecoder<T>>,
    max_response_bytes: u64,
}

impl<T> Operation<T> {
    /// Create an operation from its first request and response decoder.
    #[must_use]
    pub fn new(
        request: WireRequest,
        decoder: impl FnOnce(WireResponse) -> Result<T, crate::Error> + 'static,
    ) -> Self {
        Self::from_decoder(
            request,
            Box::new(move |response| decoder(response).map(ClientStep::Done)),
            ResourcePolicy::secure(),
        )
    }

    fn with_policy(
        request: WireRequest,
        decoder: impl FnOnce(WireResponse) -> Result<T, crate::Error> + 'static,
        policy: ResourcePolicy,
    ) -> Self {
        Self::from_decoder(
            request,
            Box::new(move |response| decoder(response).map(ClientStep::Done)),
            policy,
        )
    }

    fn from_decoder(
        request: WireRequest,
        decoder: ResponseDecoder<T>,
        policy: ResourcePolicy,
    ) -> Self {
        Self {
            request: Some(request),
            result: None,
            decoder: Some(decoder),
            max_response_bytes: policy.limits.max_response_bytes(),
        }
    }

    /// The request the caller should execute next.
    #[must_use]
    pub fn next_request(&self) -> Option<&WireRequest> {
        self.request.as_ref()
    }

    /// Supply the HTTP response to the current request.
    pub fn accept(&mut self, response: WireResponse) -> Result<(), crate::Error> {
        self.request
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-response"))?;
        let decoder = self
            .decoder
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-response"))?;
        if u64::try_from(response.body.len()).unwrap_or(u64::MAX) > self.max_response_bytes {
            self.result = Some(Err(crate::Error::single(
                "icalkit.caldav.response-too-large",
            )));
            return Ok(());
        }
        match decoder(response) {
            Ok(ClientStep::Request(request, decoder)) => {
                self.request = Some(request);
                self.decoder = Some(decoder);
            },
            Ok(ClientStep::Done(result)) => self.result = Some(Ok(result)),
            Err(error) => self.result = Some(Err(error)),
        }
        Ok(())
    }

    /// Finish after all required responses have been supplied.
    pub fn finish(mut self) -> Result<T, crate::Error> {
        self.result
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.operation-incomplete"))?
    }
}

impl<T> Debug for Operation<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Operation")
            .field("has_request", &self.request.is_some())
            .field("has_result", &self.result.is_some())
            .field("max_response_bytes", &self.max_response_bytes)
            .finish_non_exhaustive()
    }
}

/// One validated calendar object supplied by a server's storage adapter.
#[derive(Clone, Debug)]
pub struct StoredResource {
    href: String,
    etag: Option<ETag>,
    calendar: Calendar,
}

impl StoredResource {
    /// Construct a stored object, validating its URI and optional entity tag.
    pub fn new(
        href: impl Into<String>,
        etag: Option<&str>,
        calendar: Calendar,
    ) -> Result<Self, Error> {
        let href = href.into();
        validate_uri(&href)?;
        let etag = etag
            .map(|value| {
                ETag::parse(value.as_bytes())
                    .map_err(|_| Error::single("icalkit.caldav.etag-invalid"))
            })
            .transpose()?;
        Ok(Self {
            href,
            etag,
            calendar,
        })
    }

    /// Resource URI.
    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }

    /// Validated stored calendar.
    #[must_use]
    pub const fn calendar(&self) -> &Calendar {
        &self.calendar
    }
}

fn query_response(
    query: &Query,
    resources: Vec<StoredResource>,
    policy: ResourcePolicy,
) -> Result<WireResponse, Error> {
    let engine = Engine::builder().resource_policy(policy).build();
    let mut session = engine.session();
    let mut meter = Meter::new(policy.limits);
    let mut multistatus = MultiStatus::new(policy.limits);
    for resource in resources {
        match query.matches(&mut session, &resource.calendar)? {
            Match::Unmatched => continue,
            Match::Undecided => {
                return Err(Error::single("icalkit.caldav.query-undecided"));
            },
            Match::Matched => {},
        }
        let href = Href::new(resource.href.as_bytes(), policy.limits, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
        let mut response = DavResponse::with_propstats(href, policy.limits);
        let mut properties = PropStat::new(Status::OK, policy.limits);
        for name in query.query.props.names() {
            if matches!(name, PropName::Known(ElementName::Getetag)) {
                if let Some(etag) = resource.etag.as_ref() {
                    properties
                        .push(
                            DavProperty {
                                name: name.clone(),
                                value: PropValue::Entity(etag.clone()),
                            },
                            &mut meter,
                        )
                        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
                }
            }
        }
        if query.query.props.calendar_data.is_some() {
            let projected = query.project(&mut session, &resource.calendar)?;
            let payload =
                CalendarPayload::from_octets(projected.as_bytes(), policy.limits, &mut meter)
                    .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
            properties
                .push(
                    DavProperty {
                        name: PropName::Known(ElementName::CalendarData),
                        value: PropValue::CalendarData(payload),
                    },
                    &mut meter,
                )
                .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
        }
        response
            .push_propstat(properties, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
        multistatus
            .push(response, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }
    let body = encode_xml(&multistatus, policy, &mut meter)?;
    Ok(WireResponse::new(
        207,
        vec![Header::new(
            "Content-Type",
            b"application/xml; charset=utf-8".to_vec(),
        )],
        body,
    ))
}

/// A server workflow factory.
#[derive(Clone, Debug)]
pub struct Server {
    policy: ResourcePolicy,
}

type ServerResponder = Box<dyn FnOnce(ServerAnswer) -> Result<ServerStep, crate::Error>>;

enum ServerStep {
    Need(ServerNeed, ServerResponder),
    Done(WireResponse),
}

impl Server {
    /// Create a sans-I/O server workflow factory.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            policy: ResourcePolicy::secure(),
        }
    }

    /// Start a server operation whose application dependency is supplied explicitly.
    #[must_use]
    pub fn operation(
        &self,
        need: ServerNeed,
        responder: impl FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error> + 'static,
    ) -> ServerOperation {
        ServerOperation::with_policy(need, responder, self.policy)
    }

    /// Decode a supported request and expose its ACL and storage dependencies as needs.
    pub fn handle(&self, request: WireRequest) -> Result<ServerOperation, Error> {
        if request.method != "REPORT" {
            return Err(Error::single("icalkit.caldav.server-method-unsupported"));
        }
        let mut meter = Meter::new(self.policy.limits);
        let query = Query::parse_with_policy(&request.body, self.policy, &mut meter)?;
        let method = request.method;
        let uri = request.uri;
        let need = ServerNeed::request("caldav.authorize", &method, &uri);
        let policy = self.policy;
        Ok(ServerOperation::from_responder(
            need,
            Box::new(move |answer| {
                let ServerAnswerKind::Authorized(authorized) = answer.kind else {
                    return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
                };
                if !authorized {
                    return Ok(ServerStep::Done(WireResponse::new(
                        403,
                        Vec::new(),
                        Vec::new(),
                    )));
                }
                let need = ServerNeed::request("caldav.query.resources", &method, &uri);
                Ok(ServerStep::Need(
                    need,
                    Box::new(move |answer| {
                        let ServerAnswerKind::Resources(resources) = answer.kind else {
                            return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
                        };
                        query_response(&query, resources, policy).map(ServerStep::Done)
                    }),
                ))
            }),
            policy,
        ))
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

/// A need for application storage, ACL, or routing data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerNeed {
    code: &'static str,
    method: Option<String>,
    uri: Option<String>,
}

impl ServerNeed {
    /// Construct an application need identified by a stable code.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        Self {
            code,
            method: None,
            uri: None,
        }
    }

    fn request(code: &'static str, method: &str, uri: &str) -> Self {
        Self {
            code,
            method: Some(method.to_string()),
            uri: Some(uri.to_string()),
        }
    }

    /// The stable need code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// HTTP/WebDAV method associated with this need, when it came from a request.
    #[must_use]
    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }

    /// Request URI associated with this need.
    #[must_use]
    pub fn uri(&self) -> Option<&str> {
        self.uri.as_deref()
    }
}

/// An application answer supplied to a server workflow.
#[derive(Clone, Debug)]
pub struct ServerAnswer {
    kind: ServerAnswerKind,
}

#[derive(Clone, Debug)]
enum ServerAnswerKind {
    Bytes(Vec<u8>),
    Authorized(bool),
    Resources(Vec<StoredResource>),
}

impl ServerAnswer {
    /// Construct an application answer body.
    #[must_use]
    pub fn new(body: Vec<u8>) -> Self {
        Self {
            kind: ServerAnswerKind::Bytes(body),
        }
    }

    /// Supply an application ACL decision.
    #[must_use]
    pub const fn authorized(allowed: bool) -> Self {
        Self {
            kind: ServerAnswerKind::Authorized(allowed),
        }
    }

    /// Supply validated resources loaded by the application storage adapter.
    #[must_use]
    pub fn resources(resources: Vec<StoredResource>) -> Self {
        Self {
            kind: ServerAnswerKind::Resources(resources),
        }
    }

    /// Application answer octets.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        match &self.kind {
            ServerAnswerKind::Bytes(body) => body,
            ServerAnswerKind::Authorized(_) | ServerAnswerKind::Resources(_) => &[],
        }
    }
}

/// A server operation separated from storage and ACL decisions.
pub struct ServerOperation {
    need: Option<ServerNeed>,
    response: Option<WireResponse>,
    responder: Option<ServerResponder>,
    max_answer_bytes: u64,
}

impl ServerOperation {
    /// Create an operation with one application need.
    #[must_use]
    pub fn new(
        need: ServerNeed,
        responder: impl FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error> + 'static,
    ) -> Self {
        Self::with_policy(need, responder, ResourcePolicy::secure())
    }

    fn with_policy(
        need: ServerNeed,
        responder: impl FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error> + 'static,
        policy: ResourcePolicy,
    ) -> Self {
        Self::from_responder(
            need,
            Box::new(move |answer| responder(answer).map(ServerStep::Done)),
            policy,
        )
    }

    fn from_responder(
        need: ServerNeed,
        responder: ServerResponder,
        policy: ResourcePolicy,
    ) -> Self {
        Self {
            need: Some(need),
            response: None,
            responder: Some(responder),
            max_answer_bytes: policy.max_input_bytes(),
        }
    }

    /// The storage, ACL, or routing fact needed next.
    #[must_use]
    pub const fn next_need(&self) -> Option<&ServerNeed> {
        self.need.as_ref()
    }

    /// Supply the application-owned answer.
    pub fn supply(&mut self, answer: ServerAnswer) -> Result<(), crate::Error> {
        self.need
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-answer"))?;
        let responder = self
            .responder
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-answer"))?;
        if let ServerAnswerKind::Bytes(body) = &answer.kind
            && u64::try_from(body.len()).unwrap_or(u64::MAX) > self.max_answer_bytes
        {
            return Err(crate::Error::single("icalkit.caldav.answer-too-large"));
        }
        match responder(answer)? {
            ServerStep::Need(need, responder) => {
                self.need = Some(need);
                self.responder = Some(responder);
            },
            ServerStep::Done(response) => self.response = Some(response),
        }
        Ok(())
    }

    /// Finish after all application needs have been supplied.
    pub fn finish(mut self) -> Result<WireResponse, crate::Error> {
        self.response
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.server-operation-incomplete"))
    }
}

impl Debug for ServerOperation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerOperation")
            .field("has_need", &self.need.is_some())
            .field("has_response", &self.response.is_some())
            .field("max_answer_bytes", &self.max_answer_bytes)
            .finish_non_exhaustive()
    }
}

/// A projected query result that cannot be passed to persistence APIs as a [`Calendar`](crate::Calendar).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedCalendar {
    bytes: Box<[u8]>,
}

impl ProjectedCalendar {
    /// Serialized projected calendar data.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
