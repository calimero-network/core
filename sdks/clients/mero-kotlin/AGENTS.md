# kotlin-sdk (mero-kotlin) - agent guide

> **Before committing, titling a PR, or releasing, read [RELEASING.md](RELEASING.md).**
> PRs are squash-merged and the PR title is the release commit: its type decides
> whether this repository releases, and which version (the table in RELEASING.md). A release
> here goes to Maven Central, where it can never be removed.

mero-kotlin is the Android SDK for Calimero 1.0, the twin of swift-sdk and the
Android counterpart of mero-js's `CalimeroClient`: attestation, the sealed channel,
passkeys, and app intents and views. See [README.md](README.md),
[TESTING.md](TESTING.md), [PUBLISHING.md](PUBLISHING.md) and the docs site, whose source is `docs/`.

## Layout

Gradle modules (`settings.gradle.kts`):

- `mero-core/`: `CalimeroClient`, attestation, sealed transport, passkeys, the app layer.
  `mero-core/src/generated/kotlin/` is **generated, do not edit** (see below).
- `mero-compose/`: Compose session, platform passkeys, consent launcher, theme.
- `mero-testkit/`: in-memory app backend for tests with no cluster.
- `sample-app/`: the Mero Chat sample.

Also: `codegen/` (a standalone Gradle build that runs the generator), `openapi/`
(the vendored document), `ci/core-version` (the core release the SDK targets).

## Build, test, lint

JDK 17 and the Android SDK (`ANDROID_HOME`, or `sdk.dir` in `local.properties`).
These are the steps `ci.yml` runs, from the repo root:

```bash
scripts/refresh-openapi.sh --check                # the generated client is current
./gradlew ktlintCheck detekt
./gradlew testDebugUnitTest
./gradlew lint
./gradlew assembleDebug
./gradlew :sample-app:assembleDebugAndroidTest    # instrumented tests must compile
./ci/check-registry-parity.sh                     # every public CalimeroClient method has an SDK-tab entry
```

Instrumented and cluster tests are described in [TESTING.md](TESTING.md).

## Generated code: core's OpenAPI document

`mero-core/src/generated/kotlin/` (the OpenAPI client, plus the facade
`com/calimero/mero/api/facade/*.kt` by `codegen/facade.py`) is generated from
core's OpenAPI document, vendored at `openapi/openapi.json` (the core commit is in
`openapi/SOURCE`), and checked in so the Android build needs neither the generator
nor the network. Never hand-edit it; CI's `--check` fails on any drift.

- Regenerate from the vendored document: `./gradlew -p codegen generateMeroClient`
  (`scripts/refresh-openapi.sh --check` does the same and fails on a diff).
- Take a newer document from core: `scripts/refresh-openapi.sh [<core ref>]`
  (default `master`), which records it in `openapi/SOURCE` and regenerates.
- A wrong route or type is fixed in core's document or in `codegen/`
  (`prepare-spec.py`, `facade.py`, `templates/`), not in the output.
  `openapi-sync.yml` opens the sync PRs from core on its own. See [openapi/README.md](openapi/README.md).
