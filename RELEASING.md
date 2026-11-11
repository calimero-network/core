# Releasing

How a change in this repository becomes a release, and the rules every
calimero-network repository that ships releases follows. Agents and people read
this before committing, titling a pull request, or releasing.

This file is also the canonical copy of the shared rules: the section between
the `shared-release-rules` markers is copied verbatim into the `RELEASING.md` of
mero-js, mero-react, calimero-client-py, swift-sdk and kotlin-sdk.

<!-- shared-release-rules:start -->
## Shared rules

These rules are the same, word for word, in every calimero-network repository
that ships releases. The canonical copy is in
[core's RELEASING.md](https://github.com/calimero-network/core/blob/master/RELEASING.md);
change it there first, then copy the text between the `shared-release-rules`
markers into each repository unchanged. What differs per repository (branches,
tags, where things are published, manual steps) is in the repository section.

### The PR title is the release commit

Pull requests are squash-merged, so the PR title becomes the single commit that
lands on the default branch, and that commit is what release automation reads.
Commits inside the PR do not count. Write the title as a
[Conventional Commit](https://www.conventionalcommits.org/), lower case:
`<type>(<optional scope>): <subject>`.

Where a repository releases from its commits (semantic-release; the repository
section says whether it does), the type picks the version bump:

| PR title | Release |
| --- | --- |
| `feat: ...` | minor |
| `fix:`, `perf:` | patch |
| `docs:`, `chore:`, `refactor:`, `style:`, `test:`, `build:`, `ci:`, `revert:` | none |
| `<type>!: ...`, or a `BREAKING-CHANGE:` footer | major |

- Only the hyphenated `BREAKING-CHANGE:` footer keyword makes a major (besides
  `!`). The commit parser matches note keywords case-insensitively at the start
  of any line of the message, so with the default keyword `BREAKING CHANGE` a
  body line that happened to wrap onto "breaking change ..." published
  mero-react 7.0.0 from a commit that touched only CI and a test fixture. The
  `.releaserc.json` files therefore set `parserOpts.noteKeywords` to
  `["BREAKING-CHANGE"]`, and prose that says "breaking change" does nothing.
- Prefer `!` in the title over the footer: the title is visible in review,
  while the squash commit's body is whatever the merge dialog fills in.
- Only `feat:`, `fix:` and `perf:` release (plus a major from `!` or the
  footer); every other type releases nothing. A change to what users read in
  the package (a README shipped to npm, docstrings) that should reach them is
  titled `fix(docs):` (a patch). To ship a change that touches no feature or
  fix (e.g. a dependency bump consumers need), title it `fix(deps): ...`.
- Choose the type for what the change does to whoever depends on the release,
  not for the size of the diff.

### What counts as breaking

For a library (an SDK, a client, a package other code imports), a change is
breaking when code that built and worked against the previous release can stop
building or behave differently:

- an export, type, method, function or field is removed or renamed;
- a signature changes (a parameter added without a default, a parameter or
  return type changed, a call that becomes async);
- a field of a request, option or config type becomes required;
- accepted values narrow (an enum loses a member, validation refuses input it
  used to accept);
- the error a caller can catch or match on changes type or code.

Additions (a new export, method or optional field) are `feat`.

For an app or a service (a node, an image, a deployed backend), breaking is
what its users or operators must act on: an API route or field removed or
changed, stored data or a wire format that the previous release cannot read
or that cannot be read back, configuration or flags that must change, clients
that must upgrade first. Core declares these through its
[compatibility rules](https://github.com/calimero-network/core/blob/master/docs/src/content/docs/operate/compatibility.mdx)
(deprecations, hard cutovers), not through the title alone.

### Releases happen on merge, through automation

- A repository's workflows release it, triggered by a merge to the release
  branch (or, where the repository section says so, by a tag a human pushes).
  When a workflow's tag or pull request must start another workflow, it acts as
  the organization's GitHub App (`vars.ACTIONS_APP_ID` /
  `secrets.ACTIONS_APP_PRIVATE_KEY`): anything pushed with `GITHUB_TOKEN`
  starts no workflow.
- Never push a tag, create a GitHub Release, publish a package or image, or
  edit a version number by hand, unless the repository section names that
  step as manual and a human asked for it.
- AI agents never do any of these without a human's explicit approval, even
  when the repository section lists the step. An agent's part ends at a pull
  request with the right title.
- A bad release is fixed by the next release (a `fix:` pull request), not by
  moving or deleting a tag: npm, PyPI, Maven Central and crates.io never take
  a published version back.

### OpenAPI sync pull requests from core

The SDKs generate their HTTP clients from core's `docs/public/openapi.json`.
When it changes on core's master, core's
[`sdk-openapi-sync.yml`](https://github.com/calimero-network/core/blob/master/.github/workflows/sdk-openapi-sync.yml)
has each SDK re-vendor it on the machine-owned branch `chore/core-openapi` and
open (or refresh) one pull request. Core's classifier titles that pull request
from what changed in the generated surface: `feat(api)!:` when something a
caller uses was removed or changed, `feat(api):` when something was only
added, `chore(api):` otherwise (descriptions only), which releases nothing:
that change reaches consumers with the SDK's next release.

Don't retitle these down: `feat(api)!:` turned into `feat(api):` ships a
breaking change as a minor to every consumer on a `^` range. Retitle up if the
classifier missed a break. Don't push to `chore/core-openapi`; the workflow
force-pushes it. Fix the document or the generator in core instead.

### A major stops the fleet's automatic bumps

When a package is published, its repository's fleet-bump workflow opens a pull
request in every consumer listed in its `.github/fleet.json`.
`bump-fleet.sh` skips a jump across a major version unless the run sets
`allow_major` (core's fleet bump skips majors the same way), because such a bump
would arrive red on every release. So after a major, consumers stay on the old
major and stop receiving that package's releases until someone moves them on
purpose: plan the consumers' migration pull requests as part of shipping a
major, and expect an accidental major to cut the whole fleet off silently.

### Pre-release lines

Some repositories also publish pre-releases (core's `X.Y.Z-rc.N`, the `rc`
channel of calimero-client-py). Which branch makes which kind of release, and
how a pre-release line becomes a final, is in the repository section. Promoting
a line is a human's decision.

### Before you commit, title a pull request, or release

1. The title is a Conventional Commit whose type matches the table above and
   the breaking rules.
2. A breaking change has `!` in the title and says in the PR body what callers
   must change; where the repository keeps a hand-written changelog, the PR
   adds its entry.
3. Version fields, tags and release configuration are left alone unless that is
   the point of the PR and a human asked for it.
4. After the merge, check that the release landed as the repository section
   describes.
<!-- shared-release-rules:end -->

## This repository: core

Core does not release from commit types. Its version is chosen by hand in a
release pull request; the rules above still apply to every title (the `PR Title`
check lints them) and to what counts as breaking.

### How a release is cut

- **Version.** The one version of every public crate, `merod`, `meroctl` and
  `mero-sign` is `version` under `[workspace.metadata.workspaces]` in the root
  `Cargo.toml` (`1.0.0-rc.15` at the time of writing). Nothing else holds it.
- **Release pull request.** `chore(release): cut X` sets that version to `X`
  and turns `CHANGELOG.md`'s `## [Unreleased]` into a dated `## [X] - YYYY-MM-DD`
  section with the changes since the last release. `CHANGELOG.md` is
  hand-written: every pull request that changes behaviour adds its entry under
  `[Unreleased]`. Deciding to cut a release and merging this pull request is a
  human's call; an agent drafts one only when asked.
- **Merge to master.** [`release.yml`](.github/workflows/release.yml) runs on
  every push to master that touches the build. Its `prepare` job reads the
  version; when no GitHub Release named `X` exists yet, it creates the release
  and its tag `X` (the bare version, no `v`) at the merged commit, marked as a
  pre-release when the version has a suffix such as `-rc.N`. A push that does
  not change the version releases nothing (master still gets its `edge` image).
- **The upgrade path pin.** Once the release's assets are up, a follow-up pull
  request (`chore(release): upgrade path from X`) sets `.github/upgrade-from` to
  `X`, at the latest in the next release pull request. Hard cutovers set it to
  `hard-cutover: ...` instead. See
  [CONTRIBUTING.md, "Cutting a release"](CONTRIBUTING.md#cutting-a-release) and
  [Compatibility](docs/src/content/docs/operate/compatibility.mdx).

### What a release publishes

All from `release.yml`, on master only:

| What | Where |
| --- | --- |
| `merod`, `meroctl`, `mero-sign` (Linux x86_64 and aarch64), `meroctl` and `mero-sign` (macOS aarch64), `calimero-tee-verify` (Linux), `SHA256SUMS` with build provenance | the GitHub Release `X` |
| `kv-store-test-fixture.mpk`, the kv app bundle the upgrade path and clients test with | the GitHub Release `X` |
| the public crates (`cargo ws publish`) | crates.io |
| the `merod` image, multi-arch: `X`, plus `latest` for a final or `prerelease` for a pre-release | `ghcr.io/calimero-network/merod` |
| a `meroctl` formula bump | a pull request in `calimero-network/homebrew-tap` |
| SDK pin bumps for consumers | pull requests opened by `fleet-bump.yml` (below) |

Every master push that builds also pushes `ghcr.io/calimero-network/merod:edge`
and a short-sha tag; neither is a release.

### Pre-releases and the release branch

- Until 1.0 is final, releases are `1.0.0-rc.N`: GitHub pre-releases, the
  `prerelease` image tag, and pre-release crate versions.
- `release-v2-parity` is a release branch that publishes only the `merod`
  image, under `release-v2-edge` and `release-v2-<short sha>`. It cuts no
  GitHub Release, crates, tap or fleet bump.

### Manual steps that need a human

- Opening and merging the release pull request, and the upgrade path follow-up.
- A failed release build is fixed by re-running that same run (the tag points
  at its commit), not by a new tag. If the release could not be created because
  master moved on, the job's error prints the `gh api` commands to create the
  tag and release by hand; only a human runs them.
- Majors: the fleet bump skips a major jump (see the shared rules), so a core
  major needs its consumers moved deliberately, and `workflow_dispatch` on
  `fleet-bump.yml` (`dry_run` is on by default there) re-runs a bump for a
  version or a subset of repositories.

### Checking that a release landed

- The GitHub Release `X` exists with every asset above and `SHA256SUMS`.
- `docker pull ghcr.io/calimero-network/merod:X` works.
- crates.io lists the crates at `X`.
- The homebrew-tap pull request and the fleet-bump pull requests are open, and
  the `Release` run on the merge commit is green.

### How consumers are bumped

`fleet-bump.yml`, called by `release.yml` after the binaries are released,
opens one pull request per consumer in [`.github/fleet.json`](.github/fleet.json)
on the branch `chore/core-X`, as the organization's GitHub App, with
[`.github/scripts/bump-fleet.sh`](.github/scripts/bump-fleet.sh) doing the edits:

- `cargo` surface (apps, mero-ar, mero-tag, calimero-studio): pins
  `calimero-network/core` to the tag `X` (`chore(deps): pin the Calimero SDK to X`).
  A source-level break makes that pull request red until someone writes the code.
- `tee` surface (mero-tee): sets `merodVersion` in `mero-tee/versions.json`,
  re-pins `mero-tee/build-inputs.lock.json` from the release's `SHA256SUMS`, and
  cuts a new image version (`chore(image): bundle merod X and cut a new image version`).
  The image itself is released by mero-tee's own workflows.
- `npm` entries in `fleet.json` are read by mero-js, mero-react and
  design-system, whose own fleet bumps move the npm packages.

The SDKs (mero-js, calimero-client-py, swift-sdk, kotlin-sdk) follow core's API
through the OpenAPI sync described in the shared rules, not through a pin.
