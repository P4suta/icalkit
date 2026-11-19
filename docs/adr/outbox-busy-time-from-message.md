---
date: 2026-11-16
status: proposed
---

# Outbox busy time from the message

## Context

[RFC 6638 section 5](https://www.rfc-editor.org/rfc/rfc6638.html#section-5) binds Outbox POST to one free-busy request under [RFC 5546 section 3.3.2](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.3.2).
The current flow accepts arbitrary iTIP messages and uses duplicate routing headers.

## Decision

`Client::schedule` takes an Outbox URI and a validated message.
Require `METHOD:REQUEST` with one `VFREEBUSY` and derive organizer and attendees from its body.
Ignore custom routing headers.
Authorization needs carry the message and derived addresses.
Principal mapping, collection identity, privileges, and delivery remain caller decisions.
Apply the matching media `method` rule from [RFC 5545 section 8.1](https://www.rfc-editor.org/rfc/rfc5545.html#section-8.1) despite its omission in the RFC 6638 example.
Response addresses use the shared URI codec.

## Alternatives

Matching legacy headers to the body preserves compatibility but keeps redundant sources.
Generic iTIP POST needs a separate transport contract.

## Consequences

Update callers and both API snapshots before publication.
Authorization copies the bounded message.
Revisit when implementing a separate scheduling transport.

## Confirmation

Tests cover header-independent requests, forged metadata, invalid kinds, authorization context, and addresses.
Maintained checks and API snapshots must pass.
