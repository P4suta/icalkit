// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recurrence: turning `RRULE`, `RDATE`, and `EXDATE` into the occurrences a caller asked
//! for.
//!
//! Specification: RFC 5545 section 3.8.5 and the recurrence rule value type in section
//! 3.3.10 <https://www.rfc-editor.org/rfc/rfc5545#section-3.3.10>.
//!
//! An `RRULE` describes a series, and that series is usually infinite: a rule with neither
//! `COUNT` nor `UNTIL` never ends, and `FREQ=SECONDLY` is legal. There is therefore no
//! function here that expands a rule into a collection. Expansion is a lazy iterator over a
//! window the caller states, and nothing outside that window is computed, so an unbounded
//! rule is not a problem — the iterator is finite because the window is (see
//! `docs/adr/0002`).
//!
//! The search is bounded a second time, independently of the window. Some rules match
//! rarely: `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` sends a naive generator walking years
//! between hits. Each search spends a budget of candidate instants, and exhausting it is a
//! reported outcome — this rule found no match within the search limit — rather than a hang
//! or a silently empty result. The budget has a finite default, so a caller processing a
//! hostile file is protected before it knows the failure mode exists.
//!
//! Exceptions and overrides are applied inside the iterator, never by the caller filtering
//! afterwards. `EXDATE` removes instances, `RDATE` adds them, and a `RECURRENCE-ID`
//! component replaces one — possibly moving it out of the window it was generated in, or
//! into one it would never have been generated in. Deduplicating an `RDATE` that coincides
//! with a rule instance, matching an `EXDATE` whose value type differs from `DTSTART`, and
//! placing a moved override are exactly the points where implementations quietly disagree,
//! and a caller asked to reconcile them will get them wrong.
//!
//! Comparing an `UNTIL` in UTC against a `DTSTART` that is floating or zoned needs a time
//! zone, and the zone comes from the caller through `ical-tz`. Nothing here bundles a time
//! zone database or reads a clock, which is also why "is this occurrence in the past" is a
//! question the caller answers by passing in the instant it means.
//!
//! # Status
//!
//! Bootstrap. Nothing is implemented yet; see `ROADMAP.md` (M1).

#![no_std]
