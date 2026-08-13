// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A minimal external consumer: import, explicit normalization, strict promotion, and edit.

use icalkit::interop::{Import, RfcRepairV1};

const INPUT: &[u8] = b"BEGIN:VCALENDAR\n\
VERSION:2.0\n\
PRODID:-//icalkit example//EN\n\
BEGIN:VEVENT\n\
UID:example@example.test\n\
DTSTAMP:20260814T000000Z\n\
SUMMARY:Before\n\
END:VEVENT\n\
END:VCALENDAR\n";

fn main() -> Result<(), icalkit::Error> {
    let imported = Import::read(INPUT)?;
    let normalized = imported.normalize(RfcRepairV1)?;
    for change in normalized.changes() {
        println!("{} at byte {}", change.code(), change.offset());
    }

    let mut calendar = normalized.output().validate()?;
    let mut edit = calendar.edit();
    edit.set_summary("example@example.test", "After")?;
    edit.commit()?;

    print!("{}", String::from_utf8_lossy(&calendar.to_bytes()));
    Ok(())
}
