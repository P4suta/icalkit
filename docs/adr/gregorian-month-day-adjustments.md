---
date: 2026-11-17
status: proposed
---

# Adjust Gregorian month days before weekday selection

## Context

[RFC 7529 section 4.1](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1) applies day adjustments after `BYMONTHDAY` and before `BYDAY`.
Corrected candidates can overlap adjacent periods.
The [calendar control gate](recurrence-calendar-controls-before-expansion.md) rejects every non-`OMIT` rule.

## Decision

Support Gregorian `BACKWARD` and `FORWARD` for default dates and `BYMONTHDAY`.
Keep rejecting non-`OMIT` rules with `BYYEARDAY`, `BYWEEKNO`, or leap month selectors.
Keep calendar corrections separate from local time admission.
Merge corrected period candidates using the next period's lower bound before counting.
Keep originating period positions for continuation.

## Alternatives

Keeping the gate rejects leap day anniversaries that need no calendar conversion.
Sorting each period alone misorders corrected times and counts duplicates.
Converting every calendar adds a separate dependency and negotiation decision.

## Consequences

The iterator keeps pending candidates and charges their expansion to the existing budget.
Original calendar bytes remain intact.
The supported subset needs no calendar conversion dependency.

## Confirmation

Require regressions for correction order, overlapping periods, limits, budgets, and continuation.
Reconsider the subset when year day, week number, leap month, or CalDAV negotiation support changes.
