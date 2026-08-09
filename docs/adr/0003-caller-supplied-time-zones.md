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
