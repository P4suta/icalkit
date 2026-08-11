# The `ical-core` public API

- Status: accepted
- Date: 2026-08-10
- Decisions: DP-01 through DP-07 (crate), DP-08, DP-17, DP-18 (workspace)
- Skeleton: assembled with the other five into one workspace and compiled together; see
  "What the first compile changed" below

## Context

`ical-core` turns `.ics` octets into a model and writes the model back out, and it is the only
crate in the workspace that owns storage. It parses the RFC 5545 content line grammar, holds the
component and property tree, hands out typed views over the preserved text, applies scoped
mutations, and serializes. It expands no recurrence, resolves no `TZID`, attaches no meaning to
`METHOD`, opens no connection, and reads no clock. Every crate above it operates on the tree this
one produces, so the invariant it has to carry for all of them is ADR-0001's: nothing the parser
did not understand is lost, and `parse` then `serialize` is byte-identical across the conformance
corpus. Everything below is downstream of that one sentence — the byte-oriented storage, the
absent typed-value cache, the fold layout nobody wanted to store, and the refusal to canonicalize
a bare `LF`.

The skeleton is checked, not asserted. It compiles under `#![no_std]` with `alloc`, it is clean
under the workspace's clippy profile with `pedantic`, `arithmetic_side_effects`, `unwrap_used`
and `missing_docs` all live, and the four examples at the end of this document are compiled and
run against it: two of them assert byte-identical round trips, one on a folded vendor-property
calendar and one on a file with a bare `LF`, a colonless line, a blank line, a case-mismatched
`END`, CP1252 bytes and an unclosed component.

## Decision

### Storage is owned bytes, and one field DP-03 does not list

DP-01 makes `alloc` mandatory and the tree owned: `Document` carries no lifetime parameter, so
DP-07's mutation is not fighting borrowck against a caller's buffer. DP-02 makes that storage
bytes rather than `str`: unfolding is a pure octet operation, a fold may split a codepoint, and
nothing between the fold and a typed accessor is allowed to demand validity.

```rust
pub struct RawText(Box<[u8]>);
impl RawText {
    pub fn from_bytes(bytes: &[u8]) -> Self;
    pub fn from_vec(bytes: Vec<u8>) -> Self;
    pub const fn as_bytes(&self) -> &[u8];
    pub const fn len(&self) -> usize;
    pub const fn is_empty(&self) -> bool;
    pub fn as_str(&self) -> Result<&str, TextError>;   // the only decode point
    pub fn eq_name(&self, other: &[u8]) -> bool;       // ASCII-case-insensitive, RFC 5545 §3.1
}

pub struct TextError { /* wraps core::str::Utf8Error */ }
impl TextError {
    pub const fn valid_up_to(self) -> usize;
    pub const fn error_len(self) -> Option<usize>;
    pub const CODE: DiagnosticCode;                    // = DiagnosticCode::InvalidUtf8Text
}
```

`TextError` keeps the `Utf8Error` rather than collapsing to `Option`, which is DP-02's gate
requirement: a caller can say *where* the bytes went wrong today, and adding detail later is not
a breaking change.

The tree is DP-03's, with private fields and accessors:

```rust
pub struct Parameter  { /* name: RawText, value: RawText, has_value: bool */ }
pub struct Property   { /* name: RawText, parameters: Vec<Parameter>,
                          value_text: RawText, layout: LineLayout */ }
pub enum   Item       { Property(Property), Component(Component) }
pub struct Component  { /* begin: Boundary, items: Vec<Item>, end: Option<Boundary> */ }
pub struct Document   { /* items: Vec<Item> */ }
```

**Invariants.** `Property` has no typed-value field and never will: "typed access is a view" is
enforced by the absence of a second place to keep the answer, not by caller discipline. `Item` is
concrete, non-generic, never `dyn`, and closed at two variants — a content line this crate cannot
make sense of degrades to a `Property`, never to a third variant. `Component::items` is one
ordered heterogeneous sequence with no known/unknown split and no keyed map behind it.

Two fields DP-03's frozen list does not name are here, and they are the reason the round-trip
claim is testable rather than aspirational.

`Property::layout: LineLayout` records the syntax of the content line the property arrived on:
where the producer folded, whether the continuation was indented with `SP` or `HTAB`, whether the
terminator was `CRLF` or a bare `LF` or a bare `CR`, and whether a `:` was present at all. DP-01
requires unfolding into fresh owned buffers before any node is constructed; DP-02's stress
finding requires that an unedited property re-serialize byte-identically. Those two are only
compatible if the fold layout survives the unfold, because real producers fold at 73 octets, or
at 76, or not at all, and a canonical refold would silently rewrite every file in the corpus.

`Component::end: Option<Boundary>` is the second: a component whose `END` never arrived
serializes without one, and a `END:vevent` that disagreed in case with its `BEGIN:VEVENT`
serializes back in the case it was written.

```rust
pub enum LineEnding { CrLf, Lf, Cr }
pub struct FoldPoint { pub offset: u32, pub tab: bool, pub newline: LineEnding }
pub struct LineLayout { /* folds, ending, has_separator, refold */ }
impl LineLayout {
    pub fn canonical(ending: LineEnding) -> Self;   // for lines this crate authored
    pub const fn folds(&self) -> &[FoldPoint];
    pub const fn ending(&self) -> Option<LineEnding>;
    pub const fn has_separator(&self) -> bool;
    pub const fn is_refolded(&self) -> bool;
}
```

`FoldPoint::offset` is an octet position in the *unfolded* content line, counted from the first
octet of the property name, so it addresses the name, the parameters and the value uniformly.

### Structural anomalies degrade to properties

The parser has one recovery rule and it is worth stating on its own, because it is what makes
"never discards the file" mechanical rather than promised. Anything that is not a well-formed
component boundary is stored as an ordinary `Property`:

- a line with no `:` becomes a property with `has_separator == false`, which serializes with no
  `:`; a blank line is the degenerate case, a property with an empty name;
- `BEGIN` or `END` carrying parameters — illegal, and seen in the wild — is a property, with
  `DiagnosticCode::ParametersOnComponentBoundary`, because a `Boundary` has nowhere to keep them;
- an `END` with no matching `BEGIN`, or naming the wrong component, is a property, with
  `UnmatchedEnd` or `MismatchedEndName`;
- a `BEGIN` whose `END` never arrives keeps the component and reports `UnclosedComponent`.

Each of those is a diagnostic *and* a byte-identical round trip, which are not the same claim and
both have to hold.

### The grammar layer, and where it lives

DP-04 makes the token layer mandatory and public rather than an optional convenience, so that
`Document::parse` cannot fork into a second grammar. DP-17 then moved it out into
`ical-grammar`, a no-dependency crate below this one, and D-0003 moved it back: the grammar is
`crates/ical-core/src/grammar/`, a private module tree whose every item this crate's root
re-exports, so `ical_core::Token` is the one spelling of that type and DP-04's public commitment
is unchanged. Nothing outside the layer names its path and nothing inside it names anything
above itself; `gates/grammar-layering` and the second rule of `xtask purity` are what hold that,
and neither is a claim about the crate graph any more (ADR 0004 amendments 12 and 17).

```rust
pub enum Token<'a> {
    Name(&'a [u8]),
    Parameter { name: &'a [u8], value: &'a [u8], has_value: bool },
    Value { bytes: &'a [u8], more: bool },
    EndOfLine { folds: &'a [FoldPoint], ending: Option<LineEnding>, has_separator: bool },
}

pub trait ContentLineSource {
    fn next_token(&mut self) -> Option<Result<Token<'_>, ParseError>>;
}

pub struct ContentLineReader<'a>;
impl<'a> ContentLineReader<'a> {
    pub fn new(input: &'a [u8], limits: GrammarLimits) -> Self;
}
```

Three things about this shape are load-bearing and answer findings the adopted decisions left
open.

**`Token` is byte-shaped.** DP-04's stress finding showed that a `str`-shaped token forces a
choice between rejecting a CP1252 `SUMMARY`, lossily substituting `U+FFFD`, or redefining the
token — and the first two both break M0's acceptance criterion. It is redefined here, in the
direction the finding pointed.

**A value arrives in chunks.** The same finding showed the lifetime-parameterized token has no
"need more input" protocol, so a 400 MB inline `ATTACH;ENCODING=BASE64` would have to be
contiguous and resident before one token could be built. `Token::Value { bytes, more }` is that
protocol: chunks are the runs between folds, they borrow the input and are never buffered by the
reader, and the tree builder is the thing that decides to own them. What that removes is the need
for the *value* to be re-materialized; the *input it borrows from* must still be contiguous and
resident at `ContentLineReader::new`, which has no feed and no resume
([ADR 0007](../adr/0007-allocation-policy.md) amendment 1). Names and parameters *are*
reassembled, through a scratch buffer bounded by `GrammarLimits::max_header_bytes`, because they
have a bound and values do not.

**The trait is object-safe.** `&mut dyn ContentLineSource` is a legal argument and
`impl<T: ContentLineSource + ?Sized> ContentLineSource for &mut T` makes a generic consumer
accept one, which closes DP-04's dissent about dyn-safety before a sibling crate needs it.

`BEGIN` and `END` are ordinary names at this layer. The component model belongs to the crate that
has one.

```rust
impl Document {
    pub fn parse<S: DiagnosticSink + ?Sized>(input: &[u8], limits: Limits, sink: &mut S)
        -> Result<Self, ParseError>;
    pub fn from_tokens<T, S>(tokens: &mut T, meter: &mut Meter, sink: &mut S)
        -> Result<Self, ParseError>
    where T: ContentLineSource + ?Sized, S: DiagnosticSink + ?Sized;
}
```

`parse` builds a `ContentLineReader` and calls `from_tokens`. That is DP-04's gate: there is one
grammar, and a structural test asserts the constructor exists and is what `parse` uses.

### Typed access: one accessor, three states

DP-05 requires that every typed accessor return the same absent / malformed / valid shape, that
the valid value be lifetime-tied to its source, and that a name with no fixed cardinality yield
all matches rather than the first. The shape consistency is not a review convention here; there
is exactly one accessor, and per-property methods are one-line calls into it.

```rust
pub enum View<'a, T> {
    Absent,
    Malformed { source: &'a Property, diagnostic: Diagnostic },
    Valid     { source: &'a Property, value: T },
}
impl<'a, T> View<'a, T> {
    pub fn value(self) -> Option<T>;
    pub const fn source(&self) -> Option<&'a Property>;
    pub const fn diagnostic(&self) -> Option<Diagnostic>;
    pub const fn is_present(&self) -> bool;
}

pub trait DecodeValue<'a>: Sized {
    fn decode_value(bytes: &'a [u8]) -> Result<Self, DiagnosticCode>;
}

impl Property {
    pub fn value<'a, T: DecodeValue<'a>>(&'a self) -> View<'a, T>;   // the only one
}
impl Component {
    pub fn get<'a, T: DecodeValue<'a>>(&'a self, id: &PropertyId) -> View<'a, T>;
    pub fn properties_named<'a>(&'a self, id: &'a PropertyId) -> PropertiesNamed<'a>;
    pub fn dtstart(&self) -> View<'_, DateTimeValue>;   // = self.get(&PropertyId::DTSTART)
    pub fn dtend(&self)   -> View<'_, DateTimeValue>;
    pub fn summary(&self) -> View<'_, TextValue<'_>>;
    pub fn uid(&self)     -> View<'_, TextValue<'_>>;
    pub fn geo(&self)     -> View<'_, Geo>;
    pub fn sequence(&self)-> View<'_, i32>;
}
```

**Invariants.** Both non-absent arms carry `&'a Property`, so a caller always holds the original
text next to the interpretation — which is the entire answer to `GEO`, where the text is
authoritative and the `f64` pair is derived. Nothing decoded is cached; `dtstart()` re-decodes on
every call, and a caller sorting a thousand `VEVENT`s by start time caches at its own call site.
`Component::get` is only for identities RFC 5545 gives a cardinality of at most one; everything
repeatable, which includes every `X-` property and every property from an RFC published after
this code, goes through `properties_named`, which is an iterator and cannot silently keep the
first match.

The two levels are named on different axes, which closes DP-05's dissent about `as_geo()` being
callable on a property that is not `GEO`. `Property`-level accessors are *value type* decoders
(§3.3): `value::<DateTimeValue>()`, `value::<TextValue>()`, `value::<UtcOffset>()`. Component-level
accessors are *property name* accessors (§3.7, §3.8). There is no `Property::geo()` to misapply.

`TextValue` is a borrowed view that validates before it unescapes:

```rust
pub struct TextValue<'a>;
impl<'a> TextValue<'a> {
    pub const fn as_bytes(self) -> &'a [u8];
    pub fn decode(self) -> Result<Cow<'a, str>, TextError>;
}
```

Validate-then-unescape is the only sound order, and it is the order that makes the CP932 trail
byte attack in DP-02's stress record fail closed: every RFC 5545 escape substitution is ASCII, so
none can satisfy a UTF-8 continuation requirement, and an orphaned lead byte fails validation
deterministically. `Cow` borrows when there is nothing to unescape, so the common case allocates
nothing and the escaped case allocates once, without touching storage either way.

### Mutation: a handle, and a refusal

DP-07 scopes mutation to a short-lived borrow rather than a marker value.

```rust
pub struct PropertyMut<'a, T>;
impl<T> PropertyMut<'_, T> {
    pub fn property(&self) -> &Property;
    pub fn set_raw(&mut self, bytes: &[u8]) -> Result<(), MutationError>;
}
impl<T: EncodeValue> PropertyMut<'_, T> {
    pub fn set(&mut self, value: &T) -> Result<(), MutationError>;
}

pub trait EncodeValue {
    fn encode_value(&self, out: &mut ValueBuf) -> Result<(), MutationError>;
}
pub struct ValueBuf;   // implements core::fmt::Write

impl Component {
    pub fn get_mut<T>(&mut self, id: &PropertyId) -> Option<PropertyMut<'_, T>>;
    pub fn dtstart_mut(&mut self) -> Option<PropertyMut<'_, DateTimeValue>>;
}
```

**Invariants.** A `PropertyMut` borrows the whole `Component` mutably and names exactly one
property; reaching another property's storage requires visibly widening the signature. A write
discards the preserved text and the recorded fold layout of that property and of nothing else, so
every other line in the component still serializes byte-identically — which is the property the
mutation round-trip test in `ical-conform` asserts per corpus file. A type that decodes but does
not encode is readable and not writable, which is the honest answer for `Geo`.

`set_raw` refuses control characters. RFC 5545 §3.1 excludes them from a value, and a caller that
could write a bare `CRLF` into one could write a whole new content line after it: a `SUMMARY`
taken from a web form becoming a second `ATTENDEE` is a real injection and not a theoretical one.
This is a write-side check, so it costs the round-trip guarantee nothing.

Two corrections that the first attack pass on this crate forced, both about the word *the* in "the
one place `ical-core` rejects caller input". First, it is a claim about the only door, so
`Property`'s unchecked setters — `set_value_text`, `set_name`, `edit_parameters` — are
crate-private. This document never listed them as public surface and the implementation had made
all three `pub`, which put the identical injection one `items_mut()` away from any caller. A check
repeated on each would have closed two of the three, since `edit_parameters` hands out a
`&mut Vec<Parameter>` and no check stands in front of a reference after it is returned. Second,
the value side was never the only channel: `ParameterEdit` carries a *value*, so writing one means
choosing its §3.2 spelling, and there are two answers rather than one. A value carrying `:` `;` or
`,` is written inside a `DQUOTE` pair, which `quote_parameter` already knew how to
do and nothing called; one carrying a `DQUOTE` or a control character has no §3.2 spelling at all
(`parameter_is_representable`), and is `MutationError::NotRepresentable`. So there are two refusals
and they are both write-side, both stated over octets that were never read from a file.

The change vocabulary `ical-itip` reuses (DP-13) lives here rather than there:

```rust
pub struct ParameterEdit;
impl ParameterEdit {
    pub fn set(name: &[u8], value: &[u8]) -> Self;   // assign
    pub fn remove(name: &[u8]) -> Self;              // unassign
    pub fn name(&self) -> &[u8];
    pub fn value(&self) -> Option<&[u8]>;
}

pub enum ProposedChange {
    Add(RawText),                        // a whole content line
    Replace(RawText),                    // name, parameters and value, all at once
    SetParameters(Vec<ParameterEdit>),   // the value's text is untouched
    Remove,
}
impl Component {
    pub fn apply(&mut self, id: &PropertyId, change: &ProposedChange, limits: Limits)
        -> Result<(), MutationError>;
}
```

`ical-itip`'s `Transition` is `BTreeMap<PropertyId, ProposedChange>` — a map so that two
conflicting changes to one property cannot both be constructed — and applying it is one `apply`
per entry. A `Transition` built out of these is inert: it describes, and only `apply` acts.

`SetParameters` is the variant that earns its place. A `RANGE=THISANDFUTURE` edit changes a
parameter and not a value, and expressing it as a `Replace` would discard the value's preserved
text to say something that was never about the value. DP-07 leaves parameter-level invalidation
granularity open; this is the half of it the change vocabulary can answer today, and the layout
is discarded for the whole line because the parameters are part of it.

`apply` takes `Limits` because an iTIP-supplied replacement line is bytes off the wire like any
other, and it is parsed through the same `ContentLineReader` everything else goes through. A
replacement that is empty, malformed, or more than one line is
`MutationError::MalformedReplacement`.

`PropertyId` is the map key, and it is deliberately not a closed enum:

```rust
pub struct PropertyId(/* &'static [u8] or an owned uppercased name */);
impl PropertyId {
    pub const fn from_static(name: &'static [u8]) -> Self;
    pub fn from_name(name: &[u8]) -> Self;      // ASCII-uppercases
    pub fn as_bytes(&self) -> &[u8];
    pub const DTSTART: Self;  pub const SUMMARY: Self;  /* 25 well-known names */
}
```

`Ord`, `Eq` and `Hash` are hand-written over `as_bytes()`, so a `&'static` name and an owned one
compare as equal and sort together; a derived `Ord` would have sorted by representation, which is
the kind of bug that shows up only once a vendor property reaches a `BTreeMap`. The identity is
normalized because RFC 5545 names are case-insensitive; the *spelling* stays on the `Property`.

### Limits, the meter, and what is fatal

DP-08 puts one `Limits` value in `ical-core` and a running meter everywhere. Both are here, and
every sibling reads the fields that concern it.

```rust
pub struct GrammarLimits { /* max_header_bytes, max_parameters, max_folds_per_line */ }
pub struct Limits { /* grammar, max_input_bytes, max_value_bytes, max_component_depth,
                      max_items, candidates_per_period, override_entries,
                      max_vtimezone_observances, max_vtimezone_components */ }
impl Limits {
    pub const DEFAULT: Self;
    pub const fn grammar(self) -> GrammarLimits;
    pub const fn with_max_input_bytes(self, bytes: u64) -> Self;   /* and siblings */
}

pub struct LimitExceeded;   // field-free

pub struct Meter;           // neither Copy nor Default
impl Meter {
    pub const fn new(limits: Limits) -> Self;
    pub const fn with_budget(limits: Limits, budget: u64) -> Self;
    pub fn charge(&mut self, units: u64) -> bool;                          // latching
    pub fn try_charge(&mut self, units: u64) -> Result<(), LimitExceeded>; // the same, as Result
    pub fn charge_bytes(&mut self, count: u64) -> Result<(), ParseError>;
    pub fn charge_item(&mut self) -> Result<(), ParseError>;
    pub fn enter(&mut self) -> Result<(), ParseError>;
    pub fn leave(&mut self);
    pub const fn is_exhausted(&self) -> bool;
    pub const fn spent(&self) -> u64;
    pub const fn limits(&self) -> Limits;
}
```

`Limits` is `Copy` with private fields and `with_*` builders, so adding a field is not a breaking
change and no caller can construct one that skips a bound. `candidates_per_period` is defined
over candidates *generated*, not instances emitted, which is the definition that closes the
negative-`BYSETPOS` blowup DP-08 names. `max_vtimezone_observances` and
`max_vtimezone_components` are `ical-tz`'s; XML nesting depth, element count, `href` and text
length, response and property cardinality and live prefix bindings are `ical-dav`'s, and they are
fields of this same `Limits` rather than a typed `XmlLimits` sibling, because one meter cannot
charge against two policies. The cost lands on this crate's own readers: a caller who never
speaks CalDAV still carries eight thresholds that only bound XML.

`Meter` is neither `Copy` nor `Default` on purpose: minting a fresh meter inside a fan-out loop
is how a budget silently stops binding, and it should be a visible act. Exhaustion latches.
`charge` returning `bool` is the shared primitive because `ical-recur` needs a budget breach to
be a reported outcome it can keep iterating past (ADR-0002); `try_charge` is the identical charge
for callers whose surrounding code is already `Result`-shaped, which `ical-itip` and `ical-dav`
both are; and `ical-core`'s own parser converts the `false` into a `ParseError`, because a
document it cannot finish reading is not one it can hand back. Three shapes over one accounting,
rather than three accountings.

### Errors and diagnostics

DP-06 splits the two, and this crate holds both for the whole family.

```rust
pub enum ParseError {           // no boundary left to recover to
    InputTooLarge { limit: u64 },  ValueTooLarge { limit: u32 },
    HeaderTooLarge { limit: u32 }, TooManyParameters { limit: u32 },
    TooManyItems { limit: u32 },   TooDeep { limit: u16 },
    TooManyFolds { limit: u32 },
}
pub enum Severity { Note, Violation, LimitReached }
pub enum DiagnosticCode { /* #[non_exhaustive], semver-stable, golden-listed */ }
impl DiagnosticCode { pub const fn as_str(self) -> &'static str; }

pub struct Diagnostic;          // #[non_exhaustive], Copy
impl Diagnostic {
    pub const fn new(code: DiagnosticCode, severity: Severity, location: Location) -> Self;
    pub const fn at_instant(code: DiagnosticCode, severity: Severity, instant: Instant) -> Self;
    pub const fn code(self) -> DiagnosticCode;
    pub const fn severity(self) -> Severity;
    pub const fn location(self) -> Location;
    pub const fn instant(self) -> Option<Instant>;
}

pub trait DiagnosticSink { fn push(&mut self, diagnostic: Diagnostic); }
impl DiagnosticSink for Vec<Diagnostic> {}
impl<S: DiagnosticSink + ?Sized> DiagnosticSink for &mut S {}
pub struct IgnoreDiagnostics;
```

**Invariants.** `ParseError` is the small set where nothing can be constructed at all, and every
variant of it is a caller bound that guards memory. Everything else — every specification
violation, every recovery listed above — is a `Diagnostic`. `DiagnosticCode::as_str` is the
golden-list key: CI holds a committed `code -> one-line meaning` table and fails on an edit that
is not an addition, which is what lets ADR-0006's corpus assert "this input produces `X`" across
releases. `DiagnosticSink` is push-only and object-safe, so an allocating caller passes
`&mut Vec<Diagnostic>` and a `thumbv7em` caller with no allocator passes `IgnoreDiagnostics` or a
fixed-capacity sink of its own; the promise that a violation never discards the file holds with
or without an allocator linked. `Diagnostic::at_instant` exists so `ical-recur` and `ical-tz`,
which report about occurrences that exist at no offset in any file, use the same sink rather than
each inventing one.

**One deviation from DP-06, recorded rather than smuggled.** DP-06 asks that an oversized
`DESCRIPTION` be "truncated-and-flagged, not fatal". Truncation is refused here: it writes back
fewer bytes than it read, which contradicts ADR-0001 directly, and a crate cannot promise both.
Per-value size is therefore a `ParseError::ValueTooLarge` against a caller-tunable
`max_value_bytes` that defaults to 1 MiB. `Severity::LimitReached` stays in the enum and is where
DP-06's graduated rule genuinely lives: `ical-recur`'s candidate budget and `ical-dav`'s response
caps, where the work can be abandoned without losing input that was already read.

### Where the civil-time primitives live

DP-12 assigns the checked civil-date arithmetic to `ical-tz`'s ADR, but the *types* have to sit
here, and the reason is the crate graph rather than taste: `ical-recur` and `ical-tz` are
siblings, `ical-dav` needs `Instant` for `time-range` filters and does not depend on `ical-tz`,
and inherent methods cannot be added from downstream. So `ical-core` owns what a date *is* and
`ical-tz` owns what it *means* under a zone.

```rust
pub struct CivilDate;  pub struct CivilTime;  pub struct CivilDateTime;
pub struct Instant;    pub struct UtcOffset;  pub struct Duration;
pub enum   Weekday;    pub enum MonthAddOutcome { Exact(CivilDate),
                                                 Clamped { date: CivilDate, requested_day: u8 },
                                                 Overflow }
pub enum   DateTimeValue { Date(CivilDate), Local(CivilDateTime), Utc(CivilDateTime) }

impl CivilDate {
    pub const fn from_ymd(year: u16, month: u8, day: u8) -> Option<Self>;
    pub const fn is_leap_year(year: u16) -> bool;
    pub const fn days_in_month(year: u16, month: u8) -> Option<u8>;
    pub fn days_from_epoch(self) -> Option<i64>;
    pub fn weekday(self) -> Option<Weekday>;
    pub fn add_months(self, count: i32) -> MonthAddOutcome;   // the only month-stepping door
}
impl MonthAddOutcome {
    pub fn exact(self) -> Option<CivilDate>;         // Some for Exact only
    pub fn carried_date(self) -> Option<CivilDate>;  // the date carried, clamped included
}
impl CivilDateTime { pub fn at_offset(self, offset: UtcOffset) -> Option<Instant>; }
impl Instant {
    pub const fn from_unix_seconds(seconds: i64) -> Self;
    pub const fn unix_seconds(self) -> i64;
    pub const fn checked_add_seconds(self, seconds: i64) -> Option<Self>;
    pub fn to_civil(self, offset: UtcOffset) -> Option<CivilDateTime>;
}
```

**Invariants.** Every operation is total: `checked_*`, `div_euclid`, `rem_euclid`, and a failure
is `None` or a named `MonthAddOutcome` variant, never a panic and never a wrap. `Duration` has
`days` and `seconds` and no year or month field, because RFC 5545 §3.3.6's ABNF has no `Y` or `M`
designator — `P1M` is not a value this type could hold, which closes the "add a month to a date"
framing at the type level. `MonthAddOutcome::Clamped` keeps the day that was asked for so
`ical-recur` can obey §3.3.10's requirement that a generated instance which does not exist be
*ignored*, not clamped, and still report why it vanished. `DateTimeValue` deliberately carries no
`TZID`: `TZID` is a parameter, not part of the value, and keeping them apart is what lets
`ical-tz`'s `ZoneSource::resolve(tzid, local)` take this type by value.

### Writing it back

```rust
pub trait Writer {
    type Error;
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}
impl Writer for Vec<u8> { type Error = core::convert::Infallible; }
impl<W: Writer + ?Sized> Writer for &mut W {}

impl Document {
    pub fn serialize<W: Writer + ?Sized>(&self, out: &mut W) -> Result<(), W::Error>;
    pub fn to_bytes(&self) -> Vec<u8>;
}
```

`core` has no `io::Write` and `core::fmt::Write` takes `&str`, which is exactly what storage is
not, so the sink is this crate's own trait. It has an associated error rather than a fixed one so
a caller writing into a socket buffer does not pay for an error type it cannot produce, and
`Vec<u8>` uses `Infallible`.

Serialization walks the tree once. For each line it writes the name, the parameters, the `:` if
one was present and the value, counting octets as it goes and injecting the recorded fold at each
`FoldPoint::offset` with the whitespace and the terminator the producer used. A line whose layout
was discarded by a write is refolded canonically at 74 octets, backing off to the previous UTF-8
boundary rather than splitting a codepoint — which is legal per §3.1 but is the thing that breaks
every naive consumer, so this crate declines to emit it even though it must accept it.

### What each type serves

| Type | Specification |
| --- | --- |
| `ContentLineReader`, `Token`, `ContentLineSource` | RFC 5545 §3.1 content lines |
| `LineLayout`, `FoldPoint`, `LineEnding` | §3.1 folding, at octet boundaries |
| `RawText`, `TextError` | §3.1 octet storage; UTF-8 as a §3.1.4 charset question |
| `Parameter`, `ParametersNamed` | §3.2 property parameters |
| `Parameter::unquoted` | §3.2 `DQUOTE`-delimited parameter values |
| `ValueType` | §3.2.20 `VALUE`, naming §3.3.1 through §3.3.14 |
| `TextValue` | §3.3.11 `TEXT` and its escaping |
| `CivilDate`, `CivilTime`, `CivilDateTime` | §3.3.4 `DATE`, §3.3.5 `DATE-TIME`, §3.3.12 `TIME` |
| `DateTimeValue` | §3.3.4 and §3.3.5 as a property value writes them |
| `Duration` | §3.3.6 `DURATION` (no `Y`, no `M`) |
| `Geo` | §3.3.7 `FLOAT`, as used by §3.8.1.6 `GEO` |
| `UtcOffset` | §3.3.14 `UTC-OFFSET`, §3.8.3.3, §3.8.3.4 |
| `Weekday`, `MonthAddOutcome` | §3.3.10 `RECUR` arithmetic, consumed by `ical-recur` |
| `Instant` | no RFC type: the UTC scalar `ical-recur`, `ical-tz` and `ical-dav` share |
| `Property`, `Item`, `Component`, `Document` | §3.4 the iCalendar object, §3.6 components |
| `PropertyId` | §3.7 calendar properties, §3.8 component properties |
| `Component::dtstart`, `dtend` | §3.8.2.4, §3.8.2.2 |
| `Component::summary`, `uid`, `geo`, `sequence` | §3.8.1.12, §3.8.4.7, §3.8.1.6, §3.8.7.4 |
| `Component::properties_named` | §3.8.8.2 `X-` properties, and every later-RFC property |
| `PropertyMut`, `ValueBuf`, `EncodeValue` | not RFC 5545: ADR-0001's mutation boundary |
| `ProposedChange`, `ParameterEdit` | not RFC 5545: RFC 5546 §3.2 transitions, via `ical-itip` |
| `Limits`, `GrammarLimits` | not RFC 5545: ADR-0002's and ADR-0004's hostile-input posture |
| `Meter`, `LimitExceeded` | not RFC 5545: the running half of the same posture |
| `Diagnostic`, `DiagnosticCode`, `Severity` | not RFC 5545: ADR-0001 and ADR-0006 |
| `DiagnosticSink`, `IgnoreDiagnostics` | not RFC 5545: ADR-0004's no-allocator targets |
| `Writer` | not RFC 5545: `core` has no `io::Write` |

`RRULE` is the deliberate hole. `PropertyId::RRULE` exists, its value stays preserved text, and
`ValueType::Recur` names it — the §3.3.10 grammar is `ical-recur`'s and is not parsed here.

### Feature flags

One, off by default.

| Feature | Effect |
| --- | --- |
| *(none)* | `#![no_std]` plus `alloc`. The whole API above. |
| `std` | Adds `extern crate std` and a `Writer` adapter over `std::io::Write`. |

The default build is what CI checks on `thumbv7em-none-eabi` and `wasm32-unknown-unknown`, and
it is the only configuration the purity gate verifies (DP-18). The `std` feature adds no type,
changes no signature and gates nothing off; `core::error::Error` is implemented unconditionally,
so error interoperability does not need it.

Four flags that are deliberately absent, because a flag is a configuration somebody has to test:

- **`alloc`.** DP-01 makes allocation mandatory, not optional. A genuinely alloc-free tier is a
  named, deferred gap for a future separate crate with its own lint profile, not a feature on
  this one — a `no-alloc` cfg would double the build matrix for a tier nobody has built.
- **`serde`.** An outside dependency, which the purity gate forbids. A caller who wants JSON
  writes it against `Document::items()`, which is public and ordered.
- **Anything gating the token layer.** DP-04 makes it mandatory; a feature that removed it would
  let `Document::parse` fork into a second grammar, which is the failure DP-04 exists to prevent.
- **A "strict" or "lenient" mode.** Strictness is a caller reading the diagnostics, not a
  compile-time configuration. Two parse behaviors would be two corpora.

### Using it

These four are the shapes the crate's own tests exercise; the harness that once compiled them
beside this document is gone, and the crate's `tests/round_trip.rs` and `tests/edit_locality.rs`
are where they are held now.

**1. Parse, read, and write it back byte for byte.** The assertion under it is the whole crate's
reason for existing.

```rust
use ical_core::{Diagnostic, Document, Limits, ParseError, PropertyId, View};

fn round_trip(bytes: &[u8]) -> Result<(Vec<u8>, Vec<Diagnostic>), ParseError> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let document = Document::parse(bytes, Limits::default(), &mut diagnostics)?;

    for calendar in document.components() {
        for event in calendar.components().filter(|c| c.is_named(b"VEVENT")) {
            match event.dtstart() {
                View::Valid { value, source } => {
                    println!("dtstart {value:?} from {:?}", source.value_text().as_bytes());
                }
                View::Malformed { diagnostic, source } => {
                    println!("bad dtstart {diagnostic}, keeping {:?}", source.value_text());
                }
                View::Absent => println!("no dtstart"),
            }
            if let Some(text) = event.summary().value() {
                match text.decode() {
                    Ok(summary) => println!("summary {summary}"),
                    Err(error) => println!("summary is not UTF-8: {error}"),
                }
            }
            let vendor = PropertyId::from_name(b"X-MICROSOFT-CDO-BUSYSTATUS");
            for property in event.properties_named(&vendor) {
                println!("vendor {:?}", property.value_text().as_bytes());
            }
        }
    }

    Ok((document.to_bytes(), diagnostics))
}
```

```rust
// Holds for a folded, vendor-decorated calendar, and for one with a bare LF, a colonless
// line, a blank line, a case-mismatched END, CP1252 bytes and an unclosed component:
assert_eq!(round_trip(input)?.0, input);
```

**2. Count events without building a tree,** on a device that could not hold one. The argument is
`&mut dyn ContentLineSource`, so this function does not care whether the tokens came from a file,
a socket, or a filter wrapped around either.

```rust
use ical_core::{ContentLineSource, ParseError, Token};

fn count_events(source: &mut dyn ContentLineSource) -> Result<usize, ParseError> {
    let mut events = 0_usize;
    let mut in_begin = false;
    while let Some(token) = source.next_token() {
        match token? {
            Token::Name(name) => in_begin = name.eq_ignore_ascii_case(b"BEGIN"),
            Token::Value { bytes, .. } if in_begin && bytes.eq_ignore_ascii_case(b"VEVENT") => {
                events += 1;
            }
            _ => {}
        }
    }
    Ok(events)
}
```

**3. Move one property and leave every other byte alone.** This is the scenario DP-03's stress
record calls out: the vendor properties beside `DTSTART` are exactly the ones ADR-0001 exists to
protect, and the second assertion is what a caller is entitled to rely on.

```rust
use ical_core::{CivilDateTime, Component, DateTimeValue, Document, MutationError};

fn move_start(event: &mut Component, at: CivilDateTime) -> Result<(), MutationError> {
    let mut handle = event.dtstart_mut().ok_or(MutationError::Absent)?;
    handle.set(&DateTimeValue::Utc(at))
}

fn move_all_starts(document: &mut Document, at: CivilDateTime) -> Result<(), MutationError> {
    for calendar in document.components_mut() {
        for event in calendar.components_mut().filter(|c| c.is_named(b"VEVENT")) {
            move_start(event, at)?;
        }
    }
    Ok(())
}
```

```rust
move_all_starts(&mut document, noon)?;
let moved = document.to_bytes();
assert!(moved.windows(4).any(|w| w == b"1200"));       // the edit happened
assert!(moved.windows(4).any(|w| w == b"FREE"));       // the vendor property survived
assert!(moved.windows(9).any(|w| w == b"with the\r")); // an untouched fold survived
```

**4. Share one budget across the stack.** The meter outlives the parse, so what `ical-recur` and
`ical-tz` spend afterwards is charged against the same ceiling the caller stated once.

```rust
use ical_core::{ContentLineReader, Document, IgnoreDiagnostics, Limits, Meter};

fn parse_within(bytes: &[u8]) -> (Option<Document>, Meter) {
    let limits = Limits::default().with_max_input_bytes(4096);
    let mut meter = Meter::new(limits);
    let mut reader = ContentLineReader::new(bytes, limits.grammar());
    let document = Document::from_tokens(&mut reader, &mut meter, &mut IgnoreDiagnostics).ok();
    (document, meter)   // meter.spent(), meter.is_exhausted() travel onward
}
```

```rust
let calendar: Vec<u8> = b"X-FILLER:0123456789\r\n".repeat(900);
let (_, tight) = parse_within(&calendar);
assert!(tight.is_exhausted());   // a budget that binds, binds
```

The oversized input has to be *content lines* for this to be the bound it demonstrates. Nine
thousand octets with no `:` and no terminator are one content line header, and `max_header_bytes`
refuses them at octet 4097 — before the meter is charged once, so the parse fails with
`HeaderTooLarge` and leaves `is_exhausted()` false. The two bounds guard different things and
neither stands in for the other: the header ceiling bounds what the reader must buffer to lex one
line, the budget bounds what the tree may hold in total.

A fifth thing worth showing, because it is the one place the crate refuses caller input rather
than diagnosing it:

```rust
let mut handle = event
    .get_mut::<TextValue<'_>>(&PropertyId::SUMMARY)
    .ok_or(MutationError::Absent)?;
assert_eq!(
    handle.set_raw(b"hi\r\nATTENDEE:mailto:eve@example.test"),
    Err(MutationError::IllegalControlCharacter),
);
```

### For the integrator

Five sibling designs were written against the same decisions, and each reproduces the `ical-core`
items it consumes as a local stand-in module. Those stand-ins agree on nearly everything; where
they did not, this is what was taken and what it costs.

- **Taken from `ical-tz`.** `CivilDate::from_ymd`, `CivilTime::from_hms`, `days_in_month`,
  `is_leap_year`, `days_from_epoch`, `weekday`, `add_months`, `MonthAddOutcome`, `Weekday`,
  `CivilDateTime::at_offset`, `Instant::{unix_seconds, checked_add_seconds, to_civil}`,
  `UtcOffset::{UTC, from_seconds -> Option, seconds}`, and `Duration { days, seconds }` — names
  and bodies both. `ical-tz`'s `upstream` module deletes cleanly against this file.
- **Taken from `ical-recur`.** `Meter::charge -> bool` with latching exhaustion, `spent`,
  `budget`, `is_exhausted`, `with_budget`, and `Meter` being neither `Copy` nor `Default`. Also
  `Limits::candidates_per_period` and `override_entries`, spelled as `ical-recur` spells them.
- **Taken from `ical-itip`.** `ProposedChange`'s four variants, `ParameterEdit`, and
  `LimitExceeded` with `Meter::try_charge` beside `charge`. `Component::apply` gained a `Limits`
  argument as a consequence, since `Replace` carries an untrusted content line.
- **Not taken: `Limits` being non-`Copy`.** `ical-itip` makes it non-`Copy` reasoning that a
  `Copy` value under `trivial-copy-size-limit = 128` trips `clippy::trivially_copy_pass_by_ref`.
  It does — on `&Limits` receivers. `Limits` here is `Copy` and every accessor takes `self` by
  value, which was checked against the workspace's actual clippy profile and is clean. Call sites
  in `ical-itip` and `ical-dav` that write `&Limits` should drop the `&`.
- **Not taken: `Diagnostic` with public fields.** `ical-tz` reads `diagnostic.code`; this crate
  offers `diagnostic.code()`, because `Diagnostic` is `#[non_exhaustive]` and gained an
  `instant: Option<Instant>` field so `ical-recur` and `ical-tz` can report about an occurrence
  rather than a byte offset.
- **Not taken: `Instant` from `ical-tz`.** `ical-itip` imports it as `ical_tz::Instant`. It is
  `ical_core::Instant`; `ical-dav` needs it for `time-range` and does not depend on `ical-tz`.

`DiagnosticCode` is one enum for the workspace, so sibling-contributed variants land here — the
tz and recur codes their skeletons name are already in it. That is deliberate: the golden list
DP-06 requires is a single table, and a per-crate code enum would make "input X produces code Y"
a claim about which crate happened to notice.

## Rejected

**A cached typed value on `Property`** (DP-03's runner-up, and the ergonomic choice). Two places
to keep one answer is two places for them to disagree, and the field's absence is the only
enforcement that survives a contributor who has not read this document. The documented fallback
if profiling ever demands it is a cache keyed to a generation counter, not a bare second field.

**`String` or `&str` storage** (DP-02). It cannot hold a fold that splits a codepoint or a CP1252
`SUMMARY`, and both are in the corpus. The cost is that every typed text read is fallible.

**A lifetime parameter on `Document`** (DP-01's zero-copy alternative). Borrowing the caller's
buffer saves the copy and then loses the argument with borrowck the first time DP-07's mutation
handle needs a `&mut`. Every parsed value is copied out of the input instead, permanently.

**A canonical refold on write.** Cheaper, smaller, and it rewrites every file in the corpus on
the first save. `LineLayout` is the price of not doing that.

**Truncating an oversized value** (DP-06's stated preference). Recorded above with its reasoning:
it contradicts ADR-0001, so the bound is fatal and tunable instead.

**`Option<&Property>` for extension properties** (the incumbent draft's own shape, and five of
seven blind proposals'). RFC 5545 puts no repeat limit on `X-` properties; a singular lookup
silently keeps the first and drops the rest, which is the data loss this crate exists to prevent,
arriving through the accessor instead of through the parser.

**One error enum** (DP-06). Merging "the file has no recoverable boundary" with "this `DTSTART`
is malformed" forces callers to match on variants that mean opposite things about whether they
still have a document.

**A `Dirty<T>` or marker-value mutation API** (DP-07's runner-up, and five of seven proposals').
A returned marker looks like enforcement and is discardable. The borrow is not. The cost is that
edits cannot be batched or held across an await.

**A closed enum of property names.** Const-constructible and orderable, and it re-introduces the
known/unknown split DP-03 spent the tree's shape to avoid. `PropertyId` gets both from a
`&'static` / owned split with hand-written `Ord`.

**A `HashMap` index of properties by name.** Faster lookup, non-deterministic iteration, and the
serializer's output is observable to anything that diffs or signs it. `clippy.toml` forbids it
outright.

**Bundling the grammar with the model** (DP-17). A fuzz harness that wants folding and escaping
should not compile `CivilDate`. The seam costs one more published crate and one more entry in
every cross-target job, and it is insurance rather than demonstrated demand until a syntax-only
caller actually appears. **That rejection is reversed by
[ADR 0004](../adr/0004-sans-io-protocol-layer.md) amendment 12: the insurance is bought as a
compilation rather than as a published crate, and the footprint saving it was priced against
measured at zero on three targets.**

## Consequences

Every typed read costs a decode. A caller iterating a thousand `VEVENT`s by `DTSTART` pays for a
thousand decodes per pass and must cache at its own call site; the crate will not do it for them,
and that is a deliberate trade of peak ergonomics for a structural single-source-of-truth.

Every typed text read is fallible and returns `Cow`, so `&str`-only ecosystem crates need a
conversion at the boundary. And because decoding is lazy, a spec-violating property that nothing
ever reads carries invalid bytes through parse, round trip and even iTIP processing without ever
producing a diagnostic. "Byte-identical round trip" and "a violation is always visible" are in
real tension, and this design delivers the first fully and the second only for the properties a
caller touches. An eager parse-time UTF-8 sweep over text-typed properties would close it and is
logged as a follow-up, not adopted here.

`Limits` is a required argument at every entry point, which is a breaking signature change across
three crates whenever a bound is added — mitigated by private fields, but still a semver event.
And nothing compels a function that accepts a `Meter` to actually charge it before its next
allocation; that stays a review obligation until a gate exists.

Four things this document does not settle, named rather than assumed. Parameter-level
invalidation granularity — whether changing `RANGE=THISANDFUTURE` invalidates that parameter's
text or the whole property's — is deferred to ADR-0004, which needs it for `ETag`-conditional
writes. Cross-property invariants are not a mutation-API concern at all: `RRULE`'s `UNTIL` must
share `DTSTART`'s value type, `DTEND` and `DURATION` must not both appear, and
`X-MICROSOFT-CDO-ALLDAYEVENT` must not say `TRUE` beside a timed `DTSTART` — all of them surface
as diagnostics, none of them is caught at the call site, and DP-03's stress record is right that
removing the typed cache bought no protection against any of them. The `DiagnosticCode` golden
list is a mechanism this document requires and `xtask` builds: `docs/diagnostic-codes.md` carries
a row per code and `just codes` fails when a row's meaning or channel moves without the
declaration moving with it. And the genuinely alloc-free tier
remains an unimplemented gap: today's `no_std` gate proves these crates build for embedded
targets, not that they build without a global allocator.

## What the first compile changed

Six skeletons were written against each other's prose and then assembled into one workspace and
compiled together for the first time. `ical-core` absorbed most of the reconciliation, because it
owns the vocabulary the other five spell differently.

The grammar seam of [ADR 0004](../adr/0004-sans-io-protocol-layer.md) is real: the items under
`grammar` shipped from `ical-grammar`, and `ical-core` re-exported them. **D-0003 has since
collapsed the crate into this one; the seam is a module layer and the three consequences below
were recorded when it was a crate boundary.** Three consequences were not visible on paper.
`Instant` went down with them — `Diagnostic` names an instant, because
`ical-recur` and `ical-tz` report about occurrences that exist at no byte offset, so the type has
to sit under the diagnostic vocabulary; `Instant::to_civil` became `CivilDateTime::from_instant`,
since a crate may not write an inherent method for another crate's type, and
`utc_date_time_bytes` is a free function here for the same reason. `LineLayout` and `TextValue`
gained public constructors — `LineLayout::preserved`, `LineLayout::mark_refolded`,
`TextValue::from_bytes` — because the parser that used to write those fields directly is now on
the far side of a crate boundary; a private struct's fields became API. And `Token` is
`#[non_exhaustive]`, so this crate's own builder now needs a catch-all arm: adding a token variant
no longer breaks the one consumer that must handle it, which is a guarantee the split spent.
**That last clause is false and is corrected rather than reinterpreted: the attribute binds only
crates other than the defining one, so the catch-all arm was never what the split bought. It was
a silent-loss path against ADR 0001, and the collapse deleted it — both of them, since `mutate`
carried one too — while keeping the attribute. `unreachable_patterns = "deny"` was recorded here
as what keeps another from being written and does not: it rejects a catch-all after every variant
is covered, while the shape that drops a payload omits a variant. The fourth rule of
`xtask purity` reads the arms (ADR 0004 amendment 18). The three constructors named above are
still public and are still owed the change to crate-private, which the collapse did not carry:
they are named only by this crate now, so nothing outside it would notice.**

`DiagnosticCode` is one enum in the grammar layer carrying every code in the workspace, including
the ones only `ical-tz` or `ical-dav` can produce. ADR 0004 says `ical-core` "adds only the kinds
it alone can detect", and Rust has no extensible enum, so the choice was one vocabulary defined at
the bottom or two vocabularies to reconcile at the seam. The ADR forbids the second, so the
bottom layer enumerates codes it cannot itself emit. That is the honest cost, and it is also why the
golden list is a workspace artifact rather than a per-crate one.

`DiagnosticSink::push` returns `SinkOutcome` rather than `()`, which is what
[ADR 0009](../adr/0009-error-and-diagnostic-model.md) decided and this skeleton had not caught up
with. `LimitExceeded` became a `#[non_exhaustive]` enum naming the dimension that ran out, because
`ical-dav` distinguishes a body that was too long from a nesting that was too deep from an `href`
that was too long, and "some bound was crossed" is not something a caller can tune a policy on;
the field-free version cost nothing to widen and a discriminant is not an allocation.

`Limits` grew the fields its siblings needed — `max_payload_components` and `max_attendees` for
`ical-itip`, and the XML bounds for `ical-dav` that
[ADR 0010](../adr/0010-shared-resource-limits.md) lists in its decision and then leaves open in
its consequences. There is no `XmlLimits`. `Limits::GENEROUS` is the second named policy the two
crates each invented separately, once. `Meter` gained `try_charge_bytes` and `try_charge_element`
so that `ical-dav` charges the shared ledger instead of keeping its own, which is the whole point
of one meter. Every field of `Limits` and `GrammarLimits` now carries exactly one calibration
marker saying whether it is the stated envelope, derived from it by a written function, measured
against this reader, argued from shape, or merely asserted; "provisional" is no longer a property
of the type as a whole ([ADR 0010](../adr/0010-shared-resource-limits.md) amendment 1). A new
field may not be added without one.

Every entry point takes `Limits` by value rather than `&Limits`. ADR 0010 spells it `&Limits`, and
`clippy::trivially_copy_pass_by_ref` under this repository's own `trivial-copy-size-limit` rejects
that for a `Copy` type of this size — thirty-three times, in two crates. Only `&mut Meter` was
ever load-bearing: the policy is immutable and cheap, the ledger is the thing whose lifetime has
to be the caller's.
