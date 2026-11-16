---
date: 2026-11-16
status: proposed
---

# Diagnostic meanings as literal data

## Context

The [diagnostic decision](0009-error-and-diagnostic-model.md) keeps diagnostic meanings stable.
The registry gate compares the recorded strings with source comments.
Prose checks treat those fixed strings as authored explanations.

## Decision

Keep the existing registry records in one fenced text block.
Preserve the record syntax and literal source sentences.
Compare the literal content with the source comment without rewriting either string.
Keep keys, channels, milestones, and row order unchanged.

## Alternatives

Rewriting meanings changes fixed contract text.
Exempting the document also exempts its explanations.
Quoting each cell adds syntax inside an already literal block.
A separate data file adds another artifact and changes the documentation workflow.

## Consequences

The registry parser must require one closed text block.
Explanatory prose can follow current writing rules without changing contract data.

## Confirmation

Registry tests must cover missing or repeated blocks, missing terminators, changed meanings, and invalid metadata.
The committed registry must pass `just codes` and the document prose check.
