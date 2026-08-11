# Architecture

This document states the invariants. The reasoning behind each one lives in
[`docs/adr/`](docs/adr/); this file is the summary a reader needs before touching code.

## Shape

```text
        your application (has an HTTP client and a tzdb already)
                              ▲
   ical-dav ── ical-itip ── ical-recur ── ical-tz          ← semantics
                              ▲
                          ical-core                        ← model, typed views, serialize
                              │
                        (src/grammar/)                     ← content lines, diagnostics
```

Nothing here opens a connection, reads a clock, or bundles time zone data. Everything
above `ical-core` is a separate crate so that a caller who only reads `.ics` files never
compiles scheduling or CalDAV. The content line grammar is a layer and not a crate: it is a
private module tree inside `ical-core`, every item of it re-exported at that crate's root, so
`ical_core::Token` is the one spelling there is
([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md), amendments 12 and 17).

Nothing in the tree names that layer's path, and two gates keep it a layer.
`gates/grammar-layering` is an unpublished workspace member that compiles the same sources a
second time in a crate root holding no model, so a reference to `CivilDate` from inside the
grammar is `error[E0432]` with a file and a line rather than a review comment. What that
member cannot catch is `crate::X` for an `X` the crate root re-exports from the grammar
itself — it resolves there too — so `just purity` carries a second, textual rule: no path
under `crates/ical-core/src/grammar/` may resolve above that directory, no `extern crate` and
no `#[path]` inside it, every `.rs` file there declared by its `mod.rs`, and the tree stays
flat. That rule is hygiene about not routing a lateral import through the parent crate's
public surface. It is not what the layering member proves, and no compiler enforces it. The
member itself is held to the workspace by a third rule of the same task, because a pull request
deleting it used to pass every gate here (ADR 0004, amendment 18).

**One change to this shape is decided and has not landed.** `ical-query` joins the graph
*above* `ical-core`, `ical-recur`, `ical-tz` and `ical-dav` as the filter evaluator, with
`ical-dav` taking no dependency in return
([ADR 0012](docs/adr/0012-query-evaluation-crate-and-the-deferred-webdav-extraction.md)). The
diagram and the table below describe the tree as it stands today, before it lands.

`ical-recur` and `ical-tz` are siblings: neither depends on the other. Recurrence needs a
zone answer, and the caller obtains it from `ical-tz` and passes in the instant, which is why
recurrence expansion and zone resolution can be compiled apart. M2 settled the one thing that
arrangement leaves unsaid: **the timeline `ical-recur` walks for a zoned series is that series'
own wall clock projected onto UTC, not the UTC timeline.** Every instant crossing the seam in
either direction is on it, and each cadence key is resolved against the zone one at a time,
which is the only place a daylight saving transition can be seen. `ical_tz::seam` states the
contract, `ical-recur`'s crate documentation states the caller's two obligations under it, and
`crates/ical-conform/tests/break_zones.rs` is where that seam is held to a real zone — it holds
a daily 09:00 series to its wall clock across both of Europe/Berlin's 2026 transitions and
asserts that the reading which never re-resolves is 3,600 seconds out. It was the only file in
the workspace naming both crates until M2's own adversarial pass landed `break_tz_seam.rs`
beside it, and at M3 `ical-itip` took a dependency on each, which made the seam something a
shipped crate crosses rather than only something a test watches.

## Invariants

Numbers 1 through 6 date from the bootstrap; 7 through 11 come from the design bake-off that
followed it. `just purity`, `just no-std`, `just wasm` and `just codes` enforce the structural
ones today — a change that violates one fails CI. The rest are testable rather than structural,
and their
gates arrive with the code they constrain; `ROADMAP.md` says which milestone owes which gate.

1. **Nothing is lost on a round trip**
   ([ADR 0001](docs/adr/0001-lossless-round-trip.md)). Unknown properties, parameters, and
   components are preserved in position. Typed access is a view over preserved content,
   never the storage. `parse → serialize` is byte-identical across the whole corpus.

2. **Recurrence is lazy and doubly bounded**
   ([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)). The caller supplies a window;
   the search itself has a candidate budget whose exhaustion is a reported outcome. There
   is no function that expands a rule into a `Vec`.

3. **Time zone data comes from the caller**
   ([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md)). No bundled tzdb, no system
   clock. Where the embedded `VTIMEZONE` and IANA disagree, that is reported, and every
   result names its source.

4. **The protocol layer is sans-I/O and `no_std`**
   ([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md)). `ical-dav` produces requests and
   interprets responses; the same code serves clients and servers.

5. **Scheduling is separate from the model**
   ([ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md)). `ical-itip` returns a
   described transition, not a mutation, and authorization is part of the semantics.

6. **The conformance corpus is a deliverable**
   ([ADR 0006](docs/adr/0006-conformance-corpus-as-artifact.md)). Real client exports,
   reduced and anonymized, runnable against any implementation.

7. **The core is `no_std` *and* `alloc`, and every allocated byte is charged**
   ([ADR 0007](docs/adr/0007-allocation-policy.md)). There is no allocation-free build of
   these crates and no feature flag pretending otherwise. A parsed document owns its memory
   and carries no lifetime parameter, unfolding runs into a fresh buffer, and nothing is
   sliced out of pre-unfold bytes.

8. **The token layer is the parser; the document tree is one consumer**
   ([ADR 0008](docs/adr/0008-parser-layering-and-pull-api.md)). `Document::parse` goes
   through the same public token path a streaming caller uses, so the two cannot fork into
   separately maintained grammars. Every token payload is `&[u8]`; UTF-8 is demanded only in
   the typed view.

9. **Two failure channels, and a diagnostic code frozen in meaning**
   ([ADR 0009](docs/adr/0009-error-and-diagnostic-model.md)). An error is "no item can be
   built"; everything else is a diagnostic pushed to a sink that may refuse, with the
   refusals counted outside it. `DiagnosticCode` is one workspace-wide vocabulary defined at
   the bottom of the stack.

10. **One limits policy, one running meter**
    ([ADR 0010](docs/adr/0010-shared-resource-limits.md)). `Limits` is the caller's immutable
    policy and `Meter` its mutable ledger, passed as `&mut` so that five thousand individually
    bounded calls are bounded in aggregate. `Meter` is neither `Copy` nor `Default`.

11. **Civil arithmetic is checked, and invalid instances are filtered**
    ([ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md)). Every operation
    is `checked_*`, `div_euclid` or `rem_euclid`; no `Duration` carries years or months; a
    recurrence instance whose date or whose local time does not exist is dropped per RFC 5545
    section 3.3.10, never coerced to a nearby one.

## Crate boundaries

| Crate | Depends on | std | alloc | Reads a clock | State |
| --- | --- | --- | --- | --- | --- |
| `ical-core` | — | no | yes | no | landed (M0) |
| `ical-recur` | `ical-core` | no | yes | no | landed (M1) |
| `ical-tz` | `ical-core` | no | yes | no | landed (M2) |
| `ical-itip` | `ical-core`, `ical-recur`, `ical-tz` | no | yes | no | landed (M3) |
| `ical-dav` | `ical-core` | no | yes | no | landed (M4) |
| `ical-conform` | all of them | yes | yes | no | grows with each milestone (M5) |

One row is owed and does not exist yet: `ical-query` (`ical-core`, `ical-recur`, `ical-tz`,
`ical-dav`; no std, alloc, no clock). Six crates are publishable, not seven — nothing has been
published yet, and that is the count the release configuration is held to.
`gates/grammar-layering` is a workspace member and is deliberately not a row here: it declares
no dependencies, publishes nothing, and compiles `ical-core`'s own sources, so a row claiming
otherwise would describe a crate that does not exist.

"State" is the milestone whose gates the crate met, not a stability claim: nothing is
published and no public API is frozen. What each landed crate does **not** do is in its own
`# Status` section and in `ROADMAP.md`, which are the two places that stay honest about it.
`ical-dav` depends on `ical-core` and on nothing else — `just purity` rejects every declared
dependency of a core crate including dev-dependencies, so the hand-rolled XML tokenizer ADR
0004 chose is a gate rather than an intention. `ical-conform`'s five are declared as
dev-dependencies, which is the distinction that makes "runnable against any implementation"
mean anything: a competing implementation depending on this crate compiles none of them.

"Reads a clock" is a column because a calendar library that quietly asks the OS for the
current time is untestable: the answer to "is this event in the past" must come from an
instant the caller passed in.

"alloc" is a column because `no_std` alone did not capture the wiring that actually broke.
A panel proposal's `Vec<Response>: Slots<Response>` failed to compile at the
`ical-core`/`ical-dav` seam under an allocation-free reading of these crates, and no
dependency diff can see that. No crate carries a compiled minimal-usage example at its declared
setting: `just no-std` and `just wasm` build the five core crates for a bare-metal and a browser
target, which proves the targets and says nothing about the seam. So the column is a claim the
workspace holds to by hand, and that class of break stays invisible until somebody wires two
crates together and tries to compile the result.

## What lives where

- **Syntax lives in `ical-core`'s grammar layer.** Unfolding, content-line lexing, escaping,
  parameter structure — and the diagnostic vocabulary, which did not stay above the layer
  because a violation of the grammar is detected by the grammar. `src/grammar/` names nothing
  above itself; nothing outside it names its path.
- **Preservation lives in `ical-core`.** Every layer above it operates on the preserved
  model and must not require reserializing through a lossy typed form.
- **The shared vocabulary lives at or below `ical-core`.** `Limits`, `Meter`, the civil-time
  primitives and `Instant` are named by crates that do not depend on each other, so they sit
  at the common root rather than in whichever crate uses them most. `ical-tz` owns
  resolution, not the types it resolves into.
- **Ambiguity is represented, not resolved.** Non-existent and repeated local times at DST
  transitions, disagreeing time zone sources, and specification violations are all values
  a caller can inspect, not errors that discard the input.
- **A limit's number says how it was arrived at.** `max_input_bytes` is the stated allocation
  envelope of a named policy, and every other field is derived from it by a written function,
  measured against this reader, argued from shape, or explicitly marked as merely asserted
  ([ADR 0010](docs/adr/0010-shared-resource-limits.md) amendment 1).
- **No crate here translates a vendor time zone identifier.** Not behind a feature, not in a
  separate crate. Lookup is by exact bytes, an unrecognized identifier answers nothing and is
  reported, and the mapping is a decorator the caller wires
  ([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md) amendment 16).
- **Diagnostics travel with the item they concern,** so a caller can accept a
  specification-violating calendar and still know what was wrong with it.

## Where the details are

Every crate has a design document in [`docs/design/`](docs/design/) carrying its committed
public surface, the reasoning behind each signature, and a closing section recording what the
first whole-workspace compile changed. Those sections are the only place the seams between
crates are described from both sides at once. The grammar layer has no document of its own,
because DP-17 carved it out of `ical-core` after the bake-off and D-0003 put it back, so the
layer is argued inside `docs/design/ical-core-api.md` — which is where it belongs now that the
two are one crate.
