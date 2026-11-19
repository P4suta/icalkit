---
date: 2026-11-16
status: proposed
---

# Scheduling creation from absence

## Context

[RFC 5545 section 3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6) requires at least one component in an iCalendar object.
Scheduling creation tests use an empty `Calendar` because review requires held state.
[The public boundary](0013-unified-public-crate-and-explicit-interop.md) requires strict validation and explicit authorization.

## Decision

Add `Message::review_creation` and `Message::review_creation_in` for a destination that the caller has established is absent.
Represent absence in the private review target and reuse authorization capabilities.
After authorization, construct a validated `Calendar` without `METHOD`.

## Alternatives

An empty `Calendar` violates the strict boundary.
A placeholder component would add unrelated data to the destination.
`Option<&Calendar>` would centralize review but change existing calls.
A separate creation capability duplicates authorization stages.

## Consequences

Existing review methods keep their signatures.
The caller must enforce absence when writing to storage, including races after review.
Reconsider the target type if scheduling review must bind to a storage revision.

## Confirmation

Tests cover first-object creation, shared-session review, refusal of invalid creators and methods, and preservation of the message bytes.
The public API snapshots and the default and no-default test gates match the implementation.
