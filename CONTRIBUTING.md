# Contributing

## Setup

```sh
mise install        # toolchain and gate tooling
mise run hooks      # install the git hooks
just                # list the available commands
```

## The loop

```sh
just check          # fast deterministic gates
just ci             # everything CI runs, locally
```

`just ci` is also the pre-push hook. If it passes locally it passes in CI; if it fails,
fix the cause rather than narrowing the gate.

## Test-driven changes

Start by adding the smallest test that expresses the missing behavior and confirm that it fails
for the intended reason. Implement the behavior, run the focused test until it passes, then run
`just check` and the relevant portability or conformance gates. Before pushing, run `just ci`.

Work on a topic branch and open a draft pull request while the change is still evolving. Keep
generated artifacts, public API snapshots, corpus evidence, and documentation in the same pull
request as the source change that requires them. The complete hosting and merge rules are in the
[repository policy](docs/repository-policy.md).

## Rules that are not negotiable

- **No `allow` and no `ignore`.** Every gate is strict on purpose. Make the code pass
  instead of suppressing the finding. If a lint is genuinely wrong for this codebase,
  change the shared configuration and say why in the commit message.
- **The private kernel stays `no_std` and sans-I/O.** `icalkit` is the only production crate.
  Its crate root remains `#![no_std]` and its private kernel must not gain a clock, transport,
  store, application policy, or bundled time zone database. The `std` and `system-tz` features
  activate only the documented adapter boundary. `just purity`, `just no-std`, and `just wasm`
  enforce it. A zone answer comes from a caller-supplied source and names that source
  ([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md)); "now" is an instant the caller
  passed in ([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md)). A dependency's key is a
  nickname; `just purity` reads the package it links, so renaming a package cannot hide a new
  boundary. Jiff and xmlparser are the two recorded third-party boundaries; another dependency
  requires an architecture decision.
- **The kernel uses `alloc`, and every allocated byte is charged.** The production crate declares
  `extern crate alloc;` and `just purity` checks for the line. There is no allocation-free
  build and no feature flag pretending otherwise; a genuinely alloc-free tier would be a new
  crate with its own lint profile ([ADR 0007](docs/adr/0007-allocation-policy.md)). Bytes are
  charged as they are appended, so a value that crosses the budget is refused at the octet
  that crosses it rather than after it is resident, and a refusal is never a truncation.
- **One limits policy, one meter.** Every entry point that reads attacker-controlled input
  takes the shared `Limits` and `&mut Meter`
  ([ADR 0010](docs/adr/0010-shared-resource-limits.md)). Minting a fresh `Meter` inside a
  fan-out loop is how five thousand individually bounded calls become unbounded in aggregate;
  the type is neither `Copy` nor `Default` so that doing it is at least visible. Accepting a
  meter and never charging it still compiles, which is why this is a rule and not only a
  signature.
- **A violation is an issue, and an error means no validated value could be built.** Those are
  separate channels, and which one a condition travels on is frozen per code
  ([ADR 0009](docs/adr/0009-error-and-diagnostic-model.md)). The internal diagnostic vocabulary
  and the public `IssueCode` string newtype must keep stable meanings. A sink refusing retention
  never changes how much input the parser validates.
- **There is one token path.** Strict validation, lossless import, and editing all go through the
  same private content-line tokenizer; a private fast path beside it would give one public name
  two grammars ([ADR 0008](docs/adr/0008-parser-layering-and-pull-api.md)). Token payloads stay
  octets internally, and UTF-8 is demanded only by a typed view that requires it.
- **The grammar is a layer inside `icalkit`'s private kernel, and it names nothing above itself.**
  `crates/icalkit/src/internal/core/grammar/` is a private module tree inside the unified crate,
  and
  the tree stays flat. `gates/grammar-layering` compiles the same sources in a crate that has
  no model, so naming one is a compile error there. That member cannot see a `crate::X` for an
  `X` the root re-exports from the grammar itself, so the rest is the second rule of
  `just purity`: in `mod.rs` neither `crate::` nor `super::`, in the files beside it neither
  `crate::` nor `super::super::`, no path through a retired crate alias, no `extern crate`, no
  `#[path]`, and every `.rs` file beside `mod.rs` declared by it. The check is textual and a
  macro or a generated path goes through it
  ([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md) amendments 17 and 18).
- **Time arithmetic is checked and never coerces.** `checked_*`, `div_euclid`, `rem_euclid`;
  no `Duration` carries years or months; a recurrence instance whose date or local time does
  not exist is filtered per RFC 5545 section 3.3.10, never moved to a nearby one
  ([ADR 0011](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md)).
- **Nothing is lost on a round trip.** A property is not supported until `parse → serialize`
  is byte-identical for it, including the parameters, casing, and ordering nobody
  interprets. Typed access is a view over preserved text, never the storage
  ([ADR 0001](docs/adr/0001-lossless-round-trip.md)). An accessor that replaces the original
  text is a data-loss bug even when the accessor is correct.
- **Recurrence respects the budget.** Expansion is a lazy iterator over a caller-supplied
  window, and exhausting the candidate budget is a reported outcome
  ([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)). No function collects a rule into a
  `Vec`, and no search path steps around the budget because a particular rule is awkward.
- **Every rule gets a conformance case.** A rule implemented without a case in
  `icalkit-conformance` addressed to the RFC section it comes from is incomplete
  ([ADR 0006](docs/adr/0006-conformance-corpus-as-artifact.md)). Where implementations
  disagree, the case records what each one does, not only what this project chose.

## The corpus is real, which is an obligation

Cases come from calendars that real clients exported, because a fidelity claim measured
against files we wrote ourselves proves nothing. Reduction and anonymization are part of
accepting a case, not a cleanup pass afterwards: cut the export down to the smallest form
that still shows the behavior, replace names, addresses, locations, and identifiers with
values that keep the shape and carry no person, and record which client and version produced
the original.

A case that cannot be anonymized without losing the behavior it demonstrates is not
accepted. Describe the behavior in prose and construct a synthetic case that shows it.

## Code and comments are in English

The repository, including comments and documentation, is written in English so the spell
checker works and so adopters can read it. US spelling — the `typos` locale is `en-us`.
Property and component names keep the RFC's spelling in prose, which is why `typos.toml`
carries them as vocabulary rather than as suppressions.

## Commits

Conventional Commits, validated by `committed` in the commit-msg hook:

```text
feat(core): preserve unknown parameters in their original order
fix(recur): apply EXDATE inside the iterator instead of after it
docs(adr): record the caller-supplied time zone decision
```

## Where disagreement belongs

An argument about what RFC 5545 requires is settled as a conformance case citing the
section, not in an issue thread. Where the RFC permits alternatives, the answer is a
caller-visible option and a case recording every permitted outcome, rather than one of them
becoming the default because it was the first written.
