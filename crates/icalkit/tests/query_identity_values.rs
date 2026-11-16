// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence identity follows [RFC 5545 text values](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11).

#![cfg(test)]

use icalkit::caldav::{Match, Query};
use icalkit::{Calendar, Engine};

fn source(master_uid: &str, override_uid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit query identity//EN\r\n\
         BEGIN:VEVENT\r\n{master_uid}\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240101T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=3\r\n\
         SUMMARY:Master\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\n{override_uid}\r\nDTSTAMP:20240101T000000Z\r\n\
         RECURRENCE-ID:20240102T090000Z\r\nDTSTART:20240102T120000Z\r\n\
         DURATION:PT1H\r\nSUMMARY:Moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn parsed(source: &str) -> Calendar {
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(calendar.to_bytes(), source.as_bytes());
    calendar
}

fn query(start: &str, end: &str, summary: Option<&str>, projection: &str) -> Query {
    let property = summary.map_or_else(String::new, |text| {
        format!(
            "<C:prop-filter name=\"SUMMARY\"><C:text-match>{text}</C:text-match></C:prop-filter>"
        )
    });
    Query::parse(
        format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
<D:prop><C:calendar-data>{projection}</C:calendar-data></D:prop>
<C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
<C:time-range start="{start}" end="{end}"/>{property}
</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
        )
        .as_bytes(),
    )
    .unwrap()
}

fn matches(calendar: &Calendar, start: &str, end: &str, summary: Option<&str>) -> Match {
    let engine = Engine::default();
    query(start, end, summary, "")
        .matches(&mut engine.session(), calendar)
        .unwrap()
}

fn project(calendar: &Calendar, operation: &str) -> Result<String, icalkit::Error> {
    let projection =
        format!("<C:{operation} start=\"20240102T000000Z\" end=\"20240103T000000Z\"/>");
    let engine = Engine::default();
    query("20240102T000000Z", "20240103T000000Z", None, &projection)
        .project(&mut engine.session(), calendar)
        .map(|result| String::from_utf8(result.as_bytes().to_vec()).unwrap())
}

#[test]
fn equivalent_text_escapes_share_one_recurrence_set() {
    for (master, replacement) in [
        ("UID:series\\nkey", "UID:series\\Nkey"),
        ("UID;VALUE=TEXT:series\\Nkey", "uid;value=text:series\\nkey"),
    ] {
        let source = source(master, replacement);
        let calendar = parsed(&source);
        assert_eq!(
            matches(&calendar, "20240102T090000Z", "20240102T100000Z", None),
            Match::Unmatched
        );
        assert_eq!(
            matches(&calendar, "20240102T120000Z", "20240102T130000Z", None),
            Match::Matched
        );
        assert_eq!(calendar.to_bytes(), source.as_bytes());
    }
}

#[test]
fn effective_properties_use_the_decoded_series_identity() {
    let calendar = parsed(&source("UID:series\\nkey", "UID:series\\Nkey"));
    assert_eq!(
        matches(
            &calendar,
            "20240102T090000Z",
            "20240102T100000Z",
            Some("Master")
        ),
        Match::Unmatched
    );
    assert_eq!(
        matches(
            &calendar,
            "20240102T120000Z",
            "20240102T130000Z",
            Some("Moved")
        ),
        Match::Matched
    );
}

#[test]
fn expansion_emits_the_replacement_once() {
    let source = source("UID:series\\nkey", "UID:series\\Nkey");
    let calendar = parsed(&source);
    let expanded = project(&calendar, "expand").unwrap();
    assert_eq!(expanded.matches("BEGIN:VEVENT").count(), 1);
    assert!(expanded.contains("DTSTART:20240102T120000Z"));
    assert!(expanded.contains("SUMMARY:Moved"));
    assert!(!expanded.contains("DTSTART:20240102T090000Z"));
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn recurrence_limiting_drops_a_known_override_outside_the_window() {
    let source = source("UID:series\\nkey", "UID:series\\Nkey");
    let calendar = parsed(&source);
    let projection =
        "<C:limit-recurrence-set start=\"20240105T000000Z\" end=\"20240106T000000Z\"/>";
    let engine = Engine::default();
    let projected = query("20240105T000000Z", "20240106T000000Z", None, projection)
        .project(&mut engine.session(), &calendar)
        .unwrap();
    let text = String::from_utf8(projected.as_bytes().to_vec()).unwrap();
    assert_eq!(text.matches("BEGIN:VEVENT").count(), 1);
    assert!(text.contains("SUMMARY:Master"));
    assert!(!text.contains("SUMMARY:Moved"));
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

fn opaque_pairs() -> [(&'static str, &'static str); 4] {
    [
        ("UID;VALUE=X-OPAQUE:series", "UID;VALUE=X-OPAQUE:series"),
        ("UID;VALUE=X-OPAQUE:series", "UID:other"),
        ("UID:series", "UID;VALUE=X-OPAQUE:other"),
        ("UID;VALUE=X-OPAQUE:unparsed\\q", "UID:series"),
    ]
}

#[test]
fn unreadable_series_relationships_leave_time_matching_undecided() {
    for (master, replacement) in opaque_pairs() {
        let source = source(master, replacement);
        let calendar = parsed(&source);
        assert_eq!(
            matches(&calendar, "20240102T090000Z", "20240102T100000Z", None),
            Match::Undecided,
            "{master}, {replacement}"
        );
        assert_eq!(
            matches(
                &calendar,
                "20240102T090000Z",
                "20240102T100000Z",
                Some("Master")
            ),
            Match::Undecided,
            "{master}, {replacement}"
        );
        assert_eq!(calendar.to_bytes(), source.as_bytes());
    }
}

#[test]
fn unreadable_series_relationships_refuse_expansion() {
    for (master, replacement) in opaque_pairs() {
        let source = source(master, replacement);
        let calendar = parsed(&source);
        let Err(error) = project(&calendar, "expand") else {
            panic!("expanded an unreadable relationship: {master}, {replacement}");
        };
        assert_eq!(error.code().as_str(), "icalkit.caldav.query-projection");
        assert_eq!(calendar.to_bytes(), source.as_bytes());
    }
}

#[test]
fn recurrence_limiting_keeps_unreadable_overrides() {
    for (master, replacement) in opaque_pairs() {
        let calendar = parsed(&source(master, replacement));
        let projection =
            "<C:limit-recurrence-set start=\"20240105T000000Z\" end=\"20240106T000000Z\"/>";
        let engine = Engine::default();
        let projected = query("20240105T000000Z", "20240106T000000Z", None, projection)
            .project(&mut engine.session(), &calendar)
            .unwrap();
        let text = String::from_utf8(projected.as_bytes().to_vec()).unwrap();
        assert!(text.contains(replacement));
        assert!(text.contains("SUMMARY:Moved"));
    }
}

#[test]
fn distinct_decoded_identifiers_keep_independent_occurrences() {
    for (master, replacement) in [
        ("UID:series\\\\nkey", "UID:series\\nkey"),
        ("UID:Series", "UID:series"),
        ("UID:series", "UID:series "),
        ("UID:one", "UID:other"),
    ] {
        let source = source(master, replacement);
        let calendar = parsed(&source);
        assert_eq!(
            matches(&calendar, "20240102T090000Z", "20240102T100000Z", None),
            Match::Matched
        );
        assert_eq!(
            project(&calendar, "expand")
                .unwrap()
                .matches("BEGIN:VEVENT")
                .count(),
            2
        );
        assert_eq!(calendar.to_bytes(), source.as_bytes());
    }
}

#[test]
fn an_opaque_identifier_without_a_series_relationship_does_not_block_time_reads() {
    let source = source("UID;VALUE=X-OPAQUE:series", "UID:other");
    let source = source
        .split("BEGIN:VEVENT")
        .take(2)
        .collect::<Vec<_>>()
        .join("BEGIN:VEVENT");
    let source = format!("{source}END:VCALENDAR\r\n");
    let calendar = parsed(&source);
    assert_eq!(
        matches(&calendar, "20240102T090000Z", "20240102T100000Z", None),
        Match::Matched
    );
    assert_eq!(
        project(&calendar, "expand")
            .unwrap()
            .matches("BEGIN:VEVENT")
            .count(),
        1
    );
}
