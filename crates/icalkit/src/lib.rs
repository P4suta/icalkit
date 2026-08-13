// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The single public entry point for iCalendar, recurrence, scheduling and CalDAV.

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
