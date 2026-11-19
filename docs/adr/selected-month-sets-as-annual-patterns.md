---
date: 2026-11-17
status: proposed
---

# Compile selected month sets into annual patterns

## Context

[RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10) defines `BYMONTH` as a list of unsigned month numbers.
The embedded reader accepts only one month.
[Annual compilation](month-day-selectors-across-all-months.md) already bounds a rule to twelve stored patterns.

## Decision

Parse explicit months into a nonzero bit set with checked month numbers.
Normalize duplicate list values and reject malformed lists or repeated month parts as a whole.
Compile one annual pattern per selected month.
Keep explicit month presence distinct from omission, including a set containing all twelve months.
Numeric weekdays keep month scope when `BYMONTH` is present.
Preserve anchors, clocks, offsets, end bounds, explicit dates, and observance accounting.

## Alternatives

A vector also normalizes a set but allocates storage for twelve possible values.
Treating every-month selection as omission changes weekday ordinal scope.
Refusing lists preserves a specification gap.

## Consequences

Month sets need fixed storage and produce at most twelve annual patterns.
Source spellings remain unchanged.

## Confirmation

Compare every nonempty month set with independent calendar membership.
Check duplicates, ordering, syntax, default days, weekday scope, end bounds, resource limits, and public CalDAV behavior.
Reconsider for calendar scales with other month domains.
