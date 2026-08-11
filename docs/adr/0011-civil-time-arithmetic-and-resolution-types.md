# ADR-0011: civil arithmetic is checked, and invalid instances are filtered, not coerced

- Status: accepted
- Date: 2026-08-10
- Amended: 2026-08-11 (seven amendments)

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
exports say shift, the default flips; the two-gate structure does not. **Amendment 4 quantifies
"real exports say shift" and records what six library implementations answer — including that the
majority answer is one `GapPolicy` cannot produce.**

## Consequences

Naming this crate `ical-tz` and then placing most of its types elsewhere is a seam a reader
will trip over. The types are `ical-core`'s, the arithmetic on them is `ical-core`'s, and
what `ical-tz` owns is the resolution of a local time against a zone — which is the whole of
its subject and none of its vocabulary. The first compile of the whole graph is what forced
that, and it is recorded here rather than smoothed over.

The API is two shapes where one uniform `Duration` would have been friendlier, and every call site
doing month math branches on a three-way enum. The objection this leaves standing is whether
ordinary callers should face that enum at all, rather than a single default with the three-way
outcome reserved for the `RRULE` entry point. **Amendment 5 closes it on a count rather than on
taste, and adds the boundary this ADR left vague: convenience is permitted on the outcome, never
as a second entry point on the date.**

An unsatisfiable rule is bounded but not diagnosed: it ends as budget exhaustion, which is what a
merely rare rule produces too. dateutil answered the same bug (its issue 523) with an upfront
validity error, and this ADR deliberately does not mandate a static `BYMONTH`/`BYMONTHDAY`
compatibility check, so a caller cannot distinguish "can never match" from "did not match within
the limit" — a worse diagnostic than the ecosystem's best answer, and what reusing an existing
budget instead of adding a pre-flight check costs. **Amendment 6 overturns that refusal, and
narrows rather than closes the gap: the textbook case is caught at decode time and the rare rule
still ends as budget exhaustion.**

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
lookups. **Amendment 7 gives it one, at a single funnel, and narrows that divergence by count
without closing it in time.** Nor is the denial all of this rests on unconditional: it holds only while every lint
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

**4. The gap-case default stays skip, and the flip condition is quantified rather than left as a
sentence.** The Decision says the corpus may overturn this default and leaves "real exports say
shift" unmeasured, so the flip would have been argued afterwards. It is fixed here in two legs,
and only one of them can move anything.

*The flip-bearing leg.* For each of Google Calendar, Microsoft 365 and Apple Calendar, obtain one
committed export containing a recurring series whose expansion crosses a spring-forward transition
with the rule anchored inside the gap hour, and record whether that producer's own rendered
occurrence list skips the instance or shifts it. The producer's rendering is the answer; this
workspace's expansion of the same file is not. The source is real exports from accounts this
workspace does not hold, landing in M5's corpus under ADR 0006's reduction and anonymization rule —
not reachable without credentials, and this amendment says so rather than assuming somebody will
have them. **The threshold:** the default flips to shift only if at least two of the three shift
*and* none of the three skips. Two shifting against one skipping is a divergence and not a default
— it becomes an ADR 0006 case listing every observed behavior, the policy stays caller-stated, and
skip remains the default because section 3.3.10 writes a MUST and this project does not overturn a
MUST on a split field. Fewer than three producers measured is not a measurement, and no weighting
of one or two substitutes. **If the exports cannot be obtained by the close of M5**, this item does
not roll forward: skip becomes the permanent default, the Decision's "if real exports say shift,
the default flips" clause is struck as unfalsifiable in this workspace rather than left standing as
a promise, and caller-stated shift is the whole of the answer for anyone who wants section 3.3.5's
reading. A partial obtain ends the same way, recorded as an observation rather than resolved by
weighting it.

*The recorded leg, which carries zero flip weight.* Resolving the single discriminator
2007-03-11T02:30 in `America/New_York` across library subjects, into four buckets: no instant
(skip), 07:30Z (the offset before the gap, section 3.3.5's reading, this crate's shift), 07:00Z
(the gap's end), and 06:30Z (the offset *after* the gap, which no policy variant produces). It
carries no weight by construction, because these subjects read calendars and the flip condition is
about what producers write. What it has established: libical 4.0, ical.js, python-dateutil at
either fold, and Apple's own `ccs-pycalendar` all answer 06:30Z; python-icalendar answers 07:30Z at
both providers' defaults; sabre/vobject is unread and is named as unread rather than tallied. Two
recording rules bind it. A subject whose answer is parameterized by a disambiguation flag the file
does not carry is recorded as one row per flag value plus a separately marked library-default row,
never as a single bucket — a row without its flag value is not a row. And a subject named in this
amendment's prose has a row, or it is not named; ical4j and Radicale were asserted in an earlier
draft with no reading behind them and are struck for that reason.

Nothing gathered flips the default, and two corrections ride with that. Shift is not
interoperability with nobody in particular — python-icalendar emits it. And the majority answer is
a reading `GapPolicy` does not offer: 06:30Z is the wall clock read with the offset *after* the
gap, an artifact of libical's transition search inherited verbatim by ical.js and arrived at
independently by dateutil and `ccs-pycalendar`, and no RFC section reads it. A caller can compute
it from the resolution the crate already hands back, and the policy enum is `#[non_exhaustive]`, so
a fourth variant is additive whenever it is decided; that is filed as its own question rather than
decided here, because this one asks which default ships and not which variants exist.

Four costs, and the first is the real one. The default this workspace ships is the answer no
measured subject produced: four give 06:30Z, one gives 07:30Z, none drop the instance. A caller
taking the default disagrees with every implementation read, and gets nothing plus a diagnostic
where four of them would have handed back an instant. Defending a MUST against a unanimous field is
defensible; it is not free, and this amendment does not get to call the field split in its favor.
The policy enum also cannot express what most of the ecosystem does, so until a fourth variant
exists, matching the majority means destructuring the nonexistent-time resolution at the call site
— which is precisely the "caller reimplementing it at every call site" the clamp variant exists to
prevent. Striking two subjects costs this reading its JVM and server coverage outright, so
"majority" now means a majority of six C, JavaScript and Python subjects, and the corrected
sentence is narrower than the false one it replaces. And the per-flag recording rule makes the
instrument larger and version-sensitive.

The strongest rejected alternative is flipping to shift now: section 3.3.5 governs an explicit
date-time, `DTSTART` is one, python-icalendar already emits that reading, and skip is the only
candidate that destroys information. It loses on the threshold rather than on the merits — the flip
clause names producer exports, and one consumer library in six is not that measurement. The second
is adopting the 06:30Z majority as the default, on the ground that interoperability beats textual
fidelity; it loses because defaulting to a specification's MUST that implementations ignore is a
*stated* disagreement, while defaulting to an artifact nobody wrote down is an unstated one.

**5. `MonthAddOutcome` faces every caller, and convenience lives on the outcome rather than on the
date.** The Consequences leave standing the objection that ordinary callers should not face a
three-way enum, and the count answers it: across seven crates and roughly ninety thousand lines,
`add_months` has exactly one non-test call site and it is inside the `RRULE` path the objection
proposed reserving the enum for. The "ordinary caller" a convenience default would serve is empty
in-tree, and the enum's measured cost is one `let`-else. A reserved default would also be the same
erasure this ADR's own Context rejects — answering the case where the specification has an answer
and the case where it has none identically — with a smaller audience, so overturning it would be an
ADR change and not an ergonomics call.

What is *not* answered by keeping the enum, and is answered here, is the boundary. The
implementation has already made a decision no document adjudicates: the outcome's shortest accessor
hands back the carried date behind an option that only distinguishes overflow, which is the
rejected single default reachable in one method call, and it appears in neither design document nor
this ADR. Its own doc comment warns that taking its result as the answer is the coercion the
specification forbids — a rule stated in prose and enforced nowhere, in a workspace whose other
rules lean on lints. The asymmetry is the actual defect: the coercing reading has an accessor and
the conforming reading has none, so the path of least resistance points at the erasure. So four
rules. `CivilDate` has exactly one month-stepping method and its return type is the outcome; any
function returning a date or an optional date from a month count is a violation whatever crate
declares it. `MonthAddOutcome::date` becomes `carried_date`, because for the clamped variant it is
not the day that was asked for and the name must say so at the call site — and because
`CivilDateTime::date` in the same crate is total, so one name currently means two things. The
distinction-preserving accessor ships beside it and is no longer to write: `exact`, some for the
exact variant only. And both are listed in the design documents' API blocks, which list neither
today, so an accessor absent from them is a review finding.

What this makes worse. The API grows by one method in answer to a complaint that it is too
demanding, and the new pair is silently confusable — the two accessors have identical types and
only the name objects, so the rule is enforced by review rather than by a lint, which makes it the
weakest-held rule in an ADR whose others rest on denied warnings. The rename spends a naming window
that is free only because nothing is published; if a release slips in first, the same change
becomes breaking. And this closes the objection by declaring its audience empty, which is true
in-tree and unknowable outside it: the first external caller doing iTIP month math writes a
four-arm match — three variants plus a permanent wildcard — to move a date one month, and that will
be the most common complaint about this crate's date API. This ADR keeps that cost deliberately and
buys nothing back for it.

The rejected alternative is a clamping entry point on the date, which is what chrono, ICU4X and
every mainstream calendaring library ship and what users arrive expecting. It is genuinely strong
and not merely ergonomic: section 3.3.10's MUST governs recurrence expansion and does not reach an
iTIP counter-proposal or a user interface's "one month later" button, so outside recurrence this
project imposes a strictness the specification never asks for. It is rejected because a second
entry point makes the coercion the path of least resistance and moves the choice from the call site
into a method name a reviewer skims; a caller who wants clamping matches the clamped variant in one
line and has thereby said in their own code that they chose it. If this is ever revisited, what
should move the project is an out-of-tree caller count rather than an argument — and keeping the
concession on the outcome deliberately leaves room for a combinator there without a new entry point
on the date.

**6. An unsatisfiable rule earns its own diagnostic, at decode time, and the static check this ADR
refused is adopted.** The Consequences record the defect and accept it for economy of mechanism.
Three of this workspace's own precedents say it cannot stand: ADR 0010 requires that exhaustion be
reported as itself, because "cut short at the limit" and "the rule ended" must be different
answers; ADR 0002's amendment 12 split one terminal reason out of another for exactly that reason;
and its amendment 14 reported a construct it could have left quiet on ADR 0009's authority. This is
the same defect one dimension over — a provably complete empty answer arriving dressed as
truncation — and it defeats a report in its own stated purpose, since the candidates-spent figure
exists so a caller deciding whether to retry with a larger budget knows it was close rather than
nowhere, and for an unsatisfiable rule no budget suffices.

So: one new code, `unsatisfiable-recurrence-rule`, at `Severity::Note`, emitted in `ical-recur`'s
decoder beside the existing part-level refusals and before a single candidate is charged. `Note`
and not `Violation` because section 3.3.10 forbids none of these rules — it defines their reading —
and because this is the per-candidate nonexistent-instance code, already a Note, said once about a
rule instead of once per candidate. It consults `ical-core`'s existing date-validity primitive and
does not re-tabulate month lengths in the decoder. Budget exhaustion keeps its meaning exactly: no
new terminal variant, no refusal of the rule, no discarding of the file, and a caught rule still
expands and still ends where it ended. What changes is that the caller holds two facts where it
held one, and the cheap fact no longer costs a quarter of a million candidates to obtain.

The check is sound and deliberately incomplete, and its extension is named in the golden row rather
than left to the implementation: it decides only whether the product of the date-naming parts the
decoder holds names a (month, day) pair that occurs in some Gregorian year, so February 29 is
satisfiable and February 30 is not, and nothing else is decided — not weekday ordinals, not week
numbers, not a year-day alone, not set positions, not exclusion dates, not the second gate, not an
`UNTIL` that precedes the first match. Silence from the check is not a claim of satisfiability, on
the same terms ADR 0009 already sets for a diagnostic-free parse.

Static rather than runtime, against the framing the docket used. Runtime discovery is unsound as
stated, because "every period filtered" is only ever "every period walked so far": `BYMONTH=2;
BYMONTHDAY=29` filters three consecutive periods before matching and `BYYEARDAY=366` filters seven
across the 2100 century rule, so any threshold either lies about those rules or is large enough
that the diagnostic arrives at exhaustion, which is where the caller already was. And the sound
part of runtime discovery is the static check in disguise, since the reason every February
candidate is clamped is a property of the rule's parts rather than of the periods walked. A third
argument decides the placement independently: the per-candidate code fires once per filtered
candidate, so an unsatisfiable rule under the default budget pushes on the order of a quarter of a
million identical Notes — and on ADR 0009's fixed-capacity sink they are refused and become a
dropped count, so precisely the no-allocation caller that ADR promises "may lose which violations
occurred, never that they did" ends up holding a nonzero count and nothing else. One decode-time
diagnostic arrives before the flood and survives a full sink; a runtime one arrives inside it.

Four costs, and the first is structural. This starts paying a recurring tax that ADR 0009's freeze
levies on any code whose extension is an implementation detail: the meaning is frozen, the *set of
rules that produce it* is not, and a corpus case asserting "this input produces no diagnostic"
flips the day somebody teaches the check a week number or a weekday ordinal. Writing the golden row
narrowly is the mitigation and also the problem, since a narrow row must be renamed or deprecated
every time the check gets smarter, converting an improvement into ceremony. A second site in the
workspace now answers whether February 30 exists, and requiring it to consult the primitive rather
than tabulate month lengths is a stated rule that nothing in the type system enforces — a
hand-rolled table would compile, which widens the seam this ADR's Consequences already apologize
for. Legal, previously quiet input now speaks, so a deliberate no-op placeholder is diagnosed
forever after, with `Note` as the whole mitigation. And it does not close the gap it was raised
for: the dominant reason a rule matches nothing within a budget is rarity rather than impossibility,
so `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` — the rule ADR 0002 opens with and ADR 0010 builds its
amplification argument on — is satisfiable, is rare, and still ends as budget exhaustion
indistinguishable from any other. This catches the textbook case and leaves the attacker's case
where it stood.

The rejected alternative is the runtime discovery above, and it is rejected on soundness rather
than on taste: it needs no calendar knowledge in the decoder, costs nothing on satisfiable rules,
cannot misfire on parts it does not understand, and would catch unsatisfiability arising from
combinations no static table can decide. Preserve it if the adopted check's incompleteness becomes
the binding complaint — but the correct successor is then a *bounded certificate*, the expansion
reporting which parts filtered every candidate, which is a claim about evidence rather than about
the future and could be added beside the static code without contradicting it. Also rejected, and
worth keeping rejected: dateutil's own answer of an upfront error, which ADR 0001 forbids because
the file may not be discarded and ADR 0009 forbids because every part of such a rule is
individually legal. Only its timing was worth copying.

**7. `ZonedSeries::admits` charges the ledger, and so does every other zone lookup this workspace
makes on a series' behalf.** The Consequences name the hole and ADR 0010 names the rule — a bound
nobody charges is decoration — and Amendment 13 of ADR 0003 has since measured one such lookup at
about 35 ms against a definition with two thousand rule-bearing observances, so it is no longer
cheap enough to leave uncounted. The charge goes at one funnel rather than at each call site:
`ZonedSeries::answer_for` becomes the single charging point for the source's resolve, `projected`
charges the same unit for its offset lookup, and `admits` and `resolved` therefore take a meter
too. The gate `ical_recur::RecurrenceInput::admitting` accepts takes one as well, and the engine
passes down the meter it already holds. Those are the two names this was always going to move.

The dimension is a field of the one `Limits` under ADR 0010's stated shape, refused as its own
variant, and its refusal latches exactly as the candidate charge does — after which the funnel
returns the answer it already returns for an unrecognized identifier, so the search is stopped and
reported by the next candidate or occurrence charge as budget exhaustion rather than by a silently
dropped occurrence. The default is *derived* rather than asserted, and the derivation is stated:
the gated civil path spends two lookups per emitted occurrence — the gate's resolve plus the
emission-side one — one per key the gate rejects, and one per explicit date placement, so the
largest total a workload the existing calibration admits can reach is just above half a million on
the default policy, and four times the occurrence ceiling is the smallest power-of-two multiple
standing strictly above it. A ceiling equal to the occurrence ceiling, which an earlier draft
proposed, is refuted by that same arithmetic: a search legitimately emitting the full occurrence
ceiling would have exhausted its lookup ceiling at the halfway point. Charging only the keys the
gate accepts is declined for a different reason — it would leave one of the two lookups uncharged,
which reopens the hole the rule exists to close, and it would blind the ledger to the one case the
dimension is for, a crafted definition whose gaps make the gate reject nearly every key, where the
rejected keys *are* the work.

Four costs. Two public names move before 1.0, and the gate stops being a plain predicate over an
instant: every gate now threads a ledger it did not ask for, so a bare method reference or a
closure over nothing no longer fits. `resolved` and `admits` lose the property their own doc
comments sell — same policy, same answer, no meter — so a caller with no ledger must mint one,
which is the mint-inside-a-loop shape the budget module's own documentation calls out as how a
budget silently stops binding. This is the first field whose default is a function of another
field, so the type's implicit promise that each field is an independently meaningful ceiling is now
false in one place: a caller who raises the occurrence ceiling through the builder and leaves this
one alone is refused by a dimension they never set, and the const assertion defends only the two
shipped policies. And the dimension counts calls rather than work — a lookup against a
two-observance zone costs microseconds and is charged the same unit as the 35 ms one — so this
narrows the divergence the Consequences name, in *count*, without closing it in time, and nothing
here claims otherwise.

The rejected alternative is charging lookups against the existing octet budget, one unit each,
exactly as a candidate is charged one octet: no new field, no new variant, no derived default, and
the aggregate bound already exists there. It loses because that budget is set from the input's
size, so how many zone lookups a series may make would depend on how large the file that carried it
was and how much of that budget parsing already spent — a large calendar would refuse a modest
series' lookups on an unrelated basis and a small file would refuse a legitimate expansion outright
— and because a lookup and an input octet differ by three orders of magnitude in cost, so one unit
each is a fiction the ledger would then report as fact. A per-series counter inside `ical-tz` is
rejected on ADR 0010's founding premise: a threshold checked per call is bounded per call and
unbounded in aggregate.
