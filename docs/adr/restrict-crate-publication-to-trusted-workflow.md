---
date: 2026-11-19
status: proposed
---

# Restrict crate publication to a trusted workflow

## Context

Crates.io lists `icalkit` 0.1.0, and the maintainer revoked the bootstrap token.
The registry can exchange a GitHub Actions identity for a short-lived publishing token.
[Trusted Publishing](https://crates.io/docs/trusted-publishing)

## Decision

Bind `icalkit` to the repository's publication workflow and the `crates-io` environment.
Require a protected `main` dispatch, the exact revision and version, successful candidate qualification, and environment approval.
Compare the archive with the qualified archive before authentication and check the published registry checksum.
Use a separate job in the bound workflow for authentication verification.

## Alternatives

Keep using a scoped API token for operator uploads.
That approach requires credential storage and rotation.
The registry checks a token's crate scope without checking a workflow or environment.

## Consequences

Publication requires a GitHub-hosted runner and access to the protected environment.
Revisit the binding if the repository, workflow filename, or environment changes.

## Confirmation

Reject an absent or different candidate and reserve OIDC permission for the two registry jobs.
Verify that the bound workflow exchanges its GitHub identity without uploading a crate.
