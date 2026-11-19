---
date: 2026-11-16
status: proposed
---

# Decoded optional calendar identifiers

## Context

`UID` uses [`TEXT`](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.7) with [escapes](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11), while [unknown types](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20) stay opaque.
Public reads, editing, and recurrence master selection use stored spellings.
[ADR-0013](0013-unified-public-crate-and-explicit-interop.md) permits API changes before publication.

## Decision

`EventRef::uid` returns `Option<Cow<'a, str>>`.
Known text decodes once.
Ordinary text uses [borrowed storage](https://doc.rust-lang.org/alloc/borrow/enum.Cow.html).
Opaque values return `None`.
Property views preserve raw bytes.
Editors and recurrence selectors compare decoded identifiers.
Editors require one matching event.
Unreadable identities prevent selection.

## Alternatives

Adding a decoded accessor preserves compatibility but leaves two identity meanings.
Caching decoded strings adds derived state and still needs optional results for opaque values.
Owned strings need heap storage for ordinary reads.

## Consequences

Update consumers and both API snapshots together.
Escaped reads need heap storage.
Refusing unreadable or ambiguous selections is project policy.
Shared recurrence identifiers no longer select the first event for editing.
Reconsider when instance-aware editing needs a component key.

## Confirmation

Tests cover escapes, opacity, case, spaces, ambiguous edits, rollback, and recurrence resume.
The shared ADR gate and maintained checks must pass.
