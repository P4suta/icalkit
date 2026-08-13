// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Public time boundary tests.

#[cfg(feature = "system-tz")]
use icalkit::Engine;
#[cfg(feature = "system-tz")]
use icalkit::time::{DateTime, LocalKind};

#[cfg(feature = "system-tz")]
#[test]
fn system_zone_database_reports_dst_gaps_and_folds() {
    let engine = Engine::default();
    let zones = engine.zone_database().unwrap();
    let gap = zones
        .resolve_local(
            "America/New_York",
            DateTime::constant(2024, 3, 10, 2, 30, 0, 0),
        )
        .unwrap();
    let fold = zones
        .resolve_local(
            "America/New_York",
            DateTime::constant(2024, 11, 3, 1, 30, 0, 0),
        )
        .unwrap();

    assert_eq!(gap.kind(), LocalKind::Gap);
    assert!(gap.earlier().is_none());
    assert!(gap.later().is_none());
    assert_eq!(fold.kind(), LocalKind::Fold);
    assert!(fold.earlier().unwrap() < fold.later().unwrap());
    assert!(fold.has_complete_coverage());
    assert_eq!(fold.provenance(), "jiff-system-tzdb");
}
