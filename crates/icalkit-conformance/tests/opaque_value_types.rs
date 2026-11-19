// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [RFC 5545 section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20) requires preserving unknown value types without interpreting their data.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    BinaryValue, CivilDate, CivilDateTime, CivilTime, DateTimeValue, DecodeValue, DiagnosticCode,
    Document, Duration, Geo, Limits, Period, Severity, TextValue, UriValue, UtcOffset, View,
};

fn assert_opaque<T>(view: &View<'_, T>) {
    assert!(!view.is_valid());
    let diagnostic = view.diagnostic().unwrap();
    assert_eq!(diagnostic.code(), DiagnosticCode::UnknownValueType);
    assert_eq!(diagnostic.severity(), Severity::Note);
}

macro_rules! opaque_value {
    ($name:ident, $value_type:ty, $value:literal) => {
        #[test]
        fn $name() {
            let input = concat!(
                "BEGIN:VEVENT\r\nDTSTART;VALUE=X-PRIVATE:",
                $value,
                "\r\nEND:VEVENT\r\n",
            );
            let document =
                Document::parse(input.as_bytes(), Limits::DEFAULT, &mut Vec::new()).unwrap();
            let component = document.components().next().unwrap();
            let property = component.properties().next().unwrap();
            assert_opaque::<$value_type>(&property.value());
            assert_eq!(
                <$value_type>::decode_property_field(property, property.value_text().as_bytes()),
                Err(DiagnosticCode::UnknownValueType)
            );
            assert_eq!(document.to_bytes(), input.as_bytes());
        }
    };
}

opaque_value!(date, CivilDate, "20260814");
opaque_value!(time, CivilTime, "090000");
opaque_value!(local_date_time, CivilDateTime, "20260814T090000");
opaque_value!(date_time, DateTimeValue<'_>, "20260814T090000Z");
opaque_value!(duration, Duration, "P1D");
opaque_value!(offset, UtcOffset, "+0100");
opaque_value!(integer, i32, "3");
opaque_value!(float, f64, "1.5");
opaque_value!(boolean, bool, "TRUE");
opaque_value!(geo, Geo, "10.1;20.2");
opaque_value!(text, TextValue<'_>, "Keep");
opaque_value!(binary, BinaryValue<'_>, "aGVsbG8=");
opaque_value!(uri, UriValue<'_>, "https://example.test/");
opaque_value!(period, Period<'_>, "20260814T090000Z/20260814T100000Z");
opaque_value!(opaque_duration, Duration, "opaque");
opaque_value!(opaque_date_time, DateTimeValue<'_>, "opaque");
opaque_value!(opaque_period, Period<'_>, "opaque");

#[test]
fn list_fields_keep_the_property_zone() {
    let input =
        b"BEGIN:VEVENT\r\nRDATE;TZID=Test/Zone:20260815T090000,20260816T090000\r\nEND:VEVENT\r\n";
    let document = Document::parse(input, Limits::DEFAULT, &mut Vec::new()).unwrap();
    let property = document
        .components()
        .next()
        .unwrap()
        .properties()
        .next()
        .unwrap();
    for field in property
        .value_text()
        .as_bytes()
        .split(|octet| *octet == b',')
    {
        let value = DateTimeValue::decode_property_field(property, field).unwrap();
        assert_eq!(value.tzid(), Some(b"Test/Zone".as_slice()));
    }

    let input =
        b"BEGIN:VEVENT\r\nRDATE;VALUE=PERIOD;TZID=Test/Zone:20260815T090000/PT1H\r\nEND:VEVENT\r\n";
    let document = Document::parse(input, Limits::DEFAULT, &mut Vec::new()).unwrap();
    let property = document
        .components()
        .next()
        .unwrap()
        .properties()
        .next()
        .unwrap();
    let value = Period::decode_property_field(property, property.value_text().as_bytes()).unwrap();
    assert_eq!(value.start().tzid(), Some(b"Test/Zone".as_slice()));
}
