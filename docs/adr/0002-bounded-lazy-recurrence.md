# ADR-0002: recurrence is a bounded lazy iterator, never an eager expansion

- Status: accepted
- Date: 2026-08-05

## Context

`RRULE:FREQ=SECONDLY` is legal. So is a rule with no `COUNT` and no `UNTIL`, which
describes an infinite series. So is `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` — a rule whose
matching instances are rare enough that a naive generator iterates for a very long time
between hits.

An API shaped as "give me the occurrences" has to decide what that means for those inputs,
and the usual answers are all bad: allocate until something breaks, cap at an arbitrary
number and lie about completeness, or hang. The existing Rust crate in this space
documents a security warning about untrusted rules, which is the honest version of the
same problem.

The inputs here are untrusted in the ordinary case, not the exotic one. An `.ics` file
arrives as an email attachment or over CalDAV from a server the user does not control.

## Decision

Expansion is a lazy iterator over a caller-supplied window, and it is bounded twice.

The caller states the range it cares about. Nothing outside it is computed, so a rule with
no end is not a problem: the iterator is finite because the window is.

Independently, the search itself is bounded. A rule whose next match is far away consumes a
budget of candidate instants, and exhausting that budget is a reported outcome — "this rule
did not produce a match within the search limit" — not a hang and not a silent empty
result. The budget has a finite default and is part of the injected limits, so a caller
processing a hostile file does not have to know the failure mode in advance to be protected
from it.

Overrides and exceptions are applied inside the iterator, not by the caller filtering
afterwards. `EXDATE`, `RDATE`, and modified instances (`RECURRENCE-ID`) change which
occurrences exist, and a caller that has to reconcile them is a caller that will get it
wrong.

## Consequences

There is no "expand this rule" function that returns a `Vec`, and callers who want one
write it themselves with a window they chose. That is the intended friction.

Rendering a month view costs a month of computation regardless of how the rule is written,
which is the property a UI needs.

The budget is observable, which means the conformance suite can assert on it: a rule that
should find its next instance within N candidates is a testable claim, and a regression
that makes the search less efficient shows up as a limit breach rather than as a slow test.
