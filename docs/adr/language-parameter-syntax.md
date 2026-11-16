---
date: 2026-11-17
status: proposed
---

# Language parameter syntax

## Context

[RFC 5545 section 3.2.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.10) uses RFC 5646 language tags.
[Conformance classes](https://www.rfc-editor.org/rfc/rfc5646.html#section-2.2.9) distinguish syntax from registry validity at a particular date.
The calendar kernel preserves metadata without selecting locales.

## Decision

Check the language tag grammar, including grandfathered tags, and reject repeated variants and extension singletons without regard to case.
Keep input spelling and opaque extension data.
Check tag cardinality and the component restrictions for common names.
Charge tag scans and repeated variant comparisons to the shared resource budget.
Keep registry lookup and extension-specific interpretation outside the kernel.

## Alternatives

A bundled registry could establish dated subtag validity but would need updates and another data boundary.
Unchecked metadata preserves malformed tags and violates the declared syntax.

## Consequences

Syntax acceptance doesn't establish registry validity or a matching language.
Duplicate checks can exhaust the shared budget and must refuse promotion.
Revisit if a public workflow needs locale selection or dated registry validation.

## Confirmation

Conformance and workflow tests cover grammar, duplicates, byte preservation, opaque semantics, and resource refusal.
Maintained checks must pass.
