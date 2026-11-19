// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [RFC 5546 section 3.2.5](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.2.5) permits cancellation notices to omit the stored event's properties.

#![cfg(test)]

use icalkit_conformance::internal::core::{Document, Limits, ProposedChange};
use icalkit_conformance::internal::itip::{Method, ScheduledView, describe_payload};

#[test]
fn a_sparse_cancel_preserves_properties_that_it_does_not_echo() {
    let held = Document::parse(
        b"BEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTAMP:20260813T120000Z\r\n\
          DTSTART:20260814T090000Z\r\nDTEND:20260814T100000Z\r\n\
          SUMMARY:Planning\r\nDESCRIPTION:Keep notes\r\nRRULE:FREQ=DAILY\r\n\
          ATTENDEE:mailto:one@example.test\r\nATTENDEE:mailto:two@example.test\r\n\
          X-COLOR:plum\r\nEND:VEVENT\r\n",
        Limits::DEFAULT,
        &mut Vec::new(),
    )
    .unwrap();
    let offered = Document::parse(
        b"BEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTAMP:20260813T130000Z\r\n\
          ATTENDEE:mailto:two@example.test\r\nSEQUENCE:2\r\nEND:VEVENT\r\n",
        Limits::DEFAULT,
        &mut Vec::new(),
    )
    .unwrap();
    let held_view = ScheduledView::of(held.components().next().unwrap());
    let offered_view = ScheduledView::of(offered.components().next().unwrap());
    let transition = describe_payload(Method::Cancel, &offered_view, &held_view);
    let removed: Vec<_> = transition
        .changes()
        .filter(|(_, change)| matches!(change, ProposedChange::Remove))
        .map(|(at, _)| at.name())
        .collect();
    assert_eq!(removed, Vec::<&[u8]>::new());
    assert!(transition.changes().all(|(at, _)| at.name() != b"ATTENDEE"));
    assert_eq!(transition.len(), 2);

    let replacement = describe_payload(Method::Request, &offered_view, &held_view);
    assert!(replacement.changes().any(|(at, change)| {
        at.name() == b"SUMMARY" && matches!(change, ProposedChange::Remove)
    }));
}
