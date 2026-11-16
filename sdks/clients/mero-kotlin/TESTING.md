# TESTING — mero-kotlin

Run every command from this directory (`sdks/clients/mero-kotlin`). You need **JDK 17** and the **Android SDK**
(`ANDROID_HOME`, or `sdk.dir` in `local.properties`).

## 1. What CI runs (core's `.github/workflows/sdk-mero-kotlin.yml`)

```bash
./gradlew ktlintCheck detekt
./gradlew testDebugUnitTest
./gradlew lint
./gradlew assembleDebug
./gradlew :sample-app:assembleDebugAndroidTest   # instrumented tests must compile
./ci/check-registry-parity.sh                     # every public CalimeroClient method has an SDK-tab entry
```

Whether the generated client matches core's `docs/public/openapi.json` is core's `SDK codegen
drift` job (`python3 scripts/sdks.py generate mero-kotlin` from core's root, then `git status`).

The unit tests need no network and no node:

| Suite | What |
|---|---|
| `mero-core` `tee/CalimeroClientTest` | `CalimeroClient` against stand-in cloud nodes, through the real Noise NK sealed channel with a mock attestation: mock quotes refused unless allowed, intents/views, 503 retries with one idempotency key, single-flight refresh on 401, 421 failover, failover of a call whose node stays unavailable (one idempotency key across nodes), `connect` through the node directory, 412 step-ups on invites/members/API keys (and the app layer without a passkey), blob content types and grants, deployments/deployers/xcalls routes, collateral age, sealed SSE, consent, the app backend's mapping, `SoftPasskey` signatures. |
| `mero-core` `tee/*` | The sealed transport's wire format, DCAP, Sigstore releases, PCCS collateral, against pinned vectors. |
| `mero-core` `app/*` | `ContextContract`, `InMemoryAppBackend`. |
| `mero-core` `agentgate/*` | The agent gate client. |
| `sample-app` `ChatServiceTest` | The chat on `InMemoryAppBackend`. |

## 2. Instrumented UI tests (an emulator; `sdk-mero-kotlin.yml` with `instrumented: true`)

```bash
./gradlew :sample-app:connectedDebugAndroidTest
```

`LoginFlowTest` checks the sign-in screen's Cloud and developer paths, then runs the in-app demo
(no network) through Home, Chat and sign-out.

## 3. Cluster tests: against a real `merod local`

`mero-core/src/test/kotlin/com/calimero/mero/cluster/` runs the client against a real merod 1.0
cluster (three nodes, mock attestation), the Kotlin twin of mero-js's `tests/cluster/`: sign-up and
sign-in on other nodes, the sealed channel, calls with idempotency keys, views, events (resumed by
their `<index>:<seq>` ids), `412` step-ups, API keys, sessions, organizations, teams and their
deletion, invites, blobs, presence, transfer, packages signed here (`Publisher.kt`), upgrade runs,
moving an app, package origins, the consent page, account recovery, and the app layer
(`CalimeroAppBackend`). They are skipped unless a cluster is named:

```bash
MEROD_BIN=path/to/merod MERO_KV_MPK=path/to/kv-store-test-fixture.mpk scripts/cluster-test.sh
```

starts `merod local` on 3501-3503 (`MEROD_BASE_PORT`), runs them and stops it; with
`MERO_CLUSTER=http://127.0.0.1:3501,...` set it uses that running cluster instead. `MERO_KV_MPK`
is core's kv-store test fixture, whose `app.wasm` the tests deploy. Each run names its packages
anew, so one cluster takes several runs.

## 4. By hand against `merod local`

```bash
scripts/dev-cluster.sh up          # merod local: three nodes on 127.0.0.1:2501-2503, mock attestation
./gradlew :sample-app:installDebug
```

With `MEROD_BIN` set to a merod 1.0 binary (core's release ships one for Linux and Apple silicon),
[`scripts/dev-cluster.sh`](scripts/dev-cluster.sh) runs it natively; `MEROD_BASE_PORT` moves the
nodes off 2501-2503. Without it the script runs core's `ghcr.io/calimero-network/merod` image (the
release in `ci/core-version`) in Docker and publishes the nodes on the host: `merod local` listens on
127.0.0.1 inside the container, so a forwarder in the same network namespace publishes it. `up --fresh` starts from empty state, `down` stops it, and
`install app.mpk` installs an app for the sample to open.

In the app: **Developer node (merod local)** → `http://10.0.2.2:2501` (the emulator's alias for
the host), a name, *Allow mock attestation* → Connect. On a phone over USB, run
`scripts/dev-cluster.sh android` (`adb reverse` for 2501-2503) and use `http://localhost:2501`.
The first time it signs up with a software passkey kept on the device; after that it signs in
with it.

A real cluster needs a passkey the cluster's relying-party domain allows for this app
(`assetlinks.json`); use the cluster URL from the Calimero Cloud console.
