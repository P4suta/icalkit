// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scheduling views obey [RFC 5545 value declarations](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).

#![cfg(test)]

use icalkit_conformance::internal::core::{Document, IgnoreDiagnostics, Limits};
use icalkit_conformance::internal::itip::{
    InstanceMatch, PartStat, Role, ScheduledComponent as _, ScheduledView, SequenceRead,
};

const SOURCE: &str = concat!(
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit value views//EN\r\n",
    "METHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:series@example.test\r\n",
    "DTSTAMP:20260813T120000Z\r\nDTSTART:20260814T090000Z\r\nSEQUENCE:1\r\n",
    "ORGANIZER:mailto:chair@example.test\r\nATTENDEE:mailto:one@example.test\r\n",
    "END:VEVENT\r\nEND:VCALENDAR\r\n",
);

fn read(source: &str) -> Document {
    Document::parse(source.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics).unwrap()
}

fn view(document: &Document) -> ScheduledView<'_> {
    ScheduledView::of(document.components().next().unwrap())
}

#[test]
fn identity_text_requires_the_text_value_type() {
    for value_type in ["X-OPAQUE", "INTEGER", "URI", "DATE", "CAL-ADDRESS"] {
        let source = SOURCE
            .replace("METHOD:", &format!("METHOD;VALUE={value_type}:"))
            .replace("UID:", &format!("UID;VALUE={value_type}:"));
        let document = read(&source);
        let root = view(&document);
        assert_eq!(root.method(), None, "{value_type}");
        assert_eq!(root.child(0).unwrap().uid(), None, "{value_type}");
        assert_eq!(document.to_bytes(), source.as_bytes());
    }
}

#[test]
fn identity_text_is_decoded_without_rewriting_storage() {
    let source = SOURCE.replace("METHOD:", "METHOD;VALUE=TEXT:").replace(
        "UID:series@example.test",
        "UID;VALUE=TEXT:comma\\,semi\\;slash\\\\line\\nend",
    );
    let document = read(&source);
    let root = view(&document);
    assert_eq!(root.method(), Some(b"REQUEST".as_slice()));
    assert_eq!(
        root.child(0).unwrap().uid(),
        Some(b"comma,semi;slash\\line\nend".as_slice())
    );
    assert_eq!(document.to_bytes(), source.as_bytes());
}

#[test]
fn malformed_text_does_not_grant_an_identity() {
    for value in ["bad\\q", "bad;value", "bad,value", "dangling\\"] {
        let source = SOURCE.replace(
            "UID:series@example.test",
            &format!("UID;VALUE=TEXT:{value}"),
        );
        let document = read(&source);
        let root = view(&document);
        assert_eq!(root.child(0).unwrap().uid(), None, "{value}");
    }
}

#[test]
fn opaque_participants_keep_unreadable_parties_and_original_bytes() {
    for value_type in ["X-OPAQUE", "URI", "TEXT", "BINARY", "DATE"] {
        let source = SOURCE
            .replace(
                "ORGANIZER:",
                &format!("ORGANIZER;VALUE={value_type};SENT-BY=\"mailto:agent@example.test\":"),
            )
            .replace(
                "ATTENDEE:",
                &format!("ATTENDEE;VALUE={value_type};ROLE=CHAIR;PARTSTAT=ACCEPTED:"),
            );
        let document = read(&source);
        let root = view(&document);
        let event = root.child(0).unwrap();
        let organizer = event.organizer().expect("the property stays present");
        assert!(!organizer.is_readable(), "{value_type}");
        assert_eq!(organizer.raw(), b"mailto:chair@example.test");
        assert_eq!(organizer.sent_by(), None);
        let attendee = event.attendee(0).expect("the attendee stays present");
        assert!(!attendee.party().is_readable(), "{value_type}");
        assert_eq!(attendee.party().raw(), b"mailto:one@example.test");
        assert_eq!(attendee.role(), Role::RequiredParticipant);
        assert_eq!(attendee.part_stat(), PartStat::NeedsAction);
        assert_eq!(document.to_bytes(), source.as_bytes());
    }
}

#[test]
fn calendar_address_views_require_uri_syntax() {
    for value in [
        "person@example.test",
        "mailto:person name@example.test",
        "https://host:bad/people",
    ] {
        let source = SOURCE.replace(
            "ORGANIZER:mailto:chair@example.test",
            &format!("ORGANIZER:{value}"),
        );
        let document = read(&source);
        let root = view(&document);
        assert!(!root.child(0).unwrap().organizer().unwrap().is_readable());
    }
}

#[test]
fn revisions_require_integer_values() {
    for value_type in ["X-OPAQUE", "TEXT", "URI", "DATE", "DATE-TIME"] {
        let source = SOURCE.replace("SEQUENCE:", &format!("SEQUENCE;VALUE={value_type}:"));
        let document = read(&source);
        let root = view(&document);
        assert_eq!(root.child(0).unwrap().sequence(), SequenceRead::Unreadable);
    }
    let document = read(&SOURCE.replace("SEQUENCE:", "SEQUENCE;VALUE=INTEGER:"));
    let root = view(&document);
    assert_eq!(root.child(0).unwrap().sequence(), SequenceRead::Value(1));
}

#[test]
fn revision_timestamps_require_date_time_values() {
    for value_type in ["X-OPAQUE", "DATE", "TEXT", "URI", "INTEGER"] {
        let source = SOURCE.replace("DTSTAMP:", &format!("DTSTAMP;VALUE={value_type}:"));
        let document = read(&source);
        let root = view(&document);
        assert_eq!(root.child(0).unwrap().dtstamp(), None, "{value_type}");
    }
    let document = read(&SOURCE.replace("DTSTAMP:", "DTSTAMP;VALUE=DATE-TIME:"));
    let root = view(&document);
    assert!(root.child(0).unwrap().dtstamp().is_some());
}

#[test]
fn unreadable_instance_types_cannot_name_the_entire_series() {
    for declaration in [
        "VALUE=X-OPAQUE",
        "VALUE=TEXT",
        "VALUE=URI",
        "VALUE=INTEGER",
        "VALUE=DATE",
    ] {
        let source = SOURCE.replace(
            "SEQUENCE:1",
            &format!("SEQUENCE:1\r\nRECURRENCE-ID;{declaration}:20260814T090000Z"),
        );
        let document = read(&source);
        let root = view(&document);
        let instance = root
            .child(0)
            .unwrap()
            .recurrence_id()
            .expect("the property stays present");
        assert_ne!(
            instance.compare(instance),
            InstanceMatch::Same,
            "{declaration}"
        );
    }
}
