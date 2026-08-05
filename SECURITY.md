# Security Policy

## Scope

Calendar data is untrusted input that arrives by design: an `.ics` attachment in an email
from a stranger, a `REPORT` response from a server the user does not control, an iTIP
message inviting the user to a meeting. Callers embed this in mail clients and servers.

In scope:

- A panic, hang, or unbounded allocation on any input. A recurrence rule that exhausts the
  candidate budget must report that, not spin
  ([ADR 0002](docs/adr/0002-bounded-lazy-recurrence.md)); a bypass of an injected limit is
  a vulnerability
- **Scheduling authorization failures.** An attendee changing an event's time through a
  `REPLY`, a `REPLY` from an address not on the attendee list being accepted, or a stale
  `SEQUENCE` overwriting a newer version. These are the positions where calendaring
  implementations have historically been exploited
  ([ADR 0005](docs/adr/0005-scheduling-apart-from-the-model.md))
- XML external entity or entity expansion handling in the CalDAV layer
- Data loss on round trip that a caller could not detect — silently dropping a property is
  a correctness bug, but silently dropping an `ATTENDEE` is closer to a security one

Out of scope:

- Accepting a calendar that violates RFC 5545. Real calendars do, constantly, and rejecting
  them is not a security posture ([ADR 0001](docs/adr/0001-lossless-round-trip.md))
- A time zone answer you disagree with when the embedded `VTIMEZONE` and IANA differ. Both
  are reported; choosing between them is the caller's
  ([ADR 0003](docs/adr/0003-caller-supplied-time-zones.md))

## Reporting

Report privately through GitHub's ["Report a vulnerability"][advisories] flow rather than a
public issue. Include the calendar or message that reproduces, reduced and with personal
data removed.

Expect an acknowledgement within seven days.

## Supported versions

While the project is pre-1.0, only the latest release receives fixes.

[advisories]: https://github.com/P4suta/icalkit/security/advisories/new
