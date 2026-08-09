# ADR-0004: the CalDAV layer is sans-I/O and `no_std`

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10

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

The principle above stood unchanged through the design bake-off. What it lacked was a
mechanism specific enough to hold code against, so the four sections below are now part of
the decision.

### The grammar seam, and where diagnostics live (DP-17)

The spine stands: `ical-core` at the bottom, `ical-recur` and `ical-tz` as orthogonal
siblings above it, `ical-itip` above both because it needs both, `ical-dav` beside core as a
leaf, `ical-conform` on top. One seam is added below all of it. `ical-grammar` holds the
content-line grammar — unfolding, lexing, escaping, and parameter structure — and depends on
nothing. `ical-core` depends on it and adds the object model, the typed views, and
serialization. A grammar-only consumer — a linter, a diff or merge tool, a fuzz harness —
sheds the typed model: `CivilDate`, `EditSet`, and the typed accessors. It does not shed
diagnostics, and it was never going to: a violation of the grammar is detected by the
grammar, and a value truncated at a limit loses bytes inside the grammar, which ADR-0001
requires be flagged where the bytes are dropped.

The diagnostic vocabulary therefore lives at the bottom of the stack rather than in the
middle of it. `Diagnostic`, `DiagnosticKind`, `Severity`, and the sink they are reported
into are defined in `ical-grammar`; `ical-core` re-exports them unchanged and adds only the
kinds it alone can detect. There is no second diagnostic type and no wrapping layer at the
seam, so "diagnostics travel with the item they concern" survives the split as one
vocabulary rather than two that must be reconciled.

The crate table in ARCHITECTURE.md gains an `alloc` column beside `std`, because `no_std`
alone did not capture the wiring that actually broke: a panel proposal's
`Vec<Response>: Slots<Response>` failed to compile at the `ical-core`/`ical-dav` seam under
`alloc:false`, and no manifest diff can see that. Every crate therefore carries a compiled
minimal-usage example, built in CI at its declared `alloc` setting, so a break of that class
fails at the seam where it occurs instead of waiting for whoever next tries to use the crate.

### What the purity gate actually proves (DP-18)

The gate proved two textual facts: that no forbidden dependency key appears in a manifest,
and that the string `#![no_std]` appears in a `lib.rs`. Neither is the claim. The claim is
about the packages a core crate links, and a manifest states that only when it is read for
the name Cargo would resolve rather than the name somebody wrote.

A dependency's key in a manifest is a nickname its author chose; the name Cargo links is the
`package` field. For each of {`ical-grammar`, `ical-core`, `ical-recur`, `ical-tz`,
`ical-itip`, `ical-dav`}, `xtask purity` reads every dependency entry — normal, dev, build,
and `[target.'cfg(..)'.dependencies]` alike, in the inline-table spelling and in the
`[dependencies.name]` sub-table spelling — for that field, and reports the name it finds
rather than the key above it. The rename is a violation on its own, because a rename exists
only to make the linked name differ from the written one. One added line in `ical-tz` —
`ical-dav = { package = "libm", version = "0.2" }` — passed every leg of the old gate, the
manifest scan, the `#![no_std]` match, and both cross-target builds, while `cargo tree`
showed the real edge; a fixture reproducing that exact line is now a regression test.
Provenance is checked alongside identity: a core crate's dependency must come from inside
this workspace — `workspace = true`, or a `path` under `crates/` — and a version or registry
source is a violation whatever the package is called.

Two more facts are checked because this ADR's own rationale invokes them. `just no-std` and
`just wasm` build the core for `thumbv7em-none-eabi` and `wasm32-unknown-unknown`, and both
are required CI jobs, so the portability claim is verified rather than asserted. And a crate
under `crates/` that declares `#![no_std]` while absent from CORE_CRATES fails the gate,
because the list of crates the rule governs must not be able to go stale behind a new crate;
the exemption `ical-conform` holds is "not `no_std`", not "not listed". `ical-grammar`
arrived under exactly that rule, which is the first thing the amendment was used for.

### XML inside `ical-dav` (DP-14)

The XML syntax inside those requests and responses is `ical-dav`'s to own, not an outside
dependency's. The purity gate forbids `quick-xml`, `roxmltree`, and every other outside XML
crate, so `ical-dav` hand-rolls a tokenizer restricted to the closed element vocabulary
CalDAV and WebDAV actually use — the `DAV:` and `urn:ietf:params:xml:ns:caldav` tables, no
DTD, no external entities, no processing instructions, UTF-8 only, size and depth bounds per
ADR-0002's posture toward hostile input. It stays inside `ical-dav` rather than a sibling
crate or a shared `webdav-core` until a second DAV-shaped consumer — CardDAV, WebDAV-sync —
exists in this workspace to justify the extraction; that extraction is a deferred cost, not a
rejected one. Three obligations are load-bearing and enforced in code, not documented as
intent.

The size and depth bounds are checked on every event rather than declared as a struct field,
and the tokenizer is an iterative state machine, so adversarial nesting costs no stack.

Unknown elements and namespaces are skipped rather than rejected, since RFC 4918 requires
clients to tolerate server extensions. The vocabulary is closed over resolved (namespace URI,
local name) pairs, never over prefixes. This crate therefore carries real XML-namespace
machinery: a scoped stack of prefix-to-URI bindings, `xmlns:p=` and default `xmlns=`
declarations honored and unbound at the end of their element, resolution performed at every
start tag, end tag, and attribute. `<d:multistatus xmlns:d="DAV:">`,
`<multistatus xmlns="DAV:">`, and `<D:multistatus xmlns:D="DAV:">` are the same element, and
`<D:multistatus xmlns:D="http://evil.example/not-dav">` is not that element and is never
treated as one — it is skipped as unknown, like any other foreign vocabulary. Matching prefix
strings would silently drop the first two shapes, which the most widely deployed servers
emit; matching local names alone would accept the fourth, which is namespace confusion.

The tokenizer obeys XML 1.0 section 2.11 like any conformant processor: every CRLF and lone
CR becomes LF before tokenizing, inside CDATA and character data alike. Byte-exact round trip
therefore does not survive the XML envelope, and this ADR says so rather than asking for a
carve-out that cannot be satisfied. What survives is the logical content line: calendar-data
recovered from a multi-status reaches `ical-core` with bare LF terminators, which `ical-core`
accepts, and is re-terminated with CRLF when written back out. Escaping on the writing side
is ours to get right — a literal `]]>` inside a `DESCRIPTION` is ordinary calendar text, and
a CDATA writer must split it (`]]]]><![CDATA[>`) or escape the content rather than emit a
section that ends early.

### One shape for both directions (DP-15)

The types are symmetric, and the symmetry that matters is between producing and consuming,
not between client and server.

`CalendarQuery { props: Vec<PropName>, filter: Option<CompFilter> }` is the REPORT body a
client writes and a server reads. The time range inside `CompFilter` is
`TimeRange { start: Option<Instant>, end: Option<Instant> }` — two independently optional
bounds, because RFC 4791 section 9.9 permits an open start or an open end and a single
`Option<(Instant, Instant)>` cannot say that. A response is
`DavResponse { href: String, propstats: Vec<PropStat> }` with
`PropStat { status: u16, props: Vec<(PropName, PropValue)> }`, because one `href` carries
divergent per-property statuses — `getetag` at 200 beside `calendar-data` at 404 — which a
flat `status: u16` cannot express. `MultiStatus` holds its `responses` privately behind a
capped push rather than exposing a `pub Vec`, since a public field is not a choke point.

Those owned types are not the ingestion primitive. A client builds a `calendar-multiget`
whose `href` list can legitimately run to tens of thousands of entries, and a server parses
that same untrusted body: each side both produces at collection scale and consumes hostile
bytes. Reading a multi-status is therefore an incremental decoder that yields one
`DavResponse` at a time and holds only that one; writing a multi-status is an incremental
encoder that emits one `DavResponse` at a time into a caller-supplied sink. Both are defined
for both sides, and materializing a whole `MultiStatus` is one optional consumer of the
decoder, not the only way to read one. The cap belongs to that consumer — a caller's decision
about its own buffer — and not to the protocol, which is how a server enumerates a
40,000-resource collection and an embedded client refuses the same response without either
one breaking a rule the wire format never had.

The envelope is opaque to iCalendar internals by design and not by omission: `calendar-data`
is carried as opaque bytes and parsed by `ical-core`. Depth bounding of recursive
`CompFilter` on a server's parse of an untrusted REPORT body, and the HTTP envelope itself —
`Depth`, method, `Content-Type` — are out of scope here and tracked as follow-ups rather than
quietly treated as solved.

## Consequences

Nobody gets a one-line "fetch my calendar" function from this workspace. That belongs in a
thin adapter crate against a specific HTTP client, which anyone can write and which is not
this workspace's problem to choose.

Testing the protocol layer needs no server: a request is a value and a response is a byte
string, so an interoperability case is a recorded exchange rather than a live connection.

Server implementations get the parsing side for free, which is the half that does not exist
in Rust at all today.

Extracting `ical-grammar` weakens its own justification. The crate now carries the diagnostic
vocabulary and the sink as well as the tokenizer, so the compile-footprint saving that was
the argument for the seam shrinks — and byte-identical re-serialization already demands
positional and formatting fidelity from the grammar layer, which made that saving look
overstated before this amendment raised it further. The seam is insurance, not demonstrated
demand. If no real caller ever wants grammar-without-model, the honest move is to collapse
`ical-grammar` back into `ical-core` before 1.0, and nothing here decides that.

Where tree construction lives is still unstated. Unfolding and content-line lexing belong to
`ical-grammar` and the typed view belongs to `ical-core`, but a `BEGIN`/`END` nesting
mismatch — a `VEVENT` closed by `END:VTODO` — is a structural violation that needs no typed
interpretation and is not content-line grammar either. Whichever crate owns it also reports
it, and that is the same un-placed responsibility this section exists to fix one layer down.

The `alloc` column makes the `ical-core`/`ical-dav` wiring visible at the seam; it does not
decide what `ical-dav` holds when `alloc` is off. The incremental decoder is most of that
answer, and whether an owned `MultiStatus` exists at all on such a target is unanswered.

The mechanical result of the bake-off was that a ten-crate decomposition beat this spine by
more than the replacement margin, and it was not adopted because the margin came bundled with
`vcard`, `carddav`, and `jscal` siblings — a change to what this workspace is for, never
scored on its own. An adjudicator who disagrees should adopt the full graph and justify the
product-scope expansion in its own ADR rather than let it ride in on this one.

The purity gate reads manifests, never resolution and never the artifact. A `[patch]`
section, a `[source] replace-with` in `.cargo/config.toml`, a vendored registry, or a
poisoned lockfile can still make a correctly declared name describe different bytes. Closing
the first of those needs `cargo metadata`, whose output is JSON and whose reader would be
this tool's first dependency — a cost this decision declines rather than hides — and closing
the rest needs artifact-level evidence, the emitted rlib's extern crate list or a locked
offline build from a verified vendor tree, which nothing here proposes. By the same token the
rule asks only whether a dependency is ours, never whether ours is correct: a path dependency
under `crates/` is trusted by construction.

Coverage is the manifest's, not the graph's. Every dependency table in a core manifest is
read whatever `cfg` or feature gates it, which is more than a feature-matrix walk would give
and less than resolution: a package reaching a core crate through another workspace member is
invisible here, and so is anything a build script pulls in at build time. And the gate has no
custodian of its own — `xtask` is governed by no purity rule, so the same pull request that
breaks the rule can weaken the check that enforces it, and `ical-conform`'s exemption is
still just absence from a hand-maintained const.

Two costs of the zero-dependency rule stand unpaid. `ical-tz` and `ical-recur` compute over
data that has already been parsed, yet they live under the same rule as the crates that read
attacker-controlled bytes, with a different real justification and one blanket answer for
both. And the rule's near-unanimous support in the bake-off was support on supply-chain
grounds only: no blind architect reached zero-allowlist unprompted, no proposal argued it on
conformance evidence — `RANGE=THISANDFUTURE`, a `VALUE=DATE` `UNTIL`, a negative `BYSETPOS`,
a fold splitting a codepoint — and a discipline sold as needing no judgment calls turned out
to need one, since a single ordinary line of Cargo.toml defeated every leg of it for the
gate's entire life.

Deferring the shared `webdav-core` extraction costs more than it did. What is being kept
private is now a namespace-resolving, reference-resolving reader and writer rather than a
small tag matcher, and it is what CardDAV or WebDAV-sync would have to migrate on the day
either arrives. The deferral stands, and the panel's top-scored reading — extract now, before
external users depend on `ical-dav`'s internals — is the first thing to revisit if CardDAV is
closer on the roadmap than this assumed.

The fidelity retraction is scoped to this ADR and nowhere else. ADR-0001 still says
byte-identical and ADR-0006 still says round-tripped byte-for-byte, both without
qualification, and ADR-0006's corpus is exactly where a recorded DAV exchange would be
compared. Nothing tells that harness yet that a DAV-sourced case compares logical lines while
an ICS-sourced case compares octets, and no gate enforces the distinction. The repair also
rests on a decision this ADR does not own — `ical-core` accepting a bare LF as a line
terminator — so it is filed as a conformance case addressed to `ical-core`, which is a claim
on the corpus and not a gate; a strict-CRLF unfolder is a defensible reading of RFC 5545
section 3.1 and would invalidate the repair silently.

The XML audit is unfinished. Newline normalization is one place where XML's own rules bite
calendar data; attribute-value normalization, `xml:space`, and whitespace in element content
have not been swept, nor has the question of whether any of them touch an `href` or a
`getetag`. The size cap is likewise untuned: one fixed number either rejects a legitimate
large multiget response or is loose enough to weaken the memory guarantee, and the binding
stack now draws on the same budget without anyone having named the numbers.

The incremental codec pair is load-bearing and has never been compiled. It is the shared
encode/decode trait this decision listed as debt, promoted to prerequisite, and the one
proposal that attempted something like it failed `cargo check` on an `alloc` contradiction
and scored last. Whether a dyn-safe sink-driven encoder and a pull decoder are expressible in
`ical-dav` under `#![no_std]`, a zero-dependency gate, and `alloc:false` on
`thumbv7em-none-eabi` is genuinely unknown; if they are not, this section reopens rather than
degrading gracefully. That the trait-object design is untested rather than refuted is the
fairest reading of the bake-off, and it is not evidence in its favor.

Streaming removes the memory amplification and leaves the work amplification. A client that
decodes 40,000 `href`s one at a time still has 40,000 resources to fetch and 40,000 events to
evaluate, and that fan-out loop lives in the application, outside every crate this workspace
ships. Nor is content truthfulness any more decidable: a caller still cannot distinguish a
real 40,000-event collection from a forged one, and the amendment only makes that distinction
unnecessary for staying inside memory, not available for deciding whether to believe a
server.

Two protocol debts are untouched and one is sharper for the change. `CompFilter` recursion
depth on a server's parse of an untrusted REPORT body is still unbounded in the decision, and
a streaming decoder does not bound nesting depth for free. The HTTP envelope is still
unmodeled, which now matters more: a producer that hits its own resource wall mid-encode has
no in-scope way to say so, because `507` lives at the layer this ADR declines to describe.
