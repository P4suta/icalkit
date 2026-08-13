# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing has been released. This section is everything since the first commit — the workspace
bootstrap and the five milestones over it — and the entries in each group run oldest first,
because several of them are corrections to entries above them and read as nothing at all in
the other order. What version this becomes is a decision nobody has made.

### Added

- Workspace bootstrap: crate skeletons, quality gates, and the day-one architectural
  decision records — the frame, written before anything it was built to hold. Everything
  under it is in the entries below.
- Five architectural decisions from the design bake-off — allocation policy, parser layering
  and the pull API, the error and diagnostic model, shared resource limits, and civil-time
  arithmetic — and a per-crate design document for each crate's committed public surface.
- `ical-grammar`, holding the content line grammar and the diagnostic vocabulary below
  `ical-core`, so a linter or a fuzz harness never compiles the typed model.
- The content line layer, and the model on top of it: `ContentLineReader` unfolds and lexes in
  one pass, `Document::parse` builds a tree from that same public token path, and
  `Document::serialize` writes it back octet for octet — folds where the producer put them,
  the terminator it chose, parameters in the order and case it wrote them. Around that, the
  checked civil arithmetic, a decoder for every value type of RFC 5545 sections 3.3.4 through
  3.3.14, the typed accessors as views over preserved text, and scoped mutation through
  `PropertyMut`, which is the only way to write and which discards exactly one line's layout.
  A structural violation is a diagnostic and never costs an octet; a caller-stated bound is a
  refusal and never a truncation.
- The rest of RFC 5545 section 3.3: `FLOAT`, `BINARY`, `URI`/`CAL-ADDRESS` and `PERIOD` decode,
  and the last three encode with the parameters their own shape implies — `VALUE=BINARY` with
  `ENCODING=BASE64`, and a `TZID` applied to both bounds of a period. A `DateTimeValue::Zoned`
  variant means a date-time can no longer be constructed apart from the parameter set it
  implies, which is what ADR 0001 asked for.
- Section 3.6 as a reading rather than as storage: `ComponentKind::cardinality` for the nine
  components, an advisory `Component::audit` that reports a missing, duplicated or misplaced
  property and a `VALUE` naming a type nobody defines, and seventeen typed accessors.
- RFC 6868's caret encoding in both directions (`decode_caret`, `encode_caret`), as a codec a
  caller opts into: storage still keeps the octets a producer wrote, so a `DQUOTE` written `^'`
  stays `^'` on the wire and is a `"` only in the decoded view.
- Five diagnostics the workspace defined and nothing reported now have emitters: a physical
  line over 75 octets, a control character in a name, a parameter name or a value, a parameter
  that arrived with no value, an unterminated quoted parameter, and an RFC 6868 pair nothing
  defines. Each names the octets it is about, and none of them charges the meter.
- `docs/diagnostic-codes.md`, the golden list ADR 0009 requires, and `just codes`, which fails
  unless every declared code carries a row whose meaning is the variant's own first paragraph
  byte for byte. A meaning can now only be edited by editing both files, which is the review a
  frozen meaning is owed.
- Conformance cases for the two readings this workspace had to choose — where a `DQUOTE`
  delimits a parameter value, and what an undefined caret pair means — and `sweep.rs`, the
  adversarial evidence M0-alpha reported and never committed, as a seeded, deterministic,
  time-bounded sweep that accepts a refusal only where the input itself confirms the bound.
- `DiagnosticCode::MutuallyExclusiveProperties`, and the entailment half of the section 3.6
  reading that ADR 0001 describes: `Component::audit` now reports `DTEND` against `DURATION` in
  a `VEVENT` and `DUE` against `DURATION` in a `VTODO`. `schema.rs` used to defer that pair onto
  an audit that did not make the claim; the two entailments that turn on a value rather than on
  a pair of names are recorded in that ADR as deferred rather than deferred onto anything.
- Four adversarial passes against this milestone, landed as conformance cases addressed to the
  sections they come from: `break_debts.rs`, `break_values.rs`, `break_components.rs` and
  `break_sweeps.rs`, the last of which attacks `sweep.rs` as an artifact rather than using it
  as one.
- `ical-recur` expands. `parse_recur` reads a `RECUR` value leniently — a part it cannot use is
  dropped and reported, never clamped, so one producer's `BYMONTHDAY=32` does not cost a series
  its `FREQ` — and `RecurrenceInput::search` walks one period per `FREQ` step, applies every
  `BYxxx` part through RFC 5545 section 3.3.10's own expand/limit table, selects with
  `BYSETPOS` over the closed period, and merges `RDATE`, `EXDATE` and `RECURRENCE-ID` overrides
  in one forward pass that materializes nothing. `SearchCursor` resumes a `COUNT`-bounded
  search into a later window and reproduces the recurrence set the file describes.
- All forty-two worked examples of RFC 5545 section 3.8.5.3, as a table test in `ical-conform`
  assembled through the public surface only, with the expected column transcribed from the RFC
  rather than read off the implementation. One of them — "Every other year on January,
  February, and March for 10 occurrences" — caught the omission that a recurrence set begins at
  `DTSTART`: the period holding `DTSTART` is expanded whole and offers candidates before it,
  and counting one against `COUNT` ends the series an instance early.
- Six diagnostic codes for the recurrence layer and their golden-list rows:
  `malformed-recurrence-rule`, `duplicate-recurrence-rule-part`, `unknown-recurrence-rule-part`,
  `recurrence-rule-part-out-of-range`, `mutually-exclusive-rule-parts` for a value
  carrying both `UNTIL` and `COUNT`, and `override-shift-not-representable` for an override that
  moves a start off the timeline. The last two are new here rather than declared in M0: without
  the first, a rule that states two bounds resolves silently; without the second, an override
  asking for an instant no calendar can hold reported itself as a date section 3.3.10 defines
  away, which is a different fact.
- The recurrence dimensions of the shared policy: `Limits::occurrences_per_search`,
  `rdate_entries` and `exdate_entries`, and the `Meter` charges behind them.
- Four adversarial lenses against the expansion engine, landed as conformance cases addressed
  to the sentences they come from: `break_recur_rfc.rs` (the RFC's own worked answers asked
  through windows that open partway into them, and its expand/limit table cell by cell),
  `break_recur_budget.rs`, `break_recur_set.rs` and `break_recur_calendar.rs`. Eleven of their
  cases failed against the engine as built; each is fixed below and none was made to pass by
  weakening what it asserts.
- `SearchOutcome::CalendarEnded` and `DiagnosticCode::RecurrenceCalendarEnded`, for a rule with
  neither `COUNT` nor `UNTIL` that reaches the end of the four-digit year RFC 5545 section 3.3.4
  writes. The answer is complete — there is no more calendar — and it is not `RuleEnded`, which
  would be a false claim about the rule.
- `ical-tz` resolves. A `VTIMEZONE` is read into a bounded `TransitionTable` — one `Observance`
  per subcomponent `DTSTART` and one per `RDATE` value, with the subcomponent's `RRULE` attached
  to the first — and that table is a `ZoneSource`. Rule evaluation is closed-form arithmetic over
  the weekday of the first of the month, so a lookup has no loop in it to make expensive, and
  `RuleDay` carries the shapes producers actually emit: a fixed day, an ordinal weekday, a last
  weekday, and the seven-day `BYDAY`-with-`BYMONTHDAY`-run window every `Sun>=8` rule is exported
  as. A local time that occurs twice is `LocalResolution::Ambiguous` and one that does not occur
  is `Nonexistent`, each carrying its offsets; `pick` is the one place a policy collapses them to
  an instant. `CombinedZoneSource` queries two sources unconditionally and reports rather than
  prefers. Nothing is bundled, nothing reads a clock, and a `TZID` is matched by exact bytes, so
  `W. Europe Standard Time` and `/mozilla.org/20050126_1/Europe/Berlin` are identifiers this
  crate answers about rather than names it tries to translate.
- The seam with `ical-recur`, which M1 could only half-specify and which is the reason this
  milestone existed. `ical_tz::seam` states the contract — the timeline a zoned series is
  expanded on is that series' own wall clock projected onto UTC — and `ZonedSeries` is the
  driver: `anchor` projects a `DTSTART` of any of its four shapes, `project_until` reads an
  `UNTIL` under all three of its readings, and `actual` resolves one cadence key at a time,
  which is the only place a transition can be seen. `crates/ical-conform/tests/break_zones.rs`
  is the only file in the workspace naming both crates; it expands a daily 09:00 Europe/Berlin
  series through `RecurrenceInput::search`, resolves each key through `ical-tz`, asserts the
  seven UTC instants Berlin's published rules give across both 2026 transitions, and asserts
  separately that the reading which anchors once and never re-resolves is 3,600 seconds out from
  March 29th onward — so bypassing the seam fails the suite rather than shipping quietly.
- The five remaining questions M1 recorded and could not answer without a zone, each closed with
  a case. A floating `UNTIL` against a UTC or zoned `DTSTART` is read in `DTSTART`'s own zone and
  reported on `recurrence-until-not-utc`. An `UNTIL` written as a `DATE` against a date-time
  `DTSTART` is read where `UntilReading` says, because midnight drops the named day and end of
  day keeps it and both are permitted. An `EXDATE` written as a `DATE` becomes a whole-day
  `LocalInterval` under `ExclusionReading::WholeDay`, whose boundaries are computed through the
  zone rather than assumed — the day Europe/Berlin springs forward is 82,800 seconds long and
  the day it falls back is 90,000. `WallClockShift::measure` reports an override's elapsed and
  wall-clock moves as two numbers, which differ across a transition, and `extra_widening` is the
  seconds `max_absolute_shift`'s elapsed-only widening is short by. And `OrphanScan` reports a
  `RECURRENCE-ID` that names no generated instant on `override-matches-no-instance`, which is
  the last silent drop in these crates that had no code.
- Eight diagnostic codes for the zone layer and their golden-list rows:
  `vtimezone-without-observance`, `vtimezone-rule-unsupported`, `vtimezone-observances-truncated`
  on `Severity::LimitReached`, `duplicate-time-zone-identifier`, `time-zone-coverage-exhausted`,
  `recurrence-until-not-utc`, `exdate-value-type-mismatch` and `override-matches-no-instance`.
  Five more that M0 declared against M2 now have emitters: `unknown-time-zone`,
  `missing-time-zone-definition`, `ambiguous-local-time`, `nonexistent-local-time` and
  `time-zone-source-disagreement`.
- Four adversarial lenses against the built zone layer — the transitions, the sources, the seam
  with `ical-recur`, and the bounds — landed as `break_tz_transitions.rs`, `break_tz_sources.rs`,
  `break_tz_seam.rs` and `break_tz_hostile.rs`. Every zone in them is real and every expected
  column is transcribed from that zone's published rules: `Asia/Kathmandu`'s quarter hour,
  `Africa/Monrovia`'s offset with a seconds field, `Pacific/Apia`'s missing day,
  `Australia/Lord_Howe`'s half hour, `Australia/Sydney` across the new year and the 2008 rule
  change, and a million `RDATE` transitions.
- Five diagnostic codes the lenses found the workspace had no way to say, with their golden-list
  rows: `time-zone-without-transitions`, `time-zone-before-known-transitions`,
  `vtimezone-components-truncated` on `Severity::LimitReached`, `vtimezone-observance-unreadable`
  and `exdate-zone-unknown`.
- `Diagnostic` can name what it is about. `Subject` is a bounded inline name and
  `Diagnostic::about` attaches one, so three `TZID` parameters nothing defines are three
  diagnostics a caller can tell apart rather than three equal values.
- `ical_recur::RecurrenceInput::admitting` takes the caller's own second gate on a cadence key,
  asked after the window and before `COUNT`, which is the order ADR 0011 states and the first
  time either crate could compose the two. `ical_tz::ZonedSeries::admits` is the zone half of it.
- `ical_recur::RecurrenceRule::with_limit` substitutes a projected bound into a rule read from a
  file, which is what a zoned series' `Z`-terminated `UNTIL` needs and used to require rebuilding
  the rule by hand. `ical_tz::ZonedSeries::real_anchor` is the absolute-cadence anchor for
  `FREQ=HOURLY` and finer, and `ical_tz::WallClockShift::across` measures a move between two
  cadence keys rather than between two instants the seam never carries.
- `ical_tz::VtimezoneSet::definitions` keeps every reading of one identifier reachable, and
  `ical_recur::OverrideSet::collisions` counts the overrides a repeated cadence key shadowed.
- `ical-itip` answers what an incoming RFC 5546 message would change and whether the party
  applying it may. The eight methods and their sender rules, section 3's twenty-two constraint
  tables transcribed as data, the party and instance identities, the checked-and-charged
  `ItipMessage`, the occurrence-addressed `Transition`, the octet diff, and `evaluate_message`
  running one fixed order of denials with no partial success. The description is inert:
  applying it needs an `Authorization`, and there is no route to one that does not run the gate.
- `Authorization<'a>` borrows both of its inputs, so "not encodable" is a property of the type
  rather than a promise in prose — a caller that tries to carry one across a request boundary
  gets a compile error rather than a forgeable token — and `apply_transition` takes it by value,
  so a vetted transition is single-use. `Commitment` is the one value designed to cross bytes
  and deliberately carries no authority: it is compared only to cause a refusal, its digest is a
  checksum and not a MAC, and forging one buys exactly the ability to decline to be told that
  the target moved. `SECURITY.md` now states what the gate proves and what it does not.
- `ical_core::Component::apply_to_occurrence`, the occurrence-addressed write door beside the
  identity-addressed `Component::apply`, which is unchanged. A `REPLY` answers for one
  `ATTENDEE` among many, and an identity-addressed write would answer for all of them at once;
  ADR 0001 amendment 5 records why that is a second door rather than a widened first one.
- Both bridges from `ical_core::Component` to the scheduling surface: `ScheduledView::of` for
  reading, which owns the reconstructed content lines and the RFC 6868-decoded parameter values
  a `Component` does not store, and `ComponentTarget` for writing under the caller's own
  `Limits`.
- `ical_itip::resolve_instance` closes M2's repeated-hour question for scheduling: a
  `Z`-terminated `RECURRENCE-ID` picks its own half of a fold through
  `FoldSide::from_resolution`, a wall-clock one names both halves and stays unresolved, and an
  unresolved side can never compare `Same`, so the gate denies rather than guessing which
  meeting a message is about. `check_exclusions_are_placeable` is the caller precondition for a
  series carrying an exclusion no zone could place, and an `AnswerBasis` continuation is
  reported where it decided identity rather than only a rendering.
- `ical_itip::inspect_message`, the reporting pass that says "present and unusable" where
  `Component::audit` says "present": an `ORGANIZER` or `ATTENDEE` whose `CAL-ADDRESS` does not
  decode, an unreadable `SEQUENCE`, a section 3 `0` row, a missing required row, a `RANGE` no
  method admits, and a sender RFC 5546's prose does not permit. All ten `scheduling-*`
  diagnostic codes now have an emitter.
- Two features: `imip` reads an RFC 6047 `Content-Type` header under the caller's bounds and
  answers whether the envelope's `method` agrees with the body's `METHOD` — verdict-free, so it
  can only refuse before the gate and never widen it — and `freebusy` reads RFC 5546 section
  3.3's `VFREEBUSY` window and busy periods, refusing rather than reporting an empty calendar
  wherever a bound does not read.
- The scheduling chapter of the conformance corpus — thirty-six cases addressed to RFC 5546
  subsections, each asserting the section its own `MethodRule` carries, so a case and the
  transcribed table cannot drift apart — and a twenty-case adversarial suite, one test per named
  attack, each fixture built so exactly one gate can fire, which makes the assertion about the
  gate's order and not only its answer.
- `MatchHeader`, the reading door for `If-Match` and `If-None-Match` that RFC 9110 section
  13.1.1 defines and this crate rendered without ever reading. A server assembled from the
  parts could not tell `If-Match: *` from a header value it could not parse, and those two
  demand opposite outcomes on a write; the list form is read too, because the specification
  defines it and a server that refused it would fail a conformant client.
- `CalendarQuery::shape` and `CalendarQuery::timezone`, so RFC 4791 section 9.5's own
  production — `((DAV:allprop | DAV:propname | DAV:prop)?, filter, timezone?)` — is a body this
  crate reads and writes rather than one it refuses with `DavError::Unexpected`, and the zone a
  floating `time-range` is resolved against survives a read and a re-encode.
- `ValueError::SelectionContradiction`: a `calendar-data` selection stating `allprop` beside
  named properties is a value RFC 4791 section 9.6.1's grammar cannot express, and the crate's
  precedent for one of those is a refusal rather than a body that says something else.
- Two diagnostic codes, `dav-property-markup-dropped` and `dav-sync-token-withheld`.
- The single `icalkit` scheduling facade now materializes an authorized organizer `REQUEST`
  carrying `RECURRENCE-ID;RANGE=THISANDFUTURE` as a detached component, updates an existing
  anchor instead of duplicating it, verifies the anchor against the master's recurrence set,
  and exposes `Message::review_in` so the check shares an `Engine` session's aggregate budget
  and zone database. Sender authorization, ambiguous-anchor refusal, DST gaps, transactional
  application, and CalDAV propagation to later occurrences are covered through public workflows.
- CalDAV `calendar-query` now evaluates a recurring component's `time-range` and
  `prop-filter` clauses against the same effective occurrence. A `THISANDFUTURE` anchor's
  property overlay is composed before filtering later instances, so a matching future time
  cannot be combined with a stale property from the master component.

### Fixed

- **Eleven recurrence defects, found by four adversarial lenses run against the shipped
  expansion engine.** The last period of every cadence existed and was being deleted.
  `PeriodWalk` computed each period's exclusive upper edge and refused the period when only that
  edge left the calendar, so
  `FREQ=DAILY` from 9999-12-28 stopped on the 30th, `FREQ=YEARLY` lost the year 9999, and the
  same held for the other five frequencies — although every instant in those periods is
  representable and the RFC writes them. The edge was read nowhere outside the walk's own tests;
  a period carries its anchor now.
- An `EXDATE` landing on an `RDATE` could erase occurrences it did not name. The merge documents
  a three-call protocol — `is_drained`, `takes_rule_key`, `step` — and the search called only
  `step`, reading its `None` both as "the series is over" and as "the offered rule key was
  consumed". So an exclusion on an addition deleted the rule instance after it, `DTSTART`
  included, having already spent its `COUNT`; and an exclusion on the head of an `RDATE` tail
  discarded every addition behind it.
- `BYWEEKNO` under `FREQ=YEARLY` expands a period to the weeks of its *week-numbering* year
  rather than filtering the days of its calendar year. The two readings partition the same union,
  so they differ only where a period is skipped, a `BYSETPOS` selects, or a year's week count is
  asked about — and there the old one attributed week 1 of 2020 (which begins 2019-12-30) to the
  2019 period, emitted a week 53 in years that have none, and let `BYSETPOS=1` over week one
  select January 1st.
- `BudgetExhausted::candidates_spent` reports what the search charged rather than what expansion
  handed back. A period refused while filling had paid for every candidate it generated and
  reported none of them, and a rule that produces an instance in no period spent its whole budget
  and reported zero — telling a caller deciding whether to retry that it got nowhere at the
  moment it needed to hear the opposite.
- `Meter::is_exhausted` latches for every bound the ledger keeps. It carried the octet budget
  alone, so a search stopped by `Limits::candidates_per_period` or `occurrences_per_search` left
  the caller's own meter — ADR 0002's most durable report of a truncated answer — reading clean.
- A `BYDAY` ordinal under a frequency RFC 5545 section 3.3.10 forbids one under is ignored and
  its weekday kept, under all five such frequencies. `FREQ=WEEKLY` alone used to resolve it
  inside a scope one week wide, where `BYDAY=2TU` matched nothing and silently emptied the entire
  series while `BYDAY=1TU` worked. The decoder now reports the construct on
  `recurrence-rule-part-out-of-range`.
- **Eleven zone defects, found by four adversarial lenses run against the shipped resolver.**
  A zone rule stopped being consulted once four dated transitions stood between it and the
  query, so a `Europe/Berlin` definition restating two years of its own transitions as `RDATE`
  lines answered noon on the first of July as CET — an hour wrong, with `AnswerBasis::Computed`
  and no diagnostic. Every rule a definition carries is now asked about every query, and a rule
  that fires rarely is probed back sixty-four years rather than three, which is what
  `FREQ=YEARLY;BYMONTH=2;BYDAY=5SU` needs to be found from the year after it fires.
- A zone with two transitions in one day reported seventeen hours of ordinary wall clock as
  local times that never happened, because the candidate offsets were read at the two ends of a
  two-day window and the offset governing the middle of it was never one of them. The window is
  walked now, and a gap is answered from the transition that sprang over the queried wall clock
  rather than from whichever transition the far end of the window held — which is what
  `gap_end`, `offset_before` and section 3.3.5's `shifted` reading each have to come from.
- A transition table is ordered by the instant each observance begins rather than by the wall
  clock its `DTSTART` spells. Two observances declared on one wall clock used to resolve by the
  order the producer wrote them in, an hour apart, and a `TZOFFSETFROM` further east than the
  previous observance's left the binary search placing a query among onsets that did not ascend.
- A `VTIMEZONE` that exists and carries no usable observance is no longer indistinguishable from
  a zone nobody defined. It answers `LocalResolution::Undetermined` rather than `None`,
  `ZoneSource::recognizes` is the question `offset_at` cannot answer, and
  `CombinedZoneSource::report` no longer claims `unknown-time-zone` — a violation about a zone
  the file supplies — for one.
- A table's early end is labeled. A definition whose `RDATE` lines begin in 2027 answered July
  2020 with its earliest `TZOFFSETFROM` extended backwards forever and called it computed;
  `AnswerBasis::BeforeKnownTransitions` says so. And `coverage_end` is `None` only when every
  side of a definition repeats forever, so a daylight rule that runs on beside standard onsets
  that stop in 2029 no longer claims to know what 2031 does.
- A second definition of one `TZID` is kept beside the first rather than dropped, so an empty
  placeholder `VTIMEZONE` written above a full definition can no longer erase it, and both
  readings of a file that states two stay reachable.
- A definition the caller's own zone-count bound turned back is reported as that, naming the
  zone, and the identifiers it declares are no longer reported as identifiers the calendar never
  defined.
- An observance whose required properties are all present and whose values cannot be read —
  `TZOFFSETTO:+9999`, or a `DTSTART` written as a `DATE` — is reported under
  `vtimezone-observance-unreadable`. `Component::audit` sees a property that is there and the
  reader was deferring to it, so both files produced a zone in the set that answered nothing and
  no code from anybody.
- A `Z`-terminated `EXDATE` on a series whose `TZID` no source recognizes is kept as the real
  instant it names and reported under `exdate-zone-unknown`. It used to be dropped in silence,
  which is the one outcome that layer says is indefensible.
- Two overrides naming one cadence key are ranked by file order and counted rather than refusing
  the whole series. The two halves of the hour a zone repeats are one wall clock, so a zoned
  series produces the collision without anybody making a mistake, and `InputError::Duplicated`
  cost such an event every occurrence it had.
- A `REPLY` carrying a `VALARM` was accepted whole. The gate counted a payload's properties
  against RFC 5546 section 3 and never its components, so the `VALARM` row of section 3.2.3's
  `SUBCOMPONENTS` table — which reads `0` — was unenforced, and an attendee's answer could
  install a component the recipient's client will act on. The refusal is
  `AuthorizationDenied::MethodForbidsComponent`, a variant of its own rather than the existing
  property-shaped one carrying a component's name; only the forbidden direction is read, and
  that omission is machine-checked against the transcribed tables rather than asserted in prose.
- A `PUBLISH` or a `REQUEST` about something the caller does not hold was always refused
  `OrganizerMismatch`, so the two methods RFC 5546 defines to arrive before the recipient has
  anything could never succeed and `TransitionReason::Created` was unreachable: the sending
  party was resolved only against state that, being absent, names nobody. The lookup falls back
  to the message's own payload when the prior state is absent. What rests on the transport as a
  result is stated in `SECURITY.md` and in ADR 0005 amendment 4 rather than left implicit.
- A `REFRESH` described the removal of the organizer's `DTSTART`, `RRULE` and attendee list.
  It was diffed as a restatement of the component, so it stated a removal for every property its
  four lines do not echo, and the field rule then refused the attendee for removals the diff had
  invented. It describes nothing now, per section 3.2.6. Relatedly, the revision gate runs only
  for a method whose own table admits a `SEQUENCE`: a refresh states no version of its own, so
  the absent-is-zero reading made every refresh stale against every held revision above zero.
- The octet diff counted property occurrences per name *as written* while filing them under the
  normalized identity, so an implementation reporting `DTSTART` on one line and `dtstart` on
  another counted two first occurrences and filed both under one key, silently discarding the
  first. Both sides count the normalized identity now.
- **Eleven scheduling authorization and replay defects, found by four adversarial lenses run
  against the shipped gate.** An attendee's `COUNTER` could rewrite the `ORGANIZER` line — and
  so hand itself a meeting it was merely invited to — raise `SEQUENCE` and lock the real
  organizer out of its own updates, and replace its own `ATTENDEE` line with a party nobody
  invited; a party named only inside somebody else's `DELEGATED-TO` could do the first of those
  without being on the attendee list at all. `field_rule` moves `ORGANIZER` and `SEQUENCE` to
  `OrganizerOnly`, since an attendee restating either produces no change to be asked about and
  the permission bought only the case it was not written for, and `FieldRule::AttendeeOwn` now
  asks both whether the occurrence is the actor's and whether the line the change leaves behind
  still names the actor.
- A held component whose `UID` line appeared twice read as a component the caller does not
  hold, so the sending party was looked up in the attacker's own message — where the attacker is
  the `ORGANIZER` — and a stranger was authorized to rewrite the organizer line, the time and
  the attendee list of a meeting the caller was holding. Absence is now the absence of
  everything a component could state, and the bridge reads a name stated twice with
  byte-identical lines as the one claim it is.
- A message at the revision already held overwrote it. At an equal `SEQUENCE` a revision
  stating a readable `DTSTAMP` supersedes one stating none, so a `DTSTAMP` written as a `DATE`
  or under a `TZID` no longer wins the tie it declined to offer — nor, once applied, disarms the
  ordering for every later message at that revision. An organizer-authored message that
  supersedes nothing describes nothing, because RFC 5546 section 2.1.4 requires an update to
  increment `SEQUENCE`.
- An attendee's own earlier `REPLY`, replayed, silently reverted their current answer. Two
  replies are one revision answered twice and no component can order them, so
  `ScheduledComponent::attendee_answered_at` carries the fact and the reply diff records it on
  the line it answers for as `ical_itip::ANSWERED_AT`. A state that records nothing admits the
  second answer, which keeps a change of mind working and is stated in `SECURITY.md` as a
  defense a caller can discard.
- A `RECURRENCE-ID` written as a bare wall clock was read as naming one instant, so one `REPLY`
  answered both halves of a repeated hour — the spelling the zoned form was already refused for.
  Every wall-clock spelling is now unresolved until a zone places it.
- A `CANCEL` naming one instance twice cancelled the whole series: the gate read only the `0`
  rows and the required rows of section 3's tables, so a `0 or 1` row was enforced for no method
  at all. Every row is read in both directions now.
- A calendar stating two different `METHOD`s was reported as stating none, and filed as an
  ordinary `.ics` with nothing recorded. It is `MessageError::AmbiguousMethod` with
  `scheduling-method-ambiguous` beside it.
- A `REPLY` whose `ATTENDEE` was empty or did not decode was authorized to change nothing, which
  is indistinguishable from an answer that was applied. It is
  `AuthorizationDenied::CalendarAddressUnreadable`, and an empty `CAL-ADDRESS` now identifies
  nobody rather than identifying every other empty one.
- A `RECURRENCE-ID` in an hour a zone sprang over answered identically under all three readings
  of `GapPolicy` with an empty report, and the refusal that followed claimed the instance could
  not be told from its neighbor — about an hour in which the zone showed no meeting at all.
  `resolve_instance` applies the caller's own gap reading, `FoldSide::Nowhere` is an identity
  that names no instant, and `scheduling-instance-nonexistent` says when a reading dropped one.
- A message of a hundred thousand properties was read for four units and then described in
  full, so a shared meter bounded how many messages an inbox read and not what reading one
  cost. `Limits::max_payload_properties` bounds a payload's property list and
  `ItipMessage::read` charges it.
- **Twenty CalDAV defects, three of them security findings, found by four adversarial lenses
  run against the shipped protocol layer.** A property this crate had no model for was kept as
  its decoded character data and written back unescaped, so a peer writing
  `&lt;D:href&gt;/calendars/ann/private/secret.ics&lt;/D:href&gt;` inside its own extension
  property got a real `DAV:href` element in the body a proxying server emitted;
  `PropValue::Unmodeled` now carries character data and is escaped, and `PropValue::Markup`
  carries a peer's elements as a fragment the reader re-serialized, so RFC 4918 section 9.1.3's
  own structured property survives a proxy instead of being flattened. `ETag::parse` accepted
  thirty-four octets outside RFC 9110 section 8.8.3's `etagc` — `CR` and `LF` among them — and
  an accepted tag is rendered into an `If-Match` header value, so a server answering
  `<D:getetag>"2d9&#13;&#10;If-Match: *"</D:getetag>` chose the caller's other headers and
  turned a conditional write into an unconditional one. And nothing charged the octets a
  comment occupies, so thirty-two mebibytes of `<!-- ... -->` cost 2,496 octets of a
  sixteen-mebibyte ledger; comments and the whitespace outside the root are charged now.
- An attribute value is the value XML 1.0 section 3.3.3 defines rather than the octets between
  its quotes: references are resolved and a literal tab, line feed or carriage return becomes
  one space. A `comp-filter name="VE&#78;T"` selected `VE&#78;T` here and `VENT` in every
  conformant processor, so two implementations disagreed about which components a hostile
  `calendar-query` matches; the same gap grew a request four octets a hop through a re-encode.
- A character XML 1.0 section 2.2's `Char` production excludes is refused however it is spelled.
  `&#0;` was refused under its own name and the literal `0x00` octet was not, as were the other
  C0 controls and octet sequences that are not UTF-8 — so the reader accepted documents no
  conformant processor will parse and handed its caller a run that is not text. Enforced over
  names, normalized attribute values and character data, with two stated exceptions: the
  elements the line-ending carve-out names, and `DAV:href`, whose value this crate models as
  octets on purpose.
- `CALDAV:calendar-timezone` and `CALDAV:timezone` keep their line endings. RFC 4791 section
  5.2.2 makes the first "a valid iCalendar object containing exactly one VTIMEZONE component",
  so its `CRLF` terminators are RFC 5545 syntax for exactly the reason `calendar-data`'s are; a
  client that read a collection's timezone and `PROPPATCH`ed it back rewrote the stored object,
  which is the harm ADR 0004 Amendment 1 exists to prevent, one property over.
- A run of character data that is only a line break is not layout inside a calendar. A blank
  run was dropped unconditionally, so two XML comments positioned around the `CRLF` that
  terminates a content line welded two iCalendar properties into one and changed the object's
  `UID`; the same rule read a `DAV:displayname` whose value is a space as a property that
  arrived empty.
- A value split across two runs by a comment reaches the caller whole. `text_of` kept the first
  run and discarded the rest with nothing reported, which made an `href` and — worse — a
  `DAV:sync-token` into values the peer never sent.
- A report this reader truncated states no synchronization token. RFC 6578 section 3.4 makes the
  token a statement about the whole answer, and the guard was positional: with the token written
  before the responses, a report cut short at sixteen of forty thousand handed back the full
  token, and a caller storing it would never be told about the rest.
- A status line is read as the code it states or as none. `HTTP/1.1 2000 OK` read as `200` and
  `HTTP/1.1 4045` as `404`, which promotes a malformed `DAV:propstat` into a success
  `DavResponse::successful_value` hands back.
- A precondition named inside one `propstat` stays in that group. RFC 4918 section 14.22's
  grammar puts an `error` inside the group it explains; hoisting every group's conditions into
  one bag on the response lost which refusal each one was about.
- A kept fragment carrying an ampersand can be written and read back. The encoder's reference
  check asked only whether a `;` appeared within twelve octets, so `AT&T` could not be written
  at all and `a & b; c` was emitted with a bare `&` — a document this crate's own reader
  refuses.

### Changed

- The purity gate reads the package a dependency links rather than the key it was written
  under. A `package = "..."` rename defeated every leg of the old gate; it is now a violation
  in both the inline and the sub-table spelling, alongside a dependency taken from a registry
  and a `no_std` crate missing from the gate's own crate list.
- `ParseError` gained `TooManyParameters`, so the parameter-count bound is reported as itself.
  It had been reported as `HeaderTooLarge` carrying the octet ceiling, which named a number a
  caller could raise without the refusal ever going away. `MonthAddOutcome` is
  `#[non_exhaustive]`, as ADR 0011 and `docs/design/ical-tz-api.md` always described it and the
  code did not; adding it before `ical-recur` and `ical-tz` match on the enum costs nothing.
- `Component`'s `Clone`, `PartialEq`, `Ord`, `Hash` and `Debug` are written over an explicit
  stack instead of derived. Each of the five recursed one frame per level of nesting, and
  `max_component_depth` is a `u16` a caller raises, so `document.clone()` on a twenty-thousand
  deep tree took the process down — an abort, which no `catch_unwind` sees.
- Construction is no longer an unchecked door. `Property::new`, `Parameter::new` and
  `Boundary::new` are crate-private; `Property::create`, `Parameter::create` and
  `Component::create` refuse the octets RFC 5545 section 3.1 cannot write back, so a value
  carrying its own `CRLF` can no longer be pushed into a tree and serialize as two content
  lines. Octets that were never read from anywhere have no producer's spelling to preserve, so
  the refusal costs the round-trip claim nothing.
- `max_folds_per_line` is justified by a measurement rather than by a division: the largest
  inline `ATTACH` the default policy admits is retained across 14,170 continuations folded at
  75 octets and 14,564 at 73, both under the bound, which is now documented as what the
  headroom buys — a producer folding tighter than section 3.1 asks, not a longer value.
- No write authors a component boundary. `BEGIN` and `END` read back whole, which is all the
  grammar's own predicate asks, and read back as a line that opens or closes a component — so
  `Property::create`, `set_raw`, `set` and three of `Component::apply`'s four variants refuse
  them as `MutationError::ComponentBoundary`. One addition used to move six of twelve lines
  into a component nobody added; the reader still keeps such a line, because a file holds one.
- The serializer writes the terminator a stored line owes once something is written after it.
  A property read out of a truncated export carries a layout with no terminator and is
  `Clone`: placed above another line it stored two content lines and wrote one, with the
  second line's octets glued to the first one's value and nothing reported. A line that is
  still last is still written without one.
- A line whose name begins with `SP` or `HTAB` — which only a fold at octet zero can produce —
  is refolded below a fold of its own, instead of rejoining the line above it and taking its
  property out of the file.
- `Parameter::create` and `ParameterEdit` write the value they are given in the spelling RFC
  6868 gives it: `^'` for a `DQUOTE`, `^n` for a newline, and `^^` for a caret, without which
  a value a caller spelled `Ann ^n Marie` came back a newline from this crate's own codec.
  These doors take a value rather than a spelling, so a caller moving a parameter from one
  line to another resolves it with `decode_caret` first. What neither grammar spells — a `CR`,
  and every other control octet — is still refused.
- `Component::apply` addresses the identity a change names rather than its first occurrence.
  `Replace` and `SetParameters` used to write one of two `DTSTART`s and report success, which
  left the identity the caller addressed carrying two different values; `Remove` already took
  every occurrence, and now all four agree. `Component::get_mut` still names the first, and
  still documents that it does.
- The `DURATION` encoder refuses a day or second count with no positive counterpart, which it
  used to write as twenty digits its own decoder rejects, and a `DateTimeValue::Zoned` whose
  `TZID` is empty is refused rather than written — the read side had already ruled that an
  empty `TZID` names no zone. A `FLOAT` whose decimal expansion is past the largest `f64` is
  `MalformedFloat` rather than infinity, which the guard written to keep infinity out did not
  keep out.
- `sweep.rs` covers the evidence it was landed to hold: 6,514,872 exhaustive examinations,
  2,200,000 randomized documents and 156,180 generative mutations, against the 1,900,000,
  2,200,000 and 135,000 M0-alpha reported. Its refusal predicate counts `BEGIN` after the
  folds are taken out, so a folded boundary no longer makes a sound refusal look like a defect
  in the reader; a fixture it cannot examine now fails it rather than being counted and
  skipped; and a fourth leg puts every committed fixture through one scoped write of each kind
  the change vocabulary has, which is the first time anything in that file reached P3 or P4.
- `DEFAULT_CANDIDATE_BUDGET` is 262,144 rather than 65,536, which is the calibration ADR 0010
  assigns to whoever ships the first recurrence milestone. The old number was exactly
  `Limits::DEFAULT.candidates_per_period()`, so the per-period ceiling and the whole-search
  budget were one bound wearing two names: a search that filled a single maximal period had
  already spent everything. Four times the ceiling admits a decade of a daily rule, a year of
  an hourly one and a day of `FREQ=SECONDLY`, and refuses a year of `FREQ=MINUTELY` and a week
  of `FREQ=SECONDLY`, which are policies rather than defaults. The workload table is asserted
  against the shipped constant.
- ADR 0002 carries fourteen amendments, each written because M1 found the sentence above it
  wrong rather than merely unbuilt. The largest: the `Item = Result<Occurrence, BudgetExhausted>` the
  ADR first committed to does not deliver its own guarantee, because `Result`'s `IntoIterator`
  makes `search.flatten()` discard every terminal marker; what shipped is a crate-owned
  `SearchStep` enum, the caller's latching `Meter`, and `RecurrenceSearch::outcome()`. Also
  amended: a window admits by cadence key *or* by effective start rather than by start alone,
  emission is ordered by cadence key because reordering needs a buffer nothing charges, the
  `results()` adapter is withdrawn rather than deferred, and the open dedup case the ADR filed
  without an answer is closed — an `RDATE`-added instant and a diff-moved one that collide are
  both emitted.
- `Limits::DEFAULT.occurrences_per_search` is 262,144 rather than 65,536. A whole day of
  `FREQ=SECONDLY` is 86,400 occurrences and the candidate calibration admits it, so the
  retention bound was refusing a workload the budget beside it had already agreed to pay for —
  the same "two round numbers, one of them wrong" defect the candidate budget was fixed for.
- ADR 0003 carries six amendments, each written because M2 found the sentence above it narrower
  than what the crate needed rather than merely unbuilt. The source trait has two methods, not
  one: without `offset_at` a source can carry an instant out of a zone and not into it, which
  leaves a `Z`-terminated `UNTIL`, a `RECURRENCE-ID` and an override's own ends with no reading
  at all. `LocalResolution` is `{ Unique, Ambiguous, Nonexistent }` with each reading carrying
  its own offset and daylight flag, because the shape the ADR pinned cannot express RFC 5545
  section 3.3.5 and because a flag inferred from which offset is larger answers
  `Australia/Lord_Howe` backwards. `PolicyOutcome::Agreed` keeps both answers and the enum is
  generic over them. Reporting a disagreement is a second call rather than a side effect of
  asking, because one series resolved a thousand times is one fact. The instruction that
  `LocalResolution` "must be verified against real transition data rather than merely shaped
  correctly" is discharged against four real zones read from committed fixtures. And the
  zone-count bound `VtimezoneSet::insert` charges is reported by no code, which is recorded as
  the hole it is rather than closed with a code invented during integration.
- `docs/design/ical-tz-api.md` is brought onto the shipped surface and gains a "What M2 shipped"
  section. The document described `LocalResolution::Single`/`Gap`, a `ZoneProvenance` pair
  holding a `Coverage`, public struct fields on `Observance` and `YearlyRule`, a `Limits`
  argument where a `Meter` shipped, and none of the seam at all; three of its five usage
  examples would not have compiled. Its five open questions are answered, including the one
  proposed code that was refused: an observance whose `TZOFFSETFROM` cannot be read is
  `Component::audit`'s finding under section 3.6, and a second copy of that judgment in this
  crate is a second place for the two to disagree.
- `ical-recur`'s own documentation states the seam from its side rather than only naming
  `UntilClock`: the crate-level docs carry the nominal timeline and the caller's two
  obligations under it, `UntilClock`'s own doc narrows the sentence that was true only of a
  floating or UTC series, and `max_absolute_shift` says outright that the seconds it counts are
  elapsed ones and names `ical_tz::extra_widening` as what a zoned caller adds. The crate graph
  is unchanged; `ical-recur` still has no zone and `just purity` still says so.
- `max_absolute_shift`'s documentation called its result a count of elapsed seconds, and no
  timeline makes that true: for a zoned series both instants an override carries are on the
  series' own wall clock, so the number is a wall-clock count and already exact, and
  `extra_widening`'s shortfall on that timeline is always zero. Both docs say which timeline
  they count now, and `WallClockShift::across` is where the two readings of one move are held
  apart. ADR 0002 amendment 15 records the correction.
- `ZonedSeries::actual` is documented as resolving the wall clock it is handed rather than "the
  instant the occurrence at cadence key `key` actually happens at", which was false for every
  occurrence a `RANGE=THISANDFUTURE` override moved. What to pass is `Occurrence::start`.
- `VtimezoneSet::len` counts identifiers rather than definitions, and `VtimezoneSet::table`
  answers with the first definition carrying a transition rather than the first definition.
- ADR 0003 carries thirteen amendments, ADR 0011 three, ADR 0002 sixteen and ADR 0009 one, each
  written because an adversarial case found the sentence above it wrong rather than unbuilt.
- ADR 0005 carries six amendments and ADR 0001 a fifth, and `docs/design/ical-itip-api.md`
  gains a "What M3 shipped" section for the five places it promised something the frozen
  signatures could not deliver. The largest: `impl ScheduledComponent for ical_core::Component`
  cannot exist, because `property_line` must hand back a whole content line a `Component` stores
  nowhere as one contiguous run, and RFC 6868 decoding produces octets the file does not contain
  while `Party` and `Attendee` are `Copy` over borrowed ones. The bridge is `ScheduledView`, a
  value that owns both — which costs one build pass per component and keeps three frozen files
  untouched. Also amended: `ScheduleTarget` routes through `Component::apply_to_occurrence` and
  not ADR 0001's identity-addressed `PropertyMut` guard, `MediaTypeParams::read` is bounded,
  charged and fallible, and `Authorization` borrowing its state means the same `Component`
  cannot be both the state judged and the target written.
- `ical-dav` builds and interprets every body RFC 4791 defines, from both ends, with no
  transport and no XML dependency. `RequestBody::read` answers which of the five request roots
  a server was handed — a fact about the octets rather than about the HTTP method, since
  `REPORT` carries three of them — and `WriteXml` writes each one back out of the same fields.
  `MultiStatusReader` and `MultiStatusWriter` carry a multistatus one `DavResponse` at a time,
  with the owned `MultiStatus` as one consumer of each rather than a second implementation
  beside them, so a server enumerating forty thousand resources and a client holding one
  collection run the same code.
- `XmlReader`, this crate's own tokenizer, refusing by class rather than by budget: no
  `DOCTYPE` in any casing, which closes the billion laughs and the internal, external and
  file-pointing entity together; no processing instruction, no encoding but UTF-8, no unbound
  prefix, no mismatched tag, no duplicate attribute, no `<` inside an attribute value. It is
  iterative with an explicit stack, so a hundred-thousand-deep body meets
  `LimitExceeded::Depth` rather than a stack overflow, and namespace bindings are charged as
  they are declared and released as their elements close. Every lookup is a resolved
  `(namespace, local name)`: `SabreDAV`'s `d:`/`cal:`, Radicale's `ns0:`/`ns1:` and Calendar
  Server's default `DAV:` declaration read to one value, and a familiar `D:` bound to a
  namespace of an attacker's choosing reads as foreign.
- `XmlWriter`, whose open-element stack makes an unbalanced document unrepresentable rather
  than merely unlikely: `close` writes the tag the stack names and not one a call site does,
  `finish` closes whatever is still open, `attribute` refuses `xmlns` and `xmlns:*` so a fixed
  prefix cannot be rebound to mean something else, and no door emits a `CDATA` section.
- The line-ending collision between ADR 0001 and XML 1.0 section 2.11, resolved in both
  documents' own registers rather than in a commit message. Inside `CALDAV:calendar-data` and
  nowhere else the reader hands back the octets as they arrived, so what reaches
  `Document::parse` from a multistatus is what the server sent; for that one element it is
  deliberately not a conformant XML processor, and must never be used to canonicalize or verify
  signed XML. The writer needs no departure at all — a `CR` leaves as `&#13;` — so anything
  this crate writes is recoverable by any parser. `TextPolicy::Normalized` restores conformance
  at runtime rather than behind a feature flag, because a feature is unified across a
  dependency graph and one crate could otherwise change how another's calendars parse; every
  payload it costs a `CR` reports `dav-calendar-data-line-endings-folded`, and
  `CalendarPayload::is_as_sent` is how a caller about to write the payload back finds out which
  it holds.
- `Revision` and `Precondition`, which close what M3 filed as "ADR 0004 territory and
  undesigned": what a read learned about one resource — presence, `ETag`, `schedule-tag`, sync
  token — and the conditional write that makes a second turn land on that revision or be
  refused by the server. Presence is three-valued because only `404` and `410` assert an
  absence and reading a `403` as one is how a client creates a second copy of an event it was
  merely not allowed to see. A weak `ETag` yields no precondition rather than an `If-Match` no
  server can satisfy or an `If-Match: *` that means something else. Sync tokens are carried and
  never parsed or ordered, and `Revision` refuses `Ord` for that reason.
- Two injection vectors closed on the writing side. A `PropValue::Unmodeled` is written
  verbatim — that is the losslessness claim — but only past a filter that refuses unbalanced
  tags, `<!` and `<?`, an unterminated `&` and nesting past the remaining depth budget, with
  quote-aware tag termination so a `>` inside an attribute value is not miscounted. An
  extension's local name is validated as an ASCII NCName, because a name cannot be escaped: a
  peer's property called `x/><D:href>/evil</D:href>` is not written differently, it is not
  written at all.
- Four `Limits` dimensions and seven diagnostic codes in `ical-grammar`, including the one
  ADR 0010 predicted would be missing: `max_prefix_bindings`, since a namespace declaration is
  charged by no depth counter and no element count, and one element at depth one can carry a
  thousand of them.
- `DavError::Foreign`, `ValueError::AttributeMissing` and `ValueError::AttributeValue`. Three
  units independently reached for `Syntax(Malformed)` to report a well-formed body carrying a
  vendor element or a `negate-condition` outside `(yes | no)`, which tells an operator reading
  logs that the peer sent something that is not XML. It did not.
- `crates/ical-dav/tests/interop.rs`, which drives the two halves through each other rather
  than each against a stand-in: every request body written and read back through the shipped
  tokenizer, the three real-server fixtures read and re-encoded and read again, the streaming
  and owned multistatus encoders asserted to emit identical octets, and the one input where
  `encode -> decode` is idempotent rather than the identity stated as such — a `PropRequest`
  naming `calendar-data` twice converges on the one spelling RFC 4791 section 9.6 admits.
- ADR 0004 carries four amendments and ADR 0001 a sixth. The first reverses the DP-14 paragraph
  that had chosen the conformant read, having gone back to XML 1.0 section 2.11 and RFC 4791
  section 9.6 rather than to the earlier reading of them; the fourth names the four limit
  dimensions; the third states the header boundary — which headers are protocol semantics and
  which are the transport's — that the ADR had left to be discovered. ADR 0001's amendment is
  the boundary the resolution does not reach: section 9.6 lets a server omit the `CR`, so "the
  octets this workspace was handed" is not always "the octets the producer wrote", and that gap
  is the protocol's rather than this workspace's.
- `docs/design/ical-dav-api.md` gains "What the units landed, and where this document was
  wrong": nine corrections, including a `MultiStatusReader::new` that took a body and a
  `Limits` and takes neither, a `ResponseSource` signature inconsistent with `ReadXml` inside
  one code fence, a call to an `ical-core` function that does not exist, and a promised
  `tests/adversarial.rs` that was not written — with each attack it was to carry named against
  the test that does assert it.
- **A `calendar-data` payload that is not UTF-8 is refused by the encoder rather than written.**
  A document declaring UTF-8 and carrying octets that are not is discarded *whole* by any
  conformant processor, so the peer loses the entire response and nothing on the wire says why;
  there is no escaping that helps, because a character reference names a code point and these
  octets are not one. The cost is stated in ADR 0001's own register: an `.ics` whose RFC 5545
  fold falls between a lead octet and its continuations is a file this workspace round-trips
  byte for byte and which has **no CalDAV representation at all**. That is a fact about the
  envelope, not about the file.
- `PropValue::Unmodeled` is a property's character data and `PropValue::Markup` is a property's
  elements; a property carrying both keeps its text and reports `dav-property-markup-dropped`.
  A reader that answered `PropValue::Text` for a property outside the vocabulary now answers
  `Unmodeled`, because "this crate read the value" and "this crate has no model for this
  property" are different claims and one direction has to be able to state what the other reads.
- `XmlPull::attribute` answers a slice of the tokenizer rather than of the body, since a
  normalized attribute value appears nowhere in the body contiguously, and the trait gains
  `attribute_count` and `attribute_at`. `ResponseSource` gains `was_truncated`, with a default
  of `false` for a source that has no bound of its own to stop at.
- **`ical-grammar` is gone as a crate and survives as a layer.** Its sources are
  `crates/ical-core/src/grammar/`, a private module tree whose every item the crate root
  re-exports, so `ical_core::Token` is the one spelling and `ical_grammar::Token` names
  nothing. Six crates are publishable, not seven; nothing is published yet. The seam was
  insurance against a caller that wanted the grammar without the model; ADR 0004 said what to
  do if none appeared, and none did. What replaces the crate boundary is
  `gates/grammar-layering`, an unpublished workspace member that compiles the same sources
  where no model exists, plus a second rule in `just purity` for the spelling that member
  cannot see. `Token` keeps `#[non_exhaustive]`, which now means what it says: a minor release
  outside this workspace, a compile error inside it. This entry originally credited
  `unreachable_patterns = "deny"` with keeping a wildcard arm from taking that back; the entry
  below corrects that.
- **`just purity` gained the three rules the collapse turned out to need, and lost a claim it
  could not hold.** A wildcard arm over `Token` is now refused by reading the arms, because
  `unreachable_patterns` fires only on a catch-all after every variant is covered and the shape
  that silently drops a payload omits a variant instead. The layering member is held to the
  workspace by string equality — the member line, the package name, `publish = false`, both
  `[lib]` switches and the `#[path]` string — because deleting it passed every gate here.
  `release-plz.toml` is read against the root manifest's published members, which is how it
  came to name `ical-grammar` for a whole landing after the crate ceased to exist; both stale
  references are gone. Act 2 itself gained the four spellings that walked around it: whitespace
  inside a path, `extern crate self as ical_core;`, `#[path]` into the layer, and a `.rs` file
  the module root never declares. See `docs/adr/0004`, amendment 18.
- **The iTIP kernel moved physically behind `icalkit::internal::itip`.** The scheduling facade
  calls the private module and no longer depends on `ical-itip`. A temporary unpublished
  compatibility package compiles those same source files for the legacy conformance suite, so
  there is one implementation rather than a synchronized copy; the architecture gate prevents
  the facade from depending back on the absorbed boundary. Free-busy and iMIP remain
  unconditional capabilities of `icalkit` under its two-feature contract.
- **The bounded recurrence engine moved physically behind `icalkit::internal::recur`.** The
  public recurrence workflow, CalDAV query evaluator, and migrated iTIP kernel now call that
  private module, and the facade no longer depends on `ical-recur`. Its temporary package
  compiles the same source and fixture for legacy conformance consumers; the architecture
  gate prevents a back-edge, and the in-crate `SearchStep` match is exhaustive without the
  wildcard that its former non-exhaustive crate boundary required.
