// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Protocol field and collection refusals stop the shared admission budget.

#![cfg(test)]

use core::fmt::Debug;

use icalkit_conformance::internal::core::{DiagnosticCode, LimitExceeded, Limits, Meter};
use icalkit_conformance::internal::dav::{
    Bounded, CalendarPayload, CompFilter, DavError, DavProperty, DavResponse, DecodeContext,
    ElementName, Href, MultiStatus, MultiStatusReader, MultiStatusWriter, PropName, PropStat,
    PropValue, RequestBody, ResponseSource, Status, SyncToken, WriteXml, XmlPull, XmlReader,
    XmlWriter,
};

fn assert_refusal<T: Debug>(
    result: Result<T, DavError>,
    meter: &mut Meter,
    dimension: LimitExceeded,
) {
    assert_eq!(result.unwrap_err(), DavError::Limit(dimension));
    assert!(meter.is_exhausted(), "{dimension:?}");
    assert_eq!(meter.try_charge_bytes(0), Err(LimitExceeded::Exhausted));
    assert_eq!(meter.try_charge_occurrence(), Err(LimitExceeded::Exhausted));
}

#[test]
fn a_collection_refusal_latches_and_keeps_later_errors_distinct() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut list = Bounded::with_cap(0, LimitExceeded::Properties);
    assert_refusal(
        list.push(1_u8, &mut meter),
        &mut meter,
        LimitExceeded::Properties,
    );
    assert_eq!(list.as_slice(), []);
    assert_eq!(
        list.push(2, &mut meter),
        Err(DavError::Limit(LimitExceeded::Exhausted))
    );
}

#[test]
fn an_href_refusal_latches_and_keeps_later_errors_distinct() {
    let limits = Limits::DEFAULT.with_max_href_bytes(1);
    let mut meter = Meter::new(limits);
    assert!(Href::new(b"/", limits, &mut meter).is_ok());
    assert_refusal(
        Href::new(b"/x", limits, &mut meter),
        &mut meter,
        LimitExceeded::Href,
    );
    assert_eq!(
        Href::new(b"/x", limits, &mut meter).unwrap_err(),
        DavError::Limit(LimitExceeded::Exhausted)
    );
}

#[test]
fn a_sync_token_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_href_bytes(1);
    let mut meter = Meter::new(limits);
    assert_refusal(
        SyncToken::new(b"xx", limits, &mut meter),
        &mut meter,
        LimitExceeded::Href,
    );
}

#[test]
fn a_calendar_payload_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(1);
    let mut meter = Meter::new(limits);
    assert_refusal(
        CalendarPayload::from_octets(b"xx", limits, &mut meter),
        &mut meter,
        LimitExceeded::Text,
    );
}

#[test]
fn a_filter_depth_refusal_latches_without_mutating_the_parent() {
    let limits = Limits::DEFAULT.with_max_xml_depth(1);
    let mut meter = Meter::new(limits);
    let mut parent = CompFilter::new(b"VCALENDAR", limits, &mut meter).unwrap();
    let child = CompFilter::new(b"VEVENT", limits, &mut meter).unwrap();
    assert_refusal(
        parent.push_comp(child, limits, &mut meter),
        &mut meter,
        LimitExceeded::Depth,
    );
    assert_eq!(parent.height(), 1);
}

#[test]
fn a_multistatus_depth_refusal_latches_before_output() {
    let limits = Limits::DEFAULT.with_max_xml_depth(0);
    let mut meter = Meter::new(limits);
    let mut out = Vec::new();
    assert_refusal(
        MultiStatusWriter::new(&mut out, limits, &mut meter),
        &mut meter,
        LimitExceeded::Depth,
    );
    assert_eq!(out.as_slice(), []);
}

#[test]
fn an_xml_text_refusal_latches_and_keeps_later_errors_distinct() {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_xml_text_bytes(1));
    let mut out = Vec::new();
    let mut writer = XmlWriter::fragment(&mut out);
    assert_refusal(
        writer.text(b"xx", &mut meter),
        &mut meter,
        LimitExceeded::Text,
    );
    assert_eq!(
        writer.text(b"xx", &mut meter),
        Err(DavError::Limit(LimitExceeded::Exhausted))
    );
}

#[test]
fn a_property_text_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(1);
    let mut meter = Meter::new(limits);
    let value = PropValue::Text(b"xx".to_vec().into_boxed_slice());
    assert_refusal(
        value.write_xml(&mut Vec::new(), limits, &mut meter),
        &mut meter,
        LimitExceeded::Text,
    );
}

#[test]
fn an_output_href_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_href_bytes(1);
    let mut build = Meter::new(Limits::DEFAULT);
    let href = Href::new(b"/x", Limits::DEFAULT, &mut build).unwrap();
    let response = DavResponse::with_status(href, Status::OK);
    let mut meter = Meter::new(limits);
    assert_refusal(
        response.write_xml(&mut Vec::new(), limits, &mut meter),
        &mut meter,
        LimitExceeded::Href,
    );
}

#[test]
fn an_output_property_count_refusal_latches() {
    let mut build = Meter::new(Limits::DEFAULT);
    let mut group = PropStat::new(Status::OK, Limits::DEFAULT);
    group
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Getetag),
                value: PropValue::Empty,
            },
            &mut build,
        )
        .unwrap();
    let limits = Limits::DEFAULT.with_max_props_per_response(0);
    let mut meter = Meter::new(limits);
    assert_refusal(
        group.write_xml(&mut Vec::new(), limits, &mut meter),
        &mut meter,
        LimitExceeded::Properties,
    );
}

fn read_response(
    bytes: &[u8],
    limits: Limits,
    meter: &mut Meter,
) -> Result<Option<DavResponse>, DavError> {
    let mut events = XmlReader::new(bytes);
    let mut responses = MultiStatusReader::new(&mut events);
    let mut diagnostics = Vec::new();
    let mut context = DecodeContext::new(limits, meter, &mut diagnostics);
    responses.next_response(&mut context)
}

#[test]
fn an_input_href_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_href_bytes(1);
    let mut meter = Meter::new(limits);
    let xml = br#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/x</D:href></D:response></D:multistatus>"#;
    assert_refusal(
        read_response(xml, limits, &mut meter),
        &mut meter,
        LimitExceeded::Href,
    );
}

#[test]
fn a_text_refusal_from_the_reader_latches() {
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(4);
    let mut meter = Meter::new(limits);
    let xml = br#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/</D:href><D:propstat><D:prop><D:displayname>xxx<![CDATA[yyy]]></D:displayname></D:prop></D:propstat></D:response></D:multistatus>"#;
    assert_refusal(
        read_response(xml, limits, &mut meter),
        &mut meter,
        LimitExceeded::Text,
    );
}

#[test]
fn an_input_sync_token_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_href_bytes(1);
    let mut meter = Meter::new(limits);
    let xml = br#"<D:multistatus xmlns:D="DAV:"><D:sync-token>xx</D:sync-token></D:multistatus>"#;
    assert_refusal(
        read_response(xml, limits, &mut meter),
        &mut meter,
        LimitExceeded::Href,
    );
}

#[test]
fn a_response_body_refusal_latches_before_lexical_reading() {
    let limits = Limits::DEFAULT.with_max_response_bytes(1);
    let mut meter = Meter::new(Limits::DEFAULT);
    let mut diagnostics = Vec::new();
    let mut reader = XmlReader::new(b"<x/>");
    let result = reader.next_event(&mut DecodeContext::new(
        limits,
        &mut meter,
        &mut diagnostics,
    ));
    assert_refusal(result, &mut meter, LimitExceeded::Budget);
}

fn read_request(bytes: &[u8], limits: Limits, meter: &mut Meter) -> Result<RequestBody, DavError> {
    let mut events = XmlReader::new(bytes);
    let mut diagnostics = Vec::new();
    RequestBody::read(
        &mut events,
        &mut DecodeContext::new(limits, meter, &mut diagnostics),
    )
}

#[test]
fn a_filter_reader_depth_refusal_latches_the_aggregate_ledger() {
    let xml = br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"/></C:comp-filter></C:filter></C:calendar-query>"#;
    let mut meter = Meter::new(Limits::DEFAULT);
    assert_refusal(
        read_request(xml, Limits::DEFAULT.with_max_xml_depth(1), &mut meter),
        &mut meter,
        LimitExceeded::Depth,
    );
}

#[test]
fn a_selection_reader_depth_refusal_latches_the_aggregate_ledger() {
    let xml = br#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:"><D:prop><C:calendar-data><C:comp name="VCALENDAR"><C:comp name="VEVENT"/></C:comp></C:calendar-data></D:prop><C:filter><C:comp-filter name="VCALENDAR"/></C:filter></C:calendar-query>"#;
    let mut meter = Meter::new(Limits::DEFAULT);
    assert_refusal(
        read_request(xml, Limits::DEFAULT.with_max_xml_depth(1), &mut meter),
        &mut meter,
        LimitExceeded::Depth,
    );
}

#[test]
fn malformed_markup_does_not_exhaust_the_resource_budget() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let value = PropValue::Markup(b"</D:x>".to_vec().into_boxed_slice());
    let result = value.write_xml(&mut Vec::new(), Limits::DEFAULT, &mut meter);
    assert!(result.is_err());
    assert!(!matches!(result, Err(DavError::Limit(_))));
    assert!(!meter.is_exhausted());
    assert_eq!(meter.try_charge_bytes(0), Ok(()));
}

#[test]
fn a_kept_fragment_depth_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_xml_depth(6);
    let mut meter = Meter::new(limits);
    let value = PropValue::Markup(
        b"<D:supported-report><D:report/></D:supported-report>"
            .to_vec()
            .into_boxed_slice(),
    );
    assert_refusal(
        value.write_xml(&mut Vec::new(), limits, &mut meter),
        &mut meter,
        LimitExceeded::Depth,
    );
}

#[test]
fn a_kept_open_element_depth_refusal_latches() {
    let limits = Limits::DEFAULT.with_max_xml_depth(6);
    let mut meter = Meter::new(limits);
    let value = PropValue::Markup(
        b"<D:supported-report><D:report>text</D:report></D:supported-report>"
            .to_vec()
            .into_boxed_slice(),
    );
    assert_refusal(
        value.write_xml(&mut Vec::new(), limits, &mut meter),
        &mut meter,
        LimitExceeded::Depth,
    );
}

struct ResponseBatch {
    items: std::vec::IntoIter<DavResponse>,
    token: Option<&'static [u8]>,
    truncated: bool,
    pulls: usize,
}

impl ResponseSource for ResponseBatch {
    fn next_response(
        &mut self,
        _context: &mut DecodeContext<'_>,
    ) -> Result<Option<DavResponse>, DavError> {
        self.pulls = self.pulls.saturating_add(1);
        Ok(self.items.next())
    }
    fn sync_token(&self) -> Option<&[u8]> {
        self.token
    }
    fn was_truncated(&self) -> bool {
        self.truncated
    }
}

fn response_batch(count: usize, truncated: bool) -> ResponseBatch {
    let mut meter = Meter::new(Limits::DEFAULT);
    let href = Href::new(b"/first", Limits::DEFAULT, &mut meter).unwrap();
    let response = DavResponse::with_status(href, Status::OK);
    ResponseBatch {
        items: vec![response; count].into_iter(),
        token: Some(b"token"),
        truncated,
        pulls: 0,
    }
}

#[test]
fn an_owned_collection_refusal_keeps_its_prefix_and_latches() {
    let limits = Limits::DEFAULT.with_max_responses(1);
    let mut meter = Meter::new(limits);
    let mut source = response_batch(2, false);
    let mut diagnostics = Vec::new();
    let body = MultiStatus::read(
        &mut source,
        &mut DecodeContext::new(limits, &mut meter, &mut diagnostics),
    )
    .unwrap();
    assert_eq!(body.responses().len(), 1);
    assert_eq!(body.responses()[0].href.as_bytes(), b"/first");
    assert!(body.sync_token.is_none());
    assert!(meter.is_exhausted());
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == DiagnosticCode::DavResponsesTruncated)
    );
}

#[test]
fn an_external_source_cut_latches_without_inventing_a_collection_limit() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut source = response_batch(1, true);
    let mut diagnostics = Vec::new();
    let body = MultiStatus::read(
        &mut source,
        &mut DecodeContext::new(limits, &mut meter, &mut diagnostics),
    )
    .unwrap();
    assert_eq!(body.responses().len(), 1);
    assert!(body.sync_token.is_none());
    assert!(meter.is_exhausted());
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == DiagnosticCode::ResourceBudgetExhausted)
    );
}

#[test]
fn an_exhausted_collection_refuses_before_polling_an_empty_source() {
    let limits = Limits::DEFAULT.with_occurrences_per_search(0);
    let mut meter = Meter::new(limits);
    assert_eq!(
        meter.try_charge_occurrence(),
        Err(LimitExceeded::Occurrences)
    );
    let mut source = response_batch(0, false);
    source.token = None;
    let mut diagnostics = Vec::new();
    let result = MultiStatus::read(
        &mut source,
        &mut DecodeContext::new(limits, &mut meter, &mut diagnostics),
    );
    assert_eq!(
        result.unwrap_err(),
        DavError::Limit(LimitExceeded::Exhausted)
    );
    assert_eq!(source.pulls, 0);
}
