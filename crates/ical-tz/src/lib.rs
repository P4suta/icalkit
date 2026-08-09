// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Time zones: `VTIMEZONE` interpreted against a source the caller supplies.
//!
//! Specification: RFC 5545 section 3.6.5, the `VTIMEZONE` component
//! <https://www.rfc-editor.org/rfc/rfc5545#section-3.6.5>.
//!
//! A calendar carries its own time zone definitions: `STANDARD` and `DAYLIGHT`
//! subcomponents with offsets and transition rules, written down when the file was written.
//! It also carries `TZID` strings that usually, but not always, name an IANA zone — Windows
//! zone names such as `W. Europe Standard Time` and prefixed identifiers such as
//! `/mozilla.org/20050126_1/Europe/Berlin` are both common in the wild.
//!
//! The embedded rules and today's IANA database legitimately disagree. A calendar written
//! in 2018 carries 2018's rules for a zone whose government has since changed them, and
//! which answer is right depends on the question being asked: *what did the organizer mean
//! when they scheduled this* is the embedded `VTIMEZONE`, *what time will this actually
//! happen* is the current database, and *what does the server think* is whatever it was
//! configured with. This crate therefore prefers neither. Resolution goes through a policy
//! the caller states, every result names the source that produced it, and a disagreement
//! about a given instant is a fact the caller can inspect rather than something settled out
//! of sight (see `docs/adr/0003`).
//!
//! No time zone data is bundled and no clock is read. That keeps the crate small and
//! `no_std`, and it means the library never becomes wrong because tzdata moved: it has no
//! opinion about tzdata. The cost is that a caller must supply something, which for most is
//! one line wiring in the database they already depend on.
//!
//! The awkward local times are values here, not errors. When a zone falls back, an hour
//! repeats and a local time has two instants; when it springs forward, an hour does not
//! exist and a local time has none. Real calendars contain events scheduled at 02:30 on a
//! spring-forward morning, and picking one interpretation silently is how a meeting appears
//! to move by an hour for one participant and not another.
//!
//! What this crate owns is resolution, not the types it resolves into. `CivilDate`,
//! `CivilDateTime`, `UtcOffset` and `Instant` belong to `ical-core` and below, because
//! `ical-recur` is a sibling of this crate and `ical-dav` names an instant without depending
//! on it at all; they are re-exported here so a caller still names one crate for one concept
//! (see `docs/adr/0011`). Every operation on them is checked, and no `Duration` carries years
//! or months, because RFC 5545's `DURATION` grammar has no designator for either.
//!
//! # Status
//!
//! Bootstrap. Nothing is implemented yet; see `ROADMAP.md` (M2). The public surface is
//! designed and compiled; `docs/design/ical-tz-api.md` carries it.

#![no_std]

extern crate alloc;
