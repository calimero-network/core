# Publishing & releasing

There's no "npm" for Android — the package registry is **Maven**. Artifacts are addressed by
`group:artifact:version` coordinates (e.g. `network.calimero:mero-core:0.1.0`) and resolved by
Gradle from a Maven repository. This repo publishes to two targets, both wired:

| Target | Analogous to | Consumer auth | Role |
|--------|--------------|---------------|------|
| **Maven Central** | the public npm registry | **none** | the public target |
| **GitHub Packages** | a private/org npm registry | a PAT, even for public packages | pre-releases, internal builds |

Central is the one to point people at: it resolves through the `mavenCentral()` every Android
project already declares. GitHub Packages needs no signing key and no Portal account, which is
what keeps it useful for an internal or pre-release build.

## Coordinates

`network.calimero`, not `com.calimero.*`. Central makes you prove you own the namespace, and
`com.calimero` would mean owning **calimero.com** — this is the reverse-DNS of calimero.network,
a domain we do own, verified with a TXT record. Settled before the first public release on
purpose: moving coordinates afterwards is a breaking change for every consumer.

## Testing before release

Before tagging, make sure the suite is green — see [TESTING.md](TESTING.md). At a
minimum run what CI enforces:

```bash
./gradlew ktlintCheck detekt testDebugUnitTest lint assembleDebug   # ci.yml
```

## How a release happens

[RELEASING.md](RELEASING.md) has the rules for titles and breaking changes that pick
the version, shared with every Calimero repository that ships releases.

On merge, as in the JS repos. Every push to `main` runs `.github/workflows/release-on-merge.yml`,
which runs semantic-release (`.releaserc.json`) over the conventional commits since the last
`v*` tag:

| Commit | Release |
|---|---|
| `feat!:` / `BREAKING-CHANGE:` footer | major |
| `feat:` | minor |
| `fix:`, `perf:` | patch |
| `docs:`, `chore:`, `refactor:`, `style:`, `test:`, `build:`, `ci:`, `revert:` | none |

Only the hyphenated `BREAKING-CHANGE:` footer (or `!`) makes a major; prose that happens to start a
line with "breaking change" does not (see the comment in `.releaserc.json`).

When the commits call for a release, it pushes the tag `vX.Y.Z` and creates the GitHub Release with
notes generated from those commits. The tag then runs `.github/workflows/release.yml`:
1. `./gradlew publish` → `mero-core` and `mero-compose` to **GitHub Packages**,
2. `./gradlew nmcpPublishAggregationToCentralPortal` → both modules as one signed bundle to
   **Maven Central**, published as soon as the Portal validates it,
3. the GitHub Release, when release-on-merge did not create it (a tag pushed by hand).

The tag is pushed with a GitHub App token (`vars.ACTIONS_APP_ID` / `secrets.ACTIONS_APP_PRIVATE_KEY`,
scoped to this repository and `contents: write`): a tag pushed with `GITHUB_TOKEN` does not start
`release.yml`. The app must be installed on this repository.

`VERSION_NAME` in `gradle.properties` is only the default for local and dev builds; the tag
overrides it, so a release commits nothing back to `main`. A tag pushed by hand
(`git tag v1.2.3 && git push origin v1.2.3`) still releases through `release.yml`.

### CHANGELOG.md

`CHANGELOG.md` stays hand-written: a pull request that changes what the SDK does adds its entry
under `## Unreleased`, in prose, as it always has. The per-version list of commits is the GitHub
Release that semantic-release writes. Nothing rewrites the file on release; rename `## Unreleased`
to the version in a later pull request if a version heading is wanted.

### Maven Central publishes by itself

`publishingType` is `AUTOMATIC`: the bundle is uploaded, validated and published with no click in
the [Central Portal](https://central.sonatype.com/). **A Central version can never be replaced,
deleted or re-published**, so a merge that semantic-release tags is a permanent release. A commit
that must not release carries a type that releases nothing (`ci:`, `test:`, `build:`).

### One-time setup before the first Central release

1. Register the `network.calimero` namespace on the Central Portal and verify it with the DNS
   TXT record it gives you.
2. Generate a GPG key, publish the public half to a keyserver, and export the private half
   ASCII-armored (`gpg --armor --export-secret-keys <id>`).
3. Add four repo secrets:

   | secret | what |
   |---|---|
   | `MAVEN_CENTRAL_USERNAME` | Portal user token name |
   | `MAVEN_CENTRAL_PASSWORD` | Portal user token password |
   | `SIGNING_KEY` | the ASCII-armored private key |
   | `SIGNING_PASSWORD` | its passphrase (`''` if none) |

Signing is opted into only when a key is present, so `publishToMavenLocal` and a GitHub Packages
publish still work on a machine with no GPG key.

## Consuming from Maven Central

No repository block, no credentials — `mavenCentral()` is already there:

```kotlin
dependencies {
    implementation("network.calimero:mero-core:0.1.0")
    implementation("network.calimero:mero-compose:0.1.0") // optional UI kit
}
```


## Consuming from GitHub Packages (pre-releases, internal builds)

GitHub Packages requires auth even for public packages. In the consumer project:

```kotlin
// settings.gradle.kts
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
        maven {
            url = uri("https://maven.pkg.github.com/calimero-network/kotlin-sdk")
            credentials {
                username = providers.gradleProperty("gpr.user").orNull ?: System.getenv("GITHUB_ACTOR")
                password = providers.gradleProperty("gpr.token").orNull ?: System.getenv("GITHUB_TOKEN")
            }
        }
    }
}
```

```kotlin
// build.gradle.kts
dependencies {
    implementation("network.calimero:mero-core:0.1.0")
    implementation("network.calimero:mero-compose:0.1.0") // optional UI kit
}
```

Put a personal access token (scope `read:packages`) in `~/.gradle/gradle.properties` as
`gpr.user` / `gpr.token`.

## Publishing locally (smoke test)

```bash
./gradlew publishToMavenLocal -PVERSION_NAME=0.1.0-local
# artifacts land in ~/.m2/repository/network/calimero/
```

To check the **actual Central bundle** — coordinates, javadoc jar, signatures, checksums — without
any credentials, which is what `ci.yml` does on every pull request:

```bash
ORG_GRADLE_PROJECT_signingInMemoryKey="$(gpg --armor --export-secret-keys <throwaway-id>)" \
  ./gradlew nmcpZipAggregation -PVERSION_NAME=0.0.0-local
unzip -l build/nmcp/zip/aggregation.zip
```

`nmcpCheckAggregationFiles` runs as part of that and validates the bundle against Central's rules,
so a broken release config fails on the pull request rather than on a tag that cannot be undone.

## What ships

`mero-core` and `mero-compose` only. `mero-testkit` applies `maven-publish` but registers no
publication, so it is not published — it exists for this repo's own tests. Register a publication
for it if consumers need `InMemoryAppBackend` from Maven.

Version-align releases to the mero-js contract they implement: this SDK speaks Calimero 1.0, as
mero-js ≥ 27's `CalimeroClient` does; a release that drops the 0.x API is a major version.
