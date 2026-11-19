---
date: 2026-11-17
status: proposed
---

# Outbox busy time response data

## Context

[RFC 6638 section 5](https://www.rfc-editor.org/rfc/rfc6638.html#section-5) includes calendar data for successful recipients.
The current response drops the data and rejects HTTP 204.
[RFC 5546 section 3.3.3](https://www.rfc-editor.org/rfc/rfc5546.html#section-3.3.3) binds replies to the request.

## Decision

`ScheduleDelivery::new` requires a reply `Message` for success.
Data must be one readable `VFREEBUSY` reply for the recipient.
Client and server check complete outcomes, decoded identifiers, and byte-identical participant addresses.
Expose both a validated message and the original calendar octets.
[RFC 4791 section 9.6](https://www.rfc-editor.org/rfc/rfc4791.html#section-9.6) permits LF lines.
Validate normalized CRLF while keeping original bytes.
Parse XML and messages under one shared budget.
Represent HTTP 204 explicitly without interpreting absent data as free time.

## Alternatives

Raw bytes preserve data but transfer validation to callers.
Status-only outcomes can't convey busy time.

## Consequences

Update callers and API snapshots before publication.
Replies need bounded heap storage.
Revisit for equivalent participant addresses or other calendar representations.

## Confirmation

Tests cover data fidelity, correlation, incomplete outcomes, malformed XML, shared budgets, and HTTP 204.
Maintained checks must pass.
