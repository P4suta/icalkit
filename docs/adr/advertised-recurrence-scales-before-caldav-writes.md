---
date: 2026-11-17
status: proposed
---

# Require advertised recurrence scales before CalDAV writes

## Context

[RFC 7529 section 10](https://www.rfc-editor.org/rfc/rfc7529.html#section-10) forbids `RSCALE` writes without a server advertisement.
A missing property and an advertised set without names have different meanings.

## Decision

Require an explicit PROPFIND probe before writing interpreted `RSCALE` rules.
The caller selects the associated calendar collection or calendar home URI.
The transport supplies each response to its active request.
Accept CalDAV property values with `200 OK` as policy.
An empty advertised set permits an attempt with any calendar name.
Preserve listed names and compare them without case sensitivity, including supported Gregorian aliases.
Keep conditional headers and calendar bytes through the probe.
Advertise server support only through explicit configuration and property selection.
Return the named `403` refusal for unsupported calendars after authorization.

## Alternatives

Unchecked writes violate the protocol rule.
An ordinary `allprop` request need not return this protected property.
Guessing the object's parent misses advertisements on calendar homes.

## Consequences

The write path adds a request and stops before storage on refusal.

## Confirmation

Require namespace, status, empty set, Gregorian spelling, malformed input, budget, and conditional write regressions.
Reconsider when a transport adapter supplies collection identity or a capability cache.
