# icalkit

A pure-Rust calendaring stack: iCalendar (RFC 5545), recurrence, time zones, iTIP
scheduling (RFC 5546), and CalDAV (RFC 4791).

> **Status: the stack is implemented and unreleased.** Round trip, recurrence, time zones,
> scheduling and CalDAV are landed and tested together; nothing has been published to
> crates.io and no public API is stable yet. Writing a **client** on top of this is a
> reasonable thing to attempt today — you bring the HTTP client, which is deliberate. Writing
> a **server** still needs the filter engine and the ACL vocabulary this workspace does not
> contain; [ROADMAP.md](ROADMAP.md) says exactly what is missing rather than leaving it to be
> discovered.

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

## Crates

| Crate | Responsibility |
| --- | --- |
| `ical-grammar` | RFC 5545 content lines and the diagnostic vocabulary, no model |
| `ical-core` | RFC 5545 model, parser, serializer — lossless by construction |
| `ical-recur` | `RRULE` / `RDATE` / `EXDATE` expansion, bounded and lazy |
| `ical-tz` | `VTIMEZONE` against a caller-supplied time zone source |
| `ical-itip` | RFC 5546 scheduling semantics |
| `ical-dav` | RFC 4791 CalDAV, sans-I/O — no HTTP client bundled |
| `ical-conform` | Conformance and interoperability suite |

Two changes to that table are decided and have not landed: `ical-grammar` collapses into
`ical-core` as a private module before the first publish, so six crates are published rather than
seven; and `ical-query` joins as the CalDAV filter evaluator above `ical-core`, `ical-recur`,
`ical-tz` and `ical-dav`. See [ADR 0004](docs/adr/0004-sans-io-protocol-layer.md) and
[ADR 0012](docs/adr/0012-query-evaluation-crate-and-the-deferred-webdav-extraction.md).

All but `ical-conform` are `no_std`, perform no I/O, read no clock, and bundle no time zone
database; the corpus runner is `std` and reads calendars off a disk, because a harness that
cannot open a file is not a harness — see [ARCHITECTURE.md](ARCHITECTURE.md) and the
[decision records](docs/adr/).

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
