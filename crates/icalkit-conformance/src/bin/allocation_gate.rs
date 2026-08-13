// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Isolated peak-allocation acceptance gate.
//!
//! A global allocator can only measure one process coherently, so this is a dedicated binary
//! rather than a test sharing a runner with unrelated allocations. The production crate remains
//! `#![forbid(unsafe_code)]`; the unsafe implementation is confined to this unpublished
//! measurement helper and delegates every operation directly to `System`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use icalkit_conformance::internal::core::{Diagnostic, Document, Limits, Meter};
use icalkit_conformance::internal::dav::{
    DecodeContext, MultiStatus, MultiStatusReader, XmlPull, XmlReader,
};

const PROPERTIES: usize = 200_000;
const RESPONSES: usize = 20_000;
const XML_ELEMENTS: usize = 1 + RESPONSES * 3;
const COMMENT_BYTES: usize = 8 * 1024 * 1024;

// These are intentionally expressed per retained unit as well as relative to the input. A single
// peak/input multiple cannot calibrate `max_items` or `max_xml_elements`.
const MAX_RETAINED_BYTES_PER_PROPERTY: u64 = 384;
const MAX_PEAK_BYTES_PER_PROPERTY: u64 = 768;
const MAX_RETAINED_BYTES_PER_XML_ELEMENT: u64 = 192;
const MAX_PEAK_BYTES_PER_XML_ELEMENT: u64 = 384;
const MAX_COMMENT_PEAK_BYTES: u64 = 256 * 1024;

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
            // A realloc implementation may hold the old region until the new one exists. Count
            // that conservative transient peak before delegating; it can overestimate but never
            // hides a peak the gate is intended to cap.
            note_peak(
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
    note_peak(current);
}

fn release(bytes: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let bytes = as_u64(bytes);
    let _ = CURRENT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(bytes))
    });
}

fn note_peak(candidate: u64) {
    let _ = PEAK.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |peak| {
        (candidate > peak).then_some(candidate)
    });
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        Ok::<_, icalkit_conformance::internal::dav::DavError>((seen, diagnostics))
    });
    let events = events?;
    assert_eq!(events.0, 4);
    assert!(
        comment_allocation.peak <= MAX_COMMENT_PEAK_BYTES,
        "an {COMMENT_BYTES}-byte comment used {} peak bytes, ceiling {MAX_COMMENT_PEAK_BYTES}; {comment_allocation:?}",
        comment_allocation.peak
    );

    println!(
        "allocation gate: properties={{count:{PROPERTIES},input:{},retained:{},peak:{},allocated:{},retained_per_item:{property_retained},peak_per_item:{property_peak}}}; xml={{elements:{XML_ELEMENTS},input:{},retained:{},peak:{},allocated:{},retained_per_element:{xml_retained},peak_per_element:{xml_peak}}}; comment={{input:{},peak:{}}}",
        calendar_bytes.len(),
        property_allocation.retained,
        property_allocation.peak,
        property_allocation.allocated,
        multistatus_bytes.len(),
        xml_allocation.retained,
        xml_allocation.peak,
        xml_allocation.allocated,
        comment_bytes.len(),
        comment_allocation.peak,
    );
    Ok(())
}
