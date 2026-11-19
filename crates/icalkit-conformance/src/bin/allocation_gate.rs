// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Isolated peak-allocation acceptance gate.
//!
//! A global allocator can only measure one process coherently, so this is a dedicated binary rather than a test sharing a runner with unrelated allocations.
//! The production crate remains `#![forbid(unsafe_code)]`; the unsafe implementation is confined to this unpublished measurement helper and delegates every operation directly to `System`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use icalkit::recurrence::Rule;
use icalkit_conformance::internal::core::{
    CivilDate, CivilDateTime, CivilTime, Diagnostic, DiagnosticCode, Document, IgnoreDiagnostics,
    LimitExceeded, Limits, Meter, UtcOffset,
};
use icalkit_conformance::internal::dav::{
    DavError, DecodeContext, Href, MultiStatus, MultiStatusReader, PropValue, XmlPull, XmlReader,
};
use icalkit_conformance::internal::tz::{Observance, TransitionTable, read_calendar_zones};

#[path = "allocation_gate/arithmetic.rs"]
mod arithmetic;
#[path = "allocation_gate/projection.rs"]
mod projection;

const PROPERTIES: usize = 200_000;
const RESPONSES: usize = 20_000;
const XML_ELEMENTS: usize = 1 + RESPONSES * 3;
const COMMENT_BYTES: usize = 8 * 1024 * 1024;

// These are intentionally expressed per retained unit as well as relative to the input.
// A single peak/input multiple cannot calibrate `max_items` or `max_xml_elements`.
const MAX_RETAINED_BYTES_PER_PROPERTY: u64 = 384;
const MAX_PEAK_BYTES_PER_PROPERTY: u64 = 768;
const MAX_RETAINED_BYTES_PER_XML_ELEMENT: u64 = 192;
const MAX_PEAK_BYTES_PER_XML_ELEMENT: u64 = 384;
const MAX_COMMENT_PEAK_BYTES: u64 = 256 * 1024;
// A resource refusal keeps a fixed error, so its heap ceiling is independent of input size.
const MAX_REFUSED_RULE_BYTES: u64 = 1_024;
const RDATE_OBSERVANCES: usize = 10_000;
const MAX_OBSERVANCE_STAGING_BYTES: u64 = 64 * 1024;
const MAX_CLOCK_PRODUCT_BYTES: u64 = 256 * 1024;
const TEXT_PROPERTY_BYTES: usize = 256 * 1024;
const FRAGMENT_TEXT_BYTES: usize = 24 * 1024;
const MAX_REFUSED_FRAGMENT_BYTES: u64 = 64 * 1024;
const NAMESPACE_CHARACTERS: usize = 32 * 1024;
const NAMESPACE_CHILDREN: u32 = 64;
const MAX_NAMESPACE_PEAK_BYTES: u64 = 16 * 1024;

struct CountingAllocator;

static ENABLED: AtomicBool = AtomicBool::new(false);
static CURRENT: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this implementation is a transparent accounting wrapper around the process
        // allocator and forwards the layout unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            charge(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this implementation is a transparent accounting wrapper around the process
        // allocator and forwards the layout unchanged.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            charge(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        release(layout.size());
        // SAFETY: `pointer` and `layout` are exactly the pair supplied to this allocator.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        let measured = ENABLED.load(Ordering::Relaxed);
        if measured {
            // A realloc implementation may hold the old region until the new one exists.
            // Count that conservative transient peak before delegating; it can overestimate but never hides a peak the gate is intended to cap.
            note_peak(
                &PEAK,
                CURRENT
                    .load(Ordering::Relaxed)
                    .saturating_add(as_u64(new_size)),
            );
        }
        // SAFETY: this implementation forwards the allocation and unchanged old layout to the
        // process allocator, with the caller's requested new size.
        let resized = unsafe { System.realloc(pointer, old, new_size) };
        if measured && !resized.is_null() {
            release(old.size());
            charge(new_size);
        }
        resized
    }
}

fn as_u64(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

fn charge(bytes: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let bytes = as_u64(bytes);
    ALLOCATED.fetch_add(bytes, Ordering::Relaxed);
    let current = CURRENT
        .fetch_add(bytes, Ordering::Relaxed)
        .saturating_add(bytes);
    note_peak(&PEAK, current);
}

fn release(bytes: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    release_counter(&CURRENT, as_u64(bytes));
}

fn release_counter(counter: &AtomicU64, bytes: u64) {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = arithmetic::released_count(current, bytes);
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn note_peak(counter: &AtomicU64, candidate: u64) {
    counter.fetch_max(candidate, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug)]
struct Allocation {
    retained: u64,
    peak: u64,
    allocated: u64,
}

fn measure<T>(operation: impl FnOnce() -> T) -> (T, Allocation) {
    CURRENT.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    ALLOCATED.store(0, Ordering::Relaxed);
    assert!(
        !ENABLED.swap(true, Ordering::SeqCst),
        "allocation measurements must not overlap"
    );
    let value = operation();
    let allocation = Allocation {
        retained: CURRENT.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
        allocated: ALLOCATED.load(Ordering::Relaxed),
    };
    ENABLED.store(false, Ordering::SeqCst);
    (value, allocation)
}

fn calendar_input() -> Vec<u8> {
    let mut body = Vec::with_capacity(PROPERTIES.saturating_mul(5).saturating_add(128));
    body.extend_from_slice(
        b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit allocation gate//EN\r\n",
    );
    for _ in 0..PROPERTIES {
        body.extend_from_slice(b"X:1\r\n");
    }
    body.extend_from_slice(b"END:VCALENDAR\r\n");
    body
}

fn multistatus_input() -> Vec<u8> {
    let mut body = Vec::with_capacity(RESPONSES.saturating_mul(112).saturating_add(128));
    body.extend_from_slice(
        br#"<?xml version="1.0" encoding="UTF-8"?><D:multistatus xmlns:D="DAV:">"#,
    );
    for index in 0..RESPONSES {
        body.extend_from_slice(b"<D:response><D:href>/cal/");
        body.extend_from_slice(index.to_string().as_bytes());
        body.extend_from_slice(b"</D:href><D:status>HTTP/1.1 200 OK</D:status></D:response>");
    }
    body.extend_from_slice(b"</D:multistatus>");
    body
}

fn comment_input() -> Vec<u8> {
    let mut body = Vec::with_capacity(COMMENT_BYTES.saturating_add(128));
    body.extend_from_slice(br#"<D:propfind xmlns:D="DAV:"><!--"#);
    body.resize(body.len().saturating_add(COMMENT_BYTES), b'x');
    body.extend_from_slice(b"--><D:allprop/></D:propfind>");
    body
}

fn property_input(text_bytes: usize, nested: bool) -> Vec<u8> {
    let mut body = br#"<D:multistatus xmlns:D="DAV:" xmlns:X="urn:example:values"><D:response><D:href>/one</D:href><D:propstat><D:prop><X:custom>"#.to_vec();
    if nested {
        body.extend_from_slice(b"<X:child>");
    }
    body.resize(body.len().saturating_add(text_bytes), b'>');
    if nested {
        body.extend_from_slice(b"</X:child>");
    }
    body.extend_from_slice(b"</X:custom></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>");
    body
}

fn read_properties(body: &[u8], limits: Limits) -> Result<MultiStatus, DavError> {
    let mut meter = Meter::new(limits);
    let mut diagnostics = IgnoreDiagnostics;
    let mut events = XmlReader::new(body);
    let mut source = MultiStatusReader::new(&mut events);
    MultiStatus::read(
        &mut source,
        &mut DecodeContext::new(limits, &mut meter, &mut diagnostics),
    )
}

fn check_property_content() -> Result<(), Box<dyn std::error::Error>> {
    let plain = property_input(TEXT_PROPERTY_BYTES, false);
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(u32::try_from(TEXT_PROPERTY_BYTES)?);
    let (read, plain_allocation) = measure(|| read_properties(&plain, limits));
    let read = read?;
    let PropValue::Unmodeled(text) = &read.responses()[0].propstats()[0].props()[0].value else {
        return Err("text-only property lost its character data shape".into());
    };
    assert_eq!(text.len(), TEXT_PROPERTY_BYTES);
    assert!(text.iter().all(|byte| *byte == b'>'));
    assert!(
        plain_allocation.peak <= as_u64(TEXT_PROPERTY_BYTES).saturating_add(64 * 1024),
        "text-only content created redundant storage: {plain_allocation:?}"
    );
    drop(read);

    let nested = property_input(FRAGMENT_TEXT_BYTES, true);
    let limits = Limits::DEFAULT.with_max_xml_text_bytes(32 * 1024);
    let (refused, fragment_allocation) = measure(|| read_properties(&nested, limits));
    assert_eq!(refused, Err(DavError::Limit(LimitExceeded::Text)));
    assert!(
        fragment_allocation.peak <= MAX_REFUSED_FRAGMENT_BYTES,
        "escaped fragment grew before its width refusal: {fragment_allocation:?}"
    );
    println!(
        "property content allocation: text_only_peak={}, refused_fragment_peak={}",
        plain_allocation.peak, fragment_allocation.peak
    );
    Ok(())
}

fn oversized_rule_input() -> Result<String, Box<dyn std::error::Error>> {
    let part = ";X-NOTE=1";
    let repetitions = usize::try_from(Limits::DEFAULT.max_value_bytes())?
        .checked_div(part.len())
        .and_then(|count| count.checked_add(1))
        .ok_or("rule fixture size overflow")?;
    Ok(format!("FREQ=DAILY{}", part.repeat(repetitions)))
}

fn check_rule_refusal() -> Result<(usize, Allocation), Box<dyn std::error::Error>> {
    let input = oversized_rule_input()?;
    let (refused, allocation) = measure(|| Rule::parse(&input));
    assert!(
        allocation.peak <= MAX_REFUSED_RULE_BYTES && allocation.allocated <= MAX_REFUSED_RULE_BYTES,
        "rule refusal exceeded {MAX_REFUSED_RULE_BYTES} bytes: {allocation:?}"
    );
    let error = refused.err().ok_or("accepted oversize rule input")?;
    assert_eq!(error.code().as_str(), "icalkit.recurrence.resource-limit");
    Ok((input.len(), allocation))
}

fn check_instance_copy_refusal() -> Result<(), Box<dyn std::error::Error>> {
    use icalkit_conformance::internal::core::Instant;
    use icalkit_conformance::internal::dav::TimeRange;
    use icalkit_conformance::internal::query::{
        Budget, QueryError, Reduction, Selection, Zones, expand_calendar,
    };
    use icalkit_conformance::internal::tz::FixedOffsetSource;

    let limits = Limits::DEFAULT.with_max_component_depth(2);
    let utc = FixedOffsetSource::new("UTC", UtcOffset::UTC, false);
    let window = TimeRange::new(
        Some(Instant::from_unix_seconds(1_704_067_200)),
        Some(Instant::from_unix_seconds(1_704_326_400)),
    )?;
    let cost = |bytes| -> Result<Allocation, Box<dyn std::error::Error>> {
        let description = "x".repeat(bytes);
        let input = format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:one\r\nDTSTART:20240101T090000Z\r\n\
             RRULE:FREQ=DAILY;COUNT=3\r\nDESCRIPTION:{description}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let document = Document::parse(input.as_bytes(), limits, &mut IgnoreDiagnostics)?;
        let source = Selection::new(document, Reduction::FAITHFUL);
        let mut meter = Meter::with_budget(limits, 1024);
        let (result, allocation) = measure(|| {
            expand_calendar(
                &source,
                window,
                Zones::new(&utc),
                &mut Budget::new(limits, &mut meter),
            )
        });
        assert_eq!(result.err(), Some(QueryError::Limit(LimitExceeded::Budget)));
        // Each daily period contributes a date, a time and their combination.
        assert_eq!(meter.candidates_charged(), 9);
        assert_eq!(meter.occurrences(), 3);
        Ok(allocation)
    };
    let short = cost(2048)?;
    let long = cost(4096)?;
    assert_eq!(long.allocated, short.allocated, "{long:?} and {short:?}");
    Ok(())
}

fn check_observance_staging() -> Result<(usize, Allocation), Box<dyn std::error::Error>> {
    let mut input = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit staging gate//EN\r\n\
         BEGIN:VTIMEZONE\r\nTZID:Test/Staging\r\nBEGIN:STANDARD\r\n\
         DTSTART:20240101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n\
         RDATE:",
    );
    for index in 0..RDATE_OBSERVANCES {
        if index != 0 {
            input.push(',');
            if index.is_multiple_of(4) {
                input.push_str("\r\n ");
            }
        }
        let hour = index / 3_600;
        let minute = index / 60 % 60;
        let second = index % 60;
        write!(input, "20240102T{hour:02}{minute:02}{second:02}")?;
    }
    input.push_str("\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n");
    let document = Document::parse(input.as_bytes(), Limits::GENEROUS, &mut Vec::new())?;
    let root = document
        .components()
        .next()
        .ok_or("missing staging fixture root")?;
    let ((zones, meter, diagnostics), allocation) = measure(|| {
        let mut meter = Meter::new(Limits::DEFAULT.with_max_vtimezone_observances(1));
        let mut diagnostics = Vec::new();
        let zones = read_calendar_zones(root, &mut meter, &mut diagnostics);
        (zones, meter, diagnostics)
    });
    assert!(
        allocation.peak <= MAX_OBSERVANCE_STAGING_BYTES
            && allocation.retained <= MAX_OBSERVANCE_STAGING_BYTES,
        "observance staging exceeded {MAX_OBSERVANCE_STAGING_BYTES} bytes with a one-observance budget: {allocation:?}"
    );
    let table = zones.tables().first().ok_or("missing staged zone")?;
    assert_eq!(table.observances().len(), 1);
    assert_eq!(
        table.observances()[0].start(),
        CivilDateTime::new(
            CivilDate::from_ymd(2024, 1, 1).ok_or("invalid staging anchor")?,
            CivilTime::MIDNIGHT,
        )
    );
    assert!(table.is_truncated());
    assert_eq!(meter.vtimezone_observances(), 1);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        DiagnosticCode::VtimezoneObservancesTruncated
    );
    Ok((input.len(), allocation))
}

fn check_transition_allocations()
-> Result<(usize, Allocation, Allocation), Box<dyn std::error::Error>> {
    let (input, staging) = check_observance_staging()?;
    let supplied = check_supplied_observance_retention()?;
    report_clock_product()?;
    Ok((input, staging, supplied))
}

fn report_clock_product() -> Result<(), Box<dyn std::error::Error>> {
    let allocation = check_clock_product("")?;
    println!(
        "allocation gate: clock_product={{retained:{},peak:{}}}",
        allocation.retained, allocation.peak
    );
    let positions = (-366_i16..=-1)
        .chain(1..=366)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let selected = check_clock_product(&format!(";BYSETPOS={positions}"))?;
    println!(
        "allocation gate: selected_clock_product={{retained:{},peak:{}}}",
        selected.retained, selected.peak
    );
    Ok(())
}

fn check_clock_product(selection: &str) -> Result<Allocation, Box<dyn std::error::Error>> {
    let list = |end: u8| {
        (0..=end)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let rule = format!(
        "RRULE:FREQ=YEARLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR={};BYMINUTE={};BYSECOND={}{selection}",
        list(23),
        list(59),
        list(60)
    );
    let rule = rule
        .as_bytes()
        .chunks(74)
        .map(std::str::from_utf8)
        .collect::<Result<Vec<_>, _>>()?
        .join("\r\n ");
    let input = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalkit clocks gate//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Test/Clocks\r\nBEGIN:STANDARD\r\nDTSTART:20000101T000000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\n{rule}\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    );
    let document = Document::parse(input.as_bytes(), Limits::GENEROUS, &mut Vec::new())?;
    let root = document
        .components()
        .next()
        .ok_or("missing clock fixture root")?;
    let ((zones, diagnostics), allocation) = measure(|| {
        let mut meter = Meter::new(Limits::DEFAULT.with_max_vtimezone_observances(1));
        let mut diagnostics = Vec::new();
        let zones = read_calendar_zones(root, &mut meter, &mut diagnostics);
        (zones, diagnostics)
    });
    assert!(
        allocation.peak <= MAX_CLOCK_PRODUCT_BYTES,
        "clock product exceeded {MAX_CLOCK_PRODUCT_BYTES} bytes: {allocation:?}"
    );
    assert_eq!(zones.tables()[0].observances().len(), 1);
    assert!(zones.tables()[0].is_truncated());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        DiagnosticCode::VtimezoneObservancesTruncated
    );
    Ok(allocation)
}

fn check_supplied_observance_retention() -> Result<Allocation, Box<dyn std::error::Error>> {
    let start = CivilDateTime::new(
        CivilDate::from_ymd(2024, 1, 1).ok_or("invalid fixed observance date")?,
        CivilTime::MIDNIGHT,
    );
    let observance = Observance::new(start, UtcOffset::UTC, UtcOffset::UTC, false, None);
    let ((table, meter, diagnostics), allocation) = measure(|| {
        let offered = vec![observance; RDATE_OBSERVANCES];
        let mut meter = Meter::new(Limits::DEFAULT.with_max_vtimezone_observances(1));
        let mut diagnostics = Vec::new();
        let table = TransitionTable::new(
            "Test/Supplied".into(),
            offered,
            &mut meter,
            &mut diagnostics,
        );
        (table, meter, diagnostics)
    });
    assert!(
        allocation.retained <= MAX_OBSERVANCE_STAGING_BYTES,
        "a supplied observance vector retained more than {MAX_OBSERVANCE_STAGING_BYTES} bytes with a one-observance budget: {allocation:?}"
    );
    assert_eq!(table.observances().len(), 1);
    assert!(table.is_truncated());
    assert_eq!(meter.vtimezone_observances(), 1);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        DiagnosticCode::VtimezoneObservancesTruncated
    );
    Ok(allocation)
}

fn check_comments() -> Result<(usize, Allocation), Box<dyn std::error::Error>> {
    let comment_bytes = comment_input();
    let (events, comment_allocation) = measure(|| {
        let limits = Limits::GENEROUS;
        let mut meter = Meter::new(limits);
        let mut diagnostics = Vec::<Diagnostic>::new();
        let mut reader = XmlReader::new(&comment_bytes);
        let mut context = DecodeContext::new(limits, &mut meter, &mut diagnostics);
        let mut seen = 0_u32;
        while reader.next_event(&mut context)?.is_some() {
            seen = seen.saturating_add(1);
        }
        Ok::<_, DavError>((seen, diagnostics))
    });
    let events = events?;
    assert_eq!(events.0, 4);
    assert!(
        comment_allocation.peak <= MAX_COMMENT_PEAK_BYTES,
        "an {COMMENT_BYTES}-byte comment used {} peak bytes, ceiling {MAX_COMMENT_PEAK_BYTES}; {comment_allocation:?}",
        comment_allocation.peak
    );
    Ok((comment_bytes.len(), comment_allocation))
}

fn check_namespace_allocations() -> Result<(), Box<dyn std::error::Error>> {
    let mut body = b"<x:root xmlns:x=\"urn:example:".to_vec();
    for _ in 0..NAMESPACE_CHARACTERS {
        body.extend_from_slice(b"&#97;");
    }
    body.extend_from_slice(b"\">");
    for _ in 0..NAMESPACE_CHILDREN {
        body.extend_from_slice(b"<x:child/>");
    }
    body.extend_from_slice(b"</x:root>");
    let (seen, allocation) = measure(|| {
        let limits = Limits::DEFAULT;
        let mut meter = Meter::new(limits);
        let mut diagnostics = IgnoreDiagnostics;
        let mut context = DecodeContext::new(limits, &mut meter, &mut diagnostics);
        let mut reader = XmlReader::new(&body);
        let mut seen = 0_u32;
        while reader.next_event(&mut context)?.is_some() {
            seen = seen.saturating_add(1);
        }
        Ok::<_, DavError>(seen)
    });
    assert_eq!(
        seen?,
        NAMESPACE_CHILDREN.saturating_add(1).saturating_mul(2)
    );
    assert!(
        allocation.peak <= MAX_NAMESPACE_PEAK_BYTES,
        "namespace events copied decoded names: {allocation:?}"
    );
    println!(
        "namespace normalization allocation: peak={}, allocated={}",
        allocation.peak, allocation.allocated
    );
    Ok(())
}

fn check_protocol_value_copies() -> Result<(), Box<dyn std::error::Error>> {
    for bytes in [b"/".as_slice(), b"/short", b"/longer/href"] {
        let limits = Limits::DEFAULT;
        let (value, allocation) = measure(|| Href::new(bytes, limits, &mut Meter::new(limits)));
        assert_eq!(value?.as_bytes(), bytes);
        assert_eq!(allocation.retained, as_u64(bytes.len()), "{allocation:?}");
        assert_eq!(
            allocation.allocated,
            as_u64(bytes.len()),
            "boxing must not resize: {allocation:?}"
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    check_protocol_value_copies()?;
    let calendar_bytes = calendar_input();
    let (calendar, property_allocation) = measure(|| {
        let limits = Limits::GENEROUS;
        let mut diagnostics = Vec::<Diagnostic>::new();
        Document::parse(&calendar_bytes, limits, &mut diagnostics)
            .map(|document| (document, diagnostics))
    });
    let calendar = calendar?;
    let property_retained = property_allocation
        .retained
        .checked_div(as_u64(PROPERTIES))
        .unwrap_or(u64::MAX);
    let property_peak = property_allocation
        .peak
        .checked_div(as_u64(PROPERTIES))
        .unwrap_or(u64::MAX);
    assert!(
        property_retained <= MAX_RETAINED_BYTES_PER_PROPERTY,
        "property retention is {property_retained} bytes/item, ceiling {MAX_RETAINED_BYTES_PER_PROPERTY}; {property_allocation:?}"
    );
    assert!(
        property_peak <= MAX_PEAK_BYTES_PER_PROPERTY,
        "property peak is {property_peak} bytes/item, ceiling {MAX_PEAK_BYTES_PER_PROPERTY}; {property_allocation:?}"
    );
    drop(calendar);

    let multistatus_bytes = multistatus_input();
    let (multistatus, xml_allocation) = measure(|| {
        let limits = Limits::GENEROUS;
        let mut meter = Meter::new(limits);
        let mut diagnostics = Vec::<Diagnostic>::new();
        let mut events = XmlReader::new(&multistatus_bytes);
        let mut source = MultiStatusReader::new(&mut events);
        let mut context = DecodeContext::new(limits, &mut meter, &mut diagnostics);
        MultiStatus::read(&mut source, &mut context).map(|value| {
            assert_eq!(value.responses().len(), RESPONSES);
            (value, diagnostics)
        })
    });
    let multistatus = multistatus?;
    let xml_retained = xml_allocation
        .retained
        .checked_div(as_u64(XML_ELEMENTS))
        .unwrap_or(u64::MAX);
    let xml_peak = xml_allocation
        .peak
        .checked_div(as_u64(XML_ELEMENTS))
        .unwrap_or(u64::MAX);
    assert!(
        xml_retained <= MAX_RETAINED_BYTES_PER_XML_ELEMENT,
        "XML retention is {xml_retained} bytes/element, ceiling {MAX_RETAINED_BYTES_PER_XML_ELEMENT}; {xml_allocation:?}"
    );
    assert!(
        xml_peak <= MAX_PEAK_BYTES_PER_XML_ELEMENT,
        "XML peak is {xml_peak} bytes/element, ceiling {MAX_PEAK_BYTES_PER_XML_ELEMENT}; {xml_allocation:?}"
    );
    drop(multistatus);

    check_instance_copy_refusal()?;
    projection::check()?;
    let (comment_input_bytes, comment_allocation) = check_comments()?;
    check_property_content()?;
    check_namespace_allocations()?;
    let (rule_input_bytes, rule_allocation) = check_rule_refusal()?;
    let (staging_input_bytes, staging_allocation, supplied_allocation) =
        check_transition_allocations()?;

    println!(
        "allocation gate: properties={{count:{PROPERTIES},input:{},retained:{},peak:{},allocated:{},retained_per_item:{property_retained},peak_per_item:{property_peak}}}; xml={{elements:{XML_ELEMENTS},input:{},retained:{},peak:{},allocated:{},retained_per_element:{xml_retained},peak_per_element:{xml_peak}}}; comment={{input:{},peak:{}}}; rule_refusal={{input:{},peak:{},allocated:{}}}; observance_staging={{rdates:{RDATE_OBSERVANCES},input:{},retained:{},peak:{}}}; supplied_observances={{offered:{RDATE_OBSERVANCES},retained:{}}}",
        calendar_bytes.len(),
        property_allocation.retained,
        property_allocation.peak,
        property_allocation.allocated,
        multistatus_bytes.len(),
        xml_allocation.retained,
        xml_allocation.peak,
        xml_allocation.allocated,
        comment_input_bytes,
        comment_allocation.peak,
        rule_input_bytes,
        rule_allocation.peak,
        rule_allocation.allocated,
        staging_input_bytes,
        staging_allocation.retained,
        staging_allocation.peak,
        supplied_allocation.retained,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{arithmetic::released_count, note_peak, release_counter};

    #[test]
    fn releases_preserve_zero_exact_and_saturated_boundaries() {
        for (current, bytes, expected) in [
            (0, 0, 0),
            (0, 1, 0),
            (1, 0, 1),
            (1, 1, 0),
            (1, 2, 0),
            (u64::MAX, 0, u64::MAX),
            (u64::MAX, u64::MAX, 0),
            (u64::MAX, u64::MAX - 1, 1),
        ] {
            assert_eq!(released_count(current, bytes), expected);
        }
    }

    #[test]
    fn release_counter_preserves_zero_exact_and_saturated_boundaries() {
        for (current, bytes, expected) in [
            (0, 0, 0),
            (0, 1, 0),
            (1, 0, 1),
            (1, 1, 0),
            (1, 2, 0),
            (u64::MAX, 0, u64::MAX),
            (u64::MAX, u64::MAX, 0),
            (u64::MAX, u64::MAX - 1, 1),
        ] {
            let counter = AtomicU64::new(current);
            release_counter(&counter, bytes);
            assert_eq!(counter.load(Ordering::Relaxed), expected);
        }
    }

    #[test]
    fn concurrent_releases_preserve_both_updates() {
        let counter = AtomicU64::new(16);
        std::thread::scope(|threads| {
            threads.spawn(|| release_counter(&counter, 8));
            threads.spawn(|| release_counter(&counter, 8));
        });
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn peak_counter_never_decreases() {
        let counter = AtomicU64::new(1);
        for (candidate, expected) in [(0, 1), (1, 1), (2, 2), (u64::MAX, u64::MAX)] {
            note_peak(&counter, candidate);
            assert_eq!(counter.load(Ordering::Relaxed), expected);
        }
    }
}
