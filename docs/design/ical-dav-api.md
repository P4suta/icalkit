# `ical-dav` API design

- Status: proposed
- Date: 2026-08-10
- Skeleton: assembled with the other five into one workspace and compiled together; see
  "What the first compile changed" below
- Decisions honored: DP-14, DP-15, DP-01, DP-06, DP-08, DP-17, DP-18
- ADRs: [0004](../adr/0004-sans-io-protocol-layer.md), [0007](../adr/0007-allocation-policy.md),
  [0009](../adr/0009-error-and-diagnostic-model.md), [0010](../adr/0010-shared-resource-limits.md)

## Responsibility

`ical-dav` turns CalDAV (RFC 4791) and the `WebDAV` (RFC 4918) it rests on into values: a
`calendar-query`, a `calendar-multiget`, a `PROPFIND`, a `sync-collection`, and the multistatus
that answers each of them, together with the XML tokenizer and writer those bodies need and the
protocol-defined values — an `ETag`, a `Depth`, a sync token — that travel beside them in HTTP
headers. It opens no connection, holds no client, and reads no clock; the caller moves the octets
with the stack it already has. It is opaque to iCalendar by design and not by omission: a
`calendar-data` payload is carried as octets and parsed by `ical-core`, a `time-range` is carried
and never evaluated because deciding which instances of a recurring event fall inside one is
`ical-recur`'s work and this crate does not depend on it. One implementation serves both
directions, which is the half of CalDAV that does not exist in Rust today: a client encodes a
`REPORT` and decodes the multistatus, a server decodes the same `REPORT` and encodes the same
multistatus, out of the same types.

## Context

Three constraints arrive already decided, and together they fix most of this design.

The purity gate (DP-18) forbids every outside dependency in the five core crates, dev-dependencies
included, so `quick-xml` and `roxmltree` are unavailable and the XML layer is this crate's to own
(DP-14). The crate graph (DP-17) gives `ical-dav` exactly one dependency, `ical-core`, so nothing
here may reach for `ical-recur`'s expansion or `ical-tz`'s zone resolution. And ADR-0010 makes a
`REPORT` body the second most obvious hostile input in the workspace after an `.ics` file, which
means every reading entry point takes `&Limits` and `&mut Meter` and every collection that can
grow is charged against them.

Two findings from the A1 stress pass are load-bearing and are answered below rather than deferred.
The first: XML 1.0 section 2.11 mandates that a conformant processor fold every `CRLF` to `LF`
before anything else sees it, and RFC 5545 section 3.1 makes that same `CRLF` semantically
significant inside `calendar-data`. Those two requirements are not compatible, and calling `CRLF`
preservation an ordinary implementation task would have been a lie. The second: a prefix is an
arbitrary per-document choice. `<d:multistatus xmlns:d="DAV:">` from a `SabreDAV`-backed server,
`<multistatus xmlns="DAV:">` with a default binding, and `<D:multistatus xmlns:D="http://evil">`
are respectively the same element, the same element, and a different one — and a tokenizer that
matches tag strings gets all three wrong, silently, against mainstream server software.

## Decision

**The pull layer is the parser and the owned tree is one consumer of it.** `ResponseSource` yields
one `DavResponse` at a time and never materializes a collection; `MultiStatus::read` drives that
same public path and is the only way an owned multistatus is built, so there is no private fast
path for the two to diverge along. This mirrors ADR-0008 one layer up, and it is also the honest
answer to a cap that cannot exist: no single response-count limit both defends a client with tens
of kilobytes against a forged flood and lets a server enumerate a real forty-thousand-resource
collection, because a count cannot tell a truthful entry from a forged one. Cardinality is
therefore a caller policy — `Limits::conservative()` and `Limits::generous()` are the same type
through the same code — and the constrained reader's real defense is not building the collection.

**Namespaces are resolved, never matched.** `XmlPull::resolve_prefix` maintains a bounded stack of
prefix-to-URI bindings, `Namespace::from_uri` classifies the resolved URI, and
`ElementName::resolve` takes a `Namespace` and a local name. No public function in this crate
accepts a prefix.

**Unknown is three-valued, not two.** An element outside the closed vocabulary is foreign and is
skipped with a diagnostic, as RFC 4918 section 17 requires of anything reading a body a server
extended. An element inside the vocabulary whose support is not compiled in answers
`DavError::Unsupported` — the element table is unconditional precisely so a build without a
feature says so instead of silently skipping a `REPORT` it cannot honor. Which of skip and refuse
applies to foreign elements is `UnknownPolicy`, a caller policy, because a client tolerating a
server's extensions and a server refusing a request it cannot honor are both correct.

**Line endings get a scoped, named deviation.** On write, a carriage return inside a
`preserves_line_endings` element is emitted as the character reference `&#13;`, which is the one
construct XML 1.0 exempts from section 2.11 normalization; the crate emits no `CDATA` section at
all, which is also why a literal `]]>` in a `DESCRIPTION` is not a hazard here. On read,
`TextPolicy::Verbatim` applies to exactly the elements `ElementName::preserves_line_endings`
names — `caldav:calendar-data`, and nothing else today — and hands back the octets as they
arrived. That is a deliberate divergence from XML 1.0 for one element, stated here rather than
discovered later, because a conformant read of a Radicale or `SabreDAV` response destroys the
folding it took ADR-0001 a whole document to protect.

**Every collection is a `Bounded<T>`.** The backing `Vec` is private, growth goes through one
charged `push`, and the cap comes from the caller's `Limits` at construction. This is the gate
rule from DP-15 applied uniformly rather than to `MultiStatus.responses` alone, since the argument
that a public `Vec` defeats a choke point does not weaken for `PropStat`'s property list.

## The public surface

Signatures are as they appear in the skeleton, which compiles under `#![no_std]` with the
workspace lint table and `clippy::pedantic`, for the host, `wasm32-unknown-unknown` and
`thumbv7em-none-eabi`.

### Failure and delivery

```rust
pub enum DavError { Limit(LimitExceeded), Syntax(SyntaxError), Unsupported(ElementName),
                    Unexpected(ElementName), Invalid(ValueError), Output(SinkFull) }
pub enum SyntaxError { Doctype, ProcessingInstruction, Encoding, UnboundPrefix,
                       MismatchedTag, Truncated, Malformed }
pub enum ValueError { TimeRangeUnbounded, TimeRangeInverted, StatusLine, EtagSyntax,
                      NotUtf8, FilterContradiction }

pub trait ByteSink { fn write(&mut self, bytes: &[u8]) -> Result<(), SinkFull>; }
pub struct SliceSink<'a> { /* private */ }
```

*Invariants.* `DavError` is the structural channel of ADR-0009 raised one layer: nothing in it is
recoverable by ignoring it, and everything tolerable is a `Diagnostic` on the caller's sink with
the read continuing. It hand-implements `Display` and `core::error::Error` — no derive crate
exists to reach for. `ByteSink` is implemented for `Vec<u8>` through `try_reserve`, and for
`SliceSink` over a caller-owned buffer, so an encoder never assumes an allocator is willing.

### The XML layer

```rust
pub enum Namespace<'a> { Dav, CalDav, Other(&'a [u8]) }
pub struct QName<'a> { pub namespace: Namespace<'a>, pub local_name: &'a [u8] }
pub enum ElementName { Multistatus, Response, Propstat, /* … 45 rows … */ MaxResourceSize }
pub struct ElementSpec { pub namespace: Namespace<'static>, pub local_name: &'static str,
                         pub written_name: &'static str }

impl ElementName {
    pub fn resolve(namespace: Namespace<'_>, local_name: &[u8]) -> Option<Self>;
    pub const fn preserves_line_endings(self) -> bool;
    pub fn spec(self) -> ElementSpec;
}

pub enum TextPolicy { Normalized, Verbatim }
pub enum UnknownPolicy { Skip, Reject }
pub enum TextRun<'a> { Wire(&'a [u8]), Reassembled(Box<[u8]>) }
pub enum XmlEvent<'a> { Start { name: QName<'a>, known: Option<ElementName>, depth: u16 },
                        End { .. }, Text(TextRun<'a>) }

pub trait XmlPull<'a> {
    fn next_event(&mut self, limits: &Limits, meter: &mut Meter)
        -> Result<Option<XmlEvent<'a>>, DavError>;
    fn skip_element(&mut self, limits: &Limits, meter: &mut Meter) -> Result<(), DavError>;
    fn depth(&self) -> u16;
    fn offset(&self) -> usize;
    fn resolve_prefix(&self, prefix: &[u8]) -> Option<Namespace<'a>>;
    fn text_policy(&self) -> TextPolicy;
}
```

*Invariants.* The tokenizer refuses a `DOCTYPE` or an internal entity declaration before it starts,
accepts no processing instruction beyond the XML declaration and no encoding other than UTF-8,
iterates rather than recurses, and charges the meter on every event. `TextRun::Wire` is a slice of
the caller's body and exists only when nothing had to be reassembled; a reference or a `CDATA`
boundary produces `Reassembled` and a `DavCalendarDataCopied` diagnostic, so a caller can tell
which it got. `ElementName::spec` is total, and `written_name` carries this crate's own fixed `D:`
and `C:` prefixes, which are an output choice and never an input assumption.

### Reading and writing

```rust
pub struct DecodeContext<'a> { pub unknown: UnknownPolicy, pub limits: &'a Limits,
                               pub meter: &'a mut Meter, pub sink: &'a mut dyn DiagnosticSink }

pub trait ReadXml: Sized {
    fn read_xml(events: &mut dyn XmlPull<'_>, context: &mut DecodeContext<'_>)
        -> Result<Self, DavError>;
}
pub trait WriteXml {
    fn write_xml(&self, out: &mut dyn ByteSink, limits: &Limits, meter: &mut Meter)
        -> Result<(), DavError>;
}
pub trait ResponseSource {
    fn next_response(&mut self, limits: &Limits, meter: &mut Meter,
                     sink: &mut dyn DiagnosticSink) -> Result<Option<DavResponse>, DavError>;
    fn sync_token(&self) -> Option<&[u8]>;
}
pub struct MultiStatusReader<'a> { /* private */ }
impl<'a> MultiStatusReader<'a> {
    pub fn new(body: &'a [u8], limits: &Limits) -> Result<Self, DavError>;
}
```

*Invariants.* `ReadXml` takes a diagnostic sink and `WriteXml` does not; that asymmetry runs along
encode versus decode, which both roles do, and never along client versus server. `ResponseSource`
carries no lifetime parameter and is object-safe, so a caller can hold `&mut dyn ResponseSource`
without a generic spreading through its own types — the dyn-safety ADR-0008 records as unsettled
one layer down is settled here by construction. `MultiStatusReader` is deliberately not an
`Iterator`: `Iterator::next` takes nothing but `&mut self`, and every read here carries the
caller's policy, ledger and sink.

### Values both directions share

```rust
pub struct Bounded<T> { /* private Vec + cap */ }
impl<T> Bounded<T> {
    pub const fn with_cap(cap: usize) -> Self;
    pub fn push(&mut self, item: T, meter: &mut Meter) -> Result<(), DavError>;
    pub fn as_slice(&self) -> &[T];
}

pub struct Status { /* private u16 */ }
impl Status { pub fn parse_status_line(line: &[u8]) -> Result<Self, DavError>; }
pub struct Href { /* private Box<[u8]> */ }
pub struct ETag { /* private */ }
pub struct SyncToken { /* private */ }
pub enum Precondition<'a> { Replace(&'a ETag), ReplaceAny, CreateOnly }
pub enum Depth { Zero, One, Infinity }
pub enum PropName { Known(ElementName), Extension(ExtensionName) }
pub enum PropValue { Empty, Text(Box<[u8]>), Reference(Href), Resource(ResourceType),
                     Entity(ETag), CalendarData(Box<[u8]>), Unmodeled(Box<[u8]>) }
pub struct DavProperty { pub name: PropName, pub value: PropValue }
```

*Invariants.* `Status` holds a code in `100..600` and nothing else can be constructed. `Href` is
byte-shaped, not `String`: a server is free to emit octets that are not UTF-8, and a type that
cannot model a response one can read is the failure this workspace exists to prevent — `as_str`
is a fallible view, as DP-02 has it everywhere else. `PropValue::Unmodeled` keeps the octets of a
property this crate has no model for rather than dropping them. `Precondition` and `Depth` are
protocol values that happen to travel in headers; this crate renders them and models no request.

### Requests

```rust
pub struct PropRequest { /* private Bounded<PropName> */
                         pub calendar_data: Option<CalendarDataRequest> }
pub enum PropFind { AllProp(PropRequest), Names, Props(PropRequest) }
pub struct CalendarQuery { pub props: PropRequest, pub filter: Option<CompFilter> }
pub struct CalendarMultiget { pub props: PropRequest, /* private Bounded<Href> */ }
pub struct FreeBusyQuery { pub range: TimeRange }
#[cfg(feature = "sync-collection")]
pub struct SyncCollection { pub token: Option<SyncToken>, pub level: SyncLevel,
                            pub limit: Option<u32>, pub props: PropRequest }

pub struct TimeRange { /* private */ }
impl TimeRange {
    pub fn new(start: Option<Instant>, end: Option<Instant>) -> Result<Self, DavError>;
    pub const fn starting_at(start: Instant) -> Self;
    pub const fn ending_before(end: Instant) -> Self;
}
pub struct CompFilter { /* private name, comps, props, depth */
    pub is_not_defined: bool, pub time_range: Option<TimeRange> }
impl CompFilter {
    pub fn push_comp(&mut self, child: Self, limits: &Limits, meter: &mut Meter)
        -> Result<(), DavError>;
}
pub struct PropFilter { .. } pub struct ParamFilter { .. } pub struct TextMatch { .. }
pub enum Collation { AsciiCasemap, Octet, Other(Box<[u8]>) }
```

*Invariants.* Both `TimeRange` bounds are independently optional, because RFC 4791 section 9.9
says an open-started and an open-ended range are both legal, and at least one is present, because
the same section says that too; `new` refuses `(None, None)` and refuses an end at or before its
start. `CompFilter` bounds its own nesting at construction: `push_comp` refuses a child that would
put the subtree past `limits.max_xml_depth()`, so a tree that exists is a tree the encoder may
recurse over without a stack overflow, and a server decoding an untrusted `REPORT` gets the same
bound from the same field. That closes the filter-recursion gap DP-15 left open.

### Responses

```rust
pub struct MultiStatus { /* private Bounded<DavResponse> */ pub sync_token: Option<SyncToken> }
impl MultiStatus {
    pub fn new(limits: &Limits) -> Self;
    pub fn push(&mut self, response: DavResponse, meter: &mut Meter) -> Result<(), DavError>;
    pub fn read(source: &mut dyn ResponseSource, limits: &Limits, meter: &mut Meter,
                sink: &mut dyn DiagnosticSink) -> Result<Self, DavError>;
}
pub struct DavResponse { pub href: Href, pub body: ResponseBody, pub error: Option<ErrorBody> }
pub enum ResponseBody { Status(Status), PropStats(Bounded<PropStat>) }
pub struct PropStat { pub status: Status, /* private Bounded<DavProperty> */ }
pub struct ErrorBody { /* private Bounded<PropName> */ }
pub struct ResourceType { pub collection: bool, pub calendar: bool, /* private others */ }
```

*Invariants.* `ResponseBody` is two-valued because RFC 4918 section 14.24 is: a response carries
either one status for the whole resource or per-property statuses, and one `href` reporting
`getetag` at `200` and `calendar-data` at `403` in the same breath is ordinary, not exotic.
`DavResponse::successful_value` reads across every propstat whose status is `2xx`, which is the
only correct way to ask "did I get the object" once statuses can diverge.
`MultiStatus::responses()` hands out a slice; nothing hands out the `Vec`.

## What each type serves

| Type | Specification |
| --- | --- |
| `MultiStatus`, `MultiStatusReader`, `ResponseSource` | RFC 4918 section 13, section 14.16 |
| `DavResponse`, `ResponseBody` | RFC 4918 section 14.24 |
| `PropStat`, `DavProperty` | RFC 4918 section 14.22, section 14.18 |
| `Status` | RFC 4918 section 14.28; RFC 9110 section 15 |
| `ErrorBody` | RFC 4918 section 16; RFC 4791 section 5.3.2.1 preconditions |
| `PropFind`, `PropRequest` | RFC 4918 section 9.1, sections 14.2, 14.20, 14.21 |
| `PropName`, `ExtensionName` | RFC 4918 section 14.18, section 17 (extensibility) |
| `ResourceType` | RFC 4918 section 15.9; RFC 4791 section 4.2 |
| `ETag`, `Precondition` | RFC 4918 section 15.6, section 8.6; RFC 9110 sections 8.8.3, 13.1 |
| `Depth` | RFC 4918 section 10.2 |
| `Href` | RFC 4918 section 14.7; RFC 3986 section 4.1 |
| `CalendarQuery`, `CompFilter`, `PropFilter` | RFC 4791 section 7.8, 9.7.1 to 9.7.3 |
| `ParamFilter` | RFC 4791 section 9.7.3 |
| `TextMatch`, `Collation` | RFC 4791 section 9.7.5, section 7.5 |
| `TimeRange` | RFC 4791 section 9.9 |
| `CalendarMultiget` | RFC 4791 section 7.9, section 9.10 |
| `FreeBusyQuery` | RFC 4791 section 7.10, section 9.11 |
| `CalendarDataRequest`, `PropValue::CalendarData` | RFC 4791 section 9.6, sections 9.6.1 to 9.6.5 |
| `ElementName::CalendarHomeSet`, `SupportedCalendarComponentSet` | RFC 4791 sections 6.2.1, 5.2.3 |
| `SyncCollection`, `SyncLevel`, `SyncToken` | RFC 6578 sections 3, 6.1 to 6.3 |
| `Namespace`, `QName`, `XmlPull` | XML Namespaces 1.0 section 6; XML 1.0 sections 2.7, 2.11 |
| `TextPolicy`, `write_text` | XML 1.0 section 2.11, section 4.6; RFC 5545 section 3.1 |
| `Limits`, `Meter`, `Bounded` | ADR-0010; RFC 4918 section 17 hostile-input posture |

## Feature flags

The crate has no default features. Everything RFC 4791 and RFC 4918 define is available under
`--no-default-features`, which is the configuration the `thumbv7em-none-eabi` and
`wasm32-unknown-unknown` gates build.

`sync-collection`, off by default, adds `SyncCollection` and `SyncLevel` — the RFC 6578 `REPORT`
body and its encoder. `SyncToken` and `MultiStatus::sync_token` are unconditional, because reading
a token a server volunteered costs nothing; asking for one is what this flag buys.
`ElementName`'s `sync-collection`, `sync-level`, `limit` and `nresults` rows exist either way, so
a build without the flag answers `DavError::Unsupported` rather than skipping a request it cannot
honor.

No flag changes a type's shape, and no flag is load-bearing for correctness: turning one on adds
items and never alters an existing signature, so feature unification across a dependency graph
cannot change what a compiled caller does.

Four flags were considered and rejected. `alloc` is forbidden as a feature by ADR-0007 — there is
no allocation-free build of these crates, and pretending otherwise in a manifest is how the
lowest-scored A1 proposal broke. `std` buys nothing: `core::error::Error` has been stable since
1.81 and the MSRV is 1.85. `client` and `server` would encode into the build system exactly the
direction split ADR-0004 exists to deny. And `strict-xml`, which would have turned off the
`calendar-data` verbatim carve-out, is a runtime policy instead (`TextPolicy`), because a flag
that changes read behavior is unified across a dependency graph by the union rule, so one crate
in the tree could silently change another's parse.

## Deliberately rejected

**An XML dependency.** `quick-xml` and `roxmltree` are both mature and both forbidden by the
purity gate; this is DP-14's decision, not this document's, and the cost is recorded there.

**A shared `webdav-core` crate.** The multistatus and `PROPFIND` grammar is identical for CardDAV
and for `WebDAV`-sync, and extracting it would be right the day a second consumer exists. There
is none, and extracting after external users depend on `ical-dav`'s internals is worse than
extracting before; that ordering is the acknowledged bet.

**An HTTP request or response type.** No method, no URL, no header map, no `Content-Type`. DP-15
records this as an open gap and it stays open on purpose: the protocol-defined values (`Depth`,
`ETag`, `Precondition`, `SyncToken`) are modeled and rendered, and framing them is the caller's
job with the client it already has.

**`DavResponse { href, status: u16 }`.** A single status per `href` cannot express `getetag` at
`200` beside `calendar-data` at `403`, which real servers send.

**`Option<(Instant, Instant)>` for a time range.** It cannot express the open-started and
open-ended ranges RFC 4791 section 9.9 requires.

**Public `Vec` fields.** Every one of them would be a way around the charged `push` that the whole
limits story rests on.

**`CDATA` on write.** It cannot carry a `CR` past a conformant reader and it makes a literal
`]]>` in a `DESCRIPTION` into an escaping bug. Character references have neither problem.

**A `HashMap` of properties.** Property order is observable in a serializer; the workspace bans
the type for that reason and a sorted structure would be no better, since the order that matters
is the order the peer wrote.

**An `Iterator` implementation for the response reader,** which cannot carry limits, meter and
sink through `next`.

**Evaluating `time-range` or honoring `expand`.** Both are recurrence, both need `ical-recur`, and
`ical-dav` does not depend on it. A server composes the two.

**`MKCALENDAR` and `PROPPATCH` bodies.** Both are property-set grammars rather than report
grammars, and neither is needed to read or write a calendar. Named as deferred, not as absent by
accident.

## Usage

The four functions below are compiled, not quoted: they are the `examples` module of the
skeleton, and `cargo check` builds them. In the shipped crate they import from `ical_core` rather
than from the stand-in module.

### A client builds a `calendar-query`

```rust
pub fn build_calendar_query(limits: &Limits, meter: &mut Meter) -> Result<Vec<u8>, DavError> {
    let mut query = CalendarQuery::new(limits);
    query.props.push(PropName::Known(ElementName::Getetag), meter)?;
    query.props.calendar_data = Some(CalendarDataRequest::default());

    let mut events = CompFilter::new(b"VEVENT", limits, meter)?;
    events.time_range = Some(TimeRange::new(
        Some(Instant::from_unix_seconds(1_767_225_600)),
        Some(Instant::from_unix_seconds(1_767_830_400)),
    )?);

    let mut calendar = CompFilter::new(b"VCALENDAR", limits, meter)?;
    calendar.push_comp(events, limits, meter)?;
    query.filter = Some(calendar);

    let mut body = Vec::new();
    query.write_xml(&mut body, limits, meter)?;
    Ok(body)
}
```

Every part of the request that can grow without bound grows through a charged push, so the body a
caller builds is bounded by the same policy value that will bound the response it reads back.

### A constrained client drains a multistatus without holding it

```rust
pub fn count_readable_events(
    source: &mut dyn ResponseSource,
    limits: &Limits,
    meter: &mut Meter,
    sink: &mut dyn DiagnosticSink,
) -> Result<u32, DavError> {
    let wanted = PropName::Known(ElementName::CalendarData);
    let mut readable: u32 = 0;
    while let Some(response) = source.next_response(limits, meter, sink)? {
        if let Some(PropValue::CalendarData(_)) = response.successful_value(&wanted) {
            readable = readable.saturating_add(1);
        }
    }
    Ok(readable)
}
```

Peak memory is one response. Nothing here needs to know how many the body claims to hold, which
is the only defense that works when the entries may be forgeries.

### A server answers a multiget out of the same types

```rust
pub fn build_multiget_response(
    limits: &Limits,
    meter: &mut Meter,
    ics: &[u8],
) -> Result<Vec<u8>, DavError> {
    let mut body = MultiStatus::new(limits);

    let found = Href::new(b"/calendars/ann/work/1.ics", limits, meter)?;
    let mut response = DavResponse::with_propstats(found, limits);
    let mut readable = PropStat::new(Status::OK, limits);
    readable.push(
        DavProperty {
            name: PropName::Known(ElementName::CalendarData),
            value: PropValue::CalendarData(copy_bytes(ics)?),
        },
        meter,
    )?;
    let mut refused = PropStat::new(Status::FORBIDDEN, limits);
    refused.push(
        DavProperty { name: PropName::Known(ElementName::Displayname), value: PropValue::Empty },
        meter,
    )?;
    response.push_propstat(readable, meter)?;
    response.push_propstat(refused, meter)?;
    body.push(response, meter)?;

    let missing = Href::new(b"/calendars/ann/work/2.ics", limits, meter)?;
    body.push(DavResponse::with_status(missing, Status::NOT_FOUND), meter)?;

    let mut out = Vec::new();
    body.write_xml(&mut out, limits, meter)?;
    Ok(out)
}
```

These are the types the client above reads, with one resource reporting divergent statuses for
two of its properties and another reporting a bare `404`. The direction shows up in which trait is
called and in the `Limits` value passed, never in which fields exist.

### One ledger across many exchanges

```rust
pub fn drain_all(
    sources: &mut [&mut dyn ResponseSource],
    limits: &Limits,
    sink: &mut dyn DiagnosticSink,
) -> Result<Bounded<MultiStatus>, DavError> {
    let mut meter = Meter::with_budget(limits);
    let mut collected = Bounded::with_cap(sources.len());
    for source in sources.iter_mut() {
        let one = MultiStatus::read(*source, limits, &mut meter, sink)?;
        collected.push(one, &mut meter)?;
    }
    Ok(collected)
}
```

This is the shape ADR-0010 exists for: five thousand multigets under one budget are bounded in
aggregate, and five thousand freshly minted meters would not be. Writing `Meter::with_budget`
inside the loop instead is the amplification bug, and nothing in this crate can stop a caller from
writing it — only make it visible.

## What this crate requires of `ical-core`

`Limits`, `Meter`, `LimitExceeded`, `Diagnostic`, `DiagnosticCode`, `DiagnosticSink`,
`SinkOutcome` and `Instant`, with the eight `Limits` accessors the skeleton's `upstream` module
lists: `max_xml_depth`, `max_xml_elements`, `max_body_bytes`, `max_href_bytes`, `max_text_bytes`,
`max_responses`, `max_props`, `max_prefix_bindings`. The last is the one ADR-0010 predicted would
be missing, and it is: namespace declarations are an unbounded dimension of an XML document that
no depth or element count reaches.

`Instant` is the integration seam. ADR-0011 places it in `ical-tz`, DP-17 gives `ical-dav` only
`ical-core`, and RFC 4791 section 9.9 puts a UTC `DATE-TIME` in a `time-range` attribute. Two
crates that do not depend on each other both need to name a UTC instant, so the type belongs in
their common ancestor: `ical-core` should own it or re-export it. Inventing a second name here
would leave the workspace with two instants, which is the failure ADR-0010 spent a document
preventing for limits.

## Consequences

The workspace now maintains an XML tokenizer and writer forever, and this design adds two
obligations to that burden rather than removing any. Namespace resolution is real machinery — a
scoped, bounded prefix-binding stack rebuilt at every element — which is why `quick-xml` exposes
it only through a separate reader mode, and none of the "narrow, closed, cheap to audit"
argument for hand-rolling priced it in. The line-ending carve-out is worse than machinery: it is a
deliberate, scoped violation of XML 1.0 section 2.11, and while it is stated and confined to one
element, it means the phrase "auditable against the XML specification" is now false in a way a
reader has to be told about.

The cap contradiction is named, not solved. `Limits::conservative()` and `Limits::generous()` are
an admission that a defensive read and a correct enumeration want different numbers; the type is
shared and the number is not, which is direction asymmetry relocated from a field into a policy
value. The streaming path removes the memory consequence for a reader that uses it, and nothing
removes it for a server that must answer a forty-thousand-resource collection in one body, because
RFC 4791 gives a `REPORT` no pagination and no way to signal truncation. A server in that position
still has no protocol-conformant answer, and this design does not invent one.

Threading `&Limits` and `&mut Meter` through every constructor makes the ordinary call sites
noisier than they would otherwise be — `Href::new(bytes, limits, meter)` to hold a URL is not
what anyone hopes for from an API — and it still only closes the signature gap. Accepting a meter
and never debiting it compiles today, exactly as ADR-0010 warns, and the encoders here charge at
coarse granularity: a `Status` charges thirteen octets whatever it writes, and `write_range`
charges a flat forty-eight. Those are estimates that happen to be conservative, not accounting.

Three claims in this document are prose backed by no code yet, and they are the three the next
review should attack: that the tokenizer is genuinely iterative and depth-capped, that the
skip-unknown rule keeps a `SabreDAV` response readable, and that `TextRun::Wire` really does
deliver `CRLF` intact. `ResponseSource` and `ReadXml` are declared and unimplemented, which is
what the A1 stress pass found wrong with every proposal it scored, this one's ancestor included.
The corpus of ADR-0006 is where those become evidence; until then they are intentions with
signatures attached.

## What the first compile changed

Assembling the six skeletons into one workspace settled four things this document had left to the
integrator, and none of them cost a signature this crate cares about.

There is no `XmlLimits` and no second `Meter`. The XML bounds — depth, element count, `href` and
text length, response and property cardinality, live prefix bindings — are fields of
`ical_core::Limits`, which is where [ADR 0010](../adr/0010-shared-resource-limits.md)'s decision
puts them, and this crate charges `ical_core::Meter` through `try_charge_bytes` and
`try_charge_element`. A private ledger here would have made "five thousand multigets under one
budget" false at exactly the seam the ADR wrote it for. `LimitExceeded` kept its variants; they
now live in `ical-grammar` and are shared, so a `DavError::Limit` still says which dimension ran
out.

`Diagnostic` is `ical-core`'s, which means a code, a severity and a `Location` rather than a code
and an offset; the reader's `report` helper fills the line number in as zero, because an XML body
has offsets and no content lines. `NullSink` is `ical_core::IgnoreDiagnostics`, and `SinkOutcome`
is the name the workspace kept for the acceptance-or-refusal answer that
[ADR 0009](../adr/0009-error-and-diagnostic-model.md) requires.

`Instant::to_utc_date_time` does not exist. `Instant` is a bare timeline point one layer below
`ical-core`, and rendering it as `YYYYMMDDTHHMMSSZ` is civil arithmetic, so `write_range` calls
`ical_core::utc_date_time_bytes` and reports a refusal through the new
`ValueError::TimeUnrepresentable` rather than through a placeholder that only got the epoch right.

`Limits` is taken by value here, not by reference; see the corresponding section of the
`ical-core` design document for why, and for the lint that decided it.
