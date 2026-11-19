// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar format versions follow [RFC 5545 section 3.7.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.7.4).

#![cfg(test)]

use icalkit::Calendar;
use icalkit::interop::Import;

fn calendar(version: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n{version}\r\nPRODID:-//icalkit format versions//EN\r\n\
         BEGIN:VEVENT\r\nUID:version@example.test\r\nDTSTAMP:20240101T000000Z\r\n\
         DTSTART:20240102T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn accepted(source: &[u8]) {
    assert_eq!(Calendar::parse(source).unwrap().to_bytes(), source);
    let imported = Import::read(source).unwrap();
    assert_eq!(imported.as_bytes(), source);
    assert_eq!(imported.validate().unwrap().to_bytes(), source);
}

fn rejected(source: &[u8]) {
    let error = Calendar::parse(source).expect_err("unsupported calendar format");
    assert_eq!(error.code().as_str(), "icalkit.validation.failed");
    assert!(error.issues().iter().any(|issue| issue.is_error()
        && issue.code().as_str() == "icalkit.validation.invalid-enumeration"));
    let imported = Import::read(source).unwrap();
    assert_eq!(imported.as_bytes(), source);
    assert!(imported.validate().is_err());
    assert_eq!(imported.as_bytes(), source);
}

#[test]
fn unsupported_and_malformed_version_text_cant_promote() {
    for value in [
        "banana",
        "",
        "1.0",
        "3.0",
        "2",
        "2.00",
        "02.0",
        "2.0 ",
        " 2.0",
        "2.0\\n",
        "2.0\\,2.0",
        "2.0\\;2.0",
        "2.0,2.0",
        "\"2.0\"",
        "\u{65e5}",
    ] {
        rejected(calendar(&format!("VERSION:{value}")).as_bytes());
    }
}

#[test]
fn version_ranges_need_both_supported_endpoints() {
    for value in [
        ";2.0",
        "2.0;",
        "2.0;2.0;2.0",
        "2.0;3.0",
        "1.0;2.0",
        "3.0;2.0",
        "2.0 ;2.0",
        "2.0; 2.0",
    ] {
        rejected(calendar(&format!("VERSION:{value}")).as_bytes());
    }
}

#[test]
fn calendar_versions_reject_incompatible_recognized_value_types() {
    for value_type in [
        "URI",
        "INTEGER",
        "DATE",
        "DATE-TIME",
        "BINARY",
        "BOOLEAN",
        "UID",
    ] {
        rejected(calendar(&format!("VERSION;VALUE={value_type}:2.0")).as_bytes());
    }
}

#[test]
fn malformed_version_bytes_remain_available_through_lossless_import() {
    let mut source = calendar("VERSION:2.0").into_bytes();
    let position = source.windows(3).position(|part| part == b"2.0").unwrap();
    source[position] = 0xff;
    rejected(&source);
}

#[test]
fn supported_versions_and_ranges_preserve_spelling_and_folds() {
    for version in [
        "VERSION:2.0",
        "VERSION:2.0;2.0",
        "version;X-SOURCE=example:2.0",
        "VERSION;VALUE=TEXT:2.0",
        "VERSION;VALUE=TEXT:2.0;2.0",
        "VeRsIoN;VaLuE=tExT;X-FUTURE=opaque:2.\r\n 0",
        "VERSION:2.0;\r\n 2.0",
        "VERS\r\n ION:2.0",
    ] {
        accepted(calendar(version).as_bytes());
    }
}

#[test]
fn malformed_or_repeated_value_declarations_keep_their_violation() {
    for parameters in [
        "VALUE=",
        "VALUE=TEXT;VALUE=TEXT",
        "VALUE=X-FUTURE;VALUE=TEXT",
    ] {
        let source = calendar(&format!("VERSION;{parameters}:2.0"));
        let error = Calendar::parse(source.as_bytes()).unwrap_err();
        assert!(
            error.issues().iter().any(|issue| issue.is_error()
                && issue.code().as_str() == "malformed-value-type-parameter")
        );
        let imported = Import::read(source.as_bytes()).unwrap();
        assert_eq!(imported.as_bytes(), source.as_bytes());
        assert!(imported.validate().is_err());
    }
}

#[test]
fn unknown_values_properties_and_components_keep_opaque_content() {
    let source = calendar("VERSION;VALUE=X-FUTURE:opaque;data,\\q");
    accepted(source.as_bytes());
    assert!(
        Calendar::parse(source.as_bytes())
            .unwrap()
            .issues()
            .iter()
            .any(|issue| !issue.is_error() && issue.code().as_str() == "unknown-value-type")
    );
    accepted(calendar("VERSION:2.0\r\nX-VERSION;VALUE=:opaque;data,\\q").as_bytes());
    accepted(
        concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit opaque versions//EN\r\n",
            "BEGIN:X-FUTURE\r\nVERSION:banana\r\nBEGIN:VEVENT\r\nVERSION;VALUE=URI:opaque\r\n",
            "END:VEVENT\r\nEND:X-FUTURE\r\nEND:VCALENDAR\r\n",
        )
        .as_bytes(),
    );
}
