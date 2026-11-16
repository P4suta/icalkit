// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Alarm constraints follow [RFC 5545 section 3.6.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.6) and [section 3.8.6.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.6.3).
//! Trigger controls follow [section 3.2.14](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.14).
//! Alarm identifiers and acknowledgements follow [RFC 9074 sections 4 and 6](https://www.rfc-editor.org/rfc/rfc9074.html#section-4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(kind: &str, properties: &str, alarm: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit alarms//EN\r\n\
         BEGIN:{kind}\r\nUID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n\
         {properties}BEGIN:VALARM\r\n{alarm}END:VALARM\r\n\
         END:{kind}\r\nEND:VCALENDAR\r\n"
    )
}

fn assert_valid(source: &str) {
    let parsed = Calendar::parse(source.as_bytes()).unwrap();
    assert_eq!(parsed.to_bytes(), source.as_bytes());
}

fn assert_rejected(source: &str) {
    let Err(error) = Calendar::parse(source.as_bytes()) else {
        panic!("accepted invalid alarm: {source}");
    };
    assert!(error.issues().iter().any(|issue| {
        issue.is_error() && issue.code().as_str() == "icalkit.validation.invalid-alarm"
    }));
    let imported = Import::read(source.as_bytes()).unwrap();
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source.as_bytes());
}

const START: &str = "DTSTART:20260814T090000Z\r\n";
const DISPLAY: &str = "ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nTRIGGER:-PT15M\r\n";

#[test]
fn display_and_email_actions_require_their_action_properties() {
    for alarm in [
        "ACTION:DISPLAY\r\nTRIGGER:-PT15M\r\n",
        "ACTION:EMAIL\r\nTRIGGER:-PT15M\r\n",
        "ACTION:EMAIL\r\nDESCRIPTION:Body\r\nTRIGGER:-PT15M\r\n",
        "ACTION:EMAIL\r\nDESCRIPTION:Body\r\nSUMMARY:Subject\r\nTRIGGER:-PT15M\r\n",
    ] {
        assert_rejected(&calendar("VEVENT", START, alarm));
    }
    assert_valid(&calendar("VEVENT", START, DISPLAY));
    assert_valid(&calendar(
        "VEVENT",
        START,
        "ACTION:EMAIL\r\nDESCRIPTION:Body\r\nSUMMARY:Subject\r\n\
         ATTENDEE:mailto:one@example.test\r\nATTENDEE:mailto:two@example.test\r\n\
         TRIGGER:-PT15M\r\n",
    ));
}

#[test]
fn actions_restrict_properties_and_audio_attachment_count() {
    for alarm in [
        format!("{DISPLAY}SUMMARY:Subject\r\n"),
        format!("{DISPLAY}ATTACH:https://example.test/sound\r\n"),
        "ACTION:AUDIO\r\nTRIGGER:-PT15M\r\nDESCRIPTION:Body\r\n".to_owned(),
        "ACTION:AUDIO\r\nTRIGGER:-PT15M\r\nATTENDEE:mailto:one@example.test\r\n".to_owned(),
        "ACTION:AUDIO\r\nTRIGGER:-PT15M\r\nATTACH:https://example.test/one\r\n\
         ATTACH:https://example.test/two\r\n"
            .to_owned(),
    ] {
        assert_rejected(&calendar("VEVENT", START, &alarm));
    }
    assert_valid(&calendar(
        "VEVENT",
        START,
        "ACTION:AUDIO\r\nTRIGGER:PT0S\r\nATTACH:https://example.test/sound\r\n",
    ));
}

#[test]
fn actions_require_a_nonempty_standard_or_extension_token() {
    for action in ["", "NOT AN ACTION", "DISPLAY,EMAIL"] {
        assert_rejected(&calendar(
            "VEVENT",
            START,
            &format!("ACTION:{action}\r\nTRIGGER:-PT15M\r\n"),
        ));
    }
}

#[test]
fn repeat_and_duration_must_appear_together() {
    for extra in ["REPEAT:2\r\n", "DURATION:PT5M\r\n"] {
        assert_rejected(&calendar("VEVENT", START, &format!("{DISPLAY}{extra}")));
    }
    for repeat in [0, 2] {
        assert_valid(&calendar(
            "VEVENT",
            START,
            &format!("{DISPLAY}REPEAT:{repeat}\r\nDURATION:PT5M\r\n"),
        ));
    }
}

#[test]
fn repeat_intervals_must_be_positive_and_counts_must_be_nonnegative_integers() {
    for extra in [
        "REPEAT:2\r\nDURATION:-PT5M\r\n",
        "REPEAT:2\r\nDURATION:PT0S\r\n",
        "REPEAT:2\r\nDURATION:invalid\r\n",
        "REPEAT:-1\r\nDURATION:PT5M\r\n",
        "REPEAT:two\r\nDURATION:PT5M\r\n",
        "REPEAT:2147483648\r\nDURATION:PT5M\r\n",
    ] {
        assert_rejected(&calendar("VEVENT", START, &format!("{DISPLAY}{extra}")));
    }
}

#[test]
fn relative_triggers_require_the_requested_parent_anchor() {
    assert_rejected(&calendar("VTODO", "", DISPLAY));
    assert_rejected(&calendar("VTODO", "DUE:20260814T100000Z\r\n", DISPLAY));
    let end_trigger = DISPLAY.replace("TRIGGER:-PT15M", "TRIGGER;RELATED=END:-PT15M");
    assert_rejected(&calendar("VEVENT", START, &end_trigger));
    assert_rejected(&calendar("VTODO", START, &end_trigger));
    for (kind, properties) in [
        ("VEVENT", format!("{START}DTEND:20260814T100000Z\r\n")),
        ("VEVENT", format!("{START}DURATION:PT1H\r\n")),
        ("VTODO", "DUE:20260814T100000Z\r\n".to_owned()),
        ("VTODO", format!("{START}DURATION:PT1H\r\n")),
    ] {
        assert_valid(&calendar(kind, &properties, &end_trigger));
    }
}

#[test]
fn absolute_triggers_require_explicit_utc_date_times_without_related() {
    for trigger in [
        "TRIGGER:20260814T080000Z",
        "TRIGGER;VALUE=DATE-TIME:20260814T080000",
        "TRIGGER;VALUE=DATE:20260814",
        "TRIGGER;VALUE=DATE-TIME;RELATED=START:20260814T080000Z",
        "TRIGGER;VALUE=DATE-TIME;TZID=Test/Zone:20260814T080000Z",
    ] {
        assert_rejected(&calendar(
            "VTODO",
            "",
            &format!("ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\n{trigger}\r\n"),
        ));
    }
    assert_valid(&calendar(
        "VTODO",
        "",
        "ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\n\
         TRIGGER;VALUE=DATE-TIME:20260814T080000Z\r\n",
    ));
}

#[test]
fn trigger_parameters_must_be_unambiguous() {
    for trigger in [
        "TRIGGER;RELATED=UNKNOWN:-PT15M",
        "TRIGGER;RELATED=START;RELATED=END:-PT15M",
        "TRIGGER;VALUE=DURATION;VALUE=DURATION:-PT15M",
    ] {
        assert_rejected(&calendar(
            "VEVENT",
            START,
            &DISPLAY.replace("TRIGGER:-PT15M", trigger),
        ));
    }
}

#[test]
fn trigger_relationships_use_unquoted_start_or_end_tokens() {
    for kind in ["VEVENT", "VTODO"] {
        for action in [
            "ACTION:AUDIO\r\n",
            "ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\n",
            "ACTION:EMAIL\r\nDESCRIPTION:Body\r\nSUMMARY:Subject\r\n\
             ATTENDEE:mailto:one@example.test\r\n",
        ] {
            for related in [
                "\"START\"",
                "\"END\"",
                "\"start\"",
                "\"\r\n START\"",
                "",
                "START,END",
                "START END",
                "X-FUTURE",
                "START^n",
                "\u{ff33}TART",
            ] {
                assert_rejected(&calendar(
                    kind,
                    &format!("{START}DURATION:PT1H\r\n"),
                    &format!("{action}TRIGGER;RELATED={related}:-PT15M\r\n"),
                ));
            }
        }
    }
}

#[test]
fn trigger_relationships_keep_case_folding_and_extension_parameters() {
    for kind in ["VEVENT", "VTODO"] {
        for trigger in [
            "TRIGGER:-PT15M",
            "TRIGGER;RELATED=Start:-PT15M",
            "TRIGGER;RELATED=End:-PT15M",
            "trigger;value=\"DURATION\";related=START:-PT15M",
            "TRIGGER;RELATED\r\n =END:-PT15M",
            "TRIGGER;X-SOURCE=one;RELATED=END;X-SOURCE=two:-PT15M",
        ] {
            assert_valid(&calendar(
                kind,
                &format!("{START}DURATION:PT1H\r\n"),
                &DISPLAY.replace("TRIGGER:-PT15M", trigger),
            ));
        }
    }
}

#[test]
fn unknown_trigger_values_actions_properties_and_components_stay_opaque() {
    assert_valid(&calendar(
        "VTODO",
        "",
        "ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\n\
         TRIGGER;VALUE=X-FUTURE;RELATED=\"UNKNOWN\":opaque\r\n",
    ));
    assert_valid(&calendar(
        "VTODO",
        "",
        "ACTION:X-CUSTOM\r\nTRIGGER;RELATED=\"UNKNOWN\":opaque\r\n",
    ));
    assert_valid(&calendar(
        "VEVENT",
        START,
        &format!("{DISPLAY}X-TRIGGER;RELATED=\"UNKNOWN\":opaque\r\n"),
    ));
    assert_valid(&calendar(
        "VEVENT",
        START,
        &DISPLAY.replace("TRIGGER:-PT15M", "TRIGGER;X-RELATED=\"UNKNOWN\":-PT15M"),
    ));
    assert_valid(
        &calendar(
            "VEVENT",
            START,
            &DISPLAY.replace("TRIGGER:-PT15M", "TRIGGER;RELATED=\"UNKNOWN\":opaque"),
        )
        .replace("VEVENT", "X-EXTENSION"),
    );
}

#[test]
fn extension_actions_and_value_types_keep_their_opaque_payloads() {
    assert_valid(&calendar(
        "VTODO",
        "",
        "ACTION:X-CUSTOM\r\nTRIGGER:opaque\r\nX-SETTINGS:Keep\r\n",
    ));
    assert_valid(&calendar(
        "VTODO",
        "",
        "ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nTRIGGER;VALUE=X-CUSTOM:opaque\r\n",
    ));
}

#[test]
fn alarm_identifiers_are_unique_per_component_and_acknowledgements_use_utc() {
    assert_rejected(&calendar(
        "VEVENT",
        START,
        &format!("{DISPLAY}UID:one\r\nUID:two\r\n"),
    ));
    for acknowledgement in ["20260814T080000", "20260814"] {
        assert_rejected(&calendar(
            "VEVENT",
            START,
            &format!("{DISPLAY}ACKNOWLEDGED:{acknowledgement}\r\n"),
        ));
    }
    assert_valid(&calendar(
        "VEVENT",
        START,
        &format!("{DISPLAY}UID:one\r\nRELATED-TO:original\r\nACKNOWLEDGED:20260814T080000Z\r\n"),
    ));
}
