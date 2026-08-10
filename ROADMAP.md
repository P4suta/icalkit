# Roadmap

Everything here is text in, text out. No network, no clock, no hardware: every milestone
is verifiable by `cargo test` on `ubuntu-latest`.

## M0 — Model and round trip

`ical-grammar` and `ical-core`: the RFC 5545 content line grammar, the component and property
model, and serialization. The milestone is complete when a corpus of real client exports
parses and serializes back byte-identically
([ADR 0001](docs/adr/0001-lossless-round-trip.md)).

Round-trip fidelity first, before anything is interpreted, because it is the property every
later layer has to preserve.

What now binds this milestone. The token layer is the parser and `Document::parse` is one
consumer of the same public path, with no private fast path
([ADR 0008](docs/adr/0008-parser-layering-and-pull-api.md)). Storage is owned bytes charged
against the caller's budget as they are appended, never sliced out of pre-unfold input
([ADR 0007](docs/adr/0007-allocation-policy.md)). A violation is a `Diagnostic` on a sink that
may refuse, and an error means no item could be built at all
([ADR 0009](docs/adr/0009-error-and-diagnostic-model.md)).

Gates this milestone owes, because they cannot be written before the code they read: the
`DiagnosticCode` golden list and its diff check, a round-trip property test over the corpus, a
fold that splits a UTF-8 codepoint, a CP1252 `SUMMARY`, a hostile input of 200,000 one-byte
properties, a peak-allocation ceiling as a multiple of input size, and a structural test that
`Document` is built from the public token path.

## M1 — Recurrence

`ical-recur`: `RRULE` expansion over a caller-supplied window, with `RDATE`, `EXDATE`, and
overridden instances applied inside the iterator. Bounded by a candidate budget, so a
hostile rule is a reported limit breach rather than a hang
([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)).

This is the milestone that makes a month view possible, and the one where every calendar
implementation historically gets things wrong.

What now binds it. The budget is a field of the shared `Limits` and is charged per candidate
*generated*, not per instance emitted, against a `Meter` whose lifetime is the caller's
([ADR 0010](docs/adr/0010-shared-resource-limits.md)). An instance whose date does not exist
is filtered rather than clamped, and it still debits the meter
([ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md)).

Gates this milestone owed, all met: a `FusedIterator` test that calls `next` past the end, a
`COUNT`-bounded resume matching a from-scratch expansion, a negative `BYSETPOS` that cannot
outspend its budget inside one `next`, a `RANGE=THISANDFUTURE` override that changes only a
non-time property, and an instant present in both an `EXDATE` and an override table.

**Met.** All forty-two worked examples of RFC 5545 section 3.8.5.3 are a table test in
`ical-conform`, with the expected column transcribed from the RFC. The item type ADR 0002
committed to did not survive contact with `Iterator`, and fourteen sentences of that ADR are
now amended rather than reinterpreted; the amendments are the record of what shipped.

Six of those amendments are what four adversarial lenses — the RFC's own answers, the budget,
`RDATE`/`EXDATE`/override composition, and the Gregorian calendar — found in the built engine,
and each has a case in `crates/ical-conform/tests/break_recur_*.rs` that failed before the fix.
They were: a meter that reported exhaustion for its octet budget alone, so a search stopped by
either recurrence ceiling left the durable report reading clean; a terminal report that counted
what expansion returned rather than what it charged, so the two rules that spend a budget
without producing anything reported spending nothing; an engine that inferred from the merge's
silence which source a step had consumed, so one `EXDATE` on an `RDATE` could erase the rule
instance after it and an unbounded tail besides; a period walk that deleted the last period of
every cadence to satisfy an upper edge nothing read; a `BYWEEKNO` read as a filter over the
calendar year rather than an expansion of the week-numbering one; and a `BYDAY` ordinal answered
two ways under the frequencies that forbid one, the quieter of which emptied a whole series.

Three things are known and named: emission is ordered by cadence key rather than by effective
start; the period walk's own vocabulary is on the public surface as an integration artifact and
is expected to narrow; and `UNTIL` is compared on the timeline the caller resolved, which is the
right answer for a crate that holds no zone and is why M2 owns the question of what a floating
`UNTIL` against a zoned `DTSTART` means.

## M2 — Time zones

`ical-tz`: `VTIMEZONE` interpretation against a caller-supplied source, DST transition
handling, and explicit representation of ambiguous and non-existent local times. Reporting
where the embedded rules and IANA disagree.

What now binds it. The civil-time primitives are `ical-core`'s and every operation on them is
checked ([ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md)); this crate
owns the resolution types and the source trait, and every caller-facing outcome enum is
`#[non_exhaustive]`. Transition search takes the shared limits and meter
([ADR 0010](docs/adr/0010-shared-resource-limits.md)).

Gates this milestone owed, both met: a compiled `ZoneSource` sketch before implementation
started, and a spring-forward and fall-back case per resolution outcome.

**Met.** `ical-tz` reads a `VTIMEZONE` into a transition table, evaluates its rules in closed
form, resolves a wall clock against a caller-supplied source that names itself in every answer,
and combines two such sources without preferring either. The two awkward hours are values with
three gap policies and two fold policies over them; a table that runs out answers and says the
answer was continued; and a `TZID` is compared by exact bytes and never parsed, so
`W. Europe Standard Time` and `/mozilla.org/20050126_1/Europe/Berlin` are identifiers rather
than puzzles.

The milestone's real subject was the seam with M1, which M1 could only half-specify. It is
settled: the timeline `ical-recur` walks for a zoned series is that series' own wall clock
projected onto UTC, `ical_tz::seam` states the contract, `ical-recur`'s own documentation states
it from the other side, and `crates/ical-conform/tests/break_zones.rs` — the only file naming
both crates — holds a daily 09:00 Berlin series to 09:00 across both 2026 transitions and
asserts that the reading which never re-resolves is 3,600 seconds out from the transition
onwards. All six questions M1 left open are closed with a test each, and four of them left a
golden-listed code behind that no emitter had before: `recurrence-until-not-utc`,
`exdate-value-type-mismatch`, `override-matches-no-instance` and `time-zone-coverage-exhausted`.

Four adversarial lenses — the transitions, the sources, the seam, and the bounds — were then run
against the built crate, and what they found is in `crates/ical-conform/tests/break_tz_*.rs`,
one case per finding, each failing before the fix. Six were wrong answers: a rule stopped being
consulted once four dated transitions stood between it and the query, so `Europe/Berlin`
answered CET on the first of July; a rule that fires rarely was asked about three years and not
the twenty-eight it needs; a zone with two transitions in one day reported seventeen hours of
ordinary wall clock as times that never happened, and answered the two gaps it does have with
the other one's edges; and two observances declared on one wall clock resolved by the order the
producer wrote them in. Five were losses of provenance or of a reading: an empty definition was
indistinguishable from an undefined zone and was reported as one nobody supplied, a table's
early end was unlabeled, a second definition of one identifier was dropped, a zone the caller's
own bound refused was reported as one the file never defined, and every diagnostic about a zone
was anonymous. Eleven amendments to ADR 0003, two to ADR 0002, three to ADR 0011 and one to
ADR 0009 record the claims that did not survive; five codes are new — `time-zone-without-transitions`,
`time-zone-before-known-transitions`, `vtimezone-components-truncated`,
`vtimezone-observance-unreadable` and `exdate-zone-unknown`.

Four things are known and named. `AnswerBasis` says an answer continued an observance and from
which date at either end, but what a caller should do about a continuation six years wide as
against one a day wide is still nobody's decision, exactly as ADR 0003 left it. The projection
onto a zoned series' own wall clock is not injective across the hour a zone repeats, so two
`RECURRENCE-ID`s inside a fold collide on one cadence key: the earlier applies, the collision is
counted, and closing it would need a cadence key that carries a fold side, which no RFC defines.
`COUNT` composes with the zone only when a caller states the gate, because the other composition
is what RFC 5545 section 3.8.5.3 forces for a `DTSTART` in a gap. And a `Z`-terminated `EXDATE`
on a series whose `TZID` nothing defines cannot be placed at all — it is kept as the real instant
it names and reported, and the occurrence stays.

## M3 — Scheduling

`ical-itip`: RFC 5546 message semantics as described transitions, with authorization —
an attendee cannot move a meeting by replying. iMIP (RFC 6047) as a thin layer over the
same state machine.

What now binds it. The change vocabulary is `ical-core`'s, so the dependency runs one way and
cannot invert ([ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md)). Public error
enums crossing this crate's boundary are `#[non_exhaustive]`. A transition addresses a
property *occurrence*, not a property name, because a message changes one attendee among
many.

**Met.** `ical-itip` reads an RFC 5546 message, judges it against the state a caller already
holds and the identity of the party applying it, and answers a described transition or the first
reason it was refused. The transition is `ical-core`'s own change vocabulary keyed on a property
*occurrence*, which `Component::apply_to_occurrence` writes back — a second door beside the
identity-addressed `Component::apply`, because a `REPLY` answers for one `ATTENDEE` among many
and M0 chose identity-addressing for a reason this does not disturb. Section 3's twenty-two
constraint tables are data rather than code, and every conformance case names the subsection its
expectation was read from and asserts that name against the section the implementation's own
`MethodRule` carries, so a case and the table cannot drift apart silently.

Authorization is one fixed order with no partial success, and what it proves is now written down
rather than assumed. `Authorization` borrows both of its inputs, so it has no owned form to
encode and a caller that tries to carry one across a request boundary gets a compile error
instead of a forgeable token; `apply_transition` consumes it, so it is single-use. `Commitment`
is the one value designed to cross bytes and carries no authority at all — compared only to
refuse, digest a checksum and not a MAC — so forging one buys an attacker the ability to decline
to be told that the target moved, and nothing else. `SECURITY.md` gained the paragraphs that say
so, including the one nobody wants to write: for a first `PUBLISH` or `REQUEST` there is no local
state to compare a sender against, so the gate proves the actor is a party the *message* names
and the rest rests on the transport.

Two of the questions earlier milestones left are closed here and two are not the way they were
asked. A `Z`-terminated `RECURRENCE-ID` now picks its own half of a repeated hour, so M2's
cadence-key collision stops being a coin flip for scheduling — and a `TZID`-qualified one still
names both halves, which most real clients write, so those messages are permanently denied
rather than guessed at. An `AnswerBasis` continuation is reported where it decided *identity*
rather than only a rendering, which is the caller with a stake M2 said would come. An exclusion
no zone could place is a precondition the caller checks before the gate, not a denial inside it,
because handing the gate a zone would put zone resolution inside an authorization decision. And
RFC 6868 parameter copying has a case that asserts byte-identical output through the whole
read-describe-apply path, which is the hazard ADR 0001 amendment 3 named and no gate catches.

The corpus found three defects in the gate it was measuring, which is the whole reason it is
written from the RFC rather than from the code: section 3's `SUBCOMPONENTS` rows were unread, so
an attendee's `REPLY` could install a `VALARM`; `PUBLISH` and `REQUEST` could never create
anything, because the sender was looked up in state that by definition names nobody; and a
`REFRESH` described the removal of the organizer's own calendar. All three are fixed in the
implementation and recorded as ADR 0005 amendments 4 to 6.

Four adversarial lenses were then run against the shipped gate — an invited party attacking the
authorization model, version ordering and message identity, RFC 5546 section 3's tables read
against the RFC's own text, and the composition with `ical-recur`, `ical-tz` and ADR 0010's
bounds. They landed nineteen failing cases, eleven of them security findings, and every one is
answered in the implementation rather than documented as a limitation: an attendee could rewrite
the `ORGANIZER` line and the `SEQUENCE`, substitute a stranger for itself on its own `ATTENDEE`
line, and reach both through a party named only in somebody else's `DELEGATED-TO`; a held copy
whose `UID` was stated twice read as a component the caller did not hold, so a stranger was
judged against the stranger's own message; an unreadable `DTSTAMP` won a tie it had declined to
offer, and one that had been applied disarmed the ordering for good; an attendee's own earlier
`REPLY`, replayed, reverted the current one; a `RECURRENCE-ID` written as a bare wall clock
answered both halves of a repeated hour; a `CANCEL` naming one instance twice cancelled the
series; a calendar stating two `METHOD`s was filed as an ordinary `.ics`; a `REPLY` whose
`ATTENDEE` identified nobody was authorized to change nothing; a gap read one way and a gap read
the other were the same silent answer; and a message of a hundred thousand properties was read
for four units and described in full. ADR 0005 amendments 7 to 11 record what changed, and two
diagnostic codes are new: `scheduling-method-ambiguous` and `scheduling-instance-nonexistent`.

Five things are known and named. An attendee's `REPLY` carrying a moved `DTSTART` is *ignored*
rather than refused — the transition holds one change on the sender's own `ATTENDEE` line, so
the security property holds, but a caller that applies `Authorization::message`'s payload instead
of the transition moves the meeting. A legitimate `COUNTER` is refused, because the field rule
has no per-method dimension — the interoperability cost the design document said it preferred,
now observed rather than predicted. A delegate's `REPLY` describes nothing until the delegator's
own reply has been applied, which is RFC 5546's order but is not what a caller wanting one turn
gets. Ordering two replies from one attendee needs the state to record when the first was
written, so a store that keeps no such column keeps the change-of-mind case and loses that
defense. And a component whose own `ORGANIZER` line names an attendee authorizes that attendee
to cancel it: RFC 5546 section 1.3 lets one calendar user be both, so the defense is that no
message may write that line, and the corpus asserts the state is unreachable rather than
asserting a reading of the file that cannot exist.

## M4 — CalDAV

`ical-dav`: RFC 4791 requests and responses, sans-I/O, usable from both sides. Calendar
collections, `REPORT` queries, `ETag` conditional writes, and sync tokens.

At this point writing a calendar client or a self-hosted server in Rust becomes a
reasonable thing to attempt, which it currently is not.

What now binds it. The XML tokenizer is this crate's own, namespace-resolving and bounded, and
no outside XML crate may be added ([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md)).
Reading a multi-status is an incremental decoder holding one `DavResponse` at a time and
writing one is an incremental encoder, with the owned `MultiStatus` as one optional consumer.
Per-property status is a `PropStat` list, a `time-range` has two independently optional
bounds, and a collection field is private behind a capped push.

Gates this milestone owes: compile-checked examples for those three shapes, and the
incremental codec pair compiling under `no_std` on `thumbv7em-none-eabi`, which is the part
this design has never proved.

## M5 — Interoperability evidence

`ical-conform` grown into a published differential corpus: what Google, Microsoft 365, and
Apple each emit and accept, where they disagree with the RFC and with each other, and what
this project chose. Runnable against any implementation, including ones that are not this
one.

What now binds it. A case is addressed to a specification section and evaluated through the
subject trait, and it states the `Limits` policy it ran under, because an outcome that depends
on a budget is not reproducible without one
([ADR 0010](docs/adr/0010-shared-resource-limits.md)). A case asserting a diagnostic asserts a
`DiagnosticCode` and its channel, which is what the golden list of
[ADR 0009](docs/adr/0009-error-and-diagnostic-model.md) exists to keep stable.

Gates this milestone owes: the foreign-implementation bridge job, which needs an external
runtime in CI and a kill wrapper around the child process — neither exists today, and until
both do the bridge is a best-effort check rather than a gate.

## Non-goals

Bundling a time zone database or an HTTP client. Reading the system clock. vCard and
CardDAV — the same shape, a different specification, and a decision to make later rather
than a scope to assume now.

An allocation-free tier is a named gap rather than a non-goal: it belongs to a future crate
with its own lint profile, not to a feature flag on these
([ADR 0007](docs/adr/0007-allocation-policy.md)). `just no-std` proves these crates build for
`thumbv7em-none-eabi`, not that they build without a global allocator.
