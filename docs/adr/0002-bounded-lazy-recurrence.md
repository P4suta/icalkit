# ADR-0002: recurrence is a bounded lazy iterator, never an eager expansion

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10

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

`RecurrenceSearch<'a>` implements `core::iter::Iterator`, and its `Item` is a crate-owned
`#[non_exhaustive] #[must_use] enum SearchStep { Occurrence(Occurrence),
BudgetExhausted(BudgetExhausted) }` — never `Result<Occurrence, BudgetExhausted>`. The reason is
mechanical, not stylistic. std's `impl<T, E> IntoIterator for Result<T, E>` makes
`search.flatten()` compile on a `Result` item and silently discard every terminal marker, and
`Result::ok` / `Result::is_ok` make `.filter_map(Result::ok)` and `.take_while(Result::is_ok)` do
the same; each is a reviewed-without-comment one-liner that converts budget exhaustion back into
the truncated-but-plausible answer this ADR exists to prevent, which is worse than the empty
result named above.

The claim is visibility, not impossibility, and the text claims only that. An explicit
`filter_map` that matches `SearchStep::Occurrence` and drops the rest discards the terminal step
just as thoroughly, and the opt-in `results()` adapter — which yields `Result` items so a caller
can write `let occ = step?;` — puts the original hazard one deliberate call away. What changed is
that the discard is now a visible line in a diff instead of an idiom. `Iterator::count()` is the
honest remainder: it counts steps, not occurrences, so an exhausted search returns a number
inflated by its terminal step. That is a documented hazard with a regression test, not a wrong
number the type system removes. These gates also bind this repository's own callers only:
ical-recur is a published `no_std` library and cannot lint a downstream crate, so the guarantee's
real boundary is the crate edge, not compile-time inevitability.

`RecurrenceSearch` is `FusedIterator`: calling `next()` after a terminal `BudgetExhausted`, or
after the window is exhausted, is defined and yields `None`. Resume is a `SearchCursor {
resume_after: Instant, occurrences_emitted: u32, rule_cursor: RuleCursorState }` with
`RuleCursorState` opaque — never a bare instant — so resuming a `COUNT`-bounded rule reproduces
the recurrence set RFC 5545 defines relative to `DTSTART`. Budget is charged per candidate
materialized, including candidates a `BYSETPOS` rule generates internally to fill its period
before position selection, so no single `next()` call performs unbounded uncharged work.
`RuleCursorState`'s shape and that charging rule are named here as requirements and not yet
designed; `COUNT`-bounded resume and `BYSETPOS` must not ship until that design closes, or the
crate ships an unverified correctness claim under its own advertised guarantee.

The merge is concrete. `EXDATE` and `RDATE` are caller-supplied sorted instant slices, normalized
through ical-tz before ical-recur compares anything — a `VALUE=DATE` exception and a `TZID`
cadence instant are not comparable as raw values — and they apply per candidate by set difference
and set union, never by materializing the full set. `RECURRENCE-ID` overrides are keyed by the
instant the base rule generated, not by the override component's own `DTSTART`, through a lookup
that tags each candidate with a `#[non_exhaustive]` provenance: an exact match, a
`RANGE=THISANDFUTURE` anchor, or an instant added by `RDATE`. A `THISANDFUTURE` anchor is stored
and applied as a property-level diff — which fields the override changed — spliced onto each
later candidate's own otherwise-unmodified fields, never as a scalar time delta, because RFC 5545
3.8.4.4 permits an override that changes `LOCATION` and moves nothing. That is DP-07's "mutation
states only what changed" shape one layer up. When an instant appears in both the `EXDATE` list
and the override table — spec-violating, but attested real input — `EXDATE` wins and the
occurrence is dropped. A caller-supplied override index for paged or database-backed stores is
considered and deferred; v1 callers flatten to a borrowed sorted slice.

An occurrence therefore carries two instants, and they are not interchangeable. The cadence key
is the instant the base `RRULE` or an `RDATE` generated; it is what a `RECURRENCE-ID` override is
keyed by and what the merge sorts on. The effective start is the instant after any override diff
has been applied; it is what a consumer displays and what the caller's window means. They are the
same instant only when no diff moves the time.

The caller's window admits or rejects an occurrence by its effective start, never by its cadence
key. Because a `RANGE=THISANDFUTURE` diff may move a start in either direction, the iterator
generates cadence keys over the caller's window widened by the maximum absolute time shift
present in the caller-supplied override set, computed by scanning that slice once before
generation, and then filters the resulting effective starts back to the window the caller asked
for. With no time-shifting override present the widening is zero and the window is unchanged.
Emission is ordered by effective start.

Anchors compose. A series edited "this and following" twice — a March anchor changing `LOCATION`,
a June anchor changing `SUMMARY` and restating nothing else — must not revert the March change in
July. A candidate's diff is the fold of every `THISANDFUTURE` anchor at or before it, applied in
`RECURRENCE-ID` order, a later anchor's stated fields overwriting an earlier one's; omission
means no opinion, not revert-to-base. Provenance names the nearest anchor for reporting, not the
whole set of what was applied. `EXDATE`-wins is likewise scoped to an instant and never to an
override object: a redundant `EXDATE` landing on an anchor's own instant removes that one
occurrence and leaves the anchor's diff in force for every later candidate. The other reading
turns one duplicated line, which real exporters are documented to have emitted, into the silent
reversion of an unbounded tail of the series.

## Consequences

There is no "expand this rule" function that returns a `Vec`, and callers who want one
write it themselves with a window they chose. That is the intended friction.

Rendering a month view costs a month of computation regardless of how the rule is written,
which is the property a UI needs.

The budget is observable, which means the conformance suite can assert on it: a rule that
should find its next instance within N candidates is a testable claim, and a regression
that makes the search less efficient shows up as a limit breach rather than as a slow test.

The corpus also carries the cases the types cannot decide: `next()` past a terminal step; a
resumed `COUNT`-bounded expansion matching a from-scratch one truncated at the same `COUNT`; a
negative `BYSETPOS` rule that cannot exceed budget inside one `next()`; `count()` on an exhausted
search; a `THISANDFUTURE` override changing only `LOCATION` with no time shift, which guards
against sliding back to a scalar delta; an instant in both `EXDATE` and the override table; two
chained anchors whose second diff is minimal; and a window whose upper edge falls between a
cadence key and its shifted effective start. One case is filed without an answer: an
`RDATE`-added instant colliding on the same effective start as a diff-moved one has no dedup rule
here. The case exists to force that choice, which is open.

Two dissents are kept rather than settled. The sealed non-`Iterator` cursor scored higher than
the item type adopted here and was rejected for a by-value-move trap and a `next()` sketch that
does not compile; the further argument that it silently discards a terminal signal no longer
distinguishes it, since the adopted type is now known to be discardable too, so a revisit must be
judged on the trap and the sketch alone. The scalar-delta reading of `THISANDFUTURE` also scored
higher, on the reasonable ground that most real edits are time shifts and that full RFC
generality could wait for usage data; it was overruled on the text of 3.8.4.4, and zero of three
independent architects reached the diff-based shape on their own. The first implementation of
both mechanisms should be treated as unverified: the composed-diff fold and the skew widening
have no compile evidence behind them, and ical-recur today is doc comments and `#![no_std]`.

Skew is attacker-controlled. A file may declare a `THISANDFUTURE` shift of years and force
cadence generation far outside a one-month view. The candidate budget bounds that into a reported
outcome rather than a hang, which is this ADR working as designed, but a hostile shift and a
legitimate one are textually identical, so some honest files will be reported unresolvable, and
no rule relating skew to the budget is proposed here. The deferred override index is now harder
rather than easier: a paged store must answer "maximum absolute shift" without materializing
itself. And "omission means no opinion" leaves reverting a property to its base value
unexpressible in a later override — RFC 5545 offers no syntax for it either — so that loss is
documented rather than eliminated, the same class as the `EXDATE` tie-break.
