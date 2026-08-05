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

## Rules that are not negotiable

- **No `allow` and no `ignore`.** Every gate is strict on purpose. Make the code pass
  instead of suppressing the finding. If a lint is genuinely wrong for this codebase,
  change the shared configuration and say why in the commit message.
- **The core stays `no_std` and sans-I/O.** `ical-core`, `ical-recur`, `ical-tz`,
  `ical-itip`, and `ical-dav` must not gain `std`, a bundled time zone database, a clock, or
  a transport. `just purity`, `just no-std`, and `just wasm` enforce it. A zone answer comes
  from a caller-supplied source and names that source
  ([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md)); "now" is an instant the caller
  passed in ([ADR 0004](docs/adr/0004-sans-io-protocol-layer.md)).
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
  `ical-conform` addressed to the RFC section it comes from is incomplete
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
