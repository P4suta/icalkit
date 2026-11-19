// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Event starts obey [declared value types](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;
use icalkit::time::{Date, DateTime, IcalDateTime, Timestamp};

fn source(start: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit event start//EN\r\n\
         BEGIN:VEVENT\r\nUID:start@example.test\r\nDTSTAMP:20260813T120000Z\r\n\
         {start}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn opaque_date_shaped_values_do_not_grant_a_known_start() {
    for declaration in ["X-OPAQUE", "FUTURE-TIME"] {
        for value in ["20260814", "20260814T090000", "20260814T090000Z"] {
            let source = source(&format!("DTSTART;VALUE={declaration}:{value}"));
            let calendar = Calendar::parse(source.as_bytes()).unwrap();
            assert_eq!(
                calendar.events().next().unwrap().dtstart(),
                None,
                "{declaration}, {value}"
            );
            assert_eq!(calendar.to_bytes(), source.as_bytes());
        }
    }
}

#[test]
fn opaque_zoned_values_do_not_grant_a_known_start() {
    let source = source("DTSTART;VALUE=X-OPAQUE;TZID=Test/Zone:20260814T090000");
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(calendar.events().next().unwrap().dtstart(), None);
    assert_eq!(calendar.to_bytes(), source.as_bytes());
}

#[test]
fn opaque_start_properties_stay_available_for_lossless_inspection() {
    let line = "dTsTaRt;VALUE=X-OPAQUE;X-META=kept:unparsed\\q";
    let source = source(line);
    let calendar = Calendar::parse(source.as_bytes()).unwrap();
    let event = calendar.events().next().unwrap();
    let property = event.property("DTSTART").unwrap();
    assert_eq!(property.name().as_str(), "dTsTaRt");
    assert_eq!(property.value(), b"unparsed\\q");
    assert_eq!(event.dtstart(), None);
    assert_eq!(calendar.to_bytes(), source.as_bytes());
    let imported = Import::read(source.as_bytes()).unwrap();
    assert_eq!(imported.as_bytes(), source.as_bytes());
    assert_eq!(imported.validate().unwrap().to_bytes(), source.as_bytes());
}

#[test]
fn known_start_forms_keep_their_temporal_meaning() {
    let local = "2026-08-14T09:00:00".parse::<DateTime>().unwrap();
    let utc = "2026-08-14T09:00:00Z".parse::<Timestamp>().unwrap();
    for (line, expected) in [
        ("DTSTART:20260814T090000Z", IcalDateTime::utc(utc)),
        (
            "DTSTART;VALUE=date-time:20260814T090000Z",
            IcalDateTime::utc(utc),
        ),
        ("DTSTART:20260814T090000", IcalDateTime::floating(local)),
        (
            "DTSTART;TZID=Test/Zone:20260814T090000",
            IcalDateTime::zoned(local, "Test/Zone"),
        ),
        (
            "DTSTART;VALUE=DATE:20260814",
            IcalDateTime::date("2026-08-14".parse::<Date>().unwrap()),
        ),
        (
            "DTSTART:20161231T235960Z",
            IcalDateTime::utc("2016-12-31T23:59:59Z".parse::<Timestamp>().unwrap())
                .with_leap_second()
                .unwrap(),
        ),
    ] {
        let source = source(line);
        let calendar = Calendar::parse(source.as_bytes()).unwrap();
        assert_eq!(
            calendar.events().next().unwrap().dtstart(),
            Some(expected),
            "{line}"
        );
        assert_eq!(calendar.to_bytes(), source.as_bytes());
    }
}
