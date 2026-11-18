---
date: 2026-11-19
status: proposed
---

# Checkpoint adjusted rules with a cached origin

## Context

Calendar corrections can move a recurrence key across periods under [RFC 7529](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1).
A checkpoint needs the earliest period that can reproduce any pending key.
Scanning the remaining keys after each occurrence takes quadratic work.

## Decision

Keep the earliest pending period in the search.
The walk visits periods in order, and the merge removes the smallest key first.
When a new period extends the greatest pending key, mark the old greatest key with the new period.
Removing a marked key advances the earliest period.
An open offer keeps the prior checkpoint until the merge commits it.

## Alternatives

A second index could count keys per period but would need storage for each active period.
Reusing the existing map value keeps the same storage per key.

## Consequences

Capturing a checkpoint reads a fixed number of period indices.
Charge the session for finding the greatest buffered key during expansion.
Revisit the marks if the search starts removing keys out of order.

## Confirmation

Capture a checkpoint after every minute of a 30-day month.
Find the earliest remaining period independently after each removal.
Check that a refused expansion can resume.
