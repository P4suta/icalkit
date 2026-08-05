# ADR-0003: the time zone source is supplied by the caller and named explicitly

- Status: accepted
- Date: 2026-08-05

## Context

A `.ics` file carries its own `VTIMEZONE` components: transition rules, written down at the
time the file was created, for every zone it references. It also carries `TZID` strings that
usually — but not always — match IANA identifiers.

These two sources disagree. A calendar written in 2018 has 2018's rules for a zone whose
government has since changed them. Which answer is correct depends entirely on the
question:

- *What did the organizer mean when they scheduled this?* — the embedded `VTIMEZONE`.
- *What time will this actually happen?* — today's IANA database.
- *What does the server think?* — whatever it was configured with.

Bundling a time zone database inside the library forces one of those answers and makes it
invisible. It also freezes tzdata into a crate release, so a government changing DST rules
becomes a dependency upgrade, and it makes the crate large and non-`no_std`.

## Decision

This workspace bundles no time zone data and reads no system clock.

Zone resolution goes through a caller-supplied source. The caller decides whether that is
the file's own `VTIMEZONE` definitions, an IANA database it already has, or a combination —
and the combination is expressed as an explicit policy, not a fallback chain buried in the
implementation.

Where the two sources disagree about a given instant, that is a reported fact available to
the caller, not something resolved silently. A client that wants to warn "this event was
scheduled under different DST rules" can; a client that does not care ignores it.

Every result says which source produced it.

## Consequences

Callers must provide something. For most that is one line wiring in the tzdb crate they
already depend on, and for `no_std` and WebAssembly targets it is what makes the library
usable at all.

The library never becomes wrong because tzdata moved, because it has no opinion about
tzdata.

Ambiguous and non-existent local times — the hour that repeats and the hour that does not
exist at a DST transition — are represented as such rather than silently resolved. They are
real states in a calendar and the caller has to be able to see them, which is also why the
workspace denies `clippy::arithmetic_side_effects`: every time computation states what it
does when the answer does not exist.
