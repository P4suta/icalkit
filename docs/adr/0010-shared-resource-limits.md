# ADR-0010: one limits policy, one running meter, threaded through every hostile input

- Status: accepted
- Date: 2026-08-10
- Amended: 2026-08-11 (two amendments)

## Context

[ADR 0002](0002-bounded-lazy-recurrence.md) says a recurrence search draws on "the injected
limits". The definite article points nowhere: no type is named, no crate owns one, and the
promise is kept in prose in two crates of five. `ical-core` speaks of "the caller's limits",
`ical-recur` ships a candidate budget of its own invention, and `ical-tz` and `ical-dav` never
mention a limit at all — although a CalDAV multistatus is the second most obvious hostile input
here, after an `.ics` file.

An ambient global or thread-local limit loses at once: invisible in a signature, unavailable to a
crate that owns no thread ([ADR 0004](0004-sans-io-protocol-layer.md)), and dishonest in a
library the caller drives. A limit type per crate loses too, being four spellings of one concept.

The third loser matters most, because it was the obvious answer. A threshold checked per call is
bounded per call and unbounded in aggregate. A CalDAV multiget carrying 5,000 VEVENTs, each with
a rare-match rule such as `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` over a multi-decade window, is
processed by a fan-out loop living in the calendar application — above `ical-dav`, above
`ical-recur`, in code this workspace does not ship. Every search respects its own threshold and
reports a clean result; the total is whatever the attacker chose N to be.

## Decision

A limit is two things, and this ADR names both. `Limits` is the caller's immutable policy: the
thresholds, cheap to copy, identical for every call, declared in `ical-grammar` and re-exported
by `ical-core`. The grammar charges octets before a tree exists, so the policy had to sit under
the seam ADR 0004 cut; a caller still names one crate for it. `Meter` is the
caller's mutable ledger: a running count of work already done under that policy. Every
hostile-input entry point takes both, `&Limits` and `&mut Meter` — parsing in `ical-core`,
expansion in `ical-recur`, zone-transition resolution in `ical-tz`, and the `REPORT` and
multistatus readers in `ical-dav`, inbound direction included.

The meter is passed by mutable reference precisely so that its lifetime is the caller's choice
and not the call's: one meter handed to all 5,000 searches makes 5,000 individually bounded calls
bounded in aggregate. `Meter` is neither `Copy` nor `Default`, so minting a fresh one inside a
fan-out loop is a visible act rather than an omission. Splitting a budget across workers is
likewise the caller's job; no shared meter ships here, because an atomic counter is not something
a `no_std` crate may assume.

Fields count work where the work happens. Recurrence spends candidates *generated* per period,
not instances emitted, since a fine `BYxxx` combination under a negative `BYSETPOS` does the work
either way. `max_line_len` is a byte cutoff that rejects rather than truncates and never splits a
folded line mid-codepoint. Depth bounds component and XML nesting; element and href counts bound
a wide flat document no depth counter sees; a byte count applies to the request a server reads,
not only the response it writes. Exhaustion is reported as itself: "cut short at the limit" and
"the rule ended at `UNTIL`" must be different answers, or truncation arrives dressed as an empty
result.

## Consequences

Four crates take a breaking signature change, not the three first counted: `ical-recur`'s
existing budget folds into this type rather than sitting beside it, or "one value everywhere" is
false on the day it is written. And the signature gap is the only one that closes — accepting
`&mut Meter` and never debiting it still compiles. A gate can test the debit sites someone
thought to write a test for, which is not a proof of coverage, and nothing in the type system
charges the meter at the recursion site.

The library also cannot force meter reuse. `!Copy` plus `!Default` plus documentation makes the
amplification bug visible, not impossible; a caller who writes `Meter::with_budget(D)` inside its
own fan-out loop reproduces the attack above exactly, and no gate here sees that caller's code.
The corpus ([ADR 0006](0006-conformance-corpus-as-artifact.md)) can demonstrate the right shape
and record the wrong one, not enforce either downstream — the price of the sans-I/O boundary and
of `ical-dav` not depending on `ical-recur`.

The numbers are not chosen: a budget right for a phone rendering one month is wrong for a server
indexing a decade, and calibration belongs to whoever ships the first recurrence milestone.
**Amendment 1 keeps the principle and replaces the mechanism: the deployment and the envelope
are stated per policy, and every field says how its number was arrived at.**
Relatedly, a 64-worker server has 64 times the aggregate ceiling, and nothing here answers a
server that wants one global bound.

Neither is the calendar field list, and the first gap was found by attacking the implementation
rather than by reading this document. A content line folded across fifty thousand continuations
crosses no field named above: it is one item, so `max_items` does not bind; its value is one octet,
so `max_value_bytes` does not; its header is empty, so `max_header_bytes` does not; and the fold's
terminator and continuation whitespace are neither a name, a parameter nor a value, which was all
`charge_bytes` was ever handed — so `max_input_bytes` did not bind either, and a caller stating a
sixty-four octet budget had sixteen megabytes read and several hundred retained on its behalf. The
dimension that was missing is the one thing a fold costs: it is *kept*, one `FoldPoint` per fold,
because the writer has to put it back. `GrammarLimits::max_folds_per_line` bounds what one line
may retain, refused at the fold that crosses it, and each fold's octets are charged against the
shared ledger so the same bound holds across a document of many lines. Both halves, because a
per-line ceiling is bounded per line and unbounded in aggregate, which is this ADR's own argument
turned on a field it did not have. What that says about the rest of the list is that a dimension
is missing until something counts it, and reading the list is not how the next one will be found.

"Exhaustion is reported as itself" turned out to need one more sentence than it had, and M1's
conformance sweep supplied it. A ledger reports exhaustion through `Meter::is_exhausted`, and
that flag was set only by the octet budget — so the two dimensions a recurrence search actually
stops at, `candidates_per_period` and `occurrences_per_search`, refused a charge and left the
ledger reading clean. A caller holding the meter after the search was told the truncated answer
was whole, which is the failure this paragraph names, reached through the one report that was
supposed to survive being collected through an adapter. Every bound the ledger keeps latches it
now. The aggregate consequence is the one this ADR argues for rather than an accident: a runaway
period in one series of a fan-out ends the fan-out, and a caller that wants each series bounded
on its own gives each its own meter — which is the same visible act, in the other direction, that
`!Copy` plus `!Default` exists to make visible.

The DAV field list was not proven complete, and the next review landed here as predicted.
Namespace declarations are bounded now — `max_prefix_bindings`, because one element at depth one
can carry a thousand of them — and entity expansion is refused rather than counted, since a body
that declares a `DOCTYPE` is turned back before an expansion can begin. Attribute count per
element is still bounded by nothing, which is the dimension left. What is settled is the shape
rather than the list: there is no typed `XmlLimits` sibling, the DAV dimensions are fields of the
one `Limits`, and the next one will be too. **Amendment 2 is the next one, and it is not a DAV
dimension: the zone lookups a gated recurrence search makes are counted, in this type, under this
shape.**

## Amendments

**1. A number must say how it was arrived at, and the allocation budget is promoted from a field
to the envelope every other field is stated against.** The Consequences say the numbers are not
chosen and hand calibration to a later milestone, and the docket that inherited that sentence fixed
a rule in advance: a default is the smallest round number at or above twice the maximum a
non-hostile committed fixture requires. That rule was executed and it disqualifies itself. Its
population is a set of minimal reproductions — the largest non-hostile committed fixture is 1,815
octets — so it measures how fixtures are authored rather than what clients emit, and taken
literally it yields an input budget near 4 KiB and an item ceiling near 128, which refuse
essentially every real calendar. That output is the interoperability failure
[ADR 0007](0007-allocation-policy.md) named in advance as disqualifying, so the rule's answer was
already declared unacceptable by the ADR the rule serves. Withdrawing a threshold fixed in advance
is a serious act and this is the report that was owed rather than a quiet substitution.

The replacement rests on a fact already in the type. `max_input_bytes` **is** the allocation
envelope: octets are charged as they are appended and that budget is the ceiling, so the shipped
default already declares 16 MiB of peak charged bytes for one parse and the response cap declares
64 MiB for one DAV body. The calibration question is therefore not "how big are strangers'
calendars" but "given a stated envelope, what is the largest value of this dimension that cannot on
its own breach it" — a question about this reader, answerable in-tree, and the method
`max_folds_per_line` already used successfully.

Four things follow. Each named policy states its deployment and its envelope in one sentence, the
way the grammar's own limits already do and the outer type does not: the default is the phone, at
16 MiB per parse and 64 MiB per DAV body, and the generous policy is the server. Every field of
both limit types carries exactly one calibration marker — *envelope* (the budget itself, asserted
with a named deployment), *derived* with the function written and held by a const assertion or a
test, *measured* (an in-tree test builds a maximal admissible input for the dimension and reads it
back through this reader, with the bound standing above the observed figure), *shape* (the unit
costs a frame or a slot rather than octets, bounded for recursion and aliasing safety, argued
rather than measured), or *asserted*, which is permitted and must say so. A derived byte ceiling
must also state what fraction of its envelope it claims, because the default's per-value ceiling is
one sixteenth of the parse envelope while the XML text ceiling is one sixty-fourth of the response
envelope and nobody wrote down why they differ. And two dimensions — the item count and the XML
element count — genuinely cannot be derived from the envelope today, for a precise reason: their
cost is retention per item rather than input bytes per item, since a hundred thousand one-byte
properties cost about a megabyte of input and retain a hundred thousand model entries. They become
*measured* the moment the peak-allocation gate the roadmap already owes exists, as envelope divided
by bytes retained per unit — which needs no external corpus and no credentials, only the instrument
already committed to.

The corpus's role inverts. It sets no value; it falsifies one. A committed real-client export that
the default policy refuses is a defect filed against that policy's envelope rather than a data
point in a maximum, and that is the only place recorded provenance is load-bearing for this
question.

Two things are decided now with no measurement. The XML text ceiling stays where it is and its
justification is rewritten, because it currently names a calendar file and then takes the
property-value number — the number is right and the reason is wrong, since a text node is
materialized and copied out a second time while a parse input is delivered in chunks the reader
never buffers, so the denominators differ and the tighter ceiling is deliberate. The consequence
must be stated rather than discovered: under the default policy the same two-mebibyte calendar
parses from disk and is refused inside a multiget, and that divergence is owed a conformance case.
And no shipped constant changes value here. They are ratified as *labeled*, not as correct.

Five things this makes worse. It ratifies every shipped number for now, so the live
interoperability risk stays live: the default per-value ceiling refuses an inline attachment that
sabre, Radicale and libical all accept at 10 MB, and ADR 0006 says this workspace is governed by
what Google, Microsoft 365 and Apple emit and accept — labeling that field *asserted* makes the gap
visible and does not close it, and somebody will hit it before the falsification corpus exists. It
replaces one fixed rule with a per-field obligation and a ratchet, which is more machinery and more
surface for prose that says nothing: a field carrying a bad argument passes exactly as one carrying
a good one does, because the gate counts honesty markers and cannot grade them. It does not produce
the values the blocked consumers asked for — they get a derivation rule, a labeled status quo and
one instrument they must build. Promoting the input budget to the envelope makes it load-bearing
for every other field, so changing it later is a cascading change rather than a local one, and the
first person who wants a smaller phone budget will find it expensive. And withdrawing a threshold
fixed in advance sets the precedent this docket exists to prevent; the only thing separating it
from arguing after the fact is that the measurement that killed the rule is recorded with it, and
any future withdrawal must carry the same or this amendment is the crack the next one widens.

The strongest rejected alternative is executing the withdrawn rule literally and shipping the
resulting defaults, on the ground that it is the only rule in play that was fixed before the data
existed and that a threshold abandoned on seeing its answer is not a threshold. There is even a
defensible reading of the outcome — that a default should be a floor a caller must consciously
raise, the way `!Copy` plus `!Default` forces a visible act, and that hundreds of call sites
silently inheriting a 16 MiB budget is this ADR's own amplification argument wearing a different
hat. It loses because its output is the failure ADR 0007 named in advance and because the
population defect is demonstrable rather than asserted. Second, and preserved because it will be
proposed again: adopting the ecosystem's 10 MB and libical's per-value parameter count. It loses on
denominator — those are storage-acceptance caps on what a server will keep, or per-value limits
with no document budget above them, while this type's are retention caps on what a reader holds —
and on cascade, since raising the per-value ceiling tenfold invalidates the three fold constants
that were actually measured.

**2. The next dimension is a field of the one `Limits`, and it counts zone lookups.** The paragraph
above promises that shape for whatever comes next, and
[ADR 0011](0011-civil-time-arithmetic-and-resolution-types.md)'s Amendment 7 spends it:
`max_zone_lookups` is a field here, charged through the meter, refused as its own
`LimitExceeded` variant, and it is not an XML dimension. Its default is *derived* under Amendment
1's vocabulary rather than asserted — four times the per-search occurrence ceiling, with the
relation held by a const assertion for both shipped policies — which is what keeps it outside the
corpus-measurement question entirely, since a number that is a function of another number is not
calibrated by measuring fixtures.

It is recorded here as well as there because this is the document that says a bound nobody charges
is decoration, and because it is the first field whose default is a function of another field. That
costs this type something the Amendment 1 vocabulary makes visible rather than hides: the implicit
promise that each field is an independently meaningful ceiling is now false in one place, so a
caller who raises the occurrence ceiling through the builder and leaves the lookup ceiling alone is
refused by a dimension they never set, and the const assertion defends only the two shipped
policies and never a policy a builder chain produced. Naming the dimension in the refusal is the
only thing that keeps that diagnosable rather than mysterious.
