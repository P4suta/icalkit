# ADR-0007: the core crates require `alloc`, and every allocated byte is charged

- Status: accepted
- Date: 2026-08-10
- Amended: 2026-08-11 (two amendments)

## Context

Six accepted ADRs are silent about where parsed bytes live.
[ADR 0004](0004-sans-io-protocol-layer.md) says every crate is `no_std`, which readers hear as
"this runs on a microcontroller".
[ADR 0001](0001-lossless-round-trip.md) says the model keeps everything it was handed, which is a
promise about memory nobody costed. The code has been resolving that tension silently.

Nothing in RFC 5545 bounds how many properties a component carries, how many parameters a property
carries, or how many continuation lines one value is folded across; vendor extensions are open-ended
by construction, and the corpus of [ADR 0006](0006-conformance-corpus-as-artifact.md) is full of
them. A fixed-capacity model — `heapless` vectors, a caller arena, nodes addressed by index handle —
must therefore either truncate, destroying what ADR 0001 exists to protect, or reject files Google,
Microsoft and Apple emit today. That is the entire argument against it, and it is prose: nobody has
produced a compiling, non-trivial arena `Document` to hold it against.

The other loser was a borrowed `Document<'a>` slicing the caller's buffer. It copies nothing, and it
makes every later mutation a negotiation with the borrow checker over memory the caller owns — a
lifetime parameter in the central type of five crates, forever, to save one copy per value.

## Decision

(1) `ical-core`, `ical-recur`, `ical-tz`, `ical-itip` and `ical-dav` are `no_std` *and* `alloc`, and
each declares `extern crate alloc;`. Allocation is not a feature; there is no allocation-free build
of these crates. A CI check greps for the declaration, so the policy is enforced, not implied.

(2) A parsed `Document` owns its memory. Nodes are `Box<[u8]>`- and `Vec`-backed and the document
type carries no lifetime parameter, so mutation is not fighting borrowck over a caller-owned buffer.

(3) Unfolding (RFC 5545 section 3.1, removal of CRLF followed by one WSP) runs to completion into a
fresh owned buffer, and every validation and typed view is taken from that buffer. What is forbidden
is slicing: no `Property`, `Parameter`, `Component` or typed accessor may be constructed from a span
indexed into pre-unfold bytes, because a fold may fall inside a multi-byte UTF-8 codepoint and such
a span cannot be validated or sliced cleanly. The prohibition is on slicing pre-unfold bytes, not on
retaining them. The as-read bytes of each content line — its fold positions and the WSP octet (SPACE
or HTAB) at each fold — are preserved, because byte-identical reserialization of real exports is
impossible from unfolded text alone: producers fold at differing widths, with differing WSP octets,
and some do not fold at all.

(4) The allocation budget is a field of the shared limits value this workspace threads through
hostile-input entry points, and it is charged, not checked once at the end. Bytes are charged inside
the unfold accumulation loop before each chunk is appended, so one legal `SUMMARY` folded across
five million continuation lines is refused at the octet that crosses the budget, not after 400MB is
resident. Charging goes through a single metered helper over `Vec::try_reserve`; growing an owned
buffer around it is a review error. Crossing the budget is a refusal — a budget-exceeded outcome
naming the property and the limit, in the shape [ADR 0002](0002-bounded-lazy-recurrence.md) uses for
the candidate budget — never a truncation, since a truncated value and a preserved one are
indistinguishable at the serializer. (4b) A caller needing a value larger than the budget it can
afford uses the token/pull layer, not the document tree. CI asserts a ceiling on peak charged bytes
as a multiple of input size across the corpus. **The first sentence of (4b) is restated by
Amendment 1 below: the capability split it gestures at is real and is narrower than "use the
streaming layer instead" reads, because neither layer accepts input it does not already hold.**

(5) A genuinely allocation-free tier — fixed capacity or caller arena, for hard-real-time targets —
is a named, deferred, unscheduled gap belonging to a future crate with its own lint profile banning
`Vec` and `Box`, not to a feature flag on these five. `just no-std` proves these crates build for
`thumbv7em-none-eabi`, not that they build without a global allocator.

## Consequences

Every parsed value is copied out of the input rather than borrowed — a permanent memory and
copy-time cost, paid on every file, to keep the model mutable and free of lifetimes.

The meter is bookkeeping, not an allocator. Under `no_std` plus `alloc`, allocation failure aborts.
`Vec::try_reserve` closes part of this, but `Box::new`, collection internals, and any path that
grows without the metered helper still abort, and nothing in CI can enumerate those paths. A budget
set above available memory, or fragmentation under the 2x transient of a reallocation, still aborts
on a device the budget called safe. The guarantee is relative to that budget, never absolute.

The peak-allocation gate is a constant-factor assertion, not a proof: it gets tuned to whatever the
implementation does the day it is written, allocator overhead differs across the three-OS CI matrix,
and a regression raising the real peak by 40% may still pass. Nothing mechanically distinguishes
"charged capacity correctly" from "charged nearly correctly". Default budget selection is likewise
untouched, and there is no basis here for a number: too low breaks real exports carrying legitimate
large attachments — exactly the corpus M0 must pass — and turns this into a new interoperability
failure; too high and the mechanism does nothing on small devices. That number wants corpus
measurement before it enters an ADR. **The corpus measurement was taken and it disqualifies
itself: see [ADR 0010](0010-shared-resource-limits.md)'s Amendment 1, which withdraws the
twice-the-maximum rule on the evidence that a conformance corpus measures how fixtures are
authored, and replaces it with a stated envelope and a per-field calibration marker. The
sentence above stands as the record of what was owed; what changed is who owes it.**

Clause (4b) converts an abort into a clean refusal; it does not let a 64KB-RAM device read a 400MB
calendar. The only shape that could is the token/pull layer growing a chunked or resumable value
protocol, and that layer as decided has no incomplete/need-more-input token, while an RFC 5545 value
has no interior structure to chunk on. Until that changes, "use the streaming layer instead" is a
promissory note, and the honest reading here is that large values are refusable, not processable,
anywhere in this workspace. **Two clauses of that sentence are factually wrong and are corrected
by Amendment 1 rather than reinterpreted: the chunked protocol exists and ships, and the folds
are the interior structure. The conclusion survives with a different cause.**

Two objections survive unanswered: no one has produced a compiling fixed-arena document, so its
rejection above is argument rather than counter-example, and `allocator_api` — a
`Document<A: Allocator = Global>` making the budget a property of the allocator instead of
hand-written accounting — stays unexamined, being unavailable on stable Rust today. The dependency
also deepens: the budget lives in the shared limits type of
[ADR 0010](0010-shared-resource-limits.md), whose review broke it in three places, including a
finding that the workspace is already on course for two incompatible limit types. If that decision
moves, clause (4)'s first sentence moves with it.

## Amendments

**1. The chunked value protocol already exists, so the promissory note is about input residency
rather than about the token enum.** The Consequences paragraph above rests on two claims and both
are false as of today. `Token::Value` carries a flag saying whether another chunk follows, ADR 0008
decided that, the lexer delivers a value as borrowed runs between folds, and the design document
names it as the answer to this exact finding — so the pull layer does *not* lack a way to deliver a
value in pieces. And an RFC 5545 value does have interior structure to chunk on in this workspace's
own terms: the folds are that structure, and this workspace derived a fold-count default by
counting them for a base64 attachment at two widths. A paragraph in an accepted ADR asserted an
absence its own crate contradicts, and that is corrected here rather than reinterpreted.

What is genuinely still true is narrower and has a different cause. `ContentLineReader` is
constructed over a contiguous, fully resident slice; it has no feed and no resume. So a device that
cannot hold the input cannot read the calendar at any layer in this workspace, and the reason is the
reader's constructor rather than the token enum. That is a stated limit, not a deferred
implementation.

Clause (4b)'s first sentence is therefore restated as the capability split it was gesturing at,
made specific enough to hold code against. The token and pull layer delivers a property value as
borrowed chunks, never buffers one, and imposes no value ceiling at that layer — which is real in
the limits type and not only in prose, since the grammar's own limits carry no per-value bound. A
caller that can process a value incrementally therefore never has to hold it. A caller that must
hold the whole value, the document tree included, is bounded by the per-value ceiling and is
refused at the octet that crosses it. Neither path accepts input it does not already hold. The
words "uses the token/pull layer, not the document tree" as an unqualified escape are struck; what
they gestured at is kept.

Neither a need-more-input outcome nor a second chunk protocol is added. Reversing ADR 0008's
dropped outcome would cost the object safety that decision bought and would not by itself deliver
the capability it is priced for, because without a feed door the input is still resident. No
deployed implementation surveyed offers resumable input: libical's incremental unit is a complete
unfolded logical line buffered internally, ical4j materializes the value at its content handler,
sabre/vobject concatenates continuations before returning a line, and ical.js and python-icalendar
require the whole buffer. Two of the six answer with a ceiling instead, and RFC 4791 answers at the
protocol layer with a maximum-resource-size precondition. Under ADR 0006's rule that
interoperability is decided by what real clients emit and accept, refusing is the interoperable
posture and resumable input has no partner to be interoperable with. A pull-layer value ceiling is
also refused: the absence of one is now a stated capability that the restated clause depends on.

What this makes worse. It closes the 64 KB device against the 400 MB calendar as *impossible*
rather than solving it, and says so in the ADR — accurate, and weaker to read. The guarantee that
replaces the promise is producer-dependent, and the amendment has to say that or it is a second
promissory note: chunking is worth exactly what the producer folded, so an unfolded eight-mebibyte
value is one chunk and a pull caller holding one chunk at a time is holding eight mebibytes. The
corpus cannot even exercise the question, since the longest non-hostile physical line committed is
654 octets — so the restated claim is true, conditional, and unmeasured at scale. The chunk flag
gains exactly one consumer and it is a test, so the design is demonstrated once rather than
exercised. It records an accepted ADR as factually wrong rather than merely vague, which costs the
ADR set some of its standing as a record of what is true and invites re-reading the other
Consequences paragraphs for the same failure. And it declines rather than settles the residency
question: if a caller ever appears with an input larger than its memory, this reopens.

Two adjacent contradictions are named rather than absorbed. Clause (4)'s "never a truncation" and
the refusal the tree path implements are already contradicted by ADR 0009's statement that an
oversized description is truncated, flagged and kept; that conflict exists today, this amendment
neither creates nor repairs it, and it needs its own item. And ADR 0007's own promised
peak-charged-bytes ceiling gate does not exist in the Justfile, in `xtask` or in any workflow; the
one test this amendment lands does not supply it and does not pretend to.

The strongest rejected alternative is resume-by-cursor: keep the three outcomes and object safety,
and add a resume constructor taking an opaque reader state obtainable at a chunk boundary. It is
the only option that would make the 64 KB case reachable, it costs nothing ADR 0008 bought, and
this workspace has in-house precedent for exactly that shape in `ical-recur`'s pending outcome and
owned search cursor. It is rejected on four grounds, all of them reopening conditions rather than
permanent objections: the reader state would be a public, semver-load-bearing type carrying the
header scratch buffer, the parameter spans, the fold vector and the header state machine, freezing
the lexer's internals the way ADR 0002 says freezing a cursor encoding freezes an algorithm; fold
offsets are line-relative, so a line resumed across buffers needs them rebased or ADR 0001's round
trip breaks; the caller must guarantee the next buffer begins exactly where the last ended, an
invariant no type can check; and no deployed implementation offers it, so it would ship with no
interoperability partner and no in-workspace caller. Reopen when a real caller has a real input
larger than its memory — and at that point this branch, not the need-more-input outcome, is the one
to take.

**2. The default budget's calibration moves to [ADR 0010](0010-shared-resource-limits.md), and the
rule this document asked for was measured and disqualified.** The Consequences say the number wants
corpus measurement before it enters an ADR. The measurement was taken and it reports that the
population is wrong: a conformance corpus is built to isolate one grammar or protocol fact per
file, so its maximum is a statement about how fixtures are authored rather than about what clients
emit, and a rule keyed to twice that maximum yields defaults that refuse essentially every real
calendar — which is exactly the interoperability failure the paragraph above names in advance as
disqualifying. ADR 0010's Amendment 1 carries the replacement: the allocation budget is promoted
from a field to the *stated envelope* a named policy declares, and every other field must say
whether it is that envelope, derived from it by a written function, measured against this reader,
argued from shape, or merely asserted. The sentence above is left standing as the record of what
was owed; what this amendment adds is that it is owed under a different rule, in a different
document, and that the rule it asked for was tried first.
