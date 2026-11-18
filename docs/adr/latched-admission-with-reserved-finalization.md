---
date: 2026-11-18
status: proposed
---

# Latch admission failures and reserve finalization

## Context

The [shared limits decision](0010-shared-resource-limits.md) requires every resource refusal to latch exhaustion.
Counters and local protocol checks otherwise admit later work.
Zone insertion charges after observance truncation, so latching alone discards admitted partial tables.

## Decision

Check shared exhaustion before every admission.
Route protocol width and collection bounds through the meter, preserving the first failure and returning an exhausted error on retries.
Keep cleanup and diagnostic accounting available.
Reserve each zone slot before reading observances and consume its private reservation on insertion without another charge.
XML failure responses need an available output budget.

## Alternatives

Independent counters contradict the shared contract.
Guarding only session entry points misses direct construction and streaming.
Discarding partial tables loses admitted data.

## Consequences

Later admissions stop across resource kinds.
Reconsider when another adapter needs reserved admission.

## Confirmation

Test refusal and admission pairs, zero charges, cleanup, preserved errors, partial tables, public sessions, and failure responses.
Run the shared ADR checker and maintained checks.
