# ADR-0011: civil arithmetic is checked, and invalid instances are filtered, not coerced

- Status: accepted
- Date: 2026-08-10

## Context

[ADR 0003](0003-caller-supplied-time-zones.md) requires every time computation to state what it
does when the answer does not exist, and the lint enforcing that — `arithmetic_side_effects`, a
`warn` in the workspace lint table promoted to a denial by `-D warnings` in `just lint` and
`just ci` — is already in force. What 0003 never did was name the types it constrains, or say
what "January 31 plus one month" is.

The shape a reader reaches for first is a `Duration` added to a `CivilDate`, and RFC 5545 does not
support it: the DURATION grammar in §3.3.6 has no year or month designator — `P1M` is not a legal
value — and `TRIGGER` (§3.8.6.3) and `RDATE;VALUE=PERIOD` (§3.8.5.3) inherit it unchanged. Month
stepping arises in one place only, `RRULE` with `FREQ=MONTHLY` or `FREQ=YEARLY`.

Two alternatives lost. A single silent policy — clamp January 31 to February 28 — answers the case
where the specification has an answer and the case where it has none identically, erasing the
distinction a caller needs. A saturating or panicking primitive is unavailable anyway: `panic!` is
denied outside tests and CONTRIBUTING.md forbids the `#[allow]` that would buy it back. And
§3.3.10 is not neutral — recurrence instances with an invalid date or a nonexistent local time
MUST be ignored — so filing that under [ADR 0006](0006-conformance-corpus-as-artifact.md)'s vendor
disagreements misreads a MUST.

## Decision

`CivilDate`, `CivilTime`, `CivilDateTime`, `UtcOffset` and `Duration` live in `ical-core` and
the resolution types live in `ical-tz`, and every operation on any of them is `checked_*`,
`div_euclid` or `rem_euclid`; no operator returns a value it could not compute. The placement
follows the crate graph rather than the subject matter: [ADR 0004](0004-sans-io-protocol-layer.md)
makes `ical-recur` and `ical-tz` siblings and leaves `ical-dav` depending on `ical-core` alone,
so a primitive all three speak cannot live in one of them. `Instant` sits one layer lower
still, in `ical-grammar`, because a diagnostic may name an occurrence rather than a byte
offset and the diagnostic vocabulary is the grammar's; `ical-core` re-exports it and owns
every conversion between it and a civil date-time, which is arithmetic rather than syntax. No `Duration`-shaped type carries years or months, so "one month later" is a
method on a date rather than a value that can be added to one.

`CivilDate::add_months` returns `MonthAddOutcome` — `Exact`, `Clamped`, `Overflow` —
`#[non_exhaustive]`, deriving `Debug`, `Clone` and `PartialEq` so corpus cases can assert on it.
Outside recurrence, for an iTIP reschedule one month out, the RFC has no answer and the caller
picks one in the open.

`MonthAddOutcome` governs date validity only, and there it is bounded by
[ADR 0002](0002-bounded-lazy-recurrence.md). `Clamped` — a day of month absent from the target
month — is filtered by ical-recur's monthly and yearly expansion per §3.3.10 and never coerced to
a nearby date; not a caller-chosen divergence, not a corpus slot. Because a filtered candidate is
still a candidate, every filtered instance debits the meter of
[ADR 0010](0010-shared-resource-limits.md), which counts candidates generated and not instances
emitted. A rule unsatisfiable in every period — `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30` names a date
no Gregorian year has — therefore ends as the budget-exhausted outcome instead of searching
forever, and since `COUNT` counts emitted instances only, an unsatisfiable `COUNT` ends the same
way rather than hanging.

The other half of §3.3.10 is not `MonthAddOutcome`'s to enforce: a rule anchored at 02:30 local on
a spring-forward day yields an `Exact` date whose local time does not exist, which a date
primitive cannot see. Time-of-day validity is carried by the resolution type from 0003, and an
instance is admitted only when both gates pass. The gap-case default is skip, per the MUST, and it
is the one default here the corpus may overturn: §3.3.5 resolves a nonexistent explicit DATE-TIME
using the offset before the gap, and Google and Apple are reported to shift instead. If real
exports say shift, the default flips; the two-gate structure does not.

## Consequences

Naming this crate `ical-tz` and then placing most of its types elsewhere is a seam a reader
will trip over. The types are `ical-core`'s, the arithmetic on them is `ical-core`'s, and
what `ical-tz` owns is the resolution of a local time against a zone — which is the whole of
its subject and none of its vocabulary. The first compile of the whole graph is what forced
that, and it is recorded here rather than smoothed over.

The API is two shapes where one uniform `Duration` would have been friendlier, and every call site
doing month math branches on a three-way enum. The objection this leaves standing is whether
ordinary callers should face that enum at all, rather than a single default with the three-way
outcome reserved for the `RRULE` entry point.

An unsatisfiable rule is bounded but not diagnosed: it ends as budget exhaustion, which is what a
merely rare rule produces too. dateutil answered the same bug (its issue 523) with an upfront
validity error, and this ADR deliberately does not mandate a static `BYMONTH`/`BYMONTHDAY`
compatibility check, so a caller cannot distinguish "can never match" from "did not match within
the limit" — a worse diagnostic than the ecosystem's best answer, and what reusing an existing
budget instead of adding a pre-flight check costs.

Composition of the two gates is mandated and not designed. Nothing here names the type, signature
or owner of the thing joining `MonthAddOutcome` to the resolution type, and that follow-up is now
correctness-critical rather than plumbing — a larger debt than before, not a smaller one.

Budget accounting under nested filtering is still not tight enough to assert on. 0010 charges
candidates generated per period, but one `FREQ=MONTHLY` period can produce hundreds through
`BYDAY` and `BYSETPOS`, and no document says whether a candidate filtered here before `BYSETPOS`
selection counts as generated, or whether the per-instance zone query the second gate needs debits
anything at all. Two conforming implementations can therefore differ by orders of magnitude in
when they report exhaustion for one rule, so 0002's claim that the budget is observable enough for
the conformance suite to assert on is not yet true. Nor is the denial all of this rests on
unconditional: it holds only while every lint invocation remembers `-D warnings`.
