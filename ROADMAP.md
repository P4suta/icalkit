# Roadmap

## Maintained implementation

The status describes the maintained workspace on November 17, 2026.
The milestone names keep their original scope, while `icalkit` contains the production implementation.
The [private kernel decision](docs/adr/0014-private-kernel-and-conformance-isolation.md) defines the package graph and dependency boundaries.
Git history retains the earlier milestone narrative and defect investigations.

Published specifications define normative behavior.
The [specification validation guide](docs/specification-validation.md) records the contextual constraints, source sections, regression targets, and scheduling creation workflow.
Development follows the [test-driven contribution workflow](CONTRIBUTING.md#test-driven-changes).
Further specification cases must reproduce missing behavior before implementation changes.

## M0 calendar data

The library preserves admitted input bytes through import and typed views.
Strict promotion validates known component structure, required properties, text escaping, integer and coordinate bounds, date-time forms, durations, related fields, and recurrence constraints.
Unknown components and `VALUE` types keep their opaque content.
Transactional editing and explicit normalization share the tokenizer with strict parsing.
The [round-trip decision](docs/adr/0001-lossless-round-trip.md) defines preservation requirements.

The allocation gate measures retained and peak allocation, including a 200,000-property input.
The [shared limits decision](docs/adr/0010-shared-resource-limits.md) requires operations to charge the caller's meter.
Budget exhaustion produces an error.

## M1 recurrence

Stored recurrence workflows combine `RRULE`, `RDATE`, `EXDATE`, detached overrides, and `RANGE=THISANDFUTURE`.
Searches use caller-supplied windows, charge generated candidates, and expose resumable cursors.
Public occurrences follow effective start order.
The corpus includes the 42 recurrence examples in [RFC 5545 section 3.8.5.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.3).
The [bounded recurrence decision](docs/adr/0002-bounded-lazy-recurrence.md) defines iterator and budget behavior.

The recurrence module converts registered calendar coordinates to and from Jiff Gregorian dates.
Regular month numbers and leap month identifiers remain distinct, including calendars with thirteen regular months.
The [conversion decision](docs/adr/calendar-conversion-at-the-jiff-boundary.md) records the private ICU4X adapter and Rust 1.88 floor.
The private period walker advances month and year anchors in the selected calendar, including leap months and thirteen-month years.
The [period arithmetic decision](docs/adr/calendar-coordinates-for-recurrence-periods.md) records interval spacing, index resumption, and Gregorian boundary handling.
Non-Gregorian recurrence expansion still requires every date selector and period step to use the converted coordinates.

## M2 time zones

The library reads embedded `VTIMEZONE` definitions and resolves local times through caller-supplied sources.
Resolution values expose gaps, folds, source identity, and continuation beyond known transitions.
Zoned endpoint ordering requires zone-aware evaluation.
Strict parsing preserves the identifiers without imposing wall-clock order across transitions.

An unresolved zone can prevent placement of an exclusion or recurrence identity.
Repeated local times can identify two instants, so workflows must preserve the reported ambiguity rather than choose an unstated instant.
The [time zone decision](docs/adr/0003-caller-supplied-time-zones.md) records those boundaries.
Vendor identifier mappings belong to caller-supplied adapters.

## M3 scheduling

The `iTIP` and `iMIP` workflows review messages, check authorization, and apply scheduling changes.
Authorization borrows the reviewed inputs and permits applying a change once.
Creation uses `Message::review_creation` when the destination is absent.
The caller must preserve absence when committing the result to storage.

Cancellation notices preserve stored properties omitted from sparse payloads.
The [scheduling decision](docs/adr/0005-scheduling-apart-from-the-model.md) records method constraints, authorization, ordering, delegation, and recurrence identity.
Transport authentication remains the caller's responsibility.

## M4 calendar access

The `CalDAV` client and server state machines provide discovery, queries, projection, conditional writes, synchronization, collection creation, and scheduling.
Applications supply transport, persistence, credentials, routing, and access decisions.
The [protocol boundary](docs/adr/0004-sans-io-protocol-layer.md) defines the separation.

The private XML reader resolves namespaces and enforces structural and resource limits.
The writers share one stack-balanced encoder.
Calendar data can preserve original line endings, while `TextPolicy::Normalized` follows XML text normalization.
Preserved calendar data must not serve as input for XML signature verification.
The library rejects non-UTF-8 calendar data when writing an XML envelope.

## M5 producer observations

Specification cases and producer observations serve separate purposes.
The private `icalkit-conformance/1` command protocol runs the committed corpus without another public Rust library contract.
Synthetic fixtures test specified behavior without asserting that a producer emitted or accepted the fixture.

The [corpus manifest](crates/icalkit-conformance/corpus/manifest.v1.jsonl) records one reduced Google Calendar Web `70.9054` daylight saving gap capture.
Its observed outcome is `offset-before`.
Apple Calendar and Microsoft 365 remain unobserved for the scenario.
No isolated capture environment is available, so specification implementation continues without waiting for captures.

Changing the gap default requires observations from all three producers.
At least two producers must shift, and none may skip.
The [civil time decision](docs/adr/0011-civil-time-arithmetic-and-resolution-types.md) defines the threshold and scenario.
The alternative delegation policy requires two captures from each of four clients under the scheduling decision.
Until the thresholds hold, strict behavior and the explicit delegation hold remain in force.
`CommonClientsV1` applies no repair inferred from synthetic input.
The [capture intake](crates/icalkit-conformance/CAPTURE.md) defines provenance, reduction, and anonymization requirements for future observations.

## Verification and publication

Run `mise x -- just ci` for the maintained local checks.
The command covers formatting, linting, tests, documentation, resource limits, feature combinations, target builds, public API snapshots, dependency policy, and Rust 1.88 compatibility.
The candidate workflow also runs Windows and macOS tests and the declared source proofs for one complete commit ID.

Every package remains at `0.0.0`.
Publication requires a separate release instruction and candidate qualification.
The [repository policy](docs/repository-policy.md) defines the hosting and merge requirements.

## Scope boundaries

Applications supply time zone sources, the current instant, transport, and storage.
Access-control evaluation and the principal search reports remain the caller's responsibility.
vCard and CardDAV remain outside the workspace's scope.

An allocation-free tier would require a separate crate and lint profile under the [allocation decision](docs/adr/0007-allocation-policy.md).
The bare-metal build verifies `no_std` compatibility and still needs a global allocator.
