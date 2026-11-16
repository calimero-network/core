# Core's OpenAPI document and the generated client

mero-core's API client is generated from core's node API, `docs/public/openapi.json` in this same
tree (`../../../docs/public/openapi.json` from the SDK's directory), read in place: there is no
vendored copy and nothing is fetched.

| Layer | Where | Written by |
|---|---|---|
| Facade: `client.orgs.inviteCreate(org = …, role = OrgRole.MEMBER)`, `client.apps.get(app = …)`: one suspend method per operation, grouped by its first tag, the body's fields as named parameters, the typed answer | `mero-core/src/generated/kotlin/com/calimero/mero/api/facade` | `codegen/facade.py` |
| Generated client: `AppsApi`, `OrganizationsApi`, … and their models (`com.calimero.mero.api.apis`, `.models`, `.infrastructure`), reached through `client.api()` | `mero-core/src/generated/kotlin/com/calimero/mero/api` | OpenAPI Generator 7.26.0, `kotlin` / `jvm-okhttp4`, kotlinx.serialization, coroutines, with two templates of ours (`codegen/templates`) |
| Transport: the attested sealed channel (`tee.SealedTransport`, `tee.Noise`, `http.TransportSecurity`), the node in use and failover, the credential and `401` refresh, `503` retries of calls and views, the `412` passkey step-up and the `428` operator proof, the event streams | `mero-core/src/main/kotlin` | by hand |

The generated client's `Call.Factory` is `http.MeroCallFactory`: every generated request goes through
`CalimeroClient`'s own pipeline, sealed to the attested node, exactly as the hand-written methods'
requests do. A refusal throws the `CalimeroError` it means (`ConfirmRequired`, `NotFound`, …), never
the generator's `ClientException`. A `412` is approved with the call's `passkey`, else
`CalimeroClient.passkey`; a `428` signed with the call's `operator`, else `CalimeroClient.operator`.

## Checked in, not generated at build time

The generated sources are committed. Building, testing or releasing the SDK then needs neither the
generator (about a hundred artifacts, none of them in `gradle/verification-metadata.xml`) nor the
network beyond what the build already resolves, and a spec change shows up as a reviewable diff of
the client. `codegen/` is a Gradle build of its own, with its own verification metadata, so the
generator never enters the SDK's build classpath.

```bash
python3 scripts/sdks.py generate mero-kotlin   # from core's root: regenerate the client and facade
./gradlew -p codegen generateMeroClient        # the same, from sdks/clients/mero-kotlin
```

The generated sources are `mero-core/src/generated/kotlin` (`generated` in `sdk.json`). When core's
document changes, CI's `SDK codegen drift` reports that they no longer match, and on merge
core's `sdk-regenerate.yml` regenerates every SDK and opens one pull request (see
`sdks/AGENTS.md`). `codegen/facade-diff.py` lists the facade methods one regeneration added,
removed or changed; a removed or changed one breaks callers.

## Where the document and the generated client still need help

Core's document (#4937, on master since `1ce553a58`) is closed and typed: integers carry `format:
int64`, binary bodies `format: binary`, unions have discriminators, literals are enums, meaningful
nulls are `[T, "null"]`, shared bodies are components (`ConfirmOnly`, `ProofOnly`), and objects are
closed (`additionalProperties: false`). What is left:

- **Meaningful nulls**: a team's `parent: null` (to the top), recovery `publicKey: null` (remove)
  and presence `state: null` (retract). The generated request leaves every null out
  (`explicitNulls = false`, so an optional field you leave alone is never sent), so the facade
  takes these as `Explicit<T>?`. `Explicit(null)` sends `null`; `codegen/facade.py` finds the fields
  in the spec.
- **`POST /consent/requests`** is a `oneOf` with a `kind` discriminator. OpenAPI Generator 7.26's
  kotlinx output renders it as a sealed class that no branch extends, so the spec copy the
  generator reads names it as a component (`prepare-spec.py`) and `schemaMappings` maps it to
  mero-core's `ConsentRequest`.
- **Binary bodies with an unusual media type** (`application/wasm` on `POST /bundles`, `*/*` on
  `POST /apps/{app}/blobs` and `/files`): 7.26 writes no `Content-Type` for them and its ApiClient
  then labels the bytes `application/json`. `Routes.binaryBody` (generated from the spec) gives the
  transport the right type; the wildcard goes as `application/octet-stream`.
- **`AppAccess`, `Deployer`, `OutboundSelection`** stay mapped to mero-core's sealed types, which
  read and write exactly the union's forms.
- **Untyped answers**: `GET /apps/{app}/runs` and `/apps/{app}/runs/{run}` are still free-form.
- **`POST /apps`** has no `confirmMethods`; the deprecated `createApp` still sends it when given.
- **A property named `keys`** is generated as `propertyKeys` (`GET /auth/apikeys`, `GET /apps/{app}/storage`).
- **A sealed blob upload** (`POST /apps/{app}/blobs?sealed=true`) is refused with `400` by current
  core, so `uploadFile` is deprecated with no replacement.
