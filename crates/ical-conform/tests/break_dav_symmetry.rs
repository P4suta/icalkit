// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One shape, both directions: `ical-dav`'s DP-15 claim, driven from each end in turn.
//!
//! The claim under attack is that no type in this crate has a field that is meaningful in only
//! one direction — that a `calendar-query` a client builds is the value a server reads out of
//! the octets, and that a multistatus a server builds is the value a client reads back. Every
//! case below therefore uses one shared type from *both* ends and compares the values, never
//! the octets: three servers spell one body three ways, so byte equality is not the claim.
//!
//! The expected shapes are taken from RFC 4791's own sections 7 and 9 and from RFC 4918's
//! section 9, not from this implementation. The request fixtures are the RFC's own example
//! bodies; the response fixtures are the RFC's own example bodies with a CalDAV payload put
//! where RFC 4791 section 7.8.1 puts one. `.gitattributes` marks fixtures `-text`, so the
//! `CRLF` inside `CALDAV:calendar-data` is the octets a server actually sent.

use ical_core::{Diagnostic, IgnoreDiagnostics, Instant, Limits, Meter};
use ical_dav::{
    CalendarDataRequest, CalendarPayload, CalendarQuery, Collation, CompFilter, CompSelection,
    DavError, DavProperty, DavResponse, DecodeContext, ETag, ElementName, ExtensionName, Href,
    MultiStatus, MultiStatusReader, ParamFilter, PropFilter, PropName, PropStat, PropValue,
    RequestBody, ResourceType, Status, TextMatch, TimeRange, WriteXml, XmlReader,
};

/// RFC 4791 section 7.8.1's own `calendar-query`.
const RFC_7_8_1_QUERY: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4791_7_8_1_calendar_query.xml");
/// Section 7.8.2's partial retrieval, which names components and properties.
const RFC_7_8_2_PARTIAL: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4791_7_8_2_partial_retrieval.xml");
/// Section 7.8.3's expanded retrieval, which asks the server to expand a recurrence.
const RFC_7_8_3_EXPAND: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4791_7_8_3_expand.xml");
/// Section 7.8.4's limited recurrence set.
const RFC_7_8_4_LIMIT: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4791_7_8_4_limit_recurrence_set.xml");
/// Section 7.8.5's pending alarms, whose `comp-filter` nests three deep.
const RFC_7_8_5_ALARMS: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4791_7_8_5_alarms_three_deep.xml");
/// A `text-match` carrying both a collation and a negate-condition, section 9.7.5.
const RFC_9_7_5_NEGATED: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4791_9_7_5_text_match_negated.xml");

/// Section 7.8.1's multistatus, with a payload whose lines end in `CRLF`.
const RFC_7_8_1_MULTISTATUS: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4791_7_8_1_multistatus.xml");
/// RFC 4918 section 9.1.3's multistatus: one property at 200 beside two at 403.
const RFC_9_1_3_MULTISTATUS: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/rfc4918_9_1_3_multistatus.xml");
/// Two property groups, each naming the precondition that explains its own refusal.
const PROPSTAT_ERRORS: &[u8] =
    include_bytes!("fixtures/break_dav_symmetry/propstat_error_conditions.xml");

/// 2006-01-04T00:00:00Z, the start of the window RFC 4791's examples ask over.
const JANUARY_4: i64 = 1_136_332_800;
/// 2006-01-05T00:00:00Z, its end.
const JANUARY_5: i64 = 1_136_419_200;
/// 2006-01-03T00:00:00Z, where sections 7.8.3 and 7.8.4 start theirs.
const JANUARY_3: i64 = 1_136_246_400;

// -------------------------------------------------------------------------------------------
// Driving one value through both halves.
// -------------------------------------------------------------------------------------------

/// Encode a value the way the direction that writes it would.
fn encode(value: &dyn WriteXml) -> Result<Vec<u8>, DavError> {
    let limits = Limits::DEFAULT;
    let mut out: Vec<u8> = Vec::new();
    let mut meter = Meter::new(limits);
    value.write_xml(&mut out, limits, &mut meter)?;
    Ok(out)
}

/// Read a request body the way the direction that reads it would.
fn read_request(body: &[u8]) -> Result<RequestBody, DavError> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
    let mut events = XmlReader::new(body);
    RequestBody::read(&mut events, &mut context)
}

/// The same read, keeping whatever reached the sink on the way.
fn read_request_reporting(body: &[u8]) -> (Result<RequestBody, DavError>, Vec<Diagnostic>) {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let read = {
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let mut events = XmlReader::new(body);
        RequestBody::read(&mut events, &mut context)
    };
    (read, reported)
}

/// Read a multistatus, keeping whatever reached the sink on the way.
fn read_multistatus(body: &[u8]) -> Result<(MultiStatus, Vec<Diagnostic>), DavError> {
    let limits = Limits::DEFAULT;
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

/// The query a client builds, read back the way the server on the other end reads it.
fn query_round_trip(query: &CalendarQuery) -> Result<RequestBody, DavError> {
    let wire = encode(query)?;
    read_request(&wire)
}

/// The multistatus a server builds, read back the way the client on the other end reads it.
fn multistatus_round_trip(body: &MultiStatus) -> Result<MultiStatus, DavError> {
    let wire = encode(body)?;
    let (read, _) = read_multistatus(&wire)?;
    Ok(read)
}

/// The same trip, keeping whatever the client's reader reported on the way.
fn multistatus_round_trip_reporting(
    body: &MultiStatus,
) -> Result<(MultiStatus, Vec<Diagnostic>), DavError> {
    let wire = encode(body)?;
    read_multistatus(&wire)
}

/// A window with both bounds.
///
/// `assert!` rather than an unwrap, because a helper outside a test function is production code
/// as far as the workspace lint profile is concerned.
fn window(from: i64, until: i64) -> TimeRange {
    let start = Instant::from_unix_seconds(from);
    let made = TimeRange::new(Some(start), Some(Instant::from_unix_seconds(until)));
    assert!(made.is_ok(), "{from}..{until} is a window: {made:?}");
    made.unwrap_or_else(|_| TimeRange::starting_at(start))
}

// -------------------------------------------------------------------------------------------
// The request half: a client builds, a server reads.
// -------------------------------------------------------------------------------------------

/// Everything RFC 4791 section 9.7 lets a filter say, in one query, through both halves.
#[test]
fn a_filter_carrying_everything_section_9_7_defines_reads_back_as_itself() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut query = CalendarQuery::new(limits);
    query
        .props
        .push(PropName::Known(ElementName::Getetag), &mut meter)
        .expect("one name");

    // Section 9.7.5: a collation beside a negate-condition, with a parameter test under it.
    let mut attendee = PropFilter::new(b"ATTENDEE", limits, &mut meter).expect("a prop filter");
    let mut matcher = TextMatch::new(b"mailto:lisa@example.com", &mut meter).expect("a test");
    matcher.collation = Collation::Octet;
    matcher.negate = true;
    attendee.text_match = Some(matcher);
    let mut partstat = ParamFilter::new(b"PARTSTAT", &mut meter).expect("a param filter");
    partstat.text_match = Some(TextMatch::new(b"NEEDS-ACTION", &mut meter).expect("a test"));
    attendee.push_param(partstat, &mut meter).expect("a param");

    // Section 9.7.2: the negation, which is exclusive with every other test beside it.
    let mut absent = PropFilter::new(b"X-ABC-GUID", limits, &mut meter).expect("a prop filter");
    absent.is_not_defined = true;

    // Section 7.8.5: three levels of `comp-filter`, with a window on the innermost.
    let mut alarms = CompFilter::new(b"VALARM", limits, &mut meter).expect("a comp filter");
    alarms.time_range = Some(window(JANUARY_4, JANUARY_5));
    let mut events = CompFilter::new(b"VEVENT", limits, &mut meter).expect("a comp filter");
    events.time_range = Some(window(JANUARY_4, JANUARY_5));
    events
        .push_prop(attendee, &mut meter)
        .expect("a prop filter");
    events.push_prop(absent, &mut meter).expect("a prop filter");
    events
        .push_comp(alarms, limits, &mut meter)
        .expect("a nested comp filter");
    let mut calendar = CompFilter::new(b"VCALENDAR", limits, &mut meter).expect("a comp filter");
    calendar
        .push_comp(events, limits, &mut meter)
        .expect("a nested comp filter");
    query.filter = Some(calendar);

    assert_eq!(
        query_round_trip(&query),
        Ok(RequestBody::CalendarQuery(query.clone()))
    );
}

/// Section 9.6's `calendar-data` request, with the shape and the two windows it may carry.
#[test]
fn a_calendar_data_request_with_a_shape_and_two_windows_reads_back_as_itself() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut event = CompSelection::new(b"VEVENT", limits, &mut meter).expect("a selection");
    event.push_prop(b"SUMMARY", &mut meter).expect("a property");
    event.push_prop(b"UID", &mut meter).expect("a property");
    let mut alarm = CompSelection::new(b"VALARM", limits, &mut meter).expect("a selection");
    alarm.all_props = true;
    event.push_comp(alarm, &mut meter).expect("a subcomponent");
    let mut calendar = CompSelection::new(b"VCALENDAR", limits, &mut meter).expect("a selection");
    calendar
        .push_comp(event, &mut meter)
        .expect("a subcomponent");

    let mut query = CalendarQuery::new(limits);
    query.props.calendar_data = Some(CalendarDataRequest {
        expand: Some(window(JANUARY_3, JANUARY_5)),
        limit_recurrence_set: None,
        limit_freebusy_set: Some(window(JANUARY_4, JANUARY_5)),
        comp: Some(calendar),
    });
    assert_eq!(
        query_round_trip(&query),
        Ok(RequestBody::CalendarQuery(query.clone()))
    );

    // The other of the two alternatives section 9.6's grammar offers beside `expand`.
    let mut limited = CalendarQuery::new(limits);
    limited.props.calendar_data = Some(CalendarDataRequest {
        expand: None,
        limit_recurrence_set: Some(window(JANUARY_3, JANUARY_5)),
        limit_freebusy_set: None,
        comp: None,
    });
    assert_eq!(
        query_round_trip(&limited),
        Ok(RequestBody::CalendarQuery(limited.clone()))
    );
}

/// Section 9.9 makes both bounds independently optional, in every element that carries a window.
#[test]
fn a_window_with_one_open_bound_stays_open_in_every_element_that_carries_one() {
    let limits = Limits::DEFAULT;
    let open_start = TimeRange::ending_before(Instant::from_unix_seconds(JANUARY_5));
    let open_end = TimeRange::starting_at(Instant::from_unix_seconds(JANUARY_4));
    for range in [open_start, open_end] {
        let mut meter = Meter::new(limits);
        let mut events = CompFilter::new(b"VEVENT", limits, &mut meter).expect("a comp filter");
        events.time_range = Some(range);
        let mut calendar =
            CompFilter::new(b"VCALENDAR", limits, &mut meter).expect("a comp filter");
        calendar
            .push_comp(events, limits, &mut meter)
            .expect("a nested comp filter");
        let mut query = CalendarQuery::new(limits);
        query.filter = Some(calendar);
        query.props.calendar_data = Some(CalendarDataRequest {
            expand: Some(range),
            limit_recurrence_set: None,
            limit_freebusy_set: Some(range),
            comp: None,
        });
        assert_eq!(
            query_round_trip(&query),
            Ok(RequestBody::CalendarQuery(query.clone()))
        );
    }
}

/// The RFC's own request bodies, read as a server and written back as a client.
///
/// Value equality across the re-encode, not byte equality: the RFC's indentation and this
/// crate's compact output are two spellings of one body. What must not change is the value.
#[test]
fn every_rfc_4791_request_example_survives_being_read_and_written_back() {
    let cases = [
        ("section 7.8.1", RFC_7_8_1_QUERY),
        ("section 7.8.2", RFC_7_8_2_PARTIAL),
        ("section 7.8.3", RFC_7_8_3_EXPAND),
        ("section 7.8.4", RFC_7_8_4_LIMIT),
        ("section 7.8.5", RFC_7_8_5_ALARMS),
        ("section 9.7.5", RFC_9_7_5_NEGATED),
    ];
    for (which, wire) in cases {
        let RequestBody::CalendarQuery(read) = read_request(wire)
            .unwrap_or_else(|refused| panic!("{which} is a calendar-query: {refused:?}"))
        else {
            panic!("{which} is a calendar-query");
        };
        assert_eq!(
            query_round_trip(&read),
            Ok(RequestBody::CalendarQuery(read.clone())),
            "{which}"
        );
    }
}

/// Section 7.8.5's filter is three `comp-filter`s deep, and stays three deep.
#[test]
fn the_three_deep_filter_of_section_7_8_5_arrives_three_deep() {
    let RequestBody::CalendarQuery(query) =
        read_request(RFC_7_8_5_ALARMS).expect("a calendar-query")
    else {
        panic!("a calendar-query");
    };
    let calendar = query.filter.as_ref().expect("a filter");
    assert_eq!(calendar.name(), b"VCALENDAR");
    let events = calendar.comps().first().expect("VEVENT");
    assert_eq!(events.name(), b"VEVENT");
    let alarms = events.comps().first().expect("VALARM");
    assert_eq!(alarms.name(), b"VALARM");
    assert_eq!(
        alarms.time_range.and_then(TimeRange::start),
        Some(Instant::from_unix_seconds(JANUARY_4))
    );
}

/// Section 9.7.5's collation and negate-condition both arrive.
#[test]
fn a_text_match_keeps_its_collation_and_its_negation_in_both_directions() {
    let RequestBody::CalendarQuery(query) =
        read_request(RFC_9_7_5_NEGATED).expect("a calendar-query")
    else {
        panic!("a calendar-query");
    };
    let events = query
        .filter
        .as_ref()
        .and_then(|root| root.comps().first())
        .expect("VEVENT");
    let attendee = events.props().first().expect("the ATTENDEE filter");
    let matcher = attendee.text_match.as_ref().expect("a text-match");
    assert_eq!(matcher.collation, Collation::Octet);
    assert!(
        matcher.negate,
        "negate-condition=\"yes\" is what the body said"
    );
    assert_eq!(matcher.value(), b"mailto:lisa@example.com");
}

/// Section 9.6.1's grammar offers `allprop` or named properties, and a value may hold both.
#[test]
fn a_selection_naming_properties_beside_allprop_is_not_silently_reduced() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut calendar = CompSelection::new(b"VCALENDAR", limits, &mut meter).expect("a selection");
    calendar.all_props = true;
    calendar
        .push_prop(b"VERSION", &mut meter)
        .expect("a property");
    let mut query = CalendarQuery::new(limits);
    query.props.calendar_data = Some(CalendarDataRequest {
        expand: None,
        limit_recurrence_set: None,
        limit_freebusy_set: None,
        comp: Some(calendar),
    });
    // Either the encoder refuses the value it cannot express — as it refuses a filter that
    // states a condition and its own negation — or the value survives. Dropping the named
    // property and reporting nothing is the third answer, and it is the one that loses.
    let Ok(wire) = encode(&query) else {
        return;
    };
    assert_eq!(
        read_request(&wire),
        Ok(RequestBody::CalendarQuery(query.clone())),
        "{}",
        String::from_utf8_lossy(&wire)
    );
}

// -------------------------------------------------------------------------------------------
// The response half: a server builds, a client reads.
// -------------------------------------------------------------------------------------------

/// One `href` reporting a property at 200 beside one at 404, out of one shared type.
#[test]
fn one_href_with_a_property_at_200_and_another_at_404_reads_back_as_itself() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let payload = CalendarPayload::from_octets(
        b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
        limits,
        &mut meter,
    )
    .expect("a payload");

    let mut found = PropStat::new(Status::OK, limits);
    found
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Getetag),
                value: PropValue::Entity(ETag::parse(b"\"fffff-abcd2\"").expect("a tag")),
            },
            &mut meter,
        )
        .expect("a property");
    found
        .push(
            DavProperty {
                name: PropName::Known(ElementName::CalendarData),
                value: PropValue::CalendarData(payload),
            },
            &mut meter,
        )
        .expect("a property");
    let mut missing = PropStat::new(Status::NOT_FOUND, limits);
    missing
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Displayname),
                value: PropValue::Empty,
            },
            &mut meter,
        )
        .expect("a property");

    let href = Href::new(b"/bernard/work/abcd2.ics", limits, &mut meter).expect("an href");
    let mut response = DavResponse::with_propstats(href, limits);
    response.push_propstat(found, &mut meter).expect("a group");
    response
        .push_propstat(missing, &mut meter)
        .expect("a group");
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");

    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// RFC 4791 section 7.8.1's own multistatus, read as a client and written back as a server.
#[test]
fn the_multistatus_of_section_7_8_1_survives_a_read_and_a_write() {
    let (read, _) = read_multistatus(RFC_7_8_1_MULTISTATUS).expect("a multistatus");
    assert_eq!(read.responses().len(), 3);

    // The payload is the octets the server sent, `CRLF` and fold intact.
    let first = read.responses().first().expect("a response");
    let Some(PropValue::CalendarData(payload)) =
        first.successful_value(&PropName::Known(ElementName::CalendarData))
    else {
        panic!("the payload came back under its success status");
    };
    assert!(payload.is_as_sent());
    assert!(
        payload.as_bytes().windows(3).any(|at| at == b"\r\n "),
        "the fold RFC 5545 section 3.1 wrote is still a fold"
    );
    assert!(payload.as_bytes().ends_with(b"END:VCALENDAR\r\n"));

    // The second response reports one property at 200 and another at 403, and the two must
    // not collapse: `calendar-data` was refused there and must not read as returned.
    let second = read.responses().get(1).expect("a response");
    assert_eq!(second.propstats().len(), 2);
    assert!(
        second
            .successful_value(&PropName::Known(ElementName::CalendarData))
            .is_none()
    );

    assert_eq!(multistatus_round_trip(&read), Ok(read));
}

/// RFC 4918 section 9.1.3's multistatus, whose properties are a peer's own structure.
#[test]
fn a_structured_extension_property_is_not_flattened_by_the_trip() {
    let (read, _) = read_multistatus(RFC_9_1_3_MULTISTATUS).expect("a multistatus");
    let response = read.responses().first().expect("a response");
    assert_eq!(response.propstats().len(), 2);
    let bigbox = response
        .propstats()
        .first()
        .and_then(|group| group.props().first())
        .expect("the first property of the 200 group");
    let PropName::Extension(named) = &bigbox.name else {
        panic!("R:bigbox is outside this crate's vocabulary");
    };
    assert_eq!(named.local_name(), b"bigbox");

    // Written back by a server proxying another server's properties, the structure the peer
    // sent has to survive: `R:BoxType` is the property's value, not decoration around it.
    let written = encode(&read).expect("the multistatus encodes");
    assert!(
        String::from_utf8_lossy(&written).contains("BoxType"),
        "the peer's own element is gone from the body written back:\n{}",
        String::from_utf8_lossy(&written)
    );
}

/// A precondition RFC 4918 section 14.22 puts inside one `propstat` belongs to that group.
#[test]
fn a_precondition_named_inside_one_propstat_is_not_moved_to_another() {
    let (read, reported) = read_multistatus(PROPSTAT_ERRORS).expect("a multistatus");
    let codes: Vec<_> = reported.iter().map(|one| one.code()).collect();
    let response = read.responses().first().expect("a response");
    assert_eq!(response.propstats().len(), 3);

    // The body named one condition under the 403 and a different one under the 404. A client
    // that has to tell a user why `calendar-data` was refused cannot read both off one bag.
    let conditions = response
        .error
        .as_ref()
        .map(|named| named.conditions().to_vec())
        .unwrap_or_default();
    assert!(
        conditions.len() <= 1,
        "two groups' preconditions were merged into one bag: {conditions:?}, \
         with {codes:?} on the sink"
    );
}

/// A property whose value is octets this crate has no model for keeps those octets.
#[test]
fn a_property_with_no_modeled_shape_is_written_back_as_it_was_given() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let vendor = ExtensionName::new(b"http://apple.com/ns/ical/", b"calendar-order", &mut meter)
        .expect("a name");
    let mut group = PropStat::new(Status::OK, limits);
    group
        .push(
            DavProperty {
                name: PropName::Extension(vendor),
                value: PropValue::Unmodeled(b"<D:href>/calendars/ann/</D:href>".to_vec().into()),
            },
            &mut meter,
        )
        .expect("a property");
    let href = Href::new(b"/calendars/ann/", limits, &mut meter).expect("an href");
    let mut response = DavResponse::with_propstats(href, limits);
    response.push_propstat(group, &mut meter).expect("a group");
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");

    let round = multistatus_round_trip_reporting(&built);
    assert!(
        round.is_ok(),
        "the client refused the body the server wrote: {round:?}"
    );
    let (read, reported) = round.unwrap_or((MultiStatus::new(limits), Vec::new()));
    let codes: Vec<_> = reported.iter().map(|one| one.code()).collect();
    assert_eq!(read, built, "with {codes:?} on the sink");
}

/// A property whose value is text is a different fact from a property that arrived empty.
#[test]
fn a_property_whose_text_is_only_spaces_is_not_an_absent_property() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut group = PropStat::new(Status::OK, limits);
    group
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Displayname),
                value: PropValue::Text(b" ".to_vec().into()),
            },
            &mut meter,
        )
        .expect("a property");
    let href = Href::new(b"/calendars/ann/", limits, &mut meter).expect("an href");
    let mut response = DavResponse::with_propstats(href, limits);
    response.push_propstat(group, &mut meter).expect("a group");
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");

    let round = multistatus_round_trip_reporting(&built);
    assert!(
        round.is_ok(),
        "the client refused the body the server wrote: {round:?}"
    );
    let (read, reported) = round.unwrap_or((MultiStatus::new(limits), Vec::new()));
    let codes: Vec<_> = reported.iter().map(|one| one.code()).collect();
    assert_eq!(read, built, "with {codes:?} on the sink");
}

/// The property shapes a `PROPFIND` on a calendar home answers with, from both ends.
#[test]
fn a_resource_type_and_a_reference_property_survive_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut claimed = ResourceType::new(limits);
    claimed.collection = true;
    claimed.calendar = true;
    claimed
        .push_other(
            ExtensionName::new(b"http://calendarserver.org/ns/", b"shared", &mut meter)
                .expect("a name"),
            &mut meter,
        )
        .expect("a claim");

    let mut group = PropStat::new(Status::OK, limits);
    group
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Resourcetype),
                value: PropValue::Resource(claimed),
            },
            &mut meter,
        )
        .expect("a property");
    group
        .push(
            DavProperty {
                name: PropName::Known(ElementName::CalendarHomeSet),
                value: PropValue::Reference(
                    Href::new(b"/calendars/ann/", limits, &mut meter).expect("an href"),
                ),
            },
            &mut meter,
        )
        .expect("a property");
    group
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Getctag),
                value: PropValue::Text(b"1234".to_vec().into()),
            },
            &mut meter,
        )
        .expect("a property");

    let href = Href::new(b"/calendars/ann/work/", limits, &mut meter).expect("an href");
    let mut response = DavResponse::with_propstats(href, limits);
    response.push_propstat(group, &mut meter).expect("a group");
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");

    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

// -------------------------------------------------------------------------------------------
// The corners each side reaches for and the other may not.
// -------------------------------------------------------------------------------------------

/// One property group in one response, for the cases below that vary only the property.
fn one_property(property: DavProperty) -> MultiStatus {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut group = PropStat::new(Status::OK, limits);
    let pushed = group.push(property, &mut meter);
    assert!(pushed.is_ok(), "one property is within bounds: {pushed:?}");
    let named = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter);
    assert!(named.is_ok(), "one href is within bounds: {named:?}");
    let mut built = MultiStatus::new(limits);
    let Ok(href) = named else { return built };
    let mut response = DavResponse::with_propstats(href, limits);
    let grouped = response.push_propstat(group, &mut meter);
    assert!(grouped.is_ok(), "one group is within bounds: {grouped:?}");
    let stored = built.push(response, &mut meter);
    assert!(stored.is_ok(), "one response is within bounds: {stored:?}");
    built
}

/// An `href` is byte-shaped so that a response one can read is a response one can model.
///
/// `value.rs` states the reason: "a server is free to emit octets that are not UTF-8 in a path,
/// and a type that cannot model a response one can read is the failure this workspace exists to
/// prevent". A path a server can hold has to be one this crate writes and reads back.
#[test]
fn an_href_whose_octets_are_not_utf_8_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    // `ete.ics` with two Latin-1 accented octets, which is what a path out of an old store
    // looks like on the wire.
    let latin1 = b"/calendars/ann/\xe9t\xe9.ics".as_slice();
    let href = Href::new(latin1, limits, &mut meter).expect("an href");
    let mut built = MultiStatus::new(limits);
    built
        .push(DavResponse::with_status(href, Status::OK), &mut meter)
        .expect("a response");
    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// A payload is opaque octets, and what a server stored is what a client must read back.
#[test]
fn a_payload_carrying_markup_and_line_endings_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    // Every octet an escaper has to think about: an ampersand, a `<`, the `]]>` that would
    // close a section this crate never opens, and a fold whose continuation begins with a
    // space.
    let awkward = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:1@x\r\nDESCRIPTION:a<b & c ]]> d\r\n e\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".as_slice();
    let payload = CalendarPayload::from_octets(awkward, limits, &mut meter).expect("a payload");
    let built = one_property(DavProperty {
        name: PropName::Known(ElementName::CalendarData),
        value: PropValue::CalendarData(payload),
    });
    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// A server that stored bare `LF` terminators, which RFC 4791 section 9.6 permits it to send.
#[test]
fn a_payload_stored_with_bare_line_feeds_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let bare = b"BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n".as_slice();
    let payload = CalendarPayload::from_octets(bare, limits, &mut meter).expect("a payload");
    let built = one_property(DavProperty {
        name: PropName::Known(ElementName::CalendarData),
        value: PropValue::CalendarData(payload),
    });
    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// An `href` and a text value carrying the octets an escaper has to think about.
#[test]
fn a_property_value_carrying_markup_characters_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let href = Href::new(b"/calendars/ann/a&b<c.ics", limits, &mut meter).expect("an href");
    let mut group = PropStat::new(Status::OK, limits);
    group
        .push(
            DavProperty {
                name: PropName::Known(ElementName::Displayname),
                value: PropValue::Text(b"Ann & Bob <work>".to_vec().into()),
            },
            &mut meter,
        )
        .expect("a property");
    let mut response = DavResponse::with_propstats(href, limits);
    response.push_propstat(group, &mut meter).expect("a group");
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");
    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// A group reporting a status over an empty property list is still a group.
#[test]
fn a_property_group_with_no_properties_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let href = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter).expect("an href");
    let mut response = DavResponse::with_propstats(href, limits);
    response
        .push_propstat(PropStat::new(Status::NOT_FOUND, limits), &mut meter)
        .expect("a group");
    let mut built = MultiStatus::new(limits);
    built.push(response, &mut meter).expect("a response");
    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// A refusal RFC 4918 section 16 states for a whole resource, with the conditions it names.
#[test]
fn a_response_level_precondition_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let href = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter).expect("an href");
    let mut refused = DavResponse::with_status(href, Status::FORBIDDEN);
    let mut named = ical_dav::ErrorBody::new(limits);
    named
        .push(
            PropName::Known(ElementName::AllowedOrganizerSchedulingObjectChange),
            &mut meter,
        )
        .expect("a condition");
    named
        .push(
            PropName::Extension(
                ExtensionName::new(b"http://example.invalid/ns/", b"quota-reached", &mut meter)
                    .expect("a name"),
            ),
            &mut meter,
        )
        .expect("a condition");
    refused.error = Some(named);
    let mut built = MultiStatus::new(limits);
    built.push(refused, &mut meter).expect("a response");
    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// A synchronization answer that changed nothing is a token and no responses.
#[test]
fn a_multistatus_that_is_only_a_sync_token_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut built = MultiStatus::new(limits);
    built.sync_token = Some(
        ical_dav::SyncToken::new(b"http://sabre.io/ns/sync/9", limits, &mut meter)
            .expect("a token"),
    );
    assert_eq!(multistatus_round_trip(&built), Ok(built));
}

/// A property name outside the vocabulary, in a request, from both ends.
#[test]
fn an_extension_property_name_in_a_request_survives_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut query = CalendarQuery::new(limits);
    query
        .props
        .push(
            PropName::Extension(
                ExtensionName::new(b"http://apple.com/ns/ical/", b"calendar-color", &mut meter)
                    .expect("a name"),
            ),
            &mut meter,
        )
        .expect("a name");
    query
        .props
        .push(
            PropName::Extension(
                ExtensionName::new(b"", b"unnamespaced", &mut meter).expect("a name"),
            ),
            &mut meter,
        )
        .expect("a name");
    assert_eq!(
        query_round_trip(&query),
        Ok(RequestBody::CalendarQuery(query.clone()))
    );
}

/// A `prop-filter` may carry a window, and a `comp-filter` may say the component is absent.
#[test]
fn a_windowed_property_filter_and_an_absent_component_survive_both_directions() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut due = PropFilter::new(b"DUE", limits, &mut meter).expect("a prop filter");
    due.time_range = Some(window(JANUARY_4, JANUARY_5));
    let mut todos = CompFilter::new(b"VTODO", limits, &mut meter).expect("a comp filter");
    todos.push_prop(due, &mut meter).expect("a prop filter");
    let mut journals = CompFilter::new(b"VJOURNAL", limits, &mut meter).expect("a comp filter");
    journals.is_not_defined = true;
    let mut calendar = CompFilter::new(b"VCALENDAR", limits, &mut meter).expect("a comp filter");
    calendar
        .push_comp(todos, limits, &mut meter)
        .expect("a nested filter");
    calendar
        .push_comp(journals, limits, &mut meter)
        .expect("a nested filter");
    let mut query = CalendarQuery::new(limits);
    query.filter = Some(calendar);
    assert_eq!(
        query_round_trip(&query),
        Ok(RequestBody::CalendarQuery(query.clone()))
    );
}

/// RFC 4791 section 9.5's grammar puts `DAV:allprop` and `DAV:propname` inside a
/// `calendar-query`.
///
/// `<!ELEMENT calendar-query ((DAV:allprop | DAV:propname | DAV:prop)?, filter, timezone?)>` is
/// the RFC's own production, and `CalendarQuery::props` is a property list and nothing else, so
/// a client here cannot build two of the three shapes. What a server does with the body some
/// other client sends is the half that can still be observed.
#[test]
fn a_calendar_query_asking_for_every_property_is_a_body_rfc_4791_defines() {
    let wire = br#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:allprop/>
  <C:filter>
    <C:comp-filter name="VCALENDAR"/>
  </C:filter>
</C:calendar-query>"#;
    let read = read_request(wire);
    assert!(
        read.is_ok(),
        "section 9.5's grammar admits DAV:allprop here: {read:?}"
    );
}

/// Section 9.5's `CALDAV:timezone` states the zone a floating `time-range` is resolved in.
#[test]
fn a_calendar_query_stating_its_timezone_does_not_lose_it_silently() {
    let wire = br#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/></D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20060104T000000Z" end="20060105T000000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
  <C:timezone>BEGIN:VCALENDAR&#13;
BEGIN:VTIMEZONE&#13;
TZID:America/New_York&#13;
END:VTIMEZONE&#13;
END:VCALENDAR&#13;
</C:timezone>
</C:calendar-query>"#;
    // A zone the client stated and the server ignored resolves a floating window differently,
    // so the two honest answers are "carried" and "refused" — never "dropped without a word".
    let (read, reported) = read_request_reporting(wire);
    let RequestBody::CalendarQuery(query) = read.unwrap_or_else(|refused| {
        panic!("refusing is one of the two honest answers, and this is not it: {refused:?}")
    }) else {
        panic!("a calendar-query");
    };
    let written = encode(&query).expect("it encodes");
    let codes: Vec<_> = reported.iter().map(|one| one.code()).collect();
    assert!(
        String::from_utf8_lossy(&written).contains("timezone"),
        "the zone the client stated is gone from the body a proxy writes back, \
         with {codes:?} on the sink:\n{}",
        String::from_utf8_lossy(&written)
    );
}
