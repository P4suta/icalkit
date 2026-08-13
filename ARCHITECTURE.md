# Architecture

This document is the current structural contract. The reasoning and historical package graph
remain in [the ADRs](docs/adr/); ADR 0014 records completion of the single-crate migration.

## Production boundary

`icalkit` is the sole production crate and the only prospective registry contract:

```text
application (HTTP, storage, credentials, ACL, current-time input)
                              |
                              v
                         icalkit API
       model · time · recurrence · scheduling · caldav · interop
                              |
                              v
                    private implementation DAG
kernel -> wire (iCalendar/XML) -> model -> recurrence/timezone
       -> scheduling/query -> CalDAV workflows -> interop/adapters
```

The old `ical-core`, `ical-recur`, `ical-tz`, `ical-itip`, `ical-query`, and `ical-dav`
package names are retired. `xtask architecture` rejects their return as workspace members or
dependencies of `icalkit`. Private modules have no independent semver contract and are
unreachable through production rustdoc.

The workspace contains:

| Unit | Purpose | Published |
| --- | --- | --- |
| `crates/icalkit` | sole production API and implementation | deferred |
| `crates/icalkit-conformance` | JSONL subject, corpus, and shared-source adversarial tests | never |
| `gates/grammar-layering` | isolated compile gate for the iCalendar grammar | never |
| `gates/xml-layering` | isolated compile gate for the XML wrapper | never |
| `xtask` | architecture, API, purity, and diagnostic gates | never |

Every package remains at `0.0.0`. Publishing requires a separate explicit decision.

## Input and mutation states

```text
bytes -> Import -> Normalization -> Import -> Calendar -> bounded workflows
          |            |                     |
          |            +-- complete Change report
          +-- admitted octets unchanged       +-- strict validated CST
```

- `Calendar::parse` is strict shorthand under secure defaults. A structural or standards error
  prevents promotion; unknown valid extensions do not.
- `Import::read` preserves every admitted octet. Repair is explicit, sealed, versioned, reported,
  immutable with respect to the original, and idempotent.
- `Calendar` is an opaque validated CST. Typed getters do not repeatedly expose malformed known
  values, while generic component/property views preserve unknown data.
- `Calendar::edit` is transactional. Drop means rollback; `commit` validates a complete private
  copy before replacement. Only edited lines are canonicalized.
- `ProjectedCalendar` is a distinct type, so a reduced CalDAV response cannot overwrite a full
  stored resource.

Ordinary callers see `ResourcePolicy` and `Session` rather than the internal `Limits`, `Meter`,
tokenizer, XML events, candidate sets, or diagnostic sinks. One session ledger spans all work
performed through it.

## Purity and platform contact

Production code uses `#![no_std]` with `alloc` and `#![forbid(unsafe_code)]`. The kernel has no
clock, network, storage, runtime, credential, or ACL dependency. All current-time values,
including iTIP `DTSTAMP`, come from the caller.

The only Cargo features are:

| Feature | Default | Effect |
| --- | --- | --- |
| `std` | yes | enables Jiff's standard-library support |
| `system-tz` | yes | installs the Jiff-backed system time-zone adapter and implies `std` |

All calendar, recurrence, scheduling, query, client, and server capabilities are unconditional.
`just no-std` builds `icalkit` for `thumbv7em-none-eabi` with neither feature; `just wasm` checks
the same surface for `wasm32-unknown-unknown`.

## Time boundary

Jiff 0.2 supplies the public `Date`, `DateTime`, `Time`, `Weekday`, `Timestamp`, and
`SignedDuration` values. `IcalDateTime` is the thin domain wrapper needed for iCalendar DATE,
floating, UTC, zoned, `TZID`, and leap-second witness semantics that Jiff does not represent in
one value.

`time::ZoneDatabase` is the only application-implementable production trait. It exposes only
`resolve_local` and `offset_at`; answers include gap/fold ambiguity, provenance, and coverage.
`Engine` owns it behind a trait object so application types do not become generic over the
calendar stack.

With `system-tz`, Jiff reads the platform database on Unix/Android and uses its platform bundle
where required on Windows. That adapter is the only default OS contact. Embedded `VTIMEZONE`
interpretation stays private and disagreements are represented rather than silently preferred.

## Wire boundaries

The content-line parser and XML parser are private layers rather than public token APIs.
`xmlparser` is the private XML lexical authority. The wrapper remains responsible for namespace
scope, duplicate attributes, start/end matching, depth, aggregate budgets, rejecting DTDs and
processing instructions, and retaining `calendar-data` octets.

CalDAV exposes owned/borrowed `WireRequest`, `WireResponse`, and `Header` values containing only
method, URI, headers, and body. Client operations follow
`next_request -> accept -> finish`. Server operations follow
`next_need -> supply -> finish`. No `http`, `reqwest`, async runtime, storage, or ACL type crosses
the API.

Query evaluation returns the three-valued `Match`. Recurrence always has a time window and a
fallible pull operation, so budget exhaustion cannot be confused with completion.
`Calendar::occurrences` composes the stored master, `RDATE`, `EXDATE`, and sibling overrides
through the same zone-aware recurrence-set assembly used by query evaluation, then exposes
effective-start order and an opaque cursor. Scheduling authorization borrows the reviewed inputs
and is consumed by apply.

## Conformance isolation

`icalkit-conformance` is not another production API. Its stable interoperability boundary is
the versioned `icalkit-conformance/1` JSONL protocol. Runtime operations use `icalkit`.

Low-level adversarial tests still need access below the facade. The helper library compiles the
private module tree as shared source in a separate unpublished root. This deliberately gives the
corpus an isolation seam without making `icalkit::internal` public or recreating package
contracts. The public API snapshot proves that no helper path escapes production.

Client-shaped synthetic fixtures are labeled synthetic. Only reduced and anonymized captures
with producer version and observation date may justify a `CommonClientsV1` repair. Until that
evidence exists, the profile does not guess at CP1252 or vendor `TZID` mappings.

## Mechanical enforcement

- `just architecture` freezes the single production package, rejects retired package names,
  freezes the two-feature vocabulary, and holds the public guide's workflow order.
- `just public-api` compares committed default and no-default snapshots and rejects additions,
  removals, moves, or duplicate canonical paths.
- `just purity` and the two layering members enforce the private DAG and keep grammar/XML
  vocabulary-independent.
- `just codes` freezes diagnostic identifiers and meanings.
- `just lint` denies warnings with every feature and with no default features.
- `just test` runs process-isolated tests and compile-checked rustdoc.
- `just no-std` and `just wasm` prove the non-OS build boundary.
- `just shear`, `just deny`, and `just reuse` hold dependencies, supply-chain policy, and
  licensing.

The complete rationale is [ADR 0014](docs/adr/0014-private-kernel-and-conformance-isolation.md).
