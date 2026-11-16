// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Actor addresses follow [RFC 5545 section 3.3.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.3).
//! URI syntax follows [RFC 3986 section 3](https://www.rfc-editor.org/rfc/rfc3986.html#section-3).

#![cfg(test)]

use icalkit::scheduling::Actor;

#[test]
fn actors_refuse_relative_addresses_and_malformed_uri_components() {
    for address in [
        "person@example.test",
        "/principals/person",
        "#person",
        "mailto:bad%2",
        "mailto:bad%GG@example.test",
        "ht^tp://example.test/person",
        "https://example.test:bad/person",
        "https://[broken]/person",
        "https://example.test/\u{00e9}",
        "mailto:person\u{00a0}@example.test",
        "mailto:person\\name@example.test",
        "",
        "mailto:person name@example.test",
        "mailto:person\0@example.test",
        "mailto:person\r\n@example.test",
    ] {
        assert!(
            Actor::new(address).is_none(),
            "accepted malformed actor address {address:?}"
        );
    }
}

#[test]
fn actors_preserve_valid_calendar_user_address_spellings() {
    for address in [
        "MAILTO:Person%2Btag@Example.test",
        "mailto:person%C3%A9@example.test",
        "urn:example:person",
        "https://calendar.example.test/principals/a,b?view=x#fragment",
        "x-cua:opaque%2Cvalue",
        "https://[2001:db8::1]/principals/person",
    ] {
        let actor = Actor::new(address).unwrap();
        assert_eq!(actor.as_str(), address);
    }
}
