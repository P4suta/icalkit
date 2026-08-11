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

Budget accounting under nested filtering is tight on one side and absent on the other. 0010
charges candidates generated per period, and one `FREQ=MONTHLY` period can produce hundreds
through `BYDAY` and `BYSETPOS`; 0002's amendment 7 puts the charge at exactly one site, inside
the expansion where a candidate is generated and where a nonexistent date is discovered, so a
candidate filtered before `BYSETPOS` selection has already been paid for and the conformance
suite can assert on the number. The per-instance zone query the second gate needs debits nothing
at all — `ZonedSeries::admits` takes no ledger — so two conforming implementations can still
differ by orders of magnitude in when they report exhaustion for a rule whose cost is mostly zone
lookups. Nor is the denial all of this rests on unconditional: it holds only while every lint
invocation remembers `-D warnings`.

## Amendments

M2 built the second gate and the seam it sits on, and three sentences above needed either an
owner or a correction. Each has a case in `crates/ical-conform/tests/break_tz_seam.rs`.

**1. The composition of the two gates now has a type, a signature and an owner.** The
Consequences record that "composition of the two gates is mandated and not designed", and what
that cost was measurable: `ical-recur` owns the date gate and applies `COUNT`, `ical-tz` owns
the local-time gate and was applied afterwards, so a `COUNT=5` series with one instance in an
hour its zone never showed delivered four occurrences and no API in either crate composed them
in the order this ADR states. `ical_tz::ZonedSeries::admits` is the second gate as a predicate
and `ical_recur::RecurrenceInput::admitting` is where it goes — consulted after the window and
before the count, so a rejected key costs the series nothing and `COUNT` counts what a caller
receives.

It is opt-in, and that is the decision rather than an omission. A caller that states no gate
gets the other reading the specification licenses, which is the one section 3.8.5.3 forces for a
`DTSTART` that lands in a gap: the instance is `DTSTART`, section 3.3.10 says to ignore it, and
the count is spent on it either way.

**2. A frequency finer than a day counts elapsed time, and the anchor is where a caller says
so.** Everything this ADR and `ical_tz::seam` say about a zoned series is written about a civil
cadence: "every day at 09:00" is a statement about a clock, and the projection onto the series'
own wall clock is what keeps it at 09:00 across a transition. `FREQ=SECONDLY`, `FREQ=MINUTELY`
and `FREQ=HOURLY` say the opposite thing, and the same projection loses an hour of an hourly
series on the day a zone falls back — the wall clock reads twenty-four hours on a day that is
twenty-five hours long. Both readings ship: Google gives 25 occurrences and libical's local-time
expansion gives 24. `ical_tz::ZonedSeries::real_anchor` is the absolute reading and
`ZonedSeries::anchor` the civil one, a series anchored on the real timeline is walked there and
needs no per-occurrence resolution, and neither crate can pick between them — one has the
frequency and no zone, the other has the zone and not the frequency.

**3. The projection onto a series' own wall clock is not injective, and what that costs is
recorded rather than repaired.** The hour a zone repeats is one wall clock, so two
`RECURRENCE-ID`s naming the two real instants inside it project onto one cadence key. Nothing
downstream can tell them apart, and this ADR's own resolution types are what would have to carry
the distinction — a cadence key would have to be a wall clock *and* a fold side. It is not, in
M2, and the cost is bounded and stated: the earlier override applies, `OverrideSet::collisions`
counts the shadowed ones (`docs/adr/0002` amendment 16), and a caller that needs both must
address them by their real instants outside the seam. A `RECURRENCE-ID` carrying a fold side is
the shape that would close it, and no RFC defines one.
