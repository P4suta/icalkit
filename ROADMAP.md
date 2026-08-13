# Roadmap

Everything here is text in, text out. No network, no clock, no hardware: every milestone
is verifiable by `cargo test` on `ubuntu-latest`.

## M0 — Model and round trip

`ical-core`: the RFC 5545 content line grammar, the component and property model, and
serialization. The milestone is complete when a corpus of real client exports parses and
serializes back byte-identically
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

Gates this milestone owed, five met: the `DiagnosticCode` golden list and its diff check, a
round-trip property test over the corpus, a fold that splits a UTF-8 codepoint, a CP1252
`SUMMARY`, and a structural test that `Document` is built from the public token path. Two are
still owed: a hostile input of 200,000 one-byte properties, and a peak-allocation ceiling as a
multiple of input size. The fold bomb and the depth bomb reach neither — one bounds what a
single line retains and the other what a walk survives, and the question both of these ask is
what a whole document costs. The second is retargeted rather than merely built: it must report
bytes retained *per item* and *per XML element* as well as peak charged bytes, because per-unit
retention is what turns `max_items` and `max_xml_elements` from asserted numbers into measured
ones ([ADR 0010](docs/adr/0010-shared-resource-limits.md) amendment 1).

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
`icalkit-conformance`, with the expected column transcribed from the RFC. The item type ADR 0002
committed to did not survive contact with `Iterator`, and fourteen sentences of that ADR are
now amended rather than reinterpreted; the amendments are the record of what shipped.

Six of those amendments are what four adversarial lenses — the RFC's own answers, the budget,
`RDATE`/`EXDATE`/override composition, and the Gregorian calendar — found in the built engine,
and each has a case in `crates/icalkit-conformance/tests/break_recur_*.rs` that failed before the fix.
They were: a meter that reported exhaustion for its octet budget alone, so a search stopped by
either recurrence ceiling left the durable report reading clean; a terminal report that counted
what expansion returned rather than what it charged, so the two rules that spend a budget
without producing anything reported spending nothing; an engine that inferred from the merge's
silence which source a step had consumed, so one `EXDATE` on an `RDATE` could erase the rule
instance after it and an unbounded tail besides; a period walk that deleted the last period of
every cadence to satisfy an upper edge nothing read; a `BYWEEKNO` read as a filter over the
calendar year rather than an expansion of the week-numbering one; and a `BYDAY` ordinal answered
two ways under the frequencies that forbid one, the quieter of which emptied a whole series.

Two things are known and named: emission is ordered by cadence key rather than by effective
start, and the period walk's own vocabulary is on the public surface as an integration artifact
and is expected to narrow. A third was listed here and is struck: what a floating `UNTIL` against
a zoned `DTSTART` means was closed by M2, with a test and the golden-listed
`recurrence-until-not-utc` behind it.

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
it from the other side, and `crates/icalkit-conformance/tests/break_zones.rs` — the only file naming
both crates — holds a daily 09:00 Berlin series to 09:00 across both 2026 transitions and
asserts that the reading which never re-resolves is 3,600 seconds out from the transition
onwards. All six questions M1 left open are closed with a test each, and four of them left a
golden-listed code behind that no emitter had before: `recurrence-until-not-utc`,
`exdate-value-type-mismatch`, `override-matches-no-instance` and `time-zone-coverage-exhausted`.

Four adversarial lenses — the transitions, the sources, the seam, and the bounds — were then run
against the built crate, and what they found is in `crates/icalkit-conformance/tests/break_tz_*.rs`,
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

Five things were known and named, and three of them are now decided rather than merely named.
An attendee's `REPLY` carrying a moved `DTSTART` is *ignored* rather than refused — the transition
holds one change on the sender's own `ATTENDEE` line, so the security property holds, but a caller
that applies `Authorization::message`'s payload instead of the transition moves the meeting. A
legitimate `COUNTER` is refused, because the field rule has no per-method dimension — the
interoperability cost the design document said it preferred, now observed rather than predicted.
[ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md) amendment 12 answers both: the field
rule takes the method, so the `COUNTER` is admitted, and an overreaching `REPLY` is refused rather
than dropped. A delegate's `REPLY` describes nothing, and the sentence that stood here — that it
describes nothing *until the delegator's own reply has been applied* — is wrong: the corpus fixture
that is the post-delegator-reply state still describes nothing, because a delegator's reply writes
parameter edits and never adds the delegate's own `ATTENDEE` line. Amendment 13 names the hold and
records the organizer `REQUEST` as its only release. Ordering two replies from one attendee needs the state to record when the first was
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

Gates this milestone owed, all met: compile-checked examples for those three shapes, and
the incremental codec pair compiling under `no_std` on `thumbv7em-none-eabi`, which is the
part this design had never proved.

**Met.** `ical-dav` reads and writes every body RFC 4791 defines, from both ends, over a
tokenizer that is this crate's own and depends on nothing. A client builds a `REPORT` and
reads the multistatus; a server reads the same `REPORT` and builds the same multistatus; the
direction is visible only in which codec trait is called. `tests/interop.rs` drives the two
halves through each other rather than each against a stand-in, which is where a disagreement
between an encoder and a decoder about one element's shape shows up and nowhere else.

The XML refuses more than it accepts, and each refusal is a class rather than a budget. No
`DOCTYPE` in any casing, so the billion laughs, the internal and external parameter entity and
the file-pointing general entity are closed together rather than raced; no processing
instruction, no encoding but UTF-8, no unbound prefix, no mismatched tag, no duplicate
attribute. The reader is iterative with an explicit stack, so a hundred-thousand-deep body
meets `LimitExceeded::Depth` rather than a stack overflow, and namespace bindings are charged
and released as their elements open and close. Every lookup is on a resolved `(namespace,
local name)`: `SabreDAV`'s `d:`/`cal:`, Radicale's `ns0:`/`ns1:` and Calendar Server's default
`DAV:` declaration read to one value, and a familiar `D:` bound to a namespace of an
attacker's choosing reads as foreign, which a reader matching local names would have accepted.

The collision ADR 0001 and XML 1.0 section 2.11 were on either side of is settled and written
into both documents' own registers rather than reconciled in a commit message. Inside
`CALDAV:calendar-data` and nowhere else the reader hands back the octets as they arrived, so
what reaches `Document::parse` from a multistatus is what the server sent; it is therefore not
a conformant XML processor for that one element, and must never be used to canonicalize or
verify signed XML. The writer needs no departure at all — a `CR` goes out as `&#13;`, no
`CDATA` is ever emitted, so anything this crate writes is recoverable by any parser.
`TextPolicy::Normalized` restores conformance at runtime, and every payload it costs a `CR`
says so on the sink, because a choice being available is worth nothing if taking it is silent.

Of the six things M3 handed over, three are answered here and three are answered by saying
whose they are. Freshness is `Revision`: what a read learned about one resource, and the
`Precondition` that makes a second turn land on that revision or be refused by the server —
which is the honest closure, since the freshness a caller gets is the freshness the server
enforces. A weak `ETag` yields no precondition rather than an `If-Match` no server can satisfy
or an `If-Match: *` that means something else, and a 403 is not read as an absence, because
that is how a client creates a second copy of an event it was merely not allowed to see. The
authenticated principal is answered by the vocabulary — `DAV:current-user-principal` joined to
`CALDAV:calendar-user-address-set` — rather than by a check this crate has no standing to make.
An `ORGANIZER` change on write gets its refusal modeled and on the wire. The reply timestamp
stays a store's column. The superseding `icalkit` facade now closes organizer `REQUEST`
`RANGE=THISANDFUTURE` splitting, recurrence-set membership and later-anchor updates; remaining
method-specific range behavior is still work. A store should build the `ical_core::Component`
rather than a second source of truth for claims its own octets carry.

### Is a calendar client or a self-hosted server now a reasonable thing to attempt?

For a **client**, yes, and that is the claim this milestone was written to earn. Discovery,
`PROPFIND`, the three `REPORT`s, conditional writes and RFC 6578 synchronization are all
values a caller builds and reads; the caller brings the HTTP client it already has, which is
what ADR 0004 chose deliberately rather than what this workspace failed to supply. Nothing
below is missing from that path.

For a **server**, the sentence above is overstated, and the honest version is that this crate
supplies the protocol layer and not the server. Four things stand between a reader of this
document and a working one, none of them small:

- **Nothing here evaluates a filter yet, and the crate that will is named.** A `comp-filter`, a
  `time-range` and a `text-match` are represented, refused when they contradict themselves, and
  handed back; deciding which resources match is work a server does by composing them with
  `ical-recur` and `ical-core`. ADR 0004 always said so. It is the largest single piece of a
  server this workspace does not contain *today*:
  [ADR 0012](docs/adr/0012-query-evaluation-crate-and-the-deferred-webdav-extraction.md) decides
  that it is `ical-query`, a published crate above `ical-core`, `ical-recur`, `ical-tz` and
  `ical-dav`, and that `ical-dav` takes no dependency to make it possible.
- **The vocabulary is CalDAV's and stops there, on purpose.** `MKCALENDAR` (RFC 4791 section
  5.3.1) has a request body and no row, and that half is work this milestone owes.
  Everything in RFC 3744 is a scope decision and it is made:
  [ADR 0004](docs/adr/0004-sans-io-protocol-layer.md) amendment 14 declines ACL's semantics and
  the principal-discovery reports, which are now in the Non-goals below. Six of those roots gain
  recognition-only rows so a server can tell a standard report this crate will not honor from a
  root somebody invented — the deployed answer to which is an empty `207`, not a `403` — but
  nothing reads or writes them, and a multi-user server still supplies its own ACL.
- **Two gaps inside what is modeled — both now closed, by the attack rather than by this
  plan.** `CALDAV:timezone` has a row, a field on `CalendarQuery`, and the line-ending
  carve-out its value earns, so the zone a client stated survives a read and a re-encode
  instead of being dropped as foreign. `DAV:allprop` and `DAV:propname` inside a
  `calendar-query` are `QueryShape`, so RFC 4791 section 9.5's own production is a body this
  crate reads and writes rather than one it answers `DavError::Unexpected` to. What replaced
  them on this list was `calendar-multiget`, whose grammar admits the same three shapes and
  which still carries only a property list. That is now decided the same way: ADR 0004 amendment
  15 gives the multiget a `QueryShape`, because "nobody is known to send it" is the argument M4
  declined one element over, and it fixes the *production* as the rule that says which bodies
  carry the group at all.
- **Scheduling over HTTP is half here.** RFC 6638's preconditions, `schedule-tag` and the
  inbox and outbox properties are modeled, and `ical-itip` holds the semantics; the POST to a
  scheduling outbox and the `CALDAV:schedule-response` body it answers with are not.

So: a client, yes, today. A server, with the filter engine `ical-query` will ship and with an
ACL vocabulary a reader supplies, since this workspace has declined that one. Both of those were true before this
milestone as well; what changed is that the protocol layer under them exists and the reasons
are specific enough to be worked through rather than discovered.

### What four adversaries found after that was written

Everything above was true of a layer that had never been attacked from outside its own tests.
Four lenses then wrote 411 conformance cases against it — the XML surface, the calendar round
trip through the envelope, the one-shape-both-directions claim, and the protocol state a
conditional write and a sync token carry — and twenty of them failed. Three were security
findings and all three were real:

- **Text a peer escaped became markup on the way out.** `PropValue::Unmodeled` was documented by
  the encoder as markup and filled by the reader with decoded character data, so a proxying
  server pasted a peer's string into its own multistatus as `DAV:` elements the peer had chosen.
  The field is split — character data is escaped, markup is a re-serialized fragment — and RFC
  4918 section 9.1.3's own structured property survives a proxy for the first time.
- **A server could choose the caller's other headers.** `ETag::parse` accepted thirty-four
  octets outside RFC 9110 section 8.8.3's `etagc`, `CR` and `LF` among them, and an accepted tag
  is rendered straight into an `If-Match` value. A `getetag` spelling `&#13;&#10;If-Match: *`
  turned a caller's conditional write into an unconditional one.
- **A comment was free.** Nothing charged the octets `skip_comment` walked past, so thirty-two
  mebibytes of comment cost 2,496 octets of a sixteen-mebibyte ledger and the aggregate bound
  ADR 0010 exists for did not exist at that seam.

The other seventeen were wrong answers and silent losses of the same family: an attribute value
that was not the value XML 1.0 section 3.3.3 defines, a `Char` production enforced against one
spelling of a character and not the other, a sync token handed back for an answer that was
truncated, a status line read as `200` when the server wrote `2000`, a precondition moved from
the property group that named it to the whole response, and `CALDAV:calendar-timezone` losing
the line endings `CALDAV:calendar-data` keeps. All twenty pass. What that says about the four
paragraphs above is not that they were wrong but that "met" was a claim about a design and the
attack was the first evidence about the code.

Two things were made worse on purpose and are recorded rather than smoothed over. A
`calendar-data` payload that is not UTF-8 — including an `.ics` whose RFC 5545 fold splits a
codepoint, which ADR 0001 guarantees this workspace round-trips — is now **refused** by the
encoder instead of written into a document that declares UTF-8 and is not, because that document
is one the peer discards whole. And a property that mixes character data with elements keeps its
text and reports the markup dropped, because one `Box<[u8]>` cannot hold both without inventing
an order between them.

One piece of internal debt is named rather than left: `write_request.rs` and
`write_response.rs` each carry a private element encoder instead of going through
`XmlWriter`, because both were written before it existed. All three share the one escaping
path and `tests/interop.rs` holds all three to the same tokenizer, so they cannot drift
silently; what the two private encoders lack is `XmlWriter`'s open-element stack, which makes
an unbalanced write unrepresentable. The obstacle is a borrow — `XmlWriter` holds the meter
for its whole lifetime and a nested `WriteXml` tree hands every level its own — and it is not
resolved.

## M5 — Interoperability evidence

`icalkit-conformance` grown into a private differential corpus and versioned JSONL CLI: what
Google Calendar, Microsoft 365, and Apple Calendar each emit and accept, where they disagree
with the RFC and with each other, and what this project chose. Captures must be reduced and
anonymized with provenance; synthetic client-shaped fixtures cannot justify compatibility
repairs. The CLI is runnable without creating a second Rust library API.

What now binds it. A case is addressed to a specification section and evaluated through the
subject trait, and it states the `Limits` policy it ran under, because an outcome that depends
on a budget is not reproducible without one
([ADR 0010](docs/adr/0010-shared-resource-limits.md)). A case asserting a diagnostic asserts a
`DiagnosticCode` and its channel, which is what the golden list of
[ADR 0009](docs/adr/0009-error-and-diagnostic-model.md) exists to keep stable.

Gates this milestone owes, restated by
[ADR 0006](docs/adr/0006-conformance-corpus-as-artifact.md) amendment 1. The owed gate is the
corpus against the committed `Observed` matrix: no external runtime, no kill wrapper, runs on
every supported target including the two cross-targets, required, and red for exactly one reason
— our answers moved against the recorded ones. The foreign-implementation bridge is no longer an
owed gate. It survives as `xtask observed-refresh`, a scheduled non-required job whose only
output is a classified diff for human review, which never commits and can never fail a build;
promoting it to a required gate is a measurement with its threshold already fixed in that
amendment.

Two measurements land here and neither may be argued afterwards. Whether the gap-case default
flips to shift needs one export apiece from three producers, with two shifting and none skipping
([ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md) amendment 4); if they
cannot be obtained by the close of this milestone the clause is struck rather than rolled
forward. Whether a delegator's `REPLY` may carry two `ATTENDEE` lines needs two captures apiece
from four clients ([ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md) amendment 13);
if none can be captured the refusal stands by default and is recorded as untested.

## Non-goals

Bundling a time zone database or an HTTP client. Reading the system clock. vCard and
CardDAV — the same shape, a different specification, and a decision to make later rather
than a scope to assume now.

RFC 3744's access-control vocabulary and the principal-discovery reports
(`DAV:principal-match`, `DAV:principal-property-search`, `DAV:principal-search-property-set`,
`DAV:acl-principal-prop-set`) together with `DAV:expand-property`. This workspace does not model
them; a server that needs access control supplies it. `ical-dav` recognizes those roots by name
and honors none of them, and the boundary
[ADR 0012](docs/adr/0012-query-evaluation-crate-and-the-deferred-webdav-extraction.md) draws
inherits the limit rather than becoming the place they were always going to live
([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md) amendment 14).

A vendor-identifier-to-IANA alias table. No crate published here answers which IANA zone a
vendor `TZID` means, in any crate, behind any feature, or as a separate crate outside the purity
rule; the mapping is a caller-side decorator
([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md) amendment 16).

An allocation-free tier is a named gap rather than a non-goal: it belongs to a future crate
with its own lint profile, not to a feature flag on these
([ADR 0007](docs/adr/0007-allocation-policy.md)). `just no-std` proves these crates build for
`thumbv7em-none-eabi`, not that they build without a global allocator.
