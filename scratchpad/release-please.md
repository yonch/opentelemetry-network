# Release Please integration: findings and recommendations

This document proposes integrating Release Please to automate versioning and release notes, and to trigger the existing build-and-release pipeline by simply merging a Release PR. It follows the layout used in other OpenTelemetry repos (notably opentelemetry-js-contrib) while adapting to this repository’s mixed toolchain and current release flow.


## Goals
- Automate version bumps and changelog generation via Release PRs.
- Create a Git tag and GitHub Release automatically once the Release PR is merged.
- Trigger the existing build-and-release workflow on the published release, so RPM/DEB artifacts and container images are produced and attached without manual steps.
- Keep “unofficial” builds available via the manual workflow path.


## Current state (as of this repo)
- Version is defined in `version.sh` as three exports:
  - `EBPF_NET_MAJOR_VERSION`, `EBPF_NET_MINOR_VERSION`, `EBPF_NET_PATCH_VERSION`.
  - A build number is computed from commit count since `COLLECTOR_BUILD_NUMBER_BASE`.
- `CMakeLists.txt` and `cmake/defaults.cmake` derive version values by sourcing `version.sh` when needed.
- The release workflow is fully manual per RELEASING.md:
  - Manually tag `vMAJOR.MINOR.PATCH` and draft a release.
  - Manually run `.github/workflows/build-and-release.yaml` with inputs.
  - Manually upload generated RPM/DEB artifacts to the GitHub release.
  - Manually bump `version.sh` for next development cycle.


## How Release Please works (relevant pieces)
- Reads Conventional Commit messages to determine next version (major/minor/patch).
- Opens a Release PR that:
  - Bumps version(s) and updates changelog(s).
  - Includes release notes assembled from commits since the last release.
- On merge, the workflow runs again and creates a Git tag and a GitHub Release (with notes).
- Common setup used by OpenTelemetry repos:
  - `release-please-config.json` for configuration (changelog types, bumping semantics, per-package settings).
  - `.release-please-manifest.json` to store current versions.
  - `.github/workflows/release-please.yml` that runs on `push` to `main` and optionally `workflow_dispatch`, using the Release Please action in “manifest” mode.


## Recommended design for this repository

1) Use “manifest” mode + single top-level package
- Files:
  - `release-please-config.json`
  - `.release-please-manifest.json`
  - `.github/workflows/release-please.yml`
- Treat the whole repo as a single releasable “package” (we release RPM/DEB and Docker images as a unit). This mirrors the pattern used in opentelemetry-js-contrib’s monorepo, just with a single package here.

2) Canonical version source: introduce a `VERSION` file
- Put `0.11.0` (current) in a plain text `VERSION` file at repo root.
- Update `version.sh` to read `VERSION` and set `EBPF_NET_MAJOR_VERSION`, `EBPF_NET_MINOR_VERSION`, `EBPF_NET_PATCH_VERSION` by parsing it.
  - This keeps all existing consumers (CMake, scripts, workflow) unchanged, while giving Release Please a simple single-file target to update.
- Rationale: Release Please easily bumps a single SemVer string file; updating three separate shell variables is possible with regex updaters but is more brittle.

3) Bumping semantics (pre-1.0)
- Use the standard Release Please knobs for <1.0:
  - `bump-minor-pre-major: true` (treat `feat` as a minor bump when major is 0).
  - `bump-patch-for-minor-pre-major: true` (treat breaking changes as minor instead of major when pre-1.0).
- This matches common OTel practice prior to 1.0 and keeps risk lower while still signaling changes appropriately.
- Support overrides via commit footer `Release-As: 0.12.0` when explicit coordination is required.

4) Changelog sections (conventional commits)
- Group entries similar to other OTel repos: Features, Bug Fixes, Performance, Reverts, Dependencies, Docs, CI, Chore, Refactor, Tests, Build.
- This yields a readable CHANGELOG that aligns with OTel conventions users are accustomed to.

5) Workflow integration: trigger builds on “release published”
- Keep the existing `.github/workflows/build-and-release.yaml` for both manual and automatic runs.
- Add an additional trigger for `release: { types: [published] }` (or alternatively `push` on tags `v*`).
- When triggered by a published release, the job should:
  - Checkout the tag (`ref: ${{ github.event.release.tag_name }}`).
  - Build packages and images as it already does.
  - Upload RPM/DEB as assets to the existing GitHub Release (the release is already created by Release Please, so we attach to it rather than creating a new one).
- Preserve the manual `workflow_dispatch` path for “unofficial” builds.
- Prefer `release.published` over `push: tags` to avoid race conditions with release note creation and asset upload.


## Proposed files and contents

1) release-please-config.json
```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "bootstrap-sha": "",
  "include-v-in-tag": true,
  "bump-minor-pre-major": true,
  "bump-patch-for-minor-pre-major": true,
  "changelog-path": "CHANGELOG.md",
  "changelog-sections": [
    { "type": "feat",       "section": "Features" },
    { "type": "fix",        "section": "Bug Fixes" },
    { "type": "perf",       "section": "Performance" },
    { "type": "revert",     "section": "Reverts" },
    { "type": "deps",       "section": "Dependencies", "hidden": false },
    { "type": "docs",       "section": "Documentation", "hidden": false },
    { "type": "ci",         "section": "CI", "hidden": false },
    { "type": "chore",      "section": "Chores", "hidden": true },
    { "type": "refactor",   "section": "Refactoring", "hidden": false },
    { "type": "test",       "section": "Tests", "hidden": true },
    { "type": "build",      "section": "Build", "hidden": true }
  ],
  "packages": {
    ".": {
      "release-type": "simple",
      "changelog-path": "CHANGELOG.md",
      "extra-files": [
        "VERSION"
      ]
    }
  }
}
```

2) .release-please-manifest.json
```json
{
  ".": "0.11.0"
}
```

3) .github/workflows/release-please.yml
```yaml
name: release-please
on:
  push:
    branches: [ main ]
  workflow_dispatch: {}

permissions:
  contents: write
  pull-requests: write

jobs:
  release-please:
    runs-on: ubuntu-24.04
    steps:
      - name: Run Release Please
        uses: google-github-actions/release-please-action@v4
        with:
          command: manifest
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json
```

4) version.sh adjustment (read VERSION)
- Suggested approach: if `VERSION` exists, parse it; else fall back to current hard-coded variables as today.
- Minimal diff sketch:
```sh
# At the top after set -e
VERSION_FILE="$(dirname "$0")/VERSION"
if [ -f "$VERSION_FILE" ]; then
  EBPF_NET_VERSION=$(cat "$VERSION_FILE" | tr -d "\n" )
  # shellcheck disable=SC2001
  EBPF_NET_MAJOR_VERSION=$(echo "$EBPF_NET_VERSION" | sed -E 's/^([0-9]+)\..*$/\1/')
  EBPF_NET_MINOR_VERSION=$(echo "$EBPF_NET_VERSION" | sed -E 's/^[0-9]+\.([0-9]+)\..*$/\1/')
  EBPF_NET_PATCH_VERSION=$(echo "$EBPF_NET_VERSION" | sed -E 's/^[0-9]+\.[0-9]+\.([0-9]+).*$/\1/')
  export EBPF_NET_MAJOR_VERSION EBPF_NET_MINOR_VERSION EBPF_NET_PATCH_VERSION
else
  # existing exports remain as fallback for dev
  export EBPF_NET_MAJOR_VERSION='0'
  export EBPF_NET_MINOR_VERSION='11'
  export EBPF_NET_PATCH_VERSION='0'
fi
```
- Rationale: keeps existing environment variables intact for CMake and scripts; Release Please will only update `VERSION`.

5) build-and-release.yaml trigger and tag-aware behavior
- Add a `release` trigger and route values based on event type:
```yaml
on:
  workflow_dispatch:
    inputs: { ... as today ... }
  release:
    types: [published]

# in Checkout sources
with:
  ref: ${{ github.event_name == 'workflow_dispatch' && inputs.ref || github.event.release.tag_name }}

# Upload packages to Release step
with:
  tag_name: ${{ github.event.release.tag_name || env.github_tag }}
  # when triggered by release, the release already exists; the action attaches assets
```
- Prefer publishing on the already-created release to avoid racing the Release Please action.
- Keep `unofficial` path entirely under `workflow_dispatch` with current inputs and tag formatting.


## Release flow after adoption
- Developer uses Conventional Commits in PR titles and/or the squash commit.
- Release Please bot opens a Release PR bumping `VERSION` and updating `CHANGELOG.md`.
- Maintainer merges the Release PR.
- Release Please action runs again, creating tag `vX.Y.Z` and publishing the GitHub Release with notes.
- `build-and-release` job runs on the `release.published` event:
  - Checks out the tag, builds RPM/DEB and images, uploads assets to the release, pushes images with tags:
    - `latest`, `latest-vMAJOR.MINOR`, `vMAJOR.MINOR.PATCH`.
- No manual version bump or artifact upload required.


## Customizing version bumps
- Default: `feat` → minor, `fix`/`perf` → patch, breaking changes (e.g., `feat!:`, `BREAKING CHANGE:`) → minor while <1.0 (via `bump-patch-for-minor-pre-major: true`).
- To override, include a commit footer in the Release PR:
  - `Release-As: 0.12.0` (or similar) to force a specific next version.
- This matches the “custom increment” flexibility you mentioned and is commonly used across OTel when coordinating cross-repo releases.


## Open questions/decisions
- Is a single top-level release sufficient? (It matches today’s publish targets.)
- Do we want to generate release notes per “component” (e.g., collector, reducer) as sections? If so, we can refine `changelog-sections` and/or use labels to tag PRs by area.
- If we want nightly or ad-hoc preview builds, keep using the `workflow_dispatch` path (`release_type: unofficial`).


## Risks and mitigations
- Race conditions between tag creation and asset upload: avoided by triggering on `release.published` rather than `push: tags`.
- Version drift: avoided by making `VERSION` the single source of truth and deriving env vars from it in `version.sh`.
- Pre-1.0 semantics: explicitly configured to keep changes conservative; maintainers can override with `Release-As:` when needed.
- Build number: remains computed from commit count and will be pinned at release time; no change required to the logic that uses `COLLECTOR_BUILD_NUMBER_BASE`.


## Implementation checklist (what I can do next)
- Add `release-please-config.json` and `.release-please-manifest.json`.
- Add `.github/workflows/release-please.yml`.
- Add `VERSION` (seed with `0.11.0`) and update `version.sh` to parse it.
- Update `.github/workflows/build-and-release.yaml` to also trigger on `release.published` and attach assets to that release.
- Update `RELEASING.md` to describe the new flow and conventions.

If you want, I can implement these changes in a follow-up PR here.
