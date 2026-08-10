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

## Amendments

M1 built the engine this document specified, and eight sentences above did not survive contact
with it. Each is amended here rather than quietly reinterpreted, and each has a test in
`ical-recur` or a conformance case in `ical-conform` behind it.

**1. The item type this ADR first committed to was found not to work, and the replacement is
what the Decision now states.** The shape originally adopted — `Item = Result<Occurrence,
BudgetExhausted>`, carried into `docs/design/ical-recur-api.md` as DP-09 — does not deliver the
guarantee the rest of this document rests on: std's `impl<T, E> IntoIterator for Result<T, E>`
makes `search.flatten()` compile against it and discard every terminal marker, and
`.filter_map(Result::ok)` and `.take_while(Result::is_ok)` do the same in one reviewed-without-
comment line each. What shipped is `SearchStep<'a>` — a crate-owned `#[non_exhaustive]`
`#[must_use]` enum of `Occurrence(Occurrence<'a>)` and `BudgetExhausted(BudgetExhausted)` —
together with the caller's own `&mut Meter`, whose exhaustion flag latches and outlives every
combinator applied to the iterator, and `RecurrenceSearch::outcome()`. Three reports of one
fact, in decreasing order of survivability. The Decision above carries that text; this entry
exists because an ADR whose committed mechanism was replaced without saying so is worse than one
that was never specific.

**2. A window admits an occurrence by its cadence key *or* by its effective start, not by the
start alone.** "The caller's window admits or rejects an occurrence by its effective start,
never by its cadence key" and `crates/ical-recur/src/search.rs`'s "a window admits by cadence
key" are opposite statements, and implementing either one alone loses occurrences. Start-only
admission drops an occurrence the caller can still address by `RECURRENCE-ID` because something
moved it out; key-only admission loses an occurrence an override moved *into* the window the
caller asked about. The search asks both questions and emits on either, and an occurrence
admitted by its key whose start lies outside is reported on
`DiagnosticCode::OverrideLeftWindow`. The widening this document specifies is unchanged and is
the library's work rather than the caller's: generation runs over the asked window widened by
the largest absolute shift the override set implies, and the filter back down is inside the
search.

**3. Emission is ordered by cadence key, not by effective start.** The two cannot both hold with
a merge that materializes nothing. Effective starts are not sorted — a `THISANDFUTURE` shift
reorders them by up to the largest shift in the override set, which is attacker-controlled — so
emitting in start order needs a buffer holding every occurrence within one skew-width of the
cursor. That is a retained-size dimension with no field in `Limits` and no charge site, which is
exactly `docs/adr/0010`'s "a bound nobody charges is decoration". Cadence-key order is what a
linear merge over sorted sources can do, and it is also what makes the stop rule sound: the
first key at or past the widened generation window proves nothing after it can reach the
caller's window. A caller that wants start order sorts what it collected, which is a bound it
chose.

**4. The `results()` adapter is not built, and is withdrawn rather than deferred.** It was named
above as the opt-in that puts the original hazard one deliberate call away. Shipping it ships
the hazard back with a name on it, and nothing in M1 needed it: a caller that wants `?` writes
one `match` over a two-variant enum. `SearchStep::occurrence` is the discard this document calls
visible-but-possible, and it is deliberately not named `ok`.

**5. The recurrence set begins at `DTSTART`.** Nothing above says so, and the omission was a
defect the RFC's own worked examples caught. A period is expanded whole, so the period holding
`DTSTART` offers candidates before it: `FREQ=MONTHLY;BYMONTHDAY=1,-1` from September 30 names
September 1 of the same period. RFC 5545 section 3.8.5.3 begins every recurrence set at
`DTSTART`, so a cadence key before it is not an instance and does not spend a `COUNT`. The skip
is after `BYSETPOS` selection and not before it, because a `BYSETPOS` position counts within the
period as the rule describes it. Section 3.8.5.3's "Every other year on January, February, and
March for 10 occurrences" is the row that pins this: its own answer is one instance in the first
year and three in each of the next three, which only totals ten if January and February of the
`DTSTART` year are skipped without being counted.

**6. The collision case filed open above is closed: an `RDATE`-added instant and a diff-moved
one landing on the same effective start are both emitted, and neither is deduplicated.**
Identity in this crate is the cadence key — what a `RECURRENCE-ID` addresses, what `EXDATE`
removes, what `COUNT` counts — and the two candidates have different keys, so fusing them leaves
one addressable and the other silently gone from a file that names it. A caller can fuse and
cannot unfuse. A dedup keyed on effective start is also not a linear merge, for the reason
amendment 3 gives. Related and decided the same way: an anchor's stated time shift does not
reach an instant an `RDATE` named, while its property diff does — an `RDATE` value is a literal
instant the file states, with no cadence in it to shift.

**7. One site charges a candidate, and the default budget is calibrated.** Budget is charged
inside the expansion, where a candidate is generated and where a nonexistent date is discovered,
and nowhere else; the search counts what expansion charged for its terminal report, and counting
is not charging. Two sites would halve the advertised budget and no site would void the
guarantee. `DEFAULT_CANDIDATE_BUDGET` is 262,144 rather than 65,536: the old value was exactly
`Limits::DEFAULT.candidates_per_period()`, so the per-period ceiling and the whole-search budget
were one bound wearing two names and the second dimension `docs/adr/0010` argued for bought
nothing. The workload table the new number was read off is in `crates/ical-recur/src/accounting.rs`
and is asserted against the shipped constant.

**8. The precondition this document set on `COUNT`-bounded resume and `BYSETPOS` is discharged.**
"`RuleCursorState`'s shape and that charging rule are named here as requirements and not yet
designed; `COUNT`-bounded resume and `BYSETPOS` must not ship until that design closes." Both
closed in M1. `RuleCursorState` is an opaque period index and is deliberately not serializable —
freezing its encoding would freeze the expansion algorithm — and a resumed search restarts one
period behind the last it yielded, skipping by cadence key what it already produced, so no
candidate inside a half-read period is lost. `BYSETPOS` selects from a period that was charged
as it filled, so a negative position cannot do unbounded uncharged work inside one `next()`.

**8a. Four adversarial lenses were run against the built engine, and what follows is what they
found.** Amendments 9 through 14 are the decisions those findings forced. Each has a case in
`crates/ical-conform/tests/break_recur_*.rs` written before the fix and passing after it, and
none of them was reached by weakening what the case asserts.

**9. Exhaustion latches on every bound the ledger keeps, not only on the octet budget.** "The
caller's own `Meter`, whose exhaustion flag latches and outlives every combinator applied to the
iterator" was true of `Limits::max_input_bytes` and false of the two bounds a recurrence search
actually stops at: `Meter::try_charge_candidate` refused against `candidates_per_period` and
`try_charge_occurrence` refused against `occurrences_per_search`, and neither touched the flag.
A search terminated by either therefore left the second of the three reports reading clean beside
a truncated answer — the exact state this ADR's item type exists to prevent, reached through the
report that was supposed to be the durable one. All three refusals latch now. The cost is stated
rather than hidden: one runaway series in a fan-out over a shared meter now stops the fan-out,
loudly, instead of every later search paying for it, and a caller wanting per-series isolation
gives each series its own ledger — which is the same choice `docs/adr/0010` already names.

Related, and the second half of the same finding: `Limits::DEFAULT.occurrences_per_search` was
65,536 while the candidate calibration in amendment 7 admits a whole day of `FREQ=SECONDLY`,
which is 86,400 occurrences. A retention bound below the largest workload the candidate budget
pays for is the "two round numbers, one of them wrong" defect amendment 7 fixed one dimension of.
It is 262,144, the same four maximal periods.

**10. The terminal report counts what was charged, not what came back.**
`BudgetExhausted::candidates_spent` exists "because a caller deciding whether to retry with a
larger budget needs to know it was close rather than nowhere", and the engine was accumulating
the size of each *successfully expanded* period. A period refused while filling returns no set
and has still paid for everything it generated, and a rule that produces an instance in no
period — `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30`, the rule `accounting.rs` holds up as the one a
per-emission budget cannot see — produces no sets at all and spends the whole budget. Both
reported zero. The count is now the difference between two readings of the ledger's own
cumulative candidate count, which is exact because a search borrows that ledger exclusively and
because it does not care which code path did the charging.

**11. A window admits an occurrence only after the merge has said which source it consumed.**
The merge documents a three-call protocol — `is_drained`, then `takes_rule_key`, then `step` —
and the engine called only `step`, inferring both other answers from its silence. `Merge::step`
answers `None` both for a candidate an `EXDATE` removed and for no candidate at all, so the two
inferences failed in opposite directions: an offered rule key that an earlier `RDATE` preempted
was retired although nothing consumed it, deleting the instance after the exclusion — including
`DTSTART` itself — and an `RDATE` tail whose head was excluded was read as the end of the whole
series, discarding every addition after it. One `EXDATE` landing on any `RDATE` but the last
could erase an unbounded number of occurrences, and a `COUNT` had already been spent on each.
The engine asks all three questions, in that order.

**12. The calendar ending is a fourth terminal state, and it is not the rule ending.** Two
findings met here. `PeriodWalk` computed each period's exclusive upper edge and refused the whole
period when only that edge left the calendar, so the last period of every cadence was deleted —
`FREQ=DAILY` from 9999-12-28 lost December 31st, `FREQ=YEARLY` lost the year 9999, and so on for
all seven frequencies — although every instant in those periods is representable and RFC 5545
section 3.3.4 writes them. The edge was read nowhere: `byparts::expand_period`'s own contract is
that a period "is read for its anchor alone". `Period` carries an anchor now and the field is
gone. Second, when the walk does run dry the search was reporting `SearchOutcome::RuleEnded`,
which is documented as "the rule reached its `COUNT` or its `UNTIL`" and is false for a rule with
neither. `SearchOutcome::CalendarEnded` is that answer, `DiagnosticCode::RecurrenceCalendarEnded`
carries it to a caller that kept only the sink, and it reports `is_complete() == true` — nothing
a second search could reach is missing, because there is no more calendar. Complete and
`RuleEnded` are different questions and the caller is now able to ask each.

**13. `BYWEEKNO` expands a yearly period to the weeks of its *week-numbering* year.** The engine
read it as a filter over the days of the calendar year, comparing each day's own week-year
number and never asking whether that year was the period's. The two readings partition the same
union, which is why the RFC's own week-20 example and every rule with `INTERVAL=1` and no
`BYSETPOS` agree under both — and why this survived until a lens skipped a period. They disagree
wherever a period is skipped, a set is selected from, or a year's week count is asked about:
week one of 2020 begins on Monday 2019-12-30, which an every-other-year rule anchored in 2018
must name and which the old reading attributed to the 2019 period the interval skips. Week-year
extents tile the timeline exactly as calendar years do, so candidates still ascend across
periods and nothing else in the engine had to learn about it.

**14. A `BYDAY` ordinal under a frequency that forbids one is ignored and reported.** Section
3.3.10 says the numeric form "MUST NOT be specified when the `FREQ` rule part is not set to
`MONTHLY` or `YEARLY`" and gives no reading for a file that carries one anyway, so this is a
divergence rather than a defect — but the engine was answering it two ways. Under `DAILY` and
`HOURLY` the ordinal was ignored and the weekday kept; under `WEEKLY`, whose cell prints `Expand`
rather than `Limit`, it was resolved inside a scope one week wide, so `BYDAY=1TU` worked while
`BYDAY=2TU` silently produced an empty recurrence set — `DTSTART` included — with no diagnostic
at all. The permitted answers are: ignore the ordinal and keep the weekday; honor it in some
invented scope; or refuse the part. Ignoring is what python-dateutil does and what this crate now
does under all five forbidding frequencies; the other three ecosystem engines were not measured
for this milestone and that gap is recorded rather than papered over. The decoder reports the
construct on `DiagnosticCode::RecurrenceRulePartOutOfRange`, because a rule the author and this
crate read differently is exactly what `docs/adr/0009` says must not be silent.

**14a. M2 built the seam this document's siblings depend on, and two more sentences here did not
survive it.** Both have a case in `crates/ical-conform/tests/break_tz_seam.rs`.

**15. `max_absolute_shift` counts the timeline the caller's own instants are on, and there is no
second half to add to it.** The shipped documentation called the number a count of *elapsed*
seconds — "the whole of the move for a floating or UTC series and only part of it for a zoned
one" — with `ical_tz::extra_widening` there to give back what an elapsed count could not see.
That is a reading of nothing. This crate differences two instants an override carries, and for a
zoned series `ical_tz::seam` puts both of them on the series' own wall clock projected onto UTC:
the difference is a wall-clock count, the widening it gives is already exact for the wall-clock
move that gets propagated, and the shortfall `extra_widening` reports on that timeline is always
zero. On the real timeline the two instants are not the ones an override carries at all. The
function is unchanged and its documentation now says which timeline it counts;
`ical_tz::WallClockShift::across` is where the two readings of one move are held apart, by
converting two cadence keys into the real instants they name before measuring.

**16. Two overrides naming one instant are ranked by file order and counted, not refused.**
`OverrideSet::new` answered `InputError::Duplicated`, on the ground that two edits of one instant
have no defined precedence — which is true, and cost more than it was worth. A zoned series
produces the shape without anybody making a mistake: the two halves of the hour a zone repeats
are one wall clock and therefore one cadence key, so `RECURRENCE-ID:20261101T053000Z` and
`RECURRENCE-ID:20261101T063000Z` in `America/New_York` are two real instants an hour apart that
collide, and refusing the input lost not the second override but every occurrence of the event.
The earlier entry applies, `OverrideSet::collisions` reports how many were shadowed, and the
constructor still refuses a list that descends. The preference is stated rather than silent,
which is the property this crate is arranged around; that a fold cannot be told apart on the
nominal timeline at all is a limit of the seam and is recorded in `docs/adr/0011`.
