# ADR-0003: the time zone source is supplied by the caller and named explicitly

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10

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

### Mechanism (DP-11)

The principle above is unchanged. What it lacked was a shape specific enough to hold code
against, and that shape is now part of the decision.

The caller-supplied source is
`trait ZoneSource { fn resolve(&self, tzid: &str, local: DateTimeValue) -> Option<ZoneAnswer>; }`
— deliberately object-safe (`&self` in, an owned `ZoneAnswer` out, no generic or
associated-type parameter), because combining an embedded `VTIMEZONE` source with an IANA
source is a runtime wiring choice the caller makes once, not a compile-time generic. The
`ZoneAnswer` it hands back states the resolved time, and the resolution has three shapes:
`LocalResolution` is `{ Single(Instant), Ambiguous(Instant, Instant), Gap }`, so a single
source facing a DST fold or a spring-forward gap represents that honestly before any
comparison happens — this is the type the whole disagreement mechanism rests on, and it must
be verified against real transition data rather than merely shaped correctly.

Provenance names which source answered. It does not say how much of that source's data
actually backed the answer, and those are not the same fact. `ZoneAnswer` therefore carries
a third field — `ZoneAnswer { resolution: LocalResolution, source: ZoneProvenance, basis:
AnswerBasis }`, where `#[non_exhaustive] enum AnswerBasis { Computed,
BeyondKnownTransitions(CivilDate) }`. `Computed` means the source held a transition at or
before the queried instant and a rule or further data covering it. `BeyondKnownTransitions(last)`
means the instant lies past the last transition the source actually knows, and the answer
continues the final observance recorded at `last` — the ordinary fate of an embedded
`VTIMEZONE` whose transitions are explicit `RDATE` lines, asked about a date after they run
out. Continuing the last observance is the defensible thing for such a source to do;
reporting it as though a rule had produced it is not. Agreement between a `Computed` answer
and a `BeyondKnownTransitions` one is a different fact from agreement between two computed
ones, and `basis` is what lets a caller tell those apart instead of reading confident
concurrence into a coincidence.

A source that does not recognize an identifier returns `None`. That is what the `Option` is
for. `TZID:Eastern Standard Time` from Exchange and `TZID:/mozilla.org/20050126_1/Europe/Berlin`
from Lightning are not IANA keys, and an IANA-backed source asked for either must say it does
not know them — not default to UTC, and not quietly alias them inside the implementation,
which would be exactly the buried fallback chain this ADR rejects. Mapping a vendor
identifier onto an IANA one is a step the caller performs before `resolve` is called, where
it is visible and where its failure is visible too.

A two-source policy is a distinct, richer-typed surface, not another `ZoneSource` impl.
`CombinedZoneSource` always queries both sources — it never short-circuits — and returns
`#[non_exhaustive] enum PolicyOutcome { Agreed(ZoneAnswer), Disagreed { embedded: ZoneAnswer,
fallback: ZoneAnswer }, OnlyEmbedded(ZoneAnswer), OnlyFallback(ZoneAnswer), Neither }`. The
`Only*` variants are where a `None` lands: "one source does not know this zone" is reported
as itself, never collapsed into `Disagreed`, and never settled by preferring whichever source
happened to answer.

`ZoneSource` carries no `Send`/`Sync` bound in the base `no_std` trait; std and server
adapters get `Arc<dyn ZoneSource + Send + Sync>` for free when their concrete source is
`Send + Sync`. Callers on code-size-constrained targets with a closed set of source kinds may
prefer hand-written enum dispatch over `&dyn ZoneSource` to avoid the vtable indirection: the
trait permits dyn, it does not mandate it.

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

The combined policy costs twice a single lookup, always, because that is what refusing to
short-circuit means; a caller who wants one source pays for one by using it directly. And
`#[non_exhaustive]` on the caller-facing enums is a permanent ergonomic tax — every
downstream `match` carries a wildcard arm forever — accepted because the alternative breaks
semver the first time a policy variant is added.

`AnswerBasis` closes coverage but not staleness, and staleness is the more common case. An
embedded `VTIMEZONE` that does carry an `RRULE` computes 2035 perfectly well from rules
written in 2018 and since superseded by a government; both sides report `Computed`, the
caller gets a bare `Disagreed`, and nothing in the type hints that one side's rules are eight
years old. Fixing that needs an as-of date attached to a source, which the no-clock rule
makes genuinely awkward — the library cannot ask what "now" is, so it would have to be a
caller-supplied assertion. That is an unmade decision, not a deferred implementation.

Nor does this say what a caller should *do* with a `BeyondKnownTransitions` answer. That is
deliberate, since this ADR refuses to prefer a source, but the consequence is that every
caller re-derives the same judgment independently and they will not all reach the same one —
a weaker form of the inconsistency the decision set out to prevent. The alias question is
likewise assigned rather than answered: mapping is now the caller's visible step, which stops
sources from lying about it, but the workspace ships no Windows/CLDR identifier table and has
not decided whether it may. This ADR forbids bundling time zone *data*; whether an identifier
alias table counts as data or as vocabulary is undecided, and every caller hits the question
on its first Outlook file.

Two things carry over unclosed. Whether "continue the last observance" is even the right RFC
reading for an exhausted `RDATE`-only `VTIMEZONE` has been treated here as the defensible
default without being confirmed against section 3.6.5's observance-selection language or
against what libical does. And the vtable cost of `&dyn ZoneSource` on Cortex-M-class
hardware is unquantified — nobody benchmarked it — which is why the enum-dispatch escape
above is offered per target rather than settled here.

## Amendments

M2 built the crate this decision governs, and four sentences of the Mechanism did not survive
contact with it. Each is amended here rather than quietly reinterpreted, and each has a test in
`ical-tz` or a conformance case in `crates/ical-conform/tests/break_zones.rs` behind it. The
principle — no bundled data, no clock, the caller supplies the source, every answer names it,
and a disagreement is a reported fact — is unchanged and was not challenged once.

**1. The source trait has two methods, and the second is not a convenience.** The Mechanism
above names only
`fn resolve(&self, tzid: &str, local: DateTimeValue) -> Option<ZoneAnswer>`. What shipped is that
method taking a `CivilDateTime` — the caller with the most resolutions to do is normalizing
generated candidates that borrow from no property, so the value type would be a wrapper built and
discarded per occurrence — beside
`fn offset_at(&self, tzid: &str, instant: Instant) -> Option<OffsetAnswer>`. A source with only
`resolve` can carry an instant *out* of a zone and cannot carry one *in*, so a `Z`-terminated
`UNTIL`, a `RECURRENCE-ID` and an override's own two ends have no reading at all — and those are
not exotic inputs, they are what every zoned series in the wild is written with. `OffsetAnswer`
carries the same provenance and basis as `ZoneAnswer`, so the honesty requirement holds in both
directions rather than only in the hard one.

**2. `LocalResolution` is `{ Unique, Ambiguous, Nonexistent }` and each reading is a triple.**
The shape this ADR pinned — `{ Single(Instant), Ambiguous(Instant, Instant), Gap }` — cannot
express RFC 5545 section 3.3.5's reading of a wall clock inside a gap, so a caller obeying 3.3.5
would have to re-derive it from a second query against a source that need not agree with the
first. What shipped carries, per variant, an `Instant` with the `UtcOffset` that produced it and
the daylight flag of the observance in force, and the gap variant additionally carries
`offset_before`, `offset_after`, the transition's own edges and `shifted`. The daylight flag is
read from `DAYLIGHT` against `STANDARD` and never inferred from which offset is larger, because
`Australia/Lord_Howe` runs `+10:30` standard against `+11:00` daylight and a resolver comparing
magnitudes answers it backwards.

The instruction this ADR attached to that type — that it "must be verified against real
transition data rather than merely shaped correctly" — is discharged. Europe/Berlin, both eras of
`America/New_York`'s rules, `Australia/Lord_Howe`'s thirty-minute step and a `VTIMEZONE` whose
`RDATE` lines run out are all read from committed `.ics` fixtures and asked about the two awkward
hours, with every expected column transcribed from those zones' published rules.

**3. `PolicyOutcome::Agreed` keeps both answers, and the enum is generic.** This ADR wrote
`Agreed(ZoneAnswer)`. Collapsing the pair discards exactly the asymmetry `basis` was added to
surface: agreement between two `Computed` answers and agreement between a `Computed` one and a
`BeyondKnownTransitions` one are different facts, and a caller holding one answer cannot tell
them apart. `Agreed { embedded, fallback }` and `Disagreed { embedded, fallback }` are therefore
the same shape, and `PolicyOutcome<A = ZoneAnswer>` takes a type parameter so the same five
variants serve `offset_at`. Reading the outcome down to one answer is `embedded_first`, a method
with its preference written on its name.

**4. Reporting a disagreement is a second call, not a side effect of asking.** This ADR says a
disagreement is "a reported fact available to the caller" without saying who writes it down.
`CombinedZoneSource::report` does, on a diagnostic sink and a meter the caller owns, because a
single series resolved a thousand times against two sources that disagree is one fact and a
thousand diagnostics is a denial of service against whoever reads them. Only the caller knows
where that line is. `resolve` and `offset_at` still query both sources unconditionally every
time; what is deferred is the writing down, not the asking.

**5. What a caller should do with a `BeyondKnownTransitions` answer is still not decided, and
one consequence of that is now visible.** The Consequences above call this out and it stands: the
crate continues the final observance and says so, and refuses to prefer a source. `ical-conform`
pins the resulting case with the honest label — an `RDATE` table ending in 2029, asked about June
2035, answers CET, and real Berlin is on CEST in June, so the answer is defensibly wrong and the
test comment says exactly that. What M2 adds is the fact traveling on a golden-listed code,
`time-zone-coverage-exhausted`, so a caller that wants to act on it does not have to inspect a
field it may not know exists. Whether "continue the last observance" is the right reading of
section 3.6.5 is still unconfirmed against that section's observance-selection language and
against libical, exactly as this ADR left it.

**6. One bound is charged and reported by nobody.** `VtimezoneSet::insert` charges
`Limits::max_vtimezone_components` and hands back the table that did not fit, but no golden-listed
code names a zone-count refusal, so a calendar declaring more zones than the bound silently keeps
the ones that fit. ADR-0010 says a bound nobody charges is decoration; this is its sibling, a
charge nobody reports, and it is recorded here rather than closed with a code invented during
integration.
