// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Contextual validation follows [RFC 5545 sections 3.6.1 and 3.6.2](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.1) and [section 3.8.2.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.2.4).
//! Date-time forms follow [section 3.2.19](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.19), [section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20), and the property definitions in [section 3.8](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8).
//! Endpoint relationships follow [sections 3.8.2.2 through 3.8.2.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.2.2) and [section 3.8.4.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, properties: &str, calendar_properties: &str) -> String {
    let zone =
        if properties.contains("Test/Zone") && !calendar_properties.contains("TZID:Test/Zone") {
            concat!(
                "BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\nBEGIN:STANDARD\r\n",
                "DTSTART:20200101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\n",
                "END:STANDARD\r\nEND:VTIMEZONE\r\n",
            )
        } else {
            ""
        };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit validation//EN\r\n\
         BEGIN:{kind}\r\nUID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n\
         {properties}END:{kind}\r\n{zone}{calendar_properties}END:VCALENDAR\r\n"
    )
}

fn assert_valid(source: &str) {
    let parsed = Calendar::parse(source.as_bytes()).expect("valid RFC 5545 input");
    assert_eq!(parsed.to_bytes(), source.as_bytes());
}

fn assert_rejected(source: &str, code: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid RFC 5545 input: {source}");
    };
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.is_error() && issue.code().as_str() == code),
        "missing {code}: {error:?}"
    );
    let imported = Import::read(source.as_bytes()).expect("lossless import");
    assert_eq!(imported.as_bytes(), source.as_bytes());
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

#[test]
fn an_event_requires_start_when_its_calendar_has_no_method() {
    assert_rejected(&calendar("VEVENT", "", ""), "missing-required-property");
    assert_valid(&calendar("VEVENT", "DTSTART:20260814T090000Z\r\n", ""));
    assert_valid(&calendar("VEVENT", "dtstart;value=date:20260814\r\n", ""));
}

#[test]
fn a_method_after_the_event_allows_omitting_its_start() {
    for method in ["METHOD:CANCEL\r\n", "mEtHoD:CANCEL\r\n"] {
        assert_valid(&calendar(
            "VEVENT",
            "ORGANIZER:mailto:organizer@example.test\r\n",
            method,
        ));
    }
}

#[test]
fn a_method_inside_an_extension_does_not_apply_to_its_siblings() {
    assert_rejected(
        &calendar(
            "VEVENT",
            "",
            "BEGIN:X-EXTENSION\r\nMETHOD:CANCEL\r\nEND:X-EXTENSION\r\n",
        ),
        "missing-required-property",
    );
}

#[test]
fn a_todo_with_duration_requires_start() {
    assert_rejected(
        &calendar("VTODO", "DURATION:P1D\r\n", ""),
        "missing-required-property",
    );
    assert_valid(&calendar(
        "VTODO",
        "DTSTART;VALUE=DATE:20260814\r\nDURATION:P1D\r\n",
        "",
    ));
}

#[test]
fn a_todo_or_journal_without_a_rule_can_omit_start() {
    assert_valid(&calendar("VTODO", "", ""));
    assert_valid(&calendar("VTODO", "DUE;VALUE=DATE:20260814\r\n", ""));
    assert_valid(&calendar("VJOURNAL", "", ""));
}

#[test]
fn every_recurring_component_requires_start_even_when_method_is_present() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        assert_rejected(
            &calendar(kind, "RRULE:FREQ=DAILY;COUNT=2\r\n", "METHOD:PUBLISH\r\n"),
            "missing-required-property",
        );
        assert_valid(&calendar(
            kind,
            "DTSTART;VALUE=DATE:20260814\r\nRRULE:FREQ=DAILY;COUNT=2\r\n",
            "METHOD:PUBLISH\r\n",
        ));
    }
}

#[test]
fn component_stamps_require_utc_date_times() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"] {
        let source = calendar(kind, "DTSTART:20260814T090000Z\r\n", "");
        assert_valid(&source);
        for replacement in [
            "DTSTAMP:20260813T120000",
            "DTSTAMP;VALUE=DATE:20260813",
            "DTSTAMP;TZID=Test/Zone:20260813T120000",
        ] {
            assert_rejected(
                &source.replace("DTSTAMP:20260813T120000Z", replacement),
                "icalkit.validation.invalid-date-time",
            );
        }
    }
}

#[test]
fn creation_modification_and_completion_times_require_utc() {
    for (kind, name) in [
        ("VEVENT", "CREATED"),
        ("VTODO", "CREATED"),
        ("VJOURNAL", "CREATED"),
        ("VEVENT", "LAST-MODIFIED"),
        ("VTODO", "LAST-MODIFIED"),
        ("VJOURNAL", "LAST-MODIFIED"),
        ("VTODO", "COMPLETED"),
    ] {
        assert_valid(&calendar(
            kind,
            &format!("DTSTART:20260814T090000Z\r\n{name}:20260813T120000Z\r\n"),
            "",
        ));
        for value in [":20260813T120000", ";VALUE=DATE:20260813"] {
            assert_rejected(
                &calendar(
                    kind,
                    &format!("DTSTART:20260814T090000Z\r\n{name}{value}\r\n"),
                    "",
                ),
                "icalkit.validation.invalid-date-time",
            );
        }
    }
}

#[test]
fn date_time_values_must_match_their_declared_types() {
    for property in [
        "DTSTART:20260814",
        "DTSTART;VALUE=DATE:20260814T090000Z",
        "DTSTART;VALUE=DATE-TIME:20260814",
        "DTSTART;VALUE=TEXT:20260814T090000Z",
    ] {
        assert_rejected(
            &calendar("VEVENT", &format!("{property}\r\n"), ""),
            "icalkit.validation.invalid-date-time",
        );
    }
    for property in [
        "dtstart;value=date:20260814",
        "DTSTART:20260814T090000Z",
        "DTSTART;VALUE=DATE-TIME:20260814T090000",
        "DTSTART;TZID=Test/Zone:20260814T090000",
    ] {
        assert_valid(&calendar("VEVENT", &format!("{property}\r\n"), ""));
    }
}

#[test]
fn date_and_utc_values_cannot_carry_a_time_zone_parameter() {
    for property in [
        "DTSTART;VALUE=DATE;TZID=Test/Zone:20260814",
        "DTSTART;TZID=Test/Zone:20260814T090000Z",
        "DTSTART;TZID=\"\":20260814T090000Z",
        "DTSTART;TZID=:20260814T090000",
    ] {
        assert_rejected(
            &calendar("VEVENT", &format!("{property}\r\n"), ""),
            "icalkit.validation.invalid-date-time",
        );
    }
}

#[test]
fn date_time_parameters_cannot_repeat() {
    for property in [
        "DTSTART;VALUE=DATE-TIME;VALUE=DATE-TIME:20260814T090000Z",
        "DTSTART;TZID=Test/Zone;TZID=Test/Zone:20260814T090000",
    ] {
        assert_rejected(
            &calendar("VEVENT", &format!("{property}\r\n"), ""),
            "icalkit.validation.invalid-date-time",
        );
    }
}

#[test]
fn freebusy_bounds_require_utc_date_times() {
    let source = calendar(
        "VFREEBUSY",
        "DTSTART:20260814T090000Z\r\nDTEND:20260814T100000Z\r\n",
        "",
    );
    assert_valid(&source);
    for (original, replacement) in [
        ("DTSTART:20260814T090000Z", "DTSTART:20260814T090000"),
        ("DTEND:20260814T100000Z", "DTEND;VALUE=DATE:20260814"),
    ] {
        assert_rejected(
            &source.replace(original, replacement),
            "icalkit.validation.invalid-date-time",
        );
    }
}

#[test]
fn observance_starts_require_unzoned_local_date_times() {
    let source = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit validation//EN\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\nBEGIN:STANDARD\r\n",
        "DTSTART:20260814T090000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n",
        "END:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n",
    );
    assert_valid(source);
    for replacement in [
        "DTSTART:20260814T090000Z",
        "DTSTART;VALUE=DATE:20260814",
        "DTSTART;TZID=Test/Zone:20260814T090000",
    ] {
        assert_rejected(
            &source.replace("DTSTART:20260814T090000", replacement),
            "icalkit.validation.invalid-date-time",
        );
    }
}

#[test]
fn unknown_date_time_types_survive_without_interpretation() {
    for value in ["20260814T090000Z", "opaque"] {
        let source = calendar(
            "VEVENT",
            &format!("DTSTART;VALUE=X-PRIVATE:{value}\r\n"),
            "",
        );
        let parsed = Calendar::parse(source.as_bytes()).unwrap();
        assert_eq!(parsed.to_bytes(), source.as_bytes());
        assert!(parsed.events().next().unwrap().dtstart().is_none());
        assert!(
            parsed
                .issues()
                .iter()
                .any(|issue| { issue.is_note() && issue.code().as_str() == "unknown-value-type" })
        );
    }
}

#[test]
fn unrecognized_components_do_not_define_standard_property_semantics() {
    assert_valid(&calendar(
        "VEVENT",
        "DTSTART:20260814T090000Z\r\n",
        "BEGIN:X-OPAQUE\r\nDTSTART:opaque\r\nBEGIN:VTODO\r\n\
         SUMMARY:Extension payload\r\nEND:VTODO\r\nEND:X-OPAQUE\r\n",
    ));
}

#[test]
fn event_ends_and_todo_due_dates_match_the_start_type_and_floating_form() {
    for (kind, end_name) in [("VEVENT", "DTEND"), ("VTODO", "DUE")] {
        for (start, end) in [
            (";VALUE=DATE:20260814", ":20260815T100000Z"),
            (":20260814T090000Z", ";VALUE=DATE:20260815"),
            (":20260814T090000", ":20260814T100000Z"),
            (":20260814T090000Z", ":20260814T100000"),
        ] {
            assert_rejected(
                &calendar(kind, &format!("DTSTART{start}\r\n{end_name}{end}\r\n"), ""),
                "icalkit.validation.invalid-time-range",
            );
        }
        assert_valid(&calendar(
            kind,
            &format!("DTSTART;VALUE=DATE:20260814\r\n{end_name};VALUE=DATE:20260815\r\n"),
            "",
        ));
    }
}

#[test]
fn comparable_endpoints_must_be_strictly_after_start() {
    for (kind, end_name) in [
        ("VEVENT", "DTEND"),
        ("VTODO", "DUE"),
        ("VFREEBUSY", "DTEND"),
    ] {
        for end in ["20260814T090000Z", "20260814T085959Z"] {
            assert_rejected(
                &calendar(
                    kind,
                    &format!("DTSTART:20260814T090000Z\r\n{end_name}:{end}\r\n"),
                    "",
                ),
                "icalkit.validation.invalid-time-range",
            );
        }
        assert_valid(&calendar(
            kind,
            &format!("DTSTART:20260814T090000Z\r\n{end_name}:20260814T090001Z\r\n"),
            "",
        ));
    }
    for (start, end) in [
        (";VALUE=DATE:20260814", ";VALUE=DATE:20260814"),
        (";VALUE=DATE:20260814", ";VALUE=DATE:20260813"),
        (":20260814T090000", ":20260814T090000"),
        (":20260814T090000", ":20260814T085959"),
    ] {
        assert_rejected(
            &calendar("VEVENT", &format!("DTSTART{start}\r\nDTEND{end}\r\n"), ""),
            "icalkit.validation.invalid-time-range",
        );
    }
}

#[test]
fn identical_zoned_endpoints_cannot_form_event_or_todo_ranges() {
    let zone = concat!(
        "BEGIN:VTIMEZONE\r\nTZID:Test/Zone\r\nBEGIN:STANDARD\r\n",
        "DTSTART:20200101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\n",
        "END:STANDARD\r\nEND:VTIMEZONE\r\n",
    );
    for (kind, endpoint) in [("VEVENT", "DTEND"), ("VTODO", "DUE")] {
        let source = calendar(
            kind,
            &format!(
                "DTSTART;TZID=Test/Zone:20260814T090000\r\n{endpoint};TZID=Test/Zone:20260814T090000\r\n"
            ),
            zone,
        );
        assert_valid(&source.replace(
            &format!("{endpoint};TZID=Test/Zone:20260814T090000"),
            &format!("{endpoint};TZID=Test/Zone:20260814T100000"),
        ));
        assert_rejected(&source, "icalkit.validation.invalid-time-range");
    }
}

#[test]
fn alarm_identifiers_are_optional_singletons_and_relationships_repeat() {
    let source = calendar(
        "VEVENT",
        concat!(
            "DTSTART:20260814T090000Z\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\n",
            "TRIGGER:-PT15M\r\nDESCRIPTION:Reminder\r\nRELATED-TO:parent-a\r\nRELATED-TO:parent-b\r\n",
            "END:VALARM\r\n",
        ),
        "",
    );
    assert_valid(&source);
    assert_valid(&source.replace("END:VALARM", "UID:alarm-a\r\nEND:VALARM"));
    assert_rejected(
        &source.replace("END:VALARM", "UID:alarm-a\r\nUID:alarm-b\r\nEND:VALARM"),
        "duplicate-property",
    );
}

#[test]
fn recurrence_identifiers_match_the_start_type_without_restricting_rescheduling() {
    for kind in ["VEVENT", "VTODO", "VJOURNAL"] {
        for (start, recurrence_id) in [
            (";VALUE=DATE:20260814", ":20260814T090000Z"),
            (":20260814T090000Z", ";VALUE=DATE:20260814"),
            (":20260814T090000", ":20260814T090000Z"),
        ] {
            assert_rejected(
                &calendar(
                    kind,
                    &format!("DTSTART{start}\r\nRECURRENCE-ID{recurrence_id}\r\n"),
                    "",
                ),
                "icalkit.validation.invalid-time-range",
            );
        }
        assert_valid(&calendar(
            kind,
            "DTSTART:20260815T090000Z\r\nRECURRENCE-ID:20260814T090000Z\r\n",
            "",
        ));
    }
}

#[test]
fn event_and_todo_durations_must_be_positive_duration_values() {
    for kind in ["VEVENT", "VTODO"] {
        for duration in [
            "DURATION:PT0S",
            "DURATION:-P1D",
            "DURATION:invalid",
            "DURATION;VALUE=DATE-TIME:20260814T100000Z",
        ] {
            assert_rejected(
                &calendar(
                    kind,
                    &format!("DTSTART:20260814T090000Z\r\n{duration}\r\n"),
                    "",
                ),
                "icalkit.validation.invalid-duration",
            );
        }
        assert_valid(&calendar(
            kind,
            "DTSTART:20260814T090000Z\r\nDURATION:PT1S\r\n",
            "",
        ));
    }
}

#[test]
fn date_durations_use_day_or_week_syntax() {
    for duration in ["PT24H", "P1DT0S", "P1DT24H"] {
        assert_rejected(
            &calendar(
                "VEVENT",
                &format!("DTSTART;VALUE=DATE:20260814\r\nDURATION:{duration}\r\n"),
                "",
            ),
            "icalkit.validation.invalid-duration",
        );
    }
    for duration in ["P1D", "+P2D", "P1W"] {
        assert_valid(&calendar(
            "VEVENT",
            &format!("DTSTART;VALUE=DATE:20260814\r\nDURATION:{duration}\r\n"),
            "",
        ));
    }
}

#[test]
fn unresolved_zoned_endpoints_are_not_ordered_as_wall_clocks() {
    assert_valid(&calendar(
        "VEVENT",
        "DTSTART;TZID=Test/Zone:20260814T031500\r\nDTEND;TZID=Test/Zone:20260814T024500\r\n",
        "",
    ));
}
