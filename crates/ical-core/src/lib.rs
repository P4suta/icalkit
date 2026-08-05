// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! iCalendar (RFC 5545): the content line grammar, the object model, and serialization.
//!
//! Specification: RFC 5545, "Internet Calendaring and Scheduling Core Object Specification
//! (iCalendar)" <https://www.rfc-editor.org/rfc/rfc5545>.
//!
//! An `.ics` file is a tree of components — `VCALENDAR` wrapping `VEVENT`, `VTODO`,
//! `VJOURNAL`, `VFREEBUSY`, `VTIMEZONE` — built out of content lines, each a property name,
//! its parameters, and a value, folded at octet boundaries and escaped by rules that are
//! close enough to other formats to invite mistakes. This crate turns those bytes into a
//! model and writes the model back out. It expands no recurrence, resolves no `TZID`, and
//! attaches no meaning to `METHOD`; those live in the crates above it.
//!
//! The model preserves everything it read (see `docs/adr/0001`). Vendor properties,
//! parameters on properties that are otherwise understood, components with no type here,
//! and the original text of values that are not interpreted all stay in position and in
//! order, and serialization writes them back byte for byte. Typed access is a *view* over
//! that preserved text, never the storage behind it: reading `DTSTART` parses on demand and
//! leaves what the writer wrote intact, which also settles cases where a value cannot be
//! reproduced from its parsed form. Discarding the parts a parser does not recognize is how
//! one client silently destroys another client's data, and it is the failure this crate
//! exists to make structurally impossible.
//!
//! Calendars in the wild violate the specification constantly, so a violation is a
//! diagnostic attached to the item it concerns rather than an error that throws the file
//! away. A caller that wants strictness reads the diagnostics; a caller that wants to show
//! the user their meeting still can.
//!
//! Input is hostile in the ordinary case, not the exotic one — an `.ics` arrives as a mail
//! attachment or over CalDAV from a server the user does not control. Nothing here is sized
//! from a length found in the input without checking it against the caller's limits and the
//! bytes actually present.
//!
//! # Status
//!
//! Bootstrap. Nothing is implemented yet; see `ROADMAP.md` (M0).

#![no_std]
