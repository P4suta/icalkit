# ADR-0004: the CalDAV layer is sans-I/O and `no_std`

- Status: accepted
- Date: 2026-08-05

## Context

CalDAV is WebDAV with calendar semantics: `REPORT` and `PROPFIND` requests carrying XML
bodies, `ETag`-based conditional writes, and a synchronization token protocol. None of that
is HTTP transport — it is the shape of the request and the interpretation of the response.

A crate that bundles an HTTP client makes an irreversible choice for every user: which
client, which async runtime, which TLS stack. Calendar clients are exactly the applications
that already have all three and will not adopt a second set. Servers, meanwhile, need the
same request parsing from the other direction, which a client-shaped API cannot provide.

The same argument applies to the whole stack. Calendar UIs run in browsers, and an
embedded device rendering a schedule is a real deployment.

## Decision

Every crate here is `no_std`, performs no I/O, and opens no connection. `ical-dav` produces
requests and interprets responses; moving bytes is the caller's job, with whichever client
and runtime it already has.

Because the protocol layer is expressed as data in and data out, the same code serves both
sides: a client builds a `REPORT` and parses the multi-status, a server parses the `REPORT`
and builds the multi-status. There is no client-only shape.

`just no-std` builds the core for `thumbv7em-none-eabi`, `just wasm` builds it for
`wasm32-unknown-unknown`, and `just purity` fails on an outside dependency or a missing
`#![no_std]`. All three are required CI gates and pre-commit hooks.

## Consequences

Nobody gets a one-line "fetch my calendar" function from this workspace. That belongs in a
thin adapter crate against a specific HTTP client, which anyone can write and which is not
this workspace's problem to choose.

Testing the protocol layer needs no server: a request is a value and a response is a byte
string, so an interoperability case is a recorded exchange rather than a live connection.

Server implementations get the parsing side for free, which is the half that does not exist
in Rust at all today.
