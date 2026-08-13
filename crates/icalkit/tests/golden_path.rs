// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Golden-path tests through the public facade only.

use icalkit::interop::{CommonClientsV1, Import, RfcRepairV1};
use icalkit::{Calendar, Engine, Error, Issue, IssueCode, ResourcePolicy, Session};

const VALID: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
SUMMARY:before\r\n\
X-OWNER-COLOR:plum\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

#[test]
fn root_types_are_the_only_setup_a_normal_consumer_needs() {
    let engine = Engine::builder()
        .resource_policy(ResourcePolicy::secure())
        .build();
    let mut session: Session<'_> = engine.session();
    let calendar = session.parse(VALID).unwrap();

    assert_eq!(calendar.to_bytes(), VALID);
    assert_eq!(calendar.events().next().unwrap().uid(), "one@example.test");
    assert!(calendar.issues().is_empty());

    let _: Option<Error> = None;
    let _: Option<Issue> = None;
    let _: Option<IssueCode> = None;
}

#[test]
fn strict_parse_rejects_violations_while_import_keeps_every_octet() {
    const BROKEN: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:one@example.test\r\n\
UID:two@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    let error = Calendar::parse(BROKEN).unwrap_err();
    assert!(error.issues().iter().any(Issue::is_error));

    let import = Import::read(BROKEN).unwrap();
    assert_eq!(import.as_bytes(), BROKEN);
}

#[test]
fn notes_survive_promotion_and_unknown_extensions_are_not_errors() {
    let mut noted = VALID.to_vec();
    let marker = b"X-OWNER-COLOR:plum";
    let at = noted
        .windows(marker.len())
        .position(|run| run == marker)
        .unwrap();
    noted.splice(
        at..at + marker.len(),
        b"X-OWNER-COLOR;X-NOTE=plum^x:plum".iter().copied(),
    );

    let calendar = Calendar::parse(&noted).unwrap();
    assert!(calendar.issues().iter().any(Issue::is_note));
    assert!(
        calendar
            .events()
            .next()
            .unwrap()
            .property("X-OWNER-COLOR")
            .is_some()
    );
}

#[test]
fn normalization_reports_changes_is_idempotent_and_never_mutates_the_import() {
    let bare_lf: Vec<u8> = VALID
        .iter()
        .copied()
        .filter(|octet| *octet != b'\r')
        .collect();
    let original = Import::read(&bare_lf).unwrap();

    let repaired = original.normalize(RfcRepairV1).unwrap();
    assert_eq!(original.as_bytes(), bare_lf);
    assert!(!repaired.changes().is_empty());
    assert_eq!(repaired.output().as_bytes(), VALID);
    assert!(
        repaired
            .output()
            .normalize(RfcRepairV1)
            .unwrap()
            .changes()
            .is_empty()
    );
    assert!(
        original
            .normalize(CommonClientsV1)
            .unwrap()
            .changes()
            .is_empty()
    );
}

#[test]
fn editor_rolls_back_by_default_and_commit_is_line_local() {
    let mut calendar = Calendar::parse(VALID).unwrap();
    {
        let mut edit = calendar.edit();
        edit.set_summary("one@example.test", "discarded").unwrap();
    }
    assert_eq!(calendar.to_bytes(), VALID);

    let untouched = b"X-OWNER-COLOR:plum\r\n";
    let mut edit = calendar.edit();
    edit.set_summary("one@example.test", "after").unwrap();
    edit.commit().unwrap();
    let written = calendar.to_bytes();
    assert!(written.windows(untouched.len()).any(|run| run == untouched));
    assert!(written.windows(15).any(|run| run == b"SUMMARY:after\r\n"));
}

#[test]
fn editor_refuses_content_line_injection_without_touching_the_calendar() {
    let mut calendar = Calendar::parse(VALID).unwrap();
    let before = calendar.to_bytes();
    let mut edit = calendar.edit();
    assert!(
        edit.set_summary(
            "one@example.test",
            "safe\r\nATTENDEE:mailto:eve@example.test"
        )
        .is_err()
    );
    drop(edit);
    assert_eq!(calendar.to_bytes(), before);
}

#[test]
fn known_date_times_are_validated_once_and_leap_evidence_is_typed() {
    const LEAP: &[u8] = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//icalkit tests//EN\r\n\
BEGIN:VEVENT\r\n\
UID:leap@example.test\r\n\
DTSTAMP:20260813T120000Z\r\n\
DTSTART:20260630T235960Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
    let calendar = Calendar::parse(LEAP).unwrap();
    let start = calendar.events().next().unwrap().dtstart().unwrap();
    assert!(start.has_leap_second());

    let invalid = LEAP
        .windows(b"20260630T235960Z".len())
        .position(|window| window == b"20260630T235960Z")
        .map(|at| {
            let mut bytes = LEAP.to_vec();
            bytes.splice(
                at..at + b"20260630T235960Z".len(),
                b"20261330T235959Z".iter().copied(),
            );
            bytes
        })
        .unwrap();
    let error = Calendar::parse(&invalid).unwrap_err();
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| issue.code().as_str() == "icalkit.validation.invalid-date-time")
    );
}
