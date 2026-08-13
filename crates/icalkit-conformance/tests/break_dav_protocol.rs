// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! An attack on `ical-dav`'s protocol state: entity tags, sync tokens, conditional writes, and
//! the bounds a hostile multistatus is supposed to meet.
//!
//! The lens is the state a CalDAV exchange carries between its two turns. A client reads a
//! resource, learns an `ETag` and writes back conditionally; a client reads a sync token and
//! hands it to the next `REPORT`; a reader meets a body claiming more responses, longer `href`s
//! and more properties than its policy admits. Every one of those is a place where a wrong
//! answer is silent — an overwritten edit, a resynchronization that never happens again, a
//! request carrying octets the caller never chose.
//!
//! # What each case is addressed to
//!
//! - **RFC 9110 section 8.8.3** (`entity-tag`, `etagc`) — which octets an entity tag may hold,
//!   given that this crate renders one into a header value for a caller to frame.
//! - **RFC 9110 sections 13.1.1 and 13.1.2** (`If-Match`, `If-None-Match`) — the strong and
//!   weak comparisons, the wildcard, and the list form a server has to read.
//! - **RFC 9110 section 15** (`status-line`) — the three digits a `DAV:status` carries.
//! - **RFC 4918 sections 13, 14.24 and 14.28** — the multistatus, the response, the status.
//! - **RFC 6578 sections 3.2 and 3.4** — a removal, a token that is no longer valid, and what
//!   a token means about the answer it arrived with.
//! - **`docs/adr/0010`** — the bounds a body's own length does not reach: responses, `href`
//!   octets, properties per response, one element's character data.
//!
//! Nothing here is fixed. A failing case in this file is a finding.

use std::time::{Duration, Instant as Clock};

use icalkit_conformance::internal::core::{
    Diagnostic, DiagnosticCode, IgnoreDiagnostics, Limits, Meter, Severity,
};
use icalkit_conformance::internal::dav::{
    DavError, DavResponse, DecodeContext, ETag, ElementName, Href, MatchHeader, MultiStatus,
    MultiStatusReader, Precondition, PropName, PropStat, PropValue, ResponseBody, ResponseSource,
    Revision, Status, SyncToken, ValueError, WriteXml, XmlReader,
};

const ETAG_INJECTION: &[u8] =
    include_bytes!("fixtures/break_dav_protocol/etag-carriage-return-injection.xml");
const SYNC_REMOVAL: &[u8] =
    include_bytes!("fixtures/break_dav_protocol/sync-collection-removal.xml");
const INVALID_TOKEN: &[u8] =
    include_bytes!("fixtures/break_dav_protocol/invalid-sync-token-error.xml");
const SPLIT_TEXT: &[u8] = include_bytes!("fixtures/break_dav_protocol/split-text-nodes.xml");
const WEAK_ETAG: &[u8] = include_bytes!("fixtures/break_dav_protocol/weak-etag.xml");

/// The resource every case that needs one is about.
const RESOURCE: &[u8] = b"/calendars/ann/work/1.ics";

/// The wall-clock ceiling a bounded answer has to come in under.
///
/// A case that does not finish inside it is a hang, which is a worse outcome than a refusal
/// and is what an unbounded allocation or a non-advancing loop looks like from outside.
const PATIENCE: Duration = Duration::from_secs(20);

// -------------------------------------------------------------------------------------------
// Reading a body, the way a caller does.
// -------------------------------------------------------------------------------------------

/// Read a whole multistatus out of octets under the caller's bounds.
fn read_multistatus(
    body: &[u8],
    limits: Limits,
) -> (Result<MultiStatus, DavError>, Vec<Diagnostic>) {
    let mut meter = Meter::new(limits);
    let mut reported: Vec<Diagnostic> = Vec::new();
    let read = {
        let mut context = DecodeContext::new(limits, &mut meter, &mut reported);
        let mut events = XmlReader::new(body);
        let mut source = MultiStatusReader::new(&mut events);
        MultiStatus::read(&mut source, &mut context)
    };
    (read, reported)
}

/// Drain a multistatus one response at a time, holding no collection.
fn drain(body: &[u8], limits: Limits) -> Result<usize, DavError> {
    let mut meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
    let mut events = XmlReader::new(body);
    let mut source = MultiStatusReader::new(&mut events);
    let mut seen: usize = 0;
    loop {
        match source.next_response(&mut context) {
            Ok(Some(_)) => seen = seen.saturating_add(1),
            Ok(None) => return Ok(seen),
            Err(failure) => return Err(failure),
        }
    }
}

/// The first response of a body, or nothing when the body did not read.
fn first_response(body: &[u8]) -> Option<DavResponse> {
    let (read, _) = read_multistatus(body, Limits::DEFAULT);
    read.ok()
        .and_then(|collected| collected.responses().first().cloned())
}

/// The revision one response states, or nothing when it states none.
fn revision_of(response: &DavResponse) -> Option<Revision> {
    let mut meter = Meter::new(Limits::DEFAULT);
    Revision::from_response(response, Limits::DEFAULT, &mut meter).ok()
}

/// The header value a caller would put on the wire for a revision, if it can build one.
fn conditional_value(revision: &Revision) -> Option<Vec<u8>> {
    let precondition = revision.precondition()?;
    let mut value: Vec<u8> = Vec::new();
    precondition.write_value(&mut value).ok()?;
    Some(value)
}

/// An `href` over octets a case has already bounded.
fn href(path: &[u8], meter: &mut Meter) -> Option<Href> {
    Href::new(path, Limits::DEFAULT, meter).ok()
}

// -------------------------------------------------------------------------------------------
// RFC 9110 section 8.8.3: what an entity tag may hold.
// -------------------------------------------------------------------------------------------

/// Whether an octet may appear inside an entity tag, RFC 9110 section 8.8.3.
///
/// `etagc = "!" / %x23-7E / obs-text`. Everything else — `SP`, every control character, `DEL`,
/// and the double quote that ends the tag — is outside the production. The exclusion is not
/// decoration: a tag travels in a header value, and the octets a header value may not hold are
/// exactly the ones that end a header field.
const fn is_etagc(byte: u8) -> bool {
    byte == 0x21 || (byte >= 0x23 && byte <= 0x7e) || byte >= 0x80
}

/// A tag as a server wrote it, and what it is.
const TAG_SHAPES: [(&[u8], &str); 9] = [
    (b"\"abc\"", "an ordinary strong tag"),
    (b"W/\"abc\"", "a weak tag"),
    (b"\"\"", "the empty quoted string, which is legal"),
    (b"abc", "an unquoted tag, which is a server bug"),
    (b"*", "the wildcard, which is not a tag at all"),
    (b"\"a\"b\"", "a tag with a quote in it"),
    (b"\"a\r\nX: y\"", "a tag carrying a header separator"),
    (b"\"a\nX: y\"", "a tag carrying a bare line feed"),
    (b"\"a b\"", "a tag carrying a space"),
];

#[test]
fn an_etag_never_carries_an_octet_a_header_value_cannot() {
    // `ETag::parse` refuses the quote and nothing else, so every control character, `DEL` and
    // `SP` is accepted inside a tag and written straight back out by `ETag::write_value` —
    // which this crate documents as the way a caller renders `If-Match`.
    let mut accepted: Vec<u8> = Vec::new();
    for byte in 0u8..=255 {
        if is_etagc(byte) {
            continue;
        }
        if ETag::parse(&[b'"', b'a', byte, b'"']).is_ok() {
            accepted.push(byte);
        }
    }
    let shapes: Vec<&str> = TAG_SHAPES
        .iter()
        .filter(|(sent, _)| {
            let inside = sent.strip_prefix(b"W/".as_slice()).unwrap_or(sent);
            let inside = inside.strip_prefix(b"\"".as_slice()).unwrap_or(inside);
            let inside = inside.strip_suffix(b"\"".as_slice()).unwrap_or(inside);
            inside.iter().any(|byte| !is_etagc(*byte)) && ETag::parse(sent).is_ok()
        })
        .map(|(_, what)| *what)
        .collect();
    assert!(
        accepted.is_empty() && shapes.is_empty(),
        "octets outside RFC 9110's `etagc` were accepted inside an entity tag: \
         {accepted:?}, and so were {shapes:?}"
    );
}

#[test]
fn a_tag_a_server_chose_cannot_choose_the_caller_s_request_line() {
    // The whole path, through the tokenizer that ships: a server answers a `PROPFIND` with a
    // `getetag` whose character references resolve to `CRLF`, the reader accepts it,
    // `Revision::precondition` offers it, and `Precondition::write_value` writes those octets
    // into the buffer the caller frames as an `If-Match` header. What lands on the wire is the
    // two further headers the server wrote — here an `If-Match: *` that turns the caller's
    // conditional write into an unconditional one.
    let value = first_response(ETAG_INJECTION)
        .as_ref()
        .and_then(revision_of)
        .as_ref()
        .and_then(conditional_value)
        .unwrap_or_default();
    let split = value.windows(2).any(|pair| pair == b"\r\n");
    assert!(
        !split,
        "a rendered If-Match value carries CRLF: {:?}",
        String::from_utf8_lossy(&value)
    );
}

#[test]
fn the_wildcard_is_not_an_entity_tag_and_a_weak_one_conditions_nothing() {
    assert_eq!(
        ETag::parse(b"*"),
        Err(DavError::Invalid(ValueError::EtagSyntax))
    );
    let revision = first_response(WEAK_ETAG).as_ref().and_then(revision_of);
    let weak = revision
        .as_ref()
        .and_then(Revision::etag)
        .map(ETag::is_weak);
    assert_eq!(weak, Some(true));
    // RFC 9110 section 13.1.1: `If-Match` is a strong comparison, which a weak tag can never
    // satisfy. Downgrading to `If-Match: *` here is the silent overwrite this module hunts.
    assert_eq!(
        revision.as_ref().and_then(Revision::precondition),
        None,
        "a weak tag became a precondition"
    );
}

/// The `If-Match` header values RFC 9110 section 13.1.1 defines, as they arrive at a server.
const IF_MATCH_VALUES: [(&[u8], &str); 4] = [
    (b"\"v1\"", "one strong tag"),
    (b"*", "the wildcard: any stored copy will do"),
    (b"\"v1\", \"v2\"", "a list, which section 13.1.1 admits"),
    (b"nonsense", "a header value that is not one at all"),
];

#[test]
fn a_server_can_tell_the_wildcard_from_a_header_it_could_not_read() {
    // The direction claim, at the header that decides whether somebody's edit survives. This
    // crate renders `If-Match` through `Precondition::write_value` and judges it through
    // `Precondition::is_satisfied_by`. What it lacked was any door that *reads* one back:
    // `ETag::parse` is a tag parser and correctly refuses `*`, which is not a tag, so asking
    // it about a header value gave a server the same answer for "replace whatever is there"
    // and for "this header made no sense" — and the two demand opposite outcomes on a write.
    //
    // The finding is the missing door, not `ETag::parse`'s answer, so this asks the door that
    // now exists. Note what stays true beside it: `ETag::parse(b"*")` is still an error, which
    // `the_wildcard_is_not_an_entity_tag_and_a_weak_one_conditions_nothing` asserts.
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let outcomes: Vec<(&str, bool)> = IF_MATCH_VALUES
        .iter()
        .map(|(value, what)| (*what, MatchHeader::parse(value, limits, &mut meter).is_ok()))
        .collect();
    let wildcard = outcomes.get(1).map(|held| held.1);
    let listed = outcomes.get(2).map(|held| held.1);
    let nonsense = outcomes.get(3).map(|held| held.1);
    assert_ne!(
        wildcard, nonsense,
        "`If-Match: *` and an unreadable `If-Match` are the same answer: {outcomes:?}"
    );
    assert_eq!(
        listed,
        Some(true),
        "RFC 9110 section 13.1.1's list form is a header a server has to read: {outcomes:?}"
    );

    // And the wildcard a server read decides a write the way section 13.1.1 says it does.
    let held = ETag::parse(b"\"v1\"").expect("a tag");
    let any = MatchHeader::parse(b"*", limits, &mut meter).expect("the wildcard reads");
    assert!(any.if_match_is_satisfied_by(Some(&held)));
    assert!(!any.if_match_is_satisfied_by(None));
    let one = MatchHeader::parse(b"\"v1\", \"v2\"", limits, &mut meter).expect("a list reads");
    assert!(one.if_match_is_satisfied_by(Some(&held)));
    assert!(!one.if_match_is_satisfied_by(Some(&ETag::parse(b"\"v3\"").expect("another tag"))));
    // A weak tag never satisfies the strong comparison `If-Match` uses.
    let weak = MatchHeader::parse(b"W/\"v1\"", limits, &mut meter).expect("a weak tag reads");
    assert!(!weak.if_match_is_satisfied_by(Some(&held)));
}

/// A precondition a caller holds, the tag a server stores, and whether the write may land.
type Conditional = (&'static [u8], Option<&'static [u8]>, bool);

/// RFC 9110 section 13.1.1's strong comparison, as writes that must and must not land.
const CONDITIONAL_WRITES: [Conditional; 8] = [
    (b"\"v1\"", Some(b"\"v1\""), true),
    (b"\"v1\"", Some(b"\"v2\""), false),
    (b"\"v1\"", None, false),
    // A weak tag on either side fails the strong comparison `If-Match` uses.
    (b"W/\"v1\"", Some(b"\"v1\""), false),
    (b"\"v1\"", Some(b"W/\"v1\""), false),
    (b"W/\"v1\"", Some(b"W/\"v1\""), false),
    // The empty quoted string is a legal tag and compares like any other.
    (b"\"\"", Some(b"\"\""), true),
    (b"\"\"", Some(b"\"x\""), false),
];

#[test]
fn a_conditional_write_lands_only_where_the_comparison_rules_say_it_may() {
    let mut wrong: Vec<String> = Vec::new();
    for (wanted, stored, should_land) in CONDITIONAL_WRITES {
        let (Ok(asked), held) = (ETag::parse(wanted), stored.map(ETag::parse)) else {
            wrong.push(format!("{wanted:?} did not read as a tag"));
            continue;
        };
        let held = match held {
            Some(Ok(known)) => Some(known),
            Some(Err(_)) => {
                wrong.push(format!("{stored:?} did not read as a tag"));
                continue;
            },
            None => None,
        };
        let landed = Precondition::Replace(&asked).is_satisfied_by(held.as_ref());
        if landed != should_land {
            wrong.push(format!(
                "If-Match {wanted:?} against {stored:?} landed: {landed}"
            ));
        }
    }
    // `If-Match: *` requires a stored copy; `If-None-Match: *` requires the absence of one.
    if let Ok(held) = ETag::parse(b"\"v1\"") {
        let states = [
            (Precondition::ReplaceAny.is_satisfied_by(Some(&held)), true),
            (Precondition::ReplaceAny.is_satisfied_by(None), false),
            (Precondition::CreateOnly.is_satisfied_by(Some(&held)), false),
            (Precondition::CreateOnly.is_satisfied_by(None), true),
        ];
        for (at, (answered, expected)) in states.into_iter().enumerate() {
            if answered != expected {
                wrong.push(format!("wildcard case {at} answered {answered}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_tag_that_round_trips_reads_back_as_the_tag_that_was_written() {
    let mut wrong: Vec<String> = Vec::new();
    for sent in [b"\"abc\"".as_slice(), b"W/\"abc\"", b"\"\""] {
        let Ok(tag) = ETag::parse(sent) else {
            wrong.push(format!("{sent:?} did not read as a tag"));
            continue;
        };
        let mut rendered: Vec<u8> = Vec::new();
        if tag.write_value(&mut rendered).is_err() || rendered != sent {
            wrong.push(format!("{sent:?} rendered as {rendered:?}"));
            continue;
        }
        if ETag::parse(&rendered) != Ok(tag) {
            wrong.push(format!("{sent:?} did not read back as itself"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_tag_a_server_states_is_the_tag_a_client_reads_back() {
    // The direction claim over the XML layer alone, with the hostile tag as its subject: a
    // server holding a revision writes it into a multistatus and a client reads the same value
    // out. This pins the finding above to the header door rather than to the encoder — a `CR`
    // written as `&#13;` survives XML 1.0 section 2.11 and arrives whole.
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let Ok(hostile) = ETag::parse(b"\"a\r\nX: y\"") else {
        return;
    };
    let (Some(one), Some(two)) = (href(RESOURCE, &mut meter), href(RESOURCE, &mut meter)) else {
        return;
    };
    let held = Revision::at(one, hostile);
    let mut group = PropStat::new(Status::OK, limits);
    let mut response = DavResponse::with_propstats(two, limits);
    let mut body = MultiStatus::new(limits);
    let mut written: Vec<u8> = Vec::new();
    let staged = held
        .push_properties(&mut group, &mut meter)
        .and_then(|()| response.push_propstat(group, &mut meter))
        .and_then(|()| body.push(response, &mut meter))
        .and_then(|()| body.write_xml(&mut written, limits, &mut meter));
    assert!(staged.is_ok(), "{staged:?}");

    let read = first_response(&written).as_ref().and_then(revision_of);
    assert_eq!(
        read.as_ref().and_then(Revision::etag),
        held.etag(),
        "a tag stopped being itself across this crate's own encode and decode"
    );
}

// -------------------------------------------------------------------------------------------
// RFC 6578: a token is opaque, and it states something about the answer it arrived with.
// -------------------------------------------------------------------------------------------

/// Tokens a server may mint, including the shapes that tempt a reader into interpreting one.
const TOKENS: [&[u8]; 7] = [
    b"http://sabre.io/ns/sync/5",
    b"data:,4f2c1a90_218",
    b"42",
    b"0",
    b"-1",
    b"18446744073709551616",
    b"",
];

#[test]
fn a_sync_token_is_octets_and_never_a_number() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let mut wrong: Vec<String> = Vec::new();
    for octets in TOKENS {
        match SyncToken::new(octets, limits, &mut meter) {
            Ok(token) if token.as_bytes() == octets => {},
            other => wrong.push(format!("{octets:?} became {other:?}")),
        }
    }
    // Two tokens a numeric reading would order are only equal or unequal here.
    let low = SyncToken::new(b"9", limits, &mut meter);
    let high = SyncToken::new(b"10", limits, &mut meter);
    assert_ne!(low, high);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_enormous_sync_token_is_a_refusal_and_not_an_allocation() {
    let limits = Limits::DEFAULT;
    let mut meter = Meter::new(limits);
    let huge = vec![b'x'; 1_048_576];
    let refused = SyncToken::new(&huge, limits, &mut meter).is_err();
    assert!(
        refused,
        "a one-megabyte sync token was accepted under a 4 KiB bound"
    );
}

#[test]
fn a_sync_report_reporting_a_removal_reports_it_with_its_token() {
    let (read, _) = read_multistatus(SYNC_REMOVAL, Limits::DEFAULT);
    let Ok(collected) = read else {
        panic_free(&format!("a recorded sync report did not read: {read:?}"));
        return;
    };
    assert_eq!(collected.responses().len(), 2);
    // RFC 6578 section 3.2: a member no longer in the collection is reported with a `404`.
    let removed = collected.responses().first().map(|one| match &one.body {
        ResponseBody::Status(status) => status.code(),
        ResponseBody::PropStats(_) => 0,
    });
    assert_eq!(removed, Some(404));
    assert_eq!(
        collected.sync_token.as_ref().map(SyncToken::as_bytes),
        Some(b"http://sabre.io/ns/sync/17".as_slice()),
        "the token a sync report ends with did not survive the read"
    );
}

#[test]
fn a_token_the_server_will_no_longer_honor_is_a_reported_outcome() {
    // RFC 6578 section 3.2: a token a server can no longer use answers `403` with
    // `DAV:valid-sync-token` inside a `DAV:error` document, whose root is not a multistatus.
    // Whatever this crate does with those octets, it must be an outcome and not a panic.
    let (read, _) = read_multistatus(INVALID_TOKEN, Limits::DEFAULT);
    let refused = matches!(
        read,
        Err(DavError::Unexpected(_) | DavError::Foreign | DavError::Syntax(_))
    );
    assert!(refused, "{read:?}");
}

#[test]
fn nothing_between_two_text_runs_may_swallow_the_rest_of_a_value() {
    // A comment carries no event of its own, so a value split across two runs by one arrives
    // as two runs. Keeping the first and dropping the rest without a word makes an `href` and
    // a sync token into values the peer never sent — and the token is the one a caller hands
    // straight back to the server.
    let (read, reported) = read_multistatus(SPLIT_TEXT, Limits::DEFAULT);
    let Ok(collected) = read else {
        return;
    };
    let named = collected
        .responses()
        .first()
        .map(|one| one.href.as_bytes().to_vec())
        .unwrap_or_default();
    let token = collected.sync_token.as_ref().map(SyncToken::as_bytes);
    let complained = reported
        .iter()
        .any(|found| found.severity() >= Severity::Violation);
    let whole = named == b"/calendars/ann/work/1.icsx/2.ics"
        && token == Some(b"http://sabre.io/ns/sync/17".as_slice());
    assert!(
        whole || complained,
        "href {:?} and token {:?} were truncated with nothing reported",
        String::from_utf8_lossy(&named),
        token.map(String::from_utf8_lossy)
    );
}

// -------------------------------------------------------------------------------------------
// Bounds: what a body claims, against what a policy admits.
// -------------------------------------------------------------------------------------------

/// A multistatus claiming `count` responses.
fn many_responses(count: usize) -> Vec<u8> {
    sync_report(count, None)
}

/// A sync report claiming `count` responses, with its token wherever the server put it.
///
/// `token_first` is not the order RFC 6578 section 3.4's examples show, and it is here because
/// this reader accepts it: what a reader accepts it must also survive.
fn sync_report(count: usize, token_first: Option<bool>) -> Vec<u8> {
    const TOKEN: &[u8] = b"<D:sync-token>http://sabre.io/ns/sync/99</D:sync-token>";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(b"<D:multistatus xmlns:D=\"DAV:\">");
    if token_first == Some(true) {
        body.extend_from_slice(TOKEN);
    }
    for index in 0..count {
        body.extend_from_slice(b"<D:response><D:href>/c/");
        body.extend_from_slice(index.to_string().as_bytes());
        body.extend_from_slice(b".ics</D:href><D:status>HTTP/1.1 200 OK</D:status></D:response>");
    }
    if token_first == Some(false) {
        body.extend_from_slice(TOKEN);
    }
    body.extend_from_slice(b"</D:multistatus>");
    body
}

#[test]
fn forty_thousand_responses_meet_an_embedded_policy_as_a_report() {
    let body = many_responses(40_000);
    // The policy of a client with tens of kilobytes: sixteen responses and no more.
    let limits = Limits::DEFAULT.with_max_responses(16);
    let started = Clock::now();
    let (read, reported) = read_multistatus(&body, limits);
    let held = read.map(|collected| collected.responses().len());
    let elapsed = started.elapsed();
    assert!(
        elapsed < PATIENCE,
        "reading 40,000 responses took {elapsed:?}"
    );
    assert!(
        matches!(held, Ok(count) if count <= 16),
        "a forged flood answered {held:?}"
    );
    assert!(
        reported
            .iter()
            .any(|found| found.code() == DiagnosticCode::DavResponsesTruncated),
        "a truncated collection was not reported as truncated"
    );

    // The streaming reader holds no collection, so under the same policy it must deliver what
    // its ledger admits and then stop, without a hang.
    let started = Clock::now();
    let counted = drain(&body, limits);
    assert!(counted.is_ok(), "{counted:?}");
    assert!(started.elapsed() < PATIENCE);
}

#[test]
fn a_real_forty_thousand_resource_collection_enumerates_under_a_generous_policy() {
    // The other half of the cap contradiction `docs/design/ical-dav-api.md` names: a server's
    // own reader must walk a collection this size, one response at a time, and finish.
    let body = many_responses(40_000);
    let started = Clock::now();
    let counted = drain(&body, Limits::GENEROUS);
    let elapsed = started.elapsed();
    assert_eq!(counted.ok(), Some(40_000));
    assert!(
        elapsed < PATIENCE,
        "draining 40,000 responses took {elapsed:?}"
    );
}

#[test]
fn a_truncated_sync_report_never_hands_back_a_token_for_what_was_not_delivered() {
    // The worst outcome in this lens that is not an overwrite: a client keeping a sync token
    // that covers changes it never received will never be told about them again. RFC 6578
    // section 3.4 makes the token a statement about the whole answer, so a partial answer
    // states none — whichever end of the body the server wrote the token at.
    let limits = Limits::DEFAULT.with_max_responses(16);
    let mut wrong: Vec<String> = Vec::new();
    for token_first in [false, true] {
        let body = sync_report(40_000, Some(token_first));
        let (read, reported) = read_multistatus(&body, limits);
        let Ok(collected) = read else {
            wrong.push(format!("token_first {token_first}: {read:?}"));
            continue;
        };
        let truncated = collected.responses().len() < 40_000
            && reported
                .iter()
                .any(|found| found.code() == DiagnosticCode::DavResponsesTruncated);
        let handed = collected
            .sync_token
            .as_ref()
            .map(SyncToken::as_bytes)
            .is_some();
        if truncated && handed {
            wrong.push(format!(
                "token_first {token_first}: a report cut short at {} of 40,000 responses \
                 handed back a sync token",
                collected.responses().len()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_href_a_megabyte_long_is_refused_at_the_bound() {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(b"<D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>/");
    body.extend_from_slice(&vec![b'a'; 1_048_576]);
    body.extend_from_slice(
        b"</D:href><D:status>HTTP/1.1 200 OK</D:status></D:response></D:multistatus>",
    );
    let started = Clock::now();
    let (read, _) = read_multistatus(&body, Limits::DEFAULT);
    assert!(started.elapsed() < PATIENCE);
    let held = read.map(|collected| {
        collected
            .responses()
            .first()
            .map_or(0, |one| one.href.as_bytes().len())
    });
    assert!(
        matches!(held, Err(DavError::Limit(_))),
        "a megabyte href under a 4 KiB bound answered {held:?}"
    );
}

#[test]
fn a_hundred_thousand_properties_in_one_response_meet_a_bound() {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        b"<D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>/c/1.ics</D:href>\
          <D:propstat><D:prop>",
    );
    for _ in 0..100_000u32 {
        body.extend_from_slice(b"<D:displayname>x</D:displayname>");
    }
    body.extend_from_slice(
        b"</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>",
    );
    let started = Clock::now();
    let (read, _) = read_multistatus(&body, Limits::DEFAULT);
    let elapsed = started.elapsed();
    assert!(
        elapsed < PATIENCE,
        "reading 100,000 properties took {elapsed:?}"
    );
    let held = read.map(|collected| {
        collected
            .responses()
            .first()
            .map_or(0, |one| match &one.body {
                ResponseBody::PropStats(groups) => groups.as_slice().len(),
                ResponseBody::Status(_) => 0,
            })
    });
    assert!(
        matches!(held, Err(DavError::Limit(_)) | Ok(0..=256)),
        "a hundred thousand properties under a 256-property bound answered {held:?}"
    );
}

#[test]
fn a_calendar_data_payload_past_the_configured_limit_is_refused() {
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(4096);
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        b"<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
          <D:response><D:href>/c/1.ics</D:href><D:propstat><D:prop><C:calendar-data>",
    );
    body.extend_from_slice(&vec![b'V'; 1_048_576]);
    body.extend_from_slice(
        b"</C:calendar-data></D:prop><D:status>HTTP/1.1 200 OK</D:status>\
          </D:propstat></D:response></D:multistatus>",
    );
    let started = Clock::now();
    let (read, _) = read_multistatus(&body, limits);
    assert!(started.elapsed() < PATIENCE);
    let wanted = PropName::Known(ElementName::CalendarData);
    let carried = read.map(|collected| {
        collected
            .responses()
            .first()
            .and_then(|one| one.successful_value(&wanted))
            .map_or(0, |value| match value {
                PropValue::CalendarData(payload) => payload.as_bytes().len(),
                _ => 0,
            })
    });
    assert!(
        matches!(carried, Err(DavError::Limit(_)) | Ok(0..=4096)),
        "a megabyte payload under a 4 KiB text bound answered {carried:?}"
    );
}

// -------------------------------------------------------------------------------------------
// RFC 9110 section 15: the status line a `DAV:status` carries.
// -------------------------------------------------------------------------------------------

/// A status line a server might send, and the code a reader may take from it.
///
/// `status-line = HTTP-version SP status-code SP [ reason-phrase ]` with `status-code` exactly
/// three digits, so a fourth digit is not a status line and neither is a truncated one.
const STATUS_LINES: [(&[u8], Option<u16>); 8] = [
    (b"HTTP/1.1 200 OK", Some(200)),
    (b"HTTP/1.1 404 Not Found", Some(404)),
    (b"HTTP/1.1 200 ", Some(200)),
    (b"HTTP/1.1 2000 OK", None),
    (b"HTTP/1.1 4045", None),
    (b"HTTP/1.1 20", None),
    (b"200 OK", None),
    (b"HTTP/1.1 abc", None),
];

#[test]
fn a_status_line_is_read_as_the_code_it_states_or_as_none() {
    let mut wrong: Vec<String> = Vec::new();
    for (line, expected) in STATUS_LINES {
        let read = Status::parse_status_line(line).ok().map(Status::code);
        if read != expected {
            wrong.push(format!(
                "{:?} read as {read:?}, not {expected:?}",
                String::from_utf8_lossy(line)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// -------------------------------------------------------------------------------------------
// The values a revision is made of, once the octets are somebody else's choice.
// -------------------------------------------------------------------------------------------

#[test]
fn a_revision_binds_a_tag_to_the_resource_it_came_from() {
    let mut meter = Meter::new(Limits::DEFAULT);
    let here = href(RESOURCE, &mut meter);
    let there = href(b"/calendars/bob/work/1.ics", &mut meter);
    let (Some(here), Some(there)) = (here, there) else {
        return;
    };
    let (Ok(one), Ok(two)) = (ETag::parse(b"\"v1\""), ETag::parse(b"\"v1\"")) else {
        return;
    };
    let mine = Revision::at(here, one);
    let theirs = Revision::at(there, two);
    assert!(!mine.is_same_revision_as(&theirs));
    assert_ne!(mine.digest(), theirs.digest());
}

#[test]
fn a_response_a_reader_could_not_read_never_becomes_a_precondition() {
    // A propstat whose status nothing can read must not hand back a validator: a client that
    // wrote against one would be conditioning on a tag no server ever said was current.
    let body = b"<D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>/c/1.ics</D:href>\
                 <D:propstat><D:prop><D:getetag>\"v1\"</D:getetag></D:prop>\
                 <D:status>NOT A STATUS LINE</D:status></D:propstat></D:response>\
                 </D:multistatus>";
    let revision = first_response(body).as_ref().and_then(revision_of);
    assert_eq!(revision.as_ref().and_then(Revision::etag), None);
    assert_eq!(revision.as_ref().and_then(Revision::precondition), None);
}

#[test]
fn the_ledger_is_one_across_bodies_and_not_one_per_body() {
    // `docs/adr/0010`'s aggregate claim at the seam this lens owns: four multistatus bodies
    // read under one meter must not each be handed a fresh response budget.
    let body = many_responses(64);
    let limits = Limits::DEFAULT.with_max_responses(100);
    let mut meter = Meter::new(limits);
    let mut sink = IgnoreDiagnostics;
    let mut delivered: usize = 0;
    for _ in 0..4u8 {
        let mut context = DecodeContext::new(limits, &mut meter, &mut sink);
        let mut events = XmlReader::new(&body);
        let mut source = MultiStatusReader::new(&mut events);
        if let Ok(collected) = MultiStatus::read(&mut source, &mut context) {
            delivered = delivered.saturating_add(collected.responses().len());
        }
    }
    assert!(
        delivered <= 100,
        "four bodies of 64 responses delivered {delivered} under a 100-response budget"
    );
}

/// State a case could not run at all, without reaching for a bare panic.
///
/// A case whose fixture did not even read has found nothing and must say so rather than
/// report a break it never observed.
fn panic_free(what: &str) {
    assert!(what.is_empty(), "{what}");
}
