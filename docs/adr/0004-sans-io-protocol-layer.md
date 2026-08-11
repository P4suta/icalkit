# ADR-0004: the CalDAV layer is sans-I/O and `no_std`

- Status: accepted
- Date: 2026-08-05
- Amended: 2026-08-10, 2026-08-11 (eighteen amendments)

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
serialization. **This is the shape as argued; amendment 12 collapsed the crate and D-0003
landed it. The seam is now `crates/ical-core/src/grammar/`, a private module tree re-exported
at that crate's root, and everything below about where things live still holds — only the
crate boundary is gone.** A grammar-only consumer — a linter, a diff or merge tool, a fuzz harness —
sheds the typed model: `CivilDate`, `EditSet`, and the typed accessors. It does not shed
diagnostics, and it was never going to: a violation of the grammar is detected by the
grammar, and a value truncated at a limit loses bytes inside the grammar, which ADR-0001
requires be flagged where the bytes are dropped.

The diagnostic vocabulary therefore lives at the bottom of the stack rather than in the
middle of it. `Diagnostic`, `DiagnosticKind`, `Severity`, and the sink they are reported
into are defined in `ical-grammar`; `ical-core` re-exports them unchanged and adds only the
kinds it alone can detect. There is no second diagnostic type and no wrapping layer at the
seam, so "diagnostics travel with the item they concern" survives the split as one
vocabulary rather than two that must be reconciled. **They are defined in the grammar layer
now and re-exported by the same crate; the sentence that mattered — one vocabulary, defined
where the violation is detected — did not depend on the boundary.**

The crate table in ARCHITECTURE.md gains an `alloc` column beside `std`, because `no_std`
alone did not capture the wiring that actually broke: a panel proposal's
`Vec<Response>: Slots<Response>` failed to compile at the `ical-core`/`ical-dav` seam under
`alloc:false`, and no manifest diff can see that. No crate carries such an example, so that
class of break is still invisible until somebody wires two crates together and tries to compile
the result. What CI builds is `just no-std` and `just wasm` over the five core crates, which
proves the targets and says nothing about the seam. A compiled minimal-usage example per crate
at its declared `alloc` setting is the mechanism this paragraph wants and the workspace does not
have.

### What the purity gate actually proves (DP-18)

The gate proved two textual facts: that no forbidden dependency key appears in a manifest,
and that the string `#![no_std]` appears in a `lib.rs`. Neither is the claim. The claim is
about the packages a core crate links, and a manifest states that only when it is read for
the name Cargo would resolve rather than the name somebody wrote.

A dependency's key in a manifest is a nickname its author chose; the name Cargo links is the
`package` field. For each of {`ical-core`, `ical-recur`, `ical-tz`,
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
**Amendment 17's landing widened that walk to every root the workspace declares members under,
`gates/` included, so a directory that is not a crate cannot become one by sitting where
nothing looks; the purity partition still covers `crates/` alone. The same landing made
`CORE_CRATES` and the `Justfile`'s `core_crates` read each other, because one decision written
twice with neither copy failing on its own is a decision waiting to drift.**

### XML inside `ical-dav` (DP-14)

The XML syntax inside those requests and responses is `ical-dav`'s to own, not an outside
dependency's. The purity gate forbids `quick-xml`, `roxmltree`, and every other outside XML
crate, so `ical-dav` hand-rolls a tokenizer restricted to the closed element vocabulary
CalDAV and WebDAV actually use — the `DAV:` and `urn:ietf:params:xml:ns:caldav` tables, no
DTD, no external entities, no processing instructions, UTF-8 only, size and depth bounds per
ADR-0002's posture toward hostile input. It stays inside `ical-dav` rather than a sibling
crate or a shared `webdav-core` until a second DAV-shaped consumer — CardDAV, WebDAV-sync —
exists in this workspace to justify the extraction; that extraction is a deferred cost, not a
rejected one. **Amendment 11 gives that deferral a mechanism and a trigger instead of an
intention: the untangling happens now, the crate does not.** Three obligations are
load-bearing and enforced in code, not documented as intent.

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
`ical-grammar` back into `ical-core` before 1.0, and nothing here decides that. **Amendment 12
decides it. The trigger as written could never be evaluated — "if no real caller ever wants" is
a proposition about unbounded future time — so what replaces it is a measured baseline and a
numeric re-opening threshold, and the collapse happens before the first publish rather than
before 1.0. It has landed: six crates are publishable, not seven, and what holds the layer in
its place is `gates/grammar-layering` plus the second rule of `xtask purity`.**

Where tree construction lives was left unstated and M0 placed it. Unfolding and content-line lexing
belong to the grammar layer and the typed view belongs to the model above it, and the `BEGIN`/`END`
stack went to `ical-core` with them: a nesting mismatch — a `VEVENT` closed by `END:VTODO` — needs
no typed interpretation and is not content-line grammar either, but it is the tree builder's own
stack that sees it, and `unmatched-end`, `mismatched-end-name` and `unclosed-component` are how it
reports. That the crate owning the construction also owns the report is the rule this section
wanted; what it did not settle was which crate, and the first compile did.

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
still just absence from a hand-maintained const. **Amendment 16 gives it one, inside the
mechanism this ADR already has rather than by adopting a tool: what closes is "nobody checks
the checker's manifest", and the first sentence of this paragraph is untouched.**

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
closer on the roadmap than this assumed. **Amendment 11 answers the panel's reading without
adopting it: the harm it guards against is caused by *exporting* the grammar, not by leaving
it in place, so the grammar is untangled and unexported and the crate name is not spent.**

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
stack now draws on the same budget without anyone having named the numbers. **That last
sentence is answered by [ADR 0010](0010-shared-resource-limits.md)'s Amendment 1, which makes
the response cap the stated envelope a policy declares rather than a number nobody chose, and
requires every other DAV dimension to say how it was arrived at.**

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
round trip and decoding it would erase the difference between `%2F` and `/`. **Amendment 13
closes it, and pays the round trip for it: the encoding happens, the decoding does not, and the
inverse is offered as a segment-wise equivalence rather than as a decoded byte string.**

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

**11. The `webdav-core` deferral gets a mechanism and a trigger, and the graph change it was
tangled with is decided in its own ADR.** This document defers the extraction twice, once as "a
deferred cost, not a rejected one" and once as the panel's top-scored reading to revisit. Both
sentences describe an intention with nothing behind it, and the Consequences already record that
what is being kept private grew from a tag matcher into a namespace-resolving reader and writer.
[ADR 0012](0012-query-evaluation-crate-and-the-deferred-webdav-extraction.md) decides the whole
boundary — it adds `ical-query` above the spine and it does *not* publish `webdav-core` — and
takes this ADR's own instruction seriously that a graph change of that size be justified in its
own document rather than ride in on this one.

What changes here is the deferral's shape. The expensive half of the extraction — untangling the
tokenizer, namespace stack and writer from the CalDAV vocabulary — happens in that restructuring
rather than on the day a second consumer arrives, and the result is a module that may not name a
CalDAV type and exports nothing, including through `#[doc(hidden)] pub`. The trigger is therefore
no longer "when CardDAV is closer than this assumed" but "when a second DAV-shaped consumer is
accepted", and on that day the extraction is a file move plus a manifest rather than a redesign.
The panel's reading — extract before external users depend on `ical-dav`'s internals — is honored
without being adopted, because the harm it names is caused by *exporting* the grammar and not by
leaving it in place, and not exporting it costs a gate where publishing costs a crate name that
cannot be withdrawn.

The cost is that `ical-dav` carries a boundary and a gate forbidding CalDAV names inside its own
tokenizer, paid on every future XML fix, for one consumer that may never get a second — if none
arrives, that friction bought nothing. And the bet is bounded rather than eliminated: if a second
consumer arrives after `ical-dav` reaches 1.0, moving the module out is still a semver event for
its callers even though the code moves cleanly. Keeping the grammar private makes the move
mechanical, not free.

**12. `ical-grammar` collapses into `ical-core` before the first publish, and the layering rule it
existed to hold becomes a compilation instead of a crate boundary.** The Consequences say the
honest move is to collapse the crate if no real caller ever wants grammar-without-model, and
"ever" is why that sentence survived five milestones: it is a proposition about unbounded future
time, so today's observed zero is not evidence about it and the conditional can be evaluated at no
date. The principle of DP-17 is untouched — the content-line grammar is a layer that must not know
the object model — and only its mechanism moves, from a separate published crate to a separate
compilation unit.

The fact that decides it is not in the crate graph at all. `#[non_exhaustive]` is inert inside the
defining crate, so after the collapse an exhaustive `match` on `Token` with no wildcard compiles in
`ical-core` while external callers are still forced to write one. That reorders everything, because
the one correctness-grade defect in this area — the tree builder's wildcard arm silently dropping
octets, in a workspace whose ADR 0001 is a byte-identical round trip — has two fixes and they are
not equally priced. Deleting the attribute removes the arm and makes every future `Token` variant a
major bump across all seven crates under one `version_group`. Collapsing the crate removes the arm
*and* keeps the attribute, so external additions stay minor. The collapse strictly dominates on the
axis that actually mattered, and the wildcard arm is deleted with `unreachable_patterns = "deny"`
behind it so it cannot come back. **That lint cannot see the shape that loses data; amendment 18
withdraws the clause and puts the fourth rule of `xtask purity` behind it instead.**

The seam's stated product was insurance — "the grammar could be extracted later" — which is a
promise. What replaces it is the same insurance as a structural property: a zero-dependency,
`publish = false` workspace member under `gates/` whose entire source `#[path]`-includes the
grammar subtree into a crate root containing no model, so the same bytes compile twice per
workspace build and an upward reference is a rustc error with a file and a line rather than a
review comment. It lives under `gates/` and not `crates/` deliberately, because the purity gate's
own unregistered-crate walk covers `crates/` and would otherwise flag it on day one. The arrow of
irreversibility also runs against the incumbent: nothing is published and every version is `0.0.0`,
so collapsing costs zero and *publishing* `ical-grammar` is the act that incurs the irreversibility
cited against collapsing it. Collapse-now and re-extract-later is a minor release, because
restoring a glob re-export preserves type identity and every root path; publish-now and
collapse-later is a permanent name or a permanent shim.

The footprint claim the seam rested on is settled with numbers rather than withdrawn, so a later
challenger beats a measurement instead of a preference. Measured 2026-08: x86_64 release linked
executable identical either way; under opt-level `z` with fat LTO and stripping, the collapsed
build is 512 octets *smaller*; `wasm32` cdylib is 93 octets larger, 0.59%; and a sensitivity
control moves 24,576 octets when one `Document::parse` call is linked, which is what makes the null
result a measurement rather than a broken harness. Clean-build wall clock is the only real delta:
+4.18s x86_64 release, +1.94s dev, +7.63s `thumbv7em-none-eabi`. Re-extraction requires a *named*
consumer wanting grammar without model **and** either a 5% artifact-size reduction on one shipped
target or a 10s clean-build reduction, taken with `cargo build --timings`. On today's numbers it
fails both.

What this makes worse, in order of weight. The only measured saving the seam delivers is discarded
and the workspace pays more rather than less: the layering crate compiles roughly four thousand
lines a second time on every workspace build, so contributors pay a small permanent tax to hold a
guarantee outsiders cannot buy, and the plain collapse would have been cheaper for us. A `#[path]`
member is an unfamiliar construct with measured friction — a lint on a grammar file is reported
once per compilation unit, coverage tooling attributes those files across two units, the module
root can carry no crate-level inner attributes, and a lateral reference spelled `crate::X` compiles
in `ical-core` and fails the gate, which is a good failure that contributors must be taught. Nobody
has run this repository's full gate set against such a member: `cargo-semver-checks`,
`cargo llvm-cov` attribution, REUSE header scanning and `cargo package --verify` are unprobed, and
that is where this decision is most likely to be embarrassed — if any of them breaks, the dissent's
failure condition has fired and the collapse must be re-argued without the layering crate.
**Amendment 17 is that re-argument. The precondition fired — three gates broke, `cargo test --doc`
among them, which this paragraph does not list — and two of this verdict's own claims are corrected
there: the `#[path]` string its recipe hands forward is off by one directory level, and the
sentence above saying a lateral `crate::X` fails the gate is withdrawn, because it does not.** One
textual assertion also survives, in a gate family with no custodian: nothing stops a pull request
from deleting the layering member alongside the violation it would have caught, and the mitigation
is a string-equality check narrower than a name scanner but not zero. `unreachable_patterns` is
adopted workspace-wide for one match and will fire somewhere nobody was thinking about. Eleven
prose sites are falsified at once, two of which are not merely stale but *incorrect* and must be
corrected rather than deleted, since both assert that `#[non_exhaustive]` is what the split
spends — a claim that is false inside the defining crate. And the `ical-grammar` name is left
unclaimed on crates.io, because claiming it defensively is precisely the irreversible act being
argued against.

The dissent is preserved in full force because its central observation is true: the deletion of
`#[non_exhaustive]` is severable from the boundary, the edit that has a *deadline* is the attribute
rather than the collapse, and the ecosystem survey measures packaging rather than layering — six
deployed implementations keep the grammar seam at a module or interface boundary inside one
distribution unit, and not one reports the discipline was a mistake. It wins outright if the
`#[path]` member breaks one of the four unprobed gates, and in that case keeping the crate boundary
and paying a seven-crate major bump for a rare `Token` variant is the better trade. Also recorded
so it is not re-proposed: `publish = false` on `ical-grammar` with `ical-core` re-exporting it is
*infeasible* rather than unattractive, because `cargo publish` requires every dependency of a
published crate to resolve from the registry.

**13. A non-UTF-8 `DAV:href` is percent-encoded on the way out, never decoded on the way in, and
the inverse is an equivalence rather than a decoded byte string.** Amendment 7 closed by naming
the hole and the two answers that fail. The third is that both failures belong to the same
mistake — treating the inverse as a flattening. `write_href` emits `%XX` in uppercase hex for
every octet the crate's own RFC 3986 predicate rejects — controls, space, `"`, `<`, `>`, `\`,
`^`, backtick, `{`, `|`, `}`, `0x7F`, and every octet at or above `0x80` — and for every `%` not
followed by two hex digits, and passes everything else through. So `/` stays `/`, `%2F` stays
`%2F`, `%zz` becomes `%25zz`, and `\xe9` becomes `%E9`; the output is always a legal
URI-reference, which is the postcondition a test pins. There is one octet table, `ical-core`'s,
made public and called from `ical-dav`, because a second copy of an RFC 3986 table in this
workspace is the divergence this item's blast radius exists to prevent.

The reader is unchanged: nothing is decoded, and `Href` keeps the byte shape Amendment 7 carved
out of the `Char` production. What is added is the two doors the earlier statement was missing.
`Href::is_as_sent` answers, before a write, whether the octets held are the octets going out —
the same meaning `CalendarPayload::is_as_sent` carries, in the other direction. And
`Href::addresses_same` splits both values on unencoded `/` and compares segment by segment with
percent-decoding inside each segment and hex case folded, so `%E9` and `\xe9` name one resource,
`%e9` and `%E9` name one resource, and `%2F` and `/` do not. That is what dissolves the objection
that retired decode-on-read: the distinction between `%2F` and `/` is erased only by flattening a
path to bytes, and RFC 3986 section 3.3 decides segment structure before percent-decoding. It is
also what a server actually needs, since the only use for the inverse is deciding whether two
hrefs name one resource. `PartialEq`, `Ord` and `Hash` stay byte-wise and are documented as not
being resource identity, and `addresses_same` normalizes nothing else — not scheme case, not host
case, not dot segments, not trailing slashes.

The failure this repairs was real and was this crate's own: nothing in `ical-dav` decodes, so a
client that reads `\xe9` and echoes `%E9` in a `calendar-multiget` was not matched by a server
built on this crate comparing with `==`. A byte shape that can model a response one can read and
then cannot address what it modeled is the same failure one step later.

The bound now applies to the emitted length rather than the held length, because `max_href_bytes`
is a wire bound and the peer's reader applies it to what arrives — so an `href` that was legal to
read can be illegal to write back, as `LimitExceeded::Href`, on a round trip that previously
completed. Beyond that: ADR 0001's round trip becomes conditional for this one value, recorded in
that document's own register; the identity is restored only by a call the type does not force, so
`==` where `addresses_same` was meant is a silent wrong answer; `addresses_same` is a pairwise
linear scan with no canonical form to hash, so a large collection normalizes its own keys or
compares pairwise; an `href` already carrying a literal `%` followed by two hex digits goes out as
an escape it never was, an ambiguity that existed before the octets were handed in; and `ical-core`
gains an RFC 3986 name in an RFC 5545 crate, which is surface added to the spine for a consumer
above it.

The strongest rejected alternative is refusal on write, exactly as Amendment 7 refuses a non-UTF-8
`calendar-data` — this workspace's own precedent, on the same wire, in the same document, and the
cleanest statement of ADR 0001 anyone offered, since write-then-read stays identity-or-error with
no third outcome. It is rejected on a distinction worth recording so it is not relitigated: a
non-UTF-8 `calendar-data` payload has no CalDAV representation at all, so refusal loses nothing
that could have been sent, while a non-conformant path has exactly one representation and RFC 3986
section 2.1 is it. Refusal is right where the protocol has no spelling and wrong where it has one
and this crate declined to write it — a client able to read a `PROPFIND` listing a member it can
then never fetch is a worse outcome than a transformed octet.

**14. The vocabulary does not extend past RFC 4791 in behavior, and does extend past it in names.**
DP-14 closes the element vocabulary over the `DAV:` and CalDAV tables, and the roadmap asks whether
RFC 3744's ACL vocabulary and the principal-discovery reports come in. Those are two questions.
Whether this workspace implements another specification's *semantics* is a scope call, and the
answer is no: ACL's semantics are privilege aggregation, inheritance and principal resolution, this
workspace has no conformance corpus for any of them, and M4 already produced the standing lesson
that a vocabulary without an evaluator is half a feature. RFC 3744 and the discovery reports are
therefore written into the Non-goals beside CardDAV, and they do not migrate into
[ADR 0012](0012-query-evaluation-crate-and-the-deferred-webdav-extraction.md)'s boundary either —
whatever is untangled there inherits this limit rather than becoming the place ACL was always
going to live.

Whether the protocol layer may *name* what it declines is a different question, and there the
answer is yes, on this crate's own stated principle: a row exists for every element unconditionally
so that a build without a feature refuses the `REPORT` it cannot answer instead of quietly ignoring
it. Refusing a standard report without being able to say which one arrived is that same
silent-collapse defect one step out. So six recognition-only rows join the closed table —
`DAV:acl-principal-prop-set`, `DAV:principal-match`, `DAV:principal-property-search`,
`DAV:principal-search-property-set`, `DAV:expand-property` and `DAV:acl` — each unconditionally
unsupported in every feature combination, with no reader, no writer and no request-body variant.
They arrive as `DavError::Unsupported(name)` where today they are indistinguishable from a root an
attacker invented.

The justification is *not* that a server owes a `403` carrying `DAV:supported-report`. It is that a
server must recognize a standard report by name before it can choose any of the three answers
deployments actually send — an empty `207`, that `403`, or a `400` — and the empty `207` is the
observed one: Radicale handles `principal-search-property-set`, `principal-property-search` and
`expand-property` together, logs a warning, and returns an empty multistatus, its own comment
recording that a known client stops working if an error code is returned. `DavError::Foreign`
carries nothing by construction, so a server built on today's crate can implement that behavior
only for every unrecognized root including genuine junk, which converts a client-compatibility
workaround into a policy of answering nonsense with success. `DavError::Unsupported` therefore
prescribes no HTTP status, and its documentation must say so in those words.

Four costs. The closed table stops meaning "every element this crate can act on" and starts meaning
"every element this crate can name", which is a weaker and less legible invariant held by
documentation rather than by shape. `Unsupported` now spans two facts that call for opposite
responses — "this build lacks a feature", fixed by a rebuild, and "no build will ever answer this"
— with an identical value, so a caller logging "rebuild with the feature enabled" becomes silently
wrong for six rows. The correct-status obligation is prose only, because the mapping happens in the
caller's HTTP layer, which this ADR's sans-I/O choice puts outside the crate: this is that choice's
bill arriving. And declaring ACL out of scope tells a reader who wants a multi-user server no
rather than not-yet — the narrow reopening, preserved so the whole question need not be reopened
for it, is `DAV:expand-property` read and write alone, one report, client direction, if evidence
appears that a client built on this crate cannot discover a principal's collections.

**15. `calendar-multiget` carries `QueryShape`, and the rule that decides which bodies do is the
production rather than symmetry.** M4 gave `calendar-query` a shape because RFC 4791 section 9.5's
own production is a body this crate reads and writes rather than one it answers `DavError::
Unexpected` to, and left the identical alternation in section 9.10 refused on the ground that
nobody is known to send it. That is the argument M4 declined one element over, and no protocol
layer can answer it for a caller it has never met — a client of this crate is exactly the thing
that could start sending it. So `CalendarMultiget` gains `shape: QueryShape`, defaulting to
`Named`, read and written exactly as `CalendarQuery::shape` is.

Deciding it by the production also *closes* the set, which is the part worth holding code against.
`QueryShape` is carried by exactly the two bodies whose grammar contains
`(DAV:allprop | DAV:propname | DAV:prop)?` and by nothing else. Not `DAV:propfind`, where RFC 4918
section 14.20 requires exactly one of the three, which is why `PropFind` is a three-variant enum
and not a field. Not `DAV:sync-collection`, whose RFC 6578 production requires `prop`. Not RFC
3744's `principal-match` or `principal-property-search`, both of which are `prop?` and take an
optional property request. The mechanism becomes specific in both directions rather than growing by
analogy.

Two things ride with it because the same reading exposes them. The double choice is refused in both
bodies: `CalendarQuery::read_xml` currently sets a shape on `allprop`/`propname` without the
accept-once guard `read_propfind_child` uses, so a body carrying both `<prop>` and `<allprop/>`
reads as `AllProp` with a populated list and re-encodes with the list silently gone — a body that
read one way and wrote another, which is what DP-15 forbids. And the writer refuses a hand-built
value whose shape and property list disagree, rather than dropping the list, since after the reader
fix such a value can only be a caller error and silently discarding it is the same defect arriving
from the other side.

Four things get worse. This ships a code path for traffic nobody has observed. It hands callers a
working way to build a request the field may refuse — RFC 4791 section 9.6 puts `calendar-data`
inside `DAV:prop`, so `allprop` on a multiget cannot ask for the one payload a multiget exists to
fetch, and against the most widely deployed server the answer is not a `400` but a `207` with
nothing in it, which is quieter and worse; the doc comment carries that in prose because no type
can. The illegal state stays representable in a second public type instead of one, excluded by a
runtime refusal rather than by shape, and this knowingly duplicates the weaker representation the
`PropFind` enum next door does not have. And a writer that could not fail for this reason now can.

The alternative rejected, whose factual core survives as the doc comment `shape` must carry: keep
the refusal and record it as deliberate, on the ground that section 7.9 makes a multiget a report
about specific resources and `allprop` structurally cannot request calendar data. It applies word
for word to `calendar-query`, where M4 heard it and declined it, and deciding that a conformant
body is useless is a server's judgment rather than a sans-I/O layer's. Also recorded: collapsing
`QueryShape` and `PropFind` into one sum type that owns the property request, so the illegal pair
is unrepresentable everywhere. It loses because the three bodies disagree about *absence* — one
requires a member of the group and two make the group optional — so one type would blur an absent
group into an empty one. It is worth revisiting before 1.0, while nothing is published.

**16. The purity gate gets a custodian, and the two lists that are documented to agree are made to
agree.** The Consequences name three holes in one sentence: `xtask` is governed by no purity rule,
the same pull request that breaks the rule can weaken the check, and `ical-conform`'s exemption is
absence from a hand-maintained const. The rule that closes them is already written down twice — the
tool that enforces "the core has no outside dependencies" may not acquire one — so what was left
was to say which mechanism carries it, and the answer is the gate itself rather than a new tool.

Four legs. `collect_purity_violations` reads `xtask`'s own manifest with the same
`declared_dependencies` scanner it applies to core manifests, and any entry in any dependency table
is a violation, which turns that manifest's comment from a promise into a checked claim. The
governed crate list and the Justfile's `core_crates` are cross-read, so a name in one and not the
other fails — the const's own doc comment already asserted the mirror and nothing enforced it.
Every directory under `crates/` holding a manifest must appear in exactly one of the governed set
or a new exempt set where `ical-conform` is written down by name, and absence from both is the
violation, whether or not the crate declares `#![no_std]`; the `#![no_std]` heuristic stops being
how the gate notices a new crate. And a named list of member roots — `crates/` and `gates/` —
bounds a check that the root manifest's members equal `xtask` plus every manifest-bearing directory
under each root that exists on disk, with a root that does not exist yet not an error.

The last two legs are stated over sets and roots rather than over one const and one hard-coded
directory, deliberately, because Amendments 11 and 12 move both in the same wave: Amendment 12
registers a member under `gates/`, which a rule written over `crates/` would reject on the day it
lands, and ADR 0012 replaces the governed const with a per-crate permitted-dependency map, which a
leg naming the const would either outlive as dead text or reintroduce. The two walks keep different
domains on purpose — `gates/` is inside the membership check and outside the purity partition —
because Amendment 12 places the layering member outside `crates/` precisely so the purity walk does
not govern it, and unifying them would overturn that silently.

Four costs, and the first is a sequencing obligation rather than a design one. `xtask purity` is
rewritten three times in one landing, so this edit lands last or lands merged with the other two;
in any other order the gate is red on the day the layering member is registered, or a leg is
written against a const that no longer exists. The totality claim is only as wide as the member-root
list, which is a hand-maintained list — a smaller copy of the staleness this closes rather than its
elimination, and a crate parked in a third top-level directory stays invisible. Naming
`ical-conform` in an exempt set makes the exemption cheap and *blessed*: granting one becomes the
same single line that absence costs today, and it now looks approved, while the gate records who is
exempt and still cannot say why. And the cross-read couples the gate to the Justfile's textual form,
scanned by hand because the tool may have no dependencies, so a `just` refactor turns a green gate
red for a reason unrelated to purity. Leg one is also only manifest-deep: a `[patch]`, a vendored
registry or a build script defeats the custodian for `xtask` exactly as the paragraph above says
they defeat it for the core crates.

The alternative that would delete the question rather than answer it — move the check into
`cargo deny` bans plus a `cargo metadata` walk, which would also see the resolved graph — is
rejected on this document's own recorded ground: that reader would be this tool's first dependency,
and buying a custodian by acquiring the dependency the rule forbids inverts the rule it enforces.
It stays the right answer if this project ever decides the zero-dependency rule does not extend to
its own tooling. Folding `xtask` into the governed set instead is rejected for a duller reason: it
is not `no_std`, has no library target, and would fail three legs of the core rule for reasons
having nothing to do with dependency purity. The custodian needs its own rule.

**17. Amendment 12's own precondition fired. Three of the four unprobed gates broke, their whole
repair is two manifest lines and one `just` flag, and the collapse stands narrowed rather than
defeated.** Amendment 12 named four gates nobody had run against a `#[path]` member —
`cargo semver-checks`, `cargo llvm-cov` attribution, REUSE header scanning and
`cargo package --verify` — called them the place this decision was most likely to be embarrassed,
and attached a failure condition to them: if any breaks, the dissent's case has fired and the
collapse is re-argued without the layering crate. A faithful two-member workspace ran all four,
and the rest of this repository's gate set with them, before a byte moved here. Three broke.
**The cost of finding that out was one throwaway workspace instead of a red CI on the landing
commit**, which is the entire purchase a precondition written into a decision makes, and it is why
this amendment narrows the verdict rather than explaining a revert.

The failure condition is taken as written and then declined on the evidence, which is worth doing
out loud rather than by omission. It was aimed at a construct that would not survive this
repository's gate set. What the probe found instead is three tools that count one file twice or
look for it where cargo does not put it, repaired by `test = false`, `doc = false` and one
`--exclude`; and it found that every claim this decision actually rests on holds under test — the
layering crate cannot ship the grammar, an upward reference is a rustc error with a file and a
line, `#[non_exhaustive]` is inert in the defining crate and live outside it, and the purity walk
does not see `gates/`. A condition written before the evidence does not get to bind against the
evidence's shape, so what carries the day below is the reasoning and not the word "breaks".

*The doc tests, which are `just test` and `just test-ci`, and the only break in a gate this
repository runs.* A doc example on a grammar item is compiled a second time inside the layering
crate, which declares no dependencies, so the crate the example names cannot resolve and `Doc-tests
ical-grammar-layering` fails with `error[E0432]: unresolved import`. This is not hypothetical: **all
five** doc examples under `crates/ical-grammar/src/` open with `use ical_grammar::`, which the
collapse rewrites to `use ical_core::`, and each one then fails inside the gate. (This sentence said
"five of the ten" until amendment 18: ten is the count of ````` fences, which is two per example, so
the denominator was the fence count and every example was affected rather than half of them.) CI
would be red on the landing commit. The fix is `--exclude ical-grammar-layering` on both of the
Justfile's doc-test invocations. **The trap is worth naming because it will cost the next person an
afternoon: `[lib] doctest = false` does not fix this.** `cargo metadata` reports `doctest = False`
for the member and `cargo test --doc` runs the examples anyway — the merged-doctest runner of cargo
1.97.1 — verified three separate times, twice in isolation. Anyone who repairs this in the manifest
and does not re-run the gate will believe it fixed and be wrong.

*Coverage, and it errs in the flattering direction.* Under `cargo llvm-cov` the grammar files
appear as two rows carrying identical numbers and the total sums both: 96 regions where there are
54, 58 lines where there are 32, and 83.33% where the true figure is 74.07%. The error is
systematic rather than noisy — well-covered grammar is double-weighted against uncovered model —
so it always overstates, which is the direction a number nobody re-derives is least likely to be
questioned in. The fix is `test = false` in the gate's `[lib]`, which returns the report to 54
regions and 74.07%. The same one line is also what stops every grammar unit test running twice:
without it nextest starts two binaries for one `#[test]`, and the real tree's 81 grammar tests
would run 162 times.

*`cargo package --verify`, permanently, for the gate member.* The member's tarball contains only
`src/lib.rs` — cargo does not follow `#[path]` — so the verification build cannot find the grammar
and exits 101, and `cargo package --workspace` fails with it, because `publish = false` does not
exclude a member from `cargo package --workspace` the way it excludes one from publishing. This is
written down as a limitation and not repaired. It is not a gate this repository runs:
`cargo package` appears nowhere in the Justfile or in `ci.yml`. And the real release path is
untouched — `cargo publish --workspace --dry-run` skips a `publish = false` member entirely and
never packages it, which is the mechanism release-plz sits on, and `cargo semver-checks` skips it
for the same reason. **Anyone later adding `cargo package --workspace` to CI or to the Justfile is
the reader this paragraph is addressed to: that command needs `--exclude ical-grammar-layering`
too, or it is red the moment it is added, for a reason that has nothing to do with the change that
added it.**

Two corrections to this verdict's own text, and the second is a correction to what it *claimed*
rather than to how it spelled something. The `#[path]` string the recipe handed forward is off by
one directory level: `#[path]` on a `mod` declared in `src/lib.rs` resolves relative to `src/`, so
`"../../crates/ical-core/src/grammar/mod.rs"` lands on `gates/crates/…` and fails with os error 3.
It must be `"../../../crates/ical-core/src/grammar/mod.rs"`. And amendment 12 says that a lateral
reference spelled `crate::X` "compiles in `ical-core` and fails the gate, which is a good failure
that contributors must be taught". It does not fail the gate. `use crate::Token;` inside a grammar
file compiles in both crates on a clean build, because the gate's own root does
`pub use crate::grammar::*;`, which puts every grammar item at the gate's crate root as well. That
sentence is withdrawn.

What survives the withdrawal, and it is the part that decides how much act 2 is owed, is that the
leak is bounded to grammar items and does not reach the model. A model item named at the crate
root — `use crate::CivilDate;`, the most seductive spelling precisely because it looks lateral —
fails in the layering crate with file and line: `error[E0432]: unresolved import crate::CivilDate`,
`no CivilDate in the root`. The gate's root carries the grammar's glob and nothing else, so every
path out of a grammar file that names a model item lands in a root that has no model in it,
whether it is spelled `crate::tree::Node`, `crate::CivilDate` or `super::super::CivilDate`. The
layering guarantee is therefore exactly as strong as amendment 12 claimed; what the gate does not
see is a *grammar* item reached through the parent crate's root re-export, which is a hygiene
defect and not a layering violation. Act 2 is a rule about hygiene wearing a layering rule's
clothes, and it needed saying because a reader of amendment 12 would price it as the latter.

Act 2 is therefore enforced textually, as a leg of `xtask purity`, and is no longer described
anywhere as something the gate catches. No path inside `crates/ical-core/src/grammar/` may resolve
above the grammar root: in `grammar/mod.rs` neither `crate::` nor `super::`, in the files beside it
neither `crate::` nor `super::super::`, outside comments and string literals; and the tree stays
flat, because a subdirectory changes that arithmetic and a check that quietly stops applying is
worse than none. It goes in `purity` rather than in a new `just` recipe because that task already
walks this tree and already holds this ADR's structural rules, and a third recipe plus two CI lines
to read one directory is more mechanism than the rule is worth. What it costs: `purity` now means
two rules rather than one **— five, after amendment 18 —** and a contributor grepping for a gate
called "layering" finds nothing;
the check is textual, in the same family as the golden-list scan and defeated by the same things —
a macro, a generated path, a spelling it was not taught; it makes the flat grammar tree
load-bearing for a reason that is not about layering; and it is a fourth rewrite of `xtask` in the
landing that amendment 16 already sequences three into. The alternative of writing the rule into
CONTRIBUTING with no gate is rejected on the ground this workspace has just spent a workflow
clearing: an asserted rule with nothing behind it reads exactly like an enforced one and decays
without anybody noticing.

The rest of the gate set is confirmed rather than assumed, which is the other half of what the probe
bought. `cargo package --list` on the member emits `.cargo_vcs_info.json`, `Cargo.lock`,
`Cargo.toml`, `Cargo.toml.orig` and `src/lib.rs` — five entries in this repository, four in the
throwaway workspace the probe ran in, which was not a git checkout and so had no VCS record to
write. No grammar source is among them either way, so the layering crate can never accidentally ship
the grammar — this decision's load-bearing packaging claim, tested. `cargo semver-checks` is clean
and attributes grammar items through the private module and the glob. REUSE is unaffected, a file
compiled twice being one file on disk. `cargo shear` objects neither to `#[path]`-included sources
nor to a member with no `[dependencies]` table. Clippy is clean under `--all-features` and
`--no-default-features`, the gate satisfying `missing_docs`, `missing_debug_implementations` and
`unreachable_pub` with a two-line module doc; a doubled clippy diagnostic is noise here, since every
gate is `-D warnings` and nothing counts them. `[lib] doc = false` does its one job and no other —
no duplicate rustdoc page, no effect whatever on doctests. One incidental worth a line because it
presents as a different failure entirely: a stale `target/package/` directory makes `cargo
semver-checks` abort with "package is ambiguous: defined by multiple manifests", and cleaning it is
the fix.

Four costs this decision had not admitted. Its correctness now depends on three textual facts that
no manifest reader can derive — `test = false` and `doc = false` in the gate's `[lib]`, and
`--exclude ical-grammar-layering` on two Justfile lines — in a gate family amendment 12 already
recorded as having no custodian; deleting the coverage line in particular fails nothing and
silently flatters the number. The doc-test exclusion buys CI green by putting doc examples on
grammar items outside the layering gate, so an example may freely name a model item and nothing
objects: the layer is checked in the code and not in its prose. `cargo package --workspace` is
permanently unavailable to this workspace as written. And act 2, one of the six acts, turns out to
need a gate of its own to mean anything, which is a leg of `xtask` and a maintenance surface that
the version of this decision scored in the bake-off did not include.

**18. Act 2 was walked around in four spellings, the guard against a wildcard `Token` arm could
not fire on the shape that loses data, and the release configuration described a workspace that no
longer existed. All three are now rules of `xtask purity`, and the footprint figures amendment 12
sold as measurements are withdrawn as unreproducible and re-measured against a harness this file
records.** Four lenses attacked the landing rather than the decision, which is the review a gate
family with no custodian is owed, and everything below reproduced before it was repaired.

*A wildcard arm over `Token`.* `unreachable_patterns = "deny"` was adopted workspace-wide and
written into two files as what keeps such an arm from being added back. It does not. That lint
rejects a catch-all placed after every variant is already covered; a match that omits one variant
and adds `_` is a *reachable* wildcard and the lint is silent. Deleting `parse.rs`'s `Token::Value`
arm and appending `_ => self.take_value(b"", meter)` — `LineBuilder::take` swallowing every
property value — compiles with zero warnings under `-D warnings` and passes `purity`. It is also
the only shape a hand remembering the old cross-crate rule would write, since that rule *required*
the match to be partial. The fourth rule of `purity` now reads the arms: a `match` whose arm
patterns name `Token::` may not also carry a `_` arm, anywhere under `crates/`. The lint is kept —
a dead arm is still worth refusing — but it is no longer described as the thing that holds this.
`SyncToken::` ends in the same seven characters and is a different type, so the name is matched at
its boundary; a scrutinee written across lines is the documented cost.

*Four ways out of the layer.* Act 2 reads text, and text has more than one spelling for the same
import. `use crate ::Token;` compiles, means what `use crate::Token;` means, and was caught only by
`cargo fmt` — a gate held by a formatter is a formatter, so lines are now read with the whitespace
around `::` closed up. `extern crate self as ical_core;` inside a grammar file gives the layer a
name for its own crate root that is neither `crate::` nor `super::super::`, so `ical_core::` is
refused as a path and `extern crate` is refused outright; nothing in the layer needs one, because
`alloc` is declared by each root that compiles these sources. `#[path = "../launder.rs"]` in
`grammar/mod.rs` pulls a file into the layer that a rule reading the *directory* never opens, and
`cargo shear` caught it only by misdiagnosing the file as unlinked. And the mirror of that: a `.rs`
file the module root never declares is scanned by act 2 and compiled by nobody, so it is invisible
to `gates/grammar-layering`. The last two are one rule — the directory's files and `mod.rs`'s `mod`
declarations must be the same set, in both directions, and `#[path]` is refused inside the layer.

*The member with no custodian.* This decision recorded, at the paragraph on unclaimed assertions,
that nothing stops a pull request from deleting the layering member alongside the violation it
would have caught, and that "the mitigation is a string-equality check narrower than a name scanner
but not zero". There was no such check. A simulated deletion — the member line out of `members`,
the directory moved away, the two `--exclude` flags and the `msrv` line removed — passed `purity`,
`clippy`, `fmt` and `shear`, and with it gone `use crate::CivilDate;` inside a grammar file passed
too. The third rule of `purity` is that check: the member line, the package name, `publish = false`,
both `[lib]` switches, and the `#[path]` string, each compared against what this ADR says they must
be. The three textual facts amendment 17 admitted no manifest reader can derive now have a reader.

*`release-plz.toml`.* It declared a `[[package]]` block for `ical-grammar` and folded that name
into `ical-core`'s `changelog_include` for a full landing after the crate ceased to exist, because
the release path is the one path this repository runs no gate over — `ci.yml` reads fifteen gates
and none of them opens that file. The two stale references are removed and the fifth rule of
`purity` reads the published members out of the root manifest and holds the configuration to them:
one `[[package]]` block each, no block for a package the workspace does not build, and
`changelog_include` naming every published member but the one carrying the changelog. This is the
rule the next crate meets: `ical-query` fails `purity` until it has both.

*The footprint figures do not reproduce, and nothing in the tree said how they were taken.*
Amendment 12 sells them as settled with numbers "so a later challenger beats a measurement instead
of a preference", and hangs a numeric re-opening threshold on them, but records no profile, no
probe source and no exported symbol set: a grep of `docs/` for the profile, the artifact kind or
the control's figure returns only the claim itself. Reconstructing both trees with `git archive`
and measuring gives deltas of zero where 512 smaller and 93 larger were claimed, and a sensitivity
control roughly a third of the 24,576 octets stated. The three figures are therefore **withdrawn as
unreproducible**; the null result they were used to argue is *strengthened* rather than overturned,
and the threshold now has a harness to be evaluated against. The harness, so that the next
challenger inherits a measurement rather than a number: a `cdylib` probe crate outside the
workspace, `[profile.release]` with `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`,
`strip = true` and `panic = "abort"`, a bump allocator and a `#[panic_handler]` so that nothing of
`std` is linked, one `extern "C"` export driving `ContentLineReader::new` and `next_token` to
exhaustion, built for `wasm32-unknown-unknown` against `crates/ical-core` of each tree by path.
Measured 2026-08-11 on cargo 1.97.1: **3,089 octets against the split tree and 3,089 against the
collapsed one — identical to the octet**. With one `Document::parse` call added to the same probe:
10,427 split against 10,562 collapsed, the collapse 135 octets *larger*, 1.3%. That second pair is
also the sensitivity control — linking `parse` moves 7,338 octets — so the null on the first pair
is a measurement and not a broken harness. Re-extraction still requires a named consumer wanting
grammar without model **and** a 5% artifact-size reduction on one shipped target or a 10s
clean-build reduction; on these numbers the collapse is 1.3% the wrong way, so it fails that half
by a wider margin than before.

*Two smaller corrections, and one scope note.* `#[non_exhaustive]` on `Token` buys a minor release
for an added *variant* and says nothing about fields: the variants are not individually
non-exhaustive, and a `ContentLineSource` implementor outside the workspace necessarily writes
`Token::Parameter { name, value, has_value }` with a complete field list, so adding a field is a
major release. That is a decision — destructuring a token is what consuming it is — and it is now
written where the attribute is. And "one public spelling per grammar item" is exact for `Token` and
deliberately false for three others: `Limits` and `Meter` have three public spellings and `Instant`
two, because `ical-tz` and `ical-itip` re-export them at their own roots so that a caller names one
crate for one concept. The postcondition is about the item the collapse moved, not about every item
in the layer.
