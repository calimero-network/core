# swift-sdk (MeroKit) - agent guide

> **Before committing, titling a PR, or releasing, read [RELEASING.md](RELEASING.md).**
> PRs are squash-merged and the PR title is the release commit: its type decides
> whether this repository releases, and which version (the table in RELEASING.md).

MeroKit is the native Swift SDK for Calimero 1.0 (Swift 5.9+, iOS 15+ / macOS 12+),
the Swift port of mero-js's `CalimeroClient`: attestation, the sealed channel,
passkeys, organizations, teams, app instances and live events. It ships as a
Swift package (`Package.swift`) and a CocoaPods spec (`MeroKit.podspec`). See
[README.md](README.md), [TESTING.md](TESTING.md) and the docs site, whose source is `docs/`.

## Layout

- `Sources/MeroKit/`: the client, sealed channel, TEE trust, passkeys and app layer.
- `Sources/MeroAPI/Generated/` and `Sources/MeroKit/Api/Generated/`: **generated, do not edit** (see below).
- `Sources/MeroKitUI/`: SwiftUI session, platform passkeys, consent page.
- `Sources/MeroKitTestSupport/`: fakes (`FakeCloudNode`, `FakeCloud`, `MockURLProtocol`, ...).
- `Tests/`: `MeroKitTests` (unit and journey), `MeroKitUITests`, `MeroKitE2ETests` (need a node).
- `Examples/MeroSampleApp/`: the sample app `ui.yml` tests in the simulator.
- `ci/core-version`: the core release the SDK targets.

## Build, test, lint

These are the steps `ci.yml` runs (macOS):

```bash
swift build -v
swift test -v
swiftlint lint --strict
swift-format lint --recursive --strict Sources Tests Examples
scripts/generate-openapi.sh check      # the generated sources are current
```

Against a development cluster: `scripts/dev-cluster.sh up`, then
`MERO_E2E_NODE_URL=http://127.0.0.1:2501 swift test --filter MeroKitE2ETests`
(see [TESTING.md](TESTING.md)). The docs site (`docs/`, npm) is checked with
`npm ci && npm run check` in `docs/`.

## Generated code: core's OpenAPI document

`Sources/MeroAPI/Generated/` (`Types.swift`, `Client.swift`, by
swift-openapi-generator) and `Sources/MeroKit/Api/Generated/Facade.swift` (by
`scripts/generate-facade.py`) are generated from core's OpenAPI document, vendored
at `openapi/openapi.json` (the core commit is in `openapi/source`), and checked
in. Never hand-edit them; CI's `generated` job fails on any drift.

- Regenerate: `scripts/generate-openapi.sh`.
- Take a newer document from core: `scripts/generate-openapi.sh refresh [<core ref>]`, which also regenerates.
- A wrong route or type is fixed in core's document, `scripts/openapi-normalize.py`,
  `scripts/generate-facade.py` or `openapi/openapi-generator-config.yaml`, not in
  the output. `openapi-sync.yml` opens the sync PRs from core on its own.
