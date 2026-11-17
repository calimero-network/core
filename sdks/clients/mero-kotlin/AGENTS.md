# mero-kotlin - agent guide

mero-kotlin is the Android SDK for Calimero 1.0, the twin of mero-swift and the
Android counterpart of mero-js's `CalimeroClient`: attestation, the sealed channel,
passkeys, and app intents and views. It lives in core at `sdks/clients/mero-kotlin`
(it was the `calimero-network/kotlin-sdk` repository, imported with its history)
and ships with core, on core's version.

Read first: [sdks/AGENTS.md](../../AGENTS.md) (the rules every SDK here follows)
and core's [RELEASING.md](../../../RELEASING.md). Then [README.md](README.md),
[TESTING.md](TESTING.md), [PUBLISHING.md](PUBLISHING.md) and
[codegen/README.md](codegen/README.md). The docs site's source is `docs/`.

## Layout

Gradle (Kotlin DSL), `rootProject.name = "mero-kotlin"` (`settings.gradle.kts`):

| Directory | Gradle project | Published as (`network.calimero:`) |
|---|---|---|
| `mero-core/` | `:mero-core` | `mero-kotlin` |
| `mero-compose/` | `:mero-compose` | `mero-kotlin-compose` |
| `mero-testkit/` | `:mero-testkit` | `mero-kotlin-testkit` |
| `examples/sample-app/` | `:sample-app` | not published (the Mero Chat sample) |

- `mero-core/`: `CalimeroClient`, attestation, sealed transport, passkeys, sealed files
  (`Files.kt`, `tee/SealedFile.kt`), pipeline receipts (`Receipts.kt`, `crypto/Jcs.kt`), the app layer.
  `mero-core/src/generated/kotlin/` is **generated, do not edit** (see below).
- `mero-compose/`: Compose session, platform passkeys, consent launcher, theme.
- `mero-testkit/`: in-memory app backend for tests with no cluster.
- `examples/sample-app/`: the Mero Chat sample (`:sample-app`).

Also: `codegen/` (a standalone Gradle build that runs the generator),
`ci/check-registry-parity.sh` (every public `CalimeroClient` method has an SDK-tab entry
in the sample), `ci/core-version` (the merod image `scripts/dev-cluster.sh` runs), `docs/`
(the Starlight docs site).

### Names: artifacts renamed, packages and directories not

The artifacts follow core's `mero-<language>` rule. Up to its last release as kotlin-sdk
the SDK was `network.calimero:mero-core` and `network.calimero:mero-compose`
(`mero-testkit` was unpublished); now they are `network.calimero:mero-kotlin`,
`network.calimero:mero-kotlin-compose` and `network.calimero:mero-kotlin-testkit`.
The Kotlin packages (`com.calimero.mero`, `com.calimero.mero.compose`,
`com.calimero.mero.testkit`) are unchanged, so no import changes. The module
directories and Gradle project paths (`:mero-core`, ...) keep their old names on
purpose: renaming them would move every source path and break tools that name
`:mero-core` (core's `scripts/check-app-abi-clients.py`), for no change a consumer sees.

## Version

`VERSION_NAME` in `gradle.properties` is core's root `VERSION` (`sdk.json`
`version_files`). `python3 scripts/sdks.py version-sync` writes it; never edit it alone.

## Build, test, lint

JDK 17 and the Android SDK (`ANDROID_HOME`, or `sdk.dir` in `local.properties`, which
is git-ignored). From this directory, what `.github/workflows/sdk-mero-kotlin.yml` runs:

```bash
./gradlew ktlintCheck detekt
./gradlew testDebugUnitTest
./gradlew lint
./gradlew assembleDebug                           # includes the sample app
./gradlew :sample-app:assembleDebugAndroidTest    # instrumented tests must compile
./gradlew nmcpZipAggregation                      # the Central bundle (with a throwaway signing key in CI)
./ci/check-registry-parity.sh                     # every public CalimeroClient method has an SDK-tab entry
```

`./gradlew build` runs all of the Gradle ones but the bundle. The cluster tests (`mero-core`'s
`cluster` package) run the client against a real `merod local` and are skipped unless one is named:
`MEROD_BIN=… MERO_KV_MPK=… scripts/cluster-test.sh` ([TESTING.md](TESTING.md)). Core's `e2e.yml`
runs them (job `clients`, mero-kotlin) against the merod of the change. The instrumented UI tests
(`:sample-app:connectedDebugAndroidTest`, an emulator) run only when the workflow is
called with `instrumented: true`; see [TESTING.md](TESTING.md).

## Generated code: core's OpenAPI document

`mero-core/src/generated/kotlin/` (the OpenAPI client, plus the facade
`com/calimero/mero/api/facade/*.kt` by `codegen/facade.py`) is generated from core's
`docs/public/openapi.json`, read in place from this tree (`../../../docs/public/openapi.json`),
and checked in so the Android build needs neither the generator nor the network.

- Regenerate: `python3 scripts/sdks.py generate mero-kotlin` from core's root, which runs
  `./gradlew -p codegen generateMeroClient` here, then `scripts/refresh-receipt-vectors.sh`
  (`sdk.json` `generate`; it owns exactly `mero-core/src/generated/kotlin` and
  `mero-core/src/test/resources/fixtures/receipt-vectors.json`, `sdk.json` `generated`).
- `fixtures/receipt-vectors.json` is a copy of core's `crates/client/tests/fixtures/receipt-vectors.json`
  (an `inputs` file), which `ReceiptsTest` checks receipt verification against: never edit it by hand.
- A wrong route or type is fixed in core's document or in `codegen/`
  (`prepare-spec.py`, `facade.py`, `templates/`), never in the output.
- Core's `SDK codegen drift` job reports a stale client; `sdk-regenerate.yml` opens the
  regeneration pull request after an API change merges.

## Release

Core's `release.yml` publishes it from a core tag through `sdk-mero-kotlin.yml`'s
`publish` path: one signed bundle of the three artifacts to Maven Central, which can never
be taken back. Agents never publish; `publishToMavenLocal` is the local check.
[PUBLISHING.md](PUBLISHING.md) has the details. `CHANGELOG.md` keeps the kotlin-sdk
history; new entries go in core's release notes.

## Core's app-abi-native-clients check

`.github/workflows/app-abi-native-clients.yml` runs `scripts/check-app-abi-clients.py kotlin`,
which still clones `calimero-network/kotlin-sdk` at `KOTLIN_SDK_REF` (`v4.0.0`), appends two
modules (the `meroctl app codegen` goldens and their round-trip tests) to its
`settings.gradle.kts`, each `implementation(project(":mero-core"))` with the catalog's
`libs.plugins.*`, and runs `:<module>:compileReleaseKotlin` and `:<module>:testReleaseUnitTest`.
To check against this folder instead:

- copy `sdks/clients/mero-kotlin` to the script's work directory instead of cloning
  (without `build/`, `.gradle/` and `local.properties`), so appending to `settings.gradle.kts`
  never dirties the tree; nothing in the main build reads outside this directory (only
  `codegen/`, a separate build, reads `../../../docs/public/openapi.json`);
- keep depending on `project(":mero-core")`: the project path did not change, only the
  published artifact name;
- drop `KOTLIN_SDK_REF` and its clone, and add `sdks/clients/mero-kotlin/**` to that
  workflow's triggers so an SDK change reruns it;
- JDK 17 and an Android SDK, as now.
