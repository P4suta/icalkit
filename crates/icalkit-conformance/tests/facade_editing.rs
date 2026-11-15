// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The public editor follows the property-name rules in [RFC 5545 section 3.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.5).

use icalkit::{Calendar, Error};

const SOURCE: &str = concat!(
    "BEGIN:VCALENDAR\r\n",
    "VERSION:2.0\r\n",
    "PRODID:-//icalkit facade editing//EN\r\n",
    "BEGIN:VEVENT\r\n",
    "UID:one@example.test\r\n",
    "DTSTAMP:20260813T120000Z\r\n",
    "DTSTART:20260813T090000Z\r\n",
    "SUMMARY:before\r\n",
    "x-CoLoR;X-Meta=\"Keep\":before\r\n",
    "END:VEVENT\r\n",
    "END:VCALENDAR\r\n",
);

#[test]
fn vendor_property_creation_accepts_each_case_and_preserves_its_spelling() -> Result<(), Error> {
    for name in ["X-ADDED", "x-added", "x-AdDeD", "X-added"] {
        let mut calendar = Calendar::parse(SOURCE.as_bytes())?;
        let mut edit = calendar.edit();
        edit.set_vendor_property("one@example.test", name, b"Value")?;
        edit.commit()?;

        let expected = SOURCE.replace("END:VEVENT\r\n", &format!("{name}:Value\r\nEND:VEVENT\r\n"));
        assert_eq!(calendar.to_bytes(), expected.as_bytes(), "created {name}");
    }
    Ok(())
}

#[test]
fn vendor_property_replacement_preserves_the_existing_name_and_parameters() -> Result<(), Error> {
    for name in ["X-COLOR", "x-color", "x-CoLoR", "X-CoLoR"] {
        let mut calendar = Calendar::parse(SOURCE.as_bytes())?;
        let mut edit = calendar.edit();
        edit.set_vendor_property("one@example.test", name, b"Revised")?;
        edit.commit()?;

        let expected = SOURCE.replace(
            "x-CoLoR;X-Meta=\"Keep\":before",
            "x-CoLoR;X-Meta=\"Keep\":Revised",
        );
        assert_eq!(calendar.to_bytes(), expected.as_bytes(), "replaced {name}");
    }
    Ok(())
}

#[test]
fn rejected_names_leave_the_transaction_committable_and_unchanged() -> Result<(), Error> {
    for name in [
        "",
        "X",
        "x",
        "SUMMARY",
        "BEGIN",
        "X-BAD:NAME",
        "x-bad name",
        "x-\u{2603}",
    ] {
        let mut calendar = Calendar::parse(SOURCE.as_bytes())?;
        let mut edit = calendar.edit();
        assert!(
            edit.set_vendor_property("one@example.test", name, b"rejected")
                .is_err(),
            "accepted {name:?}"
        );
        edit.commit()?;
        assert_eq!(calendar.to_bytes(), SOURCE.as_bytes(), "rejected {name:?}");
    }
    Ok(())
}
