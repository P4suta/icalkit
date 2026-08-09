# `ical-tz` API design

- Status: proposed
- Date: 2026-08-10
- Decisions honored: DP-11, DP-12 (crate), DP-01, DP-08, DP-17, DP-18 (workspace)
- Skeleton: assembled with the other five into one workspace and compiled together; see
  "What the first compile changed" below

## Responsibility

`ical-tz` answers one question and refuses to answer it silently: given a `TZID` and a
wall-clock time, what instant is that, and who says so. It interprets `VTIMEZONE` — the
`STANDARD` and `DAYLIGHT` observances a calendar carries with it — into a transition table,
and it accepts any other zone database the caller wires in behind one object-safe trait. It
bundles no time zone data, reads no clock, and prefers neither source when they disagree:
the hour that repeats, the hour that does not exist, an identifier nobody recognizes, and a
`VTIMEZONE` that ran out of transitions eight years before the event are all values a caller
inspects, not errors that throw the file away and not defaults chosen out of sight. Anything
above that line — expanding an `RRULE`, deciding whether a nonexistent instance is skipped
or shifted, mapping `W. Europe Standard Time` onto an IANA name — belongs to a caller or to
a crate above this one, because each of those needs either data this crate will not bundle
or a policy this crate has no standing to pick.

## Where the types live

One integration decision has to be stated before the surface makes sense. DP-12 requires
concrete `Instant`, `CivilDateTime` and resolution types with checked-only arithmetic, and
files that requirement under `ical-tz`. But DP-17's adopted spine makes `ical-recur` and
`ical-tz` siblings under `ical-core`, and DP-09 puts `Instant` inside `ical-recur`'s
`SearchCursor` while DP-12 puts `MonthAddOutcome` on `ical-recur`'s monthly expansion path.
A primitive both siblings need cannot live in one of them.

So: **the civil-time primitives are defined in `ical-core` and re-exported by `ical-tz`,
which owns their specification.** `CivilDate`, `CivilTime`, `CivilDateTime`, `Instant`,
`UtcOffset`, `Duration`, `Weekday` and `MonthAddOutcome` are declared by ADR-0011 (this
crate's ADR) and compiled into `ical-core` (the shared root). Everything zone-shaped —
`ZoneSource`, `ZoneAnswer`, `LocalResolution`, `ZoneProvenance`, `Coverage`,
`CombinedZoneSource`, `PolicyOutcome`, `Tzid`, `Observance`, `YearlyRule`,
`TransitionTable`, `VtimezoneSet` — is `ical-tz`'s and appears nowhere else. In the skeleton
the `ical-core` half sits in a module named `upstream` purely so the file compiles alone;
in the real crate those lines are `pub use ical_core::{..}`.

## The public surface

### Answering: `ZoneSource`

```rust
pub trait ZoneSource {
    fn resolve(&self, tzid: &str, local: CivilDateTime) -> Option<ZoneAnswer>;
    fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer>;
}
```

Object-safe by construction — `&self` in, an owned answer out, no generic parameter, no
associated type — because combining an embedded `VTIMEZONE` with an IANA database is a
runtime wiring choice made once, not a compile-time one. No `Send`/`Sync` bound: a server
whose concrete source is both still gets an `Arc` of a `Send + Sync` trait object for free,
and an embedded caller that wants no vtable is free to hold the concrete type instead.

The invariant that carries the most weight is on the return type. **`None` means exactly one
thing: this source does not recognize this identifier.** It never means "recognized, but I
have no data for that time" — that is `Coverage`'s job — and it never licenses an
implementation to invent an answer. A source handed `W. Europe Standard Time` with no CLDR
table returns `None` and lets the hole be visible, which is what stops the alias mapping
from becoming a fallback chain buried inside somebody's `impl`.

`resolve` takes a `CivilDateTime` rather than DP-11's literal `DateTimeValue` because the
caller with the most resolutions to do is `ical-recur`, normalizing generated candidates
that borrow from no property. `ical-core`'s typed `DTSTART` view decomposes into exactly
this pair, so nothing is lost at the seam.

### The three states of a local time

```rust
#[non_exhaustive]
pub enum LocalResolution {
    Single    { instant: Instant, offset: UtcOffset, daylight: bool },
    Ambiguous { first: Instant, first_offset: UtcOffset,
                second: Instant, second_offset: UtcOffset },
    Gap       { gap_start: Instant, gap_end: Instant,
                offset_before: UtcOffset, offset_after: UtcOffset, shifted: Instant },
}
```

Invariants: `Single` is the only variant `unambiguous()` returns from; `Ambiguous.first <
Ambiguous.second` always, and `first_offset != second_offset`; `Gap.gap_start <
Gap.gap_end`, `offset_before != offset_after`, and `shifted` is the instant the queried
local time denotes when read with `offset_before`.

That last field is the crate's answer to an internal contradiction in RFC 5545 that a
library is not entitled to settle. Section 3.3.10 says a recurrence instance falling on a
nonexistent local time MUST be ignored; section 3.3.5 says an explicit `DATE-TIME` in a gap
is read with the offset in force before it, which is what Google and Apple do in practice.
`Gap` therefore hands the caller the material for both readings and picks neither: skip it,
or take `shifted`. Deciding for the caller is how one participant's meeting moves an hour
and another's does not.

### Provenance, and how much data stood behind an answer

```rust
pub struct ZoneAnswer  { pub resolution: LocalResolution, pub source: ZoneProvenance }
pub struct OffsetAnswer{ pub offset: UtcOffset, pub daylight: bool, pub source: ZoneProvenance }
pub struct ZoneProvenance { pub source: SourceKind, pub coverage: Coverage }

#[non_exhaustive] pub enum SourceKind { EmbeddedVtimezone, CallerDatabase, FixedOffset }
#[non_exhaustive] pub enum Coverage {
    Covered,
    Extrapolated { nearest_known: CivilDate },
}
```

`ZoneProvenance` and the answer structs have public fields and are not `#[non_exhaustive]`,
because callers implement `ZoneSource` and must be able to construct them. The two enums are
`#[non_exhaustive]`, so a new source kind or a new coverage class is not a breaking change.

`Coverage` exists because "which source answered" is not the same fact as "how much did that
source actually know". A `VTIMEZONE` whose transitions are three `RDATE` lines through 2029,
referenced by an event in 2035, has no data for 2035; continuing its last observance is a
reasonable thing to do and a dishonest thing to do quietly. An extrapolated answer that
happens to match a rule-derived one would otherwise read as confident corroboration by two
independent sources, which is precisely the silent-fallback shape ADR-0003 rejects, moved
from *which source won* to *how much the winner knew*.

### Two sources, one stated policy

```rust
#[non_exhaustive]
pub enum PolicyOutcome<A = ZoneAnswer> {
    Agreed    { embedded: A, fallback: A },
    Disagreed { embedded: A, fallback: A },
    OnlyEmbedded(A),
    OnlyFallback(A),
    Neither,
}

pub struct CombinedZoneSource<'a, E: ?Sized, F: ?Sized> { /* &'a E, &'a F */ }

impl<'a, E: ZoneSource + ?Sized, F: ZoneSource + ?Sized> CombinedZoneSource<'a, E, F> {
    pub const fn new(embedded: &'a E, fallback: &'a F) -> Self;
    pub fn resolve(&self, tzid: &str, local: CivilDateTime) -> PolicyOutcome<ZoneAnswer>;
    pub fn offset_at(&self, tzid: &str, instant: Instant) -> PolicyOutcome<OffsetAnswer>;
}
```

Invariants: both sources are queried on every call, unconditionally, before the outcome is
formed — there is no short circuit and no operand ordering that skips work. `Agreed` implies
the two `LocalResolution` values compared equal (for `offset_at`, the offset and the daylight
flag both matched); it does *not* imply their provenance matched, which is why both answers
survive into the variant. `OnlyEmbedded`/`OnlyFallback` mean the other source returned
`None`, that is, did not recognize the identifier — never that it disagreed. `Neither` means
nobody recognized it.

`CombinedZoneSource` deliberately does not implement `ZoneSource`. If it did, it would have
to collapse a disagreement into a single answer somewhere inside itself, and that decision
belongs to whoever is going to show it to a person. The generic form is dyn-capable rather
than dyn-mandating: `CombinedZoneSource::new(&table, &tzdb)` monomorphizes, and
`CombinedZoneSource::new(a, b)` over two `&dyn ZoneSource` does not.

### `VTIMEZONE`, interpreted

```rust
pub struct Observance {
    pub start: CivilDateTime, pub offset_from: UtcOffset, pub offset_to: UtcOffset,
    pub daylight: bool, pub rule: Option<YearlyRule>,
}
pub struct YearlyRule {
    pub month: u8, pub weekday: Weekday, pub week: NthWeek,
    pub at: CivilTime, pub through: Option<CivilDate>,
}
#[non_exhaustive] pub enum NthWeek { First, Second, Third, Fourth, Last }

impl YearlyRule { pub fn occurrence_in(self, year: u16) -> Option<CivilDate>; }

pub struct TransitionTable { /* Box<str>, Vec<Observance>, bool */ }
impl TransitionTable {
    pub fn new(tzid: Box<str>, observances: Vec<Observance>, limits: Limits) -> Self;
    pub fn tzid(&self) -> Tzid<'_>;
    pub fn observances(&self) -> &[Observance];
    pub fn is_truncated(&self) -> bool;
    pub fn coverage_end(&self) -> Option<CivilDate>;
}

pub struct VtimezoneSet { /* Vec<TransitionTable> */ }
impl VtimezoneSet {
    pub fn insert(&mut self, table: TransitionTable) -> Result<(), TransitionTable>;
    pub fn table(&self, tzid: &str) -> Option<&TransitionTable>;
}

pub trait ObservanceReader {
    fn read_vtimezone(&self, limits: Limits, sink: &mut dyn DiagnosticSink,
                      out: &mut Vec<Observance>) -> Option<Box<str>>;
}
```

Invariants: `observances()` is sorted by `start` and its length never exceeds
`Limits::max_vtimezone_observances`; `is_truncated()` is true exactly when observances were
dropped to hold that line. `coverage_end()` returns `None` when some observance repeats by a
rule with no `UNTIL` — that zone knows the future — and otherwise the last date backed by
real data, which is the value that turns a later query into `Coverage::Extrapolated`.
`YearlyRule::occurrence_in` is closed-form arithmetic over the weekday of the first of the
month: no loop, no search, therefore no candidate budget and no way to make a lookup do
unbounded work. `VtimezoneSet::insert` refuses a duplicate `TZID` by handing the table back,
so a document that declares one zone twice is a reported fact rather than a lost definition.

`Limits` (DP-08) is a mandatory argument on construction, where untrusted input is read, and
appears nowhere on the resolution path, because after construction the table is finite and
rule evaluation is O(1). `ObservanceReader` is a trait rather than an inherent constructor
for one reason: its body is real M2 work, and declaring a signature is honest where writing
a stub would not be. `ical-tz` implements it for `ical-core`'s `Component`.

### Identifiers

```rust
pub struct Tzid<'a>(/* &'a str */);
impl<'a> Tzid<'a> {
    pub const fn new(text: &'a str) -> Self;
    pub const fn as_str(self) -> &'a str;
    pub fn form(self) -> TzidForm;
    pub fn strip_global_prefix(self) -> Option<Self>;
}
#[non_exhaustive] pub enum TzidForm { GloballyUnique, IanaLike, Opaque }
```

`Tzid` classifies and never translates. `strip_global_prefix` removes the leading solidus of
a globally unique identifier, which is the only rewriting RFC 5545 section 3.2.19 licenses;
it does not try to find `Europe/Berlin` inside `/mozilla.org/20050126_1/Europe/Berlin`,
because that is a vendor convention and guessing at it is how a wrong zone gets applied
confidently. Comparison and lookup are by exact bytes as written.

### The arithmetic this crate specifies (DP-12)

```rust
impl CivilDate {
    pub const fn from_ymd(year: u16, month: u8, day: u8) -> Option<Self>;
    pub const fn is_leap_year(year: u16) -> bool;
    pub const fn days_in_month(year: u16, month: u8) -> Option<u8>;
    pub fn days_from_epoch(self) -> Option<i64>;
    pub fn weekday(self) -> Option<Weekday>;
    pub fn add_months(self, count: i32) -> MonthAddOutcome;
}
#[non_exhaustive]
pub enum MonthAddOutcome {
    Exact(CivilDate),
    Clamped { date: CivilDate, requested_day: u8 },
    Overflow,
}

impl CivilTime     { pub const fn from_hms(hour: u8, minute: u8, second: u8) -> Option<Self>; }
impl CivilDateTime { pub fn at_offset(self, offset: UtcOffset) -> Option<Instant>; }
impl Instant       { pub fn to_civil(self, offset: UtcOffset) -> Option<CivilDateTime>; }
impl UtcOffset     { pub const fn from_seconds(seconds: i32) -> Option<Self>; }
impl Duration      { pub const fn new(days: i32, seconds: i32) -> Option<Self>; }
```

Invariants: every operation is total — checked, `div_euclid`, or `rem_euclid` — so no path
can panic and none needs a lint exception, which is what
`clippy::arithmetic_side_effects` being a hard deny already required. `CivilDate` admits only
years RFC 5545 can write, `0..=9999`, which gives `MonthAddOutcome::Overflow` a meaning
narrower and more useful than integer overflow. `CivilTime` accepts a `second` of `60`
because section 3.3.12 does, and folds it onto `59` when converting to an `Instant`, so a
leap second round-trips through the preserved text without corrupting arithmetic. `Duration`
has `days` (nominal — the same wall time, that many days later) and `seconds` (exact), one
sign for the whole value, and no year or month field at all.

## Types against the specification

| Type | RFC 5545 section | What it serves |
| --- | --- | --- |
| `Tzid`, `TzidForm` | 3.2.19 `TZID` parameter, 3.8.3.1 `TZID` property | identifier as written |
| `TransitionTable`, `VtimezoneSet` | 3.6.5 `VTIMEZONE` | one document's zone definitions |
| `Observance` | 3.6.5 `STANDARD` / `DAYLIGHT` | one observance |
| `Observance` offsets | 3.8.3.3, 3.8.3.4 `TZOFFSETFROM`/`TZOFFSETTO` | the transition |
| `YearlyRule`, `NthWeek` | 3.8.5.3 `RRULE`, 3.3.10 `RECUR` (restricted) | rule-driven transitions |
| `Observance` (one per date) | 3.8.5.2 `RDATE` | date-driven transitions |
| `LocalResolution::Gap` / `Ambiguous` | 3.3.5 `DATE-TIME` form 3, 3.3.10 | the two awkward hours |
| `CivilDate`, `CivilTime`, `CivilDateTime` | 3.3.4, 3.3.12, 3.3.5 | wall-clock values |
| `Instant` | 3.3.5 `DATE-TIME` form 2 (UTC) | the timeline |
| `UtcOffset` | 3.3.14 `UTC-OFFSET` | an offset |
| `Duration` | 3.3.6 `DURATION` | a span, with no month field |
| `MonthAddOutcome` | 3.3.10 (invalid instances) | month arithmetic that can fail |
| `ZoneSource`, `ZoneAnswer` | — | ADR-0003: the source is the caller's |
| `PolicyOutcome`, `Coverage`, `ZoneProvenance` | — | ADR-0003: disagreement is reported |

The last two rows are deliberately blank on the left. RFC 5545 has nothing to say about
where a zone definition comes from or what to do when two of them disagree, and pretending
otherwise would be citing an authority that does not exist.

## Deliberately rejected

- **A bundled tzdb.** ADR-0003. It would freeze tzdata into a release, force one answer to
  a question that has three, and end `no_std`.
- **A bundled Windows-to-IANA (CLDR) alias table.** Same argument, smaller table. The crate
  reports `TzidForm::Opaque` and returns `None`; the caller, who already has CLDR or a
  server mapping, decides.
- **`CombinedZoneSource: ZoneSource`.** Convenient, and it would put a fallback chain right
  back inside an `impl` where nobody can see it.
- **An error channel on `ZoneSource::resolve`.** `Option` plus `Coverage` plus
  `PolicyOutcome` already distinguish *unknown identifier*, *thin evidence* and *sources
  differ*. A `Result` would invite implementations to report all three as one.
- **Depending on `ical-recur` to read `VTIMEZONE` rules,** and with it general `RRULE`
  support inside a `VTIMEZONE`. It would put a cycle in DP-17's spine. The restricted
  `YearlyRule` covers every rule tzdata generates; anything else becomes
  `VtimezoneRuleUnsupported` on the sink rather than a half-understood transition.
- **`Duration` with year or month fields.** RFC 5545 section 3.3.6's ABNF has no `Y` or `M`
  designator. Month arithmetic exists only as `CivilDate::add_months`, so "add `P1M`" cannot
  be written at all rather than being rejected at a call site.
- **Deciding section 3.3.5 against section 3.3.10 for the caller.** See `Gap.shifted`.
- **A meter or cache on `&self` resolution.** Interior mutability to charge a budget, or a
  memo table, would make lookups order-dependent and observable — and `HashMap` is banned
  workspace-wide for exactly that reason. Bounds here are structural instead.
- **`TZNAME` as a typed field.** It is display text with no effect on arithmetic; it stays
  in the preserved component per ADR-0001, where a round trip can return it untouched.
- **Any `now()`.** Not one function in this crate reads a clock, which is what makes "is
  this event in the past" a question with a testable answer.

## Feature flags

There is one, `vtimezone`, on by default. It compiles the `VTIMEZONE` half: `Observance`,
`YearlyRule`, `NthWeek`, `TransitionTable`, `VtimezoneSet`, `ObservanceReader` and the
`ZoneSource` implementation over a table. With it off, the crate is the trait, the answer
types, `Tzid`, `FixedOffsetSource` and `CombinedZoneSource` — the surface a caller needs when
the only zone data in play is its own, and a meaningful code-size saving on a target where
that matters. `ical-core` remains a dependency either way, because the shared value types
come from there.

That is the whole list, and the flags that were considered and refused matter as much:

- **No `std`.** Nothing here does I/O and nothing reads a clock; `core::error::Error` has
  been available without `std` since 1.81, under an MSRV of 1.85.
- **No `alloc`.** DP-01 makes an allocator mandatory for the core family; a genuinely
  allocation-free tier is a separate crate with its own lint profile, not a flag that makes
  this crate's surface change shape underneath a caller.
- **No `serde`.** It is an external dependency, and the five core crates may declare zero.
- **No `iana-aliases`, no `bundled-tzdata`.** See the rejections above.

Each flag doubles the matrix that DP-18's cross-target gate has to build for
`wasm32-unknown-unknown` and `thumbv7em-none-eabi`. One flag means four builds; that is the
budget.

## Using it

Every example below compiles against `skeletons/ical-tz.rs` as a downstream crate.

**Resolving a `DTSTART`, stating what to do about the two awkward hours.**

```rust
use ical_tz::{CivilDateTime, Instant, LocalResolution, ZoneSource};

pub enum GapPolicy { ShiftForward, Skip }

pub fn start_instant(
    source: &dyn ZoneSource,
    tzid: &str,
    local: CivilDateTime,
    gaps: GapPolicy,
) -> Option<Instant> {
    let answer = source.resolve(tzid, local)?;
    match answer.resolution {
        LocalResolution::Single { instant, .. } => Some(instant),
        // The hour repeated; a calendar shows the first one.
        LocalResolution::Ambiguous { first, .. } => Some(first),
        LocalResolution::Gap { shifted, .. } => match gaps {
            GapPolicy::ShiftForward => Some(shifted), // RFC 5545 section 3.3.5
            GapPolicy::Skip => None,                  // RFC 5545 section 3.3.10
        },
        // `LocalResolution` is `#[non_exhaustive]`; a later variant is not silently a gap.
        _ => None,
    }
}
```

**Wiring the embedded `VTIMEZONE` against the caller's database and reading the outcome.**

```rust
use ical_tz::{CivilDateTime, CombinedZoneSource, Instant, PolicyOutcome, ZoneSource};

pub enum ZoneNote {
    Confident, AgreedButExtrapolated, Disagreement, SingleSource, Unresolvable,
}

pub fn resolve_and_note(
    embedded: &dyn ZoneSource,
    fallback: &dyn ZoneSource,
    tzid: &str,
    local: CivilDateTime,
) -> (Option<Instant>, ZoneNote) {
    let combined = CombinedZoneSource::new(embedded, fallback);
    match combined.resolve(tzid, local) {
        PolicyOutcome::Agreed { embedded: near, fallback: far } => {
            let note = if near.source.coverage.is_extrapolated()
                || far.source.coverage.is_extrapolated()
            {
                ZoneNote::AgreedButExtrapolated
            } else {
                ZoneNote::Confident
            };
            (near.resolution.earliest(), note)
        }
        PolicyOutcome::Disagreed { embedded: near, .. } => {
            (near.resolution.earliest(), ZoneNote::Disagreement)
        }
        PolicyOutcome::OnlyEmbedded(answer) | PolicyOutcome::OnlyFallback(answer) => {
            (answer.resolution.earliest(), ZoneNote::SingleSource)
        }
        // `Neither`, and anything a later version adds: nothing to show and nothing to
        // pretend. `PolicyOutcome` is `#[non_exhaustive]`, so this arm is required.
        _ => (None, ZoneNote::Unresolvable),
    }
}
```

**Building a table from a `VTIMEZONE`'s rules, and asking a rule for a year directly.**

```rust
use ical_tz::{
    CivilDate, CivilDateTime, CivilTime, Limits, NthWeek, Observance, TransitionTable,
    UtcOffset, Weekday, YearlyRule,
};

pub fn us_eastern(limits: Limits) -> Option<TransitionTable> {
    let eastern = UtcOffset::from_seconds(-18_000)?;
    let daylight = UtcOffset::from_seconds(-14_400)?;
    let two_am = CivilTime::from_hms(2, 0, 0)?;
    let observances = alloc::vec![
        Observance {
            start: CivilDateTime::new(CivilDate::from_ymd(2007, 3, 11)?, two_am),
            offset_from: eastern,
            offset_to: daylight,
            daylight: true,
            rule: Some(YearlyRule {
                month: 3, weekday: Weekday::Sunday, week: NthWeek::Second,
                at: two_am, through: None,
            }),
        },
        Observance {
            start: CivilDateTime::new(CivilDate::from_ymd(2007, 11, 4)?, two_am),
            offset_from: daylight,
            offset_to: eastern,
            daylight: false,
            rule: Some(YearlyRule {
                month: 11, weekday: Weekday::Sunday, week: NthWeek::First,
                at: two_am, through: None,
            }),
        },
    ];
    // `coverage_end()` on the result is `None`: both rules run on, so no answer this table
    // ever gives will be marked `Extrapolated`.
    Some(TransitionTable::new(alloc::boxed::Box::from("America/New_York"), observances, limits))
}

pub fn spring_forward(year: u16) -> Option<CivilDate> {
    YearlyRule {
        month: 3, weekday: Weekday::Sunday, week: NthWeek::Second,
        at: CivilTime::midnight(), through: None,
    }
    .occurrence_in(year) // closed form: no search, no budget
}
```

**Normalizing an `EXDATE` list for `ical-recur` (DP-10, amendment (c)).**

```rust
use alloc::vec::Vec;
use ical_tz::{CivilDateTime, Instant, ZoneSource};

pub fn normalize_exdates(
    source: &dyn ZoneSource,
    tzid: &str,
    excluded: &[CivilDateTime],
    out: &mut Vec<Instant>,
) {
    for local in excluded {
        if let Some(instant) = start_instant(source, tzid, *local, GapPolicy::Skip) {
            out.push(instant);
        }
    }
    out.sort_unstable();
}
```

**Month arithmetic where the RFC has no opinion (an iTIP counter-proposal).**

```rust
use ical_tz::{CivilDate, MonthAddOutcome};

pub enum Reschedule { Moved(CivilDate), MovedToMonthEnd(CivilDate), Refused }

pub fn one_month_later(start: CivilDate) -> Reschedule {
    match start.add_months(1) {
        MonthAddOutcome::Exact(date) => Reschedule::Moved(date),
        MonthAddOutcome::Clamped { date, .. } => Reschedule::MovedToMonthEnd(date),
        // `Overflow`, and anything a later version adds.
        _ => Reschedule::Refused,
    }
}
```

## Where this design goes past the adopted decisions

Three payloads are richer than the adopted amendments spell out. Variant and type names are
unchanged, so a sibling crate matching on them still compiles; only the fields grew.

1. `LocalResolution`'s variants carry their offsets, and `Gap` carries `shifted`. DP-11
   pinned `{ Single(Instant), Ambiguous(Instant, Instant), Gap }`, which cannot express the
   RFC 5545 section 3.3.5 reading at all, so a caller obeying 3.3.5 would have to re-derive
   it from a second query against a source that may not agree with the first.
2. `ZoneProvenance` is a pair — kind and `Coverage` — rather than a bare source label. DP-11
   pinned provenance as "which source produced it", which cannot distinguish a computed
   answer from a clamp past the end of an `RDATE` list.
3. `PolicyOutcome::Agreed` keeps both answers instead of one. Collapsing them discards
   exactly the coverage asymmetry point 2 exists to surface.

`PolicyOutcome` also gained a type parameter defaulting to `ZoneAnswer`, so the same five
variants serve `offset_at`. `PolicyOutcome` on its own still reads as the adopted type.

## Consequences

Every caller pays for honesty. A client that just wants an instant writes a four-arm match
where a lesser library returns an `i64`, and `#[non_exhaustive]` means the wildcard arm is
permanent. Wiring two sources costs two lookups, always, by design — a caller who wants one
lookup uses one source and gets no disagreement reporting, which is the correct trade and
not a hidden one.

The dyn indirection nobody measured is still unmeasured: `&dyn ZoneSource` costs a vtable
hop per lookup on a Cortex-M-class target, and this design permits the concrete form but
provides no evidence about which a real embedded caller should choose.

Two gaps stay open with their names on them. First, `Coverage` reports *that* an answer was
extrapolated, not *how far* — a source continued one day past its data and one continued six
years, and they look alike to a caller unless it does the date arithmetic itself. Second,
this crate makes termination possible but does not guarantee it: `YearlyRule::occurrence_in`
never loops, but a caller searching for an instance that can never exist — `BYMONTH=2`
paired with `BYMONTHDAY=30`, where `add_months` reports `Clamped` in every year forever — is
relying on `ical-recur`'s candidate budget to stop, not on anything here. `MonthAddOutcome`
carries `requested_day` so that caller can at least say why it gave up. And nothing in this
document has yet been checked against real transition data; the whole disagreement mechanism
rests on `LocalResolution` being right about actual folds and gaps, which is a test suite
that does not exist yet, not a claim this design has earned.

## Open questions

1. **Placement of the civil-time primitives.** This document puts `CivilDate`, `Instant`,
   `UtcOffset`, `Duration` and `MonthAddOutcome` in `ical-core`. The `ical-core` and
   `ical-recur` designs must agree, or the graph acquires a cycle.
2. **`DateTimeValue` versus `CivilDateTime` on `ZoneSource::resolve`.** Settled here in
   favor of the value; needs `ical-core`'s typed-view design to expose the decomposition.
3. **The `DiagnosticCode` variants this crate contributes to DP-06's golden list.** Proposed:
   `VtimezoneRuleUnsupported`, `ObservancesTruncated`, `ZoneOffsetInvalid`,
   `NoObservanceDefined`, `DuplicateTimeZoneIdentifier`.
4. **Whether `Limits` is passed by value.** It is `Copy` and small, and `&Limits` trips
   `clippy::trivially_copy_pass_by_ref` at the workspace's 128-byte threshold. If `ical-core`
   grows `Limits` past that, every entry point in this crate changes shape.
5. **Whether `ical-itip` wants `PolicyOutcome` or a collapsed answer.** If it only ever needs
   one instant, the combinator may belong behind a caller-facing helper rather than in its
   scheduling path.

## What the first compile changed

Almost nothing, which is the useful result. The `upstream` module was deleted and the civil-time
primitives, the diagnostic vocabulary and the limits policy now arrive as
`pub use ical_core::{..}` re-exports, so a caller still names one crate for one concept. Two
adjustments were needed: the fields of `Limits` are private, so `limits.max_vtimezone_observances`
became `limits.max_vtimezone_observances()`, and `Instant::to_civil` is now
`CivilDateTime::from_instant`, since `Instant` sits below `ical-core` and inherent methods for it
cannot be written from above.

[ADR 0011](../adr/0011-civil-time-arithmetic-and-resolution-types.md) said these types live here.
They do not, and its decision text now says so: the crate graph makes `ical-recur` a sibling and
leaves `ical-dav` depending on `ical-core` alone, so a primitive all three speak cannot live in
one of them. What this crate owns is resolution — the whole of its subject and none of its
vocabulary.
