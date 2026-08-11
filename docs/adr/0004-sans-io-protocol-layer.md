# ADR-0004: the CalDAV layer is sans-I/O and `no_std`

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10, 2026-08-11

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
`alloc:false`, and no manifest diff can see that. No crate carries such an example, so that
class of break is still invisible until somebody wires two crates together and tries to compile
the result. What CI builds is `just no-std` and `just wasm` over the six core crates, which
proves the targets and says nothing about the seam. A compiled minimal-usage example per crate
at its declared `alloc` setting is the mechanism this paragraph wants and the workspace does not
have.

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
carve-out that cannot be satisfied. **This paragraph is reversed by Amendment 1 below, which
was written against the specification text and against three real servers' output rather than
against the reading recorded here.** What survives is the logical content line: calendar-data
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
`CompFilter` on a server's parse of an untrusted REPORT body was out of scope here and is not
any more: `read_comp_filter` counts its own nesting and refuses `LimitExceeded::Depth` at
`Limits::max_xml_depth`. No amendment below added that dimension — ADR 0010's decision named it
and `max_xml_depth` has been a field since before this crate existed — so what closed here is
the filter tree's second recursion, not the bound. `Depth` became a value under Amendment 3.
What is still out of scope, and tracked as a follow-up rather than quietly treated as solved,
is the rest of the HTTP envelope — the method, `Content-Type`, and the framing a header value
sits inside.

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

Where tree construction lives was left unstated and M0 placed it. Unfolding and content-line
lexing belong to `ical-grammar` and the typed view belongs to `ical-core`, and the `BEGIN`/`END`
stack went to `ical-core` with them: a nesting mismatch — a `VEVENT` closed by `END:VTODO` —
needs no typed interpretation and is not content-line grammar either, but it is the tree
builder's own stack that sees it, and `unmatched-end`, `mismatched-end-name` and
`unclosed-component` are how it reports. That the crate owning the construction also owns the
report is the rule this section wanted; what it did not settle was which crate, and the first
compile did.

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

The XML audit is unfinished, and one line of it is closed. Newline normalization is one place
where XML's own rules bite calendar data, and attribute-value normalization was the second:
Amendment 8 swept it, because a `comp-filter name="VE&#78;T"` selected a component here that no
conformant processor selects. `xml:space` and whitespace in element content have not been swept,
nor has the question of whether either touches an `href` or a
`getetag`. The size cap is likewise untuned: one fixed number either rejects a legitimate
large multiget response or is loose enough to weaken the memory guarantee, and the binding
stack now draws on the same budget without anyone having named the numbers.

The incremental codec pair is load-bearing and it compiles. It is the shared encode/decode trait
this decision listed as debt and then promoted to prerequisite, and the one proposal that
attempted something like it failed `cargo check` on an `alloc` contradiction and scored last. M4
answered most of what this paragraph asked: a dyn-safe sink-driven encoder and a pull decoder
are expressible in `ical-dav` under `#![no_std]` and the zero-dependency gate, and the
cross-target jobs build them. The `alloc:false` half was not answered and was not asked again —
this crate declares `alloc` — so what `ical-dav` holds on a target without one is still unknown,
exactly as the paragraph above it says.

Streaming removes the memory amplification and leaves the work amplification. A client that
decodes 40,000 `href`s one at a time still has 40,000 resources to fetch and 40,000 events to
evaluate, and that fan-out loop lives in the application, outside every crate this workspace
ships. Nor is content truthfulness any more decidable: a caller still cannot distinguish a
real 40,000-event collection from a forged one, and the amendment only makes that distinction
unnecessary for staying inside memory, not available for deciding whether to believe a
server.

One protocol debt is paid and one is sharper for the change. `CompFilter` recursion depth on a
server's parse of an untrusted REPORT body is bounded — `read_comp_filter` counts its own
nesting and refuses `LimitExceeded::Depth` on the way down, at the same number
`Limits::max_xml_depth` gives the tokenizer — precisely because a streaming decoder does not
bound nesting depth for free and the filter tree is a second recursion beside the element tree.
The HTTP envelope is still
unmodeled, which now matters more: a producer that hits its own resource wall mid-encode has
no in-scope way to say so, because `507` lives at the layer this ADR declines to describe.

## Amendments

**1. The reader keeps `calendar-data`'s line endings, and that is a stated, scoped departure
from XML 1.0 section 2.11 rather than a conformant read.** The DP-14 paragraph above chose the
conformant read and accepted the loss. M4 went back to the specification text before writing
the code, and what it found changes the arithmetic in both directions.

Section 2.11 says the processor "MUST behave as if it normalized all line breaks in external
parsed entities (including the document entity) on input, before parsing, by translating both
the two-character sequence #xD #xA and any #xD that is not followed by #xA to a single #xA
character". That is the rule the paragraph above read correctly. What it did not read is RFC
4791 section 9.6, which anticipates exactly this and says so: "Given that XML parsers normalize
the two-character sequence CRLF ... to a single LF character ..., the CR character ... MAY be
omitted in calendar object resources specified in the CALDAV:calendar-data XML element." So a
server that never sends a `CR` is conformant, no reader can recover what such a server never
sent, and CalDAV never promised end-to-end octet fidelity through this element. The second half
of the paragraph above — that byte-exact round trip does not survive the envelope — is true as
a statement about *the protocol* and stays true.

What is not true is that a normalizing read is therefore the right implementation. Section 9.6
permits a server to omit the `CR`. It does not permit a client to rewrite the line endings of a
resource it received intact, which is what a normalizing read followed by a `PUT` does — to
another client's data, with a changed `ETag` as the only trace. `SabreDAV` and Radicale both
write the `CRLF` octets literally; Calendar Server writes the `CR` as `&#13;`. Two of those
three lose data through a conformant reader and the third does not, and nothing about the
element tells them apart afterwards.

The decision, therefore:

*Reading.* Character data is normalized per section 2.11 everywhere except inside the elements
`ElementName::preserves_line_endings` names — today `CALDAV:calendar-data` and nothing else —
where the octets are handed back as they arrived. References are still resolved inside that
element, because a reference is markup and not a line break: `&#13;&#10;` and a literal `CRLF`
converge on the same two octets, which is what makes this one rule rather than two dialects.
Inside that element this reader is **not** a conformant XML 1.0 processor: two documents equal
as infosets come out of it as different octets, and it must never be used to canonicalize or to
verify signed XML.

*The way out.* `TextPolicy::Normalized` restores section 2.11 everywhere, at runtime rather
than behind a feature flag — a feature is unified across a dependency graph by the union rule,
so one crate in a build could otherwise change how another's calendars parse. Every payload
that loses a `CR` to it reports `DiagnosticCode::DavCalendarDataLineEndingsFolded`, because a
choice being available is worth nothing if taking it is silent.

*The witness.* `CalendarPayload` carries a `LineEndings` beside its octets, and
`is_as_sent()` answers the only question a caller writing the payload back actually has. This
is the part neither shape offered by the bake-off had: the loss is permitted by section 9.6 and
is invisible in the octets afterwards, so the type says which it is instead of leaving a caller
to assume.

*Writing.* Strictly conformant, and it needs no departure at all. A `CR` is written as `&#13;`,
which section 2.11 does not reach because a reference is resolved after normalization, so any
conformant processor recovers it. No `CDATA` section is ever emitted — it cannot carry a `CR`
past a conformant reader and it makes a literal `]]>` in a `DESCRIPTION` an escaping bug — and
`>` is escaped unconditionally so that sequence is unwritable by accident. This crate's output
is therefore readable, losslessly, by any XML parser; only its reader departs, for one element.

*What it costs.* The phrase "auditable against the XML specification" is false for one element
and a reader has to be told. A caller can ignore the witness, and ignoring it puts the silent
rewrite back — the type makes the loss visible, not impossible. `TextRun::Wire` needs the body
in one contiguous slice, so a chunked transport gives up the borrow and pays a copy. And where
the octets were already folded upstream, by section 9.6's own permission or by something in the
path, nothing here restores them; the carve-out preserves what arrived and claims nothing more.

The conformance case is `crates/ical-dav/tests/calendar_data_collision.rs`, against fixtures
shaped like all three servers' output, in all three of their namespace-prefix spellings.

**2. ADR 0001 is not narrowed by any of this, and the fidelity retraction the Consequences
above recorded is withdrawn.** That section said ADR 0001 "still says byte-identical ... without
qualification" and filed the mismatch as debt. Under Amendment 1 the round trip through the DAV
envelope is byte-identical over the octets this crate was handed, so the corpus needs no rule
distinguishing a DAV-sourced case from an ICS-sourced one, and no gate has to enforce a
distinction. What remains, and is recorded in ADR 0001's own register rather than here, is that
"the octets this crate was handed" is not always "the octets the server stored" — and that gap
is section 9.6's, not this workspace's.

**3. The header boundary is stated rather than left to be discovered.** This ADR said the caller
moves the bytes and left "which headers are the protocol's" unanswered, which the Consequences
called an open gap. It is closed by naming both sides. Protocol semantics, modeled as values:
`If-Match` and `If-None-Match` through `Precondition`, `Depth`, `Prefer`, and the `ETag`,
`CALDAV:schedule-tag` and `DAV:sync-token` those carry — each of them changes what a request
means or what a response body contains, and getting `If-Match`'s comparison wrong is how a
conditional write silently overwrites somebody else's edit. Transport, modeled nowhere: `Host`,
`Content-Length`, `Content-Type`, `Connection`, every credential, the method, the URL,
redirects and retries. The rendering doors write a header *value* and never a name, never a
`CRLF`, never a whole line, because framing is the caller's client's job. There is still no
request type and no header map, and this amendment adds none.

**4. The DAV limit dimensions ADR 0010 predicted would be missing were missing, and are named.**
`Limits` gains `max_responses`, `max_props_per_response`, `max_xml_text_bytes` and
`max_prefix_bindings`, and `Meter` gains the charges for them. The last is the one that
document called out: namespace declarations are unbounded in a way no depth counter and no
element count reaches, since one element at depth one can carry a thousand of them. The
response cardinality is one number for two things — the `href`s a multiget asks for and the
responses that answer them — because a policy that admits the request and refuses its answer
describes an exchange nobody can complete.

**5. A property's value is character data or it is elements, and one field could not be both.**
`PropValue::Unmodeled` was documented by the writing side as "the octets it kept, which is what
makes a server proxying another server's properties lossless" and filled by the reading side with
the *decoded character data* of the property's subtree. Those are two different fields wearing one
name, and the gap between them was a security defect rather than an inelegance: a peer writing
`&lt;D:href&gt;/calendars/ann/private/secret.ics&lt;/D:href&gt;` inside its own extension property
was writing a string, and the encoder pasted the decoded octets into its own multistatus, where
`<D:href>` is an element RFC 4918 gives meaning to. A proxying server therefore emitted `DAV:`
markup the peer had chosen. The balance filter in front of the copy asked whether the tags
balanced and never whether they had been tags on the wire, so any self-balanced subtree went
through.

The same one-field-two-meanings also lost data in the other direction. `read -> write -> read` was
not a fixed point — sixty-one octets of promoted markup came back as fifteen of character data —
and a value carrying an ampersand could not be written at all (`AT&T`) or was written as a document
this crate's own reader refuses (`a & b; c` emitted a bare `&`).

The decision is to split the field along the line the values already had. `PropValue::Unmodeled`
carries **character data** and is written escaped, so what a peer sent as text leaves as text.
`PropValue::Markup` carries **elements**: the reader re-serializes the subtree in this crate's own
prefixes, with each element declaring the namespace it resolved to and every text run escaped by
the same door every other run goes through, and the writer copies those octets to the sink after a
refusal filter that now also requires every `&` to begin a reference a reader would resolve and the
octets to be UTF-8. RFC 4918 section 9.1.3's own example — `<R:bigbox><R:BoxType>Box type
A</R:BoxType></R:bigbox>` — survives a proxy for the first time, which is what "lossless" was
supposed to mean.

What is *not* closed: a property that mixes character data with elements keeps its character data
and reports `DiagnosticCode::DavPropertyMarkupDropped` at `Severity::Violation`. One `Box<[u8]>`
cannot say where a peer's markup sat among a peer's text without inventing an order between them,
and this ADR is not willing to grow `PropValue` a tree for a shape no mainstream server emits. The
loss is reported rather than silent, which is the line `docs/adr/0001` actually draws.

**6. `CALDAV:calendar-timezone` and `CALDAV:timezone` are iCalendar objects too.** Amendment 1
scoped the line-ending carve-out to `CALDAV:calendar-data` "and nothing else today", and the
"today" turned out to be doing real work. RFC 4791 section 5.2.2 makes `CALDAV:calendar-timezone`
"a valid iCalendar object containing exactly one VTIMEZONE component" and section 9.5 puts the same
value inside a `calendar-query` as `CALDAV:timezone`. Every sentence of Amendment 1's argument
applies unchanged: RFC 5545 section 3.1 makes `CRLF` those objects' line syntax, `SabreDAV` and
Radicale write the octets literally, and a client that reads a collection's timezone under a
folding read and `PROPPATCH`es it back rewrites the stored object — the exact harm the carve-out
exists to prevent, one property over. All three elements are now what
`ElementName::preserves_line_endings` names, and `DiagnosticCode::DavCalendarDataLineEndingsFolded`
reports the conformant read's loss for all three.

The reason the gap survived Amendment 1 is worth recording because it is a shape of blind spot
rather than an oversight: `tests/interop.rs` only ever round-trips *this crate's own output*, and
this crate writes every `CR` as `&#13;`, which is markup and survives section 2.11 in every mode.
Write-then-read was the identity on `calendar-timezone` and always would have been. Only a fixture
holding a real server's spelling could see it.

**7. XML 1.0 section 2.2's `Char` production is enforced, and where it is not is stated.** The
reader refused `&#0;` under `SyntaxError::ForbiddenCharacter`, naming that production, and accepted
the literal `0x00` octet, `0x08`, and `\xc3\x28\xff\xfe` — so one spelling of a forbidden character
was a violation and the other was invisible, and the run handed to a caller was not text. It is now
enforced over element and attribute names, over normalized attribute values, and over character
data, with two stated exceptions. Inside the elements Amendment 6 names, because that is where this
reader has already stopped being a conformant XML 1.0 processor and because a fold that splits a
codepoint is a resource ADR 0001 guarantees this workspace round-trips. And inside `DAV:href`,
whose value `value.rs` models as octets on the explicit ground that "a type that cannot model a
response one can read is the failure this workspace exists to prevent".

The write side gains the matching refusal and it costs something real. A `calendar-data` payload
that is not UTF-8 is refused with `ValueError::NotUtf8` rather than written, because a document
declaring UTF-8 and carrying octets that are not is discarded *whole* by any conformant processor —
the peer loses the entire response, not one property, and nothing on the wire says why. There is no
escaping that helps: a character reference names a code point and these octets are not one. **A
`.ics` whose RFC 5545 fold falls between a lead octet and its continuations therefore has no CalDAV
representation at all.** That is a fact about the envelope and not about the file, which this
workspace still reads and writes byte for byte; it is recorded in ADR 0001's register as well as
here. The remaining hole is named rather than closed: a non-UTF-8 `DAV:href` is still written
through, because percent-encoding it on the way out without decoding on the way in would break the
round trip and decoding it would erase the difference between `%2F` and `/`.

**8. An attribute value is the value XML 1.0 section 3.3.3 defines.** `XmlPull::attribute` answered
the octets between the quotes, with the reader's own justification that "the attributes this crate's
vocabulary defines ... are all `US-ASCII` values with nothing to escape". That is an assumption
about a cooperative peer. A `comp-filter name="VE&#78;T"` named a component spelled `VE&#78;T` here
and `VENT` in every conformant processor, so two implementations disagreed about which components a
hostile `calendar-query` selects; the same gap made a request round trip grow four octets a hop,
because the encoder escaped an `&` the reader had never resolved. References are now resolved and a
literal tab, line feed or carriage return becomes one space, before the value is delivered. The
signature changes: the normalized value borrows the tokenizer rather than the body, because it
appears nowhere in the body contiguously. `XmlPull` also gains `attribute_count` and `attribute_at`,
without which Amendment 5's kept fragment would silently drop everything written *on* a peer's
elements.

**9. A comment costs what its octets cost, and a truncated report states no token.** Two claims
this ADR and ADR 0010 make together were false at one seam each. `skip_comment` advanced the cursor
and charged nothing, so eight mebibytes refused as character data were free as `<!-- ... -->`; the
only remaining bound was `max_response_bytes`, which is per body, so a peer bought unmetered
octet-by-octet scanning at 64 MiB a request against the ledger that is supposed to be the aggregate
one. Comments and the whitespace outside the root are now charged like the octets they are. And RFC
6578 section 3.4 makes `DAV:sync-token` a statement about the whole of a report, so a report this
reader truncated at `max_responses` now states none — the guard is the fact of truncation and not
the position of the element, because a server writing the token before its responses (which this
reader accepts) otherwise handed back a full token for sixteen of forty thousand changes, and a
caller storing it would never be told about the rest.

**10. The header boundary gained the door it was missing, and `ETag` gained its production.**
Amendment 3 named `If-Match` as protocol semantics and modeled the writing half. There was no
reading half at all, and the only reading door there was — `ETag::parse` — answered identically for
`If-Match: *` and for a header value it could not parse, which are the two cases that demand
opposite outcomes on a write. `MatchHeader` reads one, including RFC 9110 section 13.1.1's list
form, and judges it under the strong comparison `If-Match` uses and the weak one `If-None-Match`
uses.

`ETag::parse` now holds every octet between the quotes to RFC 9110 section 8.8.3's `etagc`
production. That is a security bound and not pedantry: an accepted tag is rendered straight into a
header *value* for a caller to frame, so a server answering
`<D:getetag>"2d9&#13;&#10;If-Match: *"</D:getetag>` could choose the caller's other headers — the
caller's conditional write became unconditional and silently replaced whatever another client had
stored. Thirty-four octets outside the production were accepted before, `CR` and `LF` among them.
`Status::parse_status_line` is held to section 15's three digits for the same class of reason: a
fourth digit read as a success the server never stated, which promotes a malformed `DAV:propstat`
into a property `DavResponse::successful_value` hands back.
