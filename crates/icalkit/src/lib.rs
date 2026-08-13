// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The single public entry point for iCalendar, recurrence, scheduling and CalDAV.
//!
//! Input moves through an explicit typestate:
//!
//! ```text
//! bytes -> Import (lossless) -> explicit normalization -> Calendar (validated)
//!       -> recurrence / scheduling / CalDAV
//! ```
//!
//! Compatibility repair never runs implicitly. Use an [`Engine`] session when several
//! operations must share one aggregate resource budget or a caller-supplied time-zone database.
//!
//! # Strict parsing
//!
//! [`Calendar::parse`] is the secure-default shorthand. Standards errors prevent promotion;
//! notes and valid unknown extensions survive.
//!
//! ```
//! # const BYTES: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//example//EN\r\nBEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTAMP:20260814T000000Z\r\nSUMMARY:Planning\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
//! let calendar = icalkit::Calendar::parse(BYTES)?;
//! assert_eq!(calendar.events().next().unwrap().uid(), "one@example.test");
//! # Ok::<(), icalkit::Error>(())
//! ```
//!
//! # Explicit normalization
//!
//! [`interop::Import`] retains the admitted input unchanged. A versioned profile produces a
//! separate output and reports every repair.
//!
//! ```
//! use icalkit::interop::{Import, RfcRepairV1};
//!
//! let input = b"BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//example//EN\nEND:VCALENDAR\n";
//! let imported = Import::read(input)?;
//! let normalized = imported.normalize(RfcRepairV1)?;
//! assert_eq!(imported.as_bytes(), input);
//! assert!(!normalized.changes().is_empty());
//! let calendar = normalized.output().validate()?;
//! # Ok::<(), icalkit::Error>(())
//! ```
//!
//! # Transactional editing
//!
//! [`Calendar::edit`] changes a private copy. Drop rolls back; [`Editor::commit`] validates
//! before atomically replacing the calendar.
//!
//! ```
//! # const BYTES: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//example//EN\r\nBEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTAMP:20260814T000000Z\r\nSUMMARY:Planning\r\nX-COLOR:plum\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
//! let mut calendar = icalkit::Calendar::parse(BYTES)?;
//! let mut edit = calendar.edit();
//! edit.set_summary("one@example.test", "Revised")?;
//! edit.commit()?;
//! assert!(calendar.to_bytes().windows(12).any(|part| part == b"X-COLOR:plum"));
//! # Ok::<(), icalkit::Error>(())
//! ```
//!
//! # DST-aware recurrence
//!
//! Calendar-aware workflows resolve `TZID` through [`time::ZoneDatabase`], preserving
//! gap/fold, provenance, and coverage. The standalone recurrence API is a lazy absolute-time
//! stream with a mandatory half-open window and a fallible pull.
//!
//! ```
//! use icalkit::recurrence::{Rule, Window};
//! use icalkit::time::Timestamp;
//!
//! let start = Timestamp::constant(1_704_067_200, 0);
//! let end = Timestamp::constant(1_704_672_000, 0);
//! let window = Window::new(start, end).unwrap();
//! let engine = icalkit::Engine::default();
//! let mut session = engine.session();
//! let rule = Rule::parse("FREQ=DAILY;COUNT=3")?;
//! let mut occurrences = rule.occurrences(&mut session, start, window)?;
//! while let Some(occurrence) = occurrences.try_next()? {
//!     assert!(window.contains(occurrence.start()));
//! }
//! # Ok::<(), icalkit::Error>(())
//! ```
//!
//! # iTIP scheduling
//!
//! Inbound scheduling uses a borrowed read-review-authorize-apply capability. Outbound builders
//! require the caller's timestamp and never read a clock.
//!
//! ```
//! use icalkit::scheduling::Message;
//! use icalkit::time::Timestamp;
//!
//! # const PAYLOAD: &[u8] = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//example//EN\r\nBEGIN:VEVENT\r\nUID:one@example.test\r\nDTSTART:20260815T090000Z\r\nSUMMARY:Planning\r\nORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\nSEQUENCE:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
//! let message = Message::request(PAYLOAD, Timestamp::constant(1_786_656_000, 0))?;
//! assert_eq!(message.method(), "REQUEST");
//! # Ok::<(), icalkit::Error>(())
//! ```
//!
//! # CalDAV sync and server workflows
//!
//! CalDAV is sans-I/O. Client operations follow `next_request -> accept -> finish`; server
//! operations mirror them with `next_need -> supply -> finish`.
//!
//! ```
//! use icalkit::caldav::{Client, SyncToken};
//!
//! let token = SyncToken::new("data:,sync-1").unwrap();
//! let operation = Client::new().sync("/calendars/alice/work/", Some(&token))?;
//! let request = operation.next_request().unwrap();
//! assert_eq!(request.method(), "REPORT");
//! # Ok::<(), icalkit::Error>(())
//! ```

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

// Root-local dependency adapter for the XML layer, whose source is also compiled by the
// standalone layering gate and the private conformance helper. Keeping this spelling stable
// lets that layer name only its metering dependency and never its protocol parent.
extern crate self as ical_core;
#[allow(unused_imports)]
pub(crate) use crate::internal::core::{LimitExceeded, Limits, Meter};

mod calendar;
mod engine;
mod failure;
mod internal;
/// Explicit compatibility import and normalization.
pub mod interop;
/// Validated read-only calendar views.
pub mod model;
mod policy;
/// Jiff-based time values and the application zone database port.
pub mod time;

pub mod caldav;
pub mod recurrence;
pub mod scheduling;

pub use crate::calendar::{Calendar, Editor};
pub use crate::engine::{Engine, EngineBuilder, Session};
pub use crate::failure::{Error, Issue, IssueCode};
pub use crate::policy::ResourcePolicy;
