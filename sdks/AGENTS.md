# sdks/: the client SDKs and components, on core's version

Every official SDK lives here and ships with core: one version (the root
`VERSION`), one tag, one release. Read this before adding an SDK, changing
how one is built, tested, generated or published, or touching `VERSION`.

```
sdks/
├── clients/                 # talk to a node: one per language
│   ├── mero-js/             # TypeScript  npm  @calimero-network/mero-js
│   ├── mero-py/             # Python      PyPI mero-py
│   ├── mero-swift/          # Swift       SwiftPM from core's URL (the root Package.swift)
│   └── mero-kotlin/         # Kotlin      Maven network.calimero:mero-kotlin(-*)
├── components/              # UI built on a client
│   ├── mero-react/          # React       npm  @calimero-network/mero-react (on mero-js)
│   ├── mero-design/             # React       npm  @calimero-network/mero-design (design system; on mero-icons, mero-tokens)
│   ├── mero-icons/          # React       npm  @calimero-network/mero-icons
│   ├── mero-tokens/         # TypeScript  npm  @calimero-network/mero-tokens
│   └── mero-charts/         # React       npm  @calimero-network/mero-charts (on mero-design)
├── plugins/connectors/      # Rust crates an app compiles in to reach one external service
│   └── nearai/              # calimero-connector-nearai: NEAR AI Cloud chat completions
├── abi/                     # apps' released ABIs the clients generate typed app clients from (SOURCE: where each comes from)
├── package.json             # the pnpm workspace of the JS SDKs (private)
└── pnpm-workspace.yaml
```

## Rules

- **Names**: every SDK is `mero-<language>` (a component: `mero-<framework>`),
  the same name for its directory, its `sdk.json` `name` and what it publishes
  where the registry allows it. `scripts/sdks.py` refuses any other name.
- **One version**: the root `VERSION`, which is core's release: the root
  `Cargo.toml`'s `[workspace.metadata.workspaces]` `version` (e.g.
  `1.0.0-rc.30`). Every file that states an SDK's version is listed in its
  `sdk.json` `version_files`. A release cut bumps Cargo's version and runs
  `python3 scripts/sdks.py version-sync` in the same commit, which writes it
  into VERSION and every SDK; the `SDK scope` job (and release.yml) fail while
  any of them differs. Never bump one SDK on its own: SDKs release only with
  core.
- **Inside the workspace** mero-react depends on mero-js, mero-design on mero-icons
  and mero-tokens, and mero-charts on mero-design, through `workspace:*`; pnpm
  publishes that as the exact version.
- **Generated vs written**: each client's generated layer (types, transport,
  the method list) comes from the node's API document,
  `docs/public/openapi.json`, through the SDK's `generate` command
  (`python3 scripts/sdks.py generate [name]`). Never edit generated files by
  hand; change the server (or the generator) and regenerate. The one command
  works on macOS and Linux: with no `swift` installed, mero-swift generates in a
  pinned `swift` Docker image (`SDKS_GENERATE_IN_CONTAINER=1` forces that).
  A client's typed app clients (the Confidential KV's) are generated the same
  way, by core's `meroctl app codegen` (`scripts/app-codegen.sh`, from this
  tree) from an ABI vendored once in `sdks/abi/`.
  The hand-written, ergonomic layer is everything else.
- **Docs**: a client's guides and reference are pages of core's docs site,
  `docs/src/content/docs/clients/<name>/` (the Clients section), built and
  link-checked with the rest of `docs/`; no SDK keeps a docs site of its own.
  A generated page (mero-py's method reference) is listed in `generated` by
  its path from the SDK's directory (`../../../docs/...`). A change to a
  client's API updates its pages there in the same pull request.
- **Examples** live in each SDK's own `examples/` and are built by its CI job.
- **Tests run only when they can be affected**: `scripts/sdks.py scope` maps a
  change to the SDKs it reaches (own directory, a dependency, an `inputs` file,
  its workflow, VERSION, the shared machinery). `.github/workflows/sdks.yml`
  runs one reusable workflow per SDK (`sdk-<name>.yml`) and reports the single
  required check `SDKs`.

## What each SDK carries

`sdks/<kind>/<name>/sdk.json` (schema in `scripts/sdks.py`'s docstring):
`name`, `kind` (`client` | `component`), `language`, `published`,
`version_files`, `depends_on`, `inputs`, `generate`, `generated`, `workflow`.

Its own `AGENTS.md` explains its layout, its build and test commands and its
generator. Its `CHANGELOG.md` keeps the history it had as a separate
repository; new entries go in core's release notes.

## When the server's API changes

1. A PR changes the server; CI regenerates `docs/public/openapi.json`
   (calimero-node's openapi test).
2. The same PR regenerates the SDKs (`python3 scripts/sdks.py generate`),
   fixes any hand-written layer the change breaks, and commits both.
   `SDK codegen drift` regenerates every SDK the change reaches and fails while
   any generated file differs; it is part of the required `SDKs` check, so a PR
   with stale clients cannot merge.
3. Should master drift anyway, `.github/workflows/sdk-regenerate.yml`
   regenerates every SDK and opens (or refreshes) one pull request,
   `chore/sdk-regenerate`, titled by what the change does to callers
   (`scripts/openapi-classify.py`) and labelled `api-breaking` when it breaks
   them.

## Release

A release cut (RELEASING.md) releases everything from the same commit:
`release.yml` checks that `VERSION` is the release, and once core's binaries
are released publishes each SDK through its workflow's `publish` path (npm:
mero-js then mero-react, and mero-icons and mero-tokens, then mero-design, then
mero-charts; PyPI: mero-py; Maven Central: mero-kotlin; mero-swift needs nothing: SwiftPM
installs it from core's tag, through the root `Package.swift`). Then
`fleet-bump.yml` opens the downstream pull requests: cargo and tee as before,
and npm (`surfaces: npm`) for the apps that pin the JS SDKs and components. A run of
release.yml by hand runs every publish path as a dry run.

## Adding an SDK

1. `git subtree add --prefix=sdks/<kind>/mero-<lang> <repo> <branch>` (keeps
   its history), then remove what the monorepo replaces: its `.github/`,
   semantic-release config, its own version bumps.
2. Add `sdk.json`, its `AGENTS.md` (and `CLAUDE.md -> AGENTS.md`), and
   `.github/workflows/sdk-<name>.yml` (`workflow_call`, with a `publish` input).
3. Add its job to `sdks.yml` (the `scope` output and the `SDKs` gate's needs)
   and its publish job to `release.yml`.
4. `python3 scripts/sdks.py version-sync && python3 scripts/tests/sdks-test.py`.
