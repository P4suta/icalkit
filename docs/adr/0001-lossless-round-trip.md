# ADR-0001: unknown properties and components survive a round trip

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10

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
