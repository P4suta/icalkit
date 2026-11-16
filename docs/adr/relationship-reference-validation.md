---
date: 2026-11-17
status: proposed
---

# Relationship reference validation

## Context

[RFC 9253 section 9.1](https://www.rfc-editor.org/rfc/rfc9253.html#section-9.1) adds URI references while keeping textual identifiers and hierarchical compatibility.
Its prose specifies `UID` for hierarchy, but its grammar keeps default and explicit `TEXT` forms.

## Decision

Treat default, `TEXT`, and `UID` forms as scalar text identifiers.
Accept URI values for the nine new relationship types.
Default, hierarchical, unknown, and [snooze](https://www.rfc-editor.org/rfc/rfc9074.html#section-7) relationships reference identifiers.
Unknown types use the [parent fallback](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.15).
Check one relationship token and one signed duration gap across known components.
Keep unknown value types opaque.

## Alternatives

Treating default as `UID` but rejecting explicit `TEXT` for hierarchy follows the type wording and breaks earlier explicit declarations.
Keeping text-only validation rejects new URI references and leaves metadata unchecked.

## Consequences

Legacy text syntax remains valid without rewriting input.
Duration gaps share the bounded codec with case-insensitive parameter designators.
Target lookup and relationship effects stay with callers.
Revisit if an erratum clarifies the legacy type wording or a new relationship changes identifier fallback.

## Confirmation

Conformance and public workflow tests cover syntax, contexts, preservation, and refusals.
Maintained checks must pass.
