---
date: 2026-11-17
status: proposed
---

# Attachment format parameter syntax

## Context

[RFC 5545 section 3.2.8](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.8) imports the name grammar from [RFC 4288 section 4.2](https://www.rfc-editor.org/rfc/rfc4288.html#section-4.2).
[RFC 6838 section 4.2](https://www.rfc-editor.org/rfc/rfc6838.html#section-4.2) adds a first-character constraint for registrations.
The kernel reads preserved calendar data.

## Decision

Validate one unquoted type/subtype pair with 1 through 127 allowed ASCII characters per name.
Preserve case and non-standard names without registry lookup.
The registration recommendation of 64 characters doesn't govern archived input.
Keep unknown value types and component semantics opaque.

## Alternatives

Current registration syntax would reject names that the protocol's cited grammar permits.
`Content-Type` parsing accepts characters and parameters that `FMTTYPE` doesn't define.
Unchecked metadata admits malformed names.

## Consequences

Syntax acceptance doesn't establish registry membership or agreement with the attachment's content.
Validation scans stored parameter bytes.
Revisit if the calendar specification changes its grammar or the library starts creating media type names.

## Confirmation

Conformance and public workflow tests cover names, bounds, folding, preservation, and malformed data.
Maintained checks must pass.
