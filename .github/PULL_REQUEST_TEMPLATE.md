## What this changes

<!-- One or two sentences. What behavior is different after this lands? -->

## Preservation

- [ ] Anything this parses, it serializes back byte-identically
- [ ] Typed access added here is a view over preserved text, not the storage for it
- [ ] A specification violation is a diagnostic attached to the item, not an error that
      discards the input

<!--
Dropping a property nobody here recognizes is how one client destroys another client's
data. See docs/adr/0001. Delete this section if the change cannot reach the model.
-->

## Bounds and time

- [ ] Recurrence work is bounded twice: by the caller's window and by a candidate budget
      whose exhaustion is a reported outcome rather than a hang or an empty result
- [ ] No clock is read and no zone is resolved without a caller-supplied source, and every
      answer says which source produced it
- [ ] Ambiguous and non-existent local times are represented rather than silently resolved

## Scheduling

<!-- Delete this section if `ical-itip` is untouched. -->

- [ ] The result is a described transition the caller can show a user, not a mutation
- [ ] Authorization is decided by the semantics: an attendee cannot move a meeting by
      replying, and a `REPLY` from an address that is not on the attendee list is rejected

## Conformance

- [ ] Every rule this touches has a case in `icalkit-conformance` addressed to its RFC section
- [ ] Where implementations disagree, the case records each observed behavior and says
      which one this project chose
- [ ] Any real export added is reduced to the smallest form that still shows the behavior,
      anonymized, and records the client and version it came from

## Checks

- [ ] `just ci` passes locally
- [ ] The sans-I/O core gained no `std`, clock, network, or time zone database dependency
      (`just purity`, `just no-std`, `just wasm`)
- [ ] No `allow` or `ignore` was added to make a gate pass

<!--
If a gate was changed rather than satisfied, say why here. That is sometimes right, and it
always deserves a sentence.
-->
