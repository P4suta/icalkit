# ADR-0007: the core crates require `alloc`, and every allocated byte is charged

- Status: accepted
- Date: 2026-08-10

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
as a multiple of input size across the corpus.

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
measurement before it enters an ADR.

Clause (4b) converts an abort into a clean refusal; it does not let a 64KB-RAM device read a 400MB
calendar. The only shape that could is the token/pull layer growing a chunked or resumable value
protocol, and that layer as decided has no incomplete/need-more-input token, while an RFC 5545 value
has no interior structure to chunk on. Until that changes, "use the streaming layer instead" is a
promissory note, and the honest reading here is that large values are refusable, not processable,
anywhere in this workspace.

Two objections survive unanswered: no one has produced a compiling fixed-arena document, so its
rejection above is argument rather than counter-example, and `allocator_api` — a
`Document<A: Allocator = Global>` making the budget a property of the allocator instead of
hand-written accounting — stays unexamined, being unavailable on stable Rust today. The dependency
also deepens: the budget lives in the shared limits type of
[ADR 0010](0010-shared-resource-limits.md), whose review broke it in three places, including a
finding that the workspace is already on course for two incompatible limit types. If that decision
moves, clause (4)'s first sentence moves with it.
