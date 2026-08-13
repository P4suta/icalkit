# icalkit

A pure-Rust calendaring stack: iCalendar (RFC 5545), recurrence, time zones, iTIP
scheduling (RFC 5546), and CalDAV (RFC 4791).

> **Status: implemented in stages and intentionally unreleased.** The production API is being
> consolidated behind the `icalkit` facade and remains at `0.0.0`. Nothing is published and
> no release will happen without a separate explicit decision. The caller still supplies HTTP,
> persistence, current time, credentials, and application ACL policy.

## Why this exists

Calendaring is one of the most widely used protocols on the internet and one of the least
pleasant to implement, so almost everybody delegates it to
[libical](https://libical.github.io/) — a C library that is maintained, but is the only
serious implementation there is.

In Rust the situation is worse than "one implementation": there is no stack at all. The
pieces exist and do not fit together — `icalendar` is a builder whose author asks for help
making it mature, `rrule` is a separate crate that has not moved since April 2025 and
carries a warning about untrusted input, and **pure-Rust CalDAV does not exist** (the
`xandikos` crate is a PyO3 wrapper around a Python server, last published in 2023).

The practical consequence was that writing a calendar application in Rust was not a
reasonable thing to attempt — which is the gap this workspace is closing, and the sentence it
intends to make false rather than the one it repeats. Half of it is false already: a client is
reasonable today, and a server is the part still being worked through.

## What makes this hard, and therefore worth doing carefully

- **Recurrence.** `RRULE` describes an infinite series. Expanding one over a window while
  honoring `EXDATE`, `RDATE`, overridden instances, and a `UNTIL` in a different time zone
  than the `DTSTART` is where every calendar bug lives.
- **Time zones.** A calendar carries its own `VTIMEZONE` definitions, which may disagree
  with what the IANA database says today because the rules changed after the file was
  written. Both answers are defensible; picking silently is not.
- **Round-tripping.** Every real calendar contains vendor properties this library has never
  heard of. Dropping them on rewrite is how a client silently destroys another client's
  data, and it is the single most common interoperability failure in this space.

## Public API

`icalkit` is the one production crate. Its normal path is deliberately short:

```rust
let calendar = icalkit::Calendar::parse(bytes)?;
for event in calendar.events() {
    println!("{}", event.uid());
}
# Ok::<(), icalkit::Error>(())
```

Strict parsing never performs compatibility repair. Applications that need recovery make it
visible and receive a change report:

```rust
use icalkit::interop::{Import, RfcRepairV1};

let imported = Import::read(bytes)?;
let normalized = imported.normalize(RfcRepairV1)?;
let calendar = normalized.output().validate()?;
# Ok::<(), icalkit::Error>(())
```

The former `ical-query` evaluator and the iTIP kernel have moved behind `icalkit` private
modules. The remaining `ical-core`, `ical-recur`, `ical-tz`, and `ical-dav` packages are
migration scaffolding while their implementations follow them. A temporary `ical-itip`
compatibility harness compiles the moved source for legacy conformance tests, but `icalkit`
cannot depend on it. New consumer tests use only `icalkit`.
The conformance corpus remains a private executable artifact rather than a second library API. See
[ADR 0013](docs/adr/0013-unified-public-crate-and-explicit-interop.md) and
[ARCHITECTURE.md](ARCHITECTURE.md).

## Development

```sh
mise install        # toolchain and gate tooling
mise run hooks      # install the git hooks
just                # list the available commands
just check          # fast inner-loop gates
just ci             # every gate CI runs
```

## License

Dual-licensed under [MIT](LICENSES/MIT.txt) or [Apache-2.0](LICENSES/Apache-2.0.txt), at
your option. The repository is [REUSE](https://reuse.software/)-compliant.
