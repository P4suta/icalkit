# Repository policy

This document is the reviewable source of truth for the public GitHub repository settings. The
offline `just repository` gate holds its links and essential phrases; maintainers periodically
compare the remote settings with this file.

## Release posture

`icalkit` is the sole future production package and remains at `0.0.0`. The repository may be
public and production-shaped without publishing a crate, creating a release, or promising
semantic-version compatibility. A tag, GitHub Release, registry publish, or version change
requires a separate explicit release decision. Ordinary pull requests and dependency updates
must not trigger a release.

## Change flow

`main` is the protected default branch. Work happens on a topic branch and enters through a
pull request. An incomplete change starts as a draft pull request; it becomes ready only after
its scope, tests, and migration impact are reviewable.

Development is test-driven: add or identify a failing test, make the smallest complete change
that passes it, then run the relevant local gates. Conventional Commits describe the final
change. Generated files, public API snapshots, and conformance evidence are reviewed with the
source that changes them.

The repository uses squash merging so one reviewed pull request becomes one linear commit.
Merge commits and rebase merges are disabled, merged branches are deleted automatically, and a
branch must be current with `main` before it can merge.

## Protected branch

The `main` rule requires:

- a pull request, with no mandatory approval count while the project has one maintainer;
- resolved review conversations;
- strict successful status checks from `ci-required`, `analyze (rust)`, and
  `analyze (actions)`;
- linear history;
- no force pushes and no branch deletion.

`ci-required` aggregates every functional, portability, policy, documentation, dependency, and
resource-bound job in `.github/workflows/ci.yml`. CodeQL remains separate so both the Rust and
GitHub Actions analyses are visible as security gates. Administrators retain an emergency
bypass, but routine work follows the same pull-request path.

## Automation and permissions

GitHub Actions receives read-only repository permissions by default and may not approve pull
requests. Each workflow grants narrower write permission only where a job needs it, currently
`security-events: write` for CodeQL uploads. Third-party Actions are pinned to full commit SHAs
and are audited by actionlint and zizmor.

Dependabot checks Cargo and GitHub Actions weekly. Compatible Cargo updates are grouped; major
updates remain isolated for explicit compatibility review. Automated dependency pull requests
must pass the same protected checks as maintainer changes.

## Security settings

Dependency graph alerts, Dependabot security updates, CodeQL, secret scanning, secret-scanning
push protection, and private vulnerability reporting are enabled. Public issues direct
security-sensitive reports to the private advisory flow in `SECURITY.md`.

The repository stores no application credentials and no raw private calendar exports. A
real-client fixture must be minimized, anonymized, and accompanied by provenance. Raw intake
stays outside the worktree and is never committed.

## Public metadata and collaboration

Issues are enabled for actionable work and GitHub Discussions is enabled for questions.
Projects and the wiki are disabled until they have an owner and a use that is not already
served by the roadmap, ADRs, or Discussions. The description and topics name Rust, iCalendar,
recurrence, iTIP, time zones, CalDAV, sans-I/O, and `no_std` so the repository is discoverable
without implying a release.

CODEOWNERS records the current maintainer and calls out architecture, security, conformance, and
automation boundaries. A future maintainer change updates CODEOWNERS and this policy together.
