# Architecture

The production structure follows the single-crate migration in [ADR 0014](docs/adr/0014-private-kernel-and-conformance-isolation.md).
The [architecture decisions](docs/adr/) record the reasoning and historical package graph.

## Production boundary

`icalkit` supplies the sole production API and implementation.
Publication requires a separate explicit decision.

```text
application (HTTP, storage, credentials, access control, current-time input)
                              |
                              v
                         icalkit API
       model · time · recurrence · scheduling · caldav · interop
                              |
                              v
                    private implementation graph
kernel -> wire (iCalendar/XML) -> model -> recurrence/timezone
       -> scheduling/query -> CalDAV workflows -> interop/adapters
```

`just architecture` rejects the retired `ical-core`, `ical-recur`, `ical-tz`, `ical-itip`, `ical-query`, and `ical-dav` package names.
They can't return as workspace members or production dependencies.
Private modules have no independent semantic versioning contract and stay outside production Rustdoc.

`crates/icalkit-conformance` supplies the corpus, shared-source adversarial tests, and the `icalkit-conformance/1` newline-delimited JSON protocol.
The `gates/grammar-layering` and `gates/xml-layering` packages compile their private layers in isolation.
`xtask` runs repository validation and orchestration.
Those packages remain unpublished, and every workspace package remains at `0.0.0`.

## Input and mutation states

```text
bytes -> Import -> Normalization -> Import -> Calendar -> bounded workflows
          |            |                     |
          |            +-- complete Change report
          +-- admitted octets unchanged       +-- strict validated syntax tree
```

`Calendar::parse` applies strict validation with secure resource defaults.
A structural or standards error prevents promotion.
Valid unknown extensions preserve their content.

`Import::read` preserves every admitted octet.
Normalization uses an explicit profile with a fixed version and sealed implementations.
It reports every change.
It preserves the original import and produces the same bytes when applied again.

`Calendar` holds a validated concrete syntax tree.
Typed getters expose admitted known values, while generic component and property views preserve unknown data.

`Calendar::edit` stages a private copy.
Dropping the editor rolls back pending changes.
`commit` validates the complete copy before replacing the calendar.
Editing canonicalizes only changed lines.
`ProjectedCalendar` keeps a projected CalDAV response distinct from a complete resource.

Callers configure `ResourcePolicy` and work through a `Session`.
The private kernel holds limits, meters, tokens, XML events, candidate sets, and diagnostic sinks.
One session meter spans every operation performed through that session.

## Purity and platform contact

Production code uses `#![no_std]` with `alloc` and `#![forbid(unsafe_code)]`.
The kernel has no clock, network, storage, runtime, credential, or access-control dependency.
The caller supplies the current instant, including the value used for iTIP `DTSTAMP`.

`std` enables Jiff's standard-library support.
`system-tz` installs the Jiff system time zone adapter and implies `std`.
`default` enables both features.
The manifest declares `default`, `std`, and `system-tz` as its complete feature vocabulary.
Calendar, recurrence, scheduling, query, client, and server capabilities remain unconditional.

`just no-std` builds `icalkit` for `thumbv7em-none-eabi` without default features.
`just wasm` checks the same configuration for `wasm32-unknown-unknown`.
An executable still needs a global allocator for the library's allocations.

## Time boundary

Jiff 0.2 supplies the public `Date`, `DateTime`, `Time`, `Weekday`, `Timestamp`, and `SignedDuration` values.
`IcalDateTime` represents iCalendar `DATE`, floating, UTC, zoned, `TZID`, and leap-second witness semantics around Jiff values.

ICU4X 2.4 converts registered calendar coordinates through the private `icu_calendar` dependency.
`recurrence::CalendarDate` converts to and from Jiff's Gregorian `Date`.
`recurrence::CalendarMonth` distinguishes regular and leap months.
`icu_calendar` has `default-features = false` and uses compiled calendar data.
The [calendar conversion decision](docs/adr/calendar-conversion-at-the-jiff-boundary.md) records the dependency and shared Rust 1.88 floor.
ICU4X types stay outside the public API.

Callers supply time zone sources through `time::ZoneDatabase`.
It exposes `resolve_local` and `offset_at`.
Answers include gap or fold ambiguity, provenance, and coverage.
`Engine` holds the database behind a trait object.

With `system-tz`, Jiff reads the platform database on Unix and Android and uses its platform bundle where Windows requires one.
The adapter supplies the default operating system contact.
Embedded `VTIMEZONE` interpretation stays private and reports disagreements between zone answers.

## Wire boundaries

The content-line parser and XML parser remain private.
`xmlparser` supplies XML tokens.
The wrapper validates namespaces, duplicate attributes, matching elements, depth, and shared resource budgets.
It rejects document type declarations and processing instructions and retains `calendar-data` octets.

CalDAV exposes owned and borrowed `WireRequest`, `WireResponse`, and `Header` values containing methods, URIs, headers, and bodies.
Client operations follow `next_request -> accept -> finish`.
Server operations follow `next_need -> supply -> finish`.
The API exposes no `http`, `reqwest`, asynchronous runtime, storage, or access-control type.

Query evaluation returns the three-valued `Match`.
Recurrence uses a caller-supplied time window.
Its fallible pull operation distinguishes budget exhaustion from completion.
`Calendar::occurrences` combines the stored master, `RDATE`, `EXDATE`, and sibling overrides through the recurrence-set assembly that query evaluation uses.
It exposes occurrences in effective start order and an opaque cursor.
Scheduling authorization borrows the reviewed inputs, and applying a change consumes that authorization.

## Conformance isolation

`icalkit-conformance` keeps its private Rust helpers outside the production API.
The `icalkit-conformance/1` protocol provides its interoperability boundary, and runtime operations use `icalkit`.

The helper library compiles the private module tree as shared source in a separate unpublished root.
Adversarial tests can exercise private behavior without exposing `icalkit::internal` or recreating package contracts.
Public API snapshots reject changes that expose helper paths.

The corpus labels synthetic fixtures as synthetic.
A `CommonClientsV1` repair requires reduced, anonymized captures with the producer version and observation date.
The profile applies no repair inferred from synthetic input.
The [capture intake](crates/icalkit-conformance/CAPTURE.md) defines the observation requirements.

## Mechanical enforcement

`just architecture` checks the single production package, retired package names, feature vocabulary, public guide, and roadmap.
It keeps both CalDAV body encoders on the shared `XmlWriter`.
`just purity` checks dependency identities and private implementation boundaries.
The grammar and XML layering packages compile the isolated sources.

`just public-api` compares the default and no-default API snapshots.
`just codes` checks diagnostic identifiers and meanings.
`just lint` denies warnings with all features and without default features.
`just test` runs process-isolated tests and compile-checked Rustdoc examples.

`just allocation` measures peak and retained allocation for iCalendar properties and XML elements.
The conformance suite includes a 200,000-property input and a query over 5,000 resources.
`just no-std` and `just wasm` check the supported targets without operating system adapters.

`just shear` checks unused dependencies.
`just deny` checks dependency advisories, sources, versions, and licenses.
`just reuse` checks file licensing.
The [contribution guide](CONTRIBUTING.md) defines the development commands and test-driven workflow.
