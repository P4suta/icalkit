// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Identifier selection uses [TEXT values](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.7).

#![cfg(test)]

use std::borrow::Cow;

use icalkit::recurrence::Window;
use icalkit::time::Timestamp;
use icalkit::{Calendar, Engine};

fn event(uid: &str, summary: &str, extra: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\n{uid}\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\nSUMMARY:{summary}\r\n\
         x-CoLoR;X-Meta=kept:before\r\n{extra}END:VEVENT\r\n"
    )
}

fn source(components: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit identity values//EN\r\n\
         {components}END:VCALENDAR\r\n"
    )
}

fn read(source: &str) -> Calendar {
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(calendar.to_bytes(), source.as_bytes());
    calendar
}

fn window() -> Window {
    Window::new(
        "2024-01-01T00:00:00Z".parse::<Timestamp>().unwrap(),
        "2024-01-04T00:00:00Z".parse::<Timestamp>().unwrap(),
    )
    .unwrap()
}

#[test]
fn ordinary_identifier_views_borrow_calendar_storage() {
    let original = source(&event("UID:plain", "Before", ""));
    let calendar = read(&original);
    let uid: Option<Cow<'_, str>> = calendar.events().next().unwrap().uid();
    assert!(matches!(uid, Some(Cow::Borrowed("plain"))));
    assert_eq!(calendar.to_bytes(), original.as_bytes());
}

#[test]
fn identifier_views_decode_text_escapes_without_rewriting_storage() {
    for (line, expected) in [
        (
            "UID;VALUE=TEXT:comma\\,semi\\;slash\\\\line\\nend",
            "comma,semi;slash\\line\nend",
        ),
        ("UID:series\\Nkey", "series\nkey"),
        ("UID:series\\\\nkey", "series\\nkey"),
        ("uid;value=text:series\\\r\n nkey", "series\nkey"),
    ] {
        let original = source(&event(line, "Before", ""));
        let calendar = read(&original);
        let uid = calendar.events().next().unwrap().uid().unwrap();
        assert!(matches!(uid, Cow::Owned(_)));
        assert_eq!(uid.as_ref(), expected);
        assert_eq!(calendar.to_bytes(), original.as_bytes());
    }
}

#[test]
fn opaque_identifiers_keep_bytes_without_granting_a_known_identity() {
    for (declaration, value) in [
        ("X-OPAQUE", "plain"),
        ("FUTURE-TYPE", "series\\nkey"),
        ("X-OPAQUE", "unparsed\\q"),
    ] {
        let original = source(&event(
            &format!("UID;VALUE={declaration}:{value}"),
            "Before",
            "",
        ));
        let calendar = read(&original);
        let event = calendar.events().next().unwrap();
        assert_eq!(event.uid(), None);
        assert_eq!(event.property("UID").unwrap().value(), value.as_bytes());
        assert_eq!(calendar.to_bytes(), original.as_bytes());
    }

    let mut original = source(&event("UID;VALUE=X-OPAQUE:invalid", "Before", "")).into_bytes();
    let at = original
        .windows(b"invalid".len())
        .position(|part| part == b"invalid")
        .unwrap();
    original[at] = 0xff;
    let calendar = Calendar::parse(&original).unwrap();
    assert_eq!(calendar.events().next().unwrap().uid(), None);
    assert_eq!(calendar.to_bytes(), original);
}

#[test]
fn summary_edits_compare_decoded_identifiers_once_without_folding_or_trimming() {
    let components = [
        ("UID:series\\nkey", "newline"),
        ("UID:series\\\\nkey", "backslash"),
        ("UID:Series\\nkey", "case"),
        ("UID:series\\nkey ", "space"),
    ]
    .map(|(uid, name)| event(uid, name, ""))
    .join("");
    let original = source(&components);
    for (uid, name) in [
        ("series\nkey", "newline"),
        ("series\\nkey", "backslash"),
        ("Series\nkey", "case"),
        ("series\nkey ", "space"),
    ] {
        let mut calendar = read(&original);
        let mut editor = calendar.edit();
        editor.set_summary(uid, "Changed").unwrap();
        editor.commit().unwrap();
        let expected = original.replace(&format!("SUMMARY:{name}\r\n"), "SUMMARY:Changed\r\n");
        assert_eq!(calendar.to_bytes(), expected.as_bytes(), "{name}");
    }
}

#[test]
fn vendor_edits_use_decoded_identifiers_and_keep_source_metadata() {
    let original = source(&event(
        "uid;value=text:comma\\,semi\\;slash\\\\line\\Nend",
        "Before",
        "",
    ));
    let mut calendar = read(&original);
    let uid = calendar
        .events()
        .next()
        .unwrap()
        .uid()
        .unwrap()
        .into_owned();
    let mut editor = calendar.edit();
    editor
        .set_vendor_property(&uid, "X-COLOR", b"after")
        .unwrap();
    editor.commit().unwrap();
    let expected = original.replace("x-CoLoR;X-Meta=kept:before", "x-CoLoR;X-Meta=kept:after");
    assert_eq!(calendar.to_bytes(), expected.as_bytes());
}

#[test]
fn unreadable_event_identifiers_cannot_select_an_edit_or_establish_absence() {
    for uid in [
        "UID;VALUE=X-OPAQUE:target",
        "UID;VALUE=FUTURE-TYPE:other",
        "UID;VALUE=X-OPAQUE:unparsed\\q",
    ] {
        let original = source(&event(uid, "Before", ""));
        let mut calendar = read(&original);
        let mut editor = calendar.edit();
        assert_eq!(
            editor
                .set_summary("target", "Changed")
                .unwrap_err()
                .code()
                .as_str(),
            "icalkit.edit.identity-unreadable"
        );
        assert_eq!(
            editor
                .set_vendor_property("target", "X-COLOR", b"after")
                .unwrap_err()
                .code()
                .as_str(),
            "icalkit.edit.identity-unreadable"
        );
        editor.commit().unwrap();
        assert_eq!(calendar.to_bytes(), original.as_bytes());
    }
}

#[test]
fn a_known_match_cannot_hide_an_unreadable_event_identity() {
    let known = event("UID:target", "Known", "");
    let opaque = event("UID;VALUE=X-OPAQUE:other", "Opaque", "");
    for components in [format!("{known}{opaque}"), format!("{opaque}{known}")] {
        let original = source(&components);
        let mut calendar = read(&original);
        let mut editor = calendar.edit();
        assert!(editor.set_summary("target", "Changed").is_err());
        assert!(
            editor
                .set_vendor_property("target", "X-COLOR", b"after")
                .is_err()
        );
        editor.commit().unwrap();
        assert_eq!(calendar.to_bytes(), original.as_bytes());
    }
}

#[test]
fn an_identifier_shared_by_a_series_does_not_choose_the_first_event_for_editing() {
    for (master_uid, override_uid, wanted) in [
        ("UID:plain", "UID:plain", "plain"),
        ("UID:series\\nkey", "UID:series\\Nkey", "series\nkey"),
    ] {
        let master = event(master_uid, "Master", "RRULE:FREQ=DAILY;COUNT=2\r\n");
        let replacement = event(
            override_uid,
            "Override",
            "RECURRENCE-ID:20240102T090000Z\r\n",
        )
        .replace("DTSTART:20240101T090000Z", "DTSTART:20240102T120000Z");
        for components in [
            format!("{master}{replacement}"),
            format!("{replacement}{master}"),
        ] {
            let original = source(&components);
            let mut calendar = read(&original);
            let mut editor = calendar.edit();
            assert_eq!(
                editor
                    .set_summary(wanted, "Changed")
                    .unwrap_err()
                    .code()
                    .as_str(),
                "icalkit.edit.event-ambiguous"
            );
            assert_eq!(
                editor
                    .set_vendor_property(wanted, "X-COLOR", b"after")
                    .unwrap_err()
                    .code()
                    .as_str(),
                "icalkit.edit.event-ambiguous"
            );
            editor.commit().unwrap();
            assert_eq!(calendar.to_bytes(), original.as_bytes());
        }
    }
}

#[test]
fn recurrence_selection_and_resume_use_the_decoded_identifier() {
    let master = event("UID:series\\nkey", "Master", "RRULE:FREQ=DAILY;COUNT=2\r\n");
    let replacement = event(
        "UID:series\\Nkey",
        "Override",
        "RECURRENCE-ID:20240102T090000Z\r\n",
    )
    .replace("DTSTART:20240101T090000Z", "DTSTART:20240102T120000Z");
    let original = source(&format!("{master}{replacement}"));
    let calendar = read(&original);
    let engine = Engine::default();
    let mut session = engine.session();
    let mut occurrences = calendar
        .occurrences(&mut session, "series\nkey", window())
        .unwrap();
    assert_eq!(
        occurrences.try_next().unwrap().unwrap().start(),
        "2024-01-01T09:00:00Z".parse::<Timestamp>().unwrap()
    );
    let cursor = occurrences.cursor();
    drop(occurrences);
    let mut resumed = calendar
        .resume_occurrences(&mut session, "series\nkey", window(), cursor)
        .unwrap();
    assert_eq!(
        resumed.try_next().unwrap().unwrap().start(),
        "2024-01-02T12:00:00Z".parse::<Timestamp>().unwrap()
    );
    assert!(resumed.try_next().unwrap().is_none());
    assert_eq!(calendar.to_bytes(), original.as_bytes());
}

#[test]
fn every_opaque_master_kind_leaves_recurrence_selection_unknown() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"] {
        let closing = if kind == "VFREEBUSY" {
            "DTEND:20240101T100000Z\r\n"
        } else {
            ""
        };
        let opaque = format!(
            "BEGIN:{kind}\r\nUID;VALUE=X-OPAQUE:other\r\nDTSTAMP:20240101T000000Z\r\n\
             DTSTART:20240101T090000Z\r\n{closing}END:{kind}\r\n"
        );
        for components in [
            opaque.clone(),
            format!(
                "{}{opaque}",
                event("UID:target", "Known", "RRULE:FREQ=DAILY;COUNT=2\r\n")
            ),
        ] {
            let original = source(&components);
            let calendar = read(&original);
            let engine = Engine::default();
            let mut session = engine.session();
            let Err(error) = calendar.occurrences(&mut session, "target", window()) else {
                panic!("selected a recurrence master beside an opaque {kind} identity");
            };
            assert_eq!(
                error.code().as_str(),
                "icalkit.recurrence.identity-unreadable"
            );
            assert_eq!(calendar.to_bytes(), original.as_bytes());
        }
    }
}

#[test]
fn equivalent_identifier_spellings_preserve_the_master_ambiguity_check() {
    let first = event("UID:series\\nkey", "First", "RRULE:FREQ=DAILY;COUNT=2\r\n");
    let second = event("UID:series\\Nkey", "Second", "RRULE:FREQ=DAILY;COUNT=2\r\n");
    let original = source(&format!("{first}{second}"));
    let calendar = read(&original);
    let engine = Engine::default();
    let mut session = engine.session();
    let Err(error) = calendar.occurrences(&mut session, "series\nkey", window()) else {
        panic!("selected one of two equivalent master identities");
    };
    assert_eq!(
        error.code().as_str(),
        "icalkit.recurrence.component-ambiguous"
    );
}

#[test]
fn unrelated_component_kinds_do_not_block_event_editing() {
    let unknown = concat!(
        "BEGIN:X-OPAQUE\r\nUID;VALUE=X-OPAQUE:target\r\nEND:X-OPAQUE\r\n",
        "BEGIN:VTODO\r\nUID;VALUE=X-OPAQUE:other\r\nDTSTAMP:20240101T000000Z\r\nEND:VTODO\r\n",
    );
    let original = source(&format!("{}{unknown}", event("UID:target", "Known", "")));
    let mut calendar = read(&original);
    let mut editor = calendar.edit();
    editor.set_summary("target", "Changed").unwrap();
    editor.commit().unwrap();
    assert_eq!(
        calendar.to_bytes(),
        original
            .replace("SUMMARY:Known", "SUMMARY:Changed")
            .as_bytes()
    );
}
