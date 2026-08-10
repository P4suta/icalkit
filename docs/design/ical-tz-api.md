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
`ZoneSource`, `ZoneAnswer`, `LocalResolution`, `ZoneProvenance`, `AnswerBasis`,
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

Two methods, not one, and the second is not a convenience. `resolve` goes from a wall clock to
an instant and is the hard direction, because a wall clock can name two instants or none.
`offset_at` goes the other way, where every instant has exactly one offset, and it is what a
caller needs to read a `Z`-terminated `UNTIL`, a `RECURRENCE-ID` or an override's own ends back
onto the clock the series is written in. A crate with only the first cannot project anything
into a zone, only out of one.

Object-safe by construction — `&self` in, an owned answer out, no generic parameter, no
associated type — because combining an embedded `VTIMEZONE` with an IANA database is a
runtime wiring choice made once, not a compile-time one. No `Send`/`Sync` bound: a server
whose concrete source is both still gets an `Arc` of a `Send + Sync` trait object for free,
and an embedded caller that wants no vtable is free to hold the concrete type instead.

The invariant that carries the most weight is on the return type. **`None` means exactly one
thing: this source does not recognize this identifier.** It never means "recognized, but I
have no data for that time" — that is `AnswerBasis`'s job — and it never licenses an
implementation to invent an answer. A source handed `W. Europe Standard Time` with no CLDR
table returns `None` and lets the hole be visible, which is what stops the alias mapping
from becoming a fallback chain buried inside somebody's `impl`.

`resolve` takes a `CivilDateTime` rather than DP-11's literal `DateTimeValue` because the
caller with the most resolutions to do is `ical-recur`, normalizing generated candidates
that borrow from no property. `ical-core`'s typed `DTSTART` view decomposes into exactly
this pair, so nothing is lost at the seam.

### The three states of a local time

```rust
pub struct Reading { pub instant: Instant, pub offset: UtcOffset, pub daylight: bool }

#[non_exhaustive]
pub enum LocalResolution {
    Unique      { reading: Reading },
    Ambiguous   { earlier: Reading, later: Reading },
    Nonexistent { gap_start: Instant, gap_end: Instant,
                  offset_before: UtcOffset, offset_after: UtcOffset, shifted: Instant },
}

impl LocalResolution {
    pub const fn unambiguous(self) -> Option<Instant>;
    pub const fn earliest(self) -> Option<Instant>;
    pub const fn pick(self, gaps: GapPolicy, folds: FoldPolicy) -> Option<Instant>;
    pub const fn diagnostic_code(self) -> Option<DiagnosticCode>;
}

#[non_exhaustive] pub enum GapPolicy  { Skip, ShiftForward, ClampToTransition }
#[non_exhaustive] pub enum FoldPolicy { Earlier, Later }
```

A `Reading` is the triple that never comes apart — an instant, the offset that produced it,
and whether the observance in force is the zone's daylight one — so that a variant holding two
of them cannot pair the second instant with the first offset. The daylight flag is read off
`DAYLIGHT` against `STANDARD` and never inferred from which offset is larger, because
`Australia/Lord_Howe` runs `+10:30` standard and `+11:00` daylight and Ireland's is inverted.

Invariants: `Unique` is the only variant `unambiguous()` returns from; `Ambiguous.earlier <
Ambiguous.later` always, and their offsets differ; `Nonexistent.gap_start < gap_end`,
`offset_before != offset_after`, and `shifted` is the instant the queried local time denotes
when read with `offset_before`.

That last field is the crate's answer to an internal contradiction in RFC 5545 that a
library is not entitled to settle. Section 3.3.10 says a recurrence instance falling on a
nonexistent local time MUST be ignored; section 3.3.5 says an explicit `DATE-TIME` in a gap
is read with the offset in force before it, which is what Google and Apple do in practice.
`Nonexistent` therefore hands the caller the material for both readings and picks neither: skip
it, take `shifted`, or clamp to the transition. Deciding for the caller is how one participant's
meeting moves an hour and another's does not.

The three collapses are named once, on `pick`, rather than written out at each call site, so
that two units cannot invent two conventions for the same fact. A gap has no width on the UTC
timeline — the clock moves at a single instant — so `gap_end` is that instant, the first the new
offset governs, and `gap_start` is the second before it; `GapPolicy::ClampToTransition` is the
only reader of either and lands an occurrence as soon as it can happen.

### Provenance, and how much data stood behind an answer

```rust
pub struct ZoneAnswer {
    pub resolution: LocalResolution, pub source: ZoneProvenance, pub basis: AnswerBasis,
}
pub struct OffsetAnswer {
    pub offset: UtcOffset, pub daylight: bool,
    pub source: ZoneProvenance, pub basis: AnswerBasis,
}

#[non_exhaustive] pub enum ZoneProvenance { EmbeddedVtimezone, CallerDatabase, FixedOffset }
#[non_exhaustive] pub enum AnswerBasis {
    Computed,
    BeyondKnownTransitions(CivilDate),
}
```

The answer structs have public fields and are not `#[non_exhaustive]`, because callers
implement `ZoneSource` and must be able to construct them. The enums are `#[non_exhaustive]`,
so a new source kind or a new basis is not a breaking change.

Provenance is a flat enum rather than a pair, because "which source answered" and "how much did
that source actually know" are two facts and nesting the second inside the first invites a
caller to read one and think it read both. `AnswerBasis` is the second fact. A `VTIMEZONE` whose
transitions are three `RDATE` lines through 2029, referenced by an event in 2035, has no data
for 2035; continuing its last observance is a reasonable thing to do and a dishonest thing to do
quietly, so the answer carries `BeyondKnownTransitions(2029-10-28)` and the date it was last
sure of. A continued answer that happens to match a rule-derived one would otherwise read as
confident corroboration by two independent sources, which is precisely the silent-fallback shape
ADR-0003 rejects, moved from *which source won* to *how much the winner knew*.

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
    pub fn report<D: DiagnosticSink + ?Sized>(
        &self, outcome: PolicyOutcome<OffsetAnswer>, at: Instant,
        meter: &mut Meter, sink: &mut D,
    );
}

pub struct FixedOffsetSource { /* &'static str, UtcOffset, bool */ }
impl FixedOffsetSource {
    pub const fn new(tzid: &'static str, offset: UtcOffset, daylight: bool) -> Self;
}
```

Reporting is separate from asking because only the caller knows how often it wants to be told:
one series resolved a thousand times against two sources that disagree is one fact, and a
thousand diagnostics is not a report but a denial of service against whoever reads them. So
`report` is a second call the caller makes where it wants the fact recorded, and it emits
exactly two codes — `time-zone-source-disagreement` on `Disagreed` and `unknown-time-zone` on
`Neither`. It deliberately does not emit `time-zone-coverage-exhausted`: that fact rides on each
answer's own `AnswerBasis`, and one golden-listed code with two emitters is one code too many.

`FixedOffsetSource` is the smallest honest `ZoneSource`: one identifier, one offset, always
`Unique`, always `Computed`, `None` for any other identifier. It exists so that "the only zone
data in play is the caller's own" is a wiring choice rather than a trait to hand-implement.

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
pub struct Observance { /* accessors below */ }
impl Observance {
    pub const fn new(start: CivilDateTime, offset_from: UtcOffset, offset_to: UtcOffset,
                     daylight: bool, rule: Option<YearlyRule>) -> Self;
    pub const fn start(self) -> CivilDateTime;      // read against `offset_from`
    pub const fn offset_from(self) -> UtcOffset;
    pub const fn offset_to(self) -> UtcOffset;
    pub const fn daylight(self) -> bool;
    pub const fn rule(self) -> Option<YearlyRule>;
    pub fn covered_through(self) -> Option<CivilDate>;
    pub fn transition_in(self, year: u16) -> Option<CivilDateTime>;
}

pub struct YearlyRule { /* month, RuleDay, CivilTime, Option<CivilDate> */ }
impl YearlyRule {
    pub const fn new(month: u8, day: RuleDay, at: CivilTime,
                     through: Option<CivilDate>) -> Option<Self>;
    pub fn applies_in(self, year: u16) -> bool;
    pub fn occurrence_in(self, year: u16) -> Option<CivilDate>;
}

#[non_exhaustive] pub enum NthWeek { First, Second, Third, Fourth, Fifth, Last }
#[non_exhaustive] pub enum RuleDay {
    DayOfMonth(u8),
    LastDayOfMonth,
    Nth       { weekday: Weekday, week: NthWeek },
    OnOrAfter { weekday: Weekday, day: u8 },
    OnOrBefore{ weekday: Weekday, day: u8 },
}

pub struct TransitionTable { /* Box<str>, Vec<Observance>, bool, Option<CivilDate> */ }
impl TransitionTable {
    pub fn new<S: DiagnosticSink + ?Sized>(tzid: Box<str>, observances: Vec<Observance>,
                                           meter: &mut Meter, sink: &mut S) -> Self;
    pub fn tzid(&self) -> Tzid<'_>;
    pub fn observances(&self) -> &[Observance];
    pub const fn is_truncated(&self) -> bool;
    pub const fn coverage_end(&self) -> Option<CivilDate>;
    pub fn observance_at(&self, instant: Instant) -> Option<Observance>;
    pub fn observances_around(&self, local: CivilDateTime) -> LocalResolution;
}

pub struct VtimezoneSet { /* Vec<TransitionTable> */ }
impl VtimezoneSet {
    pub fn insert(&mut self, table: TransitionTable, meter: &mut Meter)
        -> Result<(), ZoneSetError>;
    pub fn table(&self, tzid: &str) -> Option<&TransitionTable>;
}
#[non_exhaustive] pub enum ZoneSetError { Duplicate(TransitionTable),
                                          TooMany(TransitionTable, LimitExceeded) }

pub trait ObservanceReader {
    fn read_vtimezone(&self, meter: &mut Meter, sink: &mut dyn DiagnosticSink,
                      out: &mut Vec<Observance>) -> Option<Box<str>>;
}

pub fn read_calendar_zones<S: DiagnosticSink + ?Sized>(
    calendar: &Component, meter: &mut Meter, sink: &mut S,
) -> VtimezoneSet;
```

The fields are private behind `const` accessors rather than public, because an `Observance`'s
`start` is a wall clock read against `offset_from` and not against `offset_to`, and a struct
literal is where a caller silently disagrees about which. `RuleDay` is a wider vocabulary than a
weekday and an ordinal: `BYMONTHDAY` alone names a fixed day, and the `BYDAY` paired with a
`BYMONTHDAY` *run* — `SU` with `8,9,10,11,12,13,14` — is the seven-day window every tzdata
`Sun>=8` rule is exported as, which collapses to `OnOrAfter` and not to any `NthWeek`. `Fifth`
and `Last` are separate because they differ in every month without five of that weekday, and a
producer that wrote `BYDAY=5SU` meant the fifth.

Invariants: `observances()` is sorted by `start` and its length never exceeds
`Limits::max_vtimezone_observances`; `is_truncated()` is true exactly when observances were
dropped to hold that line, and they are dropped from the end so the table's coverage ends
earlier rather than acquiring a hole. `coverage_end()` returns `None` when some observance
repeats by a rule with no `UNTIL` — that zone knows the future — and otherwise the last date
backed by real data, which is the value that turns a later query into
`AnswerBasis::BeyondKnownTransitions`. `occurrence_in` is closed-form arithmetic over the
weekday of the first of the month: no loop, no search, therefore no candidate budget and no way
to make a lookup do unbounded work. `VtimezoneSet::insert` refuses a duplicate `TZID` by handing
the table back, so a document that declares one zone twice is a reported fact rather than a lost
definition.

The meter (DP-08, ADR-0010) is a mandatory argument on construction, where untrusted input is
read, and appears nowhere on the resolution path, because after construction the table is finite
and rule evaluation is O(1). `ObservanceReader` is a trait rather than an inherent constructor
because the thing that holds a `VTIMEZONE` is `ical_core::Component` and a caller with its own
representation should not have to build one; `ical-tz` implements it for `Component`, and
`read_calendar_zones` is the whole-calendar walk over that implementation, which additionally
reports a `TZID` referenced by an event that no `VTIMEZONE` defines.

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

### The seam with `ical-recur`

Nothing in the surface above expands a rule, and `ical-recur` holds no zone. The timeline
between them was left half-specified by M1 and is stated here, because getting it wrong puts
every zoned series an hour out for half the year and nothing about the types would show it.

```rust
pub mod seam {
    pub fn nominal(local: CivilDateTime) -> Option<Instant>;
    pub fn wall_clock(nominal_instant: Instant) -> Option<CivilDateTime>;

    pub struct LocalInterval { /* half-open, on the nominal timeline */ }
    #[non_exhaustive] pub enum UntilReading     { Midnight, EndOfDay }
    #[non_exhaustive] pub enum ExclusionReading { Instantaneous, WholeDay }
    pub struct ResolutionPolicy { /* gaps, folds, until, exclusions */ }
}

pub struct ZonedSeries<'a, S: ?Sized> { /* &'a S, &'a str, ResolutionPolicy */ }
impl<'a, S: ZoneSource + ?Sized> ZonedSeries<'a, S> {
    pub fn anchor(&self, dtstart: DateTimeValue<'_>) -> Option<Instant>;
    pub fn to_nominal(&self, utc: Instant) -> Option<Instant>;
    pub fn project_until<D: DiagnosticSink + ?Sized>(
        &self, until: DateTimeValue<'_>, dtstart: DateTimeValue<'_>,
        meter: &mut Meter, sink: &mut D,
    ) -> Option<Instant>;
    pub fn answer_for(&self, key: Instant) -> Option<ZoneAnswer>;
    pub fn actual<D: DiagnosticSink + ?Sized>(
        &self, key: Instant, meter: &mut Meter, sink: &mut D,
    ) -> Option<Instant>;
}

pub struct ResolvedExclusions { /* Vec<Instant>, Vec<LocalInterval> */ }
impl ResolvedExclusions {
    pub fn read<S: ZoneSource + ?Sized, D: DiagnosticSink + ?Sized>(
        series: &ZonedSeries<'_, S>, dtstart_kind: ValueType,
        excluded: &[DateTimeValue<'_>], meter: &mut Meter, sink: &mut D,
    ) -> Self;
    pub fn instants(&self) -> &[Instant];
    pub fn spans(&self) -> &[LocalInterval];
    pub fn excludes(&self, key: Instant) -> bool;
}

pub struct WallClockShift { /* elapsed seconds, wall-clock seconds */ }
impl WallClockShift {
    pub fn measure<S: ZoneSource + ?Sized>(source: &S, tzid: &str,
                                           from: Instant, to: Instant) -> Option<Self>;
    pub const fn crossed_a_transition(self) -> bool;
}
pub fn extra_widening(shifts: &[WallClockShift]) -> i64;

pub struct OrphanScan<'a> { /* &'a [Instant], Vec<bool> */ }
impl<'a> OrphanScan<'a> {
    pub fn new(recurrence_ids: &'a [Instant]) -> Self;
    pub fn observe(&mut self, key: Instant);
    pub fn finish<D: DiagnosticSink + ?Sized>(self, meter: &mut Meter, sink: &mut D) -> u32;
}
```

**The timeline `ical-recur` walks for a zoned series is the series' own wall clock projected
onto UTC, and not the UTC timeline.** Call a position on it *nominal*. `nominal` and
`wall_clock` are that projection and its inverse; arithmetically each is the identity on the
numbers, and the whole content of the contract is which of the two facts a given `Instant` is.
Every instant crossing into the search — `DTSTART`, `UNTIL`, each `RDATE`, `EXDATE` and
`RECURRENCE-ID` — is nominal, every cadence key coming back is nominal, and `actual` resolves
each key against the zone one at a time, which is the only place a transition can be seen. A
daily 09:00 series is then stable on the wall clock because the wall clock is what was
generated; a caller that anchors at the real UTC instant and never re-resolves is exactly one
transition's width out from the transition onwards, which `ical-conform` asserts as a number
rather than leaving as a warning.

Only two shapes need converting rather than reading: a `Z`-terminated value, which is a real
instant and goes through `to_nominal`, and a `DATE`, which names a day rather than a moment and
is read where `UntilReading` and `ExclusionReading` say. Those two policies exist because RFC
5545 permits both readings of a mismatched value type and real clients ship both: an `UNTIL`
written as a `DATE` against a date-time `DTSTART` drops the named day's instances at midnight
and keeps them at end of day, and an `EXDATE` written as a `DATE` removes one instant under
`Instantaneous` — usually none at all — and a whole day under `WholeDay`. The default of each is
the conservative reading, and the choice is the caller's because neither is a repair.

`WallClockShift` exists because `ical_recur::max_absolute_shift` widens the generation window by
a count of *elapsed* seconds, and across a transition an override's wall-clock move and its
elapsed move are different numbers. `extra_widening` reports the seconds that widening is short
by, never fewer, so a zoned caller adds it rather than re-deriving it. `OrphanScan` closes the
other side of the override question: a `RECURRENCE-ID` that names no generated instant is inert,
and every other silent drop in this workspace has a code.

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
| `LocalResolution::Nonexistent` / `Ambiguous` | 3.3.5 `DATE-TIME` form 3, 3.3.10 | the two awkward hours |
| `CivilDate`, `CivilTime`, `CivilDateTime` | 3.3.4, 3.3.12, 3.3.5 | wall-clock values |
| `Instant` | 3.3.5 `DATE-TIME` form 2 (UTC) | the timeline |
| `UtcOffset` | 3.3.14 `UTC-OFFSET` | an offset |
| `Duration` | 3.3.6 `DURATION` | a span, with no month field |
| `MonthAddOutcome` | 3.3.10 (invalid instances) | month arithmetic that can fail |
| `ZoneSource`, `ZoneAnswer` | — | ADR-0003: the source is the caller's |
| `PolicyOutcome`, `AnswerBasis`, `ZoneProvenance` | — | ADR-0003: disagreement is reported |

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
- **An error channel on `ZoneSource::resolve`.** `Option` plus `AnswerBasis` plus
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
use ical_tz::{CivilDateTime, FoldPolicy, GapPolicy, Instant, ZoneSource};

pub fn start_instant(
    source: &dyn ZoneSource,
    tzid: &str,
    local: CivilDateTime,
    gaps: GapPolicy,
) -> Option<Instant> {
    // `pick` is the one place three states collapse into one instant, so two call sites in
    // one program cannot disagree about what `ShiftForward` means. RFC 5545 section 3.3.5 is
    // `ShiftForward`, section 3.3.10 is `Skip`, and the crate refuses to choose between them.
    source.resolve(tzid, local)?.resolution.pick(gaps, FoldPolicy::Earlier)
}
```

**Wiring the embedded `VTIMEZONE` against the caller's database and reading the outcome.**

```rust
use ical_tz::{CivilDateTime, CombinedZoneSource, Instant, PolicyOutcome, ZoneSource};

pub enum ZoneNote {
    Confident, AgreedButContinued, Disagreement, SingleSource, Unresolvable,
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
            let note = if near.basis.is_beyond_known_transitions()
                || far.basis.is_beyond_known_transitions()
            {
                ZoneNote::AgreedButContinued
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
    CivilDate, CivilDateTime, CivilTime, Meter, NthWeek, Observance, RuleDay, TransitionTable,
    UtcOffset, Weekday, YearlyRule,
};

pub fn us_eastern<S: ical_core::DiagnosticSink + ?Sized>(
    meter: &mut Meter,
    sink: &mut S,
) -> Option<TransitionTable> {
    let eastern = UtcOffset::from_seconds(-18_000)?;
    let daylight = UtcOffset::from_seconds(-14_400)?;
    let two_am = CivilTime::from_hms(2, 0, 0)?;
    let second_sunday_of_march = RuleDay::Nth { weekday: Weekday::Sunday, week: NthWeek::Second };
    let first_sunday_of_november = RuleDay::Nth { weekday: Weekday::Sunday, week: NthWeek::First };
    let observances = alloc::vec![
        Observance::new(
            CivilDateTime::new(CivilDate::from_ymd(2007, 3, 11)?, two_am),
            eastern,
            daylight,
            true,
            YearlyRule::new(3, second_sunday_of_march, two_am, None),
        ),
        Observance::new(
            CivilDateTime::new(CivilDate::from_ymd(2007, 11, 4)?, two_am),
            daylight,
            eastern,
            false,
            YearlyRule::new(11, first_sunday_of_november, two_am, None),
        ),
    ];
    // `coverage_end()` on the result is `None`: both rules run on, so no answer this table
    // ever gives will carry `AnswerBasis::BeyondKnownTransitions`.
    Some(TransitionTable::new(
        alloc::boxed::Box::from("America/New_York"),
        observances,
        meter,
        sink,
    ))
}

pub fn spring_forward(year: u16) -> Option<CivilDate> {
    YearlyRule::new(
        3,
        RuleDay::Nth { weekday: Weekday::Sunday, week: NthWeek::Second },
        CivilTime::midnight(),
        None,
    )?
    .occurrence_in(year) // closed form: no search, no budget
}
```

**Normalizing an `EXDATE` list for `ical-recur` (DP-10, amendment (c)).**

```rust
use ical_core::{DateTimeValue, Meter, ValueType};
use ical_tz::{ResolutionPolicy, ResolvedExclusions, ZoneSource, ZonedSeries};

pub fn normalize_exdates<S: ZoneSource + ?Sized, D: ical_core::DiagnosticSink + ?Sized>(
    source: &S,
    tzid: &str,
    excluded: &[DateTimeValue<'_>],
    meter: &mut Meter,
    sink: &mut D,
) -> ResolvedExclusions {
    // The list `ical_recur::RecurrenceInput::new` takes is on the nominal timeline, sorted and
    // deduplicated, because that constructor refuses anything else. A `DATE` among date-time
    // values is a value-type mismatch that still excludes something, under whichever reading
    // the policy states, and it is reported either way.
    let series = ZonedSeries::new(source, tzid, ResolutionPolicy::DEFAULT);
    ResolvedExclusions::read(&series, ValueType::DateTime, excluded, meter, sink)
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

1. `LocalResolution`'s variants carry their offsets, and `Nonexistent` carries `shifted`.
   DP-11 pinned `{ Single(Instant), Ambiguous(Instant, Instant), Gap }`, which cannot express
   the RFC 5545 section 3.3.5 reading at all, so a caller obeying 3.3.5 would have to re-derive
   it from a second query against a source that may not agree with the first.
2. An answer carries a `basis` beside its `source` rather than a bare source label. DP-11
   pinned provenance as "which source produced it", which cannot distinguish a computed
   answer from a final observance continued past the end of an `RDATE` list.
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

Two gaps stay open with their names on them. First, `AnswerBasis` reports *that* an answer
continued a final observance and the date it was last sure of, but not what a caller should do
about a continuation six years wide as against one a day wide; the arithmetic is available and
the judgment is the caller's, and every caller will make it differently. Second,
this crate makes termination possible but does not guarantee it: `YearlyRule::occurrence_in`
never loops, but a caller searching for an instance that can never exist — `BYMONTH=2`
paired with `BYMONTHDAY=30`, where `add_months` reports `Clamped` in every year forever — is
relying on `ical-recur`'s candidate budget to stop, not on anything here. `MonthAddOutcome`
carries `requested_day` so that caller can at least say why it gave up.

The third gap this section named — that nothing here had been checked against real transition
data — is closed. `crates/ical-conform/tests/break_zones.rs` puts Europe/Berlin, both eras of
`America/New_York`, `Australia/Lord_Howe`'s thirty-minute step and an `RDATE` table that runs
out through the real surface, with every expectation transcribed from those zones' published
rules rather than read off an answer this workspace gave.

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

## What M2 shipped

The crate behind the surface above was built in M2 and is no longer a proposal. The blocks in
"The public surface" carry the shipped shapes; this section carries what changed and why, and
answers the five open questions.

**The three states were renamed and given a `Reading`.** `Single`/`Gap` are `Unique` and
`Nonexistent`, because `Single` reads as a count of sources rather than of instants and `Gap`
names the zone's behavior rather than the wall clock's answer. More usefully, each reading is a
struct rather than loose fields: an instant, its offset and its daylight flag travel together, so
a variant holding two readings cannot pair the later instant with the earlier offset. `GapPolicy`
gained a third arm, `ClampToTransition`, because the two the RFC argues about — skip it, or shift
it — are not the two a calendar client offers, and "as soon as it can happen" is what a user
means by moving a 02:30 meeting out of a gap.

**Provenance is flat and the basis sits beside it.** `ZoneProvenance { source, coverage }` became
the enum `ZoneProvenance` plus a separate `basis: AnswerBasis` field on each answer, as ADR-0003's
own amended mechanism states. Nesting the second fact inside the first invited a caller to read
`answer.source` and believe it had read both, which is the failure the pair exists to prevent.
`Coverage::Extrapolated { nearest_known }` is `AnswerBasis::BeyondKnownTransitions(CivilDate)`:
the same fact, named after what the source did rather than after a statistical operation it did
not perform.

**The five open questions are closed.** (1) The civil-time primitives are in `ical-core` and
ADR-0011 says so; the graph has no cycle. (2) `CivilDateTime` on `resolve` stands, and
`ical-core`'s `DateTimeValue` decomposes into it at exactly one place, `ZonedSeries::anchor`.
(3) This crate emits thirteen golden-listed codes, of which two are named as this document
proposed them: `vtimezone-rule-unsupported` and `duplicate-time-zone-identifier`.
`ObservancesTruncated` shipped as `vtimezone-observances-truncated` on `Severity::LimitReached`
and `NoObservanceDefined` as `vtimezone-without-observance`. `ZoneOffsetInvalid` was **not**
taken: an observance whose `TZOFFSETFROM` or `TZOFFSETTO` cannot be read is `Component::audit`'s
finding under section 3.6, and a second copy of that judgment here is a second place for the two
to disagree. Five were already listed against M2 before a line of this crate was written —
`unknown-time-zone`, `missing-time-zone-definition`, `ambiguous-local-time`,
`nonexistent-local-time`, `time-zone-source-disagreement` — and four the milestone's agenda
produced were added to the list with it: `time-zone-coverage-exhausted`,
`recurrence-until-not-utc`, `exdate-value-type-mismatch` and `override-matches-no-instance`. (4) `Limits` is not passed at all:
`Meter` carries it and one argument does both jobs, which is what ADR-0010 asks for anyway.
(5) Left to `ical-itip`, unchanged: nothing in M2 forced the question.

**`RuleDay` is wider than this document's `weekday` plus `week`.** A `VTIMEZONE`'s `RRULE` is
written by producers, not by tzdata, and the shape they emit for `Sun>=8` is `BYDAY=SU` paired
with a seven-day `BYMONTHDAY` run. Collapsing that onto an `NthWeek` is wrong in the months where
the run does not begin on a week boundary, so the model carries the window as itself. `NthWeek`
also gained `Fifth`, which differs from `Last` in exactly the months without five of that weekday
and is what a producer writing `BYDAY=5SU` asked for.

**The reader parses `RRULE` itself rather than depending on `ical-recur`.** The rejection above
stands and is now load-bearing: `ical-tz` declares `ical-core` and nothing else, `just purity`
enforces it, and the yearly subset the reader accepts is a few dozen lines. Anything outside it
is `vtimezone-rule-unsupported` with the `DTSTART` still standing, which is a transition the
table keeps rather than a definition it throws away.

**One documented behavior is a gap rather than a decision.** `ZoneSetError::TooMany` hands back a
table that did not fit under `Limits::max_vtimezone_components`, and its own doc says "a limit
breach is already reported as itself by whoever charged the meter" — but the charge site is
inside `VtimezoneSet::insert`, nothing reports it, and the golden list has no code for a
zone-count refusal. A calendar declaring more zones than the bound silently keeps the ones that
fit. That is the behavior `read_calendar_zones` documents and a test pins, and it is a smaller
hole than inventing a ninth code during integration, but it is a hole: either the doc comment or
the code list is wrong, and M3 should settle which.
