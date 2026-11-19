// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared exhaustion blocks every later admission while preserving accepted results.

#![cfg(test)]

use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, LimitExceeded,
    Limits, Meter, ParseError, UtcOffset,
};
use icalkit_conformance::internal::recur::{InputError, InputList, OverrideSet};
use icalkit_conformance::internal::tz::{
    Observance, TransitionTable, VtimezoneSet, ZoneSetError, read_calendar_zones,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    Parse(ParseError),
    Limit(LimitExceeded),
}

#[derive(Clone, Copy, Debug)]
enum Charge {
    Bytes,
    Items,
    Depth,
    Elements,
    XmlDepth,
    Responses,
    Text,
    Bindings,
    Candidates,
    Occurrences,
    Rdates,
    Exdates,
    Overrides,
    Observances,
    Zones,
    Freebusy,
}

impl Charge {
    const ALL: [Self; 16] = [
        Self::Bytes,
        Self::Items,
        Self::Depth,
        Self::Elements,
        Self::XmlDepth,
        Self::Responses,
        Self::Text,
        Self::Bindings,
        Self::Candidates,
        Self::Occurrences,
        Self::Rdates,
        Self::Exdates,
        Self::Overrides,
        Self::Observances,
        Self::Zones,
        Self::Freebusy,
    ];
    const BULK: [Self; 6] = [
        Self::Bytes,
        Self::Text,
        Self::Rdates,
        Self::Exdates,
        Self::Overrides,
        Self::Observances,
    ];

    fn limits(self) -> Limits {
        match self {
            Self::Bytes => Limits::DEFAULT.with_max_input_bytes(0),
            Self::Items => Limits::DEFAULT.with_max_items(0),
            Self::Depth => Limits::DEFAULT.with_max_component_depth(0),
            Self::Elements => Limits::DEFAULT.with_max_xml_elements(0),
            Self::XmlDepth => Limits::DEFAULT.with_max_xml_depth(0),
            Self::Responses => Limits::DEFAULT.with_max_responses(0),
            Self::Text => Limits::DEFAULT.with_max_xml_text_bytes(0),
            Self::Bindings => Limits::DEFAULT.with_max_prefix_bindings(0),
            Self::Candidates => Limits::DEFAULT.with_candidates_per_period(0),
            Self::Occurrences => Limits::DEFAULT.with_occurrences_per_search(0),
            Self::Rdates => Limits::DEFAULT.with_rdate_entries(0),
            Self::Exdates => Limits::DEFAULT.with_exdate_entries(0),
            Self::Overrides => Limits::DEFAULT.with_override_entries(0),
            Self::Observances => Limits::DEFAULT.with_max_vtimezone_observances(0),
            Self::Zones => Limits::DEFAULT.with_max_vtimezone_components(0),
            Self::Freebusy => Limits::DEFAULT.with_max_freebusy_periods(0),
        }
    }

    fn apply(self, meter: &mut Meter, count: u32) -> Result<(), Refusal> {
        match self {
            Self::Items => meter.charge_item().map_err(Refusal::Parse),
            Self::Depth => meter.enter().map_err(Refusal::Parse),
            Self::Bytes => meter
                .try_charge_bytes(u64::from(count))
                .map_err(Refusal::Limit),
            Self::Elements => meter.try_charge_element().map_err(Refusal::Limit),
            Self::XmlDepth => meter.try_enter_element().map_err(Refusal::Limit),
            Self::Responses => meter.try_charge_response().map_err(Refusal::Limit),
            Self::Text => meter.try_charge_text(count).map_err(Refusal::Limit),
            Self::Bindings => meter.try_bind_prefix().map_err(Refusal::Limit),
            Self::Candidates => meter.try_charge_candidate().map_err(Refusal::Limit),
            Self::Occurrences => meter.try_charge_occurrence().map_err(Refusal::Limit),
            Self::Rdates => meter
                .try_charge_rdate_entries(count)
                .map_err(Refusal::Limit),
            Self::Exdates => meter
                .try_charge_exdate_entries(count)
                .map_err(Refusal::Limit),
            Self::Overrides => meter
                .try_charge_override_entries(count)
                .map_err(Refusal::Limit),
            Self::Observances => meter
                .try_charge_vtimezone_observances(count)
                .map_err(Refusal::Limit),
            Self::Zones => meter
                .try_charge_vtimezone_component()
                .map_err(Refusal::Limit),
            Self::Freebusy => meter.try_charge_freebusy_period().map_err(Refusal::Limit),
        }
    }

    fn first_error(self) -> Refusal {
        match self {
            Self::Items => Refusal::Parse(ParseError::TooManyItems { limit: 0 }),
            Self::Depth => Refusal::Parse(ParseError::TooDeep { limit: 0 }),
            Self::Bytes => Refusal::Limit(LimitExceeded::Budget),
            Self::Elements => Refusal::Limit(LimitExceeded::Elements),
            Self::XmlDepth => Refusal::Limit(LimitExceeded::Depth),
            Self::Responses => Refusal::Limit(LimitExceeded::Responses),
            Self::Text => Refusal::Limit(LimitExceeded::Text),
            Self::Bindings => Refusal::Limit(LimitExceeded::PrefixBindings),
            Self::Candidates => Refusal::Limit(LimitExceeded::Candidates),
            Self::Occurrences => Refusal::Limit(LimitExceeded::Occurrences),
            Self::Rdates => Refusal::Limit(LimitExceeded::RdateEntries),
            Self::Exdates => Refusal::Limit(LimitExceeded::ExdateEntries),
            Self::Overrides => Refusal::Limit(LimitExceeded::OverrideEntries),
            Self::Observances => Refusal::Limit(LimitExceeded::VtimezoneObservances),
            Self::Zones => Refusal::Limit(LimitExceeded::VtimezoneComponents),
            Self::Freebusy => Refusal::Limit(LimitExceeded::FreeBusyPeriods),
        }
    }

    fn exhausted_error(self) -> Refusal {
        match self {
            Self::Items | Self::Depth => Refusal::Parse(ParseError::Exhausted),
            _ => Refusal::Limit(LimitExceeded::Exhausted),
        }
    }
}

fn counts(meter: &Meter) -> (u64, u32, u16, u16, u32, u32, u64, u32, u32, u32, u32) {
    (
        meter.spent(),
        meter.items(),
        meter.depth(),
        meter.live_prefix_bindings(),
        meter.responses(),
        meter.candidates_in_period(),
        meter.candidates_charged(),
        meter.occurrences(),
        meter.vtimezone_observances(),
        meter.vtimezone_components(),
        meter.freebusy_periods(),
    )
}

#[test]
fn every_first_refusal_keeps_its_dimension_and_latches_the_ledger() {
    for trigger in Charge::ALL {
        let mut meter = Meter::new(trigger.limits());
        assert_eq!(
            trigger.apply(&mut meter, 1),
            Err(trigger.first_error()),
            "{trigger:?}"
        );
        assert!(meter.is_exhausted(), "{trigger:?}");
    }
}

#[test]
fn every_admission_refuses_after_every_resource_kind_without_mutating_counts() {
    for trigger in Charge::ALL {
        for receiver in Charge::ALL {
            let mut meter = Meter::new(trigger.limits());
            trigger.apply(&mut meter, 1).unwrap_err();
            let before = counts(&meter);
            assert_eq!(
                receiver.apply(&mut meter, 1),
                Err(receiver.exhausted_error()),
                "{trigger:?} -> {receiver:?}"
            );
            assert_eq!(counts(&meter), before, "{trigger:?} -> {receiver:?}");
        }
    }
}

#[test]
fn zero_sized_charges_do_not_reopen_an_exhausted_ledger() {
    for trigger in Charge::ALL {
        for receiver in Charge::BULK {
            let mut meter = Meter::new(trigger.limits());
            trigger.apply(&mut meter, 1).unwrap_err();
            let before = counts(&meter);
            assert_eq!(
                receiver.apply(&mut meter, 0),
                Err(receiver.exhausted_error()),
                "{trigger:?} -> {receiver:?}"
            );
            assert!(!meter.charge(0));
            assert_eq!(counts(&meter), before);
        }
    }
}

#[test]
fn cleanup_and_diagnostic_accounting_remain_available_without_reopening_admission() {
    let mut meter = Meter::new(Limits::DEFAULT.with_rdate_entries(0));
    meter.enter().unwrap();
    meter.try_enter_element().unwrap();
    meter.try_bind_prefix().unwrap();
    meter.try_charge_candidate().unwrap();
    assert_eq!(
        meter.try_charge_rdate_entries(1),
        Err(LimitExceeded::RdateEntries)
    );
    let spent = meter.spent();
    meter.leave();
    meter.leave_element();
    meter.unbind_prefix();
    meter.open_period();
    meter.note_dropped_diagnostic();
    assert_eq!(meter.depth(), 0);
    assert_eq!(meter.live_prefix_bindings(), 0);
    assert_eq!(meter.candidates_in_period(), 0);
    assert_eq!(meter.diagnostics_dropped(), 1);
    assert_eq!(meter.spent(), spent);
    assert!(meter.is_exhausted());
    for receiver in Charge::ALL {
        assert_eq!(
            receiver.apply(&mut meter, 0),
            Err(receiver.exhausted_error())
        );
    }
}

#[test]
fn candidate_admission_is_atomic_when_its_aggregate_charge_fails() {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_input_bytes(0));
    assert_eq!(meter.try_charge_candidate(), Err(LimitExceeded::Budget));
    assert_eq!(meter.candidates_in_period(), 0);
    assert_eq!(meter.candidates_charged(), 0);
    assert_eq!(meter.try_charge_candidate(), Err(LimitExceeded::Exhausted));
    assert_eq!(meter.candidates_in_period(), 0);
}

const CALENDAR: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit exhaustion//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Test/First\r\nBEGIN:STANDARD\r\nDTSTART:20240101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nRDATE:20240102T000000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VTIMEZONE\r\nTZID:Test/Second\r\nBEGIN:STANDARD\r\nDTSTART:20240103T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0200\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:exhaustion@example.test\r\nDTSTAMP:20240101T000000Z\r\nDTSTART;TZID=Test/Second:20240104T090000\r\nDTEND;TZID=Absent:20240104T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn zones(limits: Limits) -> (VtimezoneSet, Meter, Vec<Diagnostic>) {
    let document = Document::parse(CALENDAR, Limits::DEFAULT, &mut Vec::new()).unwrap();
    let mut meter = Meter::new(limits);
    let mut diagnostics = Vec::new();
    let zones = read_calendar_zones(
        document.components().next().unwrap(),
        &mut meter,
        &mut diagnostics,
    );
    (zones, meter, diagnostics)
}

#[test]
fn reserved_zone_finalization_keeps_the_admitted_partial_table() {
    let (zones, meter, diagnostics) = zones(Limits::DEFAULT.with_max_vtimezone_observances(1));
    assert_eq!(zones.tables().len(), 1);
    let first = zones.table("Test/First").unwrap();
    assert_eq!(first.observances().len(), 1);
    assert!(first.is_truncated());
    assert_eq!(meter.vtimezone_observances(), 1);
    assert_eq!(meter.vtimezone_components(), 1);
    assert!(meter.is_exhausted());
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code())
            .collect::<Vec<_>>(),
        vec![
            DiagnosticCode::VtimezoneObservancesTruncated,
            DiagnosticCode::ResourceBudgetExhausted,
            DiagnosticCode::MissingTimeZoneDefinition
        ]
    );
    let missing: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() == DiagnosticCode::MissingTimeZoneDefinition)
        .collect();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].subject().unwrap().as_bytes(), b"Absent");
}

#[test]
fn a_refused_zone_reservation_does_not_charge_its_observances() {
    let (zones, meter, diagnostics) = zones(Limits::DEFAULT.with_max_vtimezone_components(0));
    assert_eq!(zones.tables(), []);
    assert_eq!(meter.vtimezone_observances(), 0);
    assert_eq!(meter.vtimezone_components(), 0);
    assert!(meter.is_exhausted());
    assert_eq!(
        diagnostics[0].code(),
        DiagnosticCode::VtimezoneComponentsTruncated
    );
    assert_eq!(
        diagnostics[1].code(),
        DiagnosticCode::ResourceBudgetExhausted
    );
}

fn exhausted() -> Meter {
    let mut meter = Meter::new(Limits::DEFAULT.with_max_input_bytes(0));
    assert!(!meter.charge(1));
    meter
}

#[test]
fn already_exhausted_table_admission_does_not_report_a_local_observance_limit() {
    let start = CivilDateTime::new(
        CivilDate::from_ymd(2024, 1, 1).unwrap(),
        CivilTime::MIDNIGHT,
    );
    let observation = Observance::new(start, UtcOffset::UTC, UtcOffset::UTC, false, None);
    let mut meter = exhausted();
    let mut diagnostics = Vec::new();
    let table = TransitionTable::new(
        "Test/Blocked".into(),
        vec![observation],
        &mut meter,
        &mut diagnostics,
    );
    assert_eq!(table.observances(), []);
    assert!(table.is_truncated());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        DiagnosticCode::ResourceBudgetExhausted
    );
}

#[test]
fn direct_zone_insertion_returns_the_unadmitted_table_with_an_exhausted_error() {
    let mut build = Meter::new(Limits::DEFAULT);
    let table = TransitionTable::new(
        "Test/Blocked".into(),
        Vec::new(),
        &mut build,
        &mut Vec::new(),
    );
    let mut set = VtimezoneSet::new();
    let error = set.insert(table, &mut exhausted()).unwrap_err();
    assert!(matches!(error, ZoneSetError::Exhausted(_)));
    assert_eq!(error.table().tzid().as_str(), "Test/Blocked");
    assert_eq!(
        error.diagnostic_code(),
        DiagnosticCode::ResourceBudgetExhausted
    );
    assert_eq!(set.tables(), []);
}

#[test]
fn empty_override_admission_preserves_shared_exhaustion_as_its_error() {
    let error = OverrideSet::new(&[], &mut exhausted()).unwrap_err();
    assert_eq!(error, InputError::Exhausted(InputList::Override));
}
