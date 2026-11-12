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
  (`params`, `returns`, `errors`, `events`), or the node's `{ bundle, abi }` answer. The node's
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
  (`call`, `callConfirmed`, `view`, `events`); no transport of their own.
- `codegen::{swift, kotlin}` write one client type over swift-sdk 2.0 (MeroKit) / kotlin-sdk
  2.0 (mero-core): built on the SDK's `CalimeroClient` and an app id (every method; Kotlin
  calls `apps.method`, Swift `callJSON` / `callConfirmed` / `viewJSON`) or on its
  `ContextContractCalling` (calls and views, so apps can pass `MeroAppBackend.contract` or a
  test double). The app's events, errors (`Event`, `Refusal`) and types are nested in it,
  so two clients share a module or package without clashing; an app type named like a type
  the file uses is renamed (`Names::reserve`). `u64`/`i64`/`bytes` are native types written
  in their D20 form by `DecimalString`/`Base64URL` (Swift) and `U64AsString`/`I64AsString`/
  `Base64Url` serializers (Kotlin).
- `diff::diff` classifies changes between two ABIs (`Severity::Breaking` / `Compatible`) and
  sets `state_error` for an unreadable state change without a state version bump.
- `load` reads an ABI from JSON, a component (top level or nested core module), an `.mpk`, or
  a `manifest.json` beside its artifacts, each capped at `MAX_INPUT_BYTES`.

## Rules

- The JSON forms in the type table (u64/i64 decimal strings, bytes base64url) are the SDK's
  (`crates/sdk-macros/src/types.rs`, D20); change them only with it.
- Generated output is pinned by golden files: `tests/fixtures/<app>.abi.json` (ABIs of
  `apps/kv-store`, `apps/rooms`, `apps/permissions-showcase`, and a crafted `kitchen-sink`)
  against `tests/golden/`. Refresh a fixture from a build: extract the component's
  `calimero:abi` section.
- The Swift and Kotlin goldens must compile with warnings as errors against the SDK releases
  they target: `.github/workflows/app-abi-native-clients.yml` runs
  `scripts/check-app-abi-clients.py swift|kotlin` (macOS / Android SDK) on a change here.
  Moving to a new SDK release moves `SWIFT_SDK_VERSION` / `KOTLIN_SDK_REF` there.

## Verify

```bash
cargo test -p calimero-app-abi
UPDATE_GOLDEN=1 cargo test -p calimero-app-abi --test golden   # then review the diff
```
