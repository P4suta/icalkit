# ADR-0001: unknown properties and components survive a round trip

- Status: accepted
- Date: 2026-08-05

## Context

Every real calendar file contains things this library has never heard of. Vendor extensions
(`X-MICROSOFT-CDO-BUSYSTATUS`, `X-APPLE-STRUCTURED-LOCATION`), properties from RFCs
published after this code was written, parameters on properties we do parse, and whole
components we have no model for.

The default behavior of a typed parser is to discard those, because they do not map onto
any field. The consequence appears the first time two clients touch the same event: one
client writes it, another opens and saves it, and information the first client depended on
is silently gone. This is the most common interoperability failure in calendaring, and it
is a data-loss bug that no test of our own model will ever catch.

The alternative — refusing to parse anything unrecognized — is worse. Calendars in the
wild violate the specification constantly, and a parser that rejects them is a parser
nobody can use.

## Decision

The parsed model preserves everything. Unknown properties, unknown parameters, unknown
components, and the original text of values we do not interpret are all retained in
position, and serialization writes them back.

Typed access is a *view* over preserved content, not the storage. A `DTSTART` accessor
returns a parsed date-time; the underlying property keeps its original text, parameters,
and ordering. Where a value cannot be reparsed to an identical byte sequence — floating
point in `GEO` is the obvious case — the original text is what gets written, and the typed
accessor is derived from it rather than replacing it.

Round-trip fidelity is a tested property, not an aspiration: parse then serialize is
byte-identical for the whole conformance corpus, which is drawn from real client exports.

## Consequences

The model is larger and less convenient than a struct of known fields. That is the price of
not destroying other people's data, and the typed accessors exist to hide it for callers
who only want the common properties.

Mutation has to say what it means. Changing a `DTSTART` invalidates the preserved text for
that property and nothing else; the API makes that boundary explicit rather than
regenerating the whole component.

A calendar that violates the specification still parses, and the violation is reported as a
diagnostic attached to the item rather than an error that discards the file. A caller that
wants strictness asks for it.
