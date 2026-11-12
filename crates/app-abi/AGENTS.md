# calimero-app-abi - an app's ABI for clients: OpenAPI, typed clients, breaking changes

## Package Identity

- **Crate**: `calimero-app-abi` (`publish = false`), a library
- **Used by**: `calimero-node` (`GET /apps/{app}/openapi.json`), `mero-app-build`
  (`codegen`, `abi-diff`), `mero-sign` (`sign --previous`)
- **Docs**: [App interface](../../docs/src/content/docs/build/app-abi.mdx) (the format, the OpenAPI
  mapping and its type table, the breaking-change rules),
  [Typed clients for your app](../../docs/src/content/docs/build/guides/typed-clients.mdx)

## What This Crate Does

- `abi::AppAbi::from_json` reads the `calimero:abi` JSON whole, free-form keys included
  (`params`, `returns`, `errors`, `events`, and `types`: each app type's `TypeDef` with its
  `Shape`, which `AppAbi::shape_of` instantiates for `Page<Item>`; an ABI without it, or an
  entry that does not read, leaves the type unknown), or the node's `{ bundle, abi }` answer. The node's
  own reader, `calimero_app_host::Abi`, keeps only what it enforces; this one is for clients
  and tools and never decides access.
- `ty::Type::parse` reads ABI type strings, the SDK's spellings (`option<T>`, `list<T>`,
  `tuple<..>`, `bytes`) and the Rust ones `abi_type` leaves (`BTreeMap<String,u32>`, `[u8;32]`);
  anything else is `Type::Named`, never an error.
- `openapi::document` maps an ABI to OpenAPI 3.1: one `POST /apps/{app}/methods/{name}` per
  method a client calls (`Method::client_callable`), the app's errors as `422 APP_ERROR`,
  `412 CONFIRM_REQUIRED` on `confirm` methods, events on `GET /apps/{app}/events`. A method
  is answered as a view exactly when `Method::runs_as_view` (the rule `component_intents` and
  `api::app_method` apply).
- `codegen::{typescript, python}` write thin clients over mero-js / calimero-client-py
  (`client.apps.call`, `client.apps.view`, `client.events`; Python through `client.json`); no
  transport of their own. The TypeScript class takes the narrow `AppCallTransport` (`apps.call`
  and `apps.view`, so `Pick<CalimeroClient, 'apps'>` or a test double of the two) and is generic
  in it (`KvStoreClient<T extends AppCallTransport = AppTransport>`): `events()` takes
  `this: KvStoreClient<AppTransport>`, so it type-checks only over a transport with `events`.
  An `after` hook runs after every request (mero-js has no session-change event to store a
  refreshed session from). Each method answers the result; the same methods under `raw`
  (`<Name>RawApi`, `rawAnswers` when the app has a method `raw`) answer the whole answer,
  `CallAnswer<T>` / `ViewAnswer<T>` with the log index and a call's events. The class
  implements `<Name>Api` (`KvStoreClient` → `KvStoreApi`), the methods alone, which a
  caller's test double implements instead of the transport. An `init` with parameters gets
  `InitArgs` (what `apps.create`'s `initArgs` carries).
- `codegen::{swift, kotlin}` write one client type over swift-sdk (MeroKit) / kotlin-sdk
  (mero-core), 2.0 or later: built on the SDK's `CalimeroClient` and an app id (every method; Kotlin
  calls `apps.method`, Swift `callJSON` / `callConfirmed` / `viewJSON`) or on its
  `ContextContractCalling` (calls and views, so apps can pass `MeroAppBackend.contract` or a
  test double). The app's events, errors (`Event`, `Refusal`) and types are nested in it,
  so two clients share a module or package without clashing; an app type named like a type
  the file uses is renamed (`Names::reserve`). `u64`/`i64`/`bytes` are native types written
  in their D20 form by `DecimalString`/`Base64URL` (Swift) and `U64AsString`/`I64AsString`/
  `Base64Url` serializers (Kotlin). A type with a shape in `types` is a nested
  `Codable, Sendable, Equatable` struct or enum (Swift: hand-written `init(from:)`/`encode(to:)`
  per serde tagging, `indirect` or `@Indirect` where it holds itself) or a data class or sealed
  interface with a hand-written serializer in its companion (Kotlin: `Item.serializer()`,
  `Page.serializer(Item.serializer())`, no plugin); one without stays `JSONValue`/`JsonElement`.
- `diff::diff` classifies changes between two ABIs (`Severity::Breaking` / `Compatible`) and
  sets `state_error` for an unreadable state change without a state version bump. `diff::shapes`
  compares `types` for each app type the old ABI's callers exchange, by name, in the direction
  it travels (sent: may widen; read: may narrow; both: must hold both).
- `load` reads an ABI from JSON, a component (top level or nested core module), an `.mpk`, or
  a `manifest.json` beside its artifacts, each capped at `MAX_INPUT_BYTES`.

## Rules

- The JSON forms in the type table (u64/i64 decimal strings, bytes base64url) are the SDK's
  (`crates/sdk-macros/src/types.rs`, D20); change them only with it.
- Generated output is pinned by golden files: `tests/fixtures/<app>.abi.json` (ABIs of
  `apps/kv-store`, `apps/rooms`, `apps/permissions-showcase`, and a crafted `kitchen-sink`)
  against `tests/golden/`. Refresh a fixture from a build: extract the component's
  `calimero:abi` section.
- The goldens must run against the published clients, which the golden test cannot see:
  `tests/ts` type-checks every golden `.ts` against mero-js (pinned `^33`) and runs them on a
  test double, `tests/py` imports every golden `.py` with calimero-client-py and binds each
  call to the real client's signature, then runs mypy. `app-abi-clients.yml` runs both on a
  change to this crate; bump the pins when a client's major moves.
- The Swift and Kotlin goldens must compile with warnings as errors against the SDK releases
  they target, and the kitchen-sink's types must round-trip their JSON (`tests/native/swift`,
  `tests/native/kotlin`: every enum tagging style, records, recursive and generic types):
  `.github/workflows/app-abi-native-clients.yml` runs
  `scripts/check-app-abi-clients.py swift|kotlin` (macOS / Android SDK) on a change here.
  Moving to a new SDK release moves `SWIFT_SDK_VERSION` / `KOTLIN_SDK_REF` there.

## Verify

```bash
cargo test -p calimero-app-abi
UPDATE_GOLDEN=1 cargo test -p calimero-app-abi --test golden   # then review the diff
(cd tests/ts && npm ci && npm run check)                         # the goldens against mero-js
(cd tests/py && pip install -r requirements.txt && python smoke.py \
  && mypy --strict --follow-untyped-imports ../golden/*.py)     # against calimero-client-py
```
