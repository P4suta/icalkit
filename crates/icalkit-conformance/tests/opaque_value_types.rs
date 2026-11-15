// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [RFC 5545 section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20) requires preserving unknown value types without interpreting their data.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    BinaryValue, CivilDate, CivilDateTime, CivilTime, DateTimeValue, DiagnosticCode, Document,
    Duration, Geo, Limits, Period, Severity, TextValue, UriValue, UtcOffset, View,
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
