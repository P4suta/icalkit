# Validation from published specifications

`Calendar::parse` and `Import::validate` use the same strict validation boundary.
`Import::read` keeps the admitted input bytes, including input that fails promotion.
The [calendar boundary](../crates/icalkit/src/calendar.rs) combines the private schema audit with constraints that depend on surrounding components and related properties.

The component hierarchy follows [RFC 5545 section 3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6).
A calendar requires a component, and a time zone requires a standard or daylight observance.
Conditional start requirements cover events without `METHOD`, recurring components with `RRULE`, and tasks with `DURATION`.
Unrecognized components keep their content without applying known component semantics inside them.

Component, property, and parameter names follow the [generic token grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.1).
Strict promotion checks names throughout unknown component subtrees.
Valid extension names keep their spelling, including folded names.
Lossless imports preserve malformed names without granting validated model views.

[Value type declarations](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20) name one nonempty token formed from ASCII letters, digits, and hyphens.
Repeated or malformed declarations produce a violation during strict promotion.
Valid unknown tokens remain opaque and produce notes.
Typed accessors refuse malformed declarations before decoding property data.

The URI codec checks the [generic component grammar](https://www.rfc-editor.org/rfc/rfc3986.html#section-3) for reading and writing.
Authority checks cover user information, registered hosts, address literals, and decimal ports.
Paths, queries, and fragments use their defined character sets.
Address literals use the URI syntax restored by [RFC 9844](https://www.rfc-editor.org/rfc/rfc9844.html#section-3).
Valid spellings keep their bytes, and lossless storage preserves malformed URI text without granting a typed URI view.

Strict promotion validates `URL`, `TZURL`, `ORGANIZER`, `ATTENDEE`, and `ATTACH` through the shared URI and binary codecs.
[Calendar user addresses](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.3) use the `CAL-ADDRESS` value type.
[Attachments](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.1) use URI references or explicit binary values with base64 encoding.
Attachment checks reject repeated `VALUE`, `ENCODING`, and `FMTTYPE` parameters.
Media types remain optional, and unknown parameters keep their bytes.
Unknown value types stay opaque during promotion.

Scalar `ATTENDEE` controls follow the [parameter grammars](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2).
`RSVP` accepts `TRUE` or `FALSE`.
`CUTYPE`, `ROLE`, and `PARTSTAT` use unquoted ASCII tokens.
The [attendee definition](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.1) permits each control at most once on `VEVENT`, `VTODO`, and `VJOURNAL`.
Registered participation statuses follow their component domains, including task `FAILED` from [RFC 10051 section 9.1](https://www.rfc-editor.org/rfc/rfc10051.html#section-9.1).
Valid extension tokens, unknown parameters, unknown value types, and unknown components keep their bytes.

Participant address parameters follow the [quoted URI grammars](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.4).
`DIR` and `SENT-BY` take single quoted values.
`MEMBER`, `DELEGATED-FROM`, and `DELEGATED-TO` take lists of quoted values.
Every URI must parse, and each parameter occurs at most once within its permitted component domain.
Agent addresses follow [RFC 6068 syntax](https://www.rfc-editor.org/rfc/rfc6068.html#section-2), including [verified erratum 7919](https://errata.rfc-editor.org/eid7919/).
The [agent address decision](adr/mailto-agent-address-validation.md) records the syntax boundary and source interpretation.
Lossless imports keep invalid metadata available for inspection.

Known text properties follow the [text grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11) after content-line unfolding.
Strict promotion rejects undefined escapes, dangling backslashes, unescaped delimiters, invalid UTF-8, and incompatible or repeated `VALUE` declarations.
`CATEGORIES` and `RESOURCES` accept comma separators under their property definitions.
Calendar metadata and time zone text use the same checks.
Unknown value types stay opaque, and the lossless text view keeps its existing behavior.

Structured `REQUEST-STATUS` values follow [RFC 5545 section 3.8.8.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.8.3).
Codes contain two or three numeric parts without a machine integer width limit.
Descriptions and optional exception fields use scalar `TEXT` syntax.
The calendar boundary enforces the text value type and at most one `LANGUAGE` parameter.
[CalDAV request status values](https://www.rfc-editor.org/rfc/rfc6638.html#section-10.4) use the same private parser during construction and XML reading.
Delivery success follows numeric class 2, including spellings with leading zeros.
Descriptions keep trailing whitespace through XML responses.
Unknown value types and unknown component semantics stay opaque during calendar promotion.

Enumerated text uses the vocabulary defined for each property and component.
Task statuses include `PENDING` and `FAILED` from [RFC 10051](https://www.rfc-editor.org/rfc/rfc10051.html#section-11.2).
The [status registry](https://www.iana.org/assignments/icalendar/icalendar.xhtml#status-values) records the supported additions.
Transparency and calendar scale use their defined values.
Classifications, methods, and `ACTION` values accept extension tokens under their grammars.
Name validation and value type declarations share the same ASCII token rule.
Enumerated values match without changing ASCII case or stored bytes.

Integer syntax follows [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.8).
`PRIORITY` ranges from [0 through 9](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.9).
`PERCENT-COMPLETE` ranges from [0 through 100](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.8).
The nonnegative `SEQUENCE` domain follows the [counter initialized at zero](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.7.4) and agrees with the scheduling reader.
Known numeric values reject incompatible or repeated `VALUE` declarations.
Unknown value types stay opaque, and successful validation preserves the original bytes.

Geographic positions follow the [coordinate bounds and decimal pair syntax](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.6).
Strict promotion uses the original decimal text to check latitude and longitude bounds.
The checks preserve decimal precision and unknown parameters.
Unknown value types stay opaque, and incompatible or repeated `VALUE` declarations fail promotion.

Observance offsets follow the [UTC offset grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.14).
Strict promotion validates both `TZOFFSETFROM` and `TZOFFSETTO`.
The shared decoder rejects negative zero and seconds fields of 60.
Unknown value types stay opaque during promotion and can't define inline query transitions.

Date-time checks enforce the declared `VALUE` type and each property's UTC form.
The `TZID` restrictions follow [RFC 5545 section 3.2.19](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.19).
Related endpoints and recurrence identifiers must agree with the start's value type and floating form.
The parse boundary checks endpoint order for dates, floating values, and UTC values.
Zoned values keep their identifiers for workflows that use the caller's `ZoneDatabase`.
Event and task durations must be positive, and date durations use day or week syntax.

The `VALARM` checks follow [RFC 5545 section 3.6.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.6) and [RFC 9074](https://www.rfc-editor.org/rfc/rfc9074.html).
They enforce the properties required by each action, paired repeat intervals and counts, trigger anchors, identifiers, and acknowledgement timestamps.
Unknown actions and value types keep their opaque data.

Recurrence rules use the maintained parser and the shared session meter.
Bounds follow the `DTSTART` conditions in [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
Floating starts follow the explicit condition that both times use local time.
Observance bounds use UTC.
Standalone timestamp searches require UTC date-time bounds.
Unknown `VALUE` types stop typed decoding and produce notes, as required by [section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).

Calendar controls follow [RFC 7529](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1).
The native engine admits the [Gregorian names](https://raw.githubusercontent.com/unicode-org/cldr/main/common/bcp47/calendar.xml) with absent or `OMIT` skipping.
Other calendars, other `SKIP` modes, and leap month selectors produce `unsupported-recurrence-semantics`.
Malformed or repeated controls fail typed construction, including `SKIP` without `RSCALE`.
Both typed decoders resolve `FREQ` independent of written order, following [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
Public rule errors keep the kernel diagnostics.
Standalone `Rule::parse` uses the inclusive value byte ceiling from the [shared resource policy](adr/0010-shared-resource-limits.md).
It charges admitted input bytes to the secure meter before parsing and refuses larger values with `icalkit.recurrence.resource-limit`.
The allocation gate bounds heap use for resource refusals independently of input size.
Imports preserve original bytes, and unrelated unknown parts keep their notes.
The [calendar control decision](adr/recurrence-calendar-controls-before-expansion.md) records the native subset and compatibility policy.
Calendar conversion, other `SKIP` modes, and CalDAV capability negotiation remain open.

`RDATE` and `EXDATE` lists follow [RFC 5545 section 3.8.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5).
Each field must match its declared type.
Period checks validate the bounds and positive lengths that [section 3.3.9](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.9) requires.
`FREEBUSY` bounds use UTC under [section 3.8.2.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.2.6).
List decoding keeps the property's parameters and refuses to interpret unknown value types during recurrence expansion.
Free/busy aggregation reports unknown value types as unreadable, and projection keeps their original content.

Recurrence lengths follow [RFC 5545 section 3.8.5.3](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5.3).
Period inclusions replace rule lengths when their start times coincide, following [section 5](https://www.rfc-editor.org/rfc/rfc5545.html#section-5).
Search bounds account for long instances that begin before the requested range.
Conflicting period lengths fail explicitly.
Expanded tasks keep `DUE` or `DURATION` because their overlap boundaries differ under [RFC 4791 section 9.9](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.9).
Journals keep their point or date form, and instantaneous events omit an end property.
Filtering and recurrence limiting use the same component overlap table.

Query grouping compares decoded scalar `UID` text under [RFC 5545 section 3.8.4.7](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.7).
Equivalent text escapes identify one recurrence set without changing stored bytes.
An unreadable identifier leaves a required relationship undecided, and expansion refuses an incomplete result.
Recurrence limiting keeps unreadable overrides through its conservative retention policy.
Known distinct identifiers keep independent occurrences.
Recurrence limiting uses original period lengths under [RFC 4791 section 9.6.6](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6.6).
Project policy keeps exceptions whose original length is unreadable.
Queries combine time ranges and property filters for detached components without a master.
Query time zones apply to both filtering and projection.
The [duration decision](adr/occurrence-duration-semantics.md) records the distinction between calendar days and elapsed time.

Range changes follow recurrence identity under [RFC 5545 section 3.8.4.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4).
The same shifts reach rule instances and `RDATE` additions.
Exact exception components keep their own starts and changes without inheriting earlier range changes.
The [range decision](adr/range-changes-by-recurrence-identity.md) revises the relevant behavior in the historical recurrence decision.

Expanded calendar data follows [RFC 4791 section 9.6.5](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6.5).
Single instances use the component overlap rules.
The rules cover tasks without a start and detached exceptions without a master.
Zoned local values become UTC throughout nested components.
Durations of single instances become elapsed time before their zoned starts become UTC.
Expansion removes time zone components and recurrence properties, including `EXRULE`.
Conversion preserves unknown parameters and rejects values whose type doesn't identify a supported date-time.
Reconstructed temporal properties keep the extension parameters stated by the applicable master or exception.
Expanded recurrence identifiers omit `RANGE`, because each returned component describes one instance.
Non-standard properties default to text under [RFC 5545 section 3.8.8.2](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.8.2).
An exclusion can remove `DTSTART` without a recurrence rule.

Query time zone definitions follow [RFC 4791 section 9.8](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.8).
Dates and floating values use the query's bounded transition definition.
Values with an explicit `TZID` use the caller's `ZoneDatabase`.
Query parsing rejects malformed definitions, unsupported observance rules, and opaque transition values.
Opaque metadata stays uninterpreted.
The observance reader guards declared value types for transition dates and recurrence rules.
Known observance `RDATE` values use local date-time form under [RFC 5545 section 3.6.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.5).
The [query zone decision](adr/query-timezone-definitions.md) records source selection and supported-definition boundaries.

Scheduling reads `METHOD` and `UID` through their declared `TEXT` type.
Identifier comparison decodes [text escapes](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11) once and keeps source bytes.
Participant addresses, revisions, timestamps, and recurrence identifiers use their declared value types.
Unknown or incompatible party values stay present as unreadable values for the existing diagnostic path.
Review refuses held scheduling components with an unreadable `UID`, following the [scheduling identity policy](adr/0005-scheduling-apart-from-the-model.md).
Review refuses a known `UID` associated with a different component kind.
Calendars and imports keep opaque property values under [RFC 5545 section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).

`EventRef::uid` returns decoded scalar text, or `None` for an unreadable identity.
Ordinary identifiers borrow source storage.
Editing and stored recurrence selection use the same decoded value.
Editing refuses more than one matching event or unreadable event identities before changing the working copy.
Stored recurrence selection refuses unreadable master identities and preserves its ambiguity check.
The [identifier decision](adr/decoded-optional-calendar-identifiers.md) records the return type change before publication.

Scheduling creation starts with `Message::review_creation` or `Message::review_creation_in` when the destination is absent.
The caller must enforce absence when storing the authorized result.
The [creation decision](adr/scheduling-creation-from-absence.md) records the API choice and its storage boundary.
Cancellation preserves stored properties omitted by the notices defined in [RFC 5546 section 3.2.5](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.2.5).

The specification cases live in `crates/icalkit-conformance/tests/`.
The creation workflows live in `crates/icalkit/tests/scheduling_creation.rs`.
Use the [contribution workflow](../CONTRIBUTING.md) to reproduce a failure before changing behavior and run the applicable checks after the fix.

```sh
mise x -- cargo nextest run --locked -p icalkit-conformance --test calendar_validation --test alarm_validation --test component_structure --test opaque_value_types --test recurrence_validation --test temporal_list_validation --test occurrence_durations --test recurrence_ranges --test calendar_expansion --test query_timezones
mise x -- cargo nextest run --locked -p icalkit --test scheduling_creation
mise x -- just ci
```

Published specifications define the validation behavior.
The [capture intake](../crates/icalkit-conformance/CAPTURE.md) describes the separate observations needed for claims about calendar producers.
