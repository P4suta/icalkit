---
date: 2026-11-18
status: proposed
---

# Latch admission failures and reserve finalization

## Context

The [shared limits decision](0010-shared-resource-limits.md) requires every refusal to latch exhaustion.
Independent counters still accept later charges.
Zone insertion charges after observance truncation, so enforcing the latch alone would discard admitted partial tables.

## Decision

Check shared exhaustion before every admission and latch every resource refusal.
Keep the first failure specific and distinguish retries with an exhausted error.
Keep cleanup and diagnostic accounting available after refusal.
Reserve a zone component before reading its observances.
Consume a private, noncopyable reservation when inserting its completed table, without another charge.
Distinguish prior exhaustion from a local dimension limit.
XML failure responses still charge output.
Finish before refusal or explicitly supply an independent output budget.

## Alternatives

Independent counters contradict the shared contract.
Discarding partial tables loses admitted data.
Deferring exhaustion permits more work after refusal.

## Consequences

Later admissions stop across resource kinds.
Reservations count before finalization.
Insertion consumes each reservation once.
Reconsider when another adapter needs reserved admission.

## Confirmation

Test all refusal and admission pairs, including zero-sized charges.
Verify cleanup, named first failures, partial tables, recurrence lists, public sessions, and failure responses.
Run the shared ADR checker and maintained checks.
