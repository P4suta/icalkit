---
date: 2026-11-17
status: proposed
---

# Candidate worktree validation

## Context

Candidate qualification must detect changed or missing tracked content.
Git's [index flags and cached metadata](https://git-scm.com/docs/git-update-index) can hide changes from the current comparison.
The caller's index and repository environment must remain intact.

## Decision

Build a private index from the observed commit with [`read-tree`](https://git-scm.com/docs/git-read-tree), without updating working files.
Compare working files through that index with monitoring and sparse checkout turned off.
Compare the original staged tree with the same commit.
Check the revision again before returning success.
Own and remove scratch storage on success and failure.

## Alternatives

Rejecting index flags also rejects clean working files.
Refreshing the original index changes caller state.
Comparing raw blobs would change Git's content normalization rules.

## Consequences

Qualification reads tracked content instead of trusting cached promises.
Git attributes keep their comparison semantics, and incomplete working trees fail.
The check needs temporary storage and a full read of tracked content.
Revisit if qualification supports a different checkout or attribute contract.

## Confirmation

Fixtures cover hidden changes, missing files, staged content, names, environment isolation, index preservation, and scratch cleanup.
Maintained gates must pass.
