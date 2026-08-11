# The decision docket

Every milestone in this workspace ended by writing down what it had not decided. Those
sentences are still where they were written — in the Consequences sections of the eleven ADRs,
in the closing sections of the six design documents, in each milestone's "known and named" list
in `ROADMAP.md`, and in the `# Status` block of each crate's `lib.rs`. This file collects them,
separates the ones that are decisions from the ones that only read like decisions, and fixes in
advance what would settle each.

Nothing here decided anything when it was written. A docket item is a question with an address,
not a verdict — and every one of the twenty-one below now carries the verdict that answered it,
beside the question it was asked as. The questions stay because the record of what was open is
part of what makes these documents trustworthy; a reader who wants only the answers reads the
**Verdict** line under each item, and the reasoning lives in the ADR amendment it names.

Three of the verdicts did not answer the question as posed, and those are the ones worth reading
in full. D-0009's threshold was executed and disqualified itself. D-0002's threshold could not
have fired, because the exhaustion it counted is a single latched event. And D-0012's source
quotation is factually wrong about RFC 6638, which is corrected under that item rather than
silently repaired.

## What a class means

- **judgment** — genuinely undecided, and decidable today from evidence that already exists:
  a specification's text, this workspace's own code, or a rule the project already holds.
- **measurement-bound** — undecided and not decidable without a measurement nobody has taken.
  Each such item states the measurement and the threshold *here*, before the number is known, so
  that a later observation decides it rather than being argued about after the fact.
- **work-not-decision** — the decision was made and only the code is missing. Ejected below.
- **document-only** — already answered elsewhere in the tree, or answered by a later milestone
  and never struck. Ejected below, with the evidence.

## What a tier means

- **Tier 1** — the answer space has three or more real options and the wrong answer is expensive
  to undo: it changes the set of published crates, a frozen public name, or what a CI gate
  means. There are three.
- **Tier 2** — everything else that needs evidence gathered, or a measurement taken, before a
  verdict.
- **Tier 3** — answerable by applying a rule the project already holds. Each Tier 3 entry names
  the rule.

## What a consumer means

The later workflow that is blocked on the answer: **Query** (the filter evaluator), **Vocabulary**
(`ical-dav`'s remaining rows and the XML audit), **Harness** (`ical-conform`'s public API),
**Corpus** (provenance and differential subjects), **Outbound** (the iTIP sending direction),
**Debt** (implementation debt). An item with no consumer earns its place only by blast radius.

---

## Tier 1

### D-0001 — Is the foreign-implementation bridge a live subprocess or a versioned static matrix?

- **Source**: `docs/adr/0006-conformance-corpus-as-artifact.md:86`
- **Quote**: "It was never sketched or scored, so it was not adopted; it is owed a prototype and
  a bake-off, and until then the bridge is the decision but not a settled one."
- **Class**: judgment · **Tier**: 1 · **Blast radius**: ci-gate
- **Consumers**: Harness, Corpus
- **Related**: D-0008
- **Verdict**: **amend-mechanism** — the checked-in matrix is the record and the live bridge is
  demoted to an off-path refresher; the M5 owed gate is the corpus against the matrix. Landed as
  ADR 0006 amendment 1, with the threshold for promoting the refresher fixed there.

ADR 0006 names its own unadopted alternative and attaches an instruction to it. That sentence is
what puts this item first. The live bridge spawns `libical` and a JS engine per case under a
wall-clock timeout; the static matrix records each foreign implementation's answers offline,
commits them, and diffs them in review. The answer space is at least three — live only, static
only, or static as the record with the live job as the staleness detector, which is what
`docs/design/ical-conform-api.md:363` already proposes under "Deliberately rejected" without
scoring it.

The wrong answer is expensive because it decides what a CI job means. M5's owed gate is exactly
this job (`ROADMAP.md:381`), it does not exist, and until this is settled the suite cannot say
whether a red bridge means an implementation changed, a tzdata bump moved a golden answer, or a
child process hung. ADR 0006 also records that `std::process::Command` exists on neither
cross-target this workspace builds for, so a live-only answer permanently excludes the wasm
build from the differential claim — a cost the static matrix does not pay.

### D-0002 — What crates exist around `ical-dav`: does a filter evaluator get one, and is the WebDAV grammar extracted at the same time?

- **Source**: `ROADMAP.md:296`, `docs/adr/0004-sans-io-protocol-layer.md:108`,
  `docs/adr/0004-sans-io-protocol-layer.md:244`
- **Quote**: "It is still the largest single piece of a server that this workspace does not
  contain."
- **Class**: judgment · **Tier**: 1 · **Blast radius**: crate-set
- **Consumers**: Query, Vocabulary
- **Related**: D-0003
- **Verdict**: **decide-new** — `ical-query` is published above the spine; `webdav-core` is not
  published and the untangling happens now. Landed as ADR 0012, with amendment notes in ADR 0004
  (amendment 11) and the design document. The budget threshold that would have sized the
  deliverable was withdrawn and replaced, because a shared meter latches globally and its clause
  could not fire.

These are one question wearing two faces, and they must be decided together or not at all.

The first face: evaluating a `time-range` needs recurrence expansion and zone resolution, and
ADR 0004's spine gives `ical-dav` only `ical-core`. So the evaluator cannot live in `ical-dav`
without inverting the graph, and cannot live in `ical-recur` or `ical-tz`, which are siblings
that do not know what a `comp-filter` is. It is either a new crate above all three, or work this
workspace declines to ship and every server author writes again.

The second face: the WebDAV grammar `ical-dav` keeps private is now "a namespace-resolving,
reference-resolving reader and writer rather than a small tag matcher"
(`docs/adr/0004-sans-io-protocol-layer.md:245`), and ADR 0004 calls its extraction "a deferred
cost, not a rejected one" whose deferral should be revisited "if CardDAV is closer on the roadmap
than this assumed". Both faces move the same boundary — what this workspace publishes around
`ical-dav` — and both are cheapest to do once. Deciding one and leaving the other open means
either restructuring the graph twice or letting an external caller take a dependency on
`ical-dav`'s internals in between.

The answer space is at least four: no new crate at all; an evaluator crate only; a `webdav-core`
extraction only; both, in one restructuring. Whichever is chosen, ADR 0004's Consequences say a
graph change of this kind "should adopt the full graph and justify the product-scope expansion in
its own ADR rather than let it ride in on this one", and `ROADMAP.md:388` lists vCard and CardDAV
as an undecided scope rather than a non-goal.

### D-0003 — Does `ical-grammar` collapse back into `ical-core` before 1.0?

- **Source**: `docs/adr/0004-sans-io-protocol-layer.md:194`
- **Quote**: "If no real caller ever wants grammar-without-model, the honest move is to collapse
  `ical-grammar` back into `ical-core` before 1.0, and nothing here decides that."
- **Class**: judgment · **Tier**: 1 · **Blast radius**: crate-set
- **Consumers**: Debt
- **Related**: D-0002
- **Verdict**: **amend-mechanism** — `ical-grammar` collapses into `ical-core` before the first
  publish, and the layering rule becomes a second compilation under `gates/`. Recorded as ADR
  0004 amendment 12, re-argued against its own probe as amendment 17, and **executed**: the
  sources are `crates/ical-core/src/grammar/`, `gates/grammar-layering` is a registered member,
  and six crates are publishable rather than seven. Half of act 2 came back textual — the layering
  member cannot see a `crate::X` the crate root re-exports from the grammar — so that half is a
  second rule of `xtask purity` and is described nowhere as something the compiler catches.

It is a docket item, and it belongs in Tier 1 for the same reason D-0002 does: it changes the set
of published crates and it is expensive to undo after external callers exist. It is not the same
item as D-0002, because it removes a crate from below the spine while D-0002 adds crates beside
and above it, and because the evidence that settles it is different.

The evidence is already gathered and points one way, which is why this needs a verdict rather
than a study. The seam was justified as a compile-footprint saving for a grammar-only consumer.
That consumer has not appeared; the crate has since acquired the diagnostic vocabulary, the sink,
`Limits`, `Meter` and `Instant` (`docs/adr/0011-civil-time-arithmetic-and-resolution-types.md:34`,
now `crates/ical-core/src/grammar/mod.rs`), so the saving is smaller than it was when ADR 0004 already
called it "insurance, not demonstrated demand"; and `ical-core` re-exports every item of it with
a glob so that "the seam is meant to be invisible". Against that: collapsing costs a published
crate name that cannot be unpublished, and ADR 0004's purity gate lists `ical-grammar` in
`CORE_CRATES`, so the gate's own list changes with it.

---

## Tier 2

### D-0004 — What a non-UTF-8 `DAV:href` does on the way out

- **Source**: `docs/adr/0001-lossless-round-trip.md:307`, `docs/adr/0004-sans-io-protocol-layer.md:463`
- **Quote**: "Percent-encoding on the way out without decoding on the way in would break the
  round trip, and decoding would erase the difference between `%2F` and `/`. Nobody has designed
  the third answer."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: public-name
- **Consumers**: Vocabulary
- **Verdict**: **decide-new** — percent-encode on the way out, never decode on the way in, and
  offer the inverse as a segment-wise equivalence. Landed as ADR 0004 amendment 13 and ADR 0001
  amendment 8; ADR 0001’s round trip becomes conditional for one class of `href`.

Both ADRs record the same hole from their own side, and both name only the two answers that do
not work. The consequence is live: a body carrying such an `href` is a body a conformant peer
discards whole, which is the same class of loss ADR 0004 Amendment 7 refused for
`calendar-data` — and there the answer was to refuse the write. Refusing an `href` is a third
answer nobody has costed; so is percent-encoding on the way out while recording that it happened,
in the shape `CalendarPayload::is_as_sent` already uses for line endings. This is decidable from
RFC 3986 and RFC 4918 as they stand, and the shape it lands on is `Href`'s, which is a public
name.

### D-0005 — Whether a zone source may be asked how old its data is

- **Source**: `docs/adr/0003-caller-supplied-time-zones.md:120`
- **Quote**: "Fixing that needs an as-of date attached to a source, which the no-clock rule makes
  genuinely awkward — the library cannot ask what "now" is, so it would have to be a
  caller-supplied assertion. That is an unmade decision, not a deferred implementation."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: public-name
- **Consumers**: Query, Corpus
- **Related**: D-0007
- **Verdict**: **decide-new** — a source may be asked how old its data is, the vintage names who
  claimed it, and nothing in this workspace compares two of them. Landed as ADR 0003 amendment 15.
  The corpus grep this was going to defer behind is withdrawn: every outcome of it yielded the
  same design.

The ADR labels this itself. `AnswerBasis` closed coverage and left staleness open: two sources
whose rules differ by eight years both answer `Computed`, the caller gets a bare `Disagreed`, and
nothing in the type says which side is old. The answer space is a caller-supplied as-of assertion
on the source, a third `AnswerBasis` variant, or nothing — and "nothing" is a real option this
item must let win, since the no-clock rule of ADR 0003 is the principle and an as-of field is a
mechanism hung off it. Whatever wins changes `ZoneSource` or `ZoneAnswer`, both frozen public
names that four crates already match on.

### D-0006 — Whether this workspace may ship a vendor-identifier alias table

- **Source**: `docs/adr/0003-caller-supplied-time-zones.md:128`
- **Quote**: "This ADR forbids bundling time zone *data*; whether an identifier alias table counts
  as data or as vocabulary is undecided, and every caller hits the question on its first Outlook
  file."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Query, Corpus
- **Related**: D-0002
- **Verdict**: **decide-new** — an alias table is data; no crate published here answers which IANA
  zone a vendor string means, scoped by question rather than by crate. Landed as ADR 0003
  amendment 16, with three conditions that would reopen it.

`TZID:Eastern Standard Time` and `/mozilla.org/20050126_1/Europe/Berlin` are what Exchange and
Lightning write, an IANA-backed source must answer `None` to both, and the mapping is the
caller's visible step. Every caller writes the same table. The options are at least three: keep
refusing (the table is data), ship it inside `ical-tz` behind a feature (it is vocabulary), or
ship it as a separate crate outside the purity rule the way `ical-conform` sits outside it —
which is why the blast radius is the crate set and not one signature. The rule to argue it
against is ADR 0003's own: bundled data makes the library wrong when the world moves, and a
CLDR windowsZones mapping moves too, though far more slowly than tzdata.

### D-0007 — Whether continuing the final observance is the right reading of section 3.6.5

- **Source**: `docs/adr/0003-caller-supplied-time-zones.md:132`
- **Quote**: "Whether "continue the last observance" is even the right RFC reading for an
  exhausted `RDATE`-only `VTIMEZONE` has been treated here as the defensible default without
  being confirmed against section 3.6.5's observance-selection language or against what libical
  does."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Corpus
- **Related**: D-0005, D-0001
- **Verdict**: **amend-mechanism** — continuing the final observance is section 3.6.5 executed
  literally, confirmed against the text and four implementations; what does not survive is the
  label on `coverage_end`. Landed as ADR 0003 amendment 14, with two `TZUNTIL` thresholds fixed in
  advance. No golden-list row moves.

Half of this is decidable today: section 3.6.5's observance-selection language exists and nobody
has read it against this implementation. The other half — what libical does — is exactly what
D-0001 decides how to find out, so this item's second leg is scheduled behind that verdict rather
than blocked forever. What is *not* on this docket is what a caller should do with a
`BeyondKnownTransitions` answer: ADR 0003 refuses to prefer a source on purpose, and
`crates/ical-conform` already pins the defensibly wrong answer with a comment saying so. The
question here is narrower and answerable: is continuing the final observance the reading, or is
some other selection rule the reading, and does the wrong choice show up as a
`time-zone-coverage-exhausted` code on a case that should not have one.

### D-0008 — Whether `ical-conform` grows a fourth comparison class

- **Source**: `docs/adr/0006-conformance-corpus-as-artifact.md:81`
- **Quote**: "A closed question vocabulary buys a loud failure at the price of an amendment: the
  next comparison class — iTIP `SEQUENCE`/`COUNTER` arbitration, CalDAV `REPORT` result sets —
  fits none of the three variants and reopens this ADR."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: public-name
- **Consumers**: Harness, Corpus
- **Related**: D-0001
- **Verdict**: **amend-mechanism** — one fourth comparison class, `Exchange`, portable rather than
  native. Landed as ADR 0006 amendment 2. The loud break this ADR reserved is spent for the first
  time.

Written as a prediction; M3 and M4 have since shipped, and the corpus now holds
`break_itip_*.rs` and `break_dav_*.rs` cases that are exactly what the sentence names. The
vocabulary is deliberately not `#[non_exhaustive]` (`docs/design/ical-conform-api.md:346`) so that
a fourth class breaks downstream builds rather than being misfiled in silence — which means
adding one is a semver event on a published crate, and *not* adding one means the iTIP and DAV
cases are filed as `Derived` under a canonical encoding that would have to be authored for them.
Three options: a fourth variant, an encoding that makes them `Derived`, or a ruling that both
belong to `NativeCase` and are never portable.

### D-0009 — The default `Limits` numbers

- **Source**: `docs/adr/0010-shared-resource-limits.md:69`, `docs/adr/0007-allocation-policy.md:76`,
  `docs/adr/0004-sans-io-protocol-layer.md:266`
- **Quote**: "The numbers are not chosen: a budget right for a phone rendering one month is wrong
  for a server indexing a decade, and calibration belongs to whoever ships the first recurrence
  milestone."
- **Class**: measurement-bound · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Corpus, Harness
- **Verdict**: **amend-mechanism** — the threshold fixed here was executed and disqualified
  itself, because a conformance corpus measures how fixtures are authored. Replaced by a stated
  envelope per policy and a calibration marker per field: see ADR 0010 amendment 1, which
  supersedes the measurement and threshold paragraphs below.

One dimension of this is closed and the rest are not. M1 calibrated `DEFAULT_CANDIDATE_BUDGET` and
`occurrences_per_search` against a workload table in `crates/ical-recur/src/accounting.rs`
(ADR 0002 amendments 7 and 9). What remains uncalibrated is the allocation budget — ADR 0007 says
plainly that "that number wants corpus measurement before it enters an ADR" — and the DAV size
caps, where ADR 0004 records that "one fixed number either rejects a legitimate large multiget
response or is loose enough to weaken the memory guarantee".

**The measurement, fixed in advance — executed, and withdrawn.** It read: over the committed
corpus plus the real-client exports `ROADMAP.md` M5 will add, record for each uncalibrated
dimension the maximum value any non-hostile committed fixture requires, and the value at which
peak charged bytes exceeds a fixed multiple of input size. **The threshold, fixed in advance —
withdrawn with it.** It read: `Limits::DEFAULT` for a dimension is the smallest round number at or
above twice the maximum a non-hostile fixture requires, `Limits::GENEROUS` the same statistic at
ten times, and a dimension whose number exceeds the peak-allocation ceiling is reported back here
as a design defect rather than resolved by picking the larger.

The first half of that measurement was run and it reports that the population is wrong: the
largest non-hostile committed fixture is 1,815 octets, because a conformance corpus is built to
isolate one fact per file, so the statistic describes authoring convention rather than what
clients emit. Executed literally the rule yields an input budget near 4 KiB and an item ceiling
near 128, which refuse essentially every real calendar — the interoperability failure ADR 0007
named in advance as disqualifying, so the rule's own output was already declared unacceptable by
the ADR it serves. This paragraph is the report-back the rule's last sentence requires for the
analogous case. What replaces it is in ADR 0010 amendment 1: `max_input_bytes` is the stated
allocation envelope of a named deployment, every field carries one calibration marker, and the
corpus falsifies a default rather than setting one. Withdrawing a threshold fixed in advance is
the precedent this file exists to prevent, and the only thing separating it from arguing after
the fact is that the measurement that killed it is recorded here with it. Any future withdrawal
owes the same.

### D-0010 — Whether the token layer grows a need-more-input outcome

- **Source**: `docs/adr/0007-allocation-policy.md:85`, `docs/adr/0008-parser-layering-and-pull-api.md:45`
- **Quote**: "Until that changes, "use the streaming layer instead" is a promissory note, and the
  honest reading here is that large values are refusable, not processable, anywhere in this
  workspace."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: public-name
- **Consumers**: Debt
- **Verdict**: **amend-mechanism** — the chunked protocol already exists and the folds are the
  interior structure, so two clauses of ADR 0007 were false; the promissory note is about input
  residency. Landed as ADR 0007 amendment 1 and ADR 0008 amendment 1. No need-more-input outcome
  and no second protocol.

ADR 0007 clause (4b) sends a caller who cannot afford a value to the pull layer; ADR 0008 decided
that the pull layer may not answer that it needs more input, and paid for object safety with
exactly that outcome. Both are stated, and together they say the promised escape does not exist.
The answer space is at least three: keep the refusal and strike clause (4b)'s promise; add the
outcome and lose `&mut dyn ContentLineSource`; or add a chunked value protocol beside the token
enum rather than inside it. `Token` and `ContentLineSource` are semver-load-bearing by ADR 0008's
own Consequences, so this is a public-name decision whichever way it goes, and the cheapest
correct answer may be the first one — amending a promise rather than a mechanism.

### D-0011 — The field-permission table's per-method dimension, and whether a change the actor may not make is ignored or refused

- **Source**: `docs/design/ical-itip-api.md:536`, `ROADMAP.md:207`
- **Quote**: "the field-permission table is a dozen lines of `field_rule` standing in for RFC
  5546's per-method restriction tables, which run to pages."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Outbound
- **Related**: D-0012
- **Verdict**: **amend-mechanism** — `field_rule` takes the method, and a payload stating a change
  its author may not make is refused rather than dropped. Landed as ADR 0005 amendment 12.

Two observed failures, one cause. A legitimate `COUNTER` is refused, which the design document
predicted it would prefer and the roadmap now records as observed rather than predicted. And an
attendee's `REPLY` carrying a moved `DTSTART` is *ignored* rather than refused — the transition
holds only the sender's own `ATTENDEE` line, so the gate's own guarantee holds, but "a caller that
applies `Authorization::message`'s payload instead of the transition moves the meeting"
(`ROADMAP.md:207`). Both are answered by deciding whether `field_rule` acquires a per-method
dimension read from the same transcribed section 3 tables the conformance gate already uses, and
whether a payload stating a change the actor may not make is a denial rather than a silent drop.
The evidence is the RFC text and the tables already committed as data; nothing here waits on a
measurement.

### D-0012 — The delegation rules, and whether a delegate's reply lands in one turn

- **Source**: `docs/adr/0005-scheduling-apart-from-the-model.md:108`, `ROADMAP.md:212`
- **Quote**: "The CAL-ADDRESS / `SENT-BY` / `SCHEDULE-AGENT` delegation rules are gestured at
  above rather than specified."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Outbound
- **Related**: D-0011
- **Verdict**: **amend-mechanism** — `SCHEDULE-AGENT` leaves the sentence as a category error,
  `SENT-BY` gets an agent-aware own-occurrence lookup, and the delegate’s hold is named with its
  real release. Landed as ADR 0005 amendment 13. Note that the quoted source below is wrong on one
  point: RFC 6638 section 1 excludes delegating and section 7.1 forbids both sides from emitting
  the parameter, so there are no `SCHEDULE-AGENT` semantics for this gate to apply.

The ADR gestures and M3 shipped a partial reading: `SENT-BY` is matched, `DELEGATED-TO` was found
to be a route an attacker could reach the gate through (ADR 0005 amendment 7's neighborhood), and
a delegate's `REPLY` "describes nothing until the delegator's own reply has been applied, which is
RFC 5546's order but is not what a caller wanting one turn gets". Whether that ordering is the
answer, or whether the gate composes the two turns, is undecided and decidable from RFC 5546
sections 3.2.3 and 3.2.5 together with the `SCHEDULE-AGENT` semantics RFC 6638 gives. No zone, no
measurement, and no server is required to settle it.

**Correction.** The last clause of that paragraph is wrong and is not repaired silently. RFC 6638
section 1 excludes delegating from its scope, and section 7.1 says servers MUST NOT include
`SCHEDULE-AGENT` in any scheduling message they send and clients MUST NOT include it in any they
send — so there are no `SCHEDULE-AGENT` semantics for an iTIP message gate to apply, and the
parameter can never reach one. It governs a stored scheduling object resource and belongs to
`ical-dav`'s vocabulary. The ADR sentence this item quotes was a category error in one of its
three terms, and ADR 0005 amendment 13 strikes it rather than answering it.

One sub-question of this item is genuinely measurement-bound and its procedure is in that
amendment: whether a delegator's `REPLY` may carry two `ATTENDEE` lines, which RFC 5546
contradicts itself about across four sections. Two captures apiece from four clients, one or more
producers emitting the shape adopts the delegator-authored addition, and if none can be captured
the refusal stands by default and is recorded as never having been tested against a real
producer.

### D-0013 — Whether the octet diff may report "unchanged" for text it cannot decode

- **Source**: `docs/adr/0005-scheduling-apart-from-the-model.md:106`
- **Quote**: "a CP1252-mangled value can report "unchanged" for an organizer-only field an
  attendee touched, and no gate above the diff sees it."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Outbound, Debt
- **Related**: D-0014
- **Verdict**: **decide-new** — the octet diff stays octet, and the charset is judged at the iMIP
  door. Landed as ADR 0005 amendment 14; the guarantee is iMIP-shaped and the Consequences
  sentence is narrowed rather than struck.

The diff compares preserved octets, chosen in M3 because its failure direction is refusal rather
than permission — and this is the case where that reasoning does not hold, because the failure
direction here is permission. It is a security question with three answers: keep octet equality
and accept the hole; compare decoded text and refuse a comparison that cannot be decoded; or
require the value to have been swept for validity earlier, which is D-0014's subject and is why
these two are related rather than merged. The two are separable: D-0014 decides whether anything
establishes validity unasked, and this one decides what the gate does when validity is unknown at
the moment of comparison.

### D-0014 — Whether a violation is established without a caller asking

- **Source**: `docs/adr/0001-lossless-round-trip.md:98`, `docs/adr/0001-lossless-round-trip.md:116`
- **Quote**: "An eager parse-time sweep over text properties would close that gap cheaply and is a
  named follow-up, not a rejected idea."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: public-name
- **Consumers**: Corpus, Debt
- **Related**: D-0013
- **Verdict**: **amend-mechanism** — the eager sweep is adopted, on reassembled octets rather than
  raw ones, and the serialize-diagnostics shape is rejected on the record. Landed as ADR 0001
  amendment 9, with a one-time frozen-meaning exemption recorded as ADR 0009 amendment 2.

ADR 0001 names one tension and two unbuilt answers to it, and they are one question. Decoding is
lazy, so invalid bytes in a property nothing reads survive parse, round trip and iTIP processing
with no diagnostic; and the entailment audit is advisory, so "a caller who never asks ships the
corrupt file, and no gate here can catch that". The recorded candidate fixes are an eager
parse-time sweep and "a serialize path that returns diagnostics alongside bytes, and that API
shape is not decided here". Either changes a public door — `Document::parse`'s cost and diagnostic
output, or `Document::serialize`'s return type — and the third answer is to do neither and state
the limit, which ADR 0009 already models for the mojibake case that no channel can detect at all.

### D-0015 — Whether the gap-case default stays skip

- **Source**: `docs/adr/0011-civil-time-arithmetic-and-resolution-types.md:59`
- **Quote**: "The gap-case default is skip, per the MUST, and it is the one default here the
  corpus may overturn: §3.3.5 resolves a nonexistent explicit DATE-TIME using the offset before
  the gap, and Google and Apple are reported to shift instead. If real exports say shift, the
  default flips; the two-gate structure does not."
- **Class**: measurement-bound · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Corpus
- **Verdict**: **defer-with-procedure** — skip stays the default; the flip needs three producer
  exports, two shifting and none skipping, and is struck rather than rolled forward if they cannot
  be obtained by the close of M5. Landed as ADR 0011 amendment 4, which also records six library
  subjects and the majority answer no `GapPolicy` variant produces.

The ADR states the flip condition and leaves "real exports say shift" unquantified, which is the
part this docket fixes so the flip is not argued afterward.

**The measurement and the threshold, fixed in advance and carried into the ADR.** Both are now
ADR 0011 amendment 4 rather than only this file, so a later reader meets them where the default
is stated. The flip-bearing leg is unchanged: one committed export apiece from Google Calendar,
Microsoft 365 and Apple Calendar, each containing a recurring series whose expansion crosses a
spring-forward transition with a rule anchored inside the gap hour, with the producer's own
rendered occurrence list as the answer rather than this workspace's; the default flips only if at
least two shift *and* none skips, and fewer than three producers is not a measurement. Two things
are added there. If the exports cannot be obtained by the close of M5 the item does not roll
forward — skip becomes permanent and ADR 0011's flip clause is struck as unfalsifiable in this
workspace, rather than left standing as a promise. And a second leg records what library subjects
answer for one discriminator, carrying zero flip weight by construction because those subjects
read calendars and the flip condition is about what producers write. That leg turned up the fact
this docket did not anticipate: the majority answer is one `GapPolicy` cannot produce.

### D-0016 — Whether the DAV vocabulary extends past RFC 4791

- **Source**: `ROADMAP.md:298`, `crates/ical-dav/src/lib.rs`
- **Quote**: "The vocabulary is CalDAV's and stops there. `MKCALENDAR` (RFC 4791 section 5.3.1)
  has a request body and no row; so does everything in RFC 3744, and a multi-user server without
  ACL is a single-user server."
- **Class**: judgment · **Tier**: 2 · **Blast radius**: crate-set
- **Consumers**: Vocabulary, Query
- **Related**: D-0002
- **Verdict**: **decide-new** — RFC 3744 and the discovery reports are out of scope and move to
  the Non-goals; six roots gain recognition-only rows so a server can name what it declines.
  Landed as ADR 0004 amendment 14.

`MKCALENDAR` is RFC 4791's own and is a row nobody wrote — that half is work. The rest is a scope
decision this workspace has not made: RFC 3744's ACL vocabulary, `DAV:expand-property` and
`DAV:principal-property-search` are other specifications, and `ROADMAP.md`'s Non-goals excludes
vCard and CardDAV without saying anything about these. Taking them in grows what `ical-dav` is
for and makes D-0002's extraction question larger; declining them means a reader of the roadmap
should be told a multi-user server is out of scope rather than left to discover it. Three options:
model them in `ical-dav`, model them in whatever D-0002 extracts, or declare them out of scope in
the Non-goals list where CardDAV already sits.

---

## Tier 3

### D-0017 — Whether ordinary callers face `MonthAddOutcome`

- **Source**: `docs/adr/0011-civil-time-arithmetic-and-resolution-types.md:74`
- **Quote**: "The objection this leaves standing is whether ordinary callers should face that enum
  at all, rather than a single default with the three-way outcome reserved for the `RRULE` entry
  point."
- **Class**: judgment · **Tier**: 3 · **Blast radius**: public-name
- **Consumers**: Debt
- **Verdict**: **amend-mechanism** — the enum stays, on a count of one non-test call site, and
  convenience is permitted on the outcome and never as a second entry point on the date. Landed as
  ADR 0011 amendment 5.

**The rule to apply**: ADR 0011's own Context rejects "a single silent policy — clamp January 31
to February 28" because it "answers the case where the specification has an answer and the case
where it has none identically, erasing the distinction a caller needs", and ADR 0003's
Consequences hold the same line for ambiguous and nonexistent local times. A convenience default
outside recurrence is that same erasure with a smaller audience. The enum stays unless the rule is
overturned, which is an ADR change and not an ergonomics call.

### D-0018 — Whether an unsatisfiable rule is diagnosed rather than merely bounded

- **Source**: `docs/adr/0011-civil-time-arithmetic-and-resolution-types.md:77`
- **Quote**: "An unsatisfiable rule is bounded but not diagnosed: it ends as budget exhaustion,
  which is what a merely rare rule produces too."
- **Class**: judgment · **Tier**: 3 · **Blast radius**: crate-set
- **Consumers**: Debt, Corpus
- **Verdict**: **amend-mechanism** — a distinct decode-time `Note`, which overturns ADR 0011’s
  refusal of a static check. Landed as ADR 0011 amendment 6; the rare rule still ends as budget
  exhaustion and the amendment says so.

**The rule to apply**: ADR 0009 requires that a fact a caller and this crate would read
differently must not be silent, and ADR 0002 amendment 14 applied exactly that rule to a
divergence it could have left quiet. "Can never match" and "did not match within the limit" are
two facts arriving as one value, which is the same defect one dimension over. The ADR's refusal to
mandate a *static* pre-flight check is untouched by this: a diagnostic raised when the expansion
itself discovers that every period filtered is not a pre-flight check, and dateutil's issue 523 is
evidence that the distinction is worth a caller's while.

### D-0019 — Whether `ZonedSeries::admits` charges the ledger

- **Source**: `docs/adr/0011-civil-time-arithmetic-and-resolution-types.md:93`
- **Quote**: "The per-instance zone query the second gate needs debits nothing at all —
  `ZonedSeries::admits` takes no ledger — so two conforming implementations can still differ by
  orders of magnitude in when they report exhaustion for a rule whose cost is mostly zone
  lookups."
- **Class**: judgment · **Tier**: 3 · **Blast radius**: public-name
- **Consumers**: Debt, Query
- **Verdict**: **amend-mechanism** — every zone lookup a series makes is charged at one funnel,
  under a derived default of four times the occurrence ceiling. Landed as ADR 0011 amendment 7 and
  ADR 0010 amendment 2. `max_zone_lookups` is recorded as derived and sits outside D-0009’s corpus
  measurement.

**The rule to apply**: ADR 0010's "a bound nobody charges is decoration", and its sibling recorded
in ADR 0003 amendment 6, "a charge nobody reports". ADR 0003 amendment 13 has since measured what
one such lookup can cost — about 35 ms against a definition with 2,000 rule-bearing observances —
so this is no longer a lookup that is cheap enough to leave uncounted. The signature is public and
`ical_recur::RecurrenceInput::admitting` takes it, so applying the rule moves two names.

### D-0020 — Whether `calendar-multiget` admits `DAV:allprop` and `DAV:propname`

- **Source**: `ROADMAP.md:309`
- **Quote**: "nobody is known to send the other two to a multiget, and that is a reason to file it
  rather than to call it closed."
- **Class**: judgment · **Tier**: 3 · **Blast radius**: crate-set
- **Consumers**: Vocabulary
- **Verdict**: **amend-mechanism** — `calendar-multiget` carries `QueryShape`, and the production
  rather than symmetry is the rule that says which bodies do. Landed as ADR 0004 amendment 15.

**The rule to apply**: the one M4 already applied one element over. `DAV:allprop` and
`DAV:propname` inside a `calendar-query` became `QueryShape` because "section 9.5's own production
is a body this crate reads and writes rather than one it answers `DavError::Unexpected` to". RFC
4791 section 9.10's grammar admits the same three shapes for a multiget. A body the grammar admits
and this crate refuses is the same defect in the same place, and "nobody is known to send it" is
the argument M4 declined for `calendar-query`.

### D-0021 — Who governs the tool that enforces the purity rule

- **Source**: `docs/adr/0004-sans-io-protocol-layer.md:230`
- **Quote**: "the gate has no custodian of its own — `xtask` is governed by no purity rule, so the
  same pull request that breaks the rule can weaken the check that enforces it, and
  `ical-conform`'s exemption is still just absence from a hand-maintained const."
- **Class**: judgment · **Tier**: 3 · **Blast radius**: ci-gate
- **Consumers**: Debt
- **Verdict**: **amend-mechanism** — four legs inside the existing gate, stated over sets and
  member roots so they survive D-0002 and D-0003 landing in the same wave. Landed as ADR 0004
  amendment 16, which also records the sequencing obligation the three gate edits share.

**The rule to apply**: `docs/diagnostic-codes.md` already states it — "the tool that enforces 'the
core has no outside dependencies' may not acquire one" — and ADR 0004 already applied the
staleness half of it by failing a `no_std` crate that is absent from `CORE_CRATES`. The rule
exists, it is written down twice, and nothing checks the tool against it. What is left is to say
which mechanism carries it, not whether it should be carried.

---

## Ejected

These read like open questions and are not decisions. Each is recorded here so that a later reader
does not put it back on a docket, and each names where it actually goes.

| ejected | source | why | goes to |
| --- | --- | --- | --- |
| The `ical-conform` public API and its canonical encodings | `docs/design/ical-conform-api.md:392`, `:380` | work-not-decision. ADR 0006 decided that a `Derived` answer is compared "in a canonical encoding this project specifies", and the design document specifies the vocabulary. "None of this exists" is a statement about unwritten code, not an undecided question. | Harness |
| The two private element encoders in `write_request.rs` and `write_response.rs` | `docs/design/ical-dav-api.md:764`, `ROADMAP.md:358` | work-not-decision, and the most convincing impostor on this list: it says "unresolved rather than decided" and then names the target (`XmlWriter`), the obstacle (a `&mut Meter` borrow held for the writer's lifetime), and that it "changes no output byte". Somebody decided; nobody wrote it. | Debt |
| What `ical-dav` holds when `alloc` is off, and whether an owned `MultiStatus` exists there | `docs/adr/0004-sans-io-protocol-layer.md:206` | document-only. ADR 0007 clause (1) says these crates are `no_std` *and* `alloc` and that "there is no allocation-free build of these crates", and clause (5) puts the allocation-free tier in a future crate with its own lint profile. The question was answered by a later ADR and never struck here. | Debt |
| The entailment-audit rows nobody wrote | `docs/adr/0001-lossless-round-trip.md:229` | work-not-decision. Amendment 4 already names which claims are unbuilt — the CDO all-day pair, `UNTIL` against `DTSTART`, section 3.6.6's `DURATION`/`REPEAT`, section 3.6.2's `DURATION` — and the audit is the decided answer for all of them. The one that was blocked on `ical-recur`'s grammar is unblocked; M1 shipped. | Debt |
| Deriving the golden list from the emission sites | `docs/adr/0009-error-and-diagnostic-model.md:66` | work-not-decision. "Deriving one from the other is the real fix and is not here" is a decision with the implementation missing. | Debt |
| M0's two owed gates: 200,000 one-byte properties, and a peak-allocation ceiling | `ROADMAP.md:27` | work-not-decision. ADR 0007 clause (4b) decided the ceiling gate exists; the multiple it asserts is D-0009's measurement, and the hostile-input case is a fixture. Neither is a verdict. | Debt |
| A compiled minimal-usage example per crate at its declared `alloc` setting | `docs/adr/0004-sans-io-protocol-layer.md:64`, `docs/adr/0005-scheduling-apart-from-the-model.md:76` | work-not-decision. ADR 0004 names the mechanism it wants in the same sentence that says the workspace does not have it. | Debt |
| Attribute count per element, and the unswept `xml:space` and element-content whitespace | `docs/adr/0010-shared-resource-limits.md:104`, `docs/adr/0004-sans-io-protocol-layer.md:265` | work-not-decision. ADR 0010 settles the shape rather than the list — "there is no typed `XmlLimits` sibling, the DAV dimensions are fields of the one `Limits`, and the next one will be too" — so the next dimension is a field to add and a charge site to write. | Vocabulary |
| What a floating `UNTIL` against a zoned `DTSTART` means | `ROADMAP.md:73` | document-only. M2 closed it: "All six questions M1 left open are closed with a test each", and the fact travels on the golden-listed `recurrence-until-not-utc`. The M1 entry was never struck. | — |
| The vtable cost of `&dyn ZoneSource` on Cortex-M | `docs/adr/0003-caller-supplied-time-zones.md:135` | document-only. The Mechanism decides it: "the trait permits dyn, it does not mandate it", and hand-written enum dispatch is offered per target. A benchmark would inform a caller's choice and change no decision here. | Debt (benchmark, optional) |
| A server that wants one global bound across 64 workers | `docs/adr/0010-shared-resource-limits.md:70` | document-only. The Decision answers it: "Splitting a budget across workers is likewise the caller's job; no shared meter ships here, because an atomic counter is not something a `no_std` crate may assume." | — |
| `allocator_api`, and the missing compiling fixed-arena `Document` | `docs/adr/0007-allocation-policy.md:89` | document-only. Clause (5) places the allocation-free tier in a future crate and `ROADMAP.md:391` repeats it; `allocator_api` is unavailable on stable Rust, so there is nothing to decide until it is. | — |
| The period walk's vocabulary on `ical-recur`'s public surface | `docs/design/ical-recur-api.md:692`, `ROADMAP.md:71` | work-not-decision. "That surface is expected to narrow" and "anything published from these four types before the surface is narrowed should expect to move" are a decision; making the modules private is the work. | Debt |
| A `TZID`-qualified `RECURRENCE-ID` inside a repeated hour | `ROADMAP.md:173`, `docs/adr/0011-civil-time-arithmetic-and-resolution-types.md:135` | document-only. ADR 0005 amendment 3 decides it and argues it: comparison is three-valued, `InstanceMatch::Ambiguous` is `AuthorizationDenied::AmbiguousInstance`, and "a guess between the two halves cancels somebody else's meeting". That the denial is common is a cost of a made decision, and ADR 0011 amendment 3 records that the shape which would close it — a fold side on a `RECURRENCE-ID` — is defined by no RFC. | — |
| A cached typed value if the accessors are ever measured hot | `docs/adr/0001-lossless-round-trip.md:168` | work-not-decision. The answer is recorded in advance — "a `OnceCell` bound to the same `RawText`, or a generation counter — never a bare second field" — and what is missing is a benchmark. | Debt |
| The uniform-`Result` accessor and parameter-granular dirty-flag dissents | `docs/adr/0001-lossless-round-trip.md:181`, `docs/design/ical-core-api.md:843` | document-only. ADR 0001's Decision fixes both mechanisms — one wrapper for the three states across every accessor, and a guard whose unit "is the whole property — its name, its parameters, and its value together" — and M0 shipped them. The invalidation-granularity question deferred to ADR 0004 was answered there by `Revision` binding an `ETag` rather than by a finer dirty flag. | Debt (dissent record) |
| The two unpaid costs of the zero-dependency rule | `docs/adr/0004-sans-io-protocol-layer.md:234` | document-only. The Decision states the rule for all five core crates and `xtask purity` enforces it; what the paragraph records is a weakness in the *argument* for a rule that is made, and this project's own convention is to amend a mechanism rather than overturn a principle. | — |
| The deferred override index for paged stores | `docs/adr/0002-bounded-lazy-recurrence.md:87` | document-only for v1. The ADR states the v1 answer in the same sentence — "v1 callers flatten to a borrowed sorted slice" — and the later observation that a paged store must answer "maximum absolute shift" without materializing itself sharpens the cost of a deferral already taken. | Debt |
| No rule relating an attacker-controlled skew to the budget | `docs/adr/0002-bounded-lazy-recurrence.md:150` | document-only. The ADR answers it in the same paragraph: the candidate budget bounds the skew into a reported outcome, "which is this ADR working as designed", and "a hostile shift and a legitimate one are textually identical", so no rule can separate them. | — |
| The `ical-core` / `ical-itip` change-vocabulary coupling | `docs/adr/0005-scheduling-apart-from-the-model.md:89` | document-only. The ADR states its own revisit trigger — "revisit it if that vocabulary grows to where most properties are never iTIP-relevant" — and the trigger is not met. A dissent with a stated, unmet condition is not an open decision. | — |
| `RANGE=THISANDFUTURE` splitting in `ical-itip`, and `ValueKind` against `ical_core::ValueType` | `docs/design/ical-itip-api.md:530`, `docs/design/ical-recur-api.md:652` | work-not-decision. ADR 0002 decided the anchor semantics and M1 built the composition; splitting a series through `ical-itip`'s types is the code. The `ValueKind` deletion is conditional on a trigger — `ical-core` narrowing its own value type — that has not fired. | Debt |
| The scheduling outbox `POST` and `CALDAV:schedule-response` | `ROADMAP.md:311` | work-not-decision. RFC 6638's preconditions, `schedule-tag` and the inbox and outbox properties are modeled and `ical-itip` holds the semantics; the two missing bodies are rows in a vocabulary whose shape M4 froze. | Outbound |
| The anonymization attestation nobody checks, and fixture provenance | `docs/design/ical-conform-api.md:387` | work-not-decision. ADR 0006 decided that "reduction and anonymization are part of accepting a case, not a cleanup pass" and that "a case records which client and version produced the original". Recording it is the work. | Corpus |
| The three unmeasured ecosystem engines on a `BYDAY` ordinal | `docs/adr/0002-bounded-lazy-recurrence.md:327` | work-not-decision. The reading is decided and shipped — ignore the ordinal, keep the weekday, report `recurrence-rule-part-out-of-range` — and what is owed is an observation for an ADR 0006 divergence case, not a verdict. | Corpus |

---

## What each later workflow must build against

The verdicts above are decisions; this section is what they oblige. It states constraints rather
than summaries, and every line is holdable against a pull request.

### Query (`ical-query`)

- The crate exists and is published: above `ical-core`, `ical-recur`, `ical-tz` and `ical-dav`,
  `#![no_std]`, in the governed crate list, green on both cross-targets. `ical-dav` gains no
  dependency and the filter types do not move out of it (ADR 0012).
- Every entry point takes `&Limits` and `&mut Meter`. Any budget the crate's tests or docs state
  must name the *budget* as well as the policy, because a meter's octet budget is separate from
  `Limits` and naming the policy alone leaves the denominator unstated. Give each resource its own
  meter in any per-resource benchmark: a shared meter across a fan-out measures one latch, not a
  rate.
- Write the walk so a prefilter is an internal step it calls, not a shape it would have to be
  rewritten into. Whether the prefilter ships is decided by the sweep in ADR 0012, and the default
  if the sweep cannot be run is the walk with the prefilter defaulting to "cannot exclude".
- A `TZID` that resolves to nothing is reported and stops the filter. Never fall back to UTC, to
  the calendar's default zone, to the caller's local zone, or to a stripped-suffix guess: a
  time-range filter whose bound cannot be placed is an unanswerable filter, not an empty result
  set and not a full one. Expect an opaque vendor identifier as the normal case (ADR 0003
  amendment 16).
- A vintage is not an input to evaluation: it never selects a source, breaks a tie, or changes
  whether an occurrence matches (ADR 0003 amendment 15).
- Zone queries go through `ZonedSeries::answer_for` or `projected` and are charged there. Do not
  call the source's `resolve` or `offset_at` directly, and do not add a second charging site. A
  new path that spends a third lookup per emitted occurrence invalidates the derived default and
  comes back to this docket rather than raising it (ADR 0011 amendment 7).
- None of the six recognition-only DAV roots carries a filter, so the boundary ADR 0012 draws does
  not move to accommodate them.

### Vocabulary (`ical-dav`)

- `MKCALENDAR` gets a full row and a real request body. The six RFC 3744 and RFC 3253 roots get
  recognition-only rows: unconditionally unsupported in every feature combination, surfacing as
  `DavError::Unsupported(name)` before the unexpected-element arm, with no reader, no writer, no
  request-body variant and no supported-report emitter (ADR 0004 amendment 14).
- `DavError::Unsupported` is not an HTTP status and its documentation must say so in those words.
  For those six roots the deployed answer is `207` with an empty multistatus; `403` with the
  supported-report precondition is the specification's answer and breaks at least one known client.
- `QueryShape` is carried by exactly the two bodies whose production contains the three-way group,
  `calendar-query` and `calendar-multiget`. `principal-match` and `principal-property-search` are
  `prop?` and take an optional property request; `sync-collection` requires `prop`. Any body added
  later quotes its own production in its doc comment, and the production decides the field — never
  symmetry with a neighbor. Land the shared accept-once refusal for the alternation before adding
  new bodies (ADR 0004 amendment 15).
- The XML tokenizer, namespace stack and writer live in one module that names no CalDAV type and
  exports nothing, including through a hidden public re-export. A new row goes in the CalDAV
  tables, never in that module. `webdav-core` is not being published, so no row and no XML change
  may assume a shared crate (ADR 0012).
- `Href` is frozen at byte-shaped storage plus `as_bytes`, `as_str`, `is_as_sent` and
  `addresses_same`; a canonicalizing constructor added later is a public-surface change needing
  its own item. Href escaping is two layers in a fixed order — percent-encoding to the octets, then
  XML escaping to the result — and no audit rule may XML-escape first, or the `%` of an escape
  becomes ambiguous. Any element added later whose value is a URI-reference uses `Href` and
  inherits the transform (ADR 0004 amendment 13).
- `ical-query` becoming a consumer freezes the filter types earlier than the rest of the
  vocabulary: a filter-shape change is now a downstream break.

### Harness (`ical-conform`)

- Publish `PortableQuestion` with all four variants in the change that first exports it. The enum
  is not `#[non_exhaustive]`, so adding the variant after a caller exists is a major version and
  the window in which it is free closes at the first publish (ADR 0006 amendment 2).
- `ConformanceSubject` carries three methods, and the exchange method must be object-safe beside
  the others, because a run mixes one in-process subject with several recorded ones.
- `ForeignRunner` stays published as the seam for out-of-tree subjects, and no in-tree test or
  required check may construct one. Anything needing a live subject belongs in `xtask`. `Observed`
  gains provenance fields — subject version, image digest, tzdata version, date — and they are
  required, not optional (ADR 0006 amendment 1).
- Build the peak-allocation gate to report per-unit retention, bytes per item and bytes per XML
  element, not only a peak-over-input multiple: the multiple alone cannot calibrate `max_items` or
  `max_xml_elements`, whose cost is retention per unit. Until it exists those two fields stay
  marked asserted and the golden count does not fall (ADR 0010 amendment 1).
- Treat no `Limits` value as calibrated because it appears in a ruling. The only fields promoted
  are those whose derivation is written and held by a test.

### Corpus

- A new portable case lands with a foreign column that is either a recorded row with full
  provenance or an explicit "not observed". A case may not be admitted on an unrecorded live
  lookup, and an exchange row a subject produced no answer for is recorded as unmeasured, never as
  agreement (ADR 0006 amendments 1 and 2).
- The iTIP arbitration cases and the DAV result-set cases are filed as exchange cases, not native
  ones. A native case may not carry an arbitration outcome or a multistatus result set. Case
  authors state the actor explicitly per case, because which party is applying is the whole
  question in an arbitration case.
- The corpus sets no `Limits` value. Its job on that axis is falsification: a committed
  real-client export that the default policy refuses is a defect filed against that policy's
  envelope. Two fixtures are owed and neither needs an external export — the divergence where the
  default accepts a two-mebibyte calendar from disk and refuses the same octets as a calendar-data
  text node, and a real-client-shaped inline attachment at the 10 MB the ecosystem accepts,
  recorded as refused (ADR 0010 amendment 1).
- Three sweep-shaped classification passes are owed, each with its threshold already fixed and
  none of them arguable afterwards: `VTIMEZONE` components bucketed as rule-bearing-endless,
  finite or `RDATE`-only with `TZUNTIL` presence broken out by producer (ADR 0003 amendment 14);
  the gap-case producer exports (ADR 0011 amendment 4); and the two-`ATTENDEE` delegation captures
  (ADR 0005 amendment 13). Each names what happens if the captures cannot be obtained, and none of
  them rolls forward by default.
- A vendor-identifier fixture is a case whose expected outcome is nothing plus
  `unknown-time-zone`, or a case where the file supplies its own `VTIMEZONE` under the vendor
  string. There is no third kind, and any mapping a case needs is written in that case's own
  source and never promoted into a shared helper — a shared mapping helper reachable from the
  corpus is the table ADR 0003 amendment 16 refuses, wearing a test's clothes.
- A case asserting a vintage asserts the `Vintage` *variant*, not the bare date: a case recording
  a date without recording who claimed it is not reproducible.
- Write every gap case against skip as the default and do not stage a flip. Any case asserting an
  instant in a gap names the policy that produced it. A Python subject in the discriminator case
  carries its fold or DST flag plus a marked library-default row, and no subject appears in that
  case or in prose citing it until it has a row.
- The post-sweep UTF-8 assertions are the regression test that keeps the predicate off the input
  buffer: the three fold-inside-a-codepoint fixtures must assert *no* `invalid-utf8-text`, and the
  genuinely invalid ones must assert the code, the byte offset, and a byte-identical round trip
  with the diagnostic raised (ADR 0001 amendment 9).
- Three recurrence cases pin ADR 0011 amendment 6 and the second is what stops the check drifting
  into the rejected heuristic: an unsatisfiable rule produces the new code *and* still ends at
  budget exhaustion; `BYMONTH=2;BYMONTHDAY=29` produces no code and emits in the leap year; and
  `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` produces no code and ends at exhaustion, which is the
  recorded non-closure asserted so the amendment's fourth cost stays true.
- The door's refusals at the iMIP charset gate are observable and must be counted: a corpus that
  cannot say how many real messages this drops cannot support the cost recorded against it (ADR
  0005 amendment 14).

### Outbound (the iTIP sending direction)

- A composed `REPLY` states no property whose octets differ from the held copy's except its own
  `ATTENDEE` line. The receiving side now refuses rather than ignores, so echoing a re-folded or
  re-ordered value is a wire failure, not noise (ADR 0005 amendment 12).
- A composed `COUNTER` may state section 3.2.7's scheduling and descriptive rows, and must never
  state `ORGANIZER`, `SEQUENCE`, or another party's `ATTENDEE` line.
- Never emit `SCHEDULE-AGENT` on `ORGANIZER` or `ATTENDEE` in any generated message: RFC 6638
  section 7.1 says clients MUST NOT, and nothing in this workspace would produce it.
- Emit a delegator's `REPLY` with exactly one `ATTENDEE` until the capture in ADR 0005 amendment
  13 lands, and a delegate's `REPLY` with exactly one carrying `DELEGATED-FROM`; expect a
  conformant recipient to hold it. If the measurement adopts the delegator-authored addition this
  constraint inverts.
- Do not generate a delegation `REQUEST`. Forwarding is a byte-for-byte relay of the held
  `REQUEST` with no `SEQUENCE` bump, so any delegation-forwarding API is a relay and not a builder.
  There is no attendee-authored message in this design that adds a party to the list; the outbound
  path that wants the delegate on the organizer's list produces an organizer-authored `REQUEST`.
- When emitting on behalf of another party, write the principal's address as the property value
  and the agent's in `SENT-BY`. Never the reverse, and never both parties as separate `ATTENDEE`
  lines.
- Write `charset=UTF-8` on every `Content-Type` this project emits, including for a pure-ASCII
  body, and `method=` equal to the object's `METHOD`. An outbound message must by construction pass
  our own door under its strictest arm (ADR 0005 amendment 14).
- The scheduling outbox `POST` and the schedule-response body remain rows in a frozen vocabulary
  shape; nothing in this wave waits on them.

### Debt

- **Sequencing is a constraint, not advice.** `xtask purity` is rewritten by three decisions in
  one landing: ADR 0012's governed-list change, ADR 0004 amendment 12's `gates/` member, and
  amendment 16's custodian legs. The custodian edit lands last or merged with the other two. In
  any other order the gate is red on the day the layering member is registered, or a leg is
  written against a list that no longer exists. **The `gates/` member's legs are in, together
  with the act-2 rule amendment 17 added and the `CORE_CRATES`/`core_crates` cross-read: the
  member root walk now covers `crates/` and `gates/`, and the purity partition still covers
  `crates/` alone. The two edits still owed to this file are ADR 0012's and amendment 16's, and
  they are owed against the shape as it now stands.** **Amendment 18 added three more rules to
  the same file — the layering member's string equality, the wildcard `Token` arm, and
  `release-plz.toml` against the published members — so the shape those two edits are owed
  against is now a five-rule `purity`, and the crate set is read out of the root manifest rather
  than written down a third time.**
- **Answered.** The four gates were run against a throwaway `#[path]` member before a byte moved,
  three broke, the precondition fired, and ADR 0004 amendment 17 is the re-argument that narrows
  the verdict rather than defeating it. Their whole repair is `test = false`, `doc = false` and
  one `--exclude`, and all three are in the tree. `cargo package --workspace` stays permanently
  unavailable for this workspace, which is addressed to whoever later adds that command rather
  than to anyone today.
- **Discharged, and these are the properties to keep true.** Six publishable crates, not seven. One
  public spelling for every grammar item: no `ical_core::grammar::` path and no `ical_grammar::`
  path. Files under the grammar directory use `super::`-relative intra-layer imports and carry no
  crate-level inner attributes; naming a model item there fails in `gates/grammar-layering` with
  a file and a line, and naming a grammar item through the crate root fails the second rule of
  `xtask purity`, which is textual — as do an `extern crate`, a `#[path]` and a file that
  directory holds without `mod.rs` declaring it. The grammar tree stays flat, because a
  subdirectory changes the depth that rule is stated in. `gates/grammar-layering` is itself held
  to the workspace by the third rule, because deleting the member used to pass every gate here.
  A wildcard arm over `Token` anywhere under `crates/` is a defect, and the fourth rule is what
  says so: `unreachable_patterns = "deny"` was recorded as saying it and cannot, since the shape
  that loses data omits a variant rather than following all of them. Adding a `Token` variant is
  a minor release for external callers and a compile error in-tree — anything pricing it as a
  seven-crate major is pricing the rejected shape; adding a *field* to one is a major release
  either way, because the variants are not individually non-exhaustive. `release-plz.toml` names
  the published members and nothing else, held by the fifth rule (ADR 0004 amendment 18).
- Do not add a fourth outcome to the token source, and do not add feed, resume or a reader-state
  type. Do not add a second value-chunk protocol beside the existing flag, and do not add a
  pull-layer value ceiling: the absence of one is now a stated capability (ADR 0007 amendment 1).
  Correct anywhere in workspace prose that says or implies a caller can stream a calendar larger
  than its memory.
- Add no month-arithmetic entry point returning a bare or optional `CivilDate`; treat one as a
  review rejection even if a downstream crate asks. The authorized work is exactly four edits —
  add `exact`, rename `date` to `carried_date`, switch the one call site, and list both accessors
  in the two design documents' API blocks — and all four are free at `0.0.0` and semver events
  after `0.1`, so sequence them before any release (ADR 0011 amendment 5).
- The unsatisfiable-rule check consults `ical-core`'s date-validity primitive; a fresh month-length
  table in the decoder is a review rejection. Add exactly one code, no terminal-outcome variant, no
  refusal of the rule or the file, and do not suppress the per-candidate stream — the new code is
  additional to it, because a bounded sink may drop either (ADR 0011 amendment 6).
- The zone-lookup funnel and the gate signature land together or the gate cannot be handed a meter
  (ADR 0011 amendment 7).
- File a separate docket item for the contradiction between ADR 0009's "truncated, flagged, and
  kept" and ADR 0007 clause (4)'s "never a truncation"; do not resolve it inside another one. The
  peak-charged-bytes ceiling gate ADR 0007 promised still does not exist anywhere.
- `field_rule`'s per-method rows are derived from the transcribed section 3 tables at the point of
  use and never re-transcribed beside them; a second copy of section 3 is a second place for the
  two to disagree.
