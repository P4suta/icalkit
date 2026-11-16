# Repository policy

The repository policy records the intended public GitHub settings.
The offline `just repository` gate checks the links and essential phrases.
Maintainers compare the remote settings with the policy.

## Release posture

`icalkit` is the sole future production package and remains at `0.0.0`.
The public repository doesn't imply a published crate, a release, or semantic-version compatibility.
A tag, GitHub Release, registry publish, or version change requires a separate explicit release decision.
Ordinary pull requests and dependency updates must not trigger a release.

## Change flow

`main` is the protected default branch.
Work happens on a topic branch and enters through a pull request.
An incomplete change starts as a draft pull request.
It becomes ready only after its scope, tests, and migration impact are reviewable.

Development is test-driven: add or identify a failing test, make the smallest complete change that passes it, then run the relevant local gates.
Conventional Commits describe the final change.
Review generated files, public API snapshots, and conformance evidence with the source that changes them.

The development Rust version is an exact pin shared by `mise.toml` and `rust-toolchain.toml`.
The declared MSRV remains a separate compatibility promise.
Dependency-resolving verification recipes use `--locked` and reject changes to the committed dependency resolution.
The ordinary test commands run the standalone core without default features before testing the workspace.
`just repository` enforces the toolchain pins, standalone test inventory, and lockfile flags in the maintained recipes.
The development configuration pre-commit hook runs the same repository gate as ordinary CI.

The repository uses squash merging so one reviewed pull request becomes one linear commit.
Repository settings prohibit merge commits and rebase merges and delete merged branches automatically.
A branch must be current with `main` before it can merge.

## Protected branch

The `main` rule requires a pull request, resolved review conversations, strict successful status checks from `required`, and linear history.
No approval count is mandatory while the project has one maintainer.
The rule forbids force pushes and branch deletion.

`required` succeeds only after the ordinary Linux gate succeeds.
That gate includes formatting, strict linting, workspace tests and doctests, allocation and documentation checks, dependency policy, and architectural and repository checks.
It runs on pull requests and merge groups without repeating the functional inventory on main pushes.
CodeQL analyzes Rust and GitHub Actions on pull requests and updates the default-branch baseline after main changes.
Neither workflow runs on a schedule.
The remote rule requires the GitHub Actions `required` context with strict branch freshness.

The candidate qualification workflow verifies that a complete commit ID matches every checkout before running the full inventory.
It adds Windows and macOS tests, feature combinations, bare-metal and WebAssembly compilation, pinned-nightly API snapshots, and every workspace crate's MSRV verification.
Its summary rejects incomplete or unsuccessful results.
The repository has no publication workflow, so successful candidate qualification is verification evidence rather than an enforced publication boundary.

The [worktree decision](adr/candidate-worktree-validation.md) defines the private index used to compare tracked files without trusting the caller's cached flags or stat data.
The check preserves the original index, detects missing tracked content, and removes scratch storage on success and failure.
A future publication workflow must require successful qualification of the exact commit it publishes.

## Automation and permissions

GitHub Actions receives read-only repository permissions by default and may not approve pull requests.
Each workflow grants write permission only where a job needs it: `security-events: write` for CodeQL uploads.
Workflows pin third-party Actions to full commit SHAs, and actionlint and zizmor audit the workflows.

The repository uses GitHub's [selected actions policy](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository#allowing-select-actions-and-reusable-workflows-to-run) for external action sources.
The patterns permit `actions/checkout@*`, `jdx/mise-action@*`, and `Swatinem/rust-cache@*`.
The CodeQL patterns are `github/codeql-action/init@*` and `github/codeql-action/analyze@*`.
The candidate proof helper pattern is `P4suta/project-template/.github/actions/ci-policy@*`.
GitHub requires every action to use a full commit SHA.
Broad permissions for GitHub actions and verified Marketplace creators are off.
Review the source and update the allowed patterns before adding an action from another repository.

Renovate checks Cargo and GitHub Actions through the centrally managed repository configuration.
Renovate groups compatible Cargo updates and isolates major updates for explicit compatibility review.
Automated dependency pull requests must pass the same protected checks as maintainer changes.

## Security settings

The repository enables dependency graph alerts, CodeQL, secret scanning, secret-scanning push protection, and private vulnerability reporting.
Public issues direct security-sensitive reports to the private advisory flow in `SECURITY.md`.

GitHub reports the optional non-provider pattern and validity extensions as off for the personal public repository.
The extensions aren't active controls.
Maintainers should enable them and update the policy if repository-plan availability changes.

The repository stores no app credentials and no raw private calendar exports.
Reduce and anonymize real-client fixtures, and record their provenance.
Raw intake stays outside the worktree and is never committed.

## Public metadata and collaboration

The repository enables Issues for maintenance work and GitHub Discussions for questions.
Keep Projects and the wiki off until they have an owner and a use that the roadmap, ADRs, or Discussions don't serve.
The description and topics name Rust, iCalendar, recurrence, iTIP, time zones, CalDAV, sans-I/O, and `no_std` without implying a release.

CODEOWNERS records the current maintainer and identifies architecture, security, conformance, and automation boundaries.
A future maintainer change updates CODEOWNERS and the repository policy together.
