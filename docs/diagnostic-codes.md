# Diagnostic codes

The [diagnostic decision](adr/0009-error-and-diagnostic-model.md) keeps each diagnostic key and its meaning stable.
A changed meaning requires a rename or deprecation.
The [conformance decision](adr/0006-conformance-corpus-as-artifact.md) requires cases for diagnostic outcomes and channels.

`just codes` compares the table with the declarations in `crates/icalkit/src/internal/core/grammar/report.rs`.
The gate checks keys, meanings, row order, channels, and milestone names.
Emitter behavior requires conformance tests alongside the registry check.

## Contract fields

The `code` field contains the key returned by `DiagnosticCode::as_str`.
The `meaning` field contains the first paragraph of the variant's source comment.
Each meaning stays unchanged inside the [literal registry block](adr/diagnostic-meanings-as-literal-data.md).
The gate compares the literal contents without changing the source sentence.

The `channel` field names the `Severity` used by the emitter.
Changing the channel can change how callers handle the same input.
Every declared severity must appear in the registry.

The `milestone` field records the implementation milestone from [ROADMAP.md](../ROADMAP.md).
Milestones describe implementation responsibility, including emitters added after the initial calendar parser.

## Notes and violations

A note describes supported preservation or behavior that requires caller policy.
Examples include unknown value types, undefined caret escapes, ambiguous local times, and foreign protocol elements.
Violations cover malformed known data and the project's strict formatting rules.
Strict promotion enforces the [recommended physical line limit](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.1) as project policy.
The limit is 75 octets, excluding line endings.
Strict promotion rejects violations and keeps notes with successfully validated values.
Resource failures use the channels defined by the diagnostic and allocation decisions.

## Registry data

```text
| code | meaning | channel | milestone |
| --- | --- | --- | --- |
| invalid-utf8-text | A typed view encountered invalid UTF-8 and kept the original octets. | Violation | M0 |
| missing-value-separator | A content line carried no `:`, so it has a name and no value. | Violation | M0 |
| empty-property-name | A content line had an empty property name. A blank line is the degenerate case. | Violation | M0 |
| parameters-on-component-boundary | A `BEGIN` or `END` line carried parameters that the component boundary syntax forbids. | Violation | M0 |
| unmatched-end | An `END` arrived with no `BEGIN` open. | Violation | M0 |
| mismatched-end-name | An `END` named a different component than the `BEGIN` it closed. | Violation | M0 |
| unclosed-component | A `BEGIN` was never closed before the input ended. | Violation | M0 |
| bare-line-feed | A line ended with a bare `LF` instead of the `CRLF` that RFC 5545 requires. | Violation | M0 |
| bare-carriage-return | A line ended with a bare `CR` instead of the `CRLF` that RFC 5545 requires. | Violation | M0 |
| missing-final-line-break | The last line of the input carried no terminator at all. | Violation | M0 |
| line-too-long | A physical line exceeded the 75 octets that RFC 5545 section 3.1 recommends. | Violation | M0 |
| control-character-in-text | A value or parameter held a control character RFC 5545 section 3.1 excludes. | Violation | M0 |
| unterminated-quoted-parameter | A `DQUOTE`-delimited parameter value was never closed. | Violation | M0 |
| undefined-caret-escape | An octet following `^` had no meaning under RFC 6868. | Note | M0 |
| parameter-without-value | A parameter arrived with a name and no `=`. | Violation | M0 |
| duplicate-property | A property the specification declares at most once occurred more than once. | Violation | M0 |
| malformed-date | A `DATE` value violated RFC 5545 section 3.3.4. | Violation | M0 |
| malformed-date-time | A `DATE-TIME` value violated RFC 5545 section 3.3.5. | Violation | M0 |
| malformed-time | A `TIME` value violated RFC 5545 section 3.3.12. | Violation | M0 |
| malformed-duration | A `DURATION` value violated RFC 5545 section 3.3.6. | Violation | M0 |
| malformed-period | A `PERIOD` value violated RFC 5545 section 3.3.9. | Violation | M0 |
| malformed-utc-offset | A `UTC-OFFSET` value violated RFC 5545 section 3.3.14. | Violation | M0 |
| malformed-geo | A `GEO` value lacked the valid `FLOAT;FLOAT` pair that RFC 5545 section 3.8.1.6 requires. | Violation | M0 |
| malformed-integer | An `INTEGER` value exceeded the supported range or fell outside the syntax of RFC 5545 section 3.3.8. | Violation | M0 |
| malformed-float | A `FLOAT` value violated RFC 5545 section 3.3.7. | Violation | M0 |
| malformed-boolean | A `BOOLEAN` value was neither `TRUE` nor `FALSE` in any casing. | Violation | M0 |
| malformed-binary | A `BINARY` value had invalid base64 encoding under RFC 5545 section 3.3.1. | Violation | M0 |
| malformed-uri | A `URI` or `CAL-ADDRESS` value had invalid URI syntax under RFC 5545 sections 3.3.13 and 3.3.3. | Violation | M0 |
| malformed-value-type-parameter | A property declared an empty, repeated, or malformed `VALUE` parameter. | Violation | M0 |
| unknown-value-type | A `VALUE` parameter named an unknown value type. | Note | M0 |
| missing-required-property | A component lacked a property that RFC 5545 section 3.6 requires. | Violation | M0 |
| property-not-allowed-here | A component carried a property outside its definition in RFC 5545 section 3.6. | Violation | M0 |
| mutually-exclusive-properties | A component carried a pair of properties that RFC 5545 section 3.6 forbids together. | Violation | M0 |
| resource-budget-exhausted | An earlier resource refusal prevented further admission. | LimitReached | M2 |
| recurrence-budget-exhausted | A recurrence search stopped at the candidate budget rather than at the rule's end. | LimitReached | M1 |
| recurrence-calendar-ended | The recurrence search reached the last representable date before the rule ended. | Note | M1 |
| nonexistent-recurrence-instance | A recurrence rule generated a nonexistent date, so expansion skipped that instance under RFC 5545 section 3.3.10. | Note | M1 |
| malformed-recurrence-rule | A `RECUR` value had no usable rule under RFC 5545 section 3.3.10. | Violation | M1 |
| duplicate-recurrence-rule-part | A `RECUR` value repeated a rule part that RFC 5545 section 3.3.10 permits once. | Violation | M1 |
| unknown-recurrence-rule-part | A `RECUR` value named a rule part outside RFC 5545 section 3.3.10. | Note | M1 |
| unsupported-recurrence-semantics | A `RECUR` value requires unsupported `RSCALE` or `SKIP` semantics. | Violation | M1 |
| recurrence-rule-part-out-of-range | A `RECUR` value included a part outside its RFC 5545 section 3.3.10 range, and the reader kept the remaining parts. | Violation | M1 |
| recurrence-until-value-type-mismatch | An `UNTIL` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545 section 3.3.10 requires to agree. | Violation | M1 |
| by-set-pos-without-by-rule | A `RECUR` value included `BYSETPOS` without another `BYxxx` part, contrary to RFC 5545 section 3.3.10. | Violation | M1 |
| mutually-exclusive-rule-parts | A `RECUR` value carried both `UNTIL` and `COUNT`, which RFC 5545 section 3.3.10 forbids in one recur. | Violation | M1 |
| extra-recurrence-rule-ignored | A component supplied more than one `RRULE` property, and expansion used the first. | Violation | M1 |
| exdate-shadows-override | An `EXDATE` and a `RECURRENCE-ID` named the same instant, and the exclusion won. | Note | M1 |
| override-left-window | An override moved an occurrence's start outside the window its cadence key fell in. | Note | M1 |
| override-shift-not-representable | An override shifted an occurrence outside the representable timeline, so expansion skipped that occurrence. | Violation | M1 |
| unknown-time-zone | A `TZID` named a zone no supplied source could resolve. | Violation | M2 |
| missing-time-zone-definition | A `TZID` parameter named a zone with no `VTIMEZONE` in the same calendar. | Violation | M2 |
| ambiguous-local-time | A local time occurs twice under its zone, at the end of a daylight saving period. | Note | M2 |
| nonexistent-local-time | A local time fell inside a gap at the start of a daylight saving period. | Violation | M2 |
| time-zone-source-disagreement | An embedded `VTIMEZONE` and the caller's other zone source disagreed about an offset. | Note | M2 |
| vtimezone-without-observance | A `VTIMEZONE` carried neither a `STANDARD` nor a `DAYLIGHT` subcomponent, which RFC 5545 section 3.6.5 requires at least one of. | Violation | M2 |
| vtimezone-rule-unsupported | An observance supplied an `RRULE` outside the supported yearly forms, so the reader derived no transition from that rule. | Note | M2 |
| vtimezone-observances-truncated | A `VTIMEZONE` exceeded the caller's observance limit, so the reader omitted the excess observances. | LimitReached | M2 |
| duplicate-time-zone-identifier | More than one `VTIMEZONE` declared the same `TZID`. | Violation | M2 |
| time-zone-coverage-exhausted | A lookup beyond the final known transition continued the zone's final observance. | Note | M2 |
| recurrence-until-not-utc | A local `UNTIL` used the `DTSTART` zone where RFC 5545 section 3.3.10 requires UTC. | Violation | M2 |
| exdate-value-type-mismatch | An `EXDATE` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545 section 3.8.5.1 requires to agree. | Violation | M2 |
| override-matches-no-instance | A `RECURRENCE-ID` had no matching series occurrence, so the override changed no instance. | Violation | M2 |
| time-zone-without-transitions | A zone source recognized a `TZID` and holds no transition for it, so no wall clock names an instant under it. | Violation | M2 |
| time-zone-before-known-transitions | A lookup before the first known transition continued the offset in force before that transition. | Note | M2 |
| time-zone-coverage-unasserted | A zone provider returned an answer without claiming complete coverage for the requested time. | Note | M2 |
| vtimezone-components-truncated | A calendar exceeded the caller's `VTIMEZONE` count limit, so the reader omitted the excess definitions. | LimitReached | M2 |
| vtimezone-observance-unreadable | An observance's required value was present and unreadable, so it stated no transition at all. | Violation | M2 |
| exdate-zone-unknown | An `EXDATE` written in UTC named no cadence key because no source recognized the series' zone, so it excluded nothing. | Violation | M2 |
| exdate-boundary-unreadable | The zone provider found an unreadable boundary for a whole-day `EXDATE`. | Violation | M2 |
| scheduling-method-unknown | A `METHOD` was present and named no method RFC 5546 defines, so the message states no scheduling semantics at all. | Violation | M3 |
| scheduling-calendar-address-unreadable | An `ORGANIZER` or `ATTENDEE` had an unreadable `CAL-ADDRESS` and identified no party. | Violation | M3 |
| scheduling-sequence-unreadable | A `SEQUENCE` value had no integer revision number. | Violation | M3 |
| scheduling-property-not-allowed | A scheduling payload carried a property RFC 5546 section 3 forbids for its `METHOD` and component type. | Violation | M3 |
| scheduling-required-property-missing | A scheduling payload lacked a property RFC 5546 section 3 requires for its `METHOD` and component type. | Violation | M3 |
| scheduling-cancellation-status-invalid | A `CANCEL` payload carried `STATUS` with a value other than `CANCELLED`. | Violation | M3 |
| scheduling-instance-ambiguous | A `RECURRENCE-ID` named a wall clock its series' zone repeats, and nothing said which of the two instants it addresses. | Violation | M3 |
| scheduling-range-not-permitted | A `RECURRENCE-ID` carried `RANGE=THISANDFUTURE` under a `METHOD` that RFC 5546 forbids for that range. | Violation | M3 |
| scheduling-exclusion-unplaced | A scheduling message addressed a component with an unplaced exclusion, leaving its instance set unresolved. | Violation | M3 |
| scheduling-zone-continued | An instance identity used a zone answer that continued an observance beyond the source's known transitions. | Note | M3 |
| scheduling-sender-not-permitted | The sender lacked permission to send the message's `METHOD` under RFC 5546 section 3. | Violation | M3 |
| scheduling-method-ambiguous | A calendar stated more than one `METHOD`, so the verb of the whole message is two claims rather than one. | Violation | M3 |
| scheduling-instance-nonexistent | A `RECURRENCE-ID` fell in a local time gap, and the caller's policy dropped the instance. | Violation | M3 |
| dav-foreign-element-skipped | The reader skipped an unfamiliar element outside the `DAV:` and CalDAV vocabulary and skipped its descendants. | Note | M4 |
| dav-calendar-data-copied | The reader copied a `calendar-data` payload that it couldn't borrow from the body. | Note | M4 |
| dav-calendar-data-line-endings-folded | A `calendar-data` payload lost carriage returns to XML 1.0 section 2.11 line-ending normalization. | Note | M4 |
| dav-property-unmodeled | The reader kept the octets of a property with an unmodeled value. | Note | M4 |
| dav-status-unreadable | A `DAV:status` element lacked a valid status line under RFC 4918 section 14.28. | Violation | M4 |
| dav-response-without-href | A `DAV:response` carried no `href`, so it names no resource. | Violation | M4 |
| dav-responses-truncated | A multistatus exceeded the caller's response limit, so the reader omitted the excess responses. | LimitReached | M4 |
| dav-property-markup-dropped | A property mixed text and child elements, and the reader dropped the child markup. | Violation | M4 |
| dav-sync-token-withheld | The reader withheld a synchronization token because the response was incomplete. | LimitReached | M4 |
| query-calendar-data-reduced | A `calendar-data` selection produced a reduced or expanded representation of the stored calendar. | Note | M5 |
| query-filter-undecidable | The reader lacked enough information to decide whether the resource matched the filter. | Note | M5 |
```
