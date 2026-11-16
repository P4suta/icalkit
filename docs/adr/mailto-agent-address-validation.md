---
date: 2026-11-16
status: proposed
---

# Validate agent address syntax at calendar promotion

## Context

[RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.18) requires a quoted `mailto` URI for `SENT-BY`.
The generic URI codec checks [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986.html#section-3), which permits bodies outside the mail address grammar.

## Decision

Validate participant address parameters before calendar promotion.
Use a private, borrowed parser for [RFC 6068](https://www.rfc-editor.org/rfc/rfc6068.html#section-2) mailbox and header syntax.
Apply [verified erratum 7919](https://errata.rfc-editor.org/eid7919/) to mailbox sub-delimiters.
Keep the shared URI codec as its outer boundary.
The parser decodes each `pct-encoded` field once and checks UTF-8 and quoted local parts.
Validate parameter lists by their quoted fields.
Keep source bytes in storage.

## Alternatives

Generic URI checks alone admit invalid agent addresses.
An established parser could share maintenance but would add an external dependency.
Rejecting complex addresses would exclude the RFC's examples.

## Consequences

Calendar promotion refuses invalid agent metadata.
Imports keep their bytes.
Identity comparison and mail delivery remain separate operations.
The private grammar adds maintenance work when its sources change.

## Confirmation

Require public regressions for the RFC examples, malformed encodings, lists, component restrictions, and lossless promotion.
Reconsider when [reported erratum 4706](https://errata.rfc-editor.org/eid4706/) resolves quoted whitespace or a reviewed shared parser supplies the boundary.
