# Support

icalkit is intentionally unreleased at `0.0.0`. Maintainers welcome reproducible feedback, but
there is no compatibility or response-time guarantee for general support before the first
release.

## Where to ask

- Use [GitHub Discussions](https://github.com/P4suta/icalkit/discussions) for usage questions,
  API exploration, and help reducing a reproducer.
- Open a [public GitHub issue](https://github.com/P4suta/icalkit/issues/new/choose) for a
  reproducible defect, an interoperability disagreement, a documentation problem, or a concrete
  design proposal. Choose the form that matches the report.
- Report a vulnerability through GitHub's private
  [security advisory flow](https://github.com/P4suta/icalkit/security/advisories/new). Do not
  open a public issue for a panic, hang, unbounded allocation, authorization bypass, or other
  security-sensitive behavior.

Search existing Discussions and issues first. Include the smallest program or calendar that
reproduces the behavior, the icalkit revision, the Rust version, the target, and the command you
ran.

## Calendar and account data

Do not include confidential calendar data, credentials, access tokens, cookies, private URLs,
or account identifiers in a Discussion or public issue. A real-client capture must be reduced
to the smallest case that preserves the behavior, then anonymized before it is shared.

The project can record an observed client behavior without receiving access to the account that
produced it. Google Calendar, Microsoft 365, and Apple Calendar evidence is useful when
available, but no contributor is required to create an account merely to open a report.

## What maintainers can support

The library owns iCalendar parsing and editing, recurrence, time-zone resolution boundaries,
iTIP semantics, and sans-I/O CalDAV state machines. Applications own HTTP execution, storage,
credentials, current-time input, and ACL decisions. Support can explain the boundary and inspect
a reduced wire exchange; it cannot operate or debug a private calendar service on a user's
behalf.
