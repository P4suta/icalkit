// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Coverage claims do not imply a transition boundary.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, DiagnosticCode, Instant, Limits, Meter, Severity,
    UtcOffset,
};
use icalkit_conformance::internal::itip::{FoldSide, InstanceClock, InstanceRef, resolve_instance};
use icalkit_conformance::internal::recur::OverrideRange;
use icalkit_conformance::internal::tz::{
    AnswerBasis, LocalResolution, OffsetAnswer, Reading, ResolutionPolicy, ZoneAnswer,
    ZoneProvenance, ZoneSource, ZonedSeries, nominal,
};

struct CoverageSource(AnswerBasis);

impl ZoneSource for CoverageSource {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer> {
        (tzid == "Test/Coverage").then_some(ZoneAnswer::new(
            LocalResolution::Unique {
                reading: Reading::new(local.at_offset(UtcOffset::UTC)?, UtcOffset::UTC, false),
            },
            ZoneProvenance::CallerDatabase,
            self.0,
        ))
    }

    fn offset_at(&self, tzid: &str, _instant: Instant) -> Option<OffsetAnswer> {
        (tzid == "Test/Coverage").then_some(OffsetAnswer::new(
            UtcOffset::UTC,
            false,
            ZoneProvenance::CallerDatabase,
            self.0,
        ))
    }
}

fn local() -> CivilDateTime {
    CivilDateTime::new(
        CivilDate::from_ymd(2024, 1, 2).unwrap(),
        CivilTime::from_hms(9, 0, 0).unwrap(),
    )
}

#[test]
fn unasserted_coverage_has_a_note_without_a_boundary() {
    let basis = AnswerBasis::CoverageUnasserted;
    assert!(!basis.is_computed());
    assert!(!basis.is_before_known_transitions());
    assert!(!basis.is_beyond_known_transitions());
    assert_eq!(basis.nearest_known(), None);
    assert_eq!(
        basis.diagnostic_code(),
        Some(DiagnosticCode::TimeZoneCoverageUnasserted)
    );
    assert_eq!(
        basis.diagnostic_code().unwrap().as_str(),
        "time-zone-coverage-unasserted"
    );
}

#[test]
fn scheduling_keeps_unasserted_coverage_distinct_from_dated_continuation() {
    let boundary = CivilDate::from_ymd(2024, 1, 1).unwrap();
    let first = CivilDate::from_ymd(2024, 1, 3).unwrap();
    for (basis, expected_code, continued, nearest) in [
        (AnswerBasis::Computed, None, false, None),
        (
            AnswerBasis::CoverageUnasserted,
            Some(DiagnosticCode::TimeZoneCoverageUnasserted),
            false,
            None,
        ),
        (
            AnswerBasis::BeforeKnownTransitions(first),
            Some(DiagnosticCode::SchedulingZoneContinued),
            true,
            Some(first),
        ),
        (
            AnswerBasis::BeyondKnownTransitions(boundary),
            Some(DiagnosticCode::SchedulingZoneContinued),
            true,
            Some(boundary),
        ),
    ] {
        for clock in [InstanceClock::Zoned, InstanceClock::Utc] {
            let source = CoverageSource(basis);
            let series = ZonedSeries::new(&source, "Test/Coverage", ResolutionPolicy::DEFAULT);
            let reference =
                InstanceRef::new(nominal(local()).unwrap(), clock, OverrideRange::ThisOnly);
            let mut meter = Meter::new(Limits::DEFAULT);
            let mut diagnostics = Vec::new();
            let resolved = resolve_instance(&series, reference, &mut meter, &mut diagnostics);
            assert_eq!(resolved.side(), FoldSide::Once);
            assert_eq!(resolved.basis(), Some(basis));
            assert_eq!(resolved.is_continued(), continued, "{basis:?}");
            assert_eq!(resolved.nearest_known(), nearest);
            assert_eq!(diagnostics.len(), usize::from(expected_code.is_some()));
            if let Some(code) = expected_code {
                assert_eq!(diagnostics[0].code(), code);
                assert_eq!(diagnostics[0].severity(), Severity::Note);
                assert_eq!(diagnostics[0].instant(), Some(reference.named()));
            }
        }
    }
}
