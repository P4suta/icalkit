# ical-conform: the public API

- Status: draft
- Date: 2026-08-10
- Skeleton: assembled with the other five into one workspace and compiled together; see
  "What the first compile changed" below
- Governed by: [ADR 0006](../adr/0006-conformance-corpus-as-artifact.md), with
  [ADR 0001](../adr/0001-lossless-round-trip.md),
  [ADR 0009](../adr/0009-error-and-diagnostic-model.md),
  [ADR 0010](../adr/0010-shared-resource-limits.md)

## Responsibility

`ical-conform` owns the corpus and the contract, and nothing else. It holds calendars exported
from real clients — reduced, anonymized, and committed verbatim — each addressed to a
specification section rather than to any implementation's types, each carrying what this project
produces, what the RFC also permits, and what Google, Microsoft 365, Apple, `libical` and
`ical.js` were actually observed to do. It defines what an implementation must be able to answer
to be measured, runs a case against one, and classifies the result. It parses nothing itself,
links no implementation it is not told to link, and reaches the outside world at exactly one seam.
Its output is not a green suite: it is a report in which "this subject reproduced its recorded
divergence" and "this subject no longer matches its record" are different sentences.

## The public surface

### Cases, and what makes one acceptable

```rust
pub struct CaseHeader { pub id: CaseId, pub spec: SpecRef, pub provenance: Provenance }
pub struct Provenance {
    pub producer: Producer,
    pub product_version: &'static str,
    pub reduction: Reduction,
    pub anonymization: Anonymization,
}
pub struct Anonymization { /* private */ }
impl Anonymization { pub const fn attested(reviewer: &'static str, date: &'static str) -> Self; }
pub struct Input { /* private: &'static [u8] */ }
```

*Invariants.* `CaseId` is never reused for a different question; changing what a case asks means
renaming it, so the change is visible in a diff. `Anonymization` has private fields and no
`Default`, so a `Provenance` cannot be constructed without naming a reviewer and a date — ADR 0006
makes anonymization a condition of acceptance, and this makes skipping it an act rather than an
omission. It does not prove the review happened; nothing in a type system can. `Input` is
`&'static [u8]` and never `&str`: a case whose entire point is a CP1252 `SUMMARY` or a fold
splitting a UTF-8 codepoint must survive being a case.

Every case is `const`-constructible, which is a constraint this crate imposes on `ical-core`:
`Limits` must remain buildable in a `const` item, or the case tables become lazily-initialized
data and the corpus stops being inspectable as source. As landed, `ical-core` satisfies this —
`Limits::DEFAULT` plus `const` `with_*` builders — and that property is now load-bearing here,
not incidental.

### The four comparison classes, and the wall between them

ADR 0006 closed the set of comparison classes at three, and its amendment 2 spends the loud break
it reserved: the set is four, and the fourth is portable. The portable/non-portable split is still
type-level, so this is two case types and two traits rather than one enum with four arms:

```rust
pub enum PortableQuestion {                                        // NOT #[non_exhaustive]
    RoundTrip,
    Derived(DerivedQuestion),
    Exchange(ExchangeQuestion),
}
pub struct ExchangeQuestion {              // #[non_exhaustive]: a new exchange is routine
    pub actor: Actor,                      // the applying party, CAL-ADDRESS-shaped
    pub continuation: &'static [Input],    // the ordered documents after `PortableCase::input`
    pub kind: ExchangeKind,                // ItipArbitration | ReportResultSet
}
pub struct DiagnosticQuestion {
    pub limits: Limits,
    pub sink: SinkCapacity,
    pub expectation: DiagnosticExpectation,
}
pub struct PortableCase { pub header: CaseHeader, pub input: Input,
                          pub question: PortableQuestion, pub expectation: Expectation }
pub struct NativeCase   { pub header: CaseHeader, pub input: Input,
                          pub question: DiagnosticQuestion }
```

*Invariants.* `PortableQuestion` is deliberately not `#[non_exhaustive]`. A new comparison class
must break every `match` in every downstream crate and force ADR 0006 open, which is the loud
failure that ADR chose over silent misfiling; the two classes that sentence predicted — iTIP
`SEQUENCE` arbitration and a CalDAV `REPORT` result set — turned out to be one class with two
members, and `Exchange` is it. Because this enum exports nothing today, the break costs nobody
anything *now* and costs a major version at the first publish, so the variant lands in the change
that first exports the type or not at all. A `PortableCase::input` still means "the document the
case is addressed to" — for an exchange, the prior state — so no existing case table moves. `DerivedQuestion` *is* `#[non_exhaustive]`, because adding a derivation is
routine where adding a class is not. A `NativeCase` pins the `Limits` it runs under, since "the
budget was exhausted after nine instances" is not reproducible against an unstated policy, and
pins a `SinkCapacity` so the corpus can drive ADR 0009's refusal protocol — `Fixed(0)` and
`Discard` both force `diagnostics_dropped` above zero on a host build that has an allocator and
would otherwise never exercise the no-alloc tier's weaker promise.

### Subjects, and the seam that needs an operating system

```rust
pub trait ConformanceSubject {
    fn identity(&self) -> SubjectIdentity;
    fn round_trip(&mut self, input: Input) -> Result<Answer, SubjectFailure>;
    fn derive(&mut self, input: Input, question: DerivedQuestion) -> Result<Answer, SubjectFailure>;
}

pub trait NativeSubject: ConformanceSubject {
    fn diagnose(&mut self, input: Input, question: DiagnosticQuestion, meter: &mut Meter)
        -> Result<DiagnosticAnswer, SubjectFailure>;
}

pub trait ForeignRunner {
    fn identity(&self) -> SubjectIdentity;
    fn run_once(&mut self, invocation: Invocation<'_>, timeout: Timeout)
        -> Result<Vec<u8>, SubjectFailure>;
}

pub struct BridgeSubject<R> { /* private */ }
impl<R: ForeignRunner> BridgeSubject<R> { pub const fn new(runner: R, timeout: Timeout) -> Self; }
impl<R: ForeignRunner> ConformanceSubject for BridgeSubject<R> { /* ... */ }
```

*Invariants.* `ConformanceSubject` takes no `Limits` and no `Meter`: a foreign implementation has
no comparable notion, and by construction a case that needs a stated policy is a `NativeCase`.
Both traits are object-safe, because a run mixes one in-process subject with several bridged ones
and that is runtime wiring, not a generic parameter. `BridgeSubject` implements
`ConformanceSubject` and cannot implement `NativeSubject` — there is no blanket impl and no way to
write one, since a child process cannot produce a `DiagnosticCode` from our golden list. That is
the whole wall, and it is checked: `run_native(&case, &mut bridge_subject)` fails with `the trait
NativeSubject is not implemented for BridgeSubject<CommandRunner>`. `Timeout` has no `Default`
and rejects a zero duration, so a bridge cannot be built without one; ADR 0006 makes the
wall-clock kill a condition of the mechanism, not a hardening item.

### Answers, and the three tiers of "right"

```rust
pub struct Answer { /* encoding: AnswerEncoding, bytes: Cow<'static, [u8]> */ }
pub struct Expectation {
    pub chosen: Answer,
    pub rationale: &'static str,
    pub permitted: &'static [Answer],
    pub observed: &'static [Observed],
}
impl Expectation { pub fn judge(&self, actual: &Answer, subject: &str) -> Verdict; }

pub enum Verdict {          // #[non_exhaustive]
    Match,
    PermittedDifference { index: usize },
    KnownDivergence { subject: &'static str, version: &'static str },
    RecordStale { subject: &'static str, recorded: ObservedAnswer, actual: Answer },
    Mismatch { expected: Answer, actual: Answer },
    SubjectFailed(SubjectFailure),
}
```

*Invariants.* Comparison is byte equality inside one `AnswerEncoding`; two answers in different
encodings are never equal and never compared. `Answer` holds a `Cow` so a recorded answer lives in
a `const` table while a produced one is owned. The three tiers are the crate's reason to exist:
`chosen` is what this project does and why, `permitted` is what the RFC also allows and is
therefore not a failure, and `observed` is the checked-in behavior matrix. `judge` consults them
in that order, and its two interesting outcomes are `KnownDivergence` — the subject reproduced its
record, which is the suite working — and `RecordStale`, which is news about the other
implementation rather than about ours. `Report::needs_attention` counts `RecordStale`, `Mismatch`
and `SubjectFailed`, and nothing else; a permitted difference is not a defect and must not be
reported as one.

Because `observed` is static data, the crate runs in two modes without a second design. With the
bridge disabled the matrix is the answer and is diffed in review; with it enabled the bridge
refreshes the matrix and `RecordStale` is how staleness surfaces. That is ADR 0006's unadopted
dissent kept reachable rather than argued away.

## What each type serves

Only the question and answer types map onto the RFC; the rest is harness vocabulary, and this
table says so rather than inventing citations for it. A row that names an adopted decision
instead of a section has no RFC to cite. Bare encoding names are `AnswerEncoding` variants.

| Type | Serves |
| --- | --- |
| `PortableQuestion::RoundTrip`, `DocumentBytes` | RFC 5545 §3.1 (folding), §3.4 (object) |
| `DerivedQuestion::Occurrences`, `Window`, `OccurrenceList` | RFC 5545 §3.3.10, §3.8.5.3 |
| `DerivedQuestion::LocalTime`, `ResolvedLocalTime` | RFC 5545 §3.3.5, §3.2.19, §3.6.5 `VTIMEZONE` |
| `Spec`, `SpecRef` | The addressing scheme itself: RFC 5545, 5546, 6047, 4791, 4918, 7986 |
| `DiagnosticQuestion`, `DiagnosticAnswer`, `Channel` | ADR 0009: the two channels |
| `DiagnosticExpectation` | ADR 0009: a code whose meaning is frozen, `diagnostics_dropped` |
| `SinkCapacity` | ADR 0009's `DiagnosticSink` refusal protocol |
| `Limits` and `Meter` on `diagnose` | ADR 0010: policy and ledger threaded, never ambient |
| `Expectation`, `Verdict` | ADR 0006: name the chosen answer, judge against three tiers |
| `Observed`, `ObservedAnswer` | ADR 0006: record every observed behavior |
| `Provenance`, `Producer`, `Reduction`, `Anonymization` | ADR 0006's acceptance conditions |
| `CaseId`, `CaseHeader`, `Input` | ADR 0006: cases addressed to specification sections |
| `PortableCase`, `NativeCase`, `Corpus` | ADR 0006: the corpus as a deliverable |
| `ConformanceSubject`, `NativeSubject` | ADR 0006: one contract, two tiers |
| `SubjectIdentity`, `SubjectKind` | ADR 0006: who answered, and from where |
| `ForeignRunner`, `BridgeSubject` | ADR 0006's subprocess bridge |
| `Timeout`, `Invocation`, `WireQuestion` | ADR 0006's mandatory wall-clock kill |
| `SubjectFailure` | Distinguishes "it refused" from "it never answered" |
| `Report`, `ReportEntry`, `Outcome`, `Tally` | ADR 0006: documentation that exists nowhere else |

## Feature flags

| Flag | Default | Effect |
| --- | --- | --- |
| `std` | on | Links `std`. Required by `bridge` and by reading corpus files. |
| `corpus` | on | Compiles the committed case tables; `committed()` returns them. |
| `subject-icalkit` | on | The `NativeSubject` implementation over the icalkit crates. |
| `bridge` | off | `Command`-backed `ForeignRunner`s with the kill wrapper. Implies `std`. |

Turning `std` off leaves a `no_std` + `alloc` crate that is vocabulary, judgment and reporting
only. Turning `corpus` off replaces the committed tables with two empty slices, which is how a
consumer supplies its own corpus against this vocabulary. `subject-icalkit` is the **only**
feature that makes `ical-conform` depend on the five core crates. `bridge` is off in the default
gate — it is `std::process::Command`-backed `ForeignRunner`s for `libical` and `ical.js`, and
ADR 0006 keeps that job best-effort until the timeout-and-kill wrapper is real.

The consequential one is `subject-icalkit`. A competing implementation depends on `ical-conform`
with `default-features = false, features = ["corpus"]`, implements `ConformanceSubject`, and never
compiles a line of icalkit — which is what "runnable against any implementation" has to mean if it
means anything. ARCHITECTURE.md's crate table lists `ical-conform` as depending on all five; that
stays true of the default build and stops being true of the interesting one, and the table should
say so.

## Usage

All three compile against the skeleton under `-D warnings` with `clippy::pedantic`.

**Declaring a case.** A Google export where a fold falls inside a multi-byte codepoint, with
`libical`'s known refolding recorded rather than treated as a failure:

```rust
const AS_EXPORTED: &[u8] =
    b"BEGIN:VCALENDAR\r\nSUMMARY:Z\xc3\r\n \xbcrich sync\r\nEND:VCALENDAR\r\n";
const LIBICAL_REFOLDED: &[u8] =
    b"BEGIN:VCALENDAR\r\nSUMMARY:Z\xc3\xbcrich sync\r\nEND:VCALENDAR\r\n";

const FOLD_MID_CODEPOINT: PortableCase = PortableCase {
    header: CaseHeader {
        id: CaseId::new("rfc5545-3.1-fold-splits-codepoint-google-01"),
        spec: SpecRef { spec: Spec::Rfc5545, section: "3.1" },
        provenance: Provenance {
            producer: Producer::GoogleCalendar,
            product_version: "web export, 2026-03-02",
            reduction: Reduction::Reduced { note: "one VEVENT kept, ATTENDEE list removed" },
            anonymization: Anonymization::attested("y.sakashita", "2026-03-04"),
        },
    },
    input: Input::new(AS_EXPORTED),
    question: PortableQuestion::RoundTrip,
    expectation: Expectation {
        chosen: Answer::recorded(AnswerEncoding::DocumentBytes, AS_EXPORTED),
        rationale: "ADR 0001: fold position and WSP octet preserved; bytes out are bytes in.",
        permitted: &[],
        observed: &[Observed {
            subject: "libical",
            version: "3.0.18",
            answer: ObservedAnswer::Produced(
                Answer::recorded(AnswerEncoding::DocumentBytes, LIBICAL_REFOLDED),
            ),
        }],
    },
};

const PORTABLE: &[PortableCase] = &[FOLD_MID_CODEPOINT];
const NATIVE: &[NativeCase] = &[OVERSIZED_DESCRIPTION];   // declared in the third example

fn run_one_case(subject: &mut dyn ConformanceSubject) -> Report {
    let corpus = Corpus::new(PORTABLE, NATIVE);
    let mut report = Report::new();
    for case in corpus.addressed_to(Spec::Rfc5545) {
        let identity = subject.identity();
        let verdict = run_portable(case, subject);
        report.record(case.header.id, &identity, Outcome::Portable(verdict));
    }
    report
}
```

**Bridging a foreign implementation.** The `std` dependency is confined to the `spawn` function;
the rest is `no_std`:

```rust
struct CommandRunner {
    identity: SubjectIdentity,
    spawn: fn(Invocation<'_>, Timeout) -> Result<Vec<u8>, SubjectFailure>,
}

impl ForeignRunner for CommandRunner {
    fn identity(&self) -> SubjectIdentity { self.identity.clone() }
    fn run_once(&mut self, invocation: Invocation<'_>, timeout: Timeout)
        -> Result<Vec<u8>, SubjectFailure> { (self.spawn)(invocation, timeout) }
}

fn compare_against_libical(
    spawn: fn(Invocation<'_>, Timeout) -> Result<Vec<u8>, SubjectFailure>,
) -> Option<Report> {
    let timeout = Timeout::new(Duration::from_secs(5))?;
    let runner = CommandRunner {
        identity: SubjectIdentity::new(
            "libical".to_string(), "3.0.18".to_string(), SubjectKind::Foreign,
        ),
        spawn,
    };
    let mut libical = BridgeSubject::new(runner, timeout);
    let report = run_one_case(&mut libical);
    // run_native(&OVERSIZED_DESCRIPTION, &mut libical) does not compile:
    // BridgeSubject<CommandRunner> is not a NativeSubject, and no impl can make it one.
    Some(report)
}

fn triage(report: &Report) -> bool {
    for entry in report.entries() {
        if let Outcome::Portable(Verdict::KnownDivergence { subject, version }) = entry.outcome {
            let _ = (subject, version); // recorded divergence reproduced: the suite working
        }
    }
    report.needs_attention()
}
```

**Asking a question no foreign subject can answer.** A limit breach travels on the diagnostic
channel, under a policy the case pins, into a sink that refuses everything:

```rust
const LONG_LINE: &[u8] =
    b"BEGIN:VCALENDAR\r\nDESCRIPTION:aaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\nEND:VCALENDAR\r\n";

const OVERSIZED_DESCRIPTION: NativeCase = NativeCase {
    header: CaseHeader {
        id: CaseId::new("adr0009-limit-breach-is-a-diagnostic-not-an-error-01"),
        spec: SpecRef { spec: Spec::Rfc5545, section: "3.1" },
        provenance: Provenance {
            producer: Producer::Other("hand-written for this case"),
            product_version: "n/a",
            reduction: Reduction::Verbatim,
            anonymization: Anonymization::attested("y.sakashita", "2026-03-04"),
        },
    },
    input: Input::new(LONG_LINE),
    question: DiagnosticQuestion {
        limits: Limits::DEFAULT.with_max_value_bytes(16),
        sink: SinkCapacity::Fixed(0),
        expectation: DiagnosticExpectation::Codes(&[DiagnosticCode::ValueLengthLimitExceeded]),
    },
};

fn check_diagnostics() -> DiagnosticVerdict {
    let mut icalkit = Icalkit;
    run_native(&OVERSIZED_DESCRIPTION, &mut icalkit)   // requires &mut dyn NativeSubject
}
```

## Deliberately rejected

**One `Case` type with a three-variant `Question` enum.** The obvious reading of ADR 0006, and it
makes `run(case, foreign_subject)` compile for a `Diagnostic` case. The check would then be a
runtime skip, which is the "documentation convention a contributor can violate silently" the
adopted decision names as unacceptable. Two case types and a trait bound cost a duplicated header
field and buy a compile error.

**`#[non_exhaustive]` on `PortableQuestion`.** Rejected for the reason it is normally added: a
new comparison class should break downstream builds. That reservation has now been *taken*, once,
by ADR 0006's amendment 2 — which is the attribute working as intended and is also the reason a
fifth class would have to argue that the set is closed on evidence rather than on assertion.

**Typed comparison instead of canonical bytes.** Comparing parsed values would encode our reading
of the specification into the comparison — the objection ADR 0006 opens with — and would make a
byte-identical round trip unexpressible.

**A `Limits` argument on `ConformanceSubject`.** It cannot mean anything to `libical`. Pushing it
onto `DiagnosticQuestion` instead makes "this policy, this budget answer" a property of the case.

**Reusing one `Meter` across cases.** `run_native` mints a fresh one per case, because case
independence is the point of a corpus. The cost is stated below.

**`std` throughout.** The crate uses `std` and always will; the *vocabulary* does not need it, and
making it `no_std` + `alloc` means an implementation under test is never forced to link `std` to
satisfy the trait. Only `ForeignRunner` implementors need an operating system.

**A static-only matrix, or a live-only bridge.** Neither, on purpose: `Observed` is checked-in
static data and the bridge is what detects that the data went stale. ADR 0006's amendment 1 scores
and adopts exactly this, and fixes the two things this entry left unsaid: the static matrix is the
only foreign evidence a required check reads and every row carries its provenance as data, and the
bridge lives in `xtask` off the required path, proposing diffs a human reviews and never failing a
build. `ForeignRunner` stays published as the seam for out-of-tree subjects, and nothing in-tree
may construct one.

## Consequences

The two-tier split means the corpus is honest about how little of it is portable. Every case that
asserts a diagnostic code, a dropped count or a budget value is a `NativeCase` and can never say
anything about anyone else's implementation — and those cases are the ones this workspace has the
most of reason to write. The cross-language claim covers round trips and derived answers only, and
the `Corpus` type now shows that ratio to anyone who counts the two slices, which is an
improvement in honesty and not in coverage.

`Answer` comparison is byte equality, so the entire risk moves into whoever writes the per-subject
normalization that turns `ICAL.Time` or `icaltimetype` into the canonical encoding. ADR 0006 says
no type detects an unfaithful adapter; nothing here changes that, and this design adds a place to
hide one — `ForeignRunner::run_once` returns already-normalized bytes, so the adapter sits below
the seam where the harness could inspect it. The canonical encodings themselves are named here and
specified nowhere, which is the largest piece of unwritten work this document creates.

A fresh `Meter` per case means the corpus demonstrates the shape ADR 0010 wants and cannot
demonstrate the failure it warns about: 5,000 bounded searches sharing one ledger is the
interesting case, and a corpus of independent cases structurally cannot contain it. That belongs
to a benchmark or a fuzz target, and naming it here does not write it.

`Anonymization`, `Timeout` and the `NativeSubject` bound are three different strengths of
guarantee presented in one API, and only the last is real. The trait bound is checked by the
compiler; the zero-duration rejection is checked at construction; the anonymization attestation is
checked by nobody. A reader who trusts the first is likely to over-trust the third.

Finally, none of this exists. The crate is a documentation module and the iTIP chapter; there is
no `Corpus`, no `committed()`, and every type above is load-bearing for zero cases. What does
exist is a corpus written the other way round — thirty-one files under `tests/`, the forty-two
worked examples of section 3.8.5.3 among them, addressed to specification sections in prose and
run against this workspace's crates directly — so the vocabulary above has contents to be fitted
to rather than none, and `PortableQuestion` is still frozen against a fourth comparison class by
a document rather than by a compiler. That is the ordinary cost of designing a vocabulary ahead
of its contents, and it is worth restating that ADR 0006 itself called the bridge "the decision
but not a settled one" — which its amendment 1 has since settled, on grounds a bake-off could not
have decided: what a red job means, which targets the differential claim can cover, and when the
first useful row exists.

## What the first compile changed

The `sibling` module was deleted and `Limits`, `Meter` and `DiagnosticCode` now come from
`ical-core`, so a case that pins a policy pins the same value the implementation under test
reads. `Instant` comes from there too, having settled one layer lower still, in the grammar.

One thing this document assumes is now provable and not yet proved: `DiagnosticCode` is a single
workspace-wide enum, so "input X produces code Y" is a claim about one stable vocabulary rather
than about whichever crate happened to detect it. The golden list that freezes those meanings is
built: `docs/diagnostic-codes.md` carries a row per code and `xtask codes` refuses a row whose
meaning or channel moved without a rename. What this document requires and does not build is a
case of its own vocabulary hung on that list.

The crate did not stay `#![no_std]`: it links `std`, `just no-std` names the five core crates and
not this one, and the `thumbv7em-none-eabi` claim is theirs rather than the workspace's. The
`no_std` + `alloc` vocabulary this document describes is therefore a shape still to be built, not
a property the crate has — which matters, because a `std`-only feature added here later would
otherwise look like the moment it was lost. The `std` half, the subprocess bridge to a foreign
implementation, is still a feature nobody has written.
