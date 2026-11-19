// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The fixed 5,000-resource measurement from ADR 0012.
//!
//! Sweep A checks how many resources exhaust a fresh 16 MiB meter and records each resource's work counters.
//! Sweep B reports how far a shared 16 MiB meter gets through the same population.

use std::fmt::Write as _;

use icalkit_conformance::internal::core::{
    Document, IgnoreDiagnostics, Instant, Limits, Meter, ParseError, UtcOffset,
};
use icalkit_conformance::internal::dav::{CompFilter, DavError, PropFilter, TextMatch, TimeRange};
use icalkit_conformance::internal::query::{self, Budget, Match, QueryError, Undecided, Zones};
use icalkit_conformance::internal::tz::FixedOffsetSource;

const RESOURCES: usize = 5_000;
const DEFAULT_BUDGET: u64 = 16_777_216;

#[derive(Debug)]
struct Measurement {
    resource: usize,
    spent: u64,
    candidates: u64,
    outcome: Result<Match, QueryError>,
    exhausted: bool,
}

fn query_filter() -> Result<CompFilter, DavError> {
    let limits = Limits::DEFAULT;
    let mut scratch = Meter::new(limits);
    let mut summary = PropFilter::new(b"SUMMARY", limits, &mut scratch)?;
    summary.text_match = Some(TextMatch::new(b"meeting", &mut scratch)?);

    let mut event = CompFilter::new(b"VEVENT", limits, &mut scratch)?;
    event.time_range = Some(TimeRange::new(
        Some(Instant::from_unix_seconds(1_772_323_200)),
        Some(Instant::from_unix_seconds(1_775_001_600)),
    )?);
    event.push_prop(summary, &mut scratch)?;

    let mut root = CompFilter::new(b"VCALENDAR", limits, &mut scratch)?;
    root.push_comp(event, limits, &mut scratch)?;
    Ok(root)
}

macro_rules! line {
    ($text:expr, $($arguments:tt)*) => {
        assert!(
            writeln!($text, $($arguments)*).is_ok(),
            "writing one generated line to a String cannot fail"
        );
    };
}

fn generated_resource(index: usize) -> Result<Document, ParseError> {
    let mut text = String::with_capacity(320);
    line!(text, "BEGIN:VCALENDAR\r");
    line!(text, "VERSION:2.0\r");
    line!(text, "PRODID:-//icalkit query scale//EN\r");
    line!(text, "BEGIN:VEVENT\r");
    line!(text, "UID:scale-{index}@example.test\r");
    line!(text, "DTSTAMP:20260101T000000Z\r");

    match index % 10 {
        // Every tenth resource exercises the rare-match, multi-decade rule from ADR 0012.
        0 => {
            line!(text, "DTSTART:19000101T090000Z\r");
            line!(text, "DURATION:PT1H\r");
            line!(text, "RRULE:FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1\r");
            line!(text, "SUMMARY:meeting archive\r");
        },
        // A bounded recurring series crossing the requested month.
        1 => {
            line!(text, "DTSTART:20260215T090000Z\r");
            line!(text, "DURATION:PT1H\r");
            line!(text, "RRULE:FREQ=DAILY;COUNT=60\r");
            line!(text, "SUMMARY:planning meeting\r");
        },
        // A matching non-recurring event.
        2..=5 => {
            let day = index
                .checked_rem(27)
                .and_then(|remainder| remainder.checked_add(1))
                .unwrap_or(1);
            line!(text, "DTSTART:202603{day:02}T090000Z\r");
            line!(text, "DURATION:PT1H\r");
            line!(text, "SUMMARY:team meeting\r");
        },
        // Inside the window but rejected by the property filter.
        6..=7 => {
            let day = index
                .checked_rem(27)
                .and_then(|remainder| remainder.checked_add(1))
                .unwrap_or(1);
            line!(text, "DTSTART:202603{day:02}T090000Z\r");
            line!(text, "DURATION:PT1H\r");
            line!(text, "SUMMARY:focus time\r");
        },
        // Expansion-free exclusion before and after the requested month.
        8 => {
            line!(text, "DTSTART:20200101T090000Z\r");
            line!(text, "DURATION:PT1H\r");
            line!(text, "SUMMARY:old meeting\r");
        },
        _ => {
            line!(text, "DTSTART:20300101T090000Z\r");
            line!(text, "DURATION:PT1H\r");
            line!(text, "SUMMARY:future meeting\r");
        },
    }

    line!(text, "END:VEVENT\r");
    line!(text, "END:VCALENDAR\r");
    Document::parse(text.as_bytes(), Limits::DEFAULT, &mut IgnoreDiagnostics)
}

fn exhausted(outcome: Result<Match, QueryError>, meter: &Meter) -> bool {
    meter.is_exhausted()
        || matches!(
            outcome,
            Ok(Match::Undecided(Undecided::SearchExhausted)) | Err(QueryError::Limit(_))
        )
}

fn resources() -> impl Iterator<Item = Result<Document, ParseError>> {
    (0..RESOURCES).map(generated_resource)
}

#[test]
fn five_thousand_resource_query_stays_below_the_fixed_exhaustion_threshold()
-> Result<(), Box<dyn std::error::Error>> {
    let filter = query_filter()?;
    let utc_source = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let zones = Zones::new(&utc_source);

    let mut measurements = Vec::with_capacity(RESOURCES);
    for (resource, calendar) in resources().enumerate() {
        let calendar = calendar?;
        let mut meter = Meter::with_budget(Limits::DEFAULT, DEFAULT_BUDGET);
        let outcome = {
            let mut budget = Budget::new(Limits::DEFAULT, &mut meter);
            query::matches(&filter, &calendar, zones, &mut budget)
        };
        measurements.push(Measurement {
            resource,
            spent: meter.spent(),
            candidates: meter.candidates_charged(),
            exhausted: exhausted(outcome, &meter),
            outcome,
        });
    }

    let exhausted_rows: Vec<_> = measurements.iter().filter(|row| row.exhausted).collect();
    let max_spent = measurements.iter().map(|row| row.spent).max().unwrap_or(0);
    let max_candidates = measurements
        .iter()
        .map(|row| row.candidates)
        .max()
        .unwrap_or(0);
    println!(
        "query scale sweep A: resources={RESOURCES}, policy=Limits::DEFAULT, budget={DEFAULT_BUDGET}, exhausted={}, max_spent={max_spent}, max_candidates={max_candidates}",
        exhausted_rows.len()
    );
    assert!(
        exhausted_rows.len() <= 5,
        "ADR 0012 clause 1 permits at most five exhausted resources; exhausted rows: {exhausted_rows:?}"
    );
    assert!(
        measurements.iter().all(|row| row.outcome.is_ok()),
        "every per-resource evaluation must produce an answer: {:?}",
        measurements
            .iter()
            .filter(|row| row.outcome.is_err())
            .collect::<Vec<_>>()
    );

    assert_eq!(measurements.len(), RESOURCES);
    assert_eq!(
        measurements.last().map(|row| row.resource),
        Some(RESOURCES - 1)
    );
    Ok(())
}

#[test]
fn aggregate_query_budget_reports_its_stopping_point() -> Result<(), Box<dyn std::error::Error>> {
    let filter = query_filter()?;
    let utc_source = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let zones = Zones::new(&utc_source);
    let mut aggregate = Meter::with_budget(Limits::DEFAULT, DEFAULT_BUDGET);
    let mut aggregate_stop = None;
    for (resource, calendar) in resources().enumerate() {
        let calendar = calendar?;
        let outcome = {
            let mut budget = Budget::new(Limits::DEFAULT, &mut aggregate);
            query::matches(&filter, &calendar, zones, &mut budget)
        };
        if exhausted(outcome, &aggregate) {
            aggregate_stop = Some((resource, outcome));
            break;
        }
    }
    println!(
        "query scale sweep B: policy=Limits::DEFAULT, budget={DEFAULT_BUDGET}, stopped={aggregate_stop:?}, spent={}, candidates={}",
        aggregate.spent(),
        aggregate.candidates_charged()
    );

    Ok(())
}
