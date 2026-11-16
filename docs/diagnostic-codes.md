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
| invalid-utf8-text | Text that a typed view had to decode was not valid UTF-8. The octets are preserved. | Violation | M0 |
| missing-value-separator | A content line carried no `:`, so it has a name and no value. | Violation | M0 |
| empty-property-name | A content line had an empty property name. A blank line is the degenerate case. | Violation | M0 |
| parameters-on-component-boundary | A `BEGIN` or `END` line carried parameters, which a component boundary cannot hold. | Violation | M0 |
| unmatched-end | An `END` arrived with no `BEGIN` open. | Violation | M0 |
| mismatched-end-name | An `END` named a different component than the `BEGIN` it closed. | Violation | M0 |
| unclosed-component | A `BEGIN` was never closed before the input ended. | Violation | M0 |
| bare-line-feed | A line was terminated by a bare `LF`, where RFC 5545 requires `CRLF`. | Violation | M0 |
| bare-carriage-return | A line was terminated by a bare `CR`, where RFC 5545 requires `CRLF`. | Violation | M0 |
| missing-final-line-break | The last line of the input carried no terminator at all. | Violation | M0 |
| line-too-long | A physical line ran past the 75 octets RFC 5545 section 3.1 allows one. | Violation | M0 |
| control-character-in-text | A value or parameter held a control character RFC 5545 section 3.1 excludes. | Violation | M0 |
| unterminated-quoted-parameter | A `DQUOTE`-delimited parameter value was never closed. | Violation | M0 |
| undefined-caret-escape | A `^` was followed by an octet RFC 6868 gives no meaning. | Note | M0 |
| parameter-without-value | A parameter arrived with a name and no `=`. | Violation | M0 |
| duplicate-property | A property the specification declares at most once occurred more than once. | Violation | M0 |
| malformed-date | A `DATE` value did not match RFC 5545 section 3.3.4. | Violation | M0 |
| malformed-date-time | A `DATE-TIME` value did not match RFC 5545 section 3.3.5. | Violation | M0 |
| malformed-time | A `TIME` value did not match RFC 5545 section 3.3.12. | Violation | M0 |
| malformed-duration | A `DURATION` value did not match RFC 5545 section 3.3.6. | Violation | M0 |
| malformed-period | A `PERIOD` value did not match RFC 5545 section 3.3.9. | Violation | M0 |
| malformed-utc-offset | A `UTC-OFFSET` value did not match RFC 5545 section 3.3.14. | Violation | M0 |
| malformed-geo | A `GEO` value was not the `FLOAT;FLOAT` pair RFC 5545 section 3.8.1.6 requires. | Violation | M0 |
| malformed-integer | An `INTEGER` value did not match RFC 5545 section 3.3.8, or did not fit. | Violation | M0 |
| malformed-float | A `FLOAT` value did not match RFC 5545 section 3.3.7. | Violation | M0 |
| malformed-boolean | A `BOOLEAN` value was neither `TRUE` nor `FALSE` in any casing. | Violation | M0 |
| malformed-binary | A `BINARY` value was not the base 64 RFC 5545 section 3.3.1 requires. | Violation | M0 |
| malformed-uri | A `URI` value did not match RFC 5545 section 3.3.13, or section 3.3.3's `CAL-ADDRESS`. | Violation | M0 |
| unknown-value-type | A `VALUE` parameter named a value type this workspace does not know. | Note | M0 |
| missing-required-property | A component did not carry a property RFC 5545 section 3.6 requires of it. | Violation | M0 |
| property-not-allowed-here | A component carried a property RFC 5545 section 3.6 does not define for it. | Violation | M0 |
| mutually-exclusive-properties | A component carried two properties RFC 5545 section 3.6 does not allow together. | Violation | M0 |
| recurrence-budget-exhausted | A recurrence search stopped at the candidate budget rather than at the rule's end. | LimitReached | M1 |
| recurrence-calendar-ended | A recurrence search reached the end of the calendar RFC 5545 section 3.3.4 can write while the rule it was expanding had reached neither its `COUNT` nor its `UNTIL`. | Note | M1 |
| nonexistent-recurrence-instance | A recurrence rule generated an instance whose date does not exist, so it was filtered per RFC 5545 section 3.3.10 rather than moved to a nearby one. | Note | M1 |
| malformed-recurrence-rule | A `RECUR` value did not match the grammar of RFC 5545 section 3.3.10. | Violation | M1 |
| duplicate-recurrence-rule-part | A `RECUR` value named one rule part more than once, which RFC 5545 section 3.3.10 allows at most once. | Violation | M1 |
| unknown-recurrence-rule-part | A `RECUR` value named a rule part RFC 5545 section 3.3.10 does not define. | Note | M1 |
| recurrence-rule-part-out-of-range | A `RECUR` rule part carried a value outside the range RFC 5545 section 3.3.10 gives it, and the rest of the rule was kept. | Violation | M1 |
| recurrence-until-value-type-mismatch | An `UNTIL` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545 section 3.3.10 requires to agree. | Violation | M1 |
| by-set-pos-without-by-rule | A `RECUR` value carried `BYSETPOS` with no other `BYxxx` rule part to select from, which RFC 5545 section 3.3.10 forbids. | Violation | M1 |
| mutually-exclusive-rule-parts | A `RECUR` value carried both `UNTIL` and `COUNT`, which RFC 5545 section 3.3.10 forbids in one recur. | Violation | M1 |
| extra-recurrence-rule-ignored | A component offered more than one `RRULE` and only the first was expanded. | Violation | M1 |
| exdate-shadows-override | An `EXDATE` and a `RECURRENCE-ID` named the same instant, and the exclusion won. | Note | M1 |
| override-left-window | An override moved an occurrence's start outside the window its cadence key fell in. | Note | M1 |
| override-shift-not-representable | An override moved an occurrence's start off the representable timeline, so the occurrence was filtered rather than moved to a nearby instant. | Violation | M1 |
| unknown-time-zone | A `TZID` named a zone no supplied source could resolve. | Violation | M2 |
| missing-time-zone-definition | A `TZID` parameter named a zone with no `VTIMEZONE` in the same calendar. | Violation | M2 |
| ambiguous-local-time | A local time occurs twice under its zone, at the end of a daylight saving period. | Note | M2 |
| nonexistent-local-time | A local time does not occur under its zone, at the start of a daylight saving period. | Violation | M2 |
| time-zone-source-disagreement | An embedded `VTIMEZONE` and the caller's other zone source disagreed about an offset. | Note | M2 |
| vtimezone-without-observance | A `VTIMEZONE` carried neither a `STANDARD` nor a `DAYLIGHT` subcomponent, which RFC 5545 section 3.6.5 requires at least one of. | Violation | M2 |
| vtimezone-rule-unsupported | An observance carried an `RRULE` outside the yearly form this crate evaluates in closed form, so no transition was derived from it. | Note | M2 |
| vtimezone-observances-truncated | A `VTIMEZONE` declared more observances than the caller's policy admits, and the ones past the bound were dropped. | LimitReached | M2 |
| duplicate-time-zone-identifier | A calendar declared two `VTIMEZONE` components under one `TZID`, and the second was not admitted. | Violation | M2 |
| time-zone-coverage-exhausted | A zone was asked about a time later than the last transition it actually knows, so the answer continues its final observance. | Note | M2 |
| recurrence-until-not-utc | An `UNTIL` was written as a local time where RFC 5545 section 3.3.10 requires UTC, and it was read in `DTSTART`'s own zone. | Violation | M2 |
| exdate-value-type-mismatch | An `EXDATE` and its `DTSTART` disagreed about `DATE` versus `DATE-TIME`, which RFC 5545 section 3.8.5.1 requires to agree. | Violation | M2 |
| override-matches-no-instance | A `RECURRENCE-ID` named an instant the series does not generate, so the override modified nothing. | Violation | M2 |
| time-zone-without-transitions | A zone source recognized a `TZID` and holds no transition for it, so no wall clock names an instant under it. | Violation | M2 |
| time-zone-before-known-transitions | A zone was asked about a time earlier than the first transition it knows, so the answer continues the offset in force before it. | Note | M2 |
| vtimezone-components-truncated | A calendar declared more `VTIMEZONE` components than the caller's policy admits, and the ones past the bound were dropped. | LimitReached | M2 |
| vtimezone-observance-unreadable | An observance's required value was present and unreadable, so it stated no transition at all. | Violation | M2 |
| exdate-zone-unknown | An `EXDATE` written in UTC named no cadence key because no source recognized the series' zone, so it excluded nothing. | Violation | M2 |
| scheduling-method-unknown | A `METHOD` was present and named no method RFC 5546 defines, so the message states no scheduling semantics at all. | Violation | M3 |
| scheduling-calendar-address-unreadable | An `ORGANIZER` or `ATTENDEE` was present and its `CAL-ADDRESS` did not decode, so it identifies no party. | Violation | M3 |
| scheduling-sequence-unreadable | A `SEQUENCE` was present and was not an integer, so no revision ordering could be read from it. | Violation | M3 |
| scheduling-property-not-allowed | A scheduling payload carried a property RFC 5546 section 3 forbids for its `METHOD` and component type. | Violation | M3 |
| scheduling-required-property-missing | A scheduling payload lacked a property RFC 5546 section 3 requires for its `METHOD` and component type. | Violation | M3 |
| scheduling-cancellation-status-invalid | A `CANCEL` payload carried `STATUS` with a value other than `CANCELLED`. | Violation | M3 |
| scheduling-instance-ambiguous | A `RECURRENCE-ID` named a wall clock its series' zone repeats, and nothing said which of the two instants it addresses. | Violation | M3 |
| scheduling-range-not-permitted | A `RECURRENCE-ID` carried `RANGE=THISANDFUTURE` under a `METHOD` RFC 5546 does not permit it for. | Violation | M3 |
| scheduling-exclusion-unplaced | A scheduling message addressed a component carrying an exclusion no zone could place, so which instances it has is not decidable. | Violation | M3 |
| scheduling-zone-continued | An instance identity was resolved through a zone answer continued past one end of its source's transition table. | Note | M3 |
| scheduling-sender-not-permitted | A scheduling message was sent by a party RFC 5546 section 3 does not permit to send its `METHOD`. | Violation | M3 |
| scheduling-method-ambiguous | A calendar stated more than one `METHOD`, so the verb of the whole message is two claims rather than one. | Violation | M3 |
| scheduling-instance-nonexistent | A `RECURRENCE-ID` named a wall clock its series' zone does not show, and the reading the caller stated dropped it. | Violation | M3 |
| dav-foreign-element-skipped | An XML element outside the `DAV:` and CalDAV vocabulary was skipped, with everything inside it. | Note | M4 |
| dav-calendar-data-copied | A `calendar-data` payload had to be copied out of the body rather than borrowed from it. | Note | M4 |
| dav-calendar-data-line-endings-folded | A `calendar-data` payload lost carriage returns to XML 1.0 section 2.11 line-ending normalization. | Note | M4 |
| dav-property-unmodeled | A property was kept as octets because this crate has no model for its value. | Note | M4 |
| dav-status-unreadable | A `DAV:status` element did not carry the status line RFC 4918 section 14.28 requires. | Violation | M4 |
| dav-response-without-href | A `DAV:response` carried no `href`, so it names no resource. | Violation | M4 |
| dav-responses-truncated | A multistatus carried more responses than the caller's policy admits, and the ones past the bound were dropped. | LimitReached | M4 |
| dav-property-markup-dropped | A property mixed character data with elements, and the elements were not kept. | Violation | M4 |
| dav-sync-token-withheld | A synchronization token was withheld because the answer it arrived with was truncated. | LimitReached | M4 |
| query-calendar-data-reduced | A `calendar-data` selection returned a calendar that is not the one the server stored. | Note | M5 |
| query-filter-undecidable | A filter could not be decided, so the resource was neither matched nor excluded. | Note | M5 |
```
