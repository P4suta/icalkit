# Validation from published specifications

`Calendar::parse` and `Import::validate` use the same strict validation boundary.
`Import::read` keeps the admitted input bytes, including input that fails promotion.
The [calendar boundary](../crates/icalkit/src/calendar.rs) combines the private schema audit with constraints that depend on surrounding components and related properties.

The component hierarchy follows [RFC 5545 section 3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6).
A calendar requires a component, and a time zone requires a standard or daylight observance.
Conditional start requirements cover events without `METHOD`, recurring components with `RRULE`, and tasks with `DURATION`.
Unrecognized components keep their content without applying known component semantics inside them.

Integer syntax follows [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.8).
`PRIORITY` ranges from [0 through 9](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.9).
`PERCENT-COMPLETE` ranges from [0 through 100](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.8).
The nonnegative `SEQUENCE` domain follows the [counter initialized at zero](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.7.4) and agrees with the scheduling reader.
Known numeric values reject incompatible or repeated `VALUE` declarations.
Unknown value types stay opaque, and successful validation preserves the original bytes.

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
