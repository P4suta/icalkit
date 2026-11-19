// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Generic URI syntax follows [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986.html#section-3).
//! Address literals follow the syntax restored by [RFC 9844](https://www.rfc-editor.org/rfc/rfc9844.html#section-3).

#![cfg(test)]

use icalkit_conformance::internal::core::{
    DecodeValue as _, DiagnosticCode, Document, EncodeValue as _, Limits, MutationError, UriValue,
    ValueBuf,
};

fn accepted(source: &str) {
    let value = UriValue::decode_value(source.as_bytes()).unwrap();
    assert_eq!(value.as_bytes(), source.as_bytes());
    let mut output = ValueBuf::new();
    value.encode_value(&mut output).unwrap();
    assert_eq!(output.as_bytes(), source.as_bytes());
}

fn rejected(source: &str) {
    assert_eq!(
        UriValue::decode_value(source.as_bytes()),
        Err(DiagnosticCode::MalformedUri),
        "{source}"
    );
    let mut output = ValueBuf::new();
    assert_eq!(
        UriValue::from_bytes(source.as_bytes()).encode_value(&mut output),
        Err(MutationError::NotRepresentable),
        "{source}"
    );
    assert_eq!(output.as_bytes(), []);
}

#[test]
fn authority_components_reject_misplaced_delimiters() {
    for source in [
        "x://user@@example.test/",
        "x://user:pass@user@example.test/",
        "x://user[name]@example.test/",
        "x://example.test]/",
        "x://exam[ple.test/",
        "x://example.test:80:90/",
        "x://[::1]suffix/",
        "x://[::1]:80:90/",
    ] {
        rejected(source);
    }
}

#[test]
fn ports_require_decimal_digits_without_a_machine_integer_limit() {
    for source in [
        "x://host:abc/",
        "x://host:+80/",
        "x://host:-1/",
        "x://host:%38%30/",
        "x://[::1]:a/",
    ] {
        rejected(source);
    }
    for source in [
        "x://host:/",
        "x://host:0/",
        "x://host:00080/",
        "x://[::1]:999999999999999999999999999999999/",
    ] {
        accepted(source);
    }
}

#[test]
fn bracketed_hosts_require_complete_ipv6_or_ipvfuture_addresses() {
    for source in [
        "x://[]/",
        "x://[example.test]/",
        "x://[127.0.0.1]/",
        "x://[1:2:3]/",
        "x://[1:2:3:4:5:6:7:8:9]/",
        "x://[1::2::3]/",
        "x://[::ffff:192.0.2.999]/",
        "x://[::1/",
        "x://[v.abc]/",
        "x://[vg.abc]/",
        "x://[v1.]/",
        "x://[v1.a%20]/",
        "x://[fe80::1%25en0]/",
    ] {
        rejected(source);
    }
    for source in [
        "x://[::]/",
        "x://[::1]/",
        "x://[2001:db8::7]/",
        "x://[1:2:3:4:5:6:7:8]/",
        "x://[::ffff:192.0.2.1]/",
        "x://[1:2:3:4:5:6:192.0.2.1]/",
        "x://[v1.example]/",
        "x://[VF.a!$&'()*+,;=:]/",
    ] {
        accepted(source);
    }
}

#[test]
fn paths_queries_and_fragments_exclude_raw_brackets_and_extra_fragments() {
    for source in [
        "x:/[item]",
        "x:root/[item]",
        "x://host/[item]",
        "x:?a[b]",
        "x:#a[b]",
        "x:#one#two",
        "x:?a#b#c",
    ] {
        rejected(source);
    }
}

#[test]
fn valid_generic_forms_keep_empty_components_and_opaque_paths() {
    for source in [
        "x:",
        "x:?",
        "x:#",
        "x:?#",
        "x:/",
        "x://",
        "x:///path",
        "x:////path",
        "x://user:pass@:/path",
        "x://@host/",
        "x://192.0.2.1/",
        "x://256.0.0.1/",
        "x:root:part@host/../item",
        "mailto:Jane.Doe@example.test",
        "urn:example:a:b",
        "data:text/plain;base64,QQ==",
        "x:/path?query?/a:b@c#fragment?/a:b@c",
    ] {
        accepted(source);
    }
}

#[test]
fn encoded_delimiters_and_source_case_are_preserved() {
    accepted("Scheme+Name.2://user%40name:password@Example%2eTest:0080/%5Bitem%5D?x=%23%3f#%5b");
    for source in [
        "x://host/%",
        "x://host/%0",
        "x://host/%gg",
        "x://host%0G/",
        "x:?%",
        "x:#%",
    ] {
        rejected(source);
    }
}

#[test]
fn lossless_storage_keeps_malformed_uri_text_without_a_typed_view() {
    let source = b"BEGIN:VEVENT\r\nURL:x://[example.test]:bad/[item]#a#b\r\nEND:VEVENT\r\n";
    let document = Document::parse(source, Limits::DEFAULT, &mut Vec::new()).unwrap();
    assert_eq!(document.to_bytes(), source);
    let property = document
        .components()
        .next()
        .unwrap()
        .properties()
        .next()
        .unwrap();
    assert!(!property.value::<UriValue<'_>>().is_valid());
    assert_eq!(
        property.value_text().as_bytes(),
        b"x://[example.test]:bad/[item]#a#b"
    );
}
