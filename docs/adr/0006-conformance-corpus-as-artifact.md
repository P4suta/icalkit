# ADR-0006: the conformance and interoperability corpus is a deliverable

- Status: accepted for evidence provenance; public/library and live-bridge delivery shape
  superseded by ADR-0014's private JSONL CLI/corpus
- Date: 2026-08-05
- Amended: 2026-08-10, 2026-08-11 (two amendments)

## Context

RFC 5545 is a large specification, and calendaring interoperability is decided less by it than by
what Google Calendar, Microsoft 365, and Apple Calendar actually emit and accept. Those three
disagree with the RFC and with each other, in ways that are folklore: everyone who has implemented
this knows some of them, and nobody has written them down where a machine can check.

A test suite organized around our own types would encode our reading of the specification and be
useless to anyone else. It would also be unable to express the interesting statement, which is not
"we parse this correctly" but "these four implementations disagree about this input, and here is
what each does".

## Decision

`ical-conform` is a published crate rather than a `tests/` directory.

Cases are addressed to specification sections and evaluated against a trait, so another
implementation can run the identical suite. This workspace supplies one implementation of that
trait.

The trait is the in-process seam for this workspace's own Rust implementation only: a Rust trait
can only be implemented by a Rust type, and libical is C. Foreign subjects reach the suite through
a subprocess bridge in `ical-conform`, which already uses `std` and sits outside the purity gate —
a thin `ConformanceSubject` that spawns the tool once per case under a hard wall-clock timeout and
terminates the child. A foreign process is a hostile and potentially non-terminating input source,
so that timeout is a condition of the mechanism, not a hardening item to add later.

A case declares what is compared, and the declaration is a type rather than a convention. The set
is closed and has three members. `RoundTrip` compares document bytes in against document bytes out.
`Derived` compares a computed answer — the first N occurrences of an `RRULE`, the UTC instant
chosen for an ambiguous local time — in a canonical encoding this project specifies. `Diagnostic`
compares our own diagnostic codes and budget-exhaustion values, which no foreign implementation is
designed to expose comparably, and never runs against a foreign subject. Without `Derived` a
recurrence case has to be filed as a round trip, where reserialization reproduces the `RRULE` text
unchanged and the comparison passes without ever touching the math.

`Derived` relocates the remaining risk rather than removing it. Neither foreign subject speaks that
canonical encoding — `ICAL.RecurExpansion` yields `ICAL.Time` values in the component's own zone,
`icalrecur_iterator` yields `icaltimetype` still needing zone handling to reach UTC — so the
normalization we author per subject sits inside the comparison, where it can hide a real divergence
and manufacture a fake one equally well. No type detects an unfaithful adapter; each is verified by
hand against that library's own published vectors, and none of that code is written yet.

The corpus is real. Calendars exported from real clients are committed verbatim and round-tripped
byte-for-byte, which is what makes the fidelity claim in
[ADR 0001](0001-lossless-round-trip.md) verifiable rather than asserted. Files are reduced to the
smallest form that still shows the behavior and stripped of personal data before being committed; a
case records which client and version produced the original.

Where implementations diverge, the case records every observed behavior and says which one this
project chose and why. Where the RFC permits alternatives, all permitted outcomes are recorded
rather than one being canonized — a human judgment made per case, and a permitted set drawn too
wide silently turns a genuine divergence into a passing recorded difference.

## Consequences

Publishing disagreements is more useful to the ecosystem than a green suite that hides them. A case
saying "Microsoft 365 emits this, the RFC forbids it, we accept it on read and never emit it" is
documentation that does not currently exist anywhere.

Every rule needs a case before it is considered implemented. That slows the first milestone and
pays back from the second, because the suite becomes the specification the implementation is
written against rather than a description of what it happens to do.

Committing real exports means a privacy obligation. Reduction and anonymization are part of
accepting a case, not a cleanup pass, and a case that cannot be anonymized is not accepted.

The bridge costs this suite its independence from the environment. CI must carry a pinned libical
build and a pinned JS engine, that job is best-effort rather than a gate until the timeout-and-kill
wrapper actually exists, and a tzdata bump can move a golden answer with no code change anywhere.
`std::process::Command` exists on neither `wasm32-unknown-unknown` nor `thumbv7em-none-eabi`, so
the suite attests "this host build against libical and ical.js" and can never prove the wasm build
agrees with ical.js inside one browser engine. That exclusion is permanent, not pending.
**Amendment 1 lifts it, because the differential claim now rests on committed data rather than on
a process.**

A closed question vocabulary buys a loud failure at the price of an amendment: the next comparison
class — iTIP `SEQUENCE`/`COUNTER` arbitration, CalDAV `REPORT` result sets — fits none of the three
variants and reopens this ADR. Better than being misfiled in silence, but it is the same recurrence
a red team found one level above where it was first patched, and nothing argues a third level away.
**Amendment 2 is that amendment, spent for the first time: the prediction came true twice in the
same shape, and the two named classes turn out to be one class with two members.**

The dissent worth remembering is that the live bridge may be answering the wrong question. A
static, versioned matrix of libical and ical.js results per case, recorded offline and diffed in
review, needs no foreign process in the CI hot path, no watchdog, and is not foreclosed on wasm. It
was never sketched or scored, so it was not adopted; it is owed a prototype and a bake-off, and
until then the bridge is the decision but not a settled one. **Amendment 1 settles it without the
bake-off, on grounds the bake-off could not have decided.**

## Amendments

**1. The matrix is the record and the bridge is demoted to its refresher.** The dissent above owes
itself a prototype and a bake-off, and neither is available — nothing is built either way. The
decision does not turn on which is cheaper, and the sentence above was wrong to imply it might:
three things settle it that are knowable without building either.

*What a red job means.* This document already concedes that the live bridge cannot distinguish an
implementation changing from a tzdata bump moving a golden answer from a child process hanging,
which is why the job had to be called best-effort. A gate whose red has three causes is not a gate.
Separating the record from the refresh separates the causes structurally.

*Target coverage.* `std::process::Command` exists on neither cross-target this workspace builds
for, so a live-only subject makes the exclusion above permanent by construction. Static data is
bytes, so the same required job runs everywhere the crates do, and the differential claim covers
the builds ADR 0004's cross-target gates exist to protect.

*Ordering.* Under live-only, nothing in the corpus says anything about a foreign implementation
until the entire bridge exists and is trusted. Under this amendment the first recorded row is
useful on the day it is committed, and the bridge is what keeps rows honest afterwards.

So, three parts. The **record**: `Observed` is checked-in static data and is the only foreign
evidence any required check reads, and every portable row carries provenance as data rather than
prose — subject name, subject version, the pinned runtime image digest, tzdata version, and the
date observed. A case whose foreign column is absent is recorded as *not observed*, never as
agreement. The **gate**: the required job runs the corpus against this workspace's implementation
and against the static matrix, spawns nothing, needs no kill wrapper to become a gate, and has
exactly one meaning when it is red — our answers moved against the recorded ones. The
**refresher**: the live bridge survives, unpublished and off the required path, as
`xtask observed-refresh` — spawn, watchdog, kill, reap, per-subject normalizers, a pinned image
manifest. It is run by a maintainer or a scheduled non-required job, its only output is a proposed
diff to the matrix reviewed by a human, it never commits and never fails a build, and no test
constructs a foreign runner. The published crate keeps that seam for out-of-tree subjects, with
nothing in-tree required to use it. Each differing row is classified as *subject changed*, *data
moved*, *harness failed* or *divergence*, fixed now so a red refresh is readable rather than
argued about; only a divergence may become a corpus case, and a harness failure may never edit a
row.

This buys no money, and that is the largest thing to say against it. Every item charged against
the live bridge is still owed in full — the kill wrapper, the two pinned runtimes, the container
and its manifest, the two normalization adapters no type can verify — and the total is *higher*,
because the matrix format, its provenance columns and the human diff review are work live-only did
not need. What is purchased is placement and ordering, not expense. It is also owed in the worst
possible crate: `xtask` declares an intentionally empty dependency table as a principle, so a
subprocess supervisor, a watchdog thread and a kill-and-reap path are hand-rolled against bare
`std` in the one crate that cannot take a dependency to do it properly, and that becomes the
least-reviewed concurrency in a workspace whose crates are otherwise single-threaded. The adapters
move further from review rather than closer: under live-only an unfaithful normalizer eventually
showed up as a red best-effort job, and here it shows up as a plausible diff a human approves in a
tool no required check runs — this document says no type detects an unfaithful adapter, and this
makes that admission worse by removing the only pressure that would have surfaced one. Staleness
stops being loud, since nothing fails when the matrix is a year old. The cross-implementation claim
weakens in wording, from "these implementations do this" to "these implementations did this, at
these versions, on this date", so a reader who skips the provenance columns over-reads the row. And
the first artifact is nearly empty, so on day one the required gate checks that this workspace
agrees with itself.

The dissent is live-only, which is what this ADR accepted, and its strongest form is the argument
this amendment must concede: the bridge has to be built either way, so building it once as the
subject is simpler than building it as a tool plus a data format plus a review ritual. It is
rejected on meaning and coverage — a job whose red has three causes cannot be the gate M5 owes, and
live-only permanently excludes the two cross-targets — and **not** on expense, because on expense
it wins. Nobody should relitigate it on expense. Also rejected: static-only with no refresher,
which is this amendment minus the ability to ever detect that the data went stale, and which turns
the corpus into folklore with version numbers.

The promotion of the refresher to a required gate is a measurement rather than an argument, and its
threshold is fixed here. **The measurement.** Over the first four scheduled refreshes after the
tool exists, count each run's classification per row, recorded alongside each matrix diff. **The
source.** The refresher's own output, run entirely from pinned public container images, with no
credentials, reachable by any contributor. **The threshold.** The refresher may be promoted only if
across those four runs no run reports a harness failure and every non-empty diff is classified as
*subject changed* or *divergence* with the classification confirmed by the reviewer. One harness
failure, or one misclassification, and it stays non-required permanently unless a new ADR reopens
it. **If the tool is not built by the end of M5**, the static matrix remains the record with no
refresher, the corpus's foreign rows are frozen at their recorded pins, and that frozen state is
stated in `ical-conform`'s own `# Status` block rather than left implied.

**2. The set of comparison classes is four, and the fourth is portable.** The prediction above has
come true twice in the same shape: M3 shipped the iTIP arbitration cases and M4 shipped the DAV
result-set cases, and both reduce to "several documents in a stated order, plus a party or a
request, yielding one canonical answer". That is one class with two members rather than two
classes, so `PortableQuestion` gains one variant — an *exchange* — carrying the applying party, the
ordered continuation and the kind, whose first two members are RFC 5546 sections 2.1.4 and 2.1.5
arbitration and RFC 4791 sections 7.8 and 7.9 result sets. The exchange kind is `#[non_exhaustive]`
because a new exchange is routine the way a new derivation is; a new *class* is not, so the
question vocabulary stays closed and stays not `#[non_exhaustive]`. A case's input keeps meaning
"the document the case is addressed to" — for an exchange, the prior state — so no existing case
table moves, and the subject trait gains exactly one object-safe method mirroring the derived one.

Three facts already in the tree decide it. The addressing scheme promises this: the case vocabulary
gives specification references a range covering RFC 5546 and RFC 4791, and the corpus already
records what Google, Microsoft 365 and Apple do with an `ATTENDEE` in a `PUBLISH` and with an
absent `SEQUENCE` in a `REPLY` — facts about other implementations, which the native tier is
defined to disclaim. The derived class cannot absorb them without paying the same arity twice,
because a scheduling case is a triple of prior state, incoming message and applying party, so
reaching it through a derivation means either changing that signature anyway — the same break minus
the loud failure — or authoring a container encoding, which is the silent misfiling this ADR
rejected and which collides with the rule that a case's bytes are the document verbatim. And the
prediction naming both classes has now been met by both.

Three costs. A published enum breaks by design, which is the property this ADR paid for and is
therefore a cost being *collected* — but it does not survive repetition: this document says of the
recurrence that nothing argues a third level away, and after this the closure is empirically a
convention with one break per two milestones behind it, so a fifth class would have to argue that
the set is closed on evidence rather than on assertion. Two more canonical encodings are named and
unspecified, enlarging what the design document already calls the largest piece of unwritten work
it creates; the result-set encoding is the harder of the two, since it must fix href ordering and
property selection or two implementations that genuinely agree will compare unequal and the suite
will manufacture a divergence. And the arity lands on the refresher's per-subject drivers, which go
from normalizing one answer to driving a foreign implementation through a multi-document exchange
offline — more adapter, on the one component that both a hidden real divergence and a manufactured
fake one pass through, with the same absence of any check on it. One rule goes with the exchange
rows: a row for which a subject produced no answer is recorded as unmeasured, never as agreement,
or a driver nobody could write reads in the matrix as three implementations concurring.

The alternative rejected is ruling both native and never portable, and its strongest form is not
about types: no foreign implementation is known to expose an arbitration answer comparably —
libical validates a message against the per-method restriction tables without arbitrating,
sabre-vobject and ical4j arbitrate inside a server behind state a harness would have to construct
through their own APIs. If offline drivers cannot be written for at least two foreign subjects,
this is a portable class with exactly one portable subject, which is the native tier with extra
ceremony. It loses because the corpus already carries observed cross-implementation behavior for
these sections and a tier that disclaims cross-implementation meaning cannot hold it — and it keeps
a stated revisit condition rather than a standing objection: if, after the refresher gains exchange
drivers, fewer than two foreign subjects can answer an arbitration at all, that is reported back as
evidence that the class is portable in name only.
