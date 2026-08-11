# ADR-0001: unknown properties and components survive a round trip

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10, 2026-08-11

## Context

Every real calendar file contains things this library has never heard of. Vendor extensions
(`X-MICROSOFT-CDO-BUSYSTATUS`, `X-APPLE-STRUCTURED-LOCATION`), properties from RFCs published
after this code was written, parameters on properties we do parse, and whole components we have no
model for.

The default behavior of a typed parser is to discard those, because they do not map onto any
field. The consequence appears the first time two clients touch the same event: one client writes
it, another opens and saves it, and information the first client depended on is silently gone.
This is the most common interoperability failure in calendaring, and it is a data-loss bug that no
test of our own model will ever catch.

The alternative — refusing to parse anything unrecognized — is worse. Calendars in the wild
violate the specification constantly, and a parser that rejects them is a parser nobody can use.

The amendment of 2026-08-10 changes none of that. It closes four places where this document stated
a principle and left the mechanism to be inferred — what text is stored as, what the tree is made
of, what an accessor hands back, what a mutation may touch — each of them satisfiable by a design
that loses data.

The second amendment of the same date closes a fifth, found by attacking the implementation rather
than the prose: the write side. "A write reaches nothing else" was stated about the model and
measured against the model, and three scoped writes turned out to reach past it in the *octets* —
a parameter assignment carrying a terminator, a parameter value carrying a `:`, and an addition
written after a line that had never been terminated. Two are refusals and one is a single octet
this document now names rather than lets a reader discover; all three are in the Consequences
below.

## Decision

The parsed model preserves everything. Unknown properties, unknown parameters, unknown components,
and the original text of values we do not interpret are all retained in position, and
serialization writes them back.

Typed access is a *view* over preserved content, not the storage. A `DTSTART` accessor returns a
parsed date-time; the underlying property keeps its original text, parameters, and ordering. Where
a value cannot be reparsed to an identical byte sequence — floating point in `GEO` is the obvious
case — the original text is what gets written, and the typed accessor is derived from it rather
than replacing it.

Storage is owned bytes end to end (`Box<[u8]>` and wrappers over it), never `&str` or `String`.
Content-line unfolding is a pure byte-level operation with no UTF-8 awareness: a fold may legally
split a multi-byte codepoint under RFC 5545's octet-boundary rule, and unfolding reassembles the
bytes without validating them. Only a typed, `str`-returning accessor attempts decoding, and on
invalid bytes — a CP1252 `SUMMARY`, a corrupted export — it reports a diagnostic carrying the
failing byte offset, never a panic and never a lossy replacement, exactly as the `GEO` float case
is handled. That is what lets such payloads round-trip byte-identically: nothing between the fold
and the accessor demands validity. The same treatment applies to parameter text, not only to
property values.

The document is one ordered heterogeneous tree and nothing else: `RawText` is a newtype over
`Box<[u8]>`; a `Parameter` is two `RawText` fields, name and value; a `Property` is a name, an
ordered `Vec<Parameter>`, and a `value_text`, and carries no typed-value field; an `Item` is
either a `Property` or a `Component`; a `Component` is a name and an ordered `Vec<Item>`. There is
no known/unknown split and no keyed map as primary storage. Typed accessors decode `value_text` on
each call and cache nothing on `Property`, so "a view, not the storage" holds by the absence of a
second field that could disagree with the first rather than by contributors remembering not to let
it. These types are concrete, non-generic, and never boxed as `dyn Trait`.

Concretely: every typed accessor over a scalar property returns one shape, distinguishing three
states — absent, present-but-malformed carrying a diagnostic, and present-and-valid — and every
accessor uses the same wrapper for that trichotomy; `dtstart()` and `geo()` may not differ in
shape from each other or from any property added later. The returned value borrows from the
component rather than owning a copy, so "keeps its original text" is enforced by a lifetime.
Cardinality declared by the specification is a claim about well-formed input, not a property of
the documents this library is actually handed. Every property name is therefore reachable through
the same iterator-backed lookup over the preserved properties, whether or not the specification
limits how often it may appear; the singular typed accessors are a convenience layered over that
lookup, never the only route to a value. When a property the specification declares at-most-once
occurs more than once, its singular accessor resolves to the present-but-malformed state carrying
a duplicate-occurrence diagnostic. It never picks a winner silently, and no fourth state is
introduced: "I cannot hand you one trusted value here" is what malformed already means, and the
diagnostic names the occurrences it saw so the caller can reach them through the general lookup.
Two `GEO` lines, or a floating `DTSTART` followed by a zoned one an hour off, are refusals with
evidence rather than a coin flip.

Round-trip fidelity is a tested property, not an aspiration: parse then serialize is
byte-identical for the whole conformance corpus, drawn from real client exports and carrying the
boundary cases this document names: a CP1252 `SUMMARY`, a fold that splits a codepoint, a
duplicated `GEO`. A second corpus test covers the path the first one misses — parse, write one
scalar property through its typed setter, reserialize, assert every other property's serialized
bytes are unchanged.

## Consequences

The model is larger and less convenient than a struct of known fields. That is the price of not
destroying other people's data, and the typed accessors exist to hide it for callers who only want
the common properties. Text accessors are fallible on read, and decoding is lazy, so invalid bytes
in a property nothing reads survive parse, round trip, and iTIP processing with no diagnostic
raised. Byte-identical round trip and always-visible violation are in tension; we deliver the
first fully and the second only where a caller looks. An eager parse-time sweep over text
properties would close that gap cheaply and is a named follow-up, not a rejected idea.

Mutation has to say what it means. Changing a `DTSTART` invalidates the preserved text for that
property and nothing else; the API makes that boundary explicit rather than regenerating the whole
component. That clause is a statement about storage, and it is not a claim that the document still
means what it meant. Preservation is a byte-level guarantee, not a semantic one. Every property
the caller did not name — including vendor properties whose semantics this library does not know —
keeps its original bytes and may now contradict the edit. `X-MICROSOFT-CDO-ALLDAYEVENT:TRUE`
surviving verbatim next to a `DTSTART` that a typed setter just made timed is this design working
as specified, not a bug in the tree. The library will not rewrite, delete, or reorder a property
the caller did not name. What it will do is say so: an entailment audit over a component reports,
as ordinary diagnostics, the known relationships an edit has broken — the all-day CDO pair against
`DTSTART`'s value type, `DTEND` against `DURATION`, `RRULE`'s `UNTIL` against `DTSTART`'s form and
zone. That audit is a finite list against an infinite problem, silent on the next vendor extension
whose meaning is private or newer than this code; it narrows the silent-corruption class, it does
not eliminate it. It is also advisory, because running it at serialize time would mean
serialization can refuse, which contradicts this document's whole posture — so a caller who never
asks ships the corrupt file, and no gate here can catch that. If callers turn out to skip it, the
honest fix is a serialize path that returns diagnostics alongside bytes, and that API shape is not
decided here.

Mutation is scoped by a short-lived handle rather than a marker value: `dtstart_mut()` returns a
guard borrowing `&mut Component` for exactly the one property its own signature names, and only a
write through that guard discards that property's preserved text. The borrow checker enforces the
boundary, not a marker the caller may drop. The unit the guard scopes is the whole property — its
name, its parameters, and its value together — not the value alone. `T` in `PropertyMut<'_, T>`
names the typed view a caller writes through; it does not bound the guard's reach. A write may,
and where RFC 5545 requires it must, rewrite that property's own parameters; it still reaches
nothing else. So that this does not become a per-property judgment call in a hand-written setter
body, every typed value used as `T` is total over the parameters the specification makes a
function of the value's shape: `DateTimeValue::Date` carries no zone, `Zoned` carries its `TZID`,
`Utc` carries neither, and a date-time cannot be constructed apart from the parameter set it
implies. Emission of those coupled parameters (`VALUE`, and `TZID` on date-time properties) is
derived from the value written and never carried over from the text replaced, while parameters
that are not a function of the value — `RANGE`, `FBTYPE`, `X-` parameters — are the caller's and
survive untouched. Which parameters are derived, for which property, is one transition table,
checked for completeness against the set of typed accessors. Converting a zoned `DTSTART` to a
date therefore emits `DTSTART;VALUE=DATE:20260815` and drops the stale `TZID`, rather than the
syntactically invalid pairing a value-only guard would leave behind.

There is exactly one octet a scoped write puts outside the property it names, and naming it is
cheaper than leaving a reader to find it. A final content line often arrives with no terminator,
and it is written back with none, because appending one would add an octet the file did not have.
That reasoning holds for as long as the line is last. An addition placed after it makes it not
last, and RFC 5545 section 3.1 delimits content lines with `CRLF`: written unchanged, the two
would serialize as one line, the addition would not exist on the next read, and the property above
it would come back carrying the addition's octets glued to its value — which is data loss arriving
through the mechanism this document exists to prevent it through. So the line above an insertion
gains the terminator at the moment the insertion creates the need for it, and at no other moment;
a terminator already there is kept as it is, bare `LF` and bare `CR` included. The alternative
that keeps this document's sentence literally intact is to refuse the addition instead, which
makes whether a calendar can be added to depend on a property of the file the caller did not
choose and mostly cannot see. Both are recorded in `ical-conform` against section 3.1, and the
line above keeps its name, its parameters, its value, and its position either way.

The write side is also where "preserves everything" stops being the whole rule, because a value a
caller hands over has no producer whose spelling to preserve. `PropertyMut::set_raw` refuses
control characters, and that refusal is only true if it is the only door: every unchecked setter
on `Property` is therefore crate-private, since a check repeated on each of them would still leave
a handed-out `&mut Vec<Parameter>` that no check can stand in front of. A parameter value is
written in the section 3.2 spelling its own octets require — quoted where `SAFE-CHAR` excludes
what it carries, refused where `QSAFE-CHAR` has no spelling at all. Refusal on the way *in* would
contradict this document; refusal on the way *out*, of octets that were never read from anywhere,
costs it nothing.

A calendar that violates the specification still parses, and the violation is reported as a
diagnostic attached to the item rather than an error that discards the file. A caller that wants
strictness asks for it.

What this costs, plainly. Decoding on every accessor call is redundant work a cache would avoid,
and the audit pays it again over more properties; if those accessors are ever measured hot, the
recorded fallback is a cached typed value with a type-level invalidation mechanism — a `OnceCell`
bound to the same `RawText`, or a generation counter — never a bare second field. Requiring the
general lookup for every property name rather than only `X-` names raises the floor on what must
be enumerable at once, at a memory cost that stays undefined until the concrete span type is
written; a 400 MB file on a constrained device may yet push back on that sentence. A guard cannot
be stored, batched, or carried across a request boundary, which is an ergonomic loss and an
unsettled collision with ical-itip's need for an inspectable change value. "Absent" still
conflates an optional property nobody set with a mandatory one nobody wrote, and the duplicate
rule says nothing about the same defect one level down, where a property carries two `TZID`
parameters. A future `*_mut` body can still reach past its own storage, policed by the corpus test
and not by the compiler; the audit itself trusts span-cutting it cannot verify. A reasonable
reviewer would have decided two of these differently — uniform `Result` accessors, and a
parameter-granular dirty flag answering the invalidation-scope question deferred to ADR-0004 — and
both remain live. None of it has been compiled: `ical-core` is doc comments and `#![no_std]`, so
every mechanism above is a promise about a repository that does not yet exist.

## Amendments

Four sentences above were written before the code was, and each is amended here rather than
quietly reinterpreted. Every one of them was found by an adversarial pass against M0, and each
has a conformance case in `ical-conform` addressed to the RFC section it comes from.

**1. The serializer writes one octet no line stored, and it is not only an insertion that
needs it.** The paragraph above scopes the terminator a line owes to `Component::apply`'s
insertion path, which is where a caller most often creates the need for one. It is not the only
path there. A property read out of a truncated export carries a layout with no terminator, is
`Clone`, and is reachable through `Document::items()`; pushed above another line through
`Component::items_mut`, it stores two content lines and writes one, with the second line's
octets glued to the first one's value and nothing reported. So the rule is stated at the
serializer instead: a stored line that carried no terminator and is not the last thing written
is written with the one section 3.1 requires. A line that is still last is still written without
one, so a file that ended mid-line still does, and for every document that was parsed the flag
is never set — a parse can only produce an unterminated line as the last line of its input.

**2. The write side refuses a line that is a component boundary.** "Refusal on the way out, of
octets that were never read from anywhere, costs nothing" was stated over the grammar's own
predicate, which asks whether a name reads back whole. `BEGIN` and `END` read back whole and
read back as something that is not a property: a line named either of them opens or closes a
component for whoever reads the file next, so a write that authored one would restructure the
document through a door that names one property. `Property::create`, `PropertyMut::set_raw`,
`PropertyMut::set` and three of `Component::apply`'s four variants refuse it as
`MutationError::ComponentBoundary`; `ProposedChange::Remove` does not, because removing a line
is not authoring one. The reader still stores such a line — section 3.6 recovery keeps a
mismatched `END` as a property — so this costs the round trip nothing and costs the caller the
ability to edit that one property's value, which is the honest price. A caller wanting a
component calls `Component::create`, which writes both of its lines.

**3. A parameter value is written in the spelling RFC 6868 gives it, not only the one section
3.2 does.** "Refused where `QSAFE-CHAR` has no spelling at all" was true of section 3.2 read
alone and is not true of the crate, which reads and writes RFC 6868's caret encoding in both
directions. A door that takes a value and picks its spelling — which quoting already made it —
owes the whole spelling: a `DQUOTE` is written `^'`, a newline `^n`, and a `^` is written `^^`,
without which a value the caller spelled `^n` comes back a newline from this crate's own codec.
What has no spelling under either grammar is still refused, which is every control character
RFC 6868 gives no pair, `CR` included — so the injection refusal is unchanged. The consequence
worth stating: these doors take a *value* rather than a spelling, so a caller moving a parameter
from one line to another resolves it with `decode_caret` first.

**4. The entailment audit is what M0 built, and it is one of the three this document names.**
`Component::audit` reports `DTEND` against `DURATION` in a `VEVENT` and `DUE` against `DURATION`
in a `VTODO`, as `DiagnosticCode::MutuallyExclusiveProperties`. The other two named above are
not built. Both turn on a *value* rather than on a pair of names — the all-day CDO pair against
`DTSTART`'s value type, and `RRULE`'s `UNTIL` against `DTSTART`'s form and zone — and the second
needs the section 3.3.10 grammar `ical-recur` owns, so it belongs to M1. Section 3.6.6's
`DURATION` and `REPEAT`, which must appear together or not at all, and section 3.6.2's
`DURATION` needing a `DTSTART` to measure from, are the same kind of claim and are also unbuilt.
The audit was already advisory and already "a finite list against an infinite problem"; what
this amendment adds is which finite list, so that a reader of `schema.rs` is not deferred onto
an audit that does not make the claim.

**5. There is a second write door, and it addresses an occurrence.** Amendment 2 and the
paragraphs above are stated over `Component::apply`, whose four variants each reach every
property carrying the identity they name. That is the right rule for a caller naming an
identity and it is not a rule about occurrences, which are a different address and now have
`Component::apply_to_occurrence(&PropertyId, usize, &ProposedChange, Limits)`. Every refusal
this document names still stands in front of it — the component-boundary refusal, the
control-character refusal, the per-value ceiling, the parameter spelling of amendment 3 — and
it shares `Component::apply`'s own reader, so a replacement is read as the content line it is.
The one rule it adds is about `ProposedChange::Add`, which has no occurrence yet: its index
must be the append position, since an addition landing elsewhere would renumber every
occurrence after it and make a transition keyed on those numbers mean something else.
`ical-itip` needs this and nobody else does, which is the honest description of why it exists.

**6. The claim is over the octets this workspace was handed, and the CalDAV envelope is where
that stops being the same thing as the octets a producer wrote.** Every sentence above is
about a parse and a serialize of one byte string, and M4 put a second layer between the
producer and this crate. XML 1.0 section 2.11 requires a conformant processor to fold every
`CRLF` and every lone `CR` to `LF` before parsing, and RFC 5545 section 3.1 makes that same
`CRLF` the syntax of a content line — so a conformant read of a `CALDAV:calendar-data` element
hands `ical-core` a calendar whose terminators the server did not write.

[ADR 0004](0004-sans-io-protocol-layer.md)'s Amendment 1 resolves that in `ical-dav`, and the
resolution keeps this document's claim intact rather than narrowing it: the reader departs from
section 2.11 inside that one element, so what reaches `Document::parse` from a multistatus is
what the server sent, and `parse -> serialize` over it is byte-identical exactly as it is over
a file read from disk. `ical-conform` needs no rule separating a DAV-sourced case from an
ICS-sourced one, and no gate has to enforce one.

What this amendment adds is the boundary that resolution does not reach, because it is not
`ical-dav`'s to reach. RFC 4791 section 9.6 explicitly permits a server to omit the `CR`
inside `calendar-data`, on the grounds that XML parsers fold it anyway. A server that does is
conformant, the calendar arrives with bare `LF` terminators, and nothing in the octets says
whether that is what the producer wrote or what the protocol was allowed to drop. This
document's guarantee is that nothing here loses a byte it was given; it has never been, and
under section 9.6 cannot be, a guarantee that the bytes given are the bytes authored.
`CalendarPayload::is_as_sent` is how a caller reads which of the two it is holding, and a
`bare-line-feed` diagnostic is how the grammar reports the terminators either way.

**7. One class of file this document guarantees has no CalDAV representation, and that is the
envelope's limit rather than this document's.** Amendment 6 recorded where "the octets this
workspace was handed" stops being "the octets a producer wrote". M4's own attack found the
sharper case, going the other way: a file this workspace reads and writes byte for byte that
cannot be *put on a CalDAV wire at all*.

An RFC 5545 fold may fall between the lead octet of a multi-octet character and its
continuations. Real exporters emit that, `crates/ical-conform/tests/fixtures/break_grammar/
fold_inside_utf8.ics` is the case, and every claim above holds over it: `parse -> serialize` is
the identity. The octets are not valid UTF-8 — the fold sequence sits inside the character — and
an XML document declares an encoding. A `<C:calendar-data>` element carrying them makes the
whole multistatus one that no conformant processor will parse: the peer loses the entire
response, not one property, and nothing on the wire says why. No escaping helps, because a
character reference names a code point and these octets are not one.

`ical-dav` therefore refuses to write such a payload
([ADR 0004](0004-sans-io-protocol-layer.md) Amendment 7) rather than emitting a body the peer
discards. Nothing above is narrowed: the file still round-trips through this workspace, and a
server storing it can still serve it over any transport that carries octets. What is recorded
here is that CalDAV is not such a transport, that the boundary is the XML envelope's and not
the grammar's, and that a caller meeting `ValueError::NotUtf8` on a write is meeting a real
property of the protocol rather than a defect in this crate.

The matching hole on the same wire is named rather than closed: `DAV:href` is byte-shaped by
this workspace's own decision — a type that cannot model a response one can read is the failure
this document exists to prevent — so a path a store holds that is not UTF-8 is still written
through, and a body carrying one is still a body a conformant peer refuses. Percent-encoding on
the way out without decoding on the way in would break the round trip, and decoding would erase
the difference between `%2F` and `/`. Nobody has designed the third answer.
