# ADR-0006: the conformance and interoperability corpus is a deliverable

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10

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

A closed question vocabulary buys a loud failure at the price of an amendment: the next comparison
class — iTIP `SEQUENCE`/`COUNTER` arbitration, CalDAV `REPORT` result sets — fits none of the three
variants and reopens this ADR. Better than being misfiled in silence, but it is the same recurrence
a red team found one level above where it was first patched, and nothing argues a third level away.

The dissent worth remembering is that the live bridge may be answering the wrong question. A
static, versioned matrix of libical and ical.js results per case, recorded offline and diffed in
review, needs no foreign process in the CI hot path, no watchdog, and is not foreclosed on wasm. It
was never sketched or scored, so it was not adopted; it is owed a prototype and a bake-off, and
until then the bridge is the decision but not a settled one.
