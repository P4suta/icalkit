---
date: 2026-11-18
status: proposed
---

# Borrow normalized namespace values

## Context

[Namespaces in XML section 2.3](https://www.w3.org/TR/xml-names/#s-ComparingURIReferences) defines namespace identity after attribute normalization.
Current bindings compare raw declaration values.
Events borrow the body and remain independent of later reader calls.

## Decision

Keep checked, borrowed views of normalized declaration values.
Use one reference decoder for attribute normalization, namespace comparison, and output.
Compare expanded names by decoded octets and enforce reserved bindings in both directions.
Copy normalized names only when building owned values, charging shared limits before allocation.

## Alternatives

Recognizing encoded spellings only for known vocabularies leaves extension identity incorrect.
Owning each event name repeats namespace allocations.
Borrowing reader-owned decoded buffers would invalidate event lifetimes or require a different pull interface.

## Consequences

Encoded names need normalization during comparison and output.
Checked views preserve input borrowing without shared allocation ownership.
Element character-data widths remain separate from declaration values.
Reconsider when repeated normalization dominates measured workloads.

## Confirmation

Test vocabulary recognition, extension round trips, duplicate attributes, scope restoration, invalid declarations, and wire admission.
Check ordering and hashing across equivalent spellings and bounded allocation without copying names per event.
Run maintained gates.
