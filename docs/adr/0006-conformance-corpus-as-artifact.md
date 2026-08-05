# ADR-0006: the conformance and interoperability corpus is a deliverable

- Status: accepted
- Date: 2026-08-05

## Context

RFC 5545 is a large specification, and calendaring interoperability is decided less by it
than by what Google Calendar, Microsoft 365, and Apple Calendar actually emit and accept.
Those three disagree with the RFC and with each other, in ways that are folklore: everyone
who has implemented this knows some of them, and nobody has written them down where a
machine can check.

A test suite organized around our own types would encode our reading of the specification
and be useless to anyone else. It would also be unable to express the interesting
statement, which is not "we parse this correctly" but "these four implementations disagree
about this input, and here is what each does".

## Decision

`ical-conform` is a published crate rather than a `tests/` directory.

Cases are addressed to specification sections and evaluated against a trait, so another
implementation can run the identical suite. This workspace supplies one implementation of
that trait.

The corpus is real. Calendars exported from real clients are committed verbatim and
round-tripped byte-for-byte, which is what makes the fidelity claim in
[ADR 0001](0001-lossless-round-trip.md) verifiable rather than asserted. Files are reduced
to the smallest form that still shows the behavior and stripped of personal data before
being committed; a case records which client and version produced the original.

Where implementations diverge, the case records every observed behavior and says which one
this project chose and why. Where the RFC permits alternatives, all permitted outcomes are
recorded rather than one being canonized.

## Consequences

Publishing disagreements is more useful to the ecosystem than a green suite that hides
them. A case saying "Microsoft 365 emits this, the RFC forbids it, we accept it on read and
never emit it" is documentation that does not currently exist anywhere.

Every rule needs a case before it is considered implemented. That slows the first milestone
and pays back from the second, because the suite becomes the specification the
implementation is written against rather than a description of what it happens to do.

Committing real exports means a privacy obligation. Reduction and anonymization are part of
accepting a case, not a cleanup pass, and a case that cannot be anonymized is not accepted.
