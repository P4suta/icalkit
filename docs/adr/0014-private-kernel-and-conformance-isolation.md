# ADR-0014: private kernel and conformance isolation after crate unification

- Status: accepted
- Date: 2026-08-14
- Supersedes: the transitional package state and migration wording in ADR-0013

## Context

ADR-0013 chose one production crate and an explicit interoperability boundary, but recorded an
in-progress state: private implementations still had package-shaped compilation roots and core
had not moved. That state was useful for behavior-preserving migration, but it is not a stable
architecture. Leaving the old packages in the workspace would preserve the very semver seams the
decision set out to remove.

The adversarial corpus still needs to compile low-level grammar, recurrence, time-zone,
scheduling, and DAV code. Making those modules public for tests would create duplicate canonical
paths. Keeping the old crates would create duplicate package contracts. The test seam therefore
has to be isolated without becoming a production seam.

The migration also makes two reviewed third-party boundaries permanent. Jiff is the public civil
time vocabulary and optional system-zone adapter. `xmlparser` is a private lexical component,
not an XML structure or CalDAV model.

## Decision

The production workspace has one implementation package: `crates/icalkit`. The package names
`ical-core`, `ical-recur`, `ical-tz`, `ical-itip`, `ical-query`, and `ical-dav` are retired.
An architecture gate rejects their return as workspace members or dependencies of `icalkit`.
No private module has its own semver contract.

Implementation follows this private dependency direction:

```text
kernel -> wire (iCalendar/XML) -> model -> recurrence/timezone
       -> scheduling/query -> CalDAV workflows -> interop/adapters
```

The kernel remains `no_std + alloc`, sans-I/O, clock-free, network-free, storage-free, and
`forbid(unsafe_code)`. The application owns HTTP, persistence, credentials, the current time,
and ACL decisions. The only default OS contact is the `system-tz` adapter.

The only Cargo features remain `std` and `system-tz`, both enabled by default. Every protocol
capability is unconditional. `system-tz` uses Jiff 0.2's platform database behavior, including
its platform bundle where required on Windows.

Jiff's `Date`, `DateTime`, `Time`, `Weekday`, `Timestamp`, and `SignedDuration` are selectively
re-exported from `icalkit::time`. `IcalDateTime` remains an opaque domain value for DATE,
floating, UTC, zoned, `TZID`, and leap-second evidence. `time::ZoneDatabase` remains the sole
application-implementable production trait and exposes only local resolution and instant
offset lookup, with ambiguity, provenance, and coverage in its answers.

`xmlparser` is the private no-std lexical authority. The icalkit wrapper, not that dependency,
owns namespace scope, structural matching, duplicate-attribute rejection, depth and aggregate
budgets, DTD/processing-instruction rejection, and `calendar-data` octet retention. No XML token
or vocabulary type reaches a public signature.

`icalkit-conformance` remains unpublished and process-oriented. Its stable subject boundary is
the versioned JSONL protocol. For low-level adversarial tests only, its helper library path-loads
the private module tree as shared source into a separate compilation root. The production
`internal` ancestor remains private, and public-API snapshots prove those paths cannot escape.
This isolation helper has no release or semver standing.

The grammar and XML layering packages remain unpublished compile gates. `xtask architecture`,
`purity`, default/no-default public-API snapshots, bare-metal and WASM builds, Clippy, and the
full conformance suite mechanically hold the resulting boundary.

Every package remains at `0.0.0`. Publication is deferred indefinitely until an explicit
release instruction; architecture completion does not imply release authorization.

## Consequences

Consumers see one canonical API and cannot couple themselves to tokenizer, XML, meter,
candidate, RFC-table, or internal writer types. Implementation refactors no longer require
coordinated semver changes across RFC-shaped packages.

The conformance corpus keeps white-box reach without weakening the production boundary. Its
second compilation root is intentionally more sensitive to source layout, so moving a private
module must update the helper and both layering gates in the same change.

Jiff and `xmlparser` are explicit reviewed exceptions to the dependency-minimal kernel. They do
not authorize HTTP, runtime, serialization-framework, or storage dependencies.

Real Google Calendar, Microsoft 365, and Apple Calendar captures remain an evidence task rather
than something inferred from synthetic fixtures. `CommonClientsV1` acquires behavior only when
such a capture is reduced, anonymized, versioned, and committed with its change-code tests.

ADR-0013 remains the rationale for the public typestate and workflow API. This ADR replaces only
its migration state and makes the final package, isolation, Jiff, and XML boundaries
authoritative.
