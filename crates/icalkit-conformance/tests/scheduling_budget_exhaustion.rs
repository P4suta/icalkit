// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scheduling validation follows [RFC 5546 section 3](https://www.rfc-editor.org/rfc/rfc5546.html#section-3).
//! Resource refusals follow [ADR 0010](../../../docs/adr/0010-shared-resource-limits.md).

#![cfg(test)]

use icalkit::scheduling::Message;
use icalkit::{Engine, ResourcePolicy};

fn refused_at_every_budget(bytes: &[u8], issue: &str) {
    let error = Message::read(bytes).unwrap_err();
    assert!(
        error
            .issues()
            .iter()
            .any(|found| found.code().as_str() == issue)
    );
    for budget in 0..=8192 {
        let engine = Engine::builder()
            .resource_policy(ResourcePolicy::secure().with_max_input_bytes(budget))
            .build();
        let result = Message::read_in(&mut engine.session(), bytes);
        assert!(
            result.is_err(),
            "accepted an invalid message with budget {budget}"
        );
    }
}

#[test]
fn exhaustion_cannot_hide_a_required_property_violation() {
    let source = include_str!("fixtures/break_itip_methods/request_without_a_summary.ics");
    let bytes = source.replace("\r\n", "\n").replace('\n', "\r\n");
    refused_at_every_budget(bytes.as_bytes(), "scheduling-required-property-missing");
}

#[test]
fn exhaustion_cannot_hide_a_forbidden_property_violation() {
    let bytes = concat!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit tests//EN\r\n",
        "METHOD:REQUEST\r\nBEGIN:VFREEBUSY\r\nUID:busy@example.test\r\n",
        "DTSTAMP:20240101T000000Z\r\nDTSTART:20240102T000000Z\r\nDTEND:20240103T000000Z\r\n",
        "ORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\n",
        "URL:https://calendar.example.test/busy\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n",
    );
    refused_at_every_budget(bytes.as_bytes(), "scheduling-property-not-allowed");
}

#[test]
fn a_sufficient_budget_preserves_a_valid_message() {
    let source = include_str!("fixtures/break_itip_methods/request_without_a_summary.ics");
    let bytes = source
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .replace("END:VEVENT", "SUMMARY:Planning\r\nEND:VEVENT");
    let engine = Engine::default();
    let message = Message::read_in(&mut engine.session(), bytes.as_bytes()).unwrap();
    assert_eq!(message.method(), "REQUEST");
    assert_eq!(message.to_bytes(), bytes.as_bytes());
}
