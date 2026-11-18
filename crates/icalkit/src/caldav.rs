// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sans-I/O CalDAV client and server workflow vocabulary.

mod resource;
mod rscale;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};

use core::str;

use crate::internal::core::codec::RequestStatus;
use crate::internal::core::{
    CivilDateTime, Component, DecodeValue as _, Diagnostic, DiagnosticCode, Instant, LimitExceeded,
    Meter, Severity, UriValue, ValueType,
};
use crate::internal::dav::{
    CalendarDataRequest, CalendarPayload, DavProperty, DavResponse, DecodeContext, ETag,
    ElementName, ExtensionName, Href, MultiStatus, MultiStatusReader, Namespace, PropFind,
    PropName, PropRequest, PropStat, PropValue, RequestBody, ResponseBody, Status, SyncCollection,
    SyncToken as DavSyncToken, UnknownPolicy, WriteXml, XmlEvent, XmlPull, XmlReader, XmlWriter,
};
use crate::internal::itip::{MediaTypeParams, Method};
use crate::internal::query::{self, Budget, Reduction, Selection, Zones};
use crate::internal::tz::{
    OffsetAnswer, VtimezoneSet, ZoneAnswer, ZoneSource, read_calendar_zones,
};
use crate::scheduling::Message;
use crate::{Calendar, Engine, Error, ResourcePolicy, Session};

/// A CalDAV calendar-query with its XML vocabulary kept private.
#[derive(Clone, Debug)]
pub struct Query {
    query: crate::internal::dav::CalendarQuery,
    query_zone: Option<QueryZoneDefinition>,
}

#[derive(Clone, Debug)]
struct QueryZoneDefinition {
    tzid: String,
    definitions: VtimezoneSet,
}

impl ZoneSource for QueryZoneDefinition {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        self.definitions.table(tzid)?.resolve(tzid, local)
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        self.definitions.table(tzid)?.offset_at(tzid, instant)
    }

    fn recognizes(&self, tzid: &str) -> bool {
        self.definitions.table(tzid).is_some()
    }
}

struct ResourceZones<'a> {
    stored: VtimezoneSet,
    external: &'a dyn ZoneSource,
}

impl<'a> ResourceZones<'a> {
    fn read(
        calendar: &Calendar,
        external: &'a dyn ZoneSource,
        meter: &mut Meter,
        notes: &mut Vec<Diagnostic>,
    ) -> Result<Self, LimitExceeded> {
        let stored = calendar
            .document
            .components()
            .next()
            .map_or_else(VtimezoneSet::new, |root| {
                read_calendar_zones(root, meter, notes)
            });
        meter.admit(|| Ok(Self { stored, external }))
    }

    fn source(&self, tzid: &str) -> &dyn ZoneSource {
        self.stored
            .table(tzid)
            .map_or(self.external, |table| table as &dyn ZoneSource)
    }

    fn for_query<'z>(&'z self, query: Option<&'z QueryZoneDefinition>) -> Zones<'z> {
        let zones = Zones::new(self);
        query.map_or(zones, |zone| zones.with_query_zone_source(&zone.tzid, zone))
    }
}

impl ZoneSource for ResourceZones<'_> {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        self.source(tzid).resolve(tzid, local)
    }

    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer> {
        self.source(tzid).offset_at(tzid, instant)
    }

    fn recognizes(&self, tzid: &str) -> bool {
        self.source(tzid).recognizes(tzid)
    }
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
        let RequestBody::CalendarQuery(query) = decode_dav_request(bytes, policy, meter)
            .map_err(|_| Error::single("icalkit.caldav.query-invalid"))?
        else {
            return Err(Error::single("icalkit.caldav.query-invalid"));
        };
        Self::from_decoded(query, policy, meter)
    }

    fn from_decoded(
        query: crate::internal::dav::CalendarQuery,
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<Self, Error> {
        let query_zone = read_query_zone(&query, policy, meter)?;
        Ok(Self { query, query_zone })
    }

    /// Evaluate this filter without collapsing an unknown answer to a non-match.
    pub fn matches(&self, session: &mut Session<'_>, calendar: &Calendar) -> Result<Match, Error> {
        let Some(filter) = self.query.filter.as_ref() else {
            return Ok(Match::Matched);
        };
        let limits = session.engine.policy.limits;
        session.with_zone_source(|source, meter, notes| {
            let sources = ResourceZones::read(calendar, source, meter, notes)
                .map_err(|_| Error::single("icalkit.caldav.query-evaluation"))?;
            let zones = sources.for_query(self.query_zone.as_ref());
            let mut budget = Budget::new(limits, meter);
            query::matches(filter, &calendar.document, zones, &mut budget)
                .map(Match::from_kernel)
                .map_err(|_| Error::single("icalkit.caldav.query-evaluation"))
        })
    }

    /// Apply this query's `calendar-data` selection without turning partial data into a persistable [`Calendar`].
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
        project_calendar_data(request, session, calendar, self.query_zone.as_ref())
    }
}

fn project_calendar_data(
    request: &CalendarDataRequest,
    session: &mut Session<'_>,
    calendar: &Calendar,
    query_zone: Option<&QueryZoneDefinition>,
) -> Result<ProjectedCalendar, Error> {
    let limits = session.engine.policy.limits;
    session.with_zone_source(|zone_source, meter, notes| {
        let mut source = Selection::new(calendar.document.clone(), Reduction::FAITHFUL);
        let mut budget = Budget::new(limits, meter);
        if let Some(window) = request.expand.or(request.limit_recurrence_set) {
            let sources = ResourceZones::read(calendar, zone_source, budget.meter, notes)
                .map_err(|_| Error::single("icalkit.caldav.query-projection"))?;
            let zones = sources.for_query(query_zone);
            source = if request.expand.is_some() {
                query::expand_calendar(&source, window, zones, &mut budget)
            } else {
                query::limit_recurrence_set_in_window(&source, window, zones, &mut budget)
            }
            .map_err(|_| Error::single("icalkit.caldav.query-projection"))?;
        }
        let mut selected = query::select(&source, request.comp.as_ref(), &mut budget)
            .map_err(|_| Error::single("icalkit.caldav.query-projection"))?;
        if let Some(window) = request.limit_freebusy_set {
            selected = query::limit_freebusy_set(&selected, window, &mut budget)
                .map_err(|_| Error::single("icalkit.caldav.query-projection"))?;
        }
        Ok(ProjectedCalendar {
            bytes: selected.calendar().to_bytes().into_boxed_slice(),
        })
    })
}

fn decode_dav_request(
    bytes: &[u8],
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<RequestBody, crate::internal::dav::DavError> {
    meter.with_live_scope(|meter| {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(policy.limits, meter, &mut diagnostics)
            .with_unknown(UnknownPolicy::Reject);
        let mut events = XmlReader::new(bytes);
        let request = RequestBody::read(&mut events, &mut context)?;
        if events.next_event(&mut context)?.is_some() {
            return Err(crate::internal::dav::DavError::Foreign);
        }
        Ok(request)
    })
}

fn read_query_zone(
    query: &crate::internal::dav::CalendarQuery,
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<Option<QueryZoneDefinition>, Error> {
    let Some(payload) = query.timezone.as_ref() else {
        return Ok(None);
    };
    let bytes = normalize_xml_calendar(payload.as_bytes(), meter)
        .map_err(|_| Error::single("icalkit.caldav.query-timezone-invalid"))?;
    let calendar = crate::calendar::parse_calendar(&bytes, policy, meter)
        .map_err(|_| Error::single("icalkit.caldav.query-timezone-invalid"))?;
    let root = calendar
        .document
        .components()
        .next()
        .ok_or_else(|| Error::single("icalkit.caldav.query-timezone-invalid"))?;
    let mut components = root
        .components()
        .filter(|component| component.is_named(b"VTIMEZONE"));
    let definition = components
        .next()
        .ok_or_else(|| Error::single("icalkit.caldav.query-timezone-invalid"))?;
    if components.next().is_some() {
        return Err(Error::single("icalkit.caldav.query-timezone-invalid"));
    }
    if unsupported_transition_types(definition) {
        return Err(Error::single("icalkit.caldav.query-timezone-unsupported"));
    }
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let definitions = read_calendar_zones(root, meter, &mut diagnostics);
    if meter.is_exhausted()
        || diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.severity(),
                Severity::Violation | Severity::LimitReached
            )
        })
    {
        return Err(Error::single("icalkit.caldav.query-timezone-invalid"));
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code() == DiagnosticCode::VtimezoneRuleUnsupported)
    {
        return Err(Error::single("icalkit.caldav.query-timezone-unsupported"));
    }
    let table = definitions
        .tables()
        .first()
        .filter(|table| !table.is_empty() && !table.is_truncated())
        .ok_or_else(|| Error::single("icalkit.caldav.query-timezone-invalid"))?;
    let tzid = table.tzid().as_str().into();
    Ok(Some(QueryZoneDefinition { tzid, definitions }))
}

fn unsupported_transition_types(definition: &Component) -> bool {
    definition
        .components()
        .filter(|component| component.is_named(b"STANDARD") || component.is_named(b"DAYLIGHT"))
        .flat_map(Component::properties)
        .any(|property| {
            let expected = if property.is_named(b"DTSTART") || property.is_named(b"RDATE") {
                ValueType::DateTime
            } else if property.is_named(b"TZOFFSETFROM") || property.is_named(b"TZOFFSETTO") {
                ValueType::UtcOffset
            } else if property.is_named(b"RRULE") {
                ValueType::Recur
            } else {
                return false;
            };
            property
                .declared_value_type()
                .is_some_and(|value_type| value_type != Ok(expected))
        })
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
    const fn from_kernel(answer: query::Match) -> Self {
        match answer {
            query::Match::Matched => Self::Matched,
            query::Match::Unmatched => Self::Unmatched,
            query::Match::Undecided(_) => Self::Undecided,
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
        let parsed = crate::internal::dav::ETag::parse(etag.as_bytes())
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

    /// Whether this revision represents a resource known not to exist.
    #[must_use]
    pub const fn is_absent(&self) -> bool {
        self.absent
    }
}

/// Result of CalDAV service discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    principal: String,
    calendar_home: String,
    scheduling_outbox: Option<String>,
    rscales: Option<Vec<String>>,
}

impl Discovery {
    /// Construct validated service discovery properties.
    pub fn new(
        principal: impl Into<String>,
        calendar_home: impl Into<String>,
        scheduling_outbox: Option<impl Into<String>>,
    ) -> Result<Self, Error> {
        let principal = principal.into();
        let calendar_home = calendar_home.into();
        let scheduling_outbox = scheduling_outbox.map(Into::into);
        validate_uri(&principal)?;
        validate_uri(&calendar_home)?;
        if let Some(outbox) = scheduling_outbox.as_deref() {
            validate_uri(outbox)?;
        }
        Ok(Self {
            principal,
            calendar_home,
            scheduling_outbox,
            rscales: None,
        })
    }

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
    /// Construct an updated member with validated URI and optional entity tag.
    pub fn updated(
        href: impl Into<String>,
        etag: Option<&str>,
        calendar: Calendar,
    ) -> Result<Self, Error> {
        let href = href.into();
        validate_uri(&href)?;
        let etag = etag.map(canonical_entity_tag).transpose()?;
        Ok(Self {
            href,
            removed: false,
            etag,
            calendar: Some(calendar),
        })
    }

    /// Construct a member removal.
    pub fn removed(href: impl Into<String>) -> Result<Self, Error> {
        let href = href.into();
        validate_uri(&href)?;
        Ok(Self {
            href,
            removed: true,
            etag: None,
            calendar: None,
        })
    }

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
    /// Construct a completed synchronization page ending at `token`.
    pub fn new(token: SyncToken, changes: Vec<SyncChange>) -> Result<Self, Error> {
        if changes.iter().enumerate().any(|(index, change)| {
            changes[..index]
                .iter()
                .any(|earlier| earlier.href == change.href)
        }) {
            return Err(Error::single("icalkit.caldav.sync-result-invalid"));
        }
        Ok(Self {
            token: Some(token),
            changes,
        })
    }

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
    calendar_data: Option<ScheduleCalendarData>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScheduleCalendarData {
    message: Message,
    bytes: Box<[u8]>,
}

impl ScheduleCalendarData {
    fn read(bytes: Vec<u8>, policy: ResourcePolicy, meter: &mut Meter) -> Result<Self, Error> {
        let normalized = normalize_xml_calendar(&bytes, meter)
            .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
        let message = Message::read_with_policy(&normalized, policy, meter)
            .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
        Ok(Self {
            message,
            bytes: bytes.into_boxed_slice(),
        })
    }
}

fn normalize_xml_calendar<'a>(
    bytes: &'a [u8],
    meter: &mut Meter,
) -> Result<Cow<'a, [u8]>, LimitExceeded> {
    let bare_lf = |(index, byte): (usize, &u8)| {
        *byte == b'\n' && index.checked_sub(1).and_then(|index| bytes.get(index)) != Some(&b'\r')
    };
    let extra = bytes
        .iter()
        .enumerate()
        .filter(|entry| bare_lf(*entry))
        .count();
    if extra == 0 {
        return Ok(Cow::Borrowed(bytes));
    }
    let length = meter.admit(|| bytes.len().checked_add(extra).ok_or(LimitExceeded::Budget))?;
    meter.try_charge_bytes(u64::try_from(length).unwrap_or(u64::MAX))?;
    let mut normalized = Vec::new();
    meter.admit(|| {
        normalized
            .try_reserve_exact(length)
            .map_err(|_| LimitExceeded::Budget)
    })?;
    for (index, byte) in bytes.iter().enumerate() {
        if bare_lf((index, byte)) {
            normalized.push(b'\r');
        }
        normalized.push(*byte);
    }
    Ok(Cow::Owned(normalized))
}

impl ScheduleDelivery {
    /// Construct an outcome with a valid status and a matching free-busy reply for success.
    pub fn new(
        recipient: impl Into<String>,
        request_status: impl Into<String>,
        calendar_data: Option<Message>,
    ) -> Result<Self, Error> {
        let calendar_data = calendar_data.map(|message| ScheduleCalendarData {
            bytes: message.to_bytes().into_boxed_slice(),
            message,
        });
        Self::from_data(recipient.into(), request_status.into(), calendar_data)
    }

    fn from_data(
        recipient: String,
        request_status: String,
        calendar_data: Option<ScheduleCalendarData>,
    ) -> Result<Self, Error> {
        if !is_cal_address(&recipient) || RequestStatus::parse(request_status.as_bytes()).is_none()
        {
            return Err(Error::single("icalkit.caldav.schedule-delivery-invalid"));
        }
        if RequestStatus::parse(request_status.as_bytes()).is_some_and(RequestStatus::is_success)
            && calendar_data.is_none()
        {
            return Err(Error::single("icalkit.caldav.schedule-delivery-invalid"));
        }
        if calendar_data.as_ref().is_some_and(|data| {
            data.message
                .busy_time_reply_envelope()
                .is_none_or(|envelope| envelope.recipients.as_slice() != [recipient.as_str()])
        }) {
            return Err(Error::single("icalkit.caldav.schedule-delivery-invalid"));
        }
        Ok(Self {
            recipient,
            request_status,
            calendar_data,
        })
    }

    /// Calendar user address this result describes.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    /// The iTIP `REQUEST-STATUS` value returned by the server.
    #[must_use]
    pub fn request_status(&self) -> &str {
        &self.request_status
    }

    /// Validated `VFREEBUSY` reply with CRLF line endings.
    #[must_use]
    pub const fn calendar_data(&self) -> Option<&Message> {
        match &self.calendar_data {
            Some(data) => Some(&data.message),
            None => None,
        }
    }

    /// Calendar octets before transport line endings are normalized.
    #[must_use]
    pub fn calendar_data_bytes(&self) -> Option<&[u8]> {
        self.calendar_data.as_ref().map(|data| data.bytes.as_ref())
    }

    /// Whether the `REQUEST-STATUS` value has numeric class 2.
    #[must_use]
    pub fn is_success(&self) -> bool {
        RequestStatus::parse(self.request_status.as_bytes()).is_some_and(RequestStatus::is_success)
    }
}

/// Typed recipient outcomes from one scheduling outbox POST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleResponse {
    kind: ScheduleResponseKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScheduleResponseKind {
    Deliveries(Vec<ScheduleDelivery>),
    NoContent,
}

impl ScheduleResponse {
    /// Construct a non-empty set of unique recipient outcomes.
    pub fn new(deliveries: Vec<ScheduleDelivery>) -> Result<Self, Error> {
        let mut recipients = BTreeSet::new();
        if deliveries.is_empty()
            || deliveries
                .iter()
                .any(|delivery| !recipients.insert(&delivery.recipient))
        {
            return Err(Error::single("icalkit.caldav.schedule-response-invalid"));
        }
        Ok(Self {
            kind: ScheduleResponseKind::Deliveries(deliveries),
        })
    }

    /// An HTTP 204 success that supplies no recipient data.
    #[must_use]
    pub const fn no_content() -> Self {
        Self {
            kind: ScheduleResponseKind::NoContent,
        }
    }

    /// Whether the response supplies no recipient outcomes through HTTP 204.
    #[must_use]
    pub const fn is_no_content(&self) -> bool {
        matches!(self.kind, ScheduleResponseKind::NoContent)
    }

    /// Recipient outcomes in server response order.
    #[must_use]
    pub fn deliveries(&self) -> &[ScheduleDelivery] {
        match &self.kind {
            ScheduleResponseKind::Deliveries(deliveries) => deliveries,
            ScheduleResponseKind::NoContent => &[],
        }
    }
}

struct ScheduleIdentity {
    uid: String,
    start: CivilDateTime,
    end: CivilDateTime,
    originator: String,
    recipients: BTreeSet<String>,
}

impl ScheduleIdentity {
    fn of(message: &Message) -> Option<Self> {
        let envelope = message.busy_time_envelope()?;
        Some(Self {
            uid: envelope.uid.into_owned(),
            start: envelope.start,
            end: envelope.end,
            originator: envelope.originator.to_string(),
            recipients: envelope
                .recipients
                .into_iter()
                .map(ToString::to_string)
                .collect(),
        })
    }

    fn validate(&self, response: &ScheduleResponse) -> Result<(), Error> {
        if response.is_no_content() {
            return Ok(());
        }
        if response.deliveries().len() != self.recipients.len()
            || response
                .deliveries()
                .iter()
                .any(|delivery| !self.recipients.contains(delivery.recipient()))
        {
            return Err(Error::single("icalkit.caldav.schedule-response-incomplete"));
        }
        for delivery in response.deliveries() {
            let Some(message) = delivery.calendar_data() else {
                continue;
            };
            let envelope = message
                .busy_time_reply_envelope()
                .ok_or_else(|| Error::single("icalkit.caldav.schedule-invalid"))?;
            if envelope.uid != self.uid
                || envelope.originator != self.originator
                || (delivery.is_success()
                    && (envelope.start > self.start || envelope.end < self.end))
            {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
        }
        Ok(())
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

fn is_cal_address(value: &str) -> bool {
    UriValue::decode_value(value.as_bytes()).is_ok()
}

fn canonical_entity_tag(value: &str) -> Result<String, Error> {
    let parsed =
        ETag::parse(value.as_bytes()).map_err(|_| Error::single("icalkit.caldav.etag-invalid"))?;
    let tag = str::from_utf8(parsed.as_bytes())
        .map_err(|_| Error::single("icalkit.caldav.etag-invalid"))?;
    if parsed.is_weak() {
        Ok(format!("W/\"{tag}\""))
    } else {
        Ok(format!("\"{tag}\""))
    }
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
    let mut meter = Meter::new(policy.limits);
    read_multistatus_metered(response, policy, &mut meter)
}

fn read_multistatus_metered(
    response: &WireResponse,
    policy: ResourcePolicy,
    meter: &mut Meter,
) -> Result<MultiStatus, Error> {
    if response.status != 207 {
        return Err(Error::single("icalkit.caldav.multistatus-expected"));
    }
    meter.with_live_scope(|meter| {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(policy.limits, meter, &mut diagnostics);
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
        let mut context = DecodeContext::new(policy.limits, meter, &mut diagnostics);
        if events
            .next_event(&mut context)
            .map_err(|_| Error::single("icalkit.caldav.response-invalid"))?
            .is_some()
        {
            return Err(Error::single("icalkit.caldav.response-invalid"));
        }
        Ok(body)
    })
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

fn entity_tag(response: &crate::internal::dav::DavResponse) -> Result<Option<String>, Error> {
    let wanted = PropName::Known(ElementName::Getetag);
    let Some(PropValue::Entity(etag)) = response.successful_value(&wanted) else {
        return Ok(None);
    };
    let tag = str::from_utf8(etag.as_bytes())
        .map_err(|_| Error::single("icalkit.caldav.etag-invalid"))?;
    let weak = if etag.is_weak() { "W/" } else { "" };
    Ok(Some(format!("{weak}\"{tag}\"")))
}

fn calendar_data(response: &crate::internal::dav::DavResponse) -> Result<Option<Calendar>, Error> {
    let wanted = PropName::Known(ElementName::CalendarData);
    let Some(PropValue::CalendarData(payload)) = response.successful_value(&wanted) else {
        return Ok(None);
    };
    Calendar::parse(payload.as_bytes()).map(Some)
}

fn decode_sync(
    response: &WireResponse,
    policy: ResourcePolicy,
    collection: &str,
) -> Result<SyncResult, Error> {
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
        if matches!(
            response.body,
            ResponseBody::Status(Status::INSUFFICIENT_STORAGE)
        ) && resource::same_resource(collection, &href)
        {
            if token.is_none() {
                return Err(Error::single("icalkit.caldav.response-truncated"));
            }
            continue;
        }
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
        let mut writer = XmlWriter::new(&mut body);
        writer
            .open_extension(&root, &mut meter)
            .and_then(|()| writer.open_extension(&set, &mut meter))
            .and_then(|()| writer.open(ElementName::Prop, &mut meter))
            .and_then(|()| writer.open(ElementName::Displayname, &mut meter))
            .and_then(|()| writer.text(display_name.as_bytes(), &mut meter))
            .and_then(|()| writer.close(&mut meter))
            .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
        if let Some(description) = description {
            writer
                .open(ElementName::CalendarDescription, &mut meter)
                .and_then(|()| writer.text(description.as_bytes(), &mut meter))
                .and_then(|()| writer.close(&mut meter))
                .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
        }
        writer
            .close(&mut meter)
            .and_then(|()| writer.close(&mut meter))
            .and_then(|()| writer.close(&mut meter))
            .and_then(|()| writer.finish(&mut meter))
            .map_err(|_| Error::single("icalkit.caldav.request-too-large"))?;
    }
    Ok(body)
}

struct MkCalendarRequest {
    body: Vec<u8>,
}

impl MkCalendarRequest {
    fn parse(body: Vec<u8>, policy: ResourcePolicy) -> Result<Self, Error> {
        if body.is_empty() {
            return Ok(Self { body });
        }

        let mut meter = Meter::new(policy.limits);
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut context = DecodeContext::new(policy.limits, &mut meter, &mut diagnostics)
            .with_unknown(UnknownPolicy::Reject);
        let mut events = XmlReader::new(&body);
        let mut root = false;
        let mut set = false;
        let mut prop = false;

        while let Some(event) = events
            .next_event(&mut context)
            .map_err(|_| Error::single("icalkit.caldav.mkcalendar-invalid"))?
        {
            match event {
                XmlEvent::Start { name, depth, .. } => {
                    let valid = match depth {
                        1 => {
                            !root
                                && name.namespace.is(Namespace::CalDav)
                                && name.local_name == b"mkcalendar"
                        },
                        2 => {
                            root && !set
                                && name.namespace.is(Namespace::Dav)
                                && name.local_name == b"set"
                        },
                        3 => {
                            set && !prop
                                && name.namespace.is(Namespace::Dav)
                                && name.local_name == b"prop"
                        },
                        _ => depth >= 4 && prop,
                    };
                    if !valid {
                        return Err(Error::single("icalkit.caldav.mkcalendar-invalid"));
                    }
                    root |= depth == 1;
                    set |= depth == 2;
                    prop |= depth == 3;
                },
                XmlEvent::Text(text) => {
                    if events.depth() <= 3
                        && !text.run.as_bytes().iter().all(u8::is_ascii_whitespace)
                    {
                        return Err(Error::single("icalkit.caldav.mkcalendar-invalid"));
                    }
                },
                XmlEvent::End { .. } => {},
            }
        }

        if !root || !set || !prop {
            return Err(Error::single("icalkit.caldav.mkcalendar-invalid"));
        }
        Ok(Self { body })
    }
}

#[derive(Clone)]
struct ScheduleRequest {
    message: Message,
    originator: String,
    recipients: Vec<String>,
}

impl ScheduleRequest {
    fn parse(headers: &[Header], body: &[u8], policy: ResourcePolicy) -> Result<Self, Error> {
        validate_header_budget(headers, policy, "icalkit.caldav.schedule-request-invalid")?;
        let content_type = required_single_header(
            headers,
            "Content-Type",
            "icalkit.caldav.schedule-request-invalid",
        )?;
        let method = content_type_method(content_type)?;
        let message = Message::read(body)
            .map_err(|_| Error::single("icalkit.caldav.schedule-request-invalid"))?;
        if !method.eq_ignore_ascii_case(message.method()) {
            return Err(Error::single("icalkit.caldav.schedule-request-invalid"));
        }
        let (originator, recipients) = {
            let envelope = message
                .busy_time_envelope()
                .ok_or_else(|| Error::single("icalkit.caldav.schedule-request-invalid"))?;
            (
                envelope.originator.to_string(),
                envelope
                    .recipients
                    .into_iter()
                    .map(ToString::to_string)
                    .collect(),
            )
        };
        Ok(Self {
            message,
            originator,
            recipients,
        })
    }
}

struct PutRequest {
    calendar: Calendar,
    revision: Revision,
}

impl PutRequest {
    fn parse(request: &WireRequest, policy: ResourcePolicy) -> Result<Self, Error> {
        const INVALID: &str = "icalkit.caldav.write-request-invalid";
        validate_header_budget(&request.headers, policy, INVALID)?;
        let content_type = required_single_header(&request.headers, "Content-Type", INVALID)?;
        let content_type = str::from_utf8(content_type).map_err(|_| Error::single(INVALID))?;
        if !content_type
            .split(';')
            .next()
            .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/calendar"))
        {
            return Err(Error::single(INVALID));
        }

        let if_match = optional_single_header(&request.headers, "If-Match", INVALID)?;
        let if_none_match = optional_single_header(&request.headers, "If-None-Match", INVALID)?;
        let revision = match (if_match, if_none_match) {
            (Some(value), None) => {
                let tag = ETag::parse(value).map_err(|_| Error::single(INVALID))?;
                if tag.is_weak() {
                    return Err(Error::single(INVALID));
                }
                let tag = str::from_utf8(tag.as_bytes()).map_err(|_| Error::single(INVALID))?;
                Revision::stored(request.uri.clone(), &format!("\"{tag}\""))
                    .map_err(|_| Error::single(INVALID))?
            },
            (None, Some(value)) => {
                let value = str::from_utf8(value).map_err(|_| Error::single(INVALID))?;
                if value.trim() != "*" {
                    return Err(Error::single(INVALID));
                }
                Revision::absent(request.uri.clone()).map_err(|_| Error::single(INVALID))?
            },
            _ => return Err(Error::single(INVALID)),
        };
        let calendar = Calendar::parse(&request.body).map_err(|error| {
            let unsupported = error
                .issues()
                .iter()
                .any(|issue| issue.code().as_str() == "unsupported-recurrence-semantics")
                && rscale::unsupported_scale_body(&request.body, policy).unwrap_or(false);
            let code = if unsupported {
                "icalkit.caldav.rscale-unsupported"
            } else {
                INVALID
            };
            Error::new(code, error.issues().to_vec())
        })?;
        Ok(Self { calendar, revision })
    }
}

fn validate_header_budget(
    headers: &[Header],
    policy: ResourcePolicy,
    error_code: &'static str,
) -> Result<(), Error> {
    let header_octets = headers.iter().fold(0u64, |total, header| {
        total
            .saturating_add(u64::try_from(header.name.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(header.value.len()).unwrap_or(u64::MAX))
    });
    if header_octets > policy.max_input_bytes() {
        return Err(Error::single(error_code));
    }
    Ok(())
}

fn optional_single_header<'a>(
    headers: &'a [Header],
    name: &str,
    error_code: &'static str,
) -> Result<Option<&'a [u8]>, Error> {
    let mut values = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_slice());
    let value = values.next();
    if values.next().is_some() {
        return Err(Error::single(error_code));
    }
    Ok(value)
}

fn required_single_header<'a>(
    headers: &'a [Header],
    name: &str,
    error_code: &'static str,
) -> Result<&'a [u8], Error> {
    optional_single_header(headers, name, error_code)?.ok_or_else(|| Error::single(error_code))
}

fn content_type_method(value: &[u8]) -> Result<&str, Error> {
    let value = str::from_utf8(value)
        .map_err(|_| Error::single("icalkit.caldav.schedule-request-invalid"))?;
    let mut parts = value.split(';');
    if !parts
        .next()
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/calendar"))
    {
        return Err(Error::single("icalkit.caldav.schedule-request-invalid"));
    }

    let mut method = None;
    for parameter in parts {
        let (name, value) = parameter
            .split_once('=')
            .ok_or_else(|| Error::single("icalkit.caldav.schedule-request-invalid"))?;
        if !name.trim().eq_ignore_ascii_case("method") {
            continue;
        }
        if method.is_some() {
            return Err(Error::single("icalkit.caldav.schedule-request-invalid"));
        }
        let value = value.trim();
        let value = match value.strip_prefix('"') {
            Some(quoted) => quoted
                .strip_suffix('"')
                .ok_or_else(|| Error::single("icalkit.caldav.schedule-request-invalid"))?,
            None => value,
        };
        if value.is_empty()
            || value
                .bytes()
                .any(|octet| !octet.is_ascii_alphanumeric() && octet != b'-')
        {
            return Err(Error::single("icalkit.caldav.schedule-request-invalid"));
        }
        method = Some(value);
    }
    method.ok_or_else(|| Error::single("icalkit.caldav.schedule-request-invalid"))
}

#[derive(Clone, Copy)]
enum ScheduleCapture {
    Recipient,
    RequestStatus,
    CalendarData,
}

struct ScheduleParser {
    stage: ScheduleStage,
    capture: Option<ScheduleCapture>,
    recipient: Vec<u8>,
    request_status: Vec<u8>,
    calendar_data: Vec<u8>,
    calendar_media: Option<MediaTypeParams>,
    skipping: Option<u16>,
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
            calendar_data: Vec::new(),
            calendar_media: None,
            skipping: None,
            fields: 0,
            deliveries: Vec::new(),
        }
    }

    fn event(
        &mut self,
        event: XmlEvent<'_>,
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<(), Error> {
        match event {
            XmlEvent::Start { name, depth, .. } => self.start(name, depth),
            XmlEvent::Text(text) => {
                if self.skipping.is_some() {
                    return Ok(());
                }
                let bytes = text.run.as_bytes();
                let target = match self.capture {
                    Some(ScheduleCapture::Recipient) => Some(&mut self.recipient),
                    Some(ScheduleCapture::RequestStatus) => Some(&mut self.request_status),
                    Some(ScheduleCapture::CalendarData) => Some(&mut self.calendar_data),
                    None => None,
                };
                if let Some(target) = target {
                    let length = target
                        .len()
                        .checked_add(bytes.len())
                        .and_then(|length| u32::try_from(length).ok())
                        .ok_or_else(|| Error::single("icalkit.caldav.schedule-invalid"))?;
                    if length > policy.limits.max_xml_text_bytes() {
                        return Err(Error::single("icalkit.caldav.schedule-invalid"));
                    }
                    meter
                        .try_charge(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                        .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
                    target.extend_from_slice(bytes);
                } else if !bytes.iter().all(u8::is_ascii_whitespace) {
                    return Err(Error::single("icalkit.caldav.schedule-invalid"));
                }
                Ok(())
            },
            XmlEvent::End { name, depth, .. } => self.end(name, depth, policy, meter),
        }
    }

    fn start(&mut self, name: crate::internal::dav::QName<'_>, depth: u16) -> Result<(), Error> {
        if self.skipping.is_some() {
            return Ok(());
        }
        if self.capture.is_some() {
            return Err(Error::single("icalkit.caldav.schedule-invalid"));
        }
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
            self.calendar_data.clear();
            self.calendar_media = None;
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
            if self.fields & 8 != 0 {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.fields |= 8;
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
        } else if depth == 3
            && self.stage == ScheduleStage::Response
            && is_name(name, Namespace::CalDav, b"calendar-data")
        {
            if self.fields & 4 != 0 {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.fields |= 4;
            self.capture = Some(ScheduleCapture::CalendarData);
        } else if matches!(
            self.stage,
            ScheduleStage::BeforeRoot | ScheduleStage::Finished
        ) || (name.namespace.is(Namespace::CalDav)
            && [
                b"schedule-response".as_slice(),
                b"response",
                b"recipient",
                b"request-status",
                b"calendar-data",
            ]
            .contains(&name.local_name))
            || is_name(name, Namespace::Dav, b"href")
        {
            return Err(Error::single("icalkit.caldav.schedule-invalid"));
        } else {
            self.skipping = Some(depth);
        }
        Ok(())
    }

    fn end(
        &mut self,
        name: crate::internal::dav::QName<'_>,
        depth: u16,
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<(), Error> {
        if let Some(skipping) = self.skipping {
            if skipping == depth {
                self.skipping = None;
            }
            return Ok(());
        }
        if depth == 4 && is_name(name, Namespace::Dav, b"href") {
            self.capture = None;
        } else if depth == 3 && is_name(name, Namespace::CalDav, b"recipient") {
            self.stage = ScheduleStage::Response;
        } else if depth == 3
            && (is_name(name, Namespace::CalDav, b"request-status")
                || is_name(name, Namespace::CalDav, b"calendar-data"))
        {
            self.capture = None;
        } else if depth == 2 && is_name(name, Namespace::CalDav, b"response") {
            if self.stage != ScheduleStage::Response || self.fields & 0b1011 != 0b1011 {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            meter
                .try_charge_response()
                .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
            let calendar_data = if self.fields & 4 != 0 {
                if self.calendar_media.as_ref().is_some_and(|media| {
                    media.charset().is_some() && !media.charset_agrees_with(&self.calendar_data)
                }) {
                    return Err(Error::single("icalkit.caldav.schedule-invalid"));
                }
                Some(ScheduleCalendarData::read(
                    core::mem::take(&mut self.calendar_data),
                    policy,
                    meter,
                )?)
            } else {
                None
            };
            let request_status = str::from_utf8(&self.request_status)
                .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
            let delivery = ScheduleDelivery::from_data(
                trimmed_utf8(&self.recipient)?.to_string(),
                request_status.to_string(),
                calendar_data,
            )
            .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
            self.deliveries.push(delivery);
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
        ScheduleResponse::new(self.deliveries)
            .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))
    }

    fn read_calendar_media(
        &mut self,
        events: &dyn XmlPull<'_>,
        policy: ResourcePolicy,
        meter: &mut Meter,
    ) -> Result<(), Error> {
        let attribute = |name| {
            events.attribute(crate::internal::dav::QName::new(
                Namespace::from_uri(b""),
                name,
            ))
        };
        if attribute(b"version").is_some_and(|version| version != b"2.0") {
            return Err(Error::single("icalkit.caldav.schedule-invalid"));
        }
        if let Some(content_type) = attribute(b"content-type") {
            let media = MediaTypeParams::read(content_type, policy.limits, meter)
                .map_err(|_| Error::single("icalkit.caldav.schedule-invalid"))?;
            if !media.is_calendar()
                || (media.method().is_some() && media.declared_method() != Some(Method::Reply))
                || media
                    .component()
                    .is_some_and(|component| !component.eq_ignore_ascii_case(b"VFREEBUSY"))
            {
                return Err(Error::single("icalkit.caldav.schedule-invalid"));
            }
            self.calendar_media = Some(media);
        }
        Ok(())
    }
}

fn is_name(name: crate::internal::dav::QName<'_>, namespace: Namespace<'_>, local: &[u8]) -> bool {
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
    if response.status == 204 {
        return if response.body.is_empty() {
            Ok(ScheduleResponse::no_content())
        } else {
            Err(Error::single("icalkit.caldav.schedule-invalid"))
        };
    }
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
        let data_start = matches!(&event, XmlEvent::Start { name, depth: 3, .. } if is_name(*name, Namespace::CalDav, b"calendar-data"));
        parser.event(event, policy, context.meter)?;
        if data_start && matches!(parser.capture, Some(ScheduleCapture::CalendarData)) {
            parser.read_calendar_media(&events, policy, context.meter)?;
        }
    }
    parser.finish()
}

fn encode_schedule_response(
    response: &ScheduleResponse,
    policy: ResourcePolicy,
) -> Result<Vec<u8>, Error> {
    let mut meter = Meter::new(policy.limits);
    let root = ExtensionName::new(Namespace::CALDAV_URI, b"schedule-response", &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    let response_element = ExtensionName::new(Namespace::CALDAV_URI, b"response", &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    let recipient = ExtensionName::new(Namespace::CALDAV_URI, b"recipient", &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    let request_status = ExtensionName::new(Namespace::CALDAV_URI, b"request-status", &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    let mut body = Vec::new();
    {
        let mut writer = XmlWriter::new(&mut body);
        writer
            .open_extension(&root, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
        for delivery in response.deliveries() {
            writer
                .open_extension(&response_element, &mut meter)
                .and_then(|()| writer.open_extension(&recipient, &mut meter))
                .and_then(|()| writer.open(ElementName::Href, &mut meter))
                .and_then(|()| writer.text(delivery.recipient().as_bytes(), &mut meter))
                .and_then(|()| writer.close(&mut meter))
                .and_then(|()| writer.close(&mut meter))
                .and_then(|()| writer.open_extension(&request_status, &mut meter))
                .and_then(|()| writer.text(delivery.request_status().as_bytes(), &mut meter))
                .and_then(|()| writer.close(&mut meter))
                .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
            if let Some(bytes) = delivery.calendar_data_bytes() {
                writer
                    .open(ElementName::CalendarData, &mut meter)
                    .and_then(|()| writer.text(bytes, &mut meter))
                    .and_then(|()| writer.close(&mut meter))
                    .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
            }
            writer
                .close(&mut meter)
                .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
        }
        writer
            .close(&mut meter)
            .and_then(|()| writer.finish(&mut meter))
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }
    Ok(body)
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
                            rscales: None,
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
        let collection = uri.to_string();
        Ok(Operation::new(request, move |response| {
            decode_sync(&response, policy, &collection)
        }))
    }

    /// PUT a validated calendar only if the supplied revision still holds.
    /// Interpreted `RSCALE` rules require [`Self::conditional_write_with_rscales`].
    pub fn conditional_write(
        &self,
        revision: &Revision,
        calendar: &Calendar,
    ) -> Result<Operation<Revision>, Error> {
        let operation = self.prepare_conditional_write(revision, calendar)?;
        if !rscale::required_scales(calendar, self.policy)?.is_empty() {
            return Err(Error::single(
                "icalkit.caldav.rscale-advertisement-required",
            ));
        }
        Ok(operation)
    }

    fn prepare_conditional_write(
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

    /// Submit a free-busy `REQUEST` to an RFC 6638 scheduling Outbox.
    pub fn schedule(
        &self,
        outbox_uri: &str,
        message: &Message,
    ) -> Result<Operation<ScheduleResponse>, Error> {
        validate_uri(outbox_uri)?;
        let identity = ScheduleIdentity::of(message)
            .ok_or_else(|| Error::single("icalkit.caldav.schedule-request-invalid"))?;
        let content_type = message.imip_content_type();
        let request = WireRequest::new(
            "POST",
            outbox_uri,
            vec![Header::new("Content-Type", content_type.into_bytes())],
            message.to_bytes(),
        );
        let policy = self.policy;
        Ok(Operation::new(request, move |response| {
            let decoded = decode_schedule_response(&response, policy)?;
            identity.validate(&decoded)?;
            Ok(decoded)
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
            if matches!(name, PropName::Known(ElementName::Getetag))
                && let Some(etag) = resource.etag.as_ref()
            {
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

fn sync_change_response(
    request: &SyncCollection,
    change: SyncChange,
    policy: ResourcePolicy,
    session: &mut Session<'_>,
    meter: &mut Meter,
) -> Result<DavResponse, Error> {
    let href = Href::new(change.href.as_bytes(), policy.limits, meter)
        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    if change.removed {
        return Ok(DavResponse::with_status(href, Status::NOT_FOUND));
    }

    let calendar = change
        .calendar
        .ok_or_else(|| Error::single("icalkit.caldav.sync-response-invalid"))?;
    let mut success = PropStat::new(Status::OK, policy.limits);
    let mut missing = PropStat::new(Status::NOT_FOUND, policy.limits);
    for name in request.props.names() {
        if matches!(name, PropName::Known(ElementName::CalendarData)) {
            continue;
        }
        let property = if matches!(name, PropName::Known(ElementName::Getetag)) {
            change
                .etag
                .as_deref()
                .map(|etag| {
                    ETag::parse(etag.as_bytes())
                        .map(|value| DavProperty {
                            name: name.clone(),
                            value: PropValue::Entity(value),
                        })
                        .map_err(|_| Error::single("icalkit.caldav.sync-response-invalid"))
                })
                .transpose()?
        } else {
            None
        };
        let (group, property) = match property {
            Some(property) => (&mut success, property),
            None => (
                &mut missing,
                DavProperty {
                    name: name.clone(),
                    value: PropValue::Empty,
                },
            ),
        };
        group
            .push(property, meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }
    if let Some(calendar_data) = request.props.calendar_data.as_ref() {
        let projected = project_calendar_data(calendar_data, session, &calendar, None)?;
        let payload = CalendarPayload::from_octets(projected.as_bytes(), policy.limits, meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
        success
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::CalendarData(payload),
                },
                meter,
            )
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }

    let mut response = DavResponse::with_propstats(href, policy.limits);
    if !success.props().is_empty() {
        response
            .push_propstat(success, meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }
    if !missing.props().is_empty() {
        response
            .push_propstat(missing, meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }
    if response.propstats().is_empty() {
        response.body = ResponseBody::Status(Status::OK);
    }
    Ok(response)
}

fn sync_response(
    request: &SyncCollection,
    result: SyncResult,
    policy: ResourcePolicy,
) -> Result<WireResponse, Error> {
    if request
        .limit
        .is_some_and(|limit| usize::try_from(limit).unwrap_or(usize::MAX) < result.changes.len())
    {
        return Err(Error::single("icalkit.caldav.sync-response-limit"));
    }
    let token = result
        .token
        .ok_or_else(|| Error::single("icalkit.caldav.sync-response-invalid"))?;
    let mut meter = Meter::new(policy.limits);
    let engine = Engine::builder().resource_policy(policy).build();
    let mut session = engine.session();
    let mut multistatus = MultiStatus::new(policy.limits);
    multistatus.sync_token = Some(
        DavSyncToken::new(token.as_bytes(), policy.limits, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?,
    );

    for change in result.changes {
        let response = sync_change_response(request, change, policy, &mut session, &mut meter)?;
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

fn discovery_response(
    uri: &str,
    request: PropFind,
    discovery: &Discovery,
    policy: ResourcePolicy,
) -> Result<WireResponse, Error> {
    let (mut names, names_only) = match request {
        PropFind::Props(properties) => (properties.names().to_vec(), false),
        PropFind::AllProp(included) => {
            let mut names = vec![
                PropName::Known(ElementName::CurrentUserPrincipal),
                PropName::Known(ElementName::CalendarHomeSet),
                PropName::Known(ElementName::ScheduleOutboxUrl),
            ];
            for name in included.names() {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            (names, false)
        },
        PropFind::Names => (
            vec![
                PropName::Known(ElementName::CurrentUserPrincipal),
                PropName::Known(ElementName::CalendarHomeSet),
                PropName::Known(ElementName::ScheduleOutboxUrl),
            ],
            true,
        ),
    };
    if names_only && discovery.rscales.is_some() {
        names.push(PropName::Known(ElementName::SupportedRscaleSet));
    }
    let mut meter = Meter::new(policy.limits);
    let href = Href::new(uri.as_bytes(), policy.limits, &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    let mut success = PropStat::new(Status::OK, policy.limits);
    let mut missing = PropStat::new(Status::NOT_FOUND, policy.limits);
    for name in names.drain(..) {
        let value = if names_only {
            Some(PropValue::Empty)
        } else if name == PropName::Known(ElementName::SupportedRscaleSet) {
            rscale::advertisement_value(discovery.rscales.as_deref(), policy, &mut meter)?
        } else {
            let target = match &name {
                PropName::Known(ElementName::CurrentUserPrincipal) => {
                    Some(discovery.principal.as_str())
                },
                PropName::Known(ElementName::CalendarHomeSet) => {
                    Some(discovery.calendar_home.as_str())
                },
                PropName::Known(ElementName::ScheduleOutboxUrl) => {
                    discovery.scheduling_outbox.as_deref()
                },
                _ => None,
            };
            target
                .map(|target| Href::new(target.as_bytes(), policy.limits, &mut meter))
                .transpose()
                .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?
                .map(PropValue::Reference)
        };
        let (group, value) = match value {
            Some(value) => (&mut success, value),
            None => (&mut missing, PropValue::Empty),
        };
        group
            .push(DavProperty { name, value }, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }

    let mut response = DavResponse::with_propstats(href, policy.limits);
    if !success.props().is_empty() {
        response
            .push_propstat(success, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }
    if !missing.props().is_empty() {
        response
            .push_propstat(missing, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
    }
    let mut multistatus = MultiStatus::new(policy.limits);
    multistatus
        .push(response, &mut meter)
        .map_err(|_| Error::single("icalkit.caldav.response-too-large"))?;
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
        match request.method.as_str() {
            "REPORT" => self.handle_report(request),
            "MKCALENDAR" => self.handle_mkcalendar(request),
            "POST" => self.handle_schedule(request),
            "PUT" => self.handle_put(request),
            "PROPFIND" => self.handle_propfind(request),
            _ => Err(Error::single("icalkit.caldav.server-method-unsupported")),
        }
    }

    fn handle_report(&self, request: WireRequest) -> Result<ServerOperation, Error> {
        let mut meter = Meter::new(self.policy.limits);
        let report = decode_dav_request(&request.body, self.policy, &mut meter)
            .map_err(|_| Error::single("icalkit.caldav.report-invalid"))?;
        match report {
            RequestBody::CalendarQuery(query) => {
                let query = Query::from_decoded(query, self.policy, &mut meter)?;
                Ok(self.handle_query(request, query))
            },
            RequestBody::SyncCollection(sync) => self.handle_sync(request, sync),
            _ => Err(Error::single("icalkit.caldav.report-unsupported")),
        }
    }

    fn handle_propfind(&self, request: WireRequest) -> Result<ServerOperation, Error> {
        let mut meter = Meter::new(self.policy.limits);
        let RequestBody::PropFind(propfind) =
            decode_dav_request(&request.body, self.policy, &mut meter)
                .map_err(|_| Error::single("icalkit.caldav.propfind-invalid"))?
        else {
            return Err(Error::single("icalkit.caldav.propfind-invalid"));
        };
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
                let need = ServerNeed::request("caldav.discovery.properties", &method, &uri);
                Ok(ServerStep::Need(
                    need,
                    Box::new(move |answer| {
                        let ServerAnswerKind::Discovery(discovery) = answer.kind else {
                            return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
                        };
                        discovery_response(&uri, propfind, &discovery, policy).map(ServerStep::Done)
                    }),
                ))
            }),
            policy,
        ))
    }

    fn handle_query(&self, request: WireRequest, query: Query) -> ServerOperation {
        let method = request.method;
        let uri = request.uri;
        let need = ServerNeed::request("caldav.authorize", &method, &uri);
        let policy = self.policy;
        ServerOperation::from_responder(
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
        )
    }

    fn handle_sync(
        &self,
        request: WireRequest,
        sync: SyncCollection,
    ) -> Result<ServerOperation, Error> {
        if sync.level != crate::internal::dav::SyncLevel::One {
            return Err(Error::single("icalkit.caldav.sync-level-unsupported"));
        }
        let token = sync
            .token
            .as_ref()
            .map(|token| SyncToken::new(token.as_bytes()))
            .transpose()?;
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
                let need =
                    ServerNeed::sync("caldav.sync.changes", &method, &uri, token, sync.limit);
                Ok(ServerStep::Need(
                    need,
                    Box::new(move |answer| {
                        let ServerAnswerKind::Sync(result) = answer.kind else {
                            return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
                        };
                        sync_response(&sync, result, policy).map(ServerStep::Done)
                    }),
                ))
            }),
            policy,
        ))
    }

    fn handle_mkcalendar(&self, request: WireRequest) -> Result<ServerOperation, Error> {
        let request_body = MkCalendarRequest::parse(request.body, self.policy)?.body;
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
                let need = ServerNeed::request_with_body(
                    "caldav.mkcalendar.create",
                    &method,
                    &uri,
                    request_body,
                );
                Ok(ServerStep::Need(
                    need,
                    Box::new(move |answer| {
                        let ServerAnswerKind::Created(created) = answer.kind else {
                            return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
                        };
                        let status = if created { 201 } else { 409 };
                        Ok(ServerStep::Done(WireResponse::new(
                            status,
                            Vec::new(),
                            Vec::new(),
                        )))
                    }),
                ))
            }),
            policy,
        ))
    }

    fn handle_schedule(&self, request: WireRequest) -> Result<ServerOperation, Error> {
        let schedule = ScheduleRequest::parse(&request.headers, &request.body, self.policy)?;
        let method = request.method;
        let uri = request.uri;
        let need = ServerNeed::schedule("caldav.authorize", &method, &uri, schedule.clone());
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
                let identity = ScheduleIdentity::of(&schedule.message)
                    .ok_or_else(|| Error::single("icalkit.caldav.schedule-request-invalid"))?;
                let need = ServerNeed::schedule("caldav.schedule.deliver", &method, &uri, schedule);
                Ok(ServerStep::Need(
                    need,
                    Box::new(move |answer| {
                        let ServerAnswerKind::Schedule(response) = answer.kind else {
                            return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
                        };
                        identity.validate(&response)?;
                        if response.is_no_content() {
                            return Ok(ServerStep::Done(WireResponse::new(
                                204,
                                Vec::new(),
                                Vec::new(),
                            )));
                        }
                        let body = encode_schedule_response(&response, policy)?;
                        Ok(ServerStep::Done(WireResponse::new(
                            200,
                            vec![Header::new(
                                "Content-Type",
                                b"application/xml; charset=utf-8".to_vec(),
                            )],
                            body,
                        )))
                    }),
                ))
            }),
            policy,
        ))
    }

    fn handle_put(&self, request: WireRequest) -> Result<ServerOperation, Error> {
        let write = match PutRequest::parse(&request, self.policy) {
            Ok(write) => write,
            Err(error) if error.code().as_str() == "icalkit.caldav.rscale-unsupported" => {
                return Ok(rscale::unsupported_put_operation(&request, self.policy));
            },
            Err(error) => return Err(error),
        };
        let method = request.method;
        let uri = request.uri;
        let need = ServerNeed::request("caldav.authorize", &method, &uri);
        let expected = write.revision.clone();
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
                let need = ServerNeed::write("caldav.resource.write", &method, &uri, write);
                Ok(ServerStep::Need(
                    need,
                    Box::new(move |answer| {
                        let ServerAnswerKind::Written(stored) = answer.kind else {
                            return Err(Error::single("icalkit.caldav.answer-kind-mismatch"));
                        };
                        let Some(stored) = stored else {
                            return Ok(ServerStep::Done(WireResponse::new(
                                412,
                                Vec::new(),
                                Vec::new(),
                            )));
                        };
                        if stored.uri != uri || stored.absent {
                            return Err(Error::single("icalkit.caldav.write-result-invalid"));
                        }
                        let etag = stored
                            .etag
                            .ok_or_else(|| Error::single("icalkit.caldav.write-result-invalid"))?;
                        let status = if expected.absent { 201 } else { 204 };
                        Ok(ServerStep::Done(WireResponse::new(
                            status,
                            vec![Header::new("ETag", etag.into_bytes())],
                            Vec::new(),
                        )))
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
    body: Vec<u8>,
    message: Option<Box<Message>>,
    originator: Option<String>,
    recipients: Vec<String>,
    calendar: Option<Box<Calendar>>,
    revision: Option<Revision>,
    sync_token: Option<SyncToken>,
    sync_limit: Option<u32>,
}

impl ServerNeed {
    /// Construct an application need identified by a stable code.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        Self {
            code,
            method: None,
            uri: None,
            body: Vec::new(),
            message: None,
            originator: None,
            recipients: Vec::new(),
            calendar: None,
            revision: None,
            sync_token: None,
            sync_limit: None,
        }
    }

    fn request(code: &'static str, method: &str, uri: &str) -> Self {
        Self {
            code,
            method: Some(method.to_string()),
            uri: Some(uri.to_string()),
            body: Vec::new(),
            message: None,
            originator: None,
            recipients: Vec::new(),
            calendar: None,
            revision: None,
            sync_token: None,
            sync_limit: None,
        }
    }

    fn request_with_body(code: &'static str, method: &str, uri: &str, body: Vec<u8>) -> Self {
        Self {
            code,
            method: Some(method.to_string()),
            uri: Some(uri.to_string()),
            body,
            message: None,
            originator: None,
            recipients: Vec::new(),
            calendar: None,
            revision: None,
            sync_token: None,
            sync_limit: None,
        }
    }

    fn schedule(code: &'static str, method: &str, uri: &str, request: ScheduleRequest) -> Self {
        Self {
            code,
            method: Some(method.to_string()),
            uri: Some(uri.to_string()),
            body: Vec::new(),
            message: Some(Box::new(request.message)),
            originator: Some(request.originator),
            recipients: request.recipients,
            calendar: None,
            revision: None,
            sync_token: None,
            sync_limit: None,
        }
    }

    fn write(code: &'static str, method: &str, uri: &str, request: PutRequest) -> Self {
        Self {
            code,
            method: Some(method.to_string()),
            uri: Some(uri.to_string()),
            body: Vec::new(),
            message: None,
            originator: None,
            recipients: Vec::new(),
            calendar: Some(Box::new(request.calendar)),
            revision: Some(request.revision),
            sync_token: None,
            sync_limit: None,
        }
    }

    fn sync(
        code: &'static str,
        method: &str,
        uri: &str,
        token: Option<SyncToken>,
        limit: Option<u32>,
    ) -> Self {
        Self {
            code,
            method: Some(method.to_string()),
            uri: Some(uri.to_string()),
            body: Vec::new(),
            message: None,
            originator: None,
            recipients: Vec::new(),
            calendar: None,
            revision: None,
            sync_token: token,
            sync_limit: limit,
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

    /// Validated request body associated with this need, when the adapter must persist it.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Validated busy time request supplied for authorization or delivery.
    #[must_use]
    pub fn message(&self) -> Option<&Message> {
        self.message.as_deref()
    }

    /// The `ORGANIZER` calendar address from the busy time request.
    #[must_use]
    pub fn originator(&self) -> Option<&str> {
        self.originator.as_deref()
    }

    /// Distinct `ATTENDEE` calendar addresses from the busy time request.
    #[must_use]
    pub fn recipients(&self) -> &[String] {
        &self.recipients
    }

    /// Strictly validated calendar associated with a resource write need.
    #[must_use]
    pub fn calendar(&self) -> Option<&Calendar> {
        self.calendar.as_deref()
    }

    /// Client revision precondition associated with a resource write need.
    #[must_use]
    pub const fn revision(&self) -> Option<&Revision> {
        self.revision.as_ref()
    }

    /// Previous opaque RFC 6578 token, or `None` for an initial synchronization.
    #[must_use]
    pub const fn sync_token(&self) -> Option<&SyncToken> {
        self.sync_token.as_ref()
    }

    /// Maximum number of changes requested by the client, when it supplied DAV:limit.
    #[must_use]
    pub const fn sync_limit(&self) -> Option<u32> {
        self.sync_limit
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
    Created(bool),
    Discovery(Discovery),
    Resources(Vec<StoredResource>),
    Schedule(ScheduleResponse),
    Sync(SyncResult),
    Written(Option<Revision>),
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

    /// Report whether a requested collection was created without a storage conflict.
    #[must_use]
    pub const fn created(created: bool) -> Self {
        Self {
            kind: ServerAnswerKind::Created(created),
        }
    }

    /// Supply service discovery properties loaded by the application adapter.
    #[must_use]
    pub fn discovery(discovery: Discovery) -> Self {
        Self {
            kind: ServerAnswerKind::Discovery(discovery),
        }
    }

    /// Supply validated resources loaded by the application storage adapter.
    #[must_use]
    pub fn resources(resources: Vec<StoredResource>) -> Self {
        Self {
            kind: ServerAnswerKind::Resources(resources),
        }
    }

    /// Supply typed outcomes for every requested scheduling recipient.
    #[must_use]
    pub fn schedule_response(response: ScheduleResponse) -> Self {
        Self {
            kind: ServerAnswerKind::Schedule(response),
        }
    }

    /// Supply a bounded change page and the token at which that page ends.
    #[must_use]
    pub fn sync_result(result: SyncResult) -> Self {
        Self {
            kind: ServerAnswerKind::Sync(result),
        }
    }

    /// Supply the new stored revision, or `None` when the precondition lost a race.
    #[must_use]
    pub fn written(revision: Option<Revision>) -> Self {
        Self {
            kind: ServerAnswerKind::Written(revision),
        }
    }

    /// Application answer octets.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        match &self.kind {
            ServerAnswerKind::Bytes(body) => body,
            ServerAnswerKind::Authorized(_)
            | ServerAnswerKind::Created(_)
            | ServerAnswerKind::Discovery(_)
            | ServerAnswerKind::Resources(_)
            | ServerAnswerKind::Schedule(_)
            | ServerAnswerKind::Sync(_)
            | ServerAnswerKind::Written(_) => &[],
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

/// A projected query result that cannot be passed to persistence APIs as a [`Calendar`].
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

#[cfg(test)]
mod tests {
    use super::{WireResponse, decode_schedule_response};
    use crate::scheduling::Message;
    use crate::{Engine, Limits, ResourcePolicy};
    use core::fmt::Write as _;

    #[test]
    fn rejected_owned_requests_release_live_namespace_reservations() {
        let policy = ResourcePolicy::secure();
        let mut meter = crate::internal::core::Meter::new(policy.limits);
        let result = super::decode_dav_request(
            br#"<X:unknown xmlns:X="urn:example:foreign"/>"#,
            policy,
            &mut meter,
        );
        assert_eq!(result.unwrap_err(), crate::internal::dav::DavError::Foreign);
        assert_eq!(meter.live_prefix_bindings(), 0);
        assert!(meter.spent() > 0);
        assert!(!meter.is_exhausted());
    }

    #[test]
    fn response_count_obeys_the_configured_ceiling() {
        let mut children = alloc::string::String::new();
        for recipient in ["mailto:bob@example.test", "mailto:carol@example.test"] {
            write!(
                children,
                "<C:response><C:recipient><D:href>{recipient}</D:href></C:recipient>\
                 <C:request-status>3.7;Invalid calendar user</C:request-status></C:response>"
            )
            .unwrap();
        }
        let body = alloc::format!(
            "<C:schedule-response xmlns:C=\"urn:ietf:params:xml:ns:caldav\" xmlns:D=\"DAV:\">\
             {children}</C:schedule-response>"
        );
        let response = WireResponse::new(200, alloc::vec::Vec::new(), body.into_bytes());
        assert!(decode_schedule_response(&response, ResourcePolicy::secure()).is_ok());
        let policy = ResourcePolicy {
            limits: Limits::DEFAULT.with_max_responses(1),
        };
        assert!(decode_schedule_response(&response, policy).is_err());
    }

    #[test]
    fn response_messages_share_one_item_budget() {
        let reply = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit tests//EN\r\n",
            "METHOD:REPLY\r\nBEGIN:VFREEBUSY\r\nUID:busy@example.test\r\n",
            "DTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nDTEND:20240103T000000Z\r\n",
            "ORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\n",
            "END:VFREEBUSY\r\nEND:VCALENDAR\r\n",
        );
        let policy = ResourcePolicy {
            limits: Limits::DEFAULT.with_max_items(20),
        };
        let engine = Engine::builder().resource_policy(policy).build();
        assert!(Message::read_in(&mut engine.session(), reply.as_bytes()).is_ok());
        let mut children = alloc::string::String::new();
        for recipient in ["mailto:bob@example.test", "mailto:carol@example.test"] {
            let data = reply.replace("mailto:bob@example.test", recipient);
            write!(
                children,
                "<C:response><C:recipient><D:href>{recipient}</D:href></C:recipient>\
                 <C:request-status>2.0;Success</C:request-status>\
                 <C:calendar-data>{data}</C:calendar-data></C:response>"
            )
            .unwrap();
        }
        let body = alloc::format!(
            "<C:schedule-response xmlns:C=\"urn:ietf:params:xml:ns:caldav\" xmlns:D=\"DAV:\">\
             {children}</C:schedule-response>"
        );
        let response = WireResponse::new(200, alloc::vec::Vec::new(), body.into_bytes());
        assert!(decode_schedule_response(&response, ResourcePolicy::secure()).is_ok());
        assert!(decode_schedule_response(&response, policy).is_err());
    }
}
