// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Public time boundary tests.

#[cfg(feature = "system-tz")]
use icalkit::Engine;
#[cfg(feature = "system-tz")]
use icalkit::time::LocalKind;
use icalkit::time::{DateTime, IcalDateTime, Timestamp};

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

#[test]
fn leap_second_evidence_survives_beside_jiffs_folded_value() {
    let ordinary = IcalDateTime::floating(DateTime::constant(2026, 6, 30, 23, 59, 59, 0));
    assert!(!ordinary.has_leap_second());

    let leap = ordinary.with_leap_second().unwrap();
    assert!(leap.has_leap_second());
    assert_eq!(
        leap.as_floating(),
        Some(DateTime::constant(2026, 6, 30, 23, 59, 59, 0))
    );

    let utc = IcalDateTime::utc(Timestamp::new(1_782_863_999, 0).unwrap())
        .with_leap_second()
        .unwrap();
    assert!(utc.has_leap_second());
    assert!(
        IcalDateTime::date(icalkit::time::Date::constant(2026, 6, 30))
            .with_leap_second()
            .is_none()
    );
}
