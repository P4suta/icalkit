// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Captured producer evidence is consumed only through the single public facade.

use icalkit::Calendar;
use icalkit::interop::{CommonClientsV1, Import};

const GOOGLE_DST_GAP: &[u8] =
    include_bytes!("fixtures/captured/google_calendar_70_9054_dst_gap.ics");

#[test]
fn google_calendar_gap_export_is_strict_and_needs_no_compatibility_repair()
-> Result<(), Box<dyn std::error::Error>> {
    assert!(
        GOOGLE_DST_GAP
            .iter()
            .enumerate()
            .all(|(index, octet)| *octet != b'\n'
                || GOOGLE_DST_GAP.get(index.wrapping_sub(1)) == Some(&b'\r')),
        "the reduced capture must retain Google's CRLF layout"
    );
    assert!(
        GOOGLE_DST_GAP
            .windows(b"UID:google-gap-2027@example.invalid".len())
            .any(|window| window == b"UID:google-gap-2027@example.invalid")
    );

    let _calendar = Calendar::parse(GOOGLE_DST_GAP)?;
    let imported = Import::read(GOOGLE_DST_GAP)?;
    assert_eq!(imported.as_bytes(), GOOGLE_DST_GAP);

    let normalized = imported.normalize(CommonClientsV1)?;
    assert!(normalized.changes().is_empty());
    assert_eq!(normalized.output().as_bytes(), GOOGLE_DST_GAP);
    let _validated = normalized.output().validate()?;
    Ok(())
}
