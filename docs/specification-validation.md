# Validation from published specifications

`Calendar::parse` and `Import::validate` use the same strict validation boundary.
`Import::read` keeps the admitted input bytes, including input that fails promotion.
The [calendar boundary](../crates/icalkit/src/calendar.rs) combines the private schema audit with constraints that depend on surrounding components and related properties.

The component hierarchy follows [RFC 5545 section 3.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6).
A calendar requires a component, and a time zone requires a standard or daylight observance.
Conditional start requirements cover events without `METHOD`, recurring components with `RRULE`, and tasks with `DURATION`.
Unrecognized components keep their content without applying known component semantics inside them.

Calendar format declarations follow the [version property](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.7.4).
Interpreted values must name `2.0` or the supported range `2.0;2.0`.
Other interpreted values and incompatible recognized value types fail promotion.
Unknown value types keep their opaque content under the existing declaration contract.
The [format version decision](adr/calendar-format-version-validation.md) separates calendar formats from package releases.
Supported spellings, folds, and extension parameters keep their original bytes through editing and scheduling reads.

Component, property, and parameter names follow the [generic token grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.1).
Strict promotion checks names throughout unknown component subtrees.
Valid extension names keep their spelling, including folded names.
Lossless imports preserve malformed names without granting validated model views.

Relationship references follow [RFC 9253 section 9.1](https://www.rfc-editor.org/rfc/rfc9253.html#section-9.1) across known components.
Strict promotion checks scalar text identifiers, URI references, relationship tokens, and signed duration gaps.
Hierarchical, default, unknown, and snooze relationships reference identifiers.
The [relationship decision](adr/relationship-reference-validation.md) records the interpretation of legacy text forms and the bounded duration codec.
Parameters compare without ASCII case changes to stored bytes, and unknown value types keep opaque semantics.
Callers maintain reference targets and relationship effects.

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
`Actor::new` applies the same URI syntax without changing the address spelling.
[Attachments](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.1.1) use URI references or explicit binary values with base64 encoding.
Attachment checks reject repeated `VALUE`, `ENCODING`, and `FMTTYPE` parameters.
Media types remain optional, and unknown parameters keep their bytes.
Unknown value types stay opaque during promotion.

Encoding declarations follow the [token grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.7).
`ENCODING` accepts unquoted `8BIT` for URI data and `BASE64` for explicit binary data.

Attachment formats follow the [type/subtype grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.8).
Strict promotion checks the permitted characters and the limit of 127 characters per name.
The [format syntax decision](adr/attachment-format-parameter-syntax.md) records the protocol's referenced grammar and the scope of current registration rules.
Valid formats keep their case and non-standard names.

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

Common names follow the [single parameter value grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.2).
Strict promotion checks UTF-8, quoting, cardinality, and participant placement while keeping empty names and caret escapes.
`ATTENDEE` permits `LANGUAGE` without `CN` under the [attendee definition](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.1).
The language syntax checks also apply in `VFREEBUSY` and `VALARM`.
Known textual properties and participant names use the [language tag grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.10).
Grandfathered tags and private use keep their spelling.
Strict promotion rejects repeated variants and extension singletons without regard to case.
Tag scans and repeated variant comparisons use the shared resource budget.
The [language syntax decision](adr/language-parameter-syntax.md) distinguishes syntax acceptance from dated registry validity and locale matching.
Unknown properties, value types, and component semantics keep their metadata.

Known text properties follow the [text grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.11) after content-line unfolding.
Strict promotion rejects undefined escapes, dangling backslashes, unescaped delimiters, invalid UTF-8, and incompatible or repeated `VALUE` declarations.
`CATEGORIES` and `RESOURCES` accept comma separators under their property definitions.
Calendar metadata and time zone text use the same checks.
Unknown value types stay opaque, and the lossless text view keeps its existing behavior.

Alternate text representations follow the [quoted URI rule](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.1).
`COMMENT`, `CONTACT`, `DESCRIPTION`, `LOCATION`, `RESOURCES`, and `SUMMARY` permit at most one `ALTREP` parameter.
The shared codec preserves each valid URI's spelling.
Commas inside the quoted URI don't use `TEXT` escaping, following [verified erratum 7691](https://www.rfc-editor.org/errata/eid7691/).

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
An hour and second pair requires a minute term, including zero minutes.

The `VALARM` checks follow [RFC 5545 section 3.6.6](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.6.6) and [RFC 9074](https://www.rfc-editor.org/rfc/rfc9074.html).
They enforce the properties required by each action, paired repeat intervals and counts, trigger anchors, identifiers, and acknowledgement timestamps.
Trigger anchors use one unquoted `START` or `END` token under the [`RELATED` parameter grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.14).
Valid casing, line folds, and extension parameters keep their input bytes.
Unknown actions and value types keep their opaque data.

Free busy classifications follow the [token grammar](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.9).
`FREEBUSY` permits at most one unquoted ASCII token for `FBTYPE`.
Valid extension tokens keep their spelling and follow the specified `BUSY` fallback.
Unknown value types keep opaque metadata.

Recurrence identifier ranges follow [RFC 5545 section 3.2.13](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.13).
A `DATE` or `DATE-TIME` recurrence identifier permits one unquoted `RANGE=THISANDFUTURE` parameter.
Strict promotion rejects unsupported range values and repeated parameters before expansion or scheduling reads the scope.
Lossless imports preserve malformed range data.
Unknown property, value type, and component semantics stay opaque.

Embedded `VTIMEZONE` rules follow [verified erratum 1913](https://errata.rfc-editor.org/eid1913) for weekday ordinal scope.
A numeric `BYDAY` counts within the stated month when `BYMONTH` is present.
Without `BYMONTH`, its ordinal counts from the start or end of the year.
Nonexistent ordinal positions produce no computed date.
The [weekday ordinal decision](adr/weekday-ordinals-in-embedded-time-zones.md) records bounded arithmetic and observance boundaries.

Embedded scalar `BYMONTHDAY` rules accept negative positions from `-31` through `-1`.
Each position counts backward from the month's final day.
Positions missing from a short month produce no computed date.
The [month-end position decision](adr/month-days-counted-from-the-end.md) records checked subtraction and the annual lookup boundary.

Embedded rules with `BYMONTH` preserve the intersection of `BYDAY` with supported seven-day `BYMONTHDAY` windows.
Positive windows keep their lower bound, so February can have no matching transition when the stated days run from 25 through 31.
The negative window from `-7` through `-1` instead selects the month's last matching weekday.
The [month-day intersection decision](adr/month-day-intersections-in-embedded-time-zones.md) records the arithmetic and short-month behavior.

Without `BYMONTH`, supported `BYMONTHDAY` selectors expand across all twelve months.
The reader compiles month-specific annual patterns with the original anchor, offsets, clock, and inclusive end bound.
The reader stores each explicit date once.
Table admission charges every stored pattern to the existing observance meter.
Explicit month selectors, default anniversaries, and yearly weekday ordinals keep their scope.
The [month expansion decision](adr/month-day-selectors-across-all-months.md) records the bounded compilation and its storage cost.

Embedded `BYMONTH` lists select nonempty sets of months from 1 through 12.
Repeated values within a list compile once without changing the stored source spelling.
Explicit month sets keep numeric weekday positions within each month, including a set containing all twelve months.
Malformed values, empty entries, and repeated month parts produce an unsupported-rule diagnostic.
The [month set decision](adr/selected-month-sets-as-annual-patterns.md) records checked parsing and annual compilation.

Embedded `BYMONTHDAY` lists combine positive and negative positions in the selected months.
`BYDAY` lists can intersect the resulting dates.
Repeated values and coincident positive and negative positions produce one computed onset per date.
Missing dates produce no computed onset, while the original anchor remains available.
The reader retains compact scalar and seven-day window patterns and compiles other sets into at most 366 annual patterns.
Every stored pattern consumes the existing observance budget.
Malformed lists and repeated month-day parts produce an unsupported-rule diagnostic.
The [month-day set decision](adr/month-day-sets-as-ranked-annual-patterns.md) records rank selection, duplicate handling, and storage bounds.

Embedded yearly `BYDAY` lists combine nonnumeric weekdays and signed positions from `-53` through `-1` or `1` through `53`.
Numeric positions keep their month or year scope before intersection with `BYMONTHDAY`.
The date filter doesn't renumber weekday positions.
Repeated terms and coincident dates produce one computed onset, and a nonnumeric weekday includes its numbered forms.
Malformed lists and repeated weekday parts produce an unsupported-rule diagnostic.
General date patterns compile into fourteen Gregorian year layouts while compact single patterns remain available.
Each compiled mask retains its month binding.
The [weekday set decision](adr/weekday-sets-as-calendar-year-masks.md) records compilation, lookup, and verification boundaries.

Embedded yearly `BYYEARDAY` lists select signed positions from `-366` through `-1` or `1` through `366`.
Year-day, month, month-day, and weekday constraints intersect before `BYSETPOS` selects from the complete year's candidates.
Set positions use the same signed range and require another supported date selector.
Positions count distinct existing dates after filtering and before anchor or inclusive end clipping.
Without an explicit day selector, selected months use the anchor's day and exclude missing dates before counting positions.
Explicit dates remain separate from recurrence position selection.
Malformed lists and repeated year-day or set-position parts produce an unsupported-rule diagnostic while preserving the anchor.
Signed positions accept one to three digits without changing the original source spelling.
Compiled patterns keep month bindings, clocks, offsets, and resource charges.
Week numbers, nonannual frequencies, intervals greater than one, and count limits remain outside the embedded compiler's supported subset.
The [annual selection decision](adr/annual-selection-before-month-patterns.md) records full-year compilation and bounded lookup.

Embedded recurrence rules require unique part names under [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
The reader accepts supported parts in any order.
Part names and enumerated values ignore ASCII case.
`INTERVAL=1` accepts leading zeros without changing yearly spacing.
`WKST` requires one weekday token even when annual patterns don't use its value.
Observance `UNTIL` requires a UTC `DATE-TIME` before conversion through `TZOFFSETFROM` to an inclusive wall date.
Malformed or repeated controls refuse the whole rule and preserve its anchor and explicit dates.

Recurrence rules use the maintained parser and the shared session meter.
Numeric selectors enforce the digit limits in [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
Clock fields, month numbers, month days, week numbers, and weekday ordinals use one or two digits.
Year days and set positions use one to three digits.
Ordinal signs and leap month suffixes follow their own grammar and don't count as digits.
Strict reading distinguishes malformed numeric spellings from values outside their calendar's range.
Recovering reading drops the complete unusable list part and preserves its established diagnostic.
Legal zero padding survives in the original calendar bytes.
`COUNT` and `INTERVAL` keep their variable digit counts within the existing input and numeric limits.

Week-numbering years follow `WKST` and the [four-day rule](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
Their candidate extent can begin before or after January 1.
Search bounds use the generation date extent while shorter periods keep their clock precision.
Completed `COUNT` limits stop before another period spends resources.
The [search bound decision](adr/recurrence-week-year-search-bounds.md) records the shared boundary and continuation requirements.
Bounds follow the `DTSTART` conditions in [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
Floating starts follow the explicit condition that both times use local time.
Observance bounds use UTC.
Standalone timestamp searches require UTC date-time bounds.
Unknown `VALUE` types stop typed decoding and produce notes, as required by [section 3.2.20](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.2.20).

Calendar controls follow [RFC 7529](https://www.rfc-editor.org/rfc/rfc7529.html#section-4.1).
The native engine admits the sixteen registered calendar algorithms supported by the conversion adapter, including their [registered aliases](https://raw.githubusercontent.com/unicode-org/cldr/main/common/bcp47/calendar.xml).
Numeric selector limits follow the selected calendar, including Hebrew year days up to 385 and Ethiopic month 13.
Regular and leap month selectors preserve their distinct identities.
`BACKWARD` and `FORWARD` handle missing months, default month days, `BYMONTHDAY`, and `BYYEARDAY`.
The recurrence engine corrects missing months before missing days.
Weekday, clock, and period position selection follow the corrections.
Year day and month day constraints intersect before correction.
Year day candidate bounds follow the corrected date when a missing day crosses a year boundary.
The search orders and deduplicates overlapping period candidates before counting.
Continuation preserves pending candidates and unconsumed rule offers beside `RDATE` additions.
Ambiguous calendar algorithms, unknown calendars, and non-`OMIT` rules with `BYWEEKNO` produce `unsupported-recurrence-semantics`.
Malformed or repeated controls fail typed construction, including `SKIP` without `RSCALE`.
Both typed decoders resolve `FREQ` independent of written order, following [RFC 5545 section 3.3.10](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10).
Public rule errors keep the kernel diagnostics.
Standalone `Rule::parse` uses the inclusive value byte ceiling from the [shared resource policy](adr/0010-shared-resource-limits.md).
It charges admitted input bytes to the secure meter before parsing and refuses larger values with `icalkit.recurrence.resource-limit`.
The allocation gate bounds heap use for resource refusals independently of input size.
Imports preserve original bytes, and unrelated unknown parts keep their notes.
The [calendar control decision](adr/recurrence-calendar-controls-before-expansion.md) records the native subset and compatibility policy.
The [month day decision](adr/gregorian-month-day-adjustments.md) extends the native subset.
The [year day decision](adr/gregorian-year-day-correction-order.md) records the source constraints and shared date arithmetic.
The [calendar selection decision](adr/recurrence-selection-in-calendar-coordinates.md) extends the shared selector pipeline across registered calendars.
Week number correction remains open.

Calendar date conversion follows [RFC 7529 section 3](https://www.rfc-editor.org/rfc/rfc7529.html#section-3).
`recurrence::CalendarScale` selects a registered calendar algorithm through explicit names and aliases.
`recurrence::CalendarMonth` distinguishes regular month numbers from leap months without using their ordinal position in a year.
`recurrence::CalendarDate` converts between validated calendar coordinates and Jiff Gregorian dates.
The adapter uses [ICU4X 2.4](https://docs.rs/icu_calendar/2.4.0/icu_calendar/) privately and requires Rust 1.88.
Calendar years use ICU4X extended year numbering consistently in both directions.
Chinese years use the related Gregorian year, so the RFC example's Chinese year 4650 corresponds to extended year 2013.
Gregorian results must fit years 0000 through 9999, while other calendar year numbers can exceed 9999.
Missing months and days fail without clamping.
The adapter refuses the unqualified `ISLAMIC` and observational `ISLAMIC-RGSA` names because they lack an unambiguous mapping.
Explicit `ISLAMIC-CIVIL`, `ISLAMIC-TBLA`, and `ISLAMIC-UMALQURA` algorithms remain distinct.
The [conversion decision](adr/calendar-conversion-at-the-jiff-boundary.md) records the dependency and compiler boundary.
Recurrence selection uses calendar fields and spans from the same conversion adapter.
Year metadata has a fixed four-entry cache, and Gregorian fields use native civil arithmetic.
Gregorian month fields use civil coordinates without computing unused spans.
Calendar arithmetic failures remain distinct from exhausted resource budgets and complete search outcomes.

The private period walker uses the selected calendar for monthly and yearly anchors.
Resumed indices derive their first anchor from normalized coordinates, while sequential walks reuse calendar boundaries.
Both paths preserve leap month progression and interval spacing.
The [period arithmetic decision](adr/calendar-coordinates-for-recurrence-periods.md) records shared calendar coordinates and Gregorian range boundaries.
Recurrence identities, additions, exclusions, and detached override keys remain Gregorian.

CalDAV recurrence scale negotiation follows [RFC 7529 section 10](https://www.rfc-editor.org/rfc/rfc7529.html#section-10).
`Client::supported_rscales` explicitly requests `CALDAV:supported-rscale-set` at Depth 0 from a calendar collection or calendar home.
An absent advertisement differs from an advertised set without calendar names.
Named sets keep unknown names and compare supported names without case sensitivity.
Calendar name comparison resolves the aliases supported by the conversion adapter.
`Client::conditional_write` refuses interpreted `RSCALE` rules without negotiation.
`Client::conditional_write_with_rscales` probes the associated collection or home URI selected by the caller before offering the conditional `PUT`.
The request keeps the original calendar bytes and revision headers.
Unknown value types and unknown components remain opaque.
The client requires a single resource response and complete, valid property content within shared resource budgets.
Project policy uses property status `200 OK` to establish support, following the successful property status described in [RFC 4918 section 9.1.2](https://www.rfc-editor.org/rfc/rfc4918.html#section-9.1.2).
Other status codes never release a scale-dependent write.
Servers advertise support through explicit `Discovery::with_supported_rscales` configuration.
The protected property appears in explicit property requests and `allprop` requests with an explicit `include` selection.
Ordinary `allprop` responses omit it.
Unsupported calendar names receive `403` with the `CALDAV:supported-rscale` precondition after authorization, without a storage request.
Rules in supported calendars reach storage after authorization, and negotiated writes preserve the original bytes and revision headers.
The [advertisement decision](adr/advertised-recurrence-scales-before-caldav-writes.md) records the caller and transport boundaries.

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
Scheduling promotion requires the full inspection to finish within the session's resource budget.
An inspection that crosses a limit can't produce a validated message, even when it hasn't yet reported a property violation.

Outbox POST follows the busy time request contract in [RFC 6638 section 5](https://www.rfc-editor.org/rfc/rfc6638.html#section-5).
Client and server require one `VFREEBUSY` with `METHOD:REQUEST` under [RFC 5546 section 3.3.2](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.3.2).
Required timestamps and participant addresses must have readable declared value types.
Delivery uses the body's organizer and distinct attendee addresses.
URI spelling and commas remain unchanged.
Custom routing headers can't replace the message's participants.
Authorization receives the validated message and addresses for caller checks of the principal, Outbox collection, and privileges.
The [Outbox decision](adr/outbox-busy-time-from-message.md) records the API change and the media `method` rule.

Successful Outbox outcomes include `CALDAV:calendar-data` under [RFC 6638 section 5](https://www.rfc-editor.org/rfc/rfc6638.html#section-5).
Data must be a readable `VFREEBUSY` reply under [RFC 5546 section 3.3.3](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.3.3).
Client and server check complete recipient coverage, decoded identifiers, and matching participant addresses.
The XML reader validates the declared media type, version, and supplied scheduling parameters.
Calendar data follows [RFC 4791 section 9.6](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6), including LF lines, `CDATA` sections, and character references.
Validation converts transport LF lines to CRLF, while a separate byte view keeps the original calendar octets.
XML fields, reply parsing, and response counts share one resource budget.
HTTP 204 has an explicit outcome without recipient data under [RFC 6638 section 5.1](https://www.rfc-editor.org/rfc/rfc6638.html#section-5.1).
The [response data decision](adr/outbox-busy-time-response-data.md) records the typed view and byte preservation contract.

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
