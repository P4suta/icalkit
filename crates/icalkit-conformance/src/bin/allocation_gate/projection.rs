// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::error::Error;

use icalkit_conformance::internal::core::{
    Component, Document, IgnoreDiagnostics, Instant, LimitExceeded, Limits, Meter, UtcOffset,
};
use icalkit_conformance::internal::dav::TimeRange;
use icalkit_conformance::internal::query::{
    Budget, QueryError, Reduction, Selection, Zones, expand_calendar, limit_freebusy_set, select,
    without_values,
};
use icalkit_conformance::internal::tz::FixedOffsetSource;

use super::{as_u64, measure};

const LIMITS: Limits = Limits::DEFAULT;

pub(super) fn check() -> Result<(), Box<dyn Error>> {
    payload_copies()?;
    refused_field_copy()?;
    refused_fold_copy()
}

fn selection(input: &str) -> Result<Selection, Box<dyn Error>> {
    let parsed = Document::parse(input.as_bytes(), LIMITS, &mut IgnoreDiagnostics)?;
    Ok(Selection::new(parsed, Reduction::FAITHFUL))
}

fn payload_copies() -> Result<(), Box<dyn Error>> {
    let window = TimeRange::new(
        Some(Instant::from_unix_seconds(1_704_067_200)),
        Some(Instant::from_unix_seconds(1_704_153_600)),
    )?;
    let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let costs = |bytes| -> Result<_, Box<dyn Error>> {
        let payload = "x".repeat(bytes);
        let freebusy = selection(&format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nFREEBUSY:{payload}\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n"
        ))?;
        let (limited, limited_alloc) = measure(|| {
            limit_freebusy_set(
                &freebusy,
                window,
                &mut Budget::new(LIMITS, &mut Meter::with_budget(LIMITS, 64)),
            )
        });
        let (stripped, stripped_alloc) = measure(|| {
            without_values(
                &freebusy,
                &[b"FREEBUSY"],
                &mut Budget::new(LIMITS, &mut Meter::with_budget(LIMITS, 64)),
            )
        });
        assert_eq!(
            limited.err(),
            Some(QueryError::Limit(LimitExceeded::Budget))
        );
        assert_eq!(
            stripped.err(),
            Some(QueryError::Limit(LimitExceeded::Budget))
        );
        let (kept, kept_alloc) = measure(|| {
            limit_freebusy_set(
                &freebusy,
                window,
                &mut Budget::new(LIMITS, &mut Meter::new(LIMITS)),
            )
        });
        assert_eq!(kept?.calendar(), freebusy.calendar());
        let event = selection(&format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:one\r\nDTSTART;X-EXTRA={payload}:20240101T090000Z\r\n\
             RRULE:FREQ=DAILY;COUNT=1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        ))?;
        let (instance, instance_alloc) = measure(|| {
            expand_calendar(
                &event,
                window,
                Zones::new(&utc),
                &mut Budget::new(LIMITS, &mut Meter::new(LIMITS)),
            )
        });
        let instance = instance?;
        let parameter = instance
            .calendar()
            .components()
            .flat_map(Component::components)
            .flat_map(Component::properties)
            .find(|line| line.is_named(b"DTSTART"))
            .and_then(|start| start.parameters_named(b"X-EXTRA").next());
        assert_eq!(
            parameter.map(|parameter| parameter.value().as_bytes()),
            Some(payload.as_bytes())
        );
        Ok([
            limited_alloc.allocated,
            stripped_alloc.allocated,
            kept_alloc.allocated,
            instance_alloc.allocated,
        ])
    };
    let short = costs(1024)?;
    let long = costs(2048)?;
    let expected_growth = [0, 0, 1024, 1024];
    for ((long, short), growth) in long.into_iter().zip(short).zip(expected_growth) {
        assert_eq!(
            long.checked_sub(short),
            Some(growth),
            "projection allocation growth"
        );
    }
    Ok(())
}

fn refused_field_copy() -> Result<(), Box<dyn Error>> {
    let window = TimeRange::new(Some(Instant::from_unix_seconds(1_704_067_200)), None)?;
    for omitted_first in [false, true] {
        let copy = |bytes| -> Result<_, Box<dyn Error>> {
            let payload = "x".repeat(bytes);
            let outside = "20230101T000000Z/PT1H";
            let fields = if omitted_first {
                format!("{outside},{payload}")
            } else {
                format!("{payload},{outside}")
            };
            let input = format!(
                "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nFREEBUSY:{fields}\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n"
            );
            let source = selection(&input)?;
            let mut document = Meter::new(LIMITS);
            select(&source, None, &mut Budget::new(LIMITS, &mut document))?;
            let scan = as_u64(b"FREEBUSY".len().saturating_add(fields.len()));
            let mut meter = Meter::with_budget(LIMITS, document.spent().saturating_add(scan));
            let (result, allocation) = measure(|| {
                limit_freebusy_set(&source, window, &mut Budget::new(LIMITS, &mut meter))
            });
            assert_eq!(result.err(), Some(QueryError::Limit(LimitExceeded::Budget)));
            Ok(allocation.allocated)
        };
        let short = copy(1024)?;
        let long = copy(2048)?;
        assert_eq!(
            long.checked_sub(short),
            Some(1024),
            "FREEBUSY field copy after budget exhaustion"
        );
    }
    Ok(())
}

fn refused_fold_copy() -> Result<(), Box<dyn Error>> {
    for (before, after) in [
        ("BEGIN:", "VCALENDAR\r\nSUMMARY:x\r\nEND:VCALENDAR"),
        ("BEGIN:VCALENDAR\r\nSUMMARY:", "x\r\nEND:VCALENDAR"),
        ("BEGIN:VCALENDAR\r\nSUMMARY:x\r\nEND:", "VCALENDAR"),
    ] {
        let copy = |count| -> Result<_, Box<dyn Error>> {
            let wire = format!("{before}{}{after}\r\n", "\r\n ".repeat(count));
            let source = selection(&wire)?;
            let mut meter = Meter::with_budget(LIMITS, 96);
            let (result, allocation) = measure(|| {
                without_values(&source, &[b"SUMMARY"], &mut Budget::new(LIMITS, &mut meter))
            });
            let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
            let window = TimeRange::new(Some(Instant::from_unix_seconds(1_704_067_200)), None)?;
            let mut expand_meter = Meter::with_budget(LIMITS, 96);
            let (expanded, expansion) = measure(|| {
                expand_calendar(
                    &source,
                    window,
                    Zones::new(&utc),
                    &mut Budget::new(LIMITS, &mut expand_meter),
                )
            });
            Ok((
                [result.err(), expanded.err()],
                [allocation.allocated, expansion.allocated],
            ))
        };
        let (short, short_bytes) = copy(32)?;
        let (long, long_bytes) = copy(64)?;
        assert_eq!(
            long_bytes, short_bytes,
            "fold layout allocation for {before:?}"
        );
        assert_eq!(short, [Some(QueryError::Limit(LimitExceeded::Budget)); 2]);
        assert_eq!(long, short);
    }
    Ok(())
}
