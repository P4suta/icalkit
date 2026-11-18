---
date: 2026-11-18
status: proposed
---

# Decoded time zone definition identifiers

## Context

[RFC 5545 section 3.8.3.1](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.3.1) defines `TZID` properties as `TEXT`.
[Text escaping](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11) and [parameter quoting](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2) have different meanings.
Raw comparison misses escaped definitions and merges names containing literal quotes.

## Decision

Register definition identifiers through the shared scalar text decoder.
Decode once and preserve literal quotes, case, prefixes, and stored bytes.
Remove surrounding parameter quotes before decoding [RFC 6868 caret escapes](https://www.rfc-editor.org/rfc/rfc6868.html#section-3).
Store the decoded bytes beside the wire value only for references with caret escapes.
Date-time views borrow the identifier throughout the parameter's lifetime.
Charge the decoding buffer before allocation.
Apply decoded identities to lookup, duplicates, and refused-definition tracking.

## Alternatives

Registering both spellings invents aliases and makes distinct definitions ambiguous.
Decoding on lookup would require owned date-time views or decoding at every consumer.

## Consequences

Replacing a parameter constructs its decoded value from the replacement spelling.
Quoted and unquoted property values identify different zones.
Opaque identifiers no longer provide query time zones.
Reconsider if a governing specification changes identifier encoding.

## Confirmation

Regressions cover escapes, literal quotes, references, duplicates, limits, opacity, and byte preservation.
Public query matching and projection exercise the reader.
