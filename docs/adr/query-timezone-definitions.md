---
date: 2026-11-16
status: proposed
---

# Query time zone definitions

## Context

[RFC 4791 section 9.8](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.8) requires the supplied definition for dates and floating times.
Keeping only its identifier lets external rules change the result.
The [zone source decision](0003-caller-supplied-time-zones.md) keeps zone data in caller-provided sources.

## Decision

Keep a validated, bounded definition with each query that supplies one.
Reuse the maintained observance reader and transition resolver.
Route dates and floating values to the query definition.
Route values carrying an explicit `TZID` to the caller's zone source.
Reject malformed or unsupported query definitions without an implicit fallback.

## Alternatives

Identifier-only routing requires external data and can ignore the supplied rules.
Using the inline definition for every matching identifier also changes values carrying an explicit zone.
Combining both readings adds a policy choice where the protocol already selects the source.

## Consequences

Query parsing keeps bounded transition data and can refuse valid rules outside the supported vocabulary.
Zone data stays in caller-provided input.
Revisit support when the observance reader gains other rule forms.

## Confirmation

Tests must cover absent and conflicting external sources, clock changes, query operations, and resource limits.
The maintained checks must pass with default features and without default features.
