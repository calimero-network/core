# calimero-context - Context Lifecycle & Local Governance

The actor that owns context creation, join/leave, execution dispatch, group governance, and cross-node sync for a Calimero node.

## Package Identity

- **Crate**: `calimero-context`
- **Entry**: `src/lib.rs`
- **Key deps**: `actix` (the `ContextManager` actor + mailbox), `calimero-governance-store` (group/namespace apply pipeline this crate wraps), `calimero-context-client` / `calimero-context-config` (its own sub-crates, see below), `calimero-storage` + `calimero-store` (Merkle entities and RocksDB-backed KV), `calimero-runtime` (WASM execution), `calimero-dag` (causal ordering for governance ops)

## Commands

```bash
# Build (all three crates in this tree)
cargo build -p calimero-context -p calimero-context-config -p calimero-context-client

# Test
cargo test -p calimero-context
cargo test -p calimero-context-config
cargo test -p calimero-context-client

# Test one integration suite (crates/context/tests/*.rs, one file = one binary)
cargo test -p calimero-context --test hlc_fence
cargo test -p calimero-context --test cascade_atomic_apply
cargo test -p calimero-context --test projection_membership_equivalence

# Test one case
cargo test -p calimero-context fences_stale_schema_delta_after_boundary -- --nocapture
```

## Handler Inventory (`src/handlers/`)

Every RPC the `ContextManager` actor serves is one `actix::Handler` module, dispatched through `ContextMessage` in `src/handlers.rs`. Grouped by concern:

| Group | Handlers |
| --- | --- |
| Context lifecycle | `create_context`, `delete_context`, `join_context`, `leave_context`, `resync_context`, `execute` (+ `execute/{signing,storage,governance_position,upgrade_gate}`), `sync`, `get_context_metadata`, `set_context_metadata`, `acquire_context_lock` |
| Group lifecycle | `create_group`, `delete_group`, `join_group`, `leave_group`, `add_group_members`, `remove_group_members`, `update_group_settings`, `update_member_role`, `set_member_auto_follow`, `rotate_group_key`, `create_group_invitation` |
| Group upgrades | `upgrade_group`, `retry_group_upgrade`, `get_group_upgrade_status`, `get_migration_status`, `abort_migration` |
| Namespace / subgroup governance | `delete_namespace`, `leave_namespace`, `list_namespaces`, `list_namespaces_for_application`, `detach_context_from_group`, `join_subgroup_inheritance`, `set_subgroup_visibility`, `get_namespace_identity`, `namespace_pending_op_count` |
| Signed-op apply (peer-to-peer) | `apply_signed_group_op`, `apply_signed_namespace_op`, `broadcast_group_local_state`, `sync_group` |
| Capabilities / metadata | `get_member_capabilities`, `set_member_capabilities`, `set_default_capabilities`, `get_member_metadata`, `set_member_metadata`, `get_group_metadata`, `set_group_metadata`, `store_*` (the `store_group_meta`, `store_group_context`, `store_member_capability`, `store_member_metadata`, `store_default_capabilities`, `store_subgroup_visibility`, `store_context_metadata`, `store_group_metadata` family - local-write halves used by the apply path) |
| Introspection / admin | `get_group_info`, `get_group_for_context`, `list_all_groups`, `list_group_members`, `list_group_contexts`, `get_cascade_status`, `issue_ownership_proof`, `admit_tee_node`, `set_tee_admission_policy` |
| Application updates | `update_application/mod.rs` |

## Background Listeners (spawned in `Actor::started`)

| Module | Reacts to | Purpose |
| --- | --- | --- |
| `auto_follow` | `OpEvent::ContextRegistered`, `OpEvent::AutoFollowSet` | Emits `JoinContext` on behalf of members with `auto_follow.contexts = true` |
| `self_purge` | `OpEvent::TeeMemberRemoved` (self only) | Drops local signing keys / gov-op log / namespace identity after a TEE eviction; deliberately skips plain `MemberRemoved` (soft-leave keeps rejoin state) |
| `tee_subgroup_admit` | `SubgroupCreated`, `TeeMemberAdmitted` | Admits entitled TEE members into `Restricted` subgroups this node holds keys for |
| `rotation_listener` | `MemberLeft` (persisted worklist) | Discharges the forward-secrecy key rotation a self-leaver cannot mint themselves; every remaining admin races to publish, convergence is by highest epoch |

**Spawn ordering is load-bearing** (see the comment block in `lib.rs`'s `Actor::started`): `auto_follow::spawn` must run before `self_purge::spawn` because auto-follow subscribes to `op_events` synchronously and has no startup re-scan of its own.

## Cache Capacity Constants (`src/lib.rs`)

| Constant | Value | Cache | Eviction basis |
| --- | --- | --- | --- |
| `MAX_CACHED_CONTEXTS` | 1024 | `contexts` | Lock-gated (`ContextLock::is_idle`) |
| `MAX_CACHED_APPLICATIONS` | 256 | `applications` | Always evictable (pure datastore mirror) |
| `MAX_CACHED_MODULES` | 32 | `modules`, `read_only_methods`, `xcall_methods` | Always evictable; compiled WASM is 2-10x source size, hence the tighter cap |
| `MAX_CACHED_NAMESPACE_DAGS` | 1024 | `namespace_dags` | Lock-gated (`Arc<Mutex<DagStore>>::strong_count == 1`) |
| `NAMESPACE_DAG_PRUNE_THRESHOLD` / `NAMESPACE_DAG_PRUNE_RETAIN` | 8192 / 4096 | per-namespace DAG history | Independent of the DAG-count cap above; prunes one hot namespace's retained deltas, not the namespace map itself |

`ContextManagerConfig` (also `src/lib.rs`) holds the two runtime-tunable knobs threaded from node config: `key_delivery_fallback_wait` (default 5s - how long `join_group` waits for a gossip-fallback `KeyDelivery` before failing) and `migration_v2` (default `true` - see the invariant below). Both are set via the builder methods `ContextManager::with_vm_limits` / `with_migration_v2` / `with_scope_projections` rather than public struct literals, so node startup and tests share one construction path.

## Mental Model

**The actor.** `ContextManager` (`src/lib.rs`) is a single `actix::Actor` with a serial mailbox - every context/group RPC funnels through `ContextMessage` (defined in the `primitives` sub-crate) and is dispatched by the `impl Handler<ContextMessage>` match in `src/handlers.rs`. Serial processing is what makes the in-memory caches and per-context locks safe without extra synchronization.

**Per-context locking, not per-actor.** Executing a context method does not block other contexts: `ContextLock` (`src/lib.rs`) wraps an `Arc<RwLock<ContextId>>` per context, acquired in exclusive mode by default and in shared (read) mode only for methods the module ABI marks `#[app::view]`. The lock is checked out as an *owned* guard (`ContextGuard`, in the `primitives` sub-crate) that can be held across the whole WASM execution, even round-tripped through `ContextAtomic::Held` for atomic multi-call batches.

**Four size-capped caches, one eviction rule.** `contexts`, `applications`, `modules`/`read_only_methods`/`xcall_methods`, and `namespace_dags` are all `BoundedCache` (`src/cache.rs`) - a single generic cap + evict abstraction keyed on the `Evictable` trait. `contexts` and `namespace_dags` are lock-gated (evictable only at `Arc::strong_count == 1`, i.e. no in-flight operation holds them); `applications` and the module caches are always-evictable pure datastore mirrors. The datastore stays authoritative in every case, so an eviction just costs a re-fetch, never a correctness issue.

**Governance is a DAG-ordered apply pipeline, wrapped, not owned, here.** The actual group/namespace-op storage and apply logic (`MembershipRepository`, `MetaRepository`, `NamespaceRepository`, `apply_local_signed_group_op`, etc.) lives in `calimero-governance-store` and is re-exported through the `group_store` and `governance_broadcast` compat shims in `src/lib.rs` (kept because `crates/server`, `crates/node`, `crates/meroctl` still import through `calimero_context::group_store::*`). This crate's own `governance_dag.rs` implements `DeltaApplier` so a `calimero-dag` `DagStore<SignedGroupOp>` / `DagStore<SignedNamespaceOp>` can delegate application to that store. `namespace_dags` holds one resident `Arc<Mutex<DagStore<SignedNamespaceOp>>>` per namespace, pruned back to a recent-delta window (`NAMESPACE_DAG_PRUNE_RETAIN`) once it exceeds `NAMESPACE_DAG_PRUNE_THRESHOLD` applied deltas - safe because the durable `NamespaceGovOp` rows and backfill responder serve peers from RocksDB, not from this in-memory DAG.

**Migration is app-schema-driven, not caller-driven.** `migration_plan.rs` derives an `UpgradeAction` (same-schema swap vs. run-migration) purely from the two embedded WASM ABI manifests (`#[app::state(version = N)]` + `#[derive(app::Migrate)]`) - no caller-supplied migrate-method string. `hlc_fence.rs` decides whether an inbound state delta was produced under a schema newer than what the receiving node's *currently loaded* binary can read (fenced on the loaded `ApplicationMeta` blob, not the governance `GroupMeta.app_key`, because under `LazyOnAccess` those two lag each other per-node) and buffers rather than drops when it can't yet be applied - "absorb, don't drop" is the controlling invariant across the whole migration-v2 framework (`ContextManagerConfig::migration_v2`, default on).

**The unified op-log is additive, not live.** `unified_op_store.rs` (persistence) and `unified_applier.rs` / `scope_projection.rs` (projection-folding) build the C2 cutover's causal-log substrate alongside the existing per-plane (data/governance/rotation) stores. Nothing in production reads from it yet - it is dual-written and exercised by its own convergence tests until the per-plane flips land.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `ContextManager` actor, `ContextLock`/`ContextMeta`, cache fields, `Actor::started` (listener spawn ordering), `governance_preflight` / `sign_and_publish_group_op` helpers, `group_store`/`governance_broadcast` compat re-exports |
| `src/handlers.rs` | `ContextMessage` dispatch match; module declarations for every handler |
| `src/handlers/execute/` | The method-execution path: signing, storage wiring, governance-position checks, the migration "upgrade gate" |
| `src/cache.rs` | `BoundedCache`, `Evictable` - the shared cap/eviction abstraction for every hot cache |
| `src/config.rs` | `ContextConfig` - the `[context]` node-config section (client signer config + `migration_v2` switch) |
| `src/governance_dag.rs` | `GroupGovernanceApplier` / namespace equivalent - `DeltaApplier` impls bridging `calimero-dag` to `calimero-governance-store` |
| `src/migration_plan.rs` | `UpgradeAction` derivation from embedded ABI manifests (pure, no I/O) |
| `src/hlc_fence.rs` | `fence_decision` / `delta_fence_decision` - buffer-vs-apply decision for schema-mismatched deltas |
| `src/activation.rs` | Per-context "last activated blob" marker (`marker == group.app_key` invariant) |
| `src/unified_op_store.rs`, `src/unified_applier.rs`, `src/scope_projection.rs` | The additive unified causal-log substrate (not yet load-bearing) |
| `src/apply_authorizer.rs` | `AtCutAuthorizer` impl resolving apply-time authorization against the folded projection |
| `src/auto_follow.rs`, `src/self_purge.rs`, `src/tee_subgroup_admit.rs`, `src/rotation_listener.rs` | The four background listeners spawned in `Actor::started` |
| `src/error.rs` | `ContextError` - typed errors (`ContextDeleted`, `StateInconsistency`, `StorageError`) |
| `tests/*.rs` | Integration suites: cascade apply/atomicity/concurrency, HLC fencing, op-store reconstruction, projection/membership equivalence |

## Invariants and Gotchas

- **Lock-gated eviction is a correctness boundary, not hygiene.** Evicting a "live" `contexts` or `namespace_dags` entry would let a new `get_or_fetch_context`/`get_or_create_namespace_dag` mint a *second* `Arc<RwLock>`/`Arc<Mutex>` for the same key, so two concurrent operations would serialize on different locks. `Evictable::is_idle` (strong-count check) is what prevents this - never bypass it for these two caches.
- **Cache-aside fetch-before-evict.** `get_or_fetch_context` checks existence in the datastore *before* touching the cache, so a lookup for a non-existent context never wastes an eviction slot on nothing.
- **Cached `Context` metadata is refreshed, not just inserted, on hit.** `dag_heads`, `root_hash`, and `application_id` are re-read from the DB on every cache hit because they can change out-of-band (network deltas, `LazyOnAccess` upgrades, cascade target-application changes). Skipping this reload was a real bug: deltas would parent onto stale `dag_heads`, or the execute path would run the OLD WASM module against already-migrated state (a borsh "Not all bytes read" panic).
- **`Actor::started` spawn ordering is load-bearing** - see the comment block above `auto_follow::spawn`; don't reorder the four listener spawns without re-reading it.
- **`tee_subgroup_admit`/`rotation_listener` call `shutdown()` before `spawn()`** because both handlers are process-global singletons and a bare `spawn` no-ops while a prior instance is still running; on actor restart with a different `Store`/`ContextClient` this is required to rebind rather than leave the old handles live.
- **`group_store`/`governance_broadcast` re-exports are a curated, audited list**, not a blanket re-export - anything not listed is `pub(crate)` inside `calimero-governance-store` and intentionally not reachable from here.
- **`MemberCapabilities` (in the `config` sub-crate) accepts unknown bits on the wire but truncates at the point of interpretation** - `from_bits` rejects undefined bits (use for operator/API input you want to refuse), `from_bits_truncate` drops them (use when interpreting a stored/received mask). This is forward-compat: an older peer must still be able to decode a governance op a newer peer produced with an extra capability bit.
- **`migration_v2` defaults on.** `ContextConfig::migration_v2` (absent in a config.toml → `true`) and `ContextManagerConfig::migration_v2` both default to the non-freezing, absorb-don't-drop migration path; setting it `false` restores the legacy group-wide `InProgress` write-freeze.

## Handler Pattern

Every handler module implements `actix::Handler<SomeRequest>` for `ContextManager`, returning `ActorResponse<Self, <SomeRequest as Message>::Result>` so async work (governance-store calls, signing, network I/O) can run inside the actor future without blocking the mailbox:

```rust
impl Handler<CreateContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateContextRequest { seed, application_id, service_name, identity_secret, init_params, group_id, name, .. }: CreateContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // ... resolve identity, run execute() against __calimero_sync_next, sign + publish the ContextCreated op
    }
}
```

The `Request`/`Response` pair and the `impl Message for Request { type Result = eyre::Result<Response>; }` live in the `primitives` sub-crate (`messages.rs` for context-scoped requests, `group.rs` for group-scoped ones); `src/handlers.rs`'s `ContextMessage` match forwards each variant to `Self::forward_handler`, which is what actually invokes the per-type `Handler` impl. Mutation handlers that touch group governance typically start with `ContextManager::governance_preflight` (resolve requester -> load group meta -> check admin -> resolve signing key) and end with `sign_and_publish_group_op` or the raw `calimero_governance_store::sign_apply_and_publish` call.

## JIT Index

```bash
# Find a handler's Request/Response types
rg -n "struct CreateContextRequest" primitives/src/messages.rs

# Find the ContextMessage dispatch match
rg -n "ContextMessage::" src/handlers.rs

# Find where a capability bit is checked
rg -n "CAN_CREATE_SUBGROUP|CAN_DELETE_SUBGROUP" primitives/src/ config/src/ ../governance-store/src/

# Find BoundedCache/Evictable usage
rg -n "impl Evictable for" src/lib.rs

# Find the migration decision table
rg -n "enum UpgradeAction" src/migration_plan.rs
```

## Sub-crates

- **`crates/context/config`** (`calimero-context-config`) - Wire/API request types (`Request`, `ContextRequestKind`, `GroupRequestKind`, `SystemRequest`), `VisibilityMode`, the `MemberCapabilities` bitset, the `[context.config]` node-config shape (`ClientConfig`/`ClientSigner`/`LocalConfig`), and the `Repr`/`ReprBytes` transmute machinery used to move typed ids across borsh/serde/bs58 boundaries.
- **`crates/context/primitives`** (`calimero-context-client`) - The `ContextClient`/`ContextRegistry` facade, `ContextGuard`/`ContextAtomic` (the per-context lock guard type shared with `calimero-context`), every `*Request`/`*Response` message type and the `ContextMessage` actix envelope, group-related types (`group.rs`), and the local-governance wire types (`SignedGroupOp`, `SignedNamespaceOp`, `AckRouter`).

Part of [crates/](../AGENTS.md).
