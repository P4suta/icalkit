# ADR-0013: one public crate with an explicit interoperability boundary

- Status: accepted
- Date: 2026-08-14
- Supersedes: the public crate graph in ADR-0003, ADR-0004, ADR-0005, and ADR-0012

## Context

The split crates proved the protocol layers independently, but exposed those implementation
seams as several semver contracts. A normal consumer had to assemble parser limits,
diagnostic sinks, time-zone sources, recurrence candidates, XML vocabulary, and scheduling
types before it could perform one calendar operation. The arrangement also made it possible
for tolerant repairs to leak into otherwise strict paths.

The useful boundary is not a crate per RFC. It is the transition from recovered input to a
validated calendar, followed by bounded calendar workflows. HTTP execution, persistence,
credentials, clocks, and application ACL decisions remain outside that boundary.

## Decision

`icalkit` is the only production crate and remains at version `0.0.0` until a separate
release decision. The existing split crates are temporary migration scaffolding. Their code
will move, without behavioral rewrites, into private modules with this dependency direction:

```text
kernel -> wire (iCalendar/XML) -> model -> recurrence/timezone
       -> scheduling/query -> CalDAV workflows -> interop/adapters
```

The kernel remains `no_std + alloc`, sans-I/O, free of clock, network, storage, and platform
policy. Production code forbids unsafe Rust. Compile gates and `xtask architecture` enforce
the module direction as the migration replaces crate boundaries.

Input follows one explicit typestate pipeline:

```text
bytes -> Import (lossless) -> explicit versioned normalization
      -> Calendar (strictly validated) -> bounded workflows
```

`Calendar::parse` is the strict shorthand with secure defaults. A structural or semantic
error prevents promotion; standards-compliant unknown extensions do not. Notes are retained.
`Engine::session` owns aggregate budgets and an optional zone source without exposing meters
or diagnostic sinks to ordinary users.

`interop::Import` retains every recoverable octet. Repairs run only when a caller selects a
sealed, versioned profile such as `RfcRepairV1` or `CommonClientsV1`. Normalization never
mutates the import, reports every change using stable codes, and must be idempotent. A changed
rule is a new profile version.

`Calendar` owns a validated lossless tree. Reads use typed views or generic validated
component/property views. Mutation is transactional through `Calendar::edit`; only a
successful `commit` replaces the calendar, and untouched content lines retain their original
octets.

Only these modules are public: `model`, `time`, `recurrence`, `scheduling`, `caldav`,
and `interop`. The principal root types are `Calendar`, `Engine`, `EngineBuilder`,
`Session`, `ResourcePolicy`, `Error`, `Issue`, and `IssueCode`. Public structs keep
private fields and stable error identifiers are string newtypes.

The only Cargo features are `std` and `system-tz`, both enabled by default. Protocol
capabilities are not features. Jiff 0.2 provides the public civil-time types. The
`system-tz` adapter may use Jiff's system database on Unix and Android and its platform
bundle on Windows. The purity gate therefore permits exactly the `icalkit -> jiff` external
edge; split implementation crates cannot acquire it. `IcalDateTime` remains an iCalendar
domain wrapper for DATE, floating, UTC, zoned, and leap-second evidence that Jiff does not
represent directly. `time::ZoneDatabase` is the sole application-implementable public trait.

CalDAV remains sans-I/O. Client operations yield `WireRequest` values and accept
`WireResponse` values. Server operations yield explicit storage, routing, or ACL needs and
consume application answers. Neither surface exposes an HTTP, async-runtime, or persistence
type. Query projections use a distinct `ProjectedCalendar` type so partial data cannot be
passed to persistence APIs as a complete calendar.

The conformance runner becomes a private CLI/corpus communicating through a versioned JSONL
subject protocol. It is not a second Rust library API.

## Consequences

Users learn one crate and one input pipeline. Repairs are auditable and cannot occur
implicitly in the strict kernel. The implementation may retain small internal crates during
migration, but tests and documentation use the facade and those crates are removed from the
release configuration when the move completes.

Jiff and the future private XML tokenizer dependency are reviewed architecture exceptions,
not permission for arbitrary third-party dependencies in the kernel. All HTTP execution,
storage, credentials, current-time acquisition, and application ACL policy remain caller
ports.

The old ADRs remain as history. Their lossless, bounded, sans-I/O, and caller-policy
principles still apply; only their public package and type boundaries are superseded.
