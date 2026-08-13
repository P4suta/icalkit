// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The two halves of this crate, driven through each other over real octets.
//!
//! Every unit inside `src/` proves one direction. The readers are tested against wire bytes
//! and the writers against the RFCs' own examples, but a unit could not test the composition:
//! each was written before the tokenizer existed, so each carries a stand-in `XmlPull` of its
//! own that refuses almost nothing and is not the one that ships. What no unit could assert is
//! the claim the whole layer rests on — that a value written here reads back here as the same
//! value, through the tokenizer callers will actually use.
//!
//! That is what this file asserts, in both directions:
//!
//! - **`value -> octets -> value`.** Each of the five request bodies and the multistatus is
//!   built through the constructors, encoded, tokenized by [`XmlReader`], decoded, and
//!   compared. A disagreement between the encoder and the decoder about one element's shape
//!   shows up here and nowhere else in the suite.
//! - **`octets -> value -> octets -> value`.** The three frozen real-server fixtures are read,
//!   re-encoded from the value, and read again. Byte equality with the fixture is *not* the
//!   claim and could not be: three servers spell one body three ways. Value equality across
//!   the re-encode is.
//!
//! One case is idempotent rather than identical, deliberately, and is asserted as such: a
//! `PropRequest` naming `CALDAV:calendar-data` twice — once in its names and once through
//! `calendar_data` — writes the element once, because RFC 4791 section 9.6's grammar admits it
//! once. `decode -> encode -> decode` is stable; `encode -> decode` is not the identity on that
//! one redundant input.

use ical_core::{Diagnostic, IgnoreDiagnostics, Instant, Limits, Meter};
use ical_dav::{
    CalendarDataRequest, CalendarMultiget, CalendarPayload, CalendarQuery, CompFilter, DavError,
    DavProperty, DavResponse, DecodeContext, ETag, ElementName, FreeBusyQuery, Href, MultiStatus,
    MultiStatusReader, MultiStatusWriter, PropFind, PropName, PropRequest, PropStat, PropValue,
    RequestBody, Status, SyncToken, TimeRange, WriteXml, XmlPull, XmlReader, XmlWriter,
};

/// The `.ics` all three fixtures carry, byte for byte.
const PAYLOAD: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/calendar-data-payload.ics");

const SABREDAV: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/sabredav-calendar-multiget.xml");
const RADICALE: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/radicale-calendar-multiget.xml");
const CALENDAR_SERVER: &[u8] =
    include_bytes!("../../icalkit/src/internal/dav/fixtures/calendarserver-calendar-multiget.xml");

/// 2006-01-04T00:00:00Z, the window RFC 4791 section 7.8.1's own example asks about.
const WINDOW_START: i64 = 1_136_332_800;
/// 2006-01-05T00:00:00Z.
const WINDOW_END: i64 = 1_136_419_200;

// -------------------------------------------------------------------------------------------
// Driving the two halves through each other.
// -------------------------------------------------------------------------------------------

/// Encode one value into a fresh buffer under the caller's bounds.
fn encode(value: &dyn WriteXml, limits: Limits) -> Result<Vec<u8>, DavError> {
    let mut out: Vec<u8> = Vec::new();
    let mut meter = Meter::new(limits);
    value.write_xml(&mut out, limits, &mut meter)?;
    Ok(out)
}

/// Read a request body out of octets, through the tokenizer that ships.
fn read_request(body: &[u8], limits: Limits) -> Result<RequestBody, DavError> {
    let mut meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
    let mut events = XmlReader::new(body);
    RequestBody::read(&mut events, &mut context)
}

/// Read a whole multistatus out of octets, through the tokenizer that ships.
fn read_multistatus(
    body: &[u8],
    limits: Limits,
) -> Result<(MultiStatus, Vec<Diagnostic>), DavError> {
    let mut meter = Meter::new(limits);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let collected = {
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let mut events = XmlReader::new(body);
        let mut source = MultiStatusReader::new(&mut events);
        MultiStatus::read(&mut source, &mut context)
    }?;
    Ok((collected, reported))
}

/// A body written, tokenized, and read back as the value that wrote it.
///
/// `wrap` is the `RequestBody` variant this body becomes once a server has learned which body
/// arrived. Taking it as a function rather than matching on an already-built `RequestBody`
/// keeps the encoder call statically typed, so a body with no `WriteXml` implementation is a
/// compile error here rather than a wildcard arm `#[non_exhaustive]` would force.
fn round_trips<B>(wrap: fn(B) -> RequestBody, body: B) -> Result<(), DavError>
where
    B: WriteXml + Clone,
{
    let limits = Limits::DEFAULT;
    let wire = encode(&body, limits)?;
    assert_eq!(read_request(&wire, limits), Ok(wrap(body)), "{wire:?}");
    Ok(())
}

// -------------------------------------------------------------------------------------------
// The values, built the way a caller builds them.
// -------------------------------------------------------------------------------------------

fn href(path: &[u8]) -> Result<Href, DavError> {
    let mut meter = Meter::new(Limits::DEFAULT);
    Href::new(path, Limits::DEFAULT, &mut meter)
}

/// The window RFC 4791 section 7.8.1's own example asks about.
fn the_window() -> Result<TimeRange, DavError> {
    TimeRange::new(
        Some(Instant::from_unix_seconds(WINDOW_START)),
        Some(Instant::from_unix_seconds(WINDOW_END)),
    )
}

/// `getetag` plus the payload itself, which is what every real client asks for.
fn etag_and_payload(limits: Limits) -> Result<PropRequest, DavError> {
    let mut meter = Meter::new(limits);
    let mut props = PropRequest::new(limits);
    props.push(PropName::Known(ElementName::Getetag), &mut meter)?;
    props.calendar_data = Some(CalendarDataRequest::default());
    Ok(props)
}

/// RFC 4791 section 7.8.1's query: this week's events, with their tags and their data.
fn a_calendar_query(limits: Limits) -> Result<CalendarQuery, DavError> {
    let mut meter = Meter::new(limits);
    let mut query = CalendarQuery::new(limits);
    query.props = etag_and_payload(limits)?;
    let mut wanted = CompFilter::new(b"VEVENT", limits, &mut meter)?;
    wanted.time_range = Some(the_window()?);
    let mut calendar = CompFilter::new(b"VCALENDAR", limits, &mut meter)?;
    calendar.push_comp(wanted, limits, &mut meter)?;
    query.filter = Some(calendar);
    Ok(query)
}

// -------------------------------------------------------------------------------------------
// value -> octets -> value, for every body this crate defines.
// -------------------------------------------------------------------------------------------

/// Every request body a client can build survives its own encoder and its own tokenizer.
///
/// This is the assertion `write_request.rs` names as its central claim and could not make: it
/// needs the tokenizer, which is another unit's type. Every root `RequestBody` dispatches on
/// has a case here.
#[test]
fn every_request_body_reads_back_as_the_value_that_wrote_it() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);

    round_trips(RequestBody::PropFind, PropFind::Names).unwrap();
    round_trips(
        RequestBody::PropFind,
        PropFind::Props(etag_and_payload(limits).unwrap()),
    )
    .unwrap();
    round_trips(
        RequestBody::PropFind,
        PropFind::AllProp(etag_and_payload(limits).unwrap()),
    )
    .unwrap();
    round_trips(
        RequestBody::CalendarQuery,
        a_calendar_query(limits).unwrap(),
    )
    .unwrap();

    let mut multiget = CalendarMultiget::new(limits);
    multiget.props = etag_and_payload(limits).unwrap();
    for path in [
        b"/bernard/work/abcd1.ics".as_slice(),
        b"/bernard/work/mtg1.ics".as_slice(),
    ] {
        multiget.push_href(href(path).unwrap(), &mut meter).unwrap();
    }
    round_trips(RequestBody::CalendarMultiget, multiget).unwrap();

    round_trips(
        RequestBody::FreeBusyQuery,
        FreeBusyQuery {
            range: the_window().unwrap(),
        },
    )
    .unwrap();
}

/// A window with one bound open survives the trip in both of its shapes.
///
/// RFC 4791 section 9.9 makes `start` and `end` independently optional, so an absent bound has
/// to arrive absent rather than as an extreme instant that means something else. Asserted on
/// the composition rather than on either half, because "the writer omits it" and "the reader
/// defaults it" are two bugs that cancel inside one unit's own tests.
#[test]
fn an_open_bound_stays_open_through_the_encoder_and_the_tokenizer() {
    let limits = Limits::DEFAULT;
    for range in [
        TimeRange::starting_at(Instant::from_unix_seconds(WINDOW_START)),
        TimeRange::ending_before(Instant::from_unix_seconds(WINDOW_END)),
    ] {
        let body = FreeBusyQuery { range };
        round_trips(RequestBody::FreeBusyQuery, body).unwrap();

        // Named rather than left to the structural equality above: a bound the writer omitted
        // and a bound the reader invented would both compare equal as a pair of `None`s.
        let wire = encode(&body, limits).unwrap();
        let RequestBody::FreeBusyQuery(read) = read_request(&wire, limits).unwrap() else {
            panic!("a free-busy-query reads back as one");
        };
        assert_eq!(read.range.start(), range.start());
        assert_eq!(read.range.end(), range.end());
    }
}

/// The `sync-collection` REPORT, in the two shapes RFC 6578 section 3 defines.
#[cfg(feature = "sync-collection")]
#[test]
fn a_synchronization_reads_back_whether_or_not_it_carries_a_token() {
    use ical_dav::{SyncCollection, SyncLevel};

    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);

    // An initial enumeration: RFC 6578 section 3 spells it as an empty `DAV:sync-token`, and
    // it has to read back as `None` rather than as a token whose octets are the empty string.
    let mut initial = SyncCollection::new(limits);
    initial.props = etag_and_payload(limits).unwrap();
    round_trips(RequestBody::SyncCollection, initial).unwrap();

    let mut resumed = SyncCollection::new(limits);
    resumed.props = etag_and_payload(limits).unwrap();
    resumed.token = Some(SyncToken::new(b"http://sabre.io/ns/sync/5", limits, &mut meter).unwrap());
    resumed.level = SyncLevel::Infinite;
    resumed.limit = Some(100);
    round_trips(RequestBody::SyncCollection, resumed).unwrap();
}

/// A build without the feature refuses the REPORT rather than answering a different question.
#[cfg(not(feature = "sync-collection"))]
#[test]
fn a_build_without_the_feature_refuses_the_report_it_cannot_honor() {
    let wire = br#"<D:sync-collection xmlns:D="DAV:"><D:sync-token/><D:sync-level>1</D:sync-level>
  <D:prop><D:getetag/></D:prop></D:sync-collection>"#;
    assert_eq!(
        read_request(wire, Limits::DEFAULT),
        Err(DavError::Unsupported(ElementName::SyncCollection))
    );
}

/// The one input where `encode -> decode` is idempotent rather than the identity.
///
/// A `PropRequest` can name `CALDAV:calendar-data` twice — once as a name and once as the
/// request for the payload's shape — and RFC 4791 section 9.6's grammar admits the element
/// once. The writer emits it once; the reader lands it on `calendar_data`. So the redundant
/// input converges on the canonical one, and the canonical one is a fixed point.
#[test]
fn naming_the_payload_twice_converges_on_the_one_spelling_the_grammar_admits() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);

    let mut redundant = PropRequest::new(limits);
    redundant
        .push(PropName::Known(ElementName::CalendarData), &mut meter)
        .expect("one property is within bounds");
    redundant.calendar_data = Some(CalendarDataRequest::default());

    let once = encode(&PropFind::Props(redundant), limits).expect("it encodes");
    // The element the value named twice appears on the wire once, which is the whole of what
    // makes this convergence rather than loss: a body naming it twice is one RFC 4791 section
    // 9.6's grammar does not admit and a strict server may refuse outright.
    assert_eq!(occurrences(&once, b"<C:calendar-data"), 1);

    let RequestBody::PropFind(PropFind::Props(canonical)) =
        read_request(&once, limits).expect("it reads back")
    else {
        panic!("a propfind with a prop list reads back as one");
    };
    // The redundancy is gone and nothing else moved: the payload is still asked for.
    assert!(canonical.calendar_data.is_some());
    assert!(
        !canonical
            .names()
            .contains(&PropName::Known(ElementName::CalendarData))
    );

    // And the canonical form is a fixed point, which is what makes this idempotence rather
    // than loss: a second trip changes nothing at all.
    round_trips(RequestBody::PropFind, PropFind::Props(canonical)).unwrap();
}

// -------------------------------------------------------------------------------------------
// The response half.
// -------------------------------------------------------------------------------------------

/// A multistatus a server builds is the multistatus a client reads, payload included.
///
/// The response side's own tests each drive one direction against a stand-in. This drives the
/// shipped encoder into the shipped tokenizer, over a body carrying the three things that are
/// easy to get wrong between them: a payload whose `CR` octets are escaped on the way out and
/// resolved on the way in, two statuses that must not collapse into one, and an `ETag` whose
/// content is the peer's octets rather than this crate's.
#[test]
fn a_multistatus_this_crate_writes_is_one_it_reads_back_to_the_same_value() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);

    let payload = CalendarPayload::from_octets(PAYLOAD, limits, &mut meter).expect("a payload");
    let mut found = PropStat::new(Status::OK, limits);
    found
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Getetag),
                value: PropValue::Entity(ETag::parse(b"\"2d9-5f1b0c4a\"").expect("a tag")),
            },
            &mut meter,
        )
        .expect("within bounds");
    found
        .push(
            DavProperty {
                name: PropName::Known(ElementName::CalendarData),
                value: PropValue::CalendarData(payload),
            },
            &mut meter,
        )
        .expect("within bounds");
    let mut refused = PropStat::new(Status::FORBIDDEN, limits);
    refused
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Displayname),
                value: PropValue::Empty,
            },
            &mut meter,
        )
        .expect("within bounds");

    let mut response =
        DavResponse::with_propstats(href(b"/bernard/work/abcd1.ics").unwrap(), limits);
    response
        .push_propstat(found, &mut meter)
        .expect("within bounds");
    response
        .push_propstat(refused, &mut meter)
        .expect("within bounds");

    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("within bounds");
    built
        .push(
            DavResponse::with_status(href(b"/bernard/work/gone.ics").unwrap(), Status::NOT_FOUND),
            &mut meter,
        )
        .expect("within bounds");
    built.sync_token =
        Some(SyncToken::new(b"http://sabre.io/ns/sync/5", limits, &mut meter).expect("a token"));

    let wire = encode(&built, limits).expect("the multistatus encodes");
    let (read, _) = read_multistatus(&wire, limits).expect("it reads back");
    assert_eq!(read, built);

    // The two statuses stayed apart rather than collapsing into the resource's own, and the
    // payload came back byte-identical with its `CRLF` terminators and its fold intact.
    let first = read.responses().first().expect("one response");
    assert_eq!(first.propstats().len(), 2);
    let Some(PropValue::CalendarData(recovered)) =
        first.successful_value(&PropName::Known(ElementName::CalendarData))
    else {
        panic!("the payload came back under its success status");
    };
    assert_eq!(recovered.as_bytes(), PAYLOAD);
    assert!(recovered.is_as_sent());
    assert!(recovered.as_bytes().windows(3).any(|at| at == b"\r\n "));
    // Nothing the client reads was a raw CR on the wire: the writer needs no departure from
    // XML 1.0 at all, so any conformant processor recovers the same octets.
    let opened = find(&wire, b"<C:calendar-data>").expect("the element is there");
    let closed = find(&wire, b"</C:calendar-data>").expect("the element is closed");
    assert!(!wire[opened..closed].contains(&b'\r'));
}

/// The incremental encoder and the owned one write the same octets.
///
/// `MultiStatus::write_xml` is documented as one consumer of `MultiStatusWriter` rather than a
/// second implementation beside it. Asserted here rather than in the unit because a server
/// streaming forty thousand responses and a client holding one collection have to agree, and a
/// divergence would be a body one of them cannot read.
#[test]
fn the_streaming_encoder_and_the_owned_one_agree_octet_for_octet() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut built = MultiStatus::new(limits);
    for path in [b"/work/a.ics".as_slice(), b"/work/b.ics".as_slice()] {
        built
            .push(
                DavResponse::with_status(href(path).unwrap(), Status::NOT_FOUND),
                &mut meter,
            )
            .expect("within bounds");
    }
    built.sync_token = Some(
        SyncToken::new(b"http://radicale.org/ns/sync/9", limits, &mut meter).expect("a token"),
    );

    let owned = encode(&built, limits).expect("the owned encoder writes it");

    let mut streamed: Vec<u8> = Vec::new();
    let mut streaming_meter = Meter::new(limits);
    let mut writer =
        MultiStatusWriter::new(&mut streamed, limits, &mut streaming_meter).expect("it opens");
    for response in built.responses() {
        writer
            .push(response, &mut streaming_meter)
            .expect("it takes one");
    }
    writer
        .finish(built.sync_token.as_ref(), &mut streaming_meter)
        .expect("it closes");

    assert_eq!(owned, streamed);
}

// -------------------------------------------------------------------------------------------
// octets -> value -> octets -> value, over the frozen fixtures.
// -------------------------------------------------------------------------------------------

/// Three servers, three prefix habits, one value that survives being written back out.
///
/// Byte equality with a fixture is not the claim and could not be: `SabreDAV` writes `d:`/`cal:`
/// and literal `CRLF`, Radicale writes `ns0:`/`ns1:`, Calendar Server writes a default `DAV:`
/// declaration and `&#13;`. This crate writes one spelling of all three. What must hold is that
/// the value is a fixed point of the re-encode — which is the property a client that reads a
/// resource, holds it, and writes it back depends on.
#[test]
fn every_server_s_spelling_survives_being_re_encoded_as_this_crate_s_own() {
    let limits = Limits::DEFAULT;
    for (server, fixture) in [
        ("SabreDAV", SABREDAV),
        ("Radicale", RADICALE),
        ("Calendar Server", CALENDAR_SERVER),
    ] {
        let (read, _) = read_multistatus(fixture, limits).expect(server);
        let rewritten = encode(&read, limits).expect(server);
        let (again, _) = read_multistatus(&rewritten, limits).expect(server);
        assert_eq!(again, read, "{server}");

        // The payload is the thing the whole carve-out exists for, so it is asserted by name
        // rather than left to the structural equality above.
        let first = again.responses().first().expect(server);
        let Some(PropValue::CalendarData(payload)) =
            first.successful_value(&PropName::Known(ElementName::CalendarData))
        else {
            panic!("{server} answers with the payload");
        };
        assert_eq!(payload.as_bytes(), PAYLOAD, "{server}");
        assert!(payload.is_as_sent(), "{server}");
    }
}

// -------------------------------------------------------------------------------------------
// The element writer, against the element reader.
// -------------------------------------------------------------------------------------------

/// What `XmlWriter` emits, `XmlReader` reads — including the payload it was built to carry.
///
/// The two body encoders in this crate are private to their own files and predate `XmlWriter`;
/// this is the assertion that keeps the public element writer honest against the same
/// tokenizer they are measured by, rather than against a reading of the specification.
#[test]
fn the_element_writer_writes_what_the_element_reader_reads() {
    let limits = Limits::DEFAULT;
    let mut out: Vec<u8> = Vec::new();
    let mut meter = Meter::new(limits);
    {
        let mut writer = XmlWriter::new(&mut out, &mut meter);
        writer.open(ElementName::Multistatus).expect("a root");
        writer.open(ElementName::Response).expect("a response");
        writer
            .element_text(ElementName::Href, b"/bernard/work/abcd1.ics")
            .expect("an href");
        writer
            .element_text(ElementName::CalendarData, PAYLOAD)
            .expect("a payload");
        writer.finish().expect("it closes what it opened");
    }

    // Well-formed to this crate's own tokenizer, which refuses more than XML does.
    let mut reader_meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, &mut reader_meter, &mut sink);
    let mut events = XmlReader::new(&out);
    let mut seen = Vec::new();
    while let Some(event) = events.next_event(&mut context).expect("it tokenizes") {
        if let ical_dav::XmlEvent::Start { known, .. } = event {
            seen.push(known);
        }
    }
    assert_eq!(
        seen,
        [
            Some(ElementName::Multistatus),
            Some(ElementName::Response),
            Some(ElementName::Href),
            Some(ElementName::CalendarData),
        ]
    );

    // And the payload inside it is the payload, with every `CR` written as `&#13;` rather than
    // raw, so the departure this crate's reader makes is not one its writer needs.
    let opened = find(&out, b"<C:calendar-data>").expect("the element is there");
    let closed = find(&out, b"</C:calendar-data>").expect("the element is closed");
    let span = &out[opened.saturating_add(b"<C:calendar-data>".len())..closed];
    assert!(!span.contains(&b'\r'));
    let decoded = ical_dav::decode_text(
        span,
        ical_dav::TextMode::Verbatim,
        0,
        &mut Meter::new(limits),
        &mut IgnoreDiagnostics,
    )
    .expect("the span decodes");
    assert_eq!(decoded.run.as_bytes(), PAYLOAD);
}

/// A sink with no room reports its own refusal rather than a bound the caller could raise.
///
/// The two are told apart because the fixes differ: a caller told its buffer is full enlarges
/// the buffer, and a caller told a limit was crossed raises the limit. Asserted across the
/// public encoder rather than inside the writer, since that is where a caller meets it.
#[test]
fn a_full_buffer_and_a_crossed_bound_are_different_answers() {
    let limits = Limits::DEFAULT;
    let query = a_calendar_query(limits).unwrap();

    let mut room = [0_u8; 16];
    let mut cramped = ical_dav::SliceSink::new(&mut room);
    let mut meter = Meter::new(limits);
    assert_eq!(
        query.write_xml(&mut cramped, limits, &mut meter),
        Err(DavError::Output(ical_dav::SinkFull))
    );

    let tight = Limits::DEFAULT.with_max_xml_depth(2);
    let mut out: Vec<u8> = Vec::new();
    let mut tight_meter = Meter::new(tight);
    assert!(matches!(
        query.write_xml(&mut out, tight, &mut tight_meter),
        Err(DavError::Limit(_))
    ));
}

/// The first offset of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// How many times `needle` occurs in `haystack`, counting overlaps as XML never produces them.
fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

// -------------------------------------------------------------------------------------------
// The fields the conformance attack added, driven through both halves the same way.
// -------------------------------------------------------------------------------------------

/// A precondition inside one property group stays inside that group, both ways.
///
/// RFC 4918 section 14.22's grammar is `propstat (prop, status, error?, responsedescription?)`.
/// Two groups naming two different conditions have said two different things about two
/// different properties, and a client asking "why was `calendar-data` refused" has to read the
/// one that belongs to that group.
#[test]
fn a_precondition_stays_in_the_property_group_that_named_it() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut refused = PropStat::new(Status::FORBIDDEN, limits);
    refused
        .push(
            DavProperty {
                name: PropName::Known(ElementName::CalendarData),
                value: PropValue::Empty,
            },
            &mut meter,
        )
        .expect("one property");
    let mut named = ical_dav::ErrorBody::new(limits);
    named
        .push(
            PropName::Known(ElementName::SupportedCalendarData),
            &mut meter,
        )
        .expect("one condition");
    refused.error = Some(named);

    let mut missing = PropStat::new(Status::NOT_FOUND, limits);
    missing
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Displayname),
                value: PropValue::Empty,
            },
            &mut meter,
        )
        .expect("one property");
    let mut other = ical_dav::ErrorBody::new(limits);
    other
        .push(PropName::Known(ElementName::SupportedFilter), &mut meter)
        .expect("one condition");
    missing.error = Some(other);

    let mut response = DavResponse::with_propstats(href(b"/c/1.ics").expect("an href"), limits);
    response
        .push_propstat(refused, &mut meter)
        .expect("a group");
    response
        .push_propstat(missing, &mut meter)
        .expect("a group");
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");

    let wire = encode(&built, limits).expect("the body encodes");
    let (read, _) = read_multistatus(&wire, limits).expect("the body reads");
    assert_eq!(read, built);
    // And the two conditions did not end up in one bag on the response.
    assert!(
        read.responses()
            .first()
            .and_then(|one| one.error.as_ref())
            .is_none()
    );
}

/// RFC 4791 section 9.5's three property shapes and its `timezone`, both directions.
#[test]
fn a_query_keeps_the_shape_it_asked_with_and_the_zone_it_stated() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let zone = b"BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:America/New_York\r\n\
END:VTIMEZONE\r\nEND:VCALENDAR\r\n"
        .as_slice();
    for shape in [
        ical_dav::QueryShape::Named,
        ical_dav::QueryShape::AllProp,
        ical_dav::QueryShape::Names,
    ] {
        let mut query = CalendarQuery::new(limits);
        query.shape = shape;
        query.filter = Some(CompFilter::new(b"VCALENDAR", limits, &mut meter).expect("a filter"));
        query.timezone =
            Some(CalendarPayload::from_octets(zone, limits, &mut meter).expect("a zone"));
        let wire = encode(&query, limits).expect("the query encodes");
        assert_eq!(
            read_request(&wire, limits),
            Ok(RequestBody::CalendarQuery(query.clone())),
            "{shape:?}: {}",
            String::from_utf8_lossy(&wire)
        );
        // The zone's own line endings are its content, exactly as `calendar-data`'s are.
        let RequestBody::CalendarQuery(read) = read_request(&wire, limits).expect("it reads")
        else {
            panic!("a calendar-query");
        };
        let carried = read.timezone.as_ref().expect("the zone");
        assert_eq!(carried.as_bytes(), zone);
        assert!(carried.is_as_sent());
    }
}

/// A property whose value is a peer's own elements survives a proxy, and one whose value is
/// text leaves as text.
///
/// The two halves of the split `PropValue::Unmodeled` used to be one field: a peer writing
/// `&lt;D:href&gt;…&lt;/D:href&gt;` as a *string* got a real `DAV:href` element in the body a
/// proxying server emitted.
#[test]
fn a_kept_value_leaves_as_the_kind_of_thing_it_arrived_as() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let vendor =
        ical_dav::ExtensionName::new(b"urn:x:vendor", b"note", &mut meter).expect("a vendor name");
    let mut group = PropStat::new(Status::OK, limits);
    group
        .push(
            DavProperty {
                name: PropName::Extension(vendor),
                value: PropValue::Unmodeled(
                    b"<D:href>/private/secret.ics</D:href>".to_vec().into(),
                ),
            },
            &mut meter,
        )
        .expect("one property");
    let mut response = DavResponse::with_propstats(href(b"/c/").expect("an href"), limits);
    group_into(&mut response, group, &mut meter);
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");

    let wire = encode(&built, limits).expect("the body encodes");
    assert!(
        find(&wire, b"<D:href>/private/secret.ics</D:href>").is_none(),
        "text a peer escaped became markup: {}",
        String::from_utf8_lossy(&wire)
    );
    let (read, _) = read_multistatus(&wire, limits).expect("the body reads");
    assert_eq!(read, built, "{}", String::from_utf8_lossy(&wire));
}

/// Push one group into a response, since three cases above do the same two lines.
fn group_into(response: &mut DavResponse, group: PropStat, meter: &mut Meter) {
    let pushed = response.push_propstat(group, meter);
    assert!(pushed.is_ok(), "one group is within bounds: {pushed:?}");
}
