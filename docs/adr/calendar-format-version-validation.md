---
date: 2026-11-17
status: proposed
---

# Validate calendar format versions

## Context

[RFC 5545 section 3.7.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.7.4) defines `2.0` and version ranges.
The maintained kernel interprets the RFC 5545 format.
Checking only property presence admits arbitrary declared formats.

## Decision

Check interpreted `VERSION` values at the shared enumeration boundary.
Admit `2.0` and the supported range `2.0;2.0`.
Reject other values and incompatible recognized value types before granting validated views.
Keep unknown value types opaque under the existing declaration contract.
Lossless imports preserve all admitted bytes.

## Alternatives

Accepting arbitrary text grants unsupported interpretation.
Token checks don't establish a supported format.
Accepting only `2.0` excludes the standard range form.
A registry lookup wouldn't provide a reader for another format.

## Consequences

Calendar format versions remain distinct from package releases.
Another format needs reader support and an updated supported vocabulary.
Revisit when that implementation exists.

## Confirmation

Strict promotion, imports, scheduling, and editing must check failures and preserve supported spellings.
Regression cases cover ranges, folding, value types, malformed bytes, and opaque content.
Maintained gates must pass.
