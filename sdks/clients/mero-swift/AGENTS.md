# mero-swift (MeroKit): agent guide

> mero-swift lives in calimero-network/core at `sdks/clients/mero-swift` and
> releases with core, on core's version (the root `VERSION`). Read
> [core's `sdks/AGENTS.md`](https://github.com/calimero-network/core/blob/master/sdks/AGENTS.md)
> and core's `RELEASING.md` before
> changing how it is built, versioned or published.
> calimero-network/swift-sdk, its old repository, is archived.

mero-swift is the native Swift SDK for Calimero 1.0 (Swift 5.9+, iOS 15+ /
macOS 12+), the Swift port of mero-js's `CalimeroClient`: attestation, the
sealed channel, passkeys, organizations, teams, app instances and live events.
See [README.md](README.md), [TESTING.md](TESTING.md) and its guides on core's docs
site (`docs/src/content/docs/clients/mero-swift/`, built with the rest of `docs/`).

## Names

| What | Name |
| --- | --- |
| SwiftPM package (`Package.swift` `name`) | `mero-swift` |
| Modules and products people import | unchanged: `MeroKit`, `MeroAPI`, `MeroKitUI`, `MeroKitTestSupport` (and the `MeroExample` executable) |
| SwiftPM URL | `https://github.com/calimero-network/core` (core's root `Package.swift`) |

Renaming the package and the pod changes no source: `import MeroKit` keeps
working. SwiftPM names a remote package by its URL, so consumers write
`.product(name: "MeroKit", package: "core")`; a path dependency on this
directory is named by the directory, `package: "mero-swift"`. Never rename a
module or product: that breaks every app that imports it.

## Layout

- `Sources/MeroKit/`: the client, sealed channel, TEE trust, passkeys and app layer.
- `Sources/MeroAPI/Generated/` and `Sources/MeroKit/Api/Generated/`: **generated, do not edit** (below).
- `Sources/MeroKitUI/`: SwiftUI session, platform passkeys, consent page.
- `Sources/MeroKitTestSupport/`: fakes (`FakeCloudNode`, `FakeCloud`, `MockURLProtocol`, ...).
- `Tests/`: `MeroKitTests` (unit and journey), `MeroKitUITests`, `MeroKitE2ETests` (need a node; skipped without `MERO_E2E_NODE_URL`).
- `examples/MeroSampleApp/`: the sample app (XcodeGen `project.yml`; the `.xcodeproj` is generated, not committed).
- `openapi/`: the generator's configuration (`openapi-generator-config.yaml`) and version (`generator`).
- `scripts/`: the generator, `refresh-receipt-vectors.sh`, and `dev-cluster.sh` (a `merod local` cluster on a Mac, at `ci/core-version`).
- `CHANGELOG.md`: the history from when this was its own repository. New entries go in core's release notes.

## Build, test, lint

What `.github/workflows/sdk-mero-swift.yml` runs (macOS), from this directory:

```bash
swift build
swift test
swiftlint lint --strict
swift-format lint --recursive --strict Sources Tests examples
cd examples/MeroSampleApp && xcodegen generate && \
  xcodebuild build-for-testing -project MeroSampleApp.xcodeproj -scheme MeroSampleApp \
    -destination 'generic/platform=iOS Simulator' CODE_SIGNING_ALLOWED=NO
```

The sample's XCUITest suite (simulator, against the in-app cluster,
`-uitest-mock`) runs when the workflow is called with `ui-tests: true`. The
live-node e2e suite (`Tests/MeroKitE2ETests`: the node API as mero-js's
cluster tests cover it, and the app layer, `CalimeroAppBackend`) runs in core's
`e2e.yml` (job `clients-swift`: macOS, the change's merod built there; a
skipped case fails it). By hand, against a `merod local` cluster, as TESTING.md shows
(`scripts/dev-cluster.sh up`, then `MERO_E2E_NODE_URL`, `MERO_E2E_NODES`,
`MERO_E2E_BUNDLE` and `MERO_E2E_MPK`).

## Generated code: core's OpenAPI document

`Sources/MeroAPI/Generated/` (`Types.swift`, `Client.swift`, by
swift-openapi-generator) and `Sources/MeroKit/Api/Generated/Facade.swift` (by
`scripts/generate-facade.py`) are generated from core's `docs/public/openapi.json`,
read in place (`../../../docs/public/openapi.json`, nothing fetched), and
checked in. They are `sdk.json`'s `generated`. Never hand-edit them.

- Regenerate: `python3 scripts/sdks.py generate mero-swift` at core's root
  (runs `scripts/generate-openapi.sh` here). The first run builds
  swift-openapi-generator (`openapi/generator`'s version) into
  `.build/openapi-generator`, which needs network once.
- `scripts/generate-openapi.sh check` fails when the checked-in output is stale.
- `Tests/MeroKitTests/Fixtures/receipts/receipt-vectors.json` is generated
  too: `scripts/refresh-receipt-vectors.sh` (part of `generate`) copies core's
  `crates/client/tests/fixtures/receipt-vectors.json`, which `ReceiptTests`
  checks receipt verification against; `refresh-receipt-vectors.sh check`
  fails when the copy differs. Never edit the copy.
- A wrong route or type is fixed in core's server (and so its document),
  `scripts/openapi-normalize.py`, `scripts/generate-facade.py` or
  `openapi/openapi-generator-config.yaml`, never in the output.

## Self-contained

This directory must build on its own (`swift build` here, with its own
`Package.swift`): nothing here may reference a file outside it. The one
exception is the generators' inputs, core's OpenAPI document and receipt
vectors; the generated output is committed, so building never needs them.

## Version

The SDK states core's `VERSION` in the install snippets of `README.md` and the docs quickstart (`exact:`).
They are `sdk.json`'s `version_files`; `python3 scripts/sdks.py version-sync`
writes them. Never bump them by hand. `ci/core-version` is not one: it is the
merod release `scripts/dev-cluster.sh` runs.

## Release: core's tag

There is nothing to upload. SwiftPM installs a package from the root of a git
repository, so core's root `Package.swift` declares this SDK's libraries with
their sources here (`path: "sdks/clients/mero-swift/Sources/..."`), and a core
release tag (`1.0.0-rc.N`, no `v`) is an SDK version. Apps clone all of core
to install it.

That manifest lists the libraries only: tests and the example stay in this
directory's `Package.swift`, which is the one to develop with. Change both
together: `sdk-mero-swift.yml`'s `Root Package.swift matches` step fails when
their library products, targets, dependencies or platforms differ.

There is no CocoaPods pod. The trunk turns read-only on 2026-12-02, and MeroKit
depends on swift-openapi-runtime and swift-http-types, which were never on it;
the last pod is swift-sdk's 0.7.0. SwiftPM is the only distribution.
Releases before 1.0.0-rc.32 were tagged in calimero-network/swift-sdk, now
archived.

## Core's App ABI check against this directory

`.github/workflows/app-abi-native-clients.yml` (`scripts/check-app-abi-clients.py
swift`) compiles the Swift clients `meroctl app codegen` writes against a
released swift-sdk (`SWIFT_SDK_VERSION`). To check against this directory
instead, its generated `Package.swift` needs:

- `.package(path: "<core checkout>/sdks/clients/mero-swift")` in place of the
  `url:`/`exact:` dependency (an absolute path, as the package is written to a
  temporary directory);
- `.product(name: "MeroKit", package: "mero-swift")`: a path dependency is
  named by its directory, not `swift-sdk`;
- `sdks/clients/mero-swift/**` (and `docs/public/openapi.json`, which reaches
  the generated layer) among the workflow's `paths`, and a macOS runner with
  network access for swift-openapi-runtime and swift-http-types.
