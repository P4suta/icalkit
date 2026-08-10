# ical-recur: the public API

- Status: proposed
- Date: 2026-08-10
- Carries: DP-09, DP-10 (crate); DP-01, DP-08, DP-17, DP-18 (workspace)
- Skeleton: assembled with the other five into one workspace and compiled together; see
  "What the first compile changed" below

## Responsibility

`ical-recur` answers one question: given a component's `DTSTART`, `RRULE`, `RDATE`, `EXDATE`
and `RECURRENCE-ID` overrides, which occurrences fall in the range the caller named, and what
is in force on each. It answers lazily, over a window the caller states, with a candidate
budget the caller charges, and it applies exceptions, additions and overrides inside the
iterator — never handing a caller a raw rule expansion to reconcile afterwards, because that
reconciliation is where every calendar implementation historically goes wrong. It decodes the
`RECUR` value type itself, since recurrence semantics are the whole of its subject and
`ical-core` stops at preserved text. It resolves nothing about time zones: every instant that
crosses this boundary — `DTSTART`, `UNTIL`, each `EXDATE`, each `RDATE`, each
`RECURRENCE-ID` — has already been through `ical-tz` in the caller's hands, because comparing
a floating local time against a UTC `UNTIL` needs a zone this crate is forbidden to have.

## Where this sits

DP-17's adopted spine makes `ical-recur` and `ical-tz` siblings: both depend on `ical-core`
and neither depends on the other. That is why normalization is the caller's step and not a
call this crate makes, and it is the one place where honoring DP-10(c) literally — "route
instants through `ical-tz`'s normalization before `ical-recur` compares them" — was impossible
as written. The mechanism survives; the caller performs it.

The same seam settles where `Instant` lives. DP-12 gave the concrete time types to `ical-tz`,
but `ical-dav` needs `Instant` for `time-range` filters and does not depend on `ical-tz`
either, so `ical-core` declares it and `ical-tz` owns every conversion between it and civil
time. This crate compares instants and never converts one.

## The public surface

Every signature below is reproduced verbatim from the skeleton. Bodies are omitted here.

### The rule (RFC 5545 §3.3.10)

```rust
pub enum Freq { Secondly, Minutely, Hourly, Daily, Weekly, Monthly, Yearly }
pub enum ValueKind { Date, DateTime }
// Weekday is ical_core::Weekday, ordered from Sunday as RFC 5545 orders BYDAY under WKST=SU

pub struct WeekdayNum { /* private */ }
impl WeekdayNum {
    pub fn new(ordinal: Option<NonZeroI8>, weekday: Weekday) -> Option<Self>;
    pub const fn ordinal(self) -> Option<NonZeroI8>;
    pub const fn weekday(self) -> Weekday;
}

pub struct ByList<T>(/* private */);
impl<T> ByList<T> {
    pub fn as_slice(&self) -> &[T];
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
}
impl<T: Clone> ByList<T> { pub fn from_slice(values: &[T]) -> Self; }

pub enum RuleLimit {
    Infinite,
    Count(NonZeroU32),
    Until { at: Instant, value_kind: ValueKind },
}

pub struct RecurrenceRule { /* private */ }
pub struct RecurrenceRuleBuilder { /* private */ }
impl RecurrenceRuleBuilder {
    pub fn new(freq: Freq) -> Self;
    pub fn interval(mut self, interval: NonZeroU32) -> Self;
    pub fn limit(mut self, limit: RuleLimit) -> Self;
    pub fn wkst(mut self, wkst: Weekday) -> Self;
    // one setter per BYxxx part: by_second, by_minute, by_hour, by_day, by_month_day,
    // by_year_day, by_week_no, by_month, by_set_pos
    pub fn build(self) -> Result<RecurrenceRule, RuleError>;
}
```

Invariants, established once at construction and never revisited: `INTERVAL` is non-zero
because `NonZeroU32` is the type; a `BYDAY` ordinal is in ±53 and is never zero, because
`WeekdayNum::new` rejects the first and `NonZeroI8` forbids the second; `BYSETPOS` is empty
unless another `BYxxx` part is present, checked in `build`. `Freq` is not `#[non_exhaustive]`:
§3.3.10 closes the set, and pretending otherwise would tax every downstream `match` forever
for a variant that cannot arrive. `Weekday` is `ical-core`'s, and `ical-core` marked it
`#[non_exhaustive]`; that is its call to make and this crate takes the tax rather than declare
a second weekday type for one crate's convenience. `ValueKind` is local and deliberately
narrower than `ical_core::ValueType`: only `Date` and `DateTime` can be the value type of a
`DTSTART` or an `UNTIL`, and a two-variant enum makes the other twelve unrepresentable.

Out-of-range values that are survivable — `BYMONTHDAY=32`, a repeated part — are not errors.
They are diagnostics on the property and the component still parses, per ADR 0001. `RuleError`
names only the conditions under which no rule exists at all.

### Exceptions, additions and overrides (§3.8.4.4, §3.8.5.1, §3.8.5.2, §3.2.13)

```rust
pub enum PropertyChange<'a> { Set(&'a Property), Removed(&'a [u8]) }
impl<'a> PropertyChange<'a> {
    pub fn name(self) -> &'a [u8];
    pub fn has_id(self, id: &PropertyId) -> bool;
}

pub struct PropertyDiff<'a> { /* private */ }
impl<'a> PropertyDiff<'a> {
    pub const fn new(changes: &'a [PropertyChange<'a>]) -> Self;
    pub const fn empty() -> Self;
    pub const fn changes(self) -> &'a [PropertyChange<'a>];
    pub fn get(self, id: &PropertyId) -> Option<&'a PropertyChange<'a>>;
}

#[non_exhaustive] pub enum OverrideRange { ThisOnly, ThisAndFuture }

pub struct Override<'a> { /* private */ }
impl<'a> Override<'a> {
    pub const fn new(recurrence_id: Instant, range: OverrideRange,
                     moved_to: Option<Instant>, diff: PropertyDiff<'a>) -> Self;
    pub const fn recurrence_id(self) -> Instant;
    pub const fn range(self) -> OverrideRange;
    pub const fn moved_to(self) -> Option<Instant>;
    pub const fn diff(self) -> PropertyDiff<'a>;
    pub fn shift_seconds(self) -> Option<i64>;
}

pub struct OverrideSet<'a> { /* private */ }
impl<'a> OverrideSet<'a> {
    pub fn new(entries: &'a [Override<'a>]) -> Result<Self, InputError>;
    pub const fn empty() -> Self;
    pub const fn entries(self) -> &'a [Override<'a>];
    pub fn exact_match(self, key: Instant) -> Option<&'a Override<'a>>;
    pub const fn anchors_before(self, key: Instant) -> AppliedDiffs<'a>;
}

pub struct AppliedDiffs<'a> { /* private */ }
impl<'a> Iterator for AppliedDiffs<'a> { type Item = &'a Override<'a>; }
impl FusedIterator for AppliedDiffs<'_> {}
```

An override is a **diff**, never a replacement component. A `RANGE=THISANDFUTURE` edit that
changes only `LOCATION` carries one change and no time shift, and a scalar-delta design cannot
express it — that is data loss on ordinary files, and it is why `PropertyDiff` exists. The
time shift a `THISANDFUTURE` anchor does imply is *derived* from the diff, not the diff itself:
`shift_seconds` is `moved_to − recurrence_id`, checked, and that number is what propagates to
later candidates' own keys.

Overrides live in **one** slice, exact matches and anchors together. Two parallel slices would
let a caller desynchronize them with nothing to check it. `OverrideSet::new` requires strictly
ascending `RECURRENCE-ID`s: two overrides claiming one instant have no defined precedence, and
guessing one silently is the failure this crate exists to prevent.

`anchors_before` yields **every** anchor at or before a key, oldest first — not the nearest
one. A series edited "this and following" twice over its life has two anchors in force, and
the later edit is under no obligation to restate what the earlier one changed.

### The input

```rust
pub enum InputList { Rdate, Exdate, Override }
#[non_exhaustive] pub enum InputError { NotAscending(InputList), Duplicated(InputList) }

pub struct RecurrenceInput<'a> { /* private */ }
impl<'a> RecurrenceInput<'a> {
    pub fn new(dtstart: Instant, dtstart_kind: ValueKind, rule: Option<&'a RecurrenceRule>,
               rdates: &'a [Instant], exdates: &'a [Instant],
               overrides: OverrideSet<'a>) -> Result<Self, InputError>;
    pub const fn dtstart(self) -> Instant;
    pub const fn dtstart_kind(self) -> ValueKind;
    pub const fn rule(self) -> Option<&'a RecurrenceRule>;
    pub const fn rdates(self) -> &'a [Instant];
    pub const fn exdates(self) -> &'a [Instant];
    pub const fn overrides(self) -> OverrideSet<'a>;
    pub fn until_value_type_agrees(self) -> bool;
}
```

Borrowed throughout and `Copy`: a caller holding a parsed document already owns all of this,
and the search allocates nothing per occurrence. Sortedness is required rather than repaired,
because a search that quietly sorted would be hiding an allocation and an `O(n log n)` inside a
call the caller was told costs a linear merge.

`until_value_type_agrees` is a predicate, not a constructor check. `UNTIL` disagreeing with
`DTSTART` about `DATE` versus `DATE-TIME` violates §3.3.10 and is emitted by half the clients
in the corpus; refusing the component over it would discard a file, which ADR 0001 forbids.

### The search (ADR 0002, ADR 0010)

```rust
pub const DEFAULT_CANDIDATE_BUDGET: u64 = 65_536;

pub struct Window { /* private */ }
impl Window {
    pub fn new(start: Instant, end: Instant) -> Option<Self>;
    pub const fn start(self) -> Instant;
    pub const fn end(self) -> Instant;
    pub fn contains(self, at: Instant) -> bool;
    pub fn widened(self, before: i64, after: i64) -> Option<Self>;
}

#[non_exhaustive]
pub enum OverrideProvenance {
    ExactMatch,
    ThisAndFuture { anchor: Instant },
    AddedByRdate,
}

pub struct Occurrence<'a> { /* private */ }
impl<'a> Occurrence<'a> {
    pub const fn new(key: Instant, start: Instant, provenance: Option<OverrideProvenance>,
                     exact: Option<&'a Override<'a>>, overrides: OverrideSet<'a>) -> Self;
    pub const fn key(self) -> Instant;
    pub const fn start(self) -> Instant;
    pub const fn provenance(self) -> Option<OverrideProvenance>;
    pub fn is_moved(self) -> bool;
    pub fn shift_seconds(self) -> Option<i64>;
    pub fn starts_within(self, window: Window) -> bool;
    pub const fn applied_anchors(self) -> AppliedDiffs<'a>;
    pub const fn exact_override(self) -> Option<&'a Override<'a>>;
    pub fn effective_change(self, id: &PropertyId) -> Option<PropertyChange<'a>>;
}

pub struct BudgetExhausted { /* private */ }
impl BudgetExhausted {
    pub const fn new(reached: Instant, candidates_spent: u64) -> Self;
    pub const fn reached(self) -> Instant;
    pub const fn candidates_spent(self) -> u64;
}
impl core::error::Error for BudgetExhausted {}

#[non_exhaustive]
pub enum SearchOutcome {
    Pending, RuleEnded, WindowEnded, CalendarEnded, BudgetExhausted(BudgetExhausted),
}

pub struct RuleCursorState { /* opaque */ }
pub struct SearchCursor { /* private */ }
impl SearchCursor {
    pub const fn new(resume_after: Instant, occurrences_emitted: u32,
                     rule_cursor: RuleCursorState) -> Self;
    pub const fn resume_after(self) -> Instant;
    pub const fn occurrences_emitted(self) -> u32;
    pub const fn rule_cursor(self) -> RuleCursorState;
}

#[non_exhaustive]
#[must_use]
pub enum SearchStep<'a> {
    Occurrence(Occurrence<'a>),
    BudgetExhausted(BudgetExhausted),
}
impl<'a> SearchStep<'a> {
    pub const fn occurrence(self) -> Option<Occurrence<'a>>;
    pub const fn is_terminal(self) -> bool;
}

pub struct RecurrenceSearch<'a, S: DiagnosticSink + ?Sized = dyn DiagnosticSink + 'a> {
    /* private */
}
impl<'a, S: DiagnosticSink + ?Sized> RecurrenceSearch<'a, S> {
    pub const fn outcome(&self) -> SearchOutcome;
    pub fn cursor(&self) -> SearchCursor;
}
impl<'a, S: DiagnosticSink + ?Sized> Iterator for RecurrenceSearch<'a, S> {
    type Item = SearchStep<'a>;
}
impl<S: DiagnosticSink + ?Sized> FusedIterator for RecurrenceSearch<'_, S> {}
```

The sink is a defaulted second type parameter because a struct with one lifetime parameter
cannot hold a `&'s mut S`. `RecurrenceSearch<'a>` still names the erased form, and a caller
passing a concrete sink gets it monomorphized with no vtable — which is the property the
`Expand` sketch below argues for, arriving here instead.

**`key` and `start` are not interchangeable, and this is the crate's sharpest invariant.**
`key` is the base cadence instant: what a `RECURRENCE-ID` addresses, what the merge sorts on,
what `COUNT` counts, and what generation walks. `start` is when the occurrence actually happens.
They differ exactly when an override moved it. A `Window` therefore admits on *either*: an
occurrence is emitted when the window the caller asked about contains its `key` **or** contains
its `start`. Start-only admission would drop an occurrence the caller can still address by
`RECURRENCE-ID`; key-only admission would lose one an override moved *into* the window. Both
halves are the library's work — generation runs over the asked window widened by the largest
absolute shift the override set implies, and the filter back down is inside the search — so
`Window::widened` is a tool a caller may reach for and never an obligation it has to discover.
An occurrence admitted by its key whose start lies outside is emitted and reported on
`DiagnosticCode::OverrideLeftWindow`, and `starts_within` is how a caller asks the second
question for itself. Emission is ordered by cadence key; see `docs/adr/0002`'s amendments 2
and 3.

`Occurrence` is `Copy` and borrows the override table, so `applied_anchors` and
`effective_change` recompose on demand and nothing is materialized per occurrence.
`effective_change` walks every anchor oldest to newest and then the exact match: a `LOCATION`
set in March survives a `SUMMARY`-only edit in June.

**The terminal state is reported three times, on purpose.** DP-09 fixed `Item =
Result<Occurrence, BudgetExhausted>` and that decision did not survive: `Result` implements
`IntoIterator`, so `search.flatten()` compiles against that exact item type and silently drops
every error; `.filter_map(Result::ok)` and `.take_while(Result::is_ok)` are indistinguishable
from it; and `for x in search {}` binds the whole `Result` in one irrefutable pattern and forces
no match at all. Each is a reviewed-without-comment one-liner that converts budget exhaustion
back into the truncated-but-plausible answer the budget exists to prevent. `SearchStep` is a
crate-owned enum, so none of the three compiles. The second report is the caller's `Meter`,
which `search` borrows rather than owns and which therefore outlives the iterator and every
combinator applied to it, and whose exhaustion flag latches. A reviewer who cannot find the
terminal arm can still find `meter.is_exhausted()`. `SearchOutcome` is the third and weakest,
available only to a caller who still holds the search by name. `Iterator::count()` remains the
honest remainder — it counts steps, so an exhausted search returns a number inflated by one —
and `SearchStep::occurrence` is deliberately not named `ok`.

### Traits whose bodies are the crate

```rust
pub trait RecurParser {
    fn parse_recur<S: DiagnosticSink + ?Sized>(value_text: &[u8], meter: &mut Meter,
                                               sink: &mut S)
        -> Result<RecurrenceRule, RuleError>;
}

pub trait OccurrenceStream<'a>: core::fmt::Debug {
    fn step(&mut self) -> Option<SearchStep<'a>>;
    fn outcome(&self) -> SearchOutcome;
    fn cursor(&self) -> SearchCursor;
}

pub trait Expand {
    fn search<'s, S: DiagnosticSink + ?Sized>(&'s self, window: Window, meter: &'s mut Meter,
                                              sink: &'s mut S) -> RecurrenceSearch<'s>;
    fn resume<'s, S: DiagnosticSink + ?Sized>(&'s self, cursor: SearchCursor, window: Window,
                                              meter: &'s mut Meter, sink: &'s mut S)
        -> RecurrenceSearch<'s>;
}
```

In the shipped crate `parse_recur`, `search` and `resume` are inherent methods on
`RecurrenceRule` and `RecurrenceInput`, and `OccurrenceStream` does not exist — the engine is a
private field. They are traits in the skeleton because their bodies *are* the milestone, and
the lint gate denies `todo!()`. Nothing about the caller-visible shape changes when they become
inherent: `Expand`'s two methods keep their signatures, and `RecurrenceSearch`'s field is
private either way.

`Expand` is deliberately not object-safe. The sink is a type parameter so a fixed-capacity sink
on a target with no heap to spare costs no vtable, and a search is a per-call thing nobody needs
behind a `dyn`.

`meter: &'s mut Meter` is the load-bearing part of that signature. One meter handed to 5,000
searches over a CalDAV multiget makes 5,000 individually bounded calls bounded in aggregate;
`Meter` being neither `Copy` nor `Default` makes minting a fresh one inside the loop a visible
act rather than an omission. Budget is charged per candidate **generated**, including candidates
materialized inside a single `step` while resolving a `BYSETPOS` period filter before position
selection, so one `next()` cannot perform unbounded uncharged work.

ADR 0010 says every hostile-input entry point takes `&Limits` *and* `&mut Meter`. These take
only the meter, because `ical-core` gave `Meter` a `Limits` field and a `Meter::limits()`
accessor, so a second parameter would be a second copy of the same policy with nothing keeping
the two equal. The ADR's intent — one policy, one running ledger, both visible in the signature
— is met; its literal parameter list is not, and this is the one place this design departs from
it. `Limits::DEFAULT.candidates_per_period()` bounds one period; `Meter`'s budget bounds the
whole search and every other search sharing the meter.

## Type to specification

| Type | Serves |
| --- | --- |
| `Freq`, `RecurrenceRule`, `ByList`, `RuleLimit` | §3.3.10, the `RECUR` value type |
| `Weekday`, `WeekdayNum` | §3.3.10, `BYDAY` and `WKST` |
| `RuleLimit::Count`, `RuleLimit::Until` | §3.3.10 `COUNT`/`UNTIL`; §3.8.5.3 recurrence set |
| `ValueKind` | §3.3.4 `DATE`, §3.3.5 `DATE-TIME`; the `UNTIL` agreement rule in §3.3.10 |
| `RecurrenceRuleBuilder`, `RuleError` | §3.3.10 well-formedness |
| `RecurParser` | §3.3.10 decoding, over §3.1 unfolded content lines |
| `RecurrenceInput::rdates` | §3.8.5.2 `RDATE` |
| `RecurrenceInput::exdates` | §3.8.5.1 `EXDATE` |
| `Override`, `OverrideSet`, `PropertyDiff`, `PropertyChange` | §3.8.4.4 `RECURRENCE-ID` |
| `OverrideRange`, `AppliedDiffs` | §3.2.13 `RANGE`, and §3.8.4.4's `THISANDFUTURE` |
| `OverrideProvenance` | §3.8.4.4 and §3.8.5.2 — which mechanism produced this instance |
| `Occurrence::key` | §3.8.4.4: a `RECURRENCE-ID` names the *original* start |
| `Occurrence::start` | §3.8.2.4 `DTSTART`, as restated by an override |
| `RecurrenceInput`, `RecurrenceSearch` | §3.8.5.3, the recurrence set as a whole |
| `Window`, `SearchCursor`, `BudgetExhausted`, `SearchOutcome` | no RFC section: ADR 0002/0010 |

The last row is the honest one. Bounding is not in RFC 5545; the specification describes an
infinite series and leaves the consequences to implementers. These four types are this
project's answer, not the RFC's.

## Diagnostics this crate raises

`DiagnosticCode` lives in `ical-core` and is semver-frozen against a committed golden list
(DP-06). `ical-core` already carries the first two; the last four are additions this design
asks for, and each needs a golden-list entry before it is used. Diagnostics from this crate are
built with `Diagnostic::at_instant`, because a rule-generated instant exists at no byte offset
in the input.

| Code | Raised when |
| --- | --- |
| `RecurUntilValueTypeMismatch` | `UNTIL` and `DTSTART` disagree about `DATE`/`DATE-TIME` |
| `RecurBySetPosWithoutByRule` | `BYSETPOS` with no other `BYxxx` part |
| `RecurInvalidInstanceSkipped` | a generated instance names a date that does not exist |
| `RecurExdateShadowsOverride` | an `EXDATE` and a `RECURRENCE-ID` name the same instant |
| `RecurOverrideLeftWindow` | a spliced override moved a `start` outside the window |
| `RecurExtraRuleIgnored` | more than one `RRULE` was offered for one component |

`RecurInvalidInstanceSkipped` records a skip, not a clamp. `FREQ=MONTHLY;BYMONTHDAY=31` has no
February instance; §3.3.10 says such instances MUST be ignored, and DP-12 makes skip-not-clamp
binding on this crate's generation path. The skipped candidate is still charged to the meter,
because it was still generated.

`RecurExdateShadowsOverride` accompanies a policy choice with no RFC behind it: **`EXDATE`
wins**. An intentional deletion beats a modification that collided with it. Applied to a
`THISANDFUTURE` anchor, the exclusion removes *that instance only* — the anchor's diff stays in
force for every later candidate. Reading "the override is dropped" as "the object is dropped"
would let one duplicate `EXDATE` line, of the kind several exporters have historically emitted,
silently revert an unbounded tail of the series. That reading is rejected here explicitly
rather than left to whoever writes the merge.

## Deliberately rejected

- **A function that expands a rule into a `Vec`.** ADR 0002. `FREQ=SECONDLY` with no `UNTIL` is
  legal and this is the whole point of the crate. Callers who want one write it with a window
  they chose.
- **A non-`Iterator` cursor plus a `Bounded` wrapper carrying a `stopped` field** (DP-09's
  runner-up, and the higher-scoring one). Moving the wrapper into a `for` loop by value discards
  the field, which reproduces exactly the silent exhaustion the design exists to prevent.
- **A scalar time delta for `RANGE=THISANDFUTURE`** (DP-10's runner-up, adopted by five of seven
  proposals). §3.8.4.4 places no restriction on which properties a `THISANDFUTURE` override may
  change; an organizer moving a recurring meeting to a new room without moving its time is
  ordinary, and a delta cannot represent it.
- **A single active anchor per instant.** The provenance tag names the nearest anchor, but
  application composes every anchor at or before the key. One tag driving one diff cannot
  represent a series edited "this and following" twice, and that is the more common lifecycle,
  not the exotic one.
- **Sorting caller-supplied `RDATE`/`EXDATE` lists in place.** Rejecting an unsorted list keeps
  the merge's advertised linear cost true. Sorting would hide an allocation on a `no_std` path.
- **More than one `RRULE` per component.** §3.8.5.3 says `SHOULD NOT`, RFC 2445 allowed it, and
  the adopted `SearchCursor` carries one `occurrences_emitted` counter. Supporting *N* rules
  needs *N* cursors and makes `COUNT` ambiguous across the union. Extra rules are dropped with
  `RecurExtraRuleIgnored` rather than silently merged.
- **`EXRULE`.** Obsoleted by RFC 5545, still found in old files. `ical-core` preserves it and
  round-trips it; this crate does not interpret it.
- **A caller-supplied override-index trait** for database-backed or paged override stores.
  Named and deferred by DP-10 rather than silently unconsidered. A caller with 50,000 overrides
  must still flatten them into one borrowed sorted slice.
- **A serializable `SearchCursor`.** `RuleCursorState` is a position in an expansion algorithm,
  not a fact about the calendar. Freezing its encoding would freeze the algorithm, and the
  purity gate forbids the `serde` dependency that would tempt someone to try.
- **Calling `ical-tz` from inside the merge.** DP-17's spine makes the two crates siblings.
  Normalization is the caller's step, performed once, before `RecurrenceInput::new`.

## Feature flags

`ical-recur` declares **no** Cargo features, and `default = []` is asserted by CI so that
`cargo check` and `cargo check --no-default-features` cannot diverge. Each candidate was
considered and each is rejected by a decision already adopted:

- **`alloc`** — would gate `extern crate alloc`. DP-01 makes alloc mandatory for the five core
  crates, not optional. A crate that is `no_std+alloc` in one configuration and `no_std` in
  another has two APIs and one name.
- **`no-alloc`** — would offer a fixed-capacity tier for hard-real-time targets. DP-01 names
  that tier as a deferred gap needing **its own crate** and its own lint profile, and says
  explicitly that it must not be a feature flag on this one.
- **`std`** — would add `impl std::error::Error`. Unnecessary: MSRV is 1.85 and
  `core::error::Error` is stable at 1.81, so `RuleError`, `InputError` and `BudgetExhausted`
  implement it unconditionally.
- **`serde`** — would derive on `RecurrenceRule` and `SearchCursor`. The purity gate (DP-18)
  permits zero external dependencies in `[dependencies]`, `[dev-dependencies]` and
  `[target.'cfg(..)'.dependencies]` alike. `SearchCursor` should not be serializable anyway.
- **`strict`** — would reject specification violations instead of reporting them. Two behaviors
  for one input makes ADR 0006's corpus results incomparable between builds. Strictness is a
  caller's reading of the diagnostics, not a compile-time mode.

The cost of this is real and worth stating: a caller who wants a smaller binary has no dial to
turn. The answer is that there is nothing here to gate — no optional back end, no alternative
representation, no bundled data. If that stops being true, this table is where the argument
starts.

## Usage

All four are compiled as `examples::*` in the skeleton.

### Building a rule and an input

```rust
pub fn first_monday_rule() -> Result<RecurrenceRule, RuleError> {
    let monday = WeekdayNum::new(None, Weekday::Monday).ok_or(RuleError::OrdinalOutOfRange)?;
    RecurrenceRuleBuilder::new(Freq::Monthly)
        .by_day(ByList::from_slice(&[monday]))
        .by_set_pos(ByList::from_slice(&[1_i16]))
        .build()
}

pub fn assemble<'a>(
    dtstart: Instant,
    rule: &'a RecurrenceRule,
    exdates: &'a [Instant],
    overrides: &'a [Override<'a>],
) -> Result<RecurrenceInput<'a>, InputError> {
    let table = OverrideSet::new(overrides)?;
    RecurrenceInput::new(dtstart, ValueKind::DateTime, Some(rule), &[], exdates, table)
}
```

### A month view that knows whether it is complete

```rust
pub fn month_view<E: Expand, S: DiagnosticSink + ?Sized>(
    series: &E,
    window: Window,
    limits: Limits,
    sink: &mut S,
) -> MonthView {
    let mut meter = Meter::with_budget(limits, DEFAULT_CANDIDATE_BUDGET);
    let (starts, outcome) = {
        let mut search = series.search(window, &mut meter, sink);
        let mut collected = Vec::new();
        for item in search.by_ref() {
            match item {
                Ok(occurrence) => collected.push(occurrence.start()),
                Err(exhausted) => {
                    let _unused = exhausted.candidates_spent();
                    break;
                },
            }
        }
        (collected, search.outcome())
    };
    MonthView { starts, outcome, truncated: meter.is_exhausted() }
}
```

The `Err` arm, `search.outcome()` and `meter.is_exhausted()` are three reports of one fact.
The third is the one that survives a later refactor into `.filter_map(Result::ok)`.

### Paging an agenda under one aggregate budget

```rust
pub fn page<E: Expand, S: DiagnosticSink + ?Sized>(
    series: &E,
    from: Option<SearchCursor>,
    window: Window,
    meter: &mut Meter,
    sink: &mut S,
) -> Page {
    let mut search = match from {
        Some(cursor) => series.resume(cursor, window, meter, sink),
        None => series.search(window, meter, sink),
    };
    let mut starts = Vec::new();
    let mut full = false;
    for item in search.by_ref() {
        match item {
            Ok(occurrence) => {
                starts.push(occurrence.start());
                if starts.len() >= 25 {
                    full = true;
                    break;
                }
            },
            Err(_) => break,
        }
    }
    let resume = if full { Some(search.cursor()) } else { None };
    Page { starts, resume }
}
```

The caller owns the meter across pages, so asking again does not buy more budget. `SearchCursor`
carries `occurrences_emitted`, so a resumed `COUNT`-bounded rule produces the recurrence set the
file describes rather than a fresh count from the resume point.

### Auditing what an override did to a window

```rust
fn fold_one(mut tally: Relocations, occurrence: Occurrence<'_>, window: Window) -> Relocations {
    if !occurrence.starts_within(window) {
        tally.escaped = tally.escaped.saturating_add(1);
    }
    if matches!(occurrence.provenance(), Some(OverrideProvenance::ThisAndFuture { .. })) {
        tally.anchored = tally.anchored.saturating_add(1);
    }
    if occurrence.effective_change(&PropertyId::LOCATION).is_some() {
        tally.relocated = tally.relocated.saturating_add(1);
    }
    tally
}
```

`escaped` counts occurrences the window admitted by key and would not have admitted by start.
The search already widened generation and filtered back, so a renderer does not have to; what
`escaped` names is the occurrence whose start the caller did not ask about, which the search
also reported on `DiagnosticCode::OverrideLeftWindow`.

## Consequences

A window admits by key or by start, and no amount of documentation makes two questions feel
like one. A caller who reads "occurrences between these two instants" and gets one starting an
hour after the second has been surprised by a correct answer, which is the worst kind — the
answer is correct because that occurrence is still addressable by `RECURRENCE-ID` inside the
window, and `DiagnosticCode::OverrideLeftWindow` is what says so out loud. The widening is the
library's, and `max_absolute_shift` computes the number `Window::widened` needs, so a caller
that wants start-only admission can express it rather than having to derive it.

`Item = SearchStep<'a>` replaces the `Result` DP-09 adopted, because `Result`'s own
`IntoIterator` makes `.flatten()` discard every terminal marker. The meter is kept beside it
because DP-09's stress testing was right that one report is not enough, and `SearchOutcome` is
the third. Three reports of one fact is a design that admits its primary mechanism is leaky.
`.count()` on an exhausted search still returns a number inflated by its terminal step, and
nothing here prevents that. M1's conformance sweep found that the second report was carrying
only the octet ledger, so a search stopped by `Limits::candidates_per_period` or by
`Limits::occurrences_per_search` left `meter.is_exhausted()` reading clean beside a truncated
answer; every bound the ledger keeps latches it now, at the price that one runaway series in a
fan-out over a shared meter stops the fan-out rather than being paid for by the ones after it.

`SearchOutcome::CalendarEnded` is the fourth terminal state and the one nothing above predicted.
A rule with neither `COUNT` nor `UNTIL` does not end; the four-digit year of RFC 5545 section
3.3.4 does. That answer is complete — nothing a second search could reach is missing — and it is
not `RuleEnded`, which is a claim about the rule that would be false. It carries
`DiagnosticCode::RecurrenceCalendarEnded` so a caller that kept only the sink still learns it.

`OverrideProvenance` is one tag where three facts sometimes apply. An `RDATE`-added instant
that an exact-match override also modifies reports `ExactMatch`, and the `RDATE` origin is lost
from the tag — recoverable only by the caller checking the `RDATE` slice. The precedence
`ExactMatch > ThisAndFuture > AddedByRdate` is stated so implementations agree, not because it
is the only defensible order. `AddedByRdate` also sits in an enum named `OverrideProvenance`
while being no kind of override; that name comes from the adopted decision and is kept so the
six crate designs agree on it.

`RecurrenceInput` holds at most one `RRULE`, so a file with two is served incompletely and told
so through a diagnostic. RFC 2445 permitted two and files with two exist. This is a scope cut
made to keep the adopted single-counter cursor coherent, and it is the first thing that will
need reopening if the corpus turns up real ones.

Nothing here has been run. The `THISANDFUTURE` diff splice, the anchor composition, and the
`EXDATE`-versus-anchor collision are all mechanisms with zero blind convergence behind them, and
the decision record says to expect the first draft of each to be wrong. The mandatory fixtures
are named in DP-10's gate change, and they should exist before any of this is called correct.

## Open questions for the integrator

This design was written against the `ical-core` skeleton as landed, so three of the four seams
it started with are already closed: `Instant`, `Limits`, `Meter`, `PropertyId`, `Weekday`,
`Diagnostic::at_instant`, `Limits::candidates_per_period` and `Limits::override_entries` all
exist there with the members used here. What remains:

1. **Four `DiagnosticCode` variants have to be added and golden-listed.**
   `RecurBySetPosWithoutByRule`, `RecurExdateShadowsOverride`, `RecurOverrideLeftWindow` and
   `RecurExtraRuleIgnored`. `ical-core` already carries `RecurUntilValueTypeMismatch` and
   `RecurInvalidInstanceSkipped`. Each addition needs a `code -> meaning` line before it is
   raised, or the CI freeze that makes ADR 0006's assertions durable has nothing to freeze.
2. **`&Limits` was dropped from the entry points.** ADR 0010's parameter list says both;
   `ical-core` put `Limits` inside `Meter`. One of the two documents should move, and this is a
   workspace call rather than this crate's. Every other crate that takes untrusted input faces
   the same choice and they should all make it the same way.
3. **`ValueKind` versus `ical_core::ValueType`.** This crate declares a two-variant enum where
   `ical-core` has a fourteen-variant one. That is a deliberate narrowing, but it means two
   spellings of `DATE` exist in the workspace, and if `ical-core` later offers its own narrowed
   date-or-date-time type this one should be deleted rather than kept beside it.
4. **`DEFAULT_CANDIDATE_BUDGET` is a placeholder.** ADR 0010 says the numbers are not chosen and
   assigns calibration to whoever ships the first recurrence milestone. That is this crate, and
   the number is currently 65,536 for no better reason than that it is a round one.

## What the first compile changed

Nothing in this crate. The `core_placeholders` module was deleted, `ical_core` was named in its
place, and the file compiled unchanged — the only skeleton of the six for which that was true.
The stand-ins had been written from `ical-core`'s own document rather than from what this crate
wished existed, and that is the whole difference.

Two things this crate reads did move underneath it. `Instant` now ships from `ical-grammar`,
because the diagnostic vocabulary sits at the bottom of the stack and a diagnostic may name an
occurrence; `ical-core` re-exports it, so the name in this crate's signatures is unchanged.
`LimitExceeded` became an enum naming the dimension that ran out. The candidate budget is
untouched: it is still `Limits::candidates_per_period`, still charged per candidate generated, and
still reported as an outcome rather than an error.

## What M1 shipped

The engine behind the surface above was built in M1 and is no longer a proposal. Four things
this document left open or stated wrongly are settled here; `docs/adr/0002`'s Amendments carry
the argument, and this section carries the consequence for the surface.

**The four open questions above are closed.** (1) The diagnostic codes exist and are golden-
listed under `ical-core`'s own naming — `by-set-pos-without-by-rule`, `exdate-shadows-override`,
`override-left-window`, `extra-recurrence-rule-ignored` — together with four this document did
not anticipate: `malformed-recurrence-rule`, `duplicate-recurrence-rule-part`,
`unknown-recurrence-rule-part` and `recurrence-rule-part-out-of-range` for the lenient reading,
plus `mutually-exclusive-rule-parts` for a value carrying both `UNTIL` and `COUNT`
and `override-shift-not-representable` for a shift that leaves the timeline. (2) `&Limits` stays
dropped: `Limits` lives inside `Meter` and one argument carries both. (3) `ValueKind` stays, and
its deletion is still owed if `ical-core` narrows its own. (4) `DEFAULT_CANDIDATE_BUDGET` is
calibrated at 262,144 — four times `Limits::DEFAULT.candidates_per_period()`, because a search
budget equal to the period ceiling is one bound wearing two names.

**A period's own vocabulary is internal and stays internal.** The walk that produces one period
per `FREQ` step, the candidate set a period expands to, and the `BYSETPOS` selection over it are
`Period`, `PeriodWalk`, `CandidateSet` and `SelectedCandidates`. They are on the crate's public
surface today because the modules holding them are private and `unreachable_pub` is denied, and
that is an integration artifact rather than a promise: `ical_core::Period` is RFC 5545 section
3.3.9's PERIOD *value type* and means something else entirely, so a caller glob-importing both
crates sees two `Period`s. Anything published from these four types before the surface is
narrowed should expect to move.

**A period anchor is the normalized start of the period, and February is a period.** A
`FREQ=MONTHLY` walk from January 31 yields anchors on the 1st of each month, so it reaches March
without ever producing February 28 *and* without deleting February — `FREQ=MONTHLY;BYMONTHDAY=1`
under that same `DTSTART` has a February instance, and a walk that skipped the month leaves
nothing downstream able to recover it. A period's extent is one `FREQ` unit and `INTERVAL` is
the distance between two anchors, so `FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=1,15` recurs twice in
January and not at all in February; a two-month-wide period would put February's candidates in
January's set, where `BYSETPOS=-1` would then pick the wrong one.

**The merge's two undecided cases are decided.** An `RDATE`-added instant colliding on the same
effective start as a diff-moved one is *not* deduplicated — identity here is the cadence key,
the two have different keys, and fusing them leaves one addressable and the other silently gone
from a file that names it. And an anchor's stated time *shift* does not reach an instant an
`RDATE` named, while its property *diff* does: an `RDATE` value is a literal instant the file
states, with no cadence in it to shift, and shifting it would render a meeting at an hour no
line of the file contains. Both are argued at length in `crates/ical-recur/src/merge.rs`.

**Every worked example in RFC 5545 section 3.8.5.3 is a test.** All forty-two of them, in
`crates/ical-conform/tests/rfc5545_recurrence_examples.rs`, assembled through this crate's
public surface only, with the expected column transcribed from the RFC rather than read off the
implementation. One of them — "Every other year on January, February, and March for 10
occurrences" — is what caught the omission that the recurrence set begins at `DTSTART`.
