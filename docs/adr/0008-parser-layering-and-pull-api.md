# ADR-0008: the token layer is the parser, and the document tree is one consumer

- Status: accepted
- Date: 2026-08-10

## Context

[ADR 0001](0001-lossless-round-trip.md) settles what parsing preserves and says nothing about how
parsing is staged. The crate documentation implies four steps in prose, but no step is a named type,
nothing says whether unfolding is observable, and a streaming API is never contrasted with the
document API at all.

Two callers pull in different directions. An application wants a tree it can walk and edit; a
device with 64 KB of RAM wants to read a calendar arriving over CalDAV without ever holding it —
the bounded, sans-I/O posture [ADR 0004](0004-sans-io-protocol-layer.md) already committed to one
layer up. Document-only serves the first and makes the second impossible.

Keeping the streaming layer private under `Document::parse` and exposing it later, if anyone asks,
is the tempting answer and the one rejected here. "Later" is how a codebase acquires two parsers
with one name: the document builder takes raw bytes because that was convenient, the public lexer
grows its own grammar, and the divergence surfaces as a corpus case that one path accepts and the
other does not. Prose cannot prevent that fork; structure can.

## Decision

Parsing is four layers with public types: unfold, content-line lex, token or tree build, typed view.
The token layer is the mandatory foundation, `Token<'a>` and a source trait that yields it, and the
document builder is one consumer of that same public path rather than a parallel implementation:
`Document::from_tokens` is the constructor `Document::parse` calls, with no private fast path.

`Token<'a>` is byte-shaped, never str-shaped. Every payload a token carries — property name,
parameter name, parameter value, property value — is `&'a [u8]`, and no layer at or below the
token boundary performs, requires, or implies UTF-8 validation; "content-line lex" names a grammar
operation over octets, not a decoding step. A str-shaped `Token` would force this layer either to
reject a CP1252 `SUMMARY` — contradicting the rule that a violation is a diagnostic attached to
the item, not an error that throws the file away — or to substitute U+FFFD, destroying the
byte-identical round trip that is M0's sole acceptance criterion over the real-client corpus of
[ADR 0006](0006-conformance-corpus-as-artifact.md). Neither is permitted. UTF-8 decoding happens
only in the typed view, where failure is a diagnostic and the preserved bytes are still written
back.

A token may deliver its value in more than one chunk, and a source may answer that it needs more
input, so a 400 MB inline `ATTACH` is never required to be resident for a token to exist; the
caller's limit meter is charged per appended chunk rather than per completed value. Two gates keep
both claims honest: the token layer compiles for `thumbv7em-none-eabi` without default features, and
a structural test proves `Document` is built from the public token path.

## Consequences

`Token<'a>` and the source trait are semver-load-bearing before M0 ships, so a new RFC 5545
construct needing a token variant is a breaking change rather than a private refactor. Byte-shaped
payloads also push work outward: every text-valued typed accessor is a fallible decode, and a
caller who expected `&str` gets `&[u8]`. That surface is larger than a str-shaped one and is also
the only shape M0 can pass with.

The tree builder is still unbounded, by design and unavoidably. ADR-0001's losslessness requires
keeping the original text, so a 400 MB value costs 400 MB in an owned tree. This ADR makes that
honest rather than fixed: an implied whole-crate guarantee becomes an explicit capability split in
which a 64 KB device may use only the pull path. A caller who wants a document view on a
constrained device is out of luck, and this says so instead of solving it.

Charging per appended chunk is an enforcement point, and the allocation-policy decision recorded
separately charges at node construction. Two documents naming different points for the same meter is
a conflict to reconcile at integration, not to assume away; whichever ships first will look right.

Dyn-safety is more urgent now and still unsettled. A need-more-input outcome and a chunk-carrying
associated type make the source trait harder to make object-safe, exactly when `ical-dav` and
`ical-recur` are most likely to want some pull parser without a generic parameter.

Cross-property gaps in the typed view stay out of scope and are worse than unfinished. `UNTIL`'s
value type agreeing with `DTSTART`'s, `RANGE=THISANDFUTURE` matched against the master, and
`X-MICROSOFT-CDO-ALLDAYEVENT` going stale after a timed edit are inter-property invariants no
layering choice and no token payload type can reach.
