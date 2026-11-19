// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Relationship references through public editing, scheduling, and query workflows.

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

const REFERENCES: &str = concat!(
    "RELATED-TO;VALUE=URI;RELTYPE=Next;GAP=-p1d:urn:other\r\n",
    "RELATED-TO;VALUE=UID;RELTYPE=PARENT:parent\\,part@example.test\r\n",
);

fn calendar(properties: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit relations//EN\r\n\
         BEGIN:VEVENT\r\nUID:relation@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\nSUMMARY:Original\r\n\
         ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:person@example.test\r\n\
         {properties}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn text_edits_keep_uri_and_uid_relationship_spellings() {
    let source = calendar(REFERENCES);
    let mut calendar = Calendar::parse(source.as_bytes()).unwrap();
    let mut edit = calendar.edit();
    edit.set_summary("relation@example.test", "Revised")
        .unwrap();
    edit.commit().unwrap();
    assert_eq!(
        calendar.to_bytes(),
        source.replace(":Original\r\n", ":Revised\r\n").as_bytes()
    );
}

#[test]
fn scheduling_keeps_unresolved_relationships_without_fetching_targets() {
    let source = calendar(REFERENCES);
    let request = source.replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
    assert_eq!(
        Message::read(request.as_bytes()).unwrap().to_bytes(),
        request.as_bytes()
    );
    let message =
        Message::request(source.as_bytes(), Timestamp::constant(1_704_067_200, 0)).unwrap();
    let bytes = message.to_bytes();
    assert!(
        bytes
            .windows(REFERENCES.len())
            .any(|part| part == REFERENCES.as_bytes())
    );
    assert!(Message::read(&bytes).is_ok());
}

#[test]
fn inbound_and_outbound_scheduling_refuse_malformed_relationships() {
    for property in [
        "RELATED-TO;RELTYPE=\"PARENT\":other\r\n",
        "RELATED-TO;RELTYPE=NEXT;GAP=P1M:other\r\n",
        "RELATED-TO;VALUE=UID:a,b\r\n",
    ] {
        let source = calendar(property);
        let request = source.replace("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n");
        assert!(Message::read(request.as_bytes()).is_err(), "{property:?}");
        assert!(
            Message::request(source.as_bytes(), Timestamp::constant(1_704_067_200, 0)).is_err(),
            "{property:?}"
        );
    }
}

#[test]
fn queries_match_and_project_relationship_references_without_rewriting() {
    let other =
        Calendar::parse(calendar(&REFERENCES.replace("urn:other", "urn:different")).as_bytes())
            .unwrap();
    let calendar = Calendar::parse(calendar(REFERENCES).as_bytes()).unwrap();
    let query = Query::parse(
        br#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data><C:comp name="VCALENDAR">
<C:comp name="VEVENT"><C:prop name="RELATED-TO"/></C:comp>
</C:comp></C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:prop-filter name="RELATED-TO"><C:text-match>urn:other</C:text-match></C:prop-filter>
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#,
    )
    .unwrap();
    let engine = Engine::default();
    let mut session = engine.session();
    assert_eq!(
        query.matches(&mut session, &calendar).unwrap(),
        Match::Matched
    );
    let projected = query.project(&mut session, &calendar).unwrap();
    assert_eq!(
        query.matches(&mut session, &other).unwrap(),
        Match::Unmatched
    );
    assert!(
        projected
            .as_bytes()
            .windows(REFERENCES.len())
            .any(|part| part == REFERENCES.as_bytes())
    );
    assert!(
        !projected
            .as_bytes()
            .windows(b"SUMMARY".len())
            .any(|part| part == b"SUMMARY")
    );
}
