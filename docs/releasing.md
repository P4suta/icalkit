# Releasing icalkit

The release process publishes `icalkit` alone.
Version changes, registry publication, and GitHub publication require a separate release decision.

## Select the first version

Add a section with the approved version to the root changelog before running release-plz.
Summarize the supported API and behavior in the first entry.
Use the approved first version for `VERSION`.

```sh
VERSION=0.1.0
mise x -- release-plz set-version "$VERSION"
```

Review the workspace manifest, local dependency requirements, and lockfile.
Run the normal pull request checks.

## Prepare later versions

Preview the release-plz result from a clean checkout in a temporary branch or worktree.

```sh
mise x -- release-plz update -p icalkit
git diff --check
git diff --stat
```

Review the version, changelog, and lockfile changes.
The [release-plz configuration](../release-plz.toml) uses the published crate as its compatibility baseline and limits proposals to `icalkit`.
It disables registry publication, tag creation, and GitHub Releases.

After the first crate is available on crates.io, configure the `release-proposals` environment for deployments from `main` and turn off administrator bypass.
Give the environment a read-only `DOPPLER_TOKEN` scoped to the repository's release-plz config.
Set `RELEASE_PLZ_DOPPLER_PROJECT` and `RELEASE_PLZ_DOPPLER_CONFIG` on the environment to resolve `RELEASE_PLZ_APP_CLIENT_ID` and `RELEASE_PLZ_APP_PRIVATE_KEY` from the shared release-plz App configuration.
Install the App on this repository with repository contents and pull request write permissions.
The [proposal workflow](../.github/workflows/release-proposals.yml) requests an installation token for this repository.

Add the workflow's reviewed `dopplerhq/secrets-fetch-action` and `actions/create-github-app-token` commit pins to the selected Actions policy.
Preserve the existing action sources and full SHA pinning.
With the repository variable `RELEASE_PLZ_PROPOSALS` set to `enabled`, changes to `main` can create or update a draft.
Run the workflow manually from `main` to retry a proposal and continue its review through the normal pull request flow.

## Qualify a release commit

Use a clean checkout of the merged preparation commit on protected `main`.
Match `VERSION` to the crate version.

```sh
REVISION=$(git rev-parse --verify HEAD)
VERSION=0.1.0
gh workflow run candidate.yml --ref main -f revision="$REVISION" -f release_version="$VERSION"
```

Check that the run reports the requested commit and that its final required job succeeds.
The package job runs the locked tests from the extracted crate with all features and runs the packaged `golden_path` example without default features.
The release version must match the crate and differ from `0.0.0`.

From a clean checkout of the same commit, rehearse Cargo's registry preparation.

```sh
mise x -- cargo publish --dry-run --locked -p icalkit
```

Keep the generated crate archive and record its SHA-256 checksum for the publication decision.
Repeat qualification if the selected source or version changes.

## Publish after authorization

The [crates.io setup](https://crates.io/docs/trusted-publishing) requires an existing crate for Trusted Publishing.
Use a short-lived registry token for the authorized first upload and retire it after use.
Verify the registry checksum for the approved version and install the published crate from a clean consumer project with `cargo add icalkit`.
Create and push the signed version tag at the approved commit.
Create the draft from the existing tag with `gh release create "v$VERSION" --draft --verify-tag` and supply the approved release notes.
Verify that the draft GitHub Release matches the approved archive, source revision, and version.
Attach the final checksums and provenance before publishing the GitHub Release.
Keep the exact release files for recovery.

After the registry records the first version, configure Trusted Publishing for the approved publication workflow.
Bind it to the exact repository, workflow, and protected environment.
