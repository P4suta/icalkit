# ADR-0012: a filter evaluator gets a crate, and the WebDAV grammar gets a boundary instead of one

- Status: accepted
- Date: 2026-08-11

## Context

M4 shipped the CalDAV protocol layer and then wrote down, in its own assessment of whether a
server is now a reasonable thing to attempt, that nothing here evaluates a filter: a
`comp-filter`, a `time-range` and a `text-match` are represented, refused when they contradict
themselves, and handed back, and deciding which resources match is work a server does by
composing them with `ical-recur` and `ical-core`. It is still the largest single piece of a
server that this workspace does not contain.

That is one face of a boundary question. The other is the WebDAV grammar `ical-dav` keeps
private, which [ADR 0004](0004-sans-io-protocol-layer.md) defers extracting "until a second
DAV-shaped consumer exists in this workspace to justify the extraction", and whose Consequences
then record that the deferral costs more than it did, because what is being kept private grew
from a small tag matcher into a namespace-resolving, reference-resolving reader and writer. Both
faces move the same boundary — what this workspace publishes around `ical-dav` — and both are
cheapest to do once.

They must therefore be decided together, and ADR 0004 says where: a graph change of this kind
"should adopt the full graph and justify the product-scope expansion in its own ADR rather than
let it ride in on this one". This is that document.

## Decision

The two faces are decided together and they get different answers.

**A filter evaluator gets a crate.** `ical-query` is published, sitting above `ical-core`,
`ical-recur`, `ical-tz` and `ical-dav`. It takes `ical-dav`'s already-public filter values —
`CompFilter`, `PropFilter`, `ParamFilter`, `TimeRange`, `TextMatch` — a parsed calendar from
`ical-core`, a zone source from `ical-tz` and expansion from `ical-recur`, and answers whether a
resource matches. `ical-dav` gains no dependency, so ADR 0004's spine is not inverted: the new
crate is a *consumer* of the spine rather than a link in it. Every entry point takes `&Limits`
and `&mut Meter` like every other hostile-input entry point
([ADR 0010](0010-shared-resource-limits.md)), it declares `#![no_std]`, and it joins the governed
crate list, `just no-std` and `just wasm`.

The evaluator cannot live anywhere else. Evaluating a `time-range` needs recurrence expansion and
zone resolution; ADR 0004's spine gives `ical-dav` only `ical-core`, so the evaluator cannot live
in `ical-dav` without inverting the graph, and it cannot live in `ical-recur` or `ical-tz`, which
are siblings that do not know what a `comp-filter` is. It is either a new crate above all three,
or work this workspace declines to ship and every server author writes again.

**`webdav-core` is not published, and the deferral stops being a promise.** The expensive half of
the extraction — the untangling — happens in this same restructuring: `ical-dav`'s tokenizer,
namespace stack and writer move into one self-contained module that may not name a CalDAV type,
enforced by a gate, and nothing of it is exported, including through a hidden public re-export. On
the day a second DAV-shaped consumer is accepted, extraction becomes a file move plus a manifest
rather than a redesign. Publishing the name today would buy insurance against a consumer that may
never exist, which is the purchase [ADR 0004](0004-sans-io-protocol-layer.md)'s Amendment 12 is on
the same docket to unwind for `ical-grammar`; a published crate name cannot be withdrawn, an
unexported module can.

Separating the outcomes is deliberate and it is defensible for a specific reason: `ical-query`
consumes filter *values*, not XML, so nothing about the new crate creates an external dependent of
the grammar `webdav-core` would have carried. The ordering bet ADR 0004 and
`docs/design/ical-dav-api.md` both acknowledge — that extracting after external users depend on
`ical-dav`'s internals is worse than extracting before — is honored by not exposing the internals,
which is cheaper than publishing a crate to protect them.

Also rejected, and recorded so it is not rediscovered: putting the evaluator inside `ical-dav`
behind a feature flag. `docs/design/ical-dav-api.md` already refuses `client` and `server` flags
for encoding a direction split into the build system, a feature is unified across a dependency
graph by the union rule so one crate in a tree could change another's behavior, and it would put
`ical-recur` and `ical-tz` under `ical-dav`, inverting the spine ADR 0004 exists to hold.

### What the measurement decides, and what it does not

The measurement below does not decide whether `ical-query` exists. That is decided above, today,
because it is the expensive-to-undo part and the evidence for it is already in the tree. What the
measurement decides is narrower: whether the plain filter walk is the deliverable, or whether the
crate must ship an expansion-free prefilter in front of it.

**The measurement.** Two sweeps over the same 5,000 resources, run as a committed `ical-conform`
case that states its `Limits` policy, because an outcome that depends on a budget is not
reproducible without one. The population is the committed DAV fixtures plus series generated from
`ical-recur`'s own workload table, including rare-match multi-decade rules of the
`FREQ=YEARLY;BYDAY=MO;BYMONTHDAY=1` family ADR 0010 names. The query is a `calendar-query`
carrying a one-month `time-range` beside a `comp-filter`/`prop-filter` pair.

*Sweep A, per resource.* Each resource is evaluated with **its own** meter at the default policy —
budget 16,777,216 octets, that policy's input ceiling, stated explicitly because a meter's octet
budget is separate from `Limits` and naming the policy alone leaves the denominator unstated.
Record per resource: whether its meter is exhausted after evaluation, the refusal variant if any,
octets spent, and candidates charged.

*Sweep B, aggregate.* The identical sweep once more under one shared meter for the whole `REPORT`,
recorded as a boolean plus an index — completed, or cut short at resource N with variant V.

**The source.** This workspace only: committed fixtures plus series generated by this workspace's
own code. No credentials, no network, no external runtime. It is reachable today and waits on
nothing.

**The threshold.** *Clause 1.* If more than 0.1% of the 5,000 resources — six or more, since 0.1%
of 5,000 is five — exhaust **their own** meter in sweep A, the plain filter walk is the wrong
deliverable, and `ical-query` ships a prefilter in front of it: per-resource expansion-free bounds
(`DTSTART`, `DTEND` or `DURATION`, and each `RRULE`'s `UNTIL` or `COUNT` upper bound) evaluated
before any expansion, with the walk run only on resources the prefilter cannot exclude. This is
countable because each per-resource meter latches on its own, and it is sensitive to the real
failure mode because the per-period candidate ceiling latches whether or not the octet budget is
touched, so a rarely-matching multi-decade rule exhausts its own resource's meter and is counted.
*Clause 2.* If five or fewer exhaust, the filter walk is the deliverable, and each exhausting
resource becomes a conformance case stating its `Limits` policy — not a reason to raise a bound.
*Clause 3.* Sweep B reports and does not gate. A shared meter latches globally, so its outcome is
one boolean about a `REPORT` and never a rate about resources; it is handed to
[ADR 0010](0010-shared-resource-limits.md)'s Amendment 1 as calibration evidence on whether 16 MiB
is a plausible per-`REPORT` ceiling for a 5,000-resource multiget, and to the caller-guidance
paragraph `ical-query`'s documentation owes on meter lifetime. Sweep B may not on its own retire
the filter walk.

**If it cannot be obtained.** If the generator cannot produce a 5,000-resource population with
hostile-shaped series without inventing provenance ADR 0006 would refuse to commit, sweep A runs at
the largest N the corpus supports with N no smaller than 1,000, and clause 1's trigger becomes
"more than 0.1% of N, and never fewer than two resources", so a single outlier cannot flip it. If
neither sweep can be run before the evaluator's shape is needed, the default is the filter walk
written so that the prefilter is an internal step it calls — defaulting to "cannot exclude" —
rather than a rewrite. That way the failing branch stays an implementation and the measurement
still decides after the crate ships.

### Why the earlier form of that measurement was withdrawn

It is recorded here rather than quietly replaced, because it was fixed in advance and it could not
have fired. The earlier clause counted resources that exhaust a meter *shared across the whole
`REPORT`*, and meter exhaustion is a single latched global event: a charge returns refusal
immediately once the flag is set, and the per-period ceiling sets it. Under one meter per `REPORT`
the per-resource population is therefore not a distribution but a step — on the strict reading
exactly one resource is the one that flips the latch, forever under 0.1%, and on the loose reading
every resource after the flip fails and the fraction jumps to near 100%. Either way the number was
decoration on a boolean, and the branch that would have retired the filter walk could not fire on
its own terms. A deferral whose discharging branch is unreachable is a postponement wearing a
number. The second defect was independently fatal to reproducibility: naming the policy without
naming the octet budget leaves the denominator unstated, which is precisely what M5 says a case may
not do. The repair is ADR 0010's own prescription — one meter per resource — with the budget
spelled out, and the aggregate run kept beside it as a boolean rather than a rate.

## Consequences

Four, and none of them are small.

`ical-query` is a published name that cannot be unpublished, added by the same wave that is
deciding, in [ADR 0004](0004-sans-io-protocol-layer.md)'s Amendment 12, that a published name
bought as insurance should be collapsed. It must be kept green on `thumbv7em-none-eabi` and
`wasm32-unknown-unknown`, inside the purity gate, on every release. And it makes `ical-dav` a
dependency of a core crate for the first time, which freezes the filter types' shape earlier than
`Href`'s open questions and the remaining vocabulary rows would otherwise have frozen them: a
downstream consumer arrives before that vocabulary is finished, so a filter-shape change is now a
downstream break.

The unexported XML module costs `ical-dav` a gate and a boundary that forbids CalDAV names inside
its own tokenizer, paid on every future XML fix by a crate that has exactly one consumer and may
never get a second. If none ever arrives, that friction bought nothing.

The deferral bet is bounded rather than eliminated. If a second DAV-shaped consumer arrives after
`ical-dav` reaches 1.0, moving the module out is still a semver event for its callers even though
the code moves cleanly; keeping the grammar private makes the move mechanical, not free.

The repaired measurement is more expensive to run than the one it replaces — 5,000 independent
meters instead of one, two sweeps instead of one, and a per-resource record of octets spent and
candidates charged. It also gives up, as a gate, the thing the shared-meter sweep was really
testing: whether the default policy is a plausible ceiling for a whole `REPORT`. That survives only
as reported evidence.

Three gate surfaces move, and the sequencing matters because two other decisions in this wave
rewrite the same gate. The governed crate list gains `ical-query`, which the purity gate requires
anyway once the crate declares `#![no_std]`, and the two cross-target recipes gain it as a build
target. A new gate covers `ical-dav`'s XML module: it may not name a CalDAV type and nothing in it
may be public outside the crate, which is what keeps the deferred extraction a file move. And the
sweep lands as a committed conformance case stating its policy and its octet budget, so clause 1's
count is reproducible rather than a number somebody once observed. `xtask purity` is rewritten
three times in one landing — here, by ADR 0004's Amendment 12, and by its Amendment 16 — so the
custodian edit lands last or lands merged with the other two; in any other order a leg is written
against a list that no longer exists.

## Alternatives

**Both, in one restructuring: publish `webdav-core` now.** This is ADR 0004's own top-scored panel
reading — extract now, before external users depend on `ical-dav`'s internals — and
`docs/design/ical-dav-api.md` calls the opposite ordering "the acknowledged bet". It was rejected
because the harm it guards against is caused by *exporting* the grammar rather than by leaving it
in place, and not exporting it costs a gate instead of a crate name; and because publishing a crate
for a consumer that does not exist is the exact insurance purchase ADR 0004's Amendment 12 is on
this same docket to unwind. It should be revisited without further argument the day CardDAV or
WebDAV-sync moves from undecided scope into the roadmap proper.

**The evaluator inside `ical-dav` behind a feature flag.** Rejected above, and recorded here so it
is not rediscovered.
