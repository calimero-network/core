# Releasing MeroKit

How a change in swift-sdk becomes a release. Agents and people read this before
committing, titling a pull request, or releasing. The first part is the set of
rules every calimero-network repository that ships releases shares; the second
is what is specific to this repository.

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

Some repositories also publish pre-releases (core's `X.Y.Z-rc.N`).
Which branch makes which kind of release, and
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

## This repository: swift-sdk

Short version: **Swift has no “npm publish”.** A Swift Package Manager (SPM)
package is distributed as a **public Git repo + semver Git tags**. Consumers point
their `Package.swift` at the repo URL and a version; SPM resolves the tag directly.
There is nothing to upload to a central host for the SwiftPM path.

### How consumers depend on it

```swift
dependencies: [
    .package(url: "https://github.com/calimero-network/swift-sdk.git", from: "0.1.0"),
]
```

SPM reads the Git tags of this repo and picks the best match for `from:` /
`.upToNextMajor(...)` etc. So “releasing a version” == **pushing a tag**.

### Cutting a release: merge to `master`

Releases are automatic. Every push to `master` runs
[`release-on-merge.yml`](.github/workflows/release-on-merge.yml), which runs
semantic-release ([`.releaserc.json`](.releaserc.json)) over the conventional
commits since the last `v*` tag:

| Commit (the squash-merge title) | Release |
| --- | --- |
| `feat: …` | minor |
| `fix:`, `perf:` | patch |
| `docs:`, `chore:`, `refactor:`, `style:`, `ci:`, `test:`, `build:`, `revert:` | none |
| `feat!: …`, or a `BREAKING-CHANGE:` footer | major |

Only the hyphenated `BREAKING-CHANGE` keyword counts: a body line that happens
to start with "breaking change" does not make a major (see the comment in
`.releaserc.json`).

When there is a release, it pushes the tag `v<version>` and creates the GitHub
Release with notes made from those commits, as the organization's GitHub App
(`vars.ACTIONS_APP_ID`, `secrets.ACTIONS_APP_PRIVATE_KEY`; the App must be
installed on this repository). A tag pushed with GITHUB_TOKEN would not start
other workflows. [`release.yml`](.github/workflows/release.yml) then builds and
tests the tag (`swift build -c release`, `swift test`); it creates a GitHub
Release only for a tag pushed by hand, so a release never gets two.

To release by hand anyway (a hotfix tag, say), push a `v`-prefixed semver tag;
`release.yml` verifies it and cuts the GitHub Release:

```bash
git tag v0.7.1
git push origin v0.7.1
```

#### CHANGELOG.md

[CHANGELOG.md](CHANGELOG.md) stays hand-written and is not touched by the
release: each change adds its own section in its pull request, headed
`## Unreleased: <what changed>`. The released version of each change is the
first `v*` tag after its merge, and the generated GitHub Release lists the
commits per version. (A release commit written back to `master` would need the
App to bypass branch protection, and the sections are prose a generated
changelog would duplicate.)

#### Release notes for a core bump

Every release that moves `ci/core-version` gets an entry in
[CHANGELOG.md](CHANGELOG.md) with an **Upgrade notes** section. Name anything a
caller must do by hand. For core rc.83 that is: log in again after the node
upgrade (tokens need a `key_id` claim, so older tokens are refused), and
`mero.logout()` is now `async`.

#### Versioning policy

Semver. We track the `mero-js` wire contract this SDK implements — aim to keep the
major aligned (e.g. **MeroKit 7.x ⇄ mero-js 7.x**) once we reach 1.0. Pre-1.0
(`0.x`) the minor may carry breaking changes.

### Discovery — the closest thing to “npm search”

The [**Swift Package Index**](https://swiftpackageindex.com) is the community
directory (search, docs, compatibility matrix). It’s an *index*, not a host — you
submit the repo URL once and it tracks the tags automatically. To list MeroKit:
open a PR adding the repo URL to
[SwiftPackageIndex/PackageList](https://github.com/SwiftPackageIndex/PackageList).
Add a `.spi.yml` to control which products/platforms it documents (optional).

### Emerging: the Swift Package Registry

Swift now has a formal package-registry protocol (`swift package-registry`,
SE-0292/SE-0391) — a genuine npm-style registry. Adoption is still limited and
GitHub doesn’t broadly host one yet, so **git-tag distribution remains the
default**. If we later publish to a registry, the release flow becomes
`swift package-registry publish` in `release.yml`; no code changes needed.

### CocoaPods (optional, for non-SPM consumers)

Not part of the automatic release: since 0.7.0 MeroKit depends on
swift-openapi-runtime and swift-http-types, which `MeroKit.podspec` cannot
express (they are not on the CocoaPods trunk), so the pod does not build as it
stands. Before that, the podspec was published by hand after tagging:

```bash
pod spec lint MeroKit.podspec
pod trunk push MeroKit.podspec
```

Bump `spec.version` in `MeroKit.podspec` to match the Git tag each release. Most
consumers should prefer SPM; the podspec exists only for teams still on CocoaPods.

### Checking that a release landed

- The tag `vX.Y.Z` and its GitHub Release exist, and the `Release` run on the
  tag (`swift build -c release`, `swift test`) is green.
- `swift package resolve` in a consumer pinned `from: "X.Y.Z"` resolves it; the
  Swift Package Index picks the tag up by itself.

### How consumers are bumped

There is no fleet bump for this SDK. The iOS apps in `calimero-network/apps` pin
it in their own `Package.swift` / `project.yml` and are moved by hand. The SDK
follows core's API through the OpenAPI sync in the shared rules.

### Manual steps that need a human

- A tag pushed by hand (the hotfix path above) and the CocoaPods trunk push are
  a human's steps; agents never do either.
- There is no pre-release line: every release from `master` is a final.
