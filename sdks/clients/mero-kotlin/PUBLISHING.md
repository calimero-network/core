# Publishing & releasing

There's no "npm" for Android — the package registry is **Maven**. Artifacts are addressed by
`group:artifact:version` coordinates (e.g. `network.calimero:mero-kotlin:1.0.0-rc.30`) and resolved
by Gradle from a Maven repository. The public target is **Maven Central**: it resolves through the
`mavenCentral()` every Android project already declares, with no credentials on the consumer side.

## Coordinates

| Artifact | Module | Was (calimero-network/kotlin-sdk) |
|---|---|---|
| `network.calimero:mero-kotlin` | `mero-core/` | `network.calimero:mero-core` |
| `network.calimero:mero-kotlin-compose` | `mero-compose/` | `network.calimero:mero-compose` |
| `network.calimero:mero-kotlin-testkit` | `mero-testkit/` | not published |

`network.calimero`, not `com.calimero.*`. Central makes you prove you own the namespace, and
`com.calimero` would mean owning **calimero.com** — this is the reverse-DNS of calimero.network,
a domain we do own, verified with a TXT record.

The artifact names follow core's `mero-<language>` rule; the Kotlin packages
(`com.calimero.mero`, ...) did not change, so moving to the new coordinates changes no import.

## Version

The version is core's: the root `VERSION` of calimero-network/core, written into
`gradle.properties` `VERSION_NAME` by `python3 scripts/sdks.py version-sync` (from core's root).
CI's `SDK scope` job fails while they differ. Never bump it on its own: the SDK releases only with
core.

## How a release happens

A core tag `X.Y.Z[-rc.N]` releases core and every SDK from that commit. Core's `release.yml` calls
`.github/workflows/sdk-mero-kotlin.yml` with `publish: true` and `dry-run: false`, which:

1. builds the three modules and runs `./gradlew nmcpPublishAggregationToCentralPortal`: one signed
   bundle to **Maven Central**, published by the Portal as soon as it validates
   (`publishingType = "AUTOMATIC"`);
2. needs the Central secrets (`MAVEN_CENTRAL_USERNAME`, `MAVEN_CENTRAL_PASSWORD`, `SIGNING_KEY`,
   `SIGNING_PASSWORD`) in the `maven-central` environment; without them the step is skipped with a
   warning.

**A Central version can never be replaced, deleted or re-published.** Core's
[RELEASING.md](../../../RELEASING.md) has the rules for what releases and when.

With `dry-run: true` (the default) the publish path builds the same publications and puts them in
the runner's Maven local repository only: nothing leaves the runner.

`CHANGELOG.md` keeps the history the SDK had as its own repository; new entries go in core's
release notes.

### One-time setup before the first Central release from core

1. The `network.calimero` namespace is registered and verified on the Central Portal (done for
   kotlin-sdk; it carries over).
2. A GPG key, its public half on a keyserver, the private half exported ASCII-armored
   (`gpg --armor --export-secret-keys <id>`).
3. Four secrets on core's `maven-central` environment:

   | secret | what |
   |---|---|
   | `MAVEN_CENTRAL_USERNAME` | Portal user token name |
   | `MAVEN_CENTRAL_PASSWORD` | Portal user token password |
   | `SIGNING_KEY` | the ASCII-armored private key |
   | `SIGNING_PASSWORD` | its passphrase (`''` if none) |

Signing is opted into only when a key is present, so `publishToMavenLocal` works on a machine
with no GPG key.

## Consuming from Maven Central

No repository block, no credentials — `mavenCentral()` is already there:

```kotlin
dependencies {
    implementation("network.calimero:mero-kotlin:1.0.0-rc.30")
    implementation("network.calimero:mero-kotlin-compose:1.0.0-rc.30") // optional UI kit
    testImplementation("network.calimero:mero-kotlin-testkit:1.0.0-rc.30")
}
```

## Publishing locally (smoke test)

```bash
./gradlew publishToMavenLocal
# artifacts land in ~/.m2/repository/network/calimero/mero-kotlin*/<VERSION_NAME>/
```

To check the **actual Central bundle** — coordinates, javadoc jar, signatures, checksums — without
any credentials, which is what `sdk-mero-kotlin.yml` does on every pull request:

```bash
ORG_GRADLE_PROJECT_signingInMemoryKey="$(gpg --armor --export-secret-keys <throwaway-id>)" \
  ./gradlew nmcpZipAggregation
unzip -l build/nmcp/zip/aggregation.zip
```

`nmcpCheckAggregationFiles` runs as part of that and validates the bundle against Central's rules,
so a broken release config fails on the pull request rather than on a tag that cannot be undone.

The modules also declare a `GitHubPackages` repository (`calimero-network/core`) for an internal
build a human publishes by hand (`./gradlew publish` with `GITHUB_ACTOR` / `GITHUB_TOKEN`); no
workflow publishes there.
