# ADR-0003: the time zone source is supplied by the caller and named explicitly

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10, 2026-08-11 (sixteen amendments)

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
reporting it as though a rule had produced it is not. **"Defensible" is superseded by
Amendment 14: it is RFC 5545 section 3.6.5's stated procedure executed literally, and the
hedge invited re-litigation of something now closed.** Agreement between a `Computed` answer
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
**Amendment 15 makes it: a source may be asked how old its data is, the asking is a separate
call from the answering, and nothing in this workspace compares two vintages.**

Nor does this say what a caller should *do* with a `BeyondKnownTransitions` answer. That is
deliberate, since this ADR refuses to prefer a source, but the consequence is that every
caller re-derives the same judgment independently and they will not all reach the same one —
a weaker form of the inconsistency the decision set out to prevent. The alias question is
likewise assigned rather than answered: mapping is now the caller's visible step, which stops
sources from lying about it, but the workspace ships no Windows/CLDR identifier table and has
not decided whether it may. This ADR forbids bundling time zone *data*; whether an identifier
alias table counts as data or as vocabulary is undecided, and every caller hits the question
on its first Outlook file. **Amendment 16 decides it: an alias table is data, no crate
published from this workspace ships one, and the refusal is scoped by question rather than by
crate.**

Two things carry over unclosed. Whether "continue the last observance" is even the right RFC
reading for an exhausted `RDATE`-only `VTIMEZONE` has been treated here as the defensible
default without being confirmed against section 3.6.5's observance-selection language or
against what libical does. **Both legs are confirmed in Amendment 14, and what does not
survive the confirmation is the label rather than the answer.** And the vtable cost of `&dyn ZoneSource` on Cortex-M-class
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
against libical, exactly as this ADR left it. **That sentence is closed by Amendment 14; both
legs were read, and the reading is confirmed against the section and against four
implementations.**

**6. One bound is charged and reported by nobody.** `VtimezoneSet::insert` charges
`Limits::max_vtimezone_components` and hands back the table that did not fit, but no golden-listed
code names a zone-count refusal, so a calendar declaring more zones than the bound silently keeps
the ones that fit. ADR-0010 says a bound nobody charges is decoration; this is its sibling, a
charge nobody reports, and it is recorded here rather than closed with a code invented during
integration.

**6a. Four adversarial lenses were run against the built crate — the transitions, the sources,
the seam with `ical-recur`, and the bounds — and amendments 7 through 12 are the decisions they
forced.** Each has a case in `crates/ical-conform/tests/break_tz_*.rs` that failed before the
fix and passes after it, and none was reached by weakening what a case asserts.

**7. `resolve` answers something for a definition that exists and holds nothing, and
recognition is a question of its own.** The Mechanism above says a source that does not
recognize an identifier returns `None`, "that is what the `Option` is for", and `answer.rs`
restated it as "`None` for exactly one condition". A `VTIMEZONE` with no usable observance —
which RFC 5545 section 3.6.5 forbids and exporters ship — made that claim false the moment it
was filed: the table recognized its own `TZID` and answered `None` to every question, so a
calendar that declared a zone and a calendar that never mentioned one arrived as one fact, and
the pair reported `unknown-time-zone` at `Severity::Violation` about a zone the file supplies.
`LocalResolution::Undetermined` is what such a table answers with now, and it invents no offset:
it is a larger claim than silence and a smaller one than UTC.

The other direction cannot be fixed that way and is not. An `OffsetAnswer` is an offset, and
"recognized, holding nothing" has nowhere to go in one; filling the field with UTC is exactly
the invention this ADR exists to refuse. So `ZoneSource::recognizes` is a third method, with a
provided implementation that asks the other two, and `PolicyOutcome::Undetermined` is the pair's
answer where a source knows the identifier and neither could answer. `unknown-time-zone` now
means what it says.

**8. A table has two ends, and `AnswerBasis` states both.** Amendment 5 and the Decision are
written about a source asked *past* the last transition it knows. A `VTIMEZONE` whose `RDATE`
lines run 2027 through 2029 answers July 2020 by extending its earliest observance's
`TZOFFSETFROM` backwards forever — `America/New_York` was on `-04:00` that July and such a table
says `-05:00` — and that answer was `Computed`, indistinguishable from one the file had data
for. `AnswerBasis::BeforeKnownTransitions(CivilDate)` carries the first date the source knows,
`TransitionTable::coverage_start` is where it comes from, and
`time-zone-before-known-transitions` is the code it travels on.

Coverage at the far end is also narrower than it was. A single endless rule used to make a whole
table claim to know the future, so a definition whose daylight rule runs forever and whose
standard onsets are three `RDATE` lines ending in 2029 answered midwinter 2031 with permanent
summer time and called it computed. `coverage_end` is `None` only when *every* side of the
definition — `STANDARD` against `DAYLIGHT` — repeats forever, because a zone that cannot say
when its summer ends does not know its own future whatever its other half states.

**9. Two definitions of one identifier both stay in the set, and an empty one may not shadow a
full one.** This ADR's whole subject is that where two sources disagree both readings stay
reachable; a calendar declaring one `TZID` twice is that case inside one file, and
`read_calendar_zones` dropped the second on the floor after reading its code. A placeholder
`VTIMEZONE` written above the real definition therefore erased a zone the file states in full.
`VtimezoneSet` now holds both in file order, `VtimezoneSet::definitions` is where a caller takes
the other reading, `VtimezoneSet::len` counts identifiers rather than definitions, and
`VtimezoneSet::table` answers with the first definition that carries a transition — a preference
stated on the accessor rather than made silently by insertion order.

**10. Every diagnostic about a zone names the zone.** "Every result says which source produced
it" was implemented for answers and not for reports: three `TZID` parameters nothing defines
produced three `Diagnostic` values equal to each other, which tells a caller that something is
missing and not what to go and find. A `Location` cannot say it — a component owns unfolded
octets and has no span back into the caller's buffer — so `Diagnostic` carries a bounded inline
`Subject`, and `missing-time-zone-definition`, `duplicate-time-zone-identifier`,
`vtimezone-without-observance` and `vtimezone-components-truncated` all name their zone. The
cost is stated rather than hidden: every `Diagnostic` in the workspace is `Subject::CAPACITY`
octets larger whether it carries one or not, which is what a `Copy` diagnostic that allocates
nothing costs.

**11. Amendment 6 is closed: the zone-count refusal has a code.** A charge nobody reports was
recorded there as a known hole; M2 found what the silence costs. A definition the caller's own
bound turned back left the identifiers it declares looking exactly like identifiers the calendar
never wrote, so `missing-time-zone-definition` — a violation, about the file — was reported for
a loss the caller's own policy caused. `vtimezone-components-truncated` says what happened, at
`Severity::LimitReached`, naming each definition refused, and the undefined-identifier walk
excludes them.

**12. An observance whose required value is present and unreadable is this crate's to report.**
`reader.rs` delegated every unreadable required value to `Component::audit`'s reading of section
3.6, which is right for a property that is *absent* and answers nothing about one that is there
and unusable. `TZOFFSETTO:+9999` is refused by the value decoder and counted by the audit;
`DTSTART;VALUE=DATE` on an observance carries no hour for a transition to happen at. Both left a
`VTIMEZONE` in the set holding nothing and answering nothing, with no code from anybody.
`vtimezone-observance-unreadable` is emitted where every required property is present and the
observance still states no transition, so one fault never earns two codes.

**13. What a lookup costs is stated again, because the transitions lens changed it.** The
crate's own prose said a resolution was "logarithmic in the table and constant in the rules",
and both halves were bought by scanning only the last four observances admitted before a query.
Four `RDATE` lines — two years of a zone that moves twice a year — filled that window, and the
rules beside them stopped being consulted at all: `Europe/Berlin` answered CET on the first of
July, an hour wrong, with `AnswerBasis::Computed` and no diagnostic. A rule is in force from its
own `DTSTART` until something later supersedes it, so *every* rule a definition carries is now
asked about every query, and a rule that fires rarely is probed back sixty-four years rather
than three, which is the widest gap `FREQ=YEARLY;BYMONTH=2;BYDAY=5SU` can produce.

A lookup is therefore logarithmic in the dated transitions and linear in the *rules*, of which a
real definition carries a handful and a hostile one carries as many as
`Limits::max_vtimezone_observances` admits. That second number is measured rather than assumed:
a definition with 2,000 rule-bearing observances answers one query in about 35 ms in a debug
build, so the work stays bounded and terminating and is no longer cheap. Nothing in this
milestone narrows it, and a caller expanding a long series against such a file pays it per
occurrence; the shape that would close it is an index over the rules by side and by year, which
is work rather than a decision and is recorded here as owed. Two further costs are named rather than hidden. The candidate
offsets are taken from every era inside one day either side of the query rather than from its
two ends, because a definition with two transitions in one day has a middle offset that governs
seventeen hours and was never considered — which reported an ordinary lunchtime as a local time
that never happened. Walking those transitions allocates a small vector on the days a zone moves
and nothing on every other day. And the table is ordered by the instant each observance
*begins* rather than by the wall clock its `DTSTART` spells, which is what makes the search's
own predicate monotone and what stops two observances declared on one wall clock from resolving
by the order the producer happened to write them in.

**14. Continuing the final observance is section 3.6.5 executed literally, and `coverage_end` is
this crate's inference rather than the file's claim.** Amendment 5 and the Consequences both left
the reading unconfirmed, and both legs the ADR named have now been read. Section 3.6.5's
selection sentence is unconditional — the offset to apply at any given time is found by locating
the observance with the last onset before the time in question, and using that observance's
offset — with no upper bound, no exhaustion case and no alternative rule. For an `RDATE` table
ending in 2029 asked about 2035, the observance with the last onset before the time in question
*is* the final `RDATE`. libical's `icaltimezone_get_utc_offset` does exactly this, and ical.js,
ical4j's `getLatestOnset` and dateutil's `_tzicalvtz` agree. The incumbent is unopposed, no code
moves, and the reading that would have been the rival — continuing the last *rule* past an
expired `UNTIL` — is foreclosed by the same section's requirement that `UNTIL` state the last
valid onset of an observance.

What does not survive the confirmation is the label the Mechanism bundled with the answer. RFC
7808 section 7.1 states that an upper bound on a `VTIMEZONE`'s validity cannot simply be derived
from the observance with the latest onset time and that RFC 5545 defines no way to get such a
bound — which is precisely what `coverage_end_of` derives it from. So the amendment says, in the
ADR and in `ical-tz`'s own doc comments, that the *answer* is RFC-backed and the *bound* is not:
`AnswerBasis::BeyondKnownTransitions` and `time-zone-coverage-exhausted` report this crate's
reading of the extent of the table it was handed, and are not a claim the file made about its own
validity. That correction lands below the first paragraph of the code's doc comment, deliberately,
because `docs/diagnostic-codes.md` freezes the first paragraph verbatim and improving the prose
below it stays free — so no code key, no channel, no golden-list row and no conformance case
moves, and Corpus is unblocked without having its cases invalidated.

`TZUNTIL` (RFC 7808 section 3.9) is the property that would replace the inference with the file's
own statement, and it is not adopted now. Two thresholds are fixed here rather than after the
corpus lands, because this project already has one default that flips on unspecified evidence and
one is enough. Reading `TZUNTIL` and preferring it where present — same variant, same code, same
channel, inference unchanged where absent — is turned on if and only if the M5 corpus shows it on
at least 1% of `VTIMEZONE` components whose table is finite, from at least two distinct producers.
Splitting the fact into two golden-listed codes, one for a stated bound and one for an inferred
one, is turned on if and only if `TZUNTIL` reaches 50% of that same population, because one code
covering both grades misdescribes the minority and describes neither once the stated bound is the
majority. The asymmetry is deliberate: preferring a file's own statement over an inference is free
correctness that needs only proof the case exists, while a golden-list event should require the
stated bound to be the common case. Nothing here is contingent on the corpus arriving — if it
never lands, the reading stays closed, the doc corrections stand, and `TZUNTIL` stays unread.

Four things this makes worse. The workspace now cites section 3.6.5 as licensing the answer while
shipping a Note whose bound has no standards-track source, and the only thing reconciling them is
a paragraph placed *below* the frozen meaning — the weaker of the two places, chosen because the
stronger one costs a rename. A reader who reads only the golden list gets the unreconciled
version. Closing the far end also sharpens an asymmetry it does not touch:
`time-zone-before-known-transitions` will sit beside a sibling that now carries a citation, and
the near end is the end where section 3.6.5 is genuinely silent and the deployed implementations
genuinely disagree three ways — this amendment makes the weaker half look stronger by proximity,
and the near end stays open. The 1% and 50% numbers are judgment without data and can be wrong in
both directions; a corpus returning 40% leaves the workspace preferring `TZUNTIL` and still
describing two grades with one code, which is the state ADR 0009's freeze exists to prevent, and
this amendment says to live there until 50%. And the endless-one-side case Amendment 8 records
leaves the frozen phrase "later than the last transition it actually knows" true only if the
reader imports Amendment 8's claim that half an alternation is not knowledge — an imprecision left
in place, because fixing it means editing a frozen meaning.

The alternative rejected, with its own thresholds attached so it is not rediscovered: reground
`coverage_end` on `TZUNTIL` now and unconditionally, on the argument that this ADR exists to
refuse answers claiming more than their source supports and RFC 7808 names the derivation invalid.
It loses on payoff rather than on principle — no implementation examined emits or reads `TZUNTIL`,
admitting a second RFC's property into `ical-core`'s `VTIMEZONE` schema is a scope decision no ADR
licenses, and the inference's failure mode is a Note a caller may ignore rather than a wrong
offset, since section 3.6.5 gives the same answer either way.

**15. A zone source may be asked how old its data is, and the asking is a separate call from the
answering.** The Consequences call staleness the more common case and then say fixing it needs an
as-of date, which the no-clock rule makes awkward. It does not: a date supplied by the caller's
wiring, or read from octets already in the file, breaks neither the no-bundled-data rule nor the
no-clock rule, because the library still cannot say what "now" is. So `ical-tz` gains
`Vintage`, whose two variants — `CallerAsserted(CivilDate)` and `FileAsserted(CivilDate)` — are
the point of the type: there is no bare-date constructor, so a caller can never receive a vintage
without also receiving its origin, which is this ADR's own rule that every result says which
source produced it, applied one level up.

The mechanism follows from the ADR's other standing rules rather than from taste.
`TransitionTable::asserted_as_of` is populated by the reader from the definition's own
`LAST-MODIFIED`, a property RFC 5545 defines on `VTIMEZONE` for this purpose and the schema
already accepts, as `FileAsserted`; `TransitionTable::with_asserted_as_of` replaces it with
`CallerAsserted`, so a preference is written on the call the caller makes rather than made
silently by precedence rules inside the reader, in the shape Amendment 3 used for
`embedded_first`. `ZoneSource::asserted_as_of` is a fourth method with a provided body returning
`None`, exactly as `recognizes` was added by Amendment 7, so it is object-safe, `no_std`, and
breaks no existing implementor — and the forwarding impl for `&T` must forward it, or a `&dyn`
wrapper answers `None` over a source that knows better. `ZoneAnswer`, `OffsetAnswer`,
`AnswerBasis` and `PolicyOutcome` are unchanged: a third `AnswerBasis` variant is refused because
`basis` states what data stood behind *this* answer at *this* instant while a vintage is a
property of the source whatever the question, and folding them would widen a value returned once
per occurrence. Nothing in this workspace compares two vintages, ranks them, or lets one settle a
`Disagreed`, so no diagnostic code is minted and ADR 0009's golden list does not move — an old
definition is not a fault, and preferring the newer side is exactly the buried fallback chain this
ADR exists to refuse.

Five things get worse, and the first two are the real ones. `FileAsserted` is the file's claim
about itself and nothing verifies it: an exporter that stamps `LAST-MODIFIED` with the moment of
export rather than the vintage of the rules it copied produces a fresh-looking date over
eight-year-old transitions, and for a caller who trusts the date that is strictly worse than the
silence it replaces — silence prompted a question, a confident wrong date does not. The variant
tag is the whole mitigation and it is a label, not a check. And the ADR's complaint is narrowed
rather than closed: a caller who never calls `asserted_as_of` still receives a bare `Disagreed`
with nothing hinting that one side is old, so staleness becomes available rather than unmissable,
which is a deliberate second-best. Beyond those, `TransitionTable` grows an `Option<Vintage>` for
every zone whether or not the definition states anything; `LAST-MODIFIED` is a UTC date-time and
`Vintage` carries a `CivilDate`, so a value present in the file arrives through this door reduced;
and `ZoneSource` reaches four methods, one more for every future implementor to decide to leave
alone.

The alternative that must be recorded because it is serious: nothing at all, leaving staleness an
admitted blind spot, on the ground that a vintage nobody can verify invites `if newer { prefer }`
in every caller — the silent resolution this ADR forbids, now with a library-blessed field to hang
it on. It is rejected because the blind spot is not neutral either. The fact is reachable today by
re-parsing a component `ical-tz` has already read, so the choice is not between a fact and no fact
but between a fact with provenance attached and a fact every caller extracts privately with none.
Also rejected, and recorded so it is not rediscovered: deferring this behind a corpus grep. The
grep would report how often real exports stamp `LAST-MODIFIED` and no value of it changes the
shape, the semantics or the ban on comparison, and a measurement whose every outcome yields the
same design is a curiosity rather than a threshold.

**16. An identifier alias table is data, and no crate published from this workspace ships one.**
The Consequences leave this undecided while `docs/design/ical-tz-api.md` already lists a bundled
CLDR table under "Deliberately rejected", so two documents disagreed about whether anything had
been decided. Closing that gap is itself a decision, and leaving the design document as the only
place a refusal is written down is how a rejection nobody ratified becomes a default nobody
examined. The refusal is scoped by *question*, not by crate: no crate published here answers
"which IANA zone does this vendor string mean" — not Windows/CLDR `windowsZones`, not the
`/mozilla.org/DATE/Zone` convention, not case folding, not any other guess at an identifier's
intent. That closes all three of the options this was posed with, including the third: shipping
the table as a separate crate outside the purity rule fails for a reason that is not purity.
`ical-conform` sits outside that rule because it is a test artifact answering no question on the
resolution path; an alias crate would answer the central question of zone identity for anyone who
added it, and the fallback chain this ADR keeps visible would then be visible only in a
`Cargo.toml` line, which is not where a wrong zone shows up.

The evidence is that a table counts as data because CLDR itself refuses to answer with one value:
`windowsZones` is keyed on (Windows identifier, territory) and gives an ordered list of IANA
identifiers per key, so collapsing it to one answer is a policy choice, and this ADR assigns
policy choices about zone identity to the caller. The ecosystem's one bundled table confirms the
premise rather than falsifying it — python-icalendar's generated `windows_to_olson.py` carries
single-string values, is applied on a path with no warning and no exception, and on a miss
silently proceeds with the unmapped name. Its `GloballyUniqueTZIDGuessed` diagnostic belongs to
vendor-prefix stripping under section 3.2.19 and not to the Windows table at all, so the one
implementation that warns warns about the mechanism this workspace refuses outright in
`Tzid::strip_global_prefix`'s documented contract.

What stands in place of a table, and what makes the refusal usable rather than merely pure, is
already built: `Tzid::form` classifies an identifier's shape without claiming what it names and
`TzidForm::Opaque` is the crate saying in a type that translation is the caller's step; lookup is
by exact bytes including case, so an Exchange file that writes both `TZID:W. Europe Standard Time`
*and* the matching `VTIMEZONE` resolves today with no table; and where nothing answers, `resolve`
returns `None` and the hole is reported as `unknown-time-zone` rather than defaulted to UTC. The
residue a table would serve is exactly the files that name a vendor zone they do not define. The
one path a table may take is caller-side, as a `ZoneSource` decorator the caller wires — and a
decorator is now also responsible for forwarding Amendment 15's `asserted_as_of`, since a wrapper
that inherits the provided body answers `None` over a source that knows better.

The costs are not small and none of them is hypothetical. A caller reading Exchange or Outlook
files that reference a Windows zone without defining it gets nothing from this workspace and must
obtain, wire and maintain a CLDR mapping; they will each do it differently, which is a weaker form
of the inconsistency this ADR set out to prevent and the same cost already recorded against
`BeyondKnownTransitions`. `unknown-time-zone` fires at `Severity::Violation` on files that open
correctly in Outlook, so this library reads as stricter than the ecosystem on inputs users consider
ordinary, and that support burden lands on whoever answers the first report. No measurement was
taken of how large the affected population is. The conformance corpus can now test only the
refusal and never the resolution. And the one genuinely good thing the ecosystem demonstrated —
python-icalendar stamping its generated table with the CLDR commit and date, sabre/vobject
stamping its own last update — is given up: a caller who wants stamped, inspectable provenance for
its own mapping builds that machinery again, and most will not.

The strongest rejected alternative is shipping the table inside `ical-tz` behind a non-default
feature, on the ground that this ADR's premise is that bundled data makes the library wrong when
the world moves *and invisibly so*, and the second half is avoidable by stamping the generated
file. That is a real weakening of the premise and it is why the reopening condition below has a
clause about provenance. It loses on a narrower and better-supported ground than "a bundled table
would hide its age", which it need not: a bundled table would hide that it *chose*. Reopening
requires all three of — measured against `ical-conform`'s corpus — at least 90% of the distinct
`TzidForm::Opaque` identifiers being `windowsZones` keys that appear in calendars carrying no
usable `VTIMEZONE` for them, a proposed table preserving (Windows identifier, territory) keys and
the full ordered candidate list so the crate reports the choice instead of making it, and a table
naming the CLDR release it was generated from in a value a caller can read at runtime with a gate
failing when it falls behind. Failing any one of the three, the refusal stands and this is closed.
