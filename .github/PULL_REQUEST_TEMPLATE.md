## What this changes

<!-- Describe the changed behavior in one or two sentences. -->

## Preservation

Confirm byte-identical serialization of parsed input and typed views over preserved text.
A specification violation must remain a diagnostic attached to the item without discarding the input.

<!--
Dropping an unrecognized property destroys another client's data.
See docs/adr/0001.
Delete this section if the change cannot reach the model.
-->

## Bounds and time

Confirm recurrence bounds from the caller's window and candidate budget, with exhaustion reported as an outcome.
Clock reads and zone resolution require caller-supplied sources, and every answer must identify its source.
Represent ambiguous and nonexistent local times explicitly.

## Scheduling

<!-- Delete this section if `icalkit::scheduling` is untouched. -->

Confirm that results describe transitions the caller can show a user without mutating state.
Authorization must reject meeting changes through attendee replies and reject a `REPLY` from an address outside the attendee list.

## Conformance

Identify a case in `icalkit-conformance` citing the RFC section for every affected rule.
Where implementations disagree, record each observed behavior and the project's choice.
Reduce and anonymize real exports while preserving the demonstrated behavior, and record the originating client and version.

## Checks

Report the result of `just check` and name every unrun check.
Use candidate qualification for affected portability or compatibility contracts.
Confirm that the sans-I/O core gained no `std`, clock, network, or time zone database dependency through `just purity`, `just no-std`, and `just wasm`.
Don't add `allow` or `ignore` to make a gate pass.

<!--
Explain every changed gate.
-->
