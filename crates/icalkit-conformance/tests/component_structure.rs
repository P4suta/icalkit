// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Component placement follows [RFC 5545 section 3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6).
//! Unknown alarm subcomponents follow [RFC 9074 section 3](https://www.rfc-editor.org/rfc/rfc9074.html#section-3).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn component(kind: &str, children: &str) -> String {
    let properties = match kind {
        "VCALENDAR" => "VERSION:2.0\r\nPRODID:-//icalkit structure//EN\r\n",
        "VTIMEZONE" => "TZID:Test/Zone\r\n",
        "STANDARD" | "DAYLIGHT" => {
            "DTSTART:20260814T090000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n"
        },
        "VALARM" => {
            "ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nTRIGGER;VALUE=DATE-TIME:20260814T080000Z\r\n"
        },
        _ => "UID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\nDTSTART:20260814T090000Z\r\n",
    };
    format!("BEGIN:{kind}\r\n{properties}{children}END:{kind}\r\n")
}

fn complete_component(kind: &str) -> String {
    match kind {
        "VCALENDAR" => component(kind, &component("VTODO", "")),
        "VTIMEZONE" => component(kind, &component("STANDARD", "")),
        _ => component(kind, ""),
    }
}

fn within_parent(parent: &str, child: &str) -> String {
    let children = if parent == "VTIMEZONE" {
        format!("{}{child}", component("STANDARD", ""))
    } else {
        child.to_owned()
    };
    let parent_component = component(parent, &children);
    let root_child = match parent {
        "VCALENDAR" => return parent_component,
        "STANDARD" | "DAYLIGHT" => component("VTIMEZONE", &parent_component),
        "VALARM" => component("VEVENT", &parent_component),
        _ => parent_component,
    };
    component("VCALENDAR", &root_child)
}

fn assert_rejected(source: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid component structure: {source}");
    };
    assert!(error.issues().iter().any(|issue| {
        issue.is_error()
            && issue.code().as_str() == "icalkit.validation.invalid-component-structure"
    }));
    let imported = Import::read(source.as_bytes()).unwrap();
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

macro_rules! rejects_nesting {
    ($name:ident, $parent:literal, $child:literal) => {
        #[test]
        fn $name() {
            assert_rejected(&within_parent($parent, &complete_component($child)));
        }
    };
}

rejects_nesting!(
    calendar_cannot_contain_another_calendar,
    "VCALENDAR",
    "VCALENDAR"
);
rejects_nesting!(calendar_cannot_contain_alarm, "VCALENDAR", "VALARM");
rejects_nesting!(calendar_cannot_contain_observance, "VCALENDAR", "STANDARD");
rejects_nesting!(event_cannot_contain_todo, "VEVENT", "VTODO");
rejects_nesting!(todo_cannot_contain_event, "VTODO", "VEVENT");
rejects_nesting!(journal_cannot_contain_alarm, "VJOURNAL", "VALARM");
rejects_nesting!(freebusy_cannot_contain_event, "VFREEBUSY", "VEVENT");
rejects_nesting!(timezone_cannot_contain_event, "VTIMEZONE", "VEVENT");
rejects_nesting!(standard_cannot_contain_event, "STANDARD", "VEVENT");
rejects_nesting!(daylight_cannot_contain_event, "DAYLIGHT", "VEVENT");
rejects_nesting!(alarm_cannot_contain_event, "VALARM", "VEVENT");

#[test]
fn calendar_requires_at_least_one_component() {
    assert_rejected(&component("VCALENDAR", ""));
}

#[test]
fn timezone_requires_at_least_one_observance() {
    assert_rejected(&component("VCALENDAR", &component("VTIMEZONE", "")));
}

#[test]
fn permitted_component_trees_preserve_their_bytes() {
    for (parent, child) in [
        ("VCALENDAR", "VEVENT"),
        ("VCALENDAR", "VTODO"),
        ("VCALENDAR", "VJOURNAL"),
        ("VCALENDAR", "VFREEBUSY"),
        ("VCALENDAR", "VTIMEZONE"),
        ("VEVENT", "VALARM"),
        ("VTODO", "VALARM"),
        ("VTIMEZONE", "STANDARD"),
        ("VTIMEZONE", "DAYLIGHT"),
    ] {
        let source = within_parent(parent, &complete_component(child));
        let parsed = Calendar::parse(source.as_bytes()).unwrap();
        assert_eq!(parsed.to_bytes(), source.as_bytes());
    }
    let source = component(
        "VCALENDAR",
        &component("VTIMEZONE", &component("DAYLIGHT", "")),
    );
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
}

#[test]
fn unknown_components_and_alarm_extensions_remain_opaque() {
    let extension = "BEGIN:X-LOCATION\r\nX-LABEL:Keep\r\nEND:X-LOCATION\r\n";
    for parent in ["VCALENDAR", "VEVENT", "VALARM"] {
        let source = within_parent(parent, extension);
        let parsed = Calendar::parse(source.as_bytes()).unwrap();
        assert_eq!(parsed.to_bytes(), source.as_bytes());
    }
    let source = component(
        "VCALENDAR",
        "BEGIN:X-OPAQUE\r\nBEGIN:VEVENT\r\nSUMMARY:Opaque content\r\nEND:VEVENT\r\nEND:X-OPAQUE\r\n",
    );
    assert_eq!(
        Calendar::parse(source.as_bytes()).unwrap().to_bytes(),
        source.as_bytes()
    );
}
