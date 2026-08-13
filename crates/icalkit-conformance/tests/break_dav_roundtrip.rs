// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! ADR-0001's round trip, carried through the CalDAV envelope and back.
//!
//! The claim under attack is the one M4 was built to resolve: a calendar that goes into a
//! `<C:calendar-data>` element comes out of it as the octets that went in, so that
//! `serialize(parse(recovered)) == sent` and a client that read a resource can write it back
//! without rewriting somebody else's data. `docs/adr/0004` Amendment 1 states it, `TextPolicy`
//! implements it, and `crates/ical-dav/tests/calendar_data_collision.rs` proves it for one
//! shape against `decode_text` alone — with the tokenizer, the property reader and
//! `CalendarPayload` all bypassed by a hand-written span finder.
//!
//! This file drives the whole path instead. Each case is a multistatus shaped like something a
//! real server sends; each is read with `XmlReader` and `MultiStatusReader`, the
//! `calendar-data` is taken out of the `PropValue` it landed in, and the octets are compared
//! against the `.ics` that was inside the element. Then those octets go to `ical-core` and the
//! document it writes back is compared against the same `.ics`.
//!
//! The shapes: literal `CRLF` with a content line folded at `CRLF SPACE`, the same calendar
//! with every `CR` as `&#13;`, the same calendar inside a `CDATA` section, a `SUMMARY` holding
//! `&`, `<` and `>`, a lone `CR` inside a value, a last line with no terminator, an `X-`
//! property carrying octets that are not UTF-8, and a calendar mixing `CRLF` and bare `LF`.
//! The last case runs the other way: a server builds a multistatus around a `CRLF` calendar,
//! writes it, and the bytes are read back and compared against the calendar's own.

use icalkit_conformance::internal::core::{
    Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics, Limits, Meter,
};
use icalkit_conformance::internal::dav::{
    CalendarPayload, DavProperty, DavResponse, DecodeContext, ElementName, Href, LineEndings,
    MultiStatus, MultiStatusReader, MultiStatusWriter, PropName, PropStat, PropValue, ResponseBody,
    ResponseSource, Status, TextPolicy, WriteXml, XmlReader,
};

/// One case: the multistatus a server sent, and the `.ics` that is inside its element.
struct Case {
    /// What the case is called in a failure.
    name: &'static str,
    /// The whole response body.
    body: &'static [u8],
    /// The octets between the `calendar-data` start tag and its end tag, decoded.
    payload: &'static [u8],
}

const CASES: &[Case] = &[
    Case {
        name: "crlf_literal",
        body: include_bytes!("fixtures/break_dav_roundtrip/crlf_literal.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/crlf_literal.ics"),
    },
    Case {
        name: "charref_crlf",
        body: include_bytes!("fixtures/break_dav_roundtrip/charref_crlf.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/charref_crlf.ics"),
    },
    Case {
        name: "cdata",
        body: include_bytes!("fixtures/break_dav_roundtrip/cdata.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/cdata.ics"),
    },
    Case {
        name: "xml_significant",
        body: include_bytes!("fixtures/break_dav_roundtrip/xml_significant.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/xml_significant.ics"),
    },
    Case {
        name: "lone_cr",
        body: include_bytes!("fixtures/break_dav_roundtrip/lone_cr.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/lone_cr.ics"),
    },
    Case {
        name: "no_final_terminator",
        body: include_bytes!("fixtures/break_dav_roundtrip/no_final_terminator.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/no_final_terminator.ics"),
    },
    Case {
        name: "not_utf8",
        body: include_bytes!("fixtures/break_dav_roundtrip/not_utf8.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/not_utf8.ics"),
    },
    Case {
        name: "mixed_terminators",
        body: include_bytes!("fixtures/break_dav_roundtrip/mixed_terminators.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/mixed_terminators.ics"),
    },
    Case {
        name: "fold_inside_utf8",
        body: include_bytes!("fixtures/break_dav_roundtrip/fold_inside_utf8.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/fold_inside_utf8.ics"),
    },
    Case {
        name: "charref_both",
        body: include_bytes!("fixtures/break_dav_roundtrip/charref_both.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/charref_both.ics"),
    },
    Case {
        name: "charref_hex",
        body: include_bytes!("fixtures/break_dav_roundtrip/charref_hex.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/charref_hex.ics"),
    },
    Case {
        name: "cdata_lone_cr",
        body: include_bytes!("fixtures/break_dav_roundtrip/cdata_lone_cr.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/cdata_lone_cr.ics"),
    },
    Case {
        name: "cdata_split",
        body: include_bytes!("fixtures/break_dav_roundtrip/cdata_split.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/cdata_split.ics"),
    },
    Case {
        name: "cdata_bracket",
        body: include_bytes!("fixtures/break_dav_roundtrip/cdata_bracket.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/cdata_bracket.ics"),
    },
    Case {
        name: "cdata_then_text",
        body: include_bytes!("fixtures/break_dav_roundtrip/cdata_then_text.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/cdata_then_text.ics"),
    },
    Case {
        name: "bom_payload",
        body: include_bytes!("fixtures/break_dav_roundtrip/bom_payload.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/bom_payload.ics"),
    },
    Case {
        name: "astral_charref",
        body: include_bytes!("fixtures/break_dav_roundtrip/astral_charref.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/astral_charref.ics"),
    },
    Case {
        name: "blank_line",
        body: include_bytes!("fixtures/break_dav_roundtrip/blank_line.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/blank_line.ics"),
    },
    Case {
        name: "with_attributes",
        body: include_bytes!("fixtures/break_dav_roundtrip/with_attributes.xml"),
        payload: include_bytes!("fixtures/break_dav_roundtrip/with_attributes.ics"),
    },
];

/// How many carriage returns a run of octets holds.
///
/// A fold rather than `filter(..).count()`, which `clippy::naive_bytecount` asks to be written
/// through a crate this workspace's purity rule does not admit.
fn carriage_returns(octets: &[u8]) -> usize {
    const CARRIAGE_RETURN: u8 = 13;
    octets.iter().fold(0_usize, |seen, byte| match *byte {
        CARRIAGE_RETURN => seen.saturating_add(1),
        _ => seen,
    })
}

/// Whether a case's payload is one an XML document can carry at all.
///
/// A document declares an encoding and this crate's declares UTF-8, so octets that are not
/// UTF-8 have no representation inside one: not escaped, not as a character reference, not at
/// all. `what_this_crate_writes_is_a_well_formed_utf8_document` is where that is the subject,
/// and the answer it settles on is that the encoder refuses rather than emitting a body the
/// peer discards whole. The four cases below are about what the encoder *writes*, so a payload
/// it may not write has nothing to say to them — the refusal is asserted where it belongs and
/// is not re-asserted here as a failure.
///
/// Two fixtures land here. `not_utf8` is a resource in some other encoding. `fold_inside_utf8`
/// is worse and is recorded in ADR 0001's register: an RFC 5545 fold that falls between a lead
/// octet and its continuations is a file this workspace round-trips byte for byte and which
/// therefore has **no CalDAV representation**. That is a fact about the envelope.
fn is_writable(case: &Case) -> bool {
    core::str::from_utf8(case.payload).is_ok()
}

/// Read a whole multistatus through the tokenizer and reader that ship.
fn read(
    body: &[u8],
) -> Result<(MultiStatus, Vec<Diagnostic>), icalkit_conformance::internal::dav::DavError> {
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

/// The `calendar-data` of the first response, and the witness it arrived with.
fn payload_of(collected: &MultiStatus) -> Option<(&[u8], LineEndings)> {
    let wanted = PropName::Known(ElementName::CalendarData);
    let first = collected.responses().first()?;
    match first.successful_value(&wanted)? {
        PropValue::CalendarData(carried) => Some((carried.as_bytes(), carried.line_endings())),
        _ => None,
    }
}

/// Whatever the first response's `calendar-data` property turned into, shape included.
///
/// Separate from [`payload_of`] because "the payload differed" and "the payload is not a
/// payload" are different findings, and a test that only asked for the octets would report the
/// second as an absence.
fn value_of(collected: &MultiStatus) -> Option<&PropValue> {
    let wanted = PropName::Known(ElementName::CalendarData);
    collected.responses().first()?.successful_value(&wanted)
}

/// Parse under the default policy and write back, keeping what was diagnosed.
fn through_core(octets: &[u8]) -> Option<Vec<u8>> {
    let mut diagnostics = Vec::new();
    Document::parse(octets, Limits::DEFAULT, &mut diagnostics)
        .ok()
        .map(|tree| tree.to_bytes())
}

#[test]
fn the_fixtures_still_carry_the_octets_every_case_is_about() {
    // `.gitattributes` marks these `-text`; a checkout that rewrote line endings would make
    // every assertion below vacuous, so the octets are asserted before they are read.
    for case in CASES {
        let has_crlf = case.payload.windows(2).any(|pair| pair == b"\r\n");
        assert!(has_crlf, "{}: no CRLF left in the fixture", case.name);
    }
    let lone = CASES
        .iter()
        .find(|case| case.name == "lone_cr")
        .expect("the lone CR case");
    assert!(lone.payload.windows(2).any(|pair| pair == b"e\r"));
    let blob = CASES
        .iter()
        .find(|case| case.name == "not_utf8")
        .expect("the non-UTF-8 case");
    assert!(core::str::from_utf8(blob.payload).is_err());
    let tail = CASES
        .iter()
        .find(|case| case.name == "no_final_terminator")
        .expect("the unterminated case");
    assert!(!tail.payload.ends_with(b"\n"));
}

/// D2. The octets inside the element are the octets the caller is handed.
#[test]
fn every_shape_of_calendar_data_reaches_the_caller_as_the_server_wrote_it() {
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        let Ok((collected, _)) = read(case.body) else {
            broken.push(format!("{}: the body was refused", case.name));
            continue;
        };
        let Some((octets, _)) = payload_of(&collected) else {
            broken.push(format!(
                "{}: no calendar-data came back; the value was {:?}",
                case.name,
                value_of(&collected)
            ));
            continue;
        };
        if octets != case.payload {
            broken.push(format!(
                "{}: got {:?}, sent {:?}",
                case.name, octets, case.payload
            ));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// D2. What `ical-core` writes back is what was inside the element.
#[test]
fn what_ical_core_writes_back_is_what_was_inside_the_element() {
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        let Ok((collected, _)) = read(case.body) else {
            broken.push(format!("{}: the body was refused", case.name));
            continue;
        };
        let Some((octets, _)) = payload_of(&collected) else {
            broken.push(format!("{}: no calendar-data came back", case.name));
            continue;
        };
        let Some(written) = through_core(octets) else {
            broken.push(format!(
                "{}: ical-core refused the recovered payload",
                case.name
            ));
            continue;
        };
        if written.as_slice() != case.payload {
            broken.push(format!(
                "{}: ical-core wrote {:?}, the element held {:?}",
                case.name, written, case.payload
            ));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// The witness has to agree with the octets, or a caller cannot use it to decide anything.
#[test]
fn the_line_ending_witness_describes_the_octets_it_travels_with() {
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        let Ok((collected, _)) = read(case.body) else {
            continue;
        };
        let Some((octets, witness)) = payload_of(&collected) else {
            continue;
        };
        let expected = LineEndings::of(octets);
        if witness != expected {
            broken.push(format!(
                "{}: the payload says {witness:?} and its octets are {expected:?}",
                case.name
            ));
        }
        if !witness.is_as_sent() {
            broken.push(format!("{}: a verbatim read reported a fold", case.name));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// D2, the other direction: a server's own octets are the octets on its wire.
#[test]
fn a_server_writes_the_calendar_it_holds_and_a_client_reads_that_calendar_back() {
    let limits = Limits::DEFAULT;
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        if !is_writable(case) {
            continue;
        }
        let mut meter = Meter::new(limits);
        let held = CalendarPayload::from_octets(case.payload, limits, &mut meter)
            .expect("a payload inside the default bounds");
        let mut body = MultiStatus::new(limits);
        let target = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter)
            .expect("an href inside the default bounds");
        let mut response = DavResponse::with_propstats(target, limits);
        let mut group = PropStat::new(Status::OK, limits);
        group
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::CalendarData(held),
                },
                &mut meter,
            )
            .expect("one property inside the default bounds");
        response
            .push_propstat(group, &mut meter)
            .expect("one propstat inside the default bounds");
        body.push(response, &mut meter)
            .expect("one response inside the default bounds");

        let mut wire: Vec<u8> = Vec::new();
        if body.write_xml(&mut wire, limits, &mut meter).is_err() {
            broken.push(format!("{}: the encoder refused the payload", case.name));
            continue;
        }
        let Ok((collected, _)) = read(&wire) else {
            broken.push(format!(
                "{}: this crate could not read its own body",
                case.name
            ));
            continue;
        };
        let Some((octets, _)) = payload_of(&collected) else {
            broken.push(format!(
                "{}: the written payload did not come back",
                case.name
            ));
            continue;
        };
        if octets != case.payload {
            broken.push(format!(
                "{}: wrote {:?}, read back {:?}",
                case.name, case.payload, octets
            ));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

// -------------------------------------------------------------------------------------------
// Shapes that stress the reader's own seams rather than one element's character data.
// -------------------------------------------------------------------------------------------

/// A `calendar-multiget` answer carrying three resources, each with its own calendar.
const MANY: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/many_responses.xml");
const MANY_FIRST: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/many_first.ics");
const MANY_SECOND: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/many_second.ics");
const MANY_THIRD: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/many_third.ics");

/// The refused propstat written before the readable one, which is an order servers emit.
const REFUSED_FIRST: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/refused_first.xml");

/// A comment splitting the payload into two runs of character data.
const COMMENT_SPLIT: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/comment_split.xml");

/// Two comments leaving a run between them that is only the `CRLF` of a content line.
const COMMENT_BLANK_RUN: &[u8] =
    include_bytes!("fixtures/break_dav_roundtrip/comment_blank_run.xml");

/// The whole body in default declarations, with no prefix anywhere.
const DEFAULT_NAMESPACE: &[u8] =
    include_bytes!("fixtures/break_dav_roundtrip/default_namespace.xml");

/// D2. Each response's payload is that response's, in order and unmixed.
#[test]
fn three_resources_in_one_body_keep_their_own_calendars() {
    let (collected, _) = read(MANY).expect("a multiget answer reads");
    let wanted = PropName::Known(ElementName::CalendarData);
    let recovered: Vec<Vec<u8>> = collected
        .responses()
        .iter()
        .map(|response| match response.successful_value(&wanted) {
            Some(PropValue::CalendarData(carried)) => carried.as_bytes().to_vec(),
            other => panic!("no payload: {other:?}"),
        })
        .collect();
    assert_eq!(
        recovered,
        vec![
            MANY_FIRST.to_vec(),
            MANY_SECOND.to_vec(),
            MANY_THIRD.to_vec()
        ]
    );
}

/// D2. A `403` propstat naming the property does not hide the `200` propstat that carries it.
#[test]
fn a_refused_propstat_written_first_does_not_hide_the_payload_behind_it() {
    let (collected, _) = read(REFUSED_FIRST).expect("a divergent-status answer reads");
    let (octets, witness) = payload_of(&collected).expect("the readable propstat's payload");
    assert_eq!(octets, MANY_FIRST);
    assert_eq!(witness, LineEndings::Crlf);
}

/// D2. A run split by markup keeps every octet it held, whatever value it lands in.
#[test]
fn a_payload_split_by_a_comment_loses_no_octet() {
    let (collected, _) = read(COMMENT_SPLIT).expect("an annotated body reads");
    let held = match value_of(&collected) {
        Some(PropValue::CalendarData(carried)) => carried.as_bytes().to_vec(),
        Some(PropValue::Unmodeled(kept)) => kept.to_vec(),
        other => panic!("nothing kept the octets: {other:?}"),
    };
    assert_eq!(held, MANY_FIRST);
}

/// D2. The run a comment leaves that is only a line break is part of the calendar.
#[test]
fn a_run_that_is_only_a_line_break_is_not_layout_inside_a_calendar() {
    let (collected, _) = read(COMMENT_BLANK_RUN).expect("an annotated body reads");
    let held = match value_of(&collected) {
        Some(PropValue::CalendarData(carried)) => carried.as_bytes().to_vec(),
        Some(PropValue::Unmodeled(kept)) => kept.to_vec(),
        other => panic!("nothing kept the octets: {other:?}"),
    };
    assert_eq!(held, MANY_FIRST);
}

/// D2. The prefix a server chose is not part of what the payload is.
#[test]
fn a_body_written_entirely_in_default_declarations_yields_the_same_payload() {
    let (collected, _) = read(DEFAULT_NAMESPACE).expect("a default-declaration body reads");
    let (octets, _) = payload_of(&collected).expect("the payload");
    assert_eq!(octets, MANY_FIRST);
}

/// D2, on the wire rather than in this crate: a `CR` a peer's parser would fold is never
/// written literally, because XML 1.0 section 2.11 reaches every one that is.
#[test]
fn nothing_this_crate_writes_carries_a_carriage_return_a_conformant_peer_would_fold() {
    let limits = Limits::DEFAULT;
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        if !is_writable(case) {
            continue;
        }
        let mut meter = Meter::new(limits);
        let held = CalendarPayload::from_octets(case.payload, limits, &mut meter)
            .expect("a payload inside the default bounds");
        let mut body = MultiStatus::new(limits);
        let target = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter)
            .expect("an href inside the default bounds");
        let mut response = DavResponse::with_propstats(target, limits);
        let mut group = PropStat::new(Status::OK, limits);
        group
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::CalendarData(held),
                },
                &mut meter,
            )
            .expect("one property inside the default bounds");
        response
            .push_propstat(group, &mut meter)
            .expect("one propstat inside the default bounds");
        body.push(response, &mut meter)
            .expect("one response inside the default bounds");
        let mut wire: Vec<u8> = Vec::new();
        body.write_xml(&mut wire, limits, &mut meter)
            .expect("the encoder writes the body");
        if wire.contains(&b'\r') {
            broken.push(format!("{}: a literal CR reached the wire", case.name));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// Read one multistatus under a caller-chosen text policy.
fn read_under(
    body: &[u8],
    policy: TextPolicy,
) -> Result<(MultiStatus, Vec<Diagnostic>), icalkit_conformance::internal::dav::DavError> {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let collected = {
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported).with_text(policy);
        let mut events = XmlReader::new(body);
        let mut source = MultiStatusReader::new(&mut events);
        MultiStatus::read(&mut source, &mut context)
    }?;
    Ok((collected, reported))
}

/// `docs/adr/0004` Amendment 1: the conformant read is lossy and never silent.
///
/// "Every payload that loses a `CR` to it reports
/// `DiagnosticCode::DavCalendarDataLineEndingsFolded`, because a choice being available is
/// worth nothing if taking it is silent." That is the contract asserted here: the witness says
/// `Folded` exactly when a `CR` went missing, and the diagnostic is on the sink exactly then.
#[test]
fn the_conformant_read_says_so_on_every_payload_it_costs_a_carriage_return() {
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        let Ok((collected, reported)) = read_under(case.body, TextPolicy::Normalized) else {
            broken.push(format!("{}: the body was refused", case.name));
            continue;
        };
        let Some((octets, witness)) = payload_of(&collected) else {
            broken.push(format!("{}: no calendar-data came back", case.name));
            continue;
        };
        let lost = carriage_returns(octets) < carriage_returns(case.payload);
        let said = witness == LineEndings::Folded;
        if lost != said {
            broken.push(format!(
                "{}: carriage returns lost = {lost}, witness = {witness:?}",
                case.name
            ));
        }
        let told = reported
            .iter()
            .any(|found| found.code() == DiagnosticCode::DavCalendarDataLineEndingsFolded);
        if lost != told {
            broken.push(format!(
                "{}: carriage returns lost = {lost}, diagnostic reported = {told}",
                case.name
            ));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// D2. `octets -> value -> octets -> value` is a fixed point on the payload.
#[test]
fn a_second_turn_through_the_envelope_changes_nothing() {
    let limits = Limits::DEFAULT;
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        if !is_writable(case) {
            continue;
        }
        let Ok((first, _)) = read(case.body) else {
            continue;
        };
        let mut meter = Meter::new(limits);
        let mut wire: Vec<u8> = Vec::new();
        if first.write_xml(&mut wire, limits, &mut meter).is_err() {
            broken.push(format!("{}: re-encoding was refused", case.name));
            continue;
        }
        let Ok((second, _)) = read(&wire) else {
            broken.push(format!("{}: the re-encoded body did not read", case.name));
            continue;
        };
        let before = payload_of(&first).map(|(octets, _)| octets.to_vec());
        let after = payload_of(&second).map(|(octets, _)| octets.to_vec());
        if before != after {
            broken.push(format!("{}: {before:?} became {after:?}", case.name));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// The body a server would put on the wire has to be a document its peer can parse.
///
/// Not a round-trip claim about this crate reading itself — that is asserted above — but about
/// the octets leaving it. An XML document declaring UTF-8 and carrying octets that are not
/// UTF-8 is refused whole by any conformant processor, so a payload this crate accepted on the
/// way in becomes a response the peer cannot read at all.
#[test]
fn what_this_crate_writes_is_a_well_formed_utf8_document() {
    let limits = Limits::DEFAULT;
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        let mut meter = Meter::new(limits);
        let held = CalendarPayload::from_octets(case.payload, limits, &mut meter)
            .expect("a payload inside the default bounds");
        let mut body = MultiStatus::new(limits);
        let target = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter)
            .expect("an href inside the default bounds");
        let mut response = DavResponse::with_propstats(target, limits);
        let mut group = PropStat::new(Status::OK, limits);
        group
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::CalendarData(held),
                },
                &mut meter,
            )
            .expect("one property inside the default bounds");
        response
            .push_propstat(group, &mut meter)
            .expect("one propstat inside the default bounds");
        body.push(response, &mut meter)
            .expect("one response inside the default bounds");
        let mut wire: Vec<u8> = Vec::new();
        let written = body.write_xml(&mut wire, limits, &mut meter);
        if written.is_err() {
            // A refusal is a correct answer here: the payload cannot be spelled in XML.
            continue;
        }
        if core::str::from_utf8(&wire).is_err() {
            broken.push(format!(
                "{}: the encoder wrote a body that is not UTF-8 and said nothing",
                case.name
            ));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// D3. Draining the stream and materializing the collection are the same read.
///
/// `docs/design/ical-dav-api.md`: "`MultiStatus::read` drives that same public path and is the
/// only way an owned multistatus is built, so there is no private fast path for the two to
/// diverge along." A payload that differed between them would be a private fast path.
#[test]
fn the_streaming_reader_and_the_owned_one_recover_the_same_payload() {
    let limits = Limits::DEFAULT;
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        let Ok((owned, _)) = read(case.body) else {
            continue;
        };
        let mut meter = Meter::new(limits);
        let mut sink = IgnoreDiagnostics;
        let mut drained: Vec<Vec<u8>> = Vec::new();
        {
            let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
            let mut events = XmlReader::new(case.body);
            let mut source = MultiStatusReader::new(&mut events);
            let wanted = PropName::Known(ElementName::CalendarData);
            while let Some(response) = source
                .next_response(&mut context)
                .expect("the body reads one response at a time")
            {
                if let Some(PropValue::CalendarData(carried)) = response.successful_value(&wanted) {
                    drained.push(carried.as_bytes().to_vec());
                }
            }
        }
        let held: Vec<Vec<u8>> = owned
            .responses()
            .iter()
            .filter_map(|response| {
                match response.successful_value(&PropName::Known(ElementName::CalendarData)) {
                    Some(PropValue::CalendarData(carried)) => Some(carried.as_bytes().to_vec()),
                    _ => None,
                }
            })
            .collect();
        if drained != held {
            broken.push(format!("{}: streaming and owned disagree", case.name));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// D3. The incremental encoder and the owned one put the same octets on the wire.
#[test]
fn the_streaming_writer_and_the_owned_one_emit_the_same_octets() {
    let limits = Limits::DEFAULT;
    let mut broken: Vec<String> = Vec::new();
    for case in CASES {
        if !is_writable(case) {
            continue;
        }
        let mut meter = Meter::new(limits);
        let held = CalendarPayload::from_octets(case.payload, limits, &mut meter)
            .expect("a payload inside the default bounds");
        let target = Href::new(b"/calendars/ann/work/1.ics", limits, &mut meter)
            .expect("an href inside the default bounds");
        let mut response = DavResponse::with_propstats(target, limits);
        let mut group = PropStat::new(Status::OK, limits);
        group
            .push(
                DavProperty {
                    name: PropName::Known(ElementName::CalendarData),
                    value: PropValue::CalendarData(held),
                },
                &mut meter,
            )
            .expect("one property inside the default bounds");
        response
            .push_propstat(group, &mut meter)
            .expect("one propstat inside the default bounds");

        let mut owned_body = MultiStatus::new(limits);
        owned_body
            .push(response.clone(), &mut meter)
            .expect("one response inside the default bounds");
        let mut owned_wire: Vec<u8> = Vec::new();
        owned_body
            .write_xml(&mut owned_wire, limits, &mut meter)
            .expect("the owned encoder writes the body");

        let mut streamed: Vec<u8> = Vec::new();
        {
            let mut writer = MultiStatusWriter::new(&mut streamed, limits, &mut meter)
                .expect("the incremental encoder opens");
            writer
                .push(&response, &mut meter)
                .expect("one response is written");
            writer
                .finish(None, &mut meter)
                .expect("the document closes");
        }
        if owned_wire != streamed {
            broken.push(format!("{}: the two encoders disagree", case.name));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// A `PROPFIND` answer carrying the collection's `CALDAV:calendar-timezone`.
const TIMEZONE_BODY: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/calendar_timezone.xml");

/// The iCalendar object inside that element, byte for byte.
const TIMEZONE_OBJECT: &[u8] = include_bytes!("fixtures/break_dav_roundtrip/calendar_timezone.ics");

/// D2. `CALDAV:calendar-timezone` is an iCalendar object too, and it is not `calendar-data`.
///
/// RFC 4791 section 5.2.2: the value of `CALDAV:calendar-timezone` "MUST be a valid iCalendar
/// object containing exactly one VTIMEZONE component". Its content lines end with `CRLF` for
/// exactly the reason a `calendar-data` payload's do — RFC 5545 section 3.1 — so a read that
/// folds them is the rewrite `docs/adr/0004` Amendment 1 exists to prevent, one property over.
/// A client that reads a collection's timezone and `PROPPATCH`es it back rewrites the stored
/// object, and neither a witness nor a diagnostic tells it so: `LineEndings` travels on
/// `CalendarPayload` alone, and this value arrives as `PropValue::Text`.
#[test]
fn the_other_icalendar_property_keeps_its_line_endings_too() {
    let (collected, reported) = read(TIMEZONE_BODY).expect("a PROPFIND answer reads");
    let wanted = PropName::Known(ElementName::CalendarTimezone);
    let held = match collected
        .responses()
        .first()
        .and_then(|response| response.successful_value(&wanted))
    {
        Some(PropValue::Text(kept) | PropValue::Unmodeled(kept)) => kept.to_vec(),
        Some(PropValue::CalendarData(carried)) => carried.as_bytes().to_vec(),
        other => panic!("nothing kept the object: {other:?}"),
    };
    // Neither of the two things a caller could use to notice is present.
    let told = reported
        .iter()
        .any(|found| found.code() == DiagnosticCode::DavCalendarDataLineEndingsFolded);
    assert_eq!(
        held,
        TIMEZONE_OBJECT.to_vec(),
        "carriage returns lost = {}, diagnostic reported = {told}",
        carriage_returns(&held) < carriage_returns(TIMEZONE_OBJECT)
    );
}

/// D3. This crate reads its own spelling of `calendar-timezone` and not a real server's.
///
/// The writer escapes a `CR` as `&#13;`, which is markup and survives section 2.11 in every
/// mode, so `write -> read` on this property is the identity — which is all `tests/interop.rs`
/// ever exercises. `SabreDAV` and Radicale write the `CRLF` octets literally, and those are
/// folded. The asymmetry is why a round trip against this crate's own output cannot see the
/// loss above.
#[test]
fn this_crates_own_output_survives_a_property_a_real_servers_output_does_not() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut body = MultiStatus::new(limits);
    let target = Href::new(b"/calendars/ann/work/", limits, &mut meter)
        .expect("an href inside the default bounds");
    let mut response = DavResponse::with_propstats(target, limits);
    let mut group = PropStat::new(Status::OK, limits);
    group
        .push(
            DavProperty {
                name: PropName::Known(ElementName::CalendarTimezone),
                value: PropValue::Text(TIMEZONE_OBJECT.to_vec().into_boxed_slice()),
            },
            &mut meter,
        )
        .expect("one property inside the default bounds");
    response
        .push_propstat(group, &mut meter)
        .expect("one propstat inside the default bounds");
    body.push(response, &mut meter)
        .expect("one response inside the default bounds");
    let mut wire: Vec<u8> = Vec::new();
    body.write_xml(&mut wire, limits, &mut meter)
        .expect("the encoder writes the body");

    let (again, _) = read(&wire).expect("this crate reads its own body");
    let wanted = PropName::Known(ElementName::CalendarTimezone);
    let held = match again
        .responses()
        .first()
        .and_then(|found| found.successful_value(&wanted))
    {
        Some(PropValue::Text(kept)) => kept.to_vec(),
        other => panic!("nothing kept the object: {other:?}"),
    };
    // Its own output survives, which is the half `tests/interop.rs` asserts.
    assert_eq!(held, TIMEZONE_OBJECT.to_vec());

    // And a real server's spelling of the same value does not, which is the half nothing
    // asserted. Both readings are of one property under one policy.
    let (from_server, _) = read(TIMEZONE_BODY).expect("a PROPFIND answer reads");
    let literal = match from_server
        .responses()
        .first()
        .and_then(|found| found.successful_value(&wanted))
    {
        Some(PropValue::Text(kept)) => kept.to_vec(),
        other => panic!("nothing kept the object: {other:?}"),
    };
    assert_eq!(
        literal, held,
        "one property, one policy, two answers depending on who wrote the CR"
    );
}

/// A propstat whose status is not `2xx` must not be read as a payload the caller received.
#[test]
fn a_response_body_that_is_a_bare_status_carries_no_payload() {
    let (collected, _) = read(CASES[0].body).expect("the first case reads");
    let first = collected.responses().first().expect("one response");
    assert!(matches!(first.body, ResponseBody::PropStats(_)));
}

/// A sanity check on the harness: `IgnoreDiagnostics` and a collecting sink read alike.
#[test]
fn the_diagnostic_sink_does_not_change_what_is_read() {
    let limits = Limits::DEFAULT;
    let case = &CASES[0];
    let mut meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let quiet = {
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
        let mut events = XmlReader::new(case.body);
        let mut source = MultiStatusReader::new(&mut events);
        MultiStatus::read(&mut source, &mut context)
    }
    .expect("the first case reads");
    let (loud, _) = read(case.body).expect("the first case reads");
    assert_eq!(
        payload_of(&quiet).map(|(octets, _)| octets.to_vec()),
        payload_of(&loud).map(|(octets, _)| octets.to_vec())
    );
}
