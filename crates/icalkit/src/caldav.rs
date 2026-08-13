// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sans-I/O CalDAV client and server workflow vocabulary.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};

use core::str;

use ical_core::{
    CivilDateTime, Component, ContentLineReader, Diagnostic, Document, Instant, Item, Meter,
    Severity, UtcOffset,
};
use ical_dav::{DecodeContext, RequestBody, UnknownPolicy, XmlReader};
use ical_query::{Budget, Zones};
use ical_tz::{
    AnswerBasis, LocalResolution, OffsetAnswer, Reading, ZoneAnswer, ZoneProvenance, ZoneSource,
};

use crate::time::{LocalKind, ZoneDatabase};
use crate::{Calendar, Error, ResourcePolicy, Session};

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
#[derive(Clone, Debug, Default)]
pub struct Client {
    marker: (),
}

type ResponseDecoder<T> = Box<dyn FnOnce(WireResponse) -> Result<T, crate::Error>>;

impl Client {
    /// Create a sans-I/O client workflow factory.
    #[must_use]
    pub const fn new() -> Self {
        Self { marker: () }
    }

    /// Start a typed operation without coupling it to an HTTP runtime.
    #[must_use]
    pub fn operation<T>(
        &self,
        request: WireRequest,
        decoder: impl FnOnce(WireResponse) -> Result<T, crate::Error> + 'static,
    ) -> Operation<T> {
        let () = self.marker;
        Operation::new(request, decoder)
    }
}

/// A typed client operation driven by requests and supplied responses.
pub struct Operation<T> {
    request: Option<WireRequest>,
    result: Option<Result<T, crate::Error>>,
    decoder: Option<ResponseDecoder<T>>,
}

impl<T> Operation<T> {
    /// Create an operation from its first request and response decoder.
    #[must_use]
    pub fn new(
        request: WireRequest,
        decoder: impl FnOnce(WireResponse) -> Result<T, crate::Error> + 'static,
    ) -> Self {
        Self {
            request: Some(request),
            result: None,
            decoder: Some(Box::new(decoder)),
        }
    }

    /// The request the caller should execute next.
    #[must_use]
    pub fn next_request(&self) -> Option<&WireRequest> {
        self.request.as_ref()
    }

    /// Supply the HTTP response to the current request.
    pub fn accept(&mut self, response: WireResponse) -> Result<(), crate::Error> {
        let decoder = self
            .decoder
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-response"))?;
        self.request = None;
        self.result = Some(decoder(response));
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
            .finish_non_exhaustive()
    }
}

/// A server workflow factory.
#[derive(Clone, Debug, Default)]
pub struct Server {
    marker: (),
}

type ServerResponder = Box<dyn FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error>>;

impl Server {
    /// Create a sans-I/O server workflow factory.
    #[must_use]
    pub const fn new() -> Self {
        Self { marker: () }
    }

    /// Start a server operation whose application dependency is supplied explicitly.
    #[must_use]
    pub fn operation(
        &self,
        need: ServerNeed,
        responder: impl FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error> + 'static,
    ) -> ServerOperation {
        let () = self.marker;
        ServerOperation::new(need, responder)
    }
}

/// A need for application storage, ACL, or routing data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerNeed {
    code: &'static str,
}

impl ServerNeed {
    /// Construct an application need identified by a stable code.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }

    /// The stable need code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

/// An application answer supplied to a server workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAnswer {
    body: Vec<u8>,
}

impl ServerAnswer {
    /// Construct an application answer body.
    #[must_use]
    pub fn new(body: Vec<u8>) -> Self {
        Self { body }
    }

    /// Application answer octets.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// A server operation separated from storage and ACL decisions.
pub struct ServerOperation {
    need: Option<ServerNeed>,
    response: Option<WireResponse>,
    responder: Option<ServerResponder>,
}

impl ServerOperation {
    /// Create an operation with one application need.
    #[must_use]
    pub fn new(
        need: ServerNeed,
        responder: impl FnOnce(ServerAnswer) -> Result<WireResponse, crate::Error> + 'static,
    ) -> Self {
        Self {
            need: Some(need),
            response: None,
            responder: Some(Box::new(responder)),
        }
    }

    /// The storage, ACL, or routing fact needed next.
    #[must_use]
    pub const fn next_need(&self) -> Option<&ServerNeed> {
        self.need.as_ref()
    }

    /// Supply the application-owned answer.
    pub fn supply(&mut self, answer: ServerAnswer) -> Result<(), crate::Error> {
        let responder = self
            .responder
            .take()
            .ok_or_else(|| crate::Error::single("icalkit.caldav.unexpected-answer"))?;
        self.need = None;
        self.response = Some(responder(answer)?);
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
