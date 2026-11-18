---
date: 2026-11-18
status: proposed
---

# Keep property content in event order

## Context

The [protocol decision](0004-sans-io-protocol-layer.md) leaves mixed content support open.
[RFC 4918 section 4.3](https://www.rfc-editor.org/rfc/rfc4918.html#section-4.3) permits mixed property content and requires servers to preserve dead property children.
The [XML Information Set](https://www.w3.org/TR/xml-infoset/#infoitem.element) orders children in document order.

## Decision

Represent captured content as resource claims, text runs, or a serialized fragment.
Start the fragment at the first child and append events in order.
Meter shared escaping chunks before buffer growth.
Preserve source prefixes and [namespace scope](https://www.w3.org/TR/xml-names/#scoping) for opaque values.
Fragment roots combine inherited bindings with envelope bindings and give the source precedence.
Emit nested declarations on their original elements.
Keep existing modeled value interpretation.

## Alternatives

Dropping markup loses permitted content.
Eager serialization inflates text-only values and can reject admitted text widths.
Inferring QName values requires the extension's schema.

## Consequences

Mixed fragments consume the width of escaped text and namespace declarations.
The prefix limit covers enclosing and fragment declarations.
Text-only values keep their existing width policy and payload witnesses.
Reconsider when callers require incremental fragment delivery.

## Confirmation

Test text positions, whitespace, references, comments, and escaped element syntax through complete responses.
Check modeled values, text-only controls, refusal precedence, and peak allocations.
Run maintained gates.
