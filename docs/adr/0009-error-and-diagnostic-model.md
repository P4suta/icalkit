# ADR-0009: two failure channels, and a diagnostic code whose meaning is frozen by CI

- Status: accepted
- Date: 2026-08-10
- Amended: 2026-08-11 (two amendments)

## Context

[ADR 0001](0001-lossless-round-trip.md) promises that a violation is "a diagnostic attached to the
item rather than an error that discards the file", and then says nothing about what a diagnostic
is, what is left over that still counts as an error, or how either survives with no allocator.

One enum for both merges "this stream has no parseable item boundary" with "this `DESCRIPTION` is
40 KB", and all a caller can do with the merged value is return it — the file-discarding parser
ADR 0001 exists to prevent. A string or `u16` code gives
[ADR 0006](0006-conformance-corpus-as-artifact.md) nothing durable to anchor a case to. An
unconditional `Vec<Diagnostic>` assumes an allocator the `thumbv7em` gate of
[ADR 0004](0004-sans-io-protocol-layer.md) does not guarantee, and on a file with thousands of
repeated violations it turns "never discard the file" into an unbounded allocation.

## Decision

Failure has two channels. `ParseError` hand-implements `core::fmt::Display` and
`core::error::Error` — no derive crate, the core crates carry no dependencies — and covers only
conditions under which no item can be constructed at all: no locatable boundary to resume from, a
reassembly that is genuinely ambiguous. A limit breach is not one of them.

`Diagnostic { code: DiagnosticCode, .. }` carries the rest, every limit breach on an otherwise
parseable value included: an oversized `DESCRIPTION` is truncated, flagged, and kept while the
calendar around it parses. Limits stay injected ([ADR 0002](0002-bounded-lazy-recurrence.md)); what
this decision fixes is which channel a breach travels on, per limit kind rather than per type.

`DiagnosticCode` is public, `#[non_exhaustive]`, and stable in meaning as well as in name —
enforced, not documented. A committed golden list carries code, one-line meaning, and channel; a CI
diff gate fails when a row's meaning or channel changes without a rename or deprecation, and passes
trivially on additions. The channel column exists because a severity retune would otherwise flip
behavior for one input while every variant and doc comment sat still, breaking ADR 0006's "input X
produces code Y"; `ical-conform` carries a case per code and per channel.

Diagnostics reach the caller through `DiagnosticSink`, push-only, implemented for
`&mut Vec<Diagnostic>`, a fixed-capacity buffer, and a sink that keeps nothing. A sink may refuse:
`push` returns acceptance or refusal, every sink is permitted to refuse, and no parser may treat
refusal as a reason to stop parsing or to fail. Refusal is never silent — the result carries a
saturating `diagnostics_dropped` count living outside the sink, so "no violation was found" stays
distinguishable from "violations were found and could not be delivered". A nonzero count is not a
clean parse, and any API summarizing a parse as successful must surface it. That is how ADR 0001's
promise survives the no-alloc tier: a caller without an allocator may lose which violations
occurred, never that they did.

## Consequences

Round-trip behavior now needs two documents held together, and callers pass a sink instead of
receiving a collection — more type surface than a returned `Vec` for everyone who has an allocator.

A diagnostic-free parse still does not mean the text is correct. CP1252 bytes that decode as valid
UTF-8 — `0xC3` followed by a byte in `0x80`-`0xBF`, the classic accented-word mojibake — are
structurally undetectable, so neither channel fires. No heuristic is committed: one would misfire
on legitimately accented text, and ADR 0001 forbids repairing bytes, so even a future suspicion
code could only annotate. All this adds is a ban on documenting silence as correctness — a
documented limit in place of an undocumented lie.

The dropped count says information was lost, not which. On the 64 KB device that made a
fixed-capacity sink necessary, "17 diagnostics dropped" is not actionable, and nothing here says
whether a filtered second pass is affordable on a 400 MB stream. The no-alloc tier keeps the weaker
promise; that asymmetry is recorded, not closed.

The golden list is hand-maintained: CI proves the table did not change, nothing yet proves the
emission sites agree with it, and a new limit kind added without a row passes every gate. Deriving
one from the other is the real fix and is not here. `DiagnosticSink`, meanwhile, was nobody's
proposal, has since acquired the refusal protocol and the out-of-band counter, and is what
ADR 0001's promise rests on with no allocator linked — the least-reviewed part of this decision and
now the most load-bearing.

## Amendments

**1. A diagnostic may name the subject it is about, inline and bounded.** This ADR gives a
`Diagnostic` a code, a severity, a location and — since M1 — an instant, and says a location
that points nowhere is honest about a thing that exists at no offset in any file. M2 found what
is left over. Three `TZID` parameters that no `VTIMEZONE` defines produce three diagnostics that
are *equal as values*: a caller learns that something is missing and not what to go and find,
and a component that owns unfolded octets has no span back into the caller's buffer for a
location to use. A borrowed name cannot go in either, because a `Diagnostic` is `Copy` and
outlives the tree it was read from.

`Subject` is therefore a fixed inline buffer of `Subject::CAPACITY` octets with a truncation
flag, `Diagnostic::about` attaches one, and `Diagnostic::subject` reads it back. The cost is
paid by every diagnostic in the workspace whether it carries a name or not, which is what a
`Copy` diagnostic that allocates nothing costs; the capacity holds every IANA identifier and
`/mozilla.org/20050126_1/Europe/Berlin` besides. The gate in `xtask` is unaffected: a subject is
not a code, and the golden list still keys on the code alone.

**2. One meaning is restated without a rename, once, and the exemption is written down with its
expiry.** The freeze this ADR installs is that a row's meaning may not change without a rename or
a deprecation, and [ADR 0001](0001-lossless-round-trip.md)'s Amendment 9 needs exactly that for
`invalid-utf8-text`, whose committed meaning describes a decode a typed view attempted and which
must now describe a stream-level fact the parser establishes unasked. The exemption is granted
because the freeze's own stated rationale is empty here: the rule exists so that "input X produces
code Y" stays true for a corpus case and a downstream `match`, and no site in the workspace
constructs this code, no conformance case asserts it, and the one textual hit elsewhere names a
fixture file rather than the code. There is nothing for the rename to protect. The window closes
the moment an emitter ships, which is the same change — so this is a one-time exemption with a
stated expiry rather than a softening of the rule.

Two things are owed with it, and both are costs rather than mitigations. The exemption rests on an
in-tree scan, so an out-of-tree consumer already keyed to that code under its narrower documented
sense gets a silently widened meaning with no version signal, and that cannot be checked from here.
And the Consequences above already say the golden list is hand-maintained and that a code can carry
a row, a channel and a milestone while being emitted by nobody; this amendment is what makes that
concrete, because the row in question has asserted a milestone the code was never delivered in. The
gate should therefore grow one leg — every declared variant has at least one construction site
outside its own declaration, with an explicitly commented allowlist for codes whose emitter a later
milestone owes — and the leg's own weakness is the one `xtask` already names about itself: a
hand-rolled source scan that stops matching reports nothing and passes, and an allowlist is an
invitation to park codes in it. It is worth having anyway, because it makes CI say what the
documentation currently admits in prose.
