# mero-kotlin — Calimero Android SDK

**Your app's whole backend, like Firebase. Except nobody can read your data, not even us.**
[Calimero](https://calimero-network.github.io/core/) gives an app auth (passkeys, sessions, scoped
API keys), a database (replicated app state), functions (Rust app methods), realtime events,
storage, hosting and typed clients, all running inside attested Intel TDX hardware. Clients verify
the node's attestation against Sigstore-signed releases before they send it anything.

mero-kotlin is the Android client for **Calimero 1.0**. It gives an Android app what
[`@calimero-network/mero-js`](https://github.com/calimero-network/mero-js) ≥ 27 gives a web app,
with the same names: `CalimeroClient` verifies a node's TDX attestation, seals every request to it
(`calimero/sealed-http/v2`, Noise NK), signs people in with **passkeys** (Android Credential
Manager) and machines with API keys, manages **organizations and teams**, calls apps' **intents**
and **views**, and streams their **events** as a `Flow`. `mero-compose` adds the signed-in app
session, the consent page in a Custom Tab and a theme; `mero-testkit` an in-memory backend for
tests. It is the Android twin of [`calimero-network/swift-sdk`](https://github.com/calimero-network/swift-sdk).

> **Calimero 1.0 only.** The 0.x API (`/admin-api`, `/jsonrpc`, `/ws`, `/sse`, `/auth/token`,
> relays, delegated execution, `/api/cloud/*`) is gone from core, and from this SDK. See
> [Migrating from 0.x](https://calimero-network.github.io/kotlin-sdk/reference/migrating-from-0x/).

> **Before committing, titling a pull request, or releasing**, read [RELEASING.md](RELEASING.md): the PR title is the release commit, and the rules are shared by every Calimero repository that ships releases.

## Modules

| Module | What it is |
|---|---|
| `mero-core` | `CalimeroClient` and its typed errors (`AppError`, `ConfirmRequired`, `WrongNode`, `LoginRequired`, `NotFound`, `UnavailableError`, …); attestation (`CloudPolicy`, `fetchAttestedKey`: mock quotes only when allowed, real quotes checked against Sigstore-signed mero-tee releases and Intel's DCAP collateral on the device); the sealed transport (`tee.SealedTransport`); passkeys (`PasskeyProvider`, `SoftPasskey`); approvals and the consent page; `SessionStore`; and the app-facing layer in `app` (`MeroAppBackend`, `ContextContract`, `CalimeroAppBackend`, `OrgInvitation`). Also the mero-agent-gate client (`agentgate`). |
| `mero-compose` | `MeroAppSession` (a mobile app's signed-in account over `CalimeroAppBackend`), `PlatformPasskey` (Android Credential Manager), `ConsentLauncher` (the node's consent page in a Custom Tab, and `resume` for the App Link it returns to), `MeroTheme`. |
| `mero-testkit` | `InMemoryAppBackend` and `RecordingContextContract`: test an app's view models with no cluster. |
| `sample-app` | **Mero Chat** sample in the light Calimero style: Home / Chat / SDK tabs. |

### The sample app

- **Sign in**: a passkey on a Calimero Cloud cluster (enter the cluster URL from the console;
  sign in, or create an account), or **Developer node** for `merod local`
  (`http://10.0.2.2:2501` from the emulator) with *Allow mock attestation* and a software passkey
  kept (encrypted) on the device. Debug builds also offer an in-app demo on `InMemoryAppBackend`.
- **Home**: the account and node, your **organizations and teams** (create one with a `general`
  team, copy an invite link, redeem one), and **approvals** (node releases your scope has not
  approved), which open the node's consent page in a Custom Tab; it returns to
  `mero-sample://consent`.
- **Chat**: mero-chat (`com.calimero.chat`) on the app-facing layer: spaces are organizations,
  channels are app instances, messages are views and calls, live updates are the app's events.
- **SDK**: every public `CalimeroClient` method as a form (`ci/check-registry-parity.sh` keeps
  that true).

## Install

```kotlin
dependencies {
    implementation("com.calimero.mero:mero-core:<version>")
    implementation("com.calimero.mero:mero-compose:<version>")  // MeroAppSession, passkeys, consent
    testImplementation("com.calimero.mero:mero-testkit:<version>")
}
```

Kotlin 2.x, coroutines and `Flow`; minSdk 24, compileSdk 34. See [PUBLISHING.md](PUBLISHING.md).

## Quick start

```kotlin
val client = CalimeroClient(
    CalimeroClientOptions(
        baseUrl = clusterUrl,
        nodes = otherNodes,                    // failover on 421 WRONG_NODE and on a node that stays down
        policy = CloudPolicy.production,       // real TDX quotes, mero-tee >= 2.3.109
        onSession = store::save,               // keep the newest session: refresh tokens are single-use
    ),
)
client.signIn(PlatformPasskey(activity))

val org = client.orgs.create(name = "Acme").orgId
val app = client.apps.create(name = "general", `package` = "com.calimero.chat", org = org, initArgs = args).appId
client.apps.intents(app = app, method = "send_message", args = message)   // retried on 503 with one idempotency key
client.apps.views(app = app, method = "get_messages", args = page).result
client.events(app).collect { /* sealed SSE */ }
```

Every route of core's node API is also a method of the facade generated from core's OpenAPI document
(`openapi/`): one suspend method per operation, named parameters, the typed answer. A `412` step-up is
approved with the client's `passkey` (or the call's), a `428` signed with its `operator`:

```kotlin
client.passkey = PlatformPasskey(activity)
val token = client.orgs.inviteCreate(org = org, role = OrgRole.MEMBER, teams = listOf(team)).token
val info = client.orgs.get(org = org)
val apps = client.apps.list().apps
```

1.0 removed the one-route methods that 0.x deprecated (`client.invite(...)`, `client.getOrg(...)`, …);
CHANGELOG.md maps each one to its facade method.

By cluster name instead of a node URL: `CalimeroClient.connect(CalimeroConnectOptions("eu-shared-1",
CloudPolicy.production))` reads the nodes from the Calimero Cloud node directory and fails over
between them.

For `merod local`: `CloudPolicy.development` (`allowMock = true`) and a `SoftPasskey("localhost",
"http://localhost:3000")`.

### A mobile app's session

```kotlin
val session = MeroAppSession.create(app, "com.example.board", callbackUrl = "board://consent",
    config = MeroAppConfig(nodeUrl = clusterUrl), scope = viewModelScope)
viewModelScope.launch { session.restore() }
session.signIn(activity)                              // the passkey sheet
session.passkey = PlatformPasskey(activity)           // approves invites, member changes, leaving (412 step-ups)
session.backend.collect { backend -> /* MeroAppBackend: orgs, teams, app instances, contract */ }
```

`MeroAppBackend` keeps the 0.x method names apps were written against (`listNamespaces`,
`createSubgroup`, `createContext`, `joinNamespace`, …), mapped onto organizations, teams and app
instances; see the [App session guide](https://calimero-network.github.io/kotlin-sdk/guides/app-session/).

### Consent: what an app's session may not do itself

Approvals, creating an organization, and minting or accepting an invitation are decided by the
account holder on the node's own consent page:

```kotlin
val returnTo = consentReturnTo(packageOrigin, version)   // the package's served page relays to the app
val started = ConsentLauncher.request(activity, client, ConsentRequest.CreateOrg("Acme", returnTo)) { s ->
    showCode(s.code)                                     // the page shows the same code
    true
}
// Back in the app: this request's decision, read from the node (or client.waitForConsent(id, timeout)).
// A return that is not it throws (CONSENT_NOT_DECIDED, CONSENT_REPLACED, CONSENT_REQUEST_MISMATCH, …).
val result = ConsentLauncher.resume(client, intent, started.requestId)
(result?.outcome as? ConsentOutcome.OrgCreated)?.let { open(it.orgId) }
```

`ConsentOutcome` is typed per kind (`OrgCreated`, `InviteCreated`, `InviteAccepted`, …); an
invitation's token never appears in `toString`. See the
[User approvals guide](https://calimero-network.github.io/kotlin-sdk/guides/user-approvals/).

## Build & test

```bash
./gradlew ktlintCheck detekt testDebugUnitTest lint assembleDebug   # what CI runs
./gradlew :sample-app:assembleDebugAndroidTest                       # instrumented tests compile
./gradlew :sample-app:connectedDebugAndroidTest                      # the demo UI test (an emulator)
./ci/check-registry-parity.sh                                        # every client method in the SDK tab
```

`CalimeroClientTest` drives the client against stand-in cloud nodes through the **real** sealed
channel and a mock attestation: retries with one idempotency key, single-flight refresh on `401`,
failover on `421 WRONG_NODE`, `412` confirmations, sealed SSE, the consent request, and the app
backend's org/team/app mapping. See [TESTING.md](TESTING.md).

Docs: <https://calimero-network.github.io/kotlin-sdk/> (sources in `docs/`).

## Related repositories

- [core](https://github.com/calimero-network/core): `merod`, the node, and the app SDK ([docs](https://calimero-network.github.io/core/))
- [mero-tee](https://github.com/calimero-network/mero-tee): the TDX node images and the signed releases this SDK verifies
- [app-registry](https://github.com/calimero-network/app-registry): the registry of signed app packages
- [apps](https://github.com/calimero-network/apps): example apps, including mero-chat
- [mero-js](https://github.com/calimero-network/mero-js), [mero-react](https://github.com/calimero-network/mero-react): the web client and its React bindings
- [swift-sdk](https://github.com/calimero-network/swift-sdk): the iOS twin of this SDK
- [calimero-client-py](https://github.com/calimero-network/calimero-client-py): the Python client

## License

MIT © Calimero Network. Ports the MIT-licensed `mero-js`.
