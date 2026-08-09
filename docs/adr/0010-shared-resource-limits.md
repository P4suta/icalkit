# ADR-0010: one limits policy, one running meter, threaded through every hostile input

- Status: accepted
- Date: 2026-08-10

## Context

[ADR 0002](0002-bounded-lazy-recurrence.md) says a recurrence search draws on "the injected
limits". The definite article points nowhere: no type is named, no crate owns one, and the
promise is kept in prose in two crates of five. `ical-core` speaks of "the caller's limits",
`ical-recur` ships a candidate budget of its own invention, and `ical-tz` and `ical-dav` never
mention a limit at all — although a CalDAV multistatus is the second most obvious hostile input
here, after an `.ics` file.

An ambient global or thread-local limit loses at once: invisible in a signature, unavailable to a
crate that owns no thread ([ADR 0004](0004-sans-io-protocol-layer.md)), and dishonest in a
library the caller drives. A limit type per crate loses too, being four spellings of one concept.

The third loser matters most, because it was the obvious answer. A threshold checked per call is
bounded per call and unbounded in aggregate. A CalDAV multiget carrying 5,000 VEVENTs, each with
a rare-match rule such as `FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` over a multi-decade window, is
processed by a fan-out loop living in the calendar application — above `ical-dav`, above
`ical-recur`, in code this workspace does not ship. Every search respects its own threshold and
reports a clean result; the total is whatever the attacker chose N to be.

## Decision

A limit is two things, and this ADR names both. `Limits` is the caller's immutable policy: the
thresholds, cheap to copy, identical for every call, owned by `ical-core`. `Meter` is the
caller's mutable ledger: a running count of work already done under that policy. Every
hostile-input entry point takes both, `&Limits` and `&mut Meter` — parsing in `ical-core`,
expansion in `ical-recur`, zone-transition resolution in `ical-tz`, and the `REPORT` and
multistatus readers in `ical-dav`, inbound direction included.

The meter is passed by mutable reference precisely so that its lifetime is the caller's choice
and not the call's: one meter handed to all 5,000 searches makes 5,000 individually bounded calls
bounded in aggregate. `Meter` is neither `Copy` nor `Default`, so minting a fresh one inside a
fan-out loop is a visible act rather than an omission. Splitting a budget across workers is
likewise the caller's job; no shared meter ships here, because an atomic counter is not something
a `no_std` crate may assume.

Fields count work where the work happens. Recurrence spends candidates *generated* per period,
not instances emitted, since a fine `BYxxx` combination under a negative `BYSETPOS` does the work
either way. `max_line_len` is a byte cutoff that rejects rather than truncates and never splits a
folded line mid-codepoint. Depth bounds component and XML nesting; element and href counts bound
a wide flat document no depth counter sees; a byte count applies to the request a server reads,
not only the response it writes. Exhaustion is reported as itself: "cut short at the limit" and
"the rule ended at `UNTIL`" must be different answers, or truncation arrives dressed as an empty
result.

## Consequences

Four crates take a breaking signature change, not the three first counted: `ical-recur`'s
existing budget folds into this type rather than sitting beside it, or "one value everywhere" is
false on the day it is written. And the signature gap is the only one that closes — accepting
`&mut Meter` and never debiting it still compiles. A gate can test the debit sites someone
thought to write a test for, which is not a proof of coverage, and nothing in the type system
charges the meter at the recursion site.

The library also cannot force meter reuse. `!Copy` plus `!Default` plus documentation makes the
amplification bug visible, not impossible; a caller who writes `Meter::with_budget(D)` inside its
own fan-out loop reproduces the attack above exactly, and no gate here sees that caller's code.
The corpus ([ADR 0006](0006-conformance-corpus-as-artifact.md)) can demonstrate the right shape
and record the wrong one, not enforce either downstream — the price of the sans-I/O boundary and
of `ical-dav` not depending on `ical-recur`.

The numbers are not chosen: a budget right for a phone rendering one month is wrong for a server
indexing a decade, and calibration belongs to whoever ships the first recurrence milestone.
Relatedly, a 64-worker server has 64 times the aggregate ceiling, and nothing here answers a
server that wants one global bound.

The DAV field list is not proven complete. Bytes, element count, and depth do not bound entity
expansion, attribute count per element, or namespace declarations, so a small inbound body can
still expand into unbounded work through a dimension none of these fields counts. Whether that
needs a typed `XmlLimits` sibling is deferred, not settled, and it is the likeliest place the
next review lands.
