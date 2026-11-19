---
date: 2026-11-17
status: proposed
---

# Require advertised recurrence scales before CalDAV writes

## Context

[RFC 7529 section 10](https://www.rfc-editor.org/rfc/rfc7529.html#section-10) forbids `RSCALE` writes without a server advertisement.
A missing property and an advertised set without names have different meanings.
A path-only probe carries no origin.

## Decision

Probe the caller's calendar collection or home with PROPFIND before writing `RSCALE` rules.
Require one [WebDAV href](https://www.rfc-editor.org/rfc/rfc4918.html#section-8.3) matching the probe's path and query.
An absolute-path href inherits the request origin.
An absolute href requires a probe with the same [HTTP origin](https://www.rfc-editor.org/rfc/rfc9110.html#section-4.3.1).
Accept CalDAV property values with `200 OK` as policy.
An empty advertised set permits an attempt with any calendar name.
Preserve listed names and compare them without case sensitivity, including supported Gregorian aliases.
Keep conditional headers and calendar bytes through the probe.
Advertise server support only through explicit configuration and property selection.
Return the named `403` refusal for unsupported calendars after authorization.

## Alternatives

An ordinary `allprop` request need not return this protected property.
Guessing the object's parent misses advertisements on calendar homes.

## Consequences

Refusal stops the write before storage.

## Confirmation

Require namespace, status, empty set, Gregorian spelling, malformed input, budget, and conditional write regressions.
Reconsider when a transport adapter supplies collection identity or a capability cache.
