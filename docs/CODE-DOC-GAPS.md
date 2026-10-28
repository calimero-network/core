# Code → Docs gap audit

The original audit covered six v1 subsystems (runtime/SDK/ABI, storage, sync,
governance/context, networking, server/auth/config). The v1 node is deleted,
so its sync, governance, networking, server and auth findings no
longer describe any code and were removed; the operator docs were rewritten
from `merod`, `meroctl` and `calimero-node`. What follows is
what still applies, to code that exists now.

## Still open

- **`minRuntimeVersion` is enforced.** The manifest field is checked at install
  (`crates/node/src/packages.rs`: the node's release must not be below
  it). Any page that says the field does not exist is wrong.
- **Authored-data migration is per-owner.** A whole-root `#[app::migrate]`
  does not convert `Authored*` entries; the generated `migrate_my_entries`
  method must be called by each owner (`crates/sdk/macros/src/state.rs`,
  `crates/sdk/src/state.rs`). → `build/migrations.mdx`.
- **JS runtime surface.** The `js_crdt_*` / `js_user_storage_*` host functions
  with their `-1`/register error ABI, and the storage-bridge functions that let
  JS produce a valid root (`crates/runtime/src/logic/host_functions/js_collections.rs`).
  → `build/host-functions.mdx`. The runtime is being replaced; check with its
  owner first.
- **ABI section facts.** The ABI lives in the `calimero_abi_v1` custom section,
  and the node reads handlers, xcall entry points, timers, search exports and
  migrations from it, so a bundle built without `cargo mero build` silently
  gets none of them. → `build/abi.mdx`, `build/cargo-mero.mdx`.
- **Storage internals (P3).** Temporal staging and one-batch atomicity, the
  runtime's deterministic-id rekey and `DeleteRef` tombstone, the
  Merkle-invisible `schema_version` tag; `#[app::view]` post-execution
  enforcement; the reserved `__calimero` prefix.

## Closed by the operator rewrite

- CLI references generated from the clap definitions (`operate/merod`,
  `operate/meroctl`).
- Every node route with its credential (`operate/admin-api`).
- `/ops/metrics` is behind the ops token and inside the sealed channel
  (`operate/observability`, `operate/security`).
- KMS release-policy verification (`MERO_KMS_RELEASE_TAG` / `MERO_KMS_VERSION`
  / `MERO_TEE_VERSION`) (`operate/deployment`, `operate/merod`).
