---
date: 2026-11-18
status: proposed
---

# Borrow normalized namespace values

## Context

[Namespaces in XML section 2.3](https://www.w3.org/TR/xml-names/#s-ComparingURIReferences) defines namespace identity after attribute normalization.
Events borrow input independently of later reader calls.
Repeated comparisons can decode padded references for each lookup.

## Decision

Borrow checked namespace values.
Factories exclude recognized vocabularies and the XML namespace from foreign names.
Known-name lookups compare classifications.
Reuse equality only for identical sources and normalization modes.
Use the shared decoder for distinct values and output.
Charge source widths before attribute comparisons and normalized storage before ownership.

## Alternatives

Partial vocabulary normalization gives foreign names inconsistent identities.
Owning event names repeats allocations.
Reader-owned buffers invalidate borrowed event lifetimes.

## Consequences

Distinct foreign spellings and output can still require decoding.
Reconsider if decoding dominates measured workloads outside metered comparison boundaries.

## Confirmation

Test vocabulary recognition, extension round trips, duplicate attributes, scope restoration, invalid bindings, and wire admission.
Check ordering, hashing, and allocation across equivalent spellings.
Measure normalized source widths and reject per-lookup decoding after binding admission.
Run maintained gates.
