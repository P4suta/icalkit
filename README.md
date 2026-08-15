# icalkit

[![CI](https://github.com/P4suta/icalkit/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/P4suta/icalkit/actions/workflows/ci.yml)
[![CodeQL](https://github.com/P4suta/icalkit/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/P4suta/icalkit/actions/workflows/codeql.yml)
![MSRV 1.85](https://img.shields.io/badge/MSRV-1.85-blue)
[![MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

A pure-Rust, sans-I/O calendaring stack for iCalendar (RFC 5545), recurrence, time
zones, iTIP scheduling (RFC 5546/6047), and CalDAV (RFC 4791/6578/6638).

> **Status: production-shaped and intentionally unreleased.** `icalkit` is the sole
> production crate and remains at `0.0.0`. Nothing is published, and no release will happen
> without a separate explicit decision. Applications still own HTTP execution, persistence,
> credentials, current-time input, and ACL decisions.

The normal data flow is explicit:

```text
bytes -> Import (lossless) -> versioned normalization -> Calendar (validated)
      -> recurrence / scheduling / CalDAV workflows
```

Unknown standards-compliant extensions survive. Compatibility repairs never run implicitly.
Every potentially unbounded workflow takes a resource policy, an aggregate session budget, or
a mandatory time window.

## Strict parsing

`Calendar::parse` is the secure-default shorthand. A standards error prevents promotion to
`Calendar`; notes remain attached to a successfully validated value.

```rust
let bytes = b"BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//example//EN\r\n\
BEGIN:VEVENT\r\n\
UID:planning@example.test\r\n\
DTSTAMP:20260814T000000Z\r\n\
SUMMARY:Planning\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

let calendar = icalkit::Calendar::parse(bytes)?;
assert_eq!(calendar.events().next().unwrap().uid(), "planning@example.test");
# Ok::<(), icalkit::Error>(())
```

The complete minimal consumer is runnable with `cargo run -p icalkit --example golden_path`. It
compiles as an external crate using only the public facade, with and without the default features.

Use `Engine::session()` when several operations must share one aggregate resource budget or a
caller-supplied time-zone database.

## Explicit normalization

`Import` keeps every admitted input octet. A sealed, versioned profile returns a separate
output and a report; the original import never changes.

```rust
use icalkit::interop::{Import, RfcRepairV1};

let input = b"BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//example//EN\nEND:VCALENDAR\n";
let imported = Import::read(input)?;
let normalization = imported.normalize(RfcRepairV1)?;

assert_eq!(imported.as_bytes(), input);
assert!(!normalization.changes().is_empty());
let calendar = normalization.output().validate()?;
# Ok::<(), icalkit::Error>(())
```

`CommonClientsV1` is deliberately evidence-driven. Synthetic client-shaped fixtures do not
authorize a repair; a rule lands only with a reduced, anonymized, versioned real-client capture.

## Transactional editing

`Calendar::edit()` works on a private copy. Dropping the editor rolls back. `commit()` validates
the complete result before atomically replacing the calendar, while untouched content lines
retain their original octets.

```rust
# let bytes = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//example//EN\r\nBEGIN:VEVENT\r\nUID:planning@example.test\r\nDTSTAMP:20260814T000000Z\r\nSUMMARY:Planning\r\nX-COLOR:plum\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
let mut calendar = icalkit::Calendar::parse(bytes)?;
let mut edit = calendar.edit();
edit.set_summary("planning@example.test", "Revised planning")?;
edit.commit()?;
assert!(calendar.to_bytes().windows(12).any(|part| part == b"X-COLOR:plum"));
# Ok::<(), icalkit::Error>(())
```

## DST-aware recurrence

Jiff's `Date`, `DateTime`, `Time`, `Weekday`, `Timestamp`, and `SignedDuration` are the public
time vocabulary. `IcalDateTime` adds the iCalendar-only DATE/floating/UTC/zoned distinctions
and leap-second evidence. Calendar-aware query and scheduling workflows resolve each local
wall time through the engine's `ZoneDatabase`, so gaps, folds, provenance, and coverage remain
explicit.

The standalone recurrence stream is lazy, requires a half-open window, and reports budget
exhaustion as an error rather than pretending the stream ended:

```rust
use icalkit::recurrence::{Rule, Window};
use icalkit::time::Timestamp;

let start = Timestamp::constant(1_704_067_200, 0);
let window = Window::new(start, Timestamp::constant(1_704_672_000, 0)).unwrap();
let engine = icalkit::Engine::default();
let mut session = engine.session();
let rule = Rule::parse("FREQ=DAILY;COUNT=3")?;
let mut occurrences = rule.occurrences(&mut session, start, window)?;
while let Some(occurrence) = occurrences.try_next()? {
    assert!(window.contains(occurrence.start()));
}
# Ok::<(), icalkit::Error>(())
```

For a validated calendar, `Calendar::occurrences` selects a master by `UID` and composes its
`RRULE`, `RDATE`, `EXDATE`, and detached overrides. It uses the session's zone database and
budget, returns effective-start order, and resumes through the same opaque `Cursor` type.

## iTIP scheduling

Inbound messages pass through a borrowed read-review-authorize-apply flow. Authorization is
bound to the exact message and held state that were reviewed, and applying it consumes the
capability. Outbound builders cover every iTIP method and require the caller to supply
`DTSTAMP` rather than reading a clock. A first `PUBLISH` or `REQUEST` can create state; all
payload components are authorized before any master or detached instance is inserted, and a
multi-component update is likewise applied as one capability.

```rust
use icalkit::scheduling::Message;
use icalkit::time::Timestamp;

# let payload = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//example//EN\r\nBEGIN:VEVENT\r\nUID:planning@example.test\r\nDTSTART:20260815T090000Z\r\nSUMMARY:Planning\r\nORGANIZER:mailto:alice@example.test\r\nATTENDEE:mailto:bob@example.test\r\nSEQUENCE:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
let message = Message::request(payload, Timestamp::constant(1_786_656_000, 0))?;
assert_eq!(message.method(), "REQUEST");
let content_type = message.imip_content_type();
let received = Message::read_imip(content_type.as_bytes(), &message.to_bytes())?;
assert_eq!(received, message);
# Ok::<(), icalkit::Error>(())
```

For email transport, the mail implementation first removes Content-Transfer-Encoding and passes
the unfolded `Content-Type` value plus decoded part octets to `Message::read_imip`. That validates
the media type, charset declaration, and envelope/body method agreement. It does not trust email
`From` as a calendar identity; pass only the separately authenticated envelope sender as `Actor`
when reviewing the message.

## CalDAV sync and server workflows

CalDAV is a state machine, not an HTTP client. A client `Operation<T>` yields an owned
`WireRequest`, accepts a `WireResponse`, and finishes as a typed result. A `ServerOperation`
asks the host for ACL/storage/routing answers and produces the wire response. Partial query
results are `ProjectedCalendar` values and therefore cannot be passed to persistence APIs as a
complete `Calendar`.

```rust
use icalkit::caldav::{Client, SyncToken};

let token = SyncToken::new("data:,sync-1").unwrap();
let operation = Client::new().sync("/calendars/alice/work/", Some(&token))?;
let request = operation.next_request().unwrap();
assert_eq!(request.method(), "REPORT");
assert_eq!(request.uri(), "/calendars/alice/work/");
# Ok::<(), icalkit::Error>(())
```

The application sends that request with any async or synchronous HTTP stack it chooses, then
feeds the response back with `accept`. Server code follows the mirror sequence:
`handle -> next_need -> supply -> finish`. Discovery, incremental sync, conditional writes,
MKCALENDAR, calendar-query, and scheduling outbox POST use the same wire vocabulary.

## Architecture

Production code lives in one crate with private layers:

```text
kernel -> wire (iCalendar/XML) -> model -> recurrence/timezone
       -> scheduling/query -> CalDAV workflows -> interop/adapters
```

The kernel is `no_std + alloc`, forbids unsafe Rust, and never touches a network, store, clock,
or application policy. The only Cargo features are `std` and `system-tz`, both enabled by
default. `xmlparser` is a private lexer; the wrapper owns namespaces, structure, budgets, and
`calendar-data` octet preservation.

See [ARCHITECTURE.md](ARCHITECTURE.md) and
[ADR 0014](docs/adr/0014-private-kernel-and-conformance-isolation.md).

## Development

```sh
mise install        # toolchain and gate tooling
mise run hooks      # install the git hooks
just                # list commands
just check          # deterministic inner-loop gates
just test           # process-isolated suite plus doctests
just ci             # every practical local CI gate
```

The private `icalkit-conformance` subject speaks a versioned JSONL protocol. Its synthetic
fixtures exercise robustness but are not evidence for `CommonClientsV1` repairs.

## Project governance

icalkit is developed in public, but it is still intentionally unreleased. Start with
[Contributing](CONTRIBUTING.md) for the TDD workflow and review rules, [Support](SUPPORT.md)
for choosing between Discussions and an issue, and the [Security policy](SECURITY.md) for
private vulnerability reports. The checked [repository policy](docs/repository-policy.md)
records the protected-branch, merge, automation, and release posture mirrored in GitHub.

Questions belong in [GitHub Discussions](https://github.com/P4suta/icalkit/discussions).
Actionable defects and proposals use the structured issue forms. Real-client captures must be
reduced and anonymized before they enter an issue or the conformance corpus.

## License

Dual-licensed under [MIT](LICENSES/MIT.txt) or [Apache-2.0](LICENSES/Apache-2.0.txt), at
your option. The repository is [REUSE](https://reuse.software/)-compliant.
