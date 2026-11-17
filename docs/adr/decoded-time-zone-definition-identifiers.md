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
Keep parameter quoting at the reference boundary.
Refuse unreadable definition identities.
Apply decoded identities to lookup, duplicates, and refused-definition tracking.

## Alternatives

Raw comparison preserves incorrect matches.
Registering both spellings invents aliases and makes distinct definitions ambiguous.

## Consequences

Escaped identifiers need decoded storage.
Quoted and unquoted property values identify different zones.
Opaque identifiers no longer provide query time zones.
Reconsider if a governing specification changes identifier encoding.

## Confirmation

Regressions cover escapes, literal quotes, references, duplicates, limits, opacity, and byte preservation.
Public query matching and projection exercise the reader.
The shared ADR gate and maintained project checks must pass.
