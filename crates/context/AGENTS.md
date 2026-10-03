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
| Account devices | `pair_device_init`, `pair_device_complete`, `relink_device`, `rescope_device`, `label_device`, `revoke_device` |
| Context lifecycle | `create_context`, `delete_context`, `join_context`, `leave_context`, `resync_context`, `execute` (+ `execute/{signing,storage,governance_position,upgrade_gate}`), `sync`, `get_context_metadata`, `set_context_metadata`, `acquire_context_lock` |
| Group lifecycle | `create_group`, `delete_group`, `join_group`, `leave_group`, `add_group_members`, `remove_group_members`, `update_member_role`, `set_member_auto_follow`, `rotate_group_key`, `create_group_invitation` |
| Group upgrades | `upgrade_group`, `retry_group_upgrade`, `get_group_upgrade_status`, `get_migration_status`, `abort_migration` |
| Namespace / subgroup governance | `delete_namespace`, `leave_namespace`, `list_namespaces`, `list_namespaces_for_application`, `detach_context_from_group`, `join_subgroup_inheritance`, `set_subgroup_visibility`, `get_namespace_identity`, `namespace_pending_op_count` |
| Signed-op apply (peer-to-peer) | `apply_signed_group_op`, `apply_signed_namespace_op`, `broadcast_group_local_state`, `sync_group` |
| Capabilities / metadata | `get_member_capabilities`, `set_member_capabilities`, `set_default_capabilities`, `get_member_metadata`, `set_member_metadata`, `get_group_metadata`, `set_group_metadata`, `store_*` (the `store_group_meta`, `store_group_context`, `store_member_capability`, `store_member_metadata`, `store_default_capabilities`, `store_subgroup_visibility`, `store_context_metadata`, `store_group_metadata` family - local-write halves used by the apply path) |
| Introspection / admin | `get_group_info`, `get_group_for_context`, `list_all_groups`, `list_group_members`, `list_group_contexts`, `get_cascade_status`, `issue_ownership_proof`, `admit_tee_node`, `set_tee_admission_policy` |
| Application updates | `update_application/mod.rs`, `precompile_application` (compiles an installed application's modules into the module cache; the admin install endpoints start it in the background so the first context does not pay for compiling) |

Two modules there are not handlers. `ensure_account_namespace` is a plain function `pair_device_complete` calls, creating the holder's account namespace on first use, naming it, and recording the holder's own device in it. `follow_namespace` is the one definition of following a namespace - note participation, subscribe, pull - called by `pair_device_init` and by the `account_follow` listener.

## Background Listeners (spawned in `Actor::started`)

| Module | Reacts to | Purpose |
| --- | --- | --- |
| `auto_follow` | `OpEvent::ContextRegistered`, `OpEvent::AutoFollowSet` | Emits `JoinContext` on behalf of members with `auto_follow.contexts = true` |
| `self_purge` | `OpEvent::TeeMemberRemoved` (self only) | Drops local signing keys / gov-op log / namespace identity after a TEE eviction; deliberately skips plain `MemberRemoved` (soft-leave keeps rejoin state) |
| `tee_subgroup_admit` | `SubgroupCreated`, `TeeMemberAdmitted` | Admits entitled TEE members into `Restricted` subgroups this node holds keys for |
| `tee_vault` | `TeeAuthorityChanged` (authoring policy, evidence), `TeeMemberAdmitted`, `TeeMemberRemoved` - for that namespace; plus a 10s sweep of every namespace | On a TEE authority: creates the namespace TEE key when the namespace has none it may seal to (none yet, or every key retired because a TEE that held it is no longer a TEE authority - removed, dropped from the authoring policy, or with lapsed evidence: the rotation), and publishes a `TeeVaultKeyDelivered` of every key it holds for each TEE authority without a copy. A no-op elsewhere |
| `rotation_listener` | `MemberLeft` (persisted worklist) | Discharges the forward-secrecy key rotation a self-leaver cannot mint themselves; every remaining admin races to publish, convergence is by highest epoch |
| `membership_events` | `MemberAdded`/`MemberRemoved`/`MemberRoleChanged`, `MigrationStarted` | Observational bridge: turns those `OpEvent`s into `NodeEvent::GroupMembership` / `NodeEvent::GroupMigration` for connected SSE/WS clients. Migration rides the apply path so every node that folds the op announces it, not only the one whose client asked |
| `account_migration` | Nothing - one shot on start | Publishes a pre-registry holder's cached device certificates into the account namespace, then drops the rows - only once every statement landed, so a partial run is retried by the next start, and never at a scope wider than the registry already holds |
| `account_follow` | `AccountNamespaceGained`, `AccountNamespaceLeft`, `AccountDeviceCertified`, `DeviceRevoked` (all only from this node's account namespace) | Follows a namespace the account gained whose application this device's registry scope covers, walks the whole set when this device's own scope arrives - following what it covers and unfollowing what it no longer does, so a narrowed scope lets those topics go - unsubscribes from one the account left, binds a newly certified sibling into every namespace this node takes part in whose target its scope covers, and carries a proof-bearing revocation into every namespace where that device is still bound |

**Spawn ordering is load-bearing** (see the comment block in `lib.rs`'s `Actor::started`): `auto_follow::spawn` must run before `self_purge::spawn` because auto-follow subscribes to `op_events` synchronously and has no startup re-scan of its own. `account_migration::spawn` takes no `OpEvent` subscription of its own, but it runs *after* `account_follow::spawn` so that the `AccountDeviceCertified` ops its one-shot publishes are projected by a listener that is already up. `account_follow` also owns `namespaces_in_reach`, the filter the node's start-up subscribe sweep and `join_context`'s namespace sync both run their participating set through: unfiltered, those sweeps race this listener's own unfollow in another actor and whichever finishes last decides whether a narrowed device keeps replicating. Within the listener, every follow or unfollow is decided and carried out under one lock, so a burst of this device's own certificates (a catch-up folding a narrowed scope with the scope that replaced it) settles on the newest scope: decided per task, the task holding the older scope could drop a topic after the newer one had kept it, and nothing followed it again. `account_follow`'s start-up sweep walks the account's namespace set itself, and that covers the follow arms only: a namespace gained, a conservative unfollow of one it left, and the set this device's own scope stopped covering. A certified or a revoked op applied before the listener subscribed is repaired by a relink or by a repeated revoke, never by the sweep - an applied op is never re-applied, and a re-received one drops its queued events.

## Cache Capacity Constants (`src/lib.rs`)

| Constant | Value | Cache | Eviction basis |
| --- | --- | --- | --- |
| `MAX_CACHED_CONTEXTS` | 1024 | `contexts` | Lock-gated (`ContextLock::is_idle`) |
| `MAX_CACHED_APPLICATIONS` | 256 | `applications` | Always evictable (pure datastore mirror) |
| `MAX_CACHED_MODULES` | 32 | `modules` (each entry also holds the module's read-only, xcall and handler method sets) | Always evictable; compiled WASM is 2-10x source size, hence the tighter cap. Compiles in flight live in `compiling` (a shared future per module) until they land here, so concurrent requests for one module share its compile |
| `MAX_CACHED_NAMESPACE_DAGS` | 1024 | `namespace_dags` | Lock-gated (`Arc<Mutex<DagStore>>::strong_count == 1`) |
| `NAMESPACE_DAG_PRUNE_THRESHOLD` / `NAMESPACE_DAG_PRUNE_RETAIN` | 8192 / 4096 | per-namespace DAG history | Independent of the DAG-count cap above; prunes one hot namespace's retained deltas, not the namespace map itself |
| `NAMESPACE_PENDING_TTL` / `NAMESPACE_PENDING_SWEEP_INTERVAL` (`src/lifecycle.rs`) | 10 min / 60 s | ops waiting on a missing parent in a namespace DAG | A periodic sweep drops pending ops older than the TTL (skipping a DAG that is mid-apply); a dropped op is fetched again by a later namespace sync |

`ContextManagerConfig` (also `src/lib.rs`) holds the two runtime-tunable knobs threaded from node config: `key_delivery_fallback_wait` (default 5s - how long `join_group` waits for a gossip-fallback `KeyDelivery` before failing) and `migration_v2` (default `true` - see the invariant below). Both are set via the builder methods `ContextManager::with_vm_limits` / `with_migration_v2` / `with_scope_projections` rather than public struct literals, so node startup and tests share one construction path.

## Mental Model

**The actor.** `ContextManager` (`src/lib.rs`) is a single `actix::Actor` with a serial mailbox - every context/group RPC funnels through `ContextMessage` (defined in the `primitives` sub-crate) and is dispatched by the `impl Handler<ContextMessage>` match in `src/handlers.rs`. Serial processing is what makes the in-memory caches and per-context locks safe without extra synchronization.

**Per-context locking, not per-actor.** Executing a context method does not block other contexts: `ContextLock` (`src/lib.rs`) wraps an `Arc<RwLock<ContextId>>` per context, acquired in exclusive mode by default and in shared (read) mode only for methods the module ABI marks `#[app::view]`. The lock is checked out as an *owned* guard (`ContextGuard`, in the `primitives` sub-crate) that can be held across the whole WASM execution, even round-tripped through `ContextAtomic::Held` for atomic multi-call batches.

**Four size-capped caches, one eviction rule.** `contexts`, `applications`, `modules`, and `namespace_dags` are all `BoundedCache` (`src/cache.rs`) - a single generic cap + evict abstraction keyed on the `Evictable` trait. `contexts` and `namespace_dags` are lock-gated (evictable only at `Arc::strong_count == 1`, i.e. no in-flight operation holds them); `applications` and `modules` are always-evictable pure datastore mirrors. The datastore stays authoritative in every case, so an eviction just costs a re-fetch, never a correctness issue.

**Governance is a DAG-ordered apply pipeline, wrapped, not owned, here.** The actual group/namespace-op storage and apply logic (`MembershipRepository`, `MetaRepository`, `NamespaceRepository`, `apply_local_signed_group_op`, etc.) lives in `calimero-governance-store`, which every caller now imports directly (the `group_store` / `governance_broadcast` re-export shims this crate kept through the extraction are gone). This crate's own `governance_dag.rs` implements `DeltaApplier` so a `calimero-dag` `DagStore<SignedGroupOp>` / `DagStore<SignedNamespaceOp>` can delegate application to that store. `namespace_dags` holds one resident `Arc<Mutex<DagStore<SignedNamespaceOp>>>` per namespace, pruned back to a recent-delta window (`NAMESPACE_DAG_PRUNE_RETAIN`) once it exceeds `NAMESPACE_DAG_PRUNE_THRESHOLD` applied deltas - safe because the durable `NamespaceGovOp` rows and backfill responder serve peers from RocksDB, not from this in-memory DAG.

**Migration is app-schema-driven, not caller-driven.** `migration_plan.rs` derives an `UpgradeAction` (same-schema swap vs. run-migration) purely from the two embedded WASM ABI manifests (`#[app::state(version = N)]` + `#[derive(app::Migrate)]`) - no caller-supplied migrate-method string. `hlc_fence.rs` decides whether an inbound state delta was produced under a schema newer than what the receiving node's *currently loaded* binary can read (fenced on the loaded `ApplicationMeta` blob, not the governance `GroupMeta.bytecode_id`, because those two can diverge per-node) and buffers rather than drops when it can't yet be applied - "absorb, don't drop" is the controlling invariant across the whole migration-v2 framework (`ContextManagerConfig::migration_v2`, default on).

**The unified op-log is additive, not live.** `unified_op_store.rs` (persistence) and `unified_applier.rs` / `scope_projection.rs` (projection-folding) build the C2 cutover's causal-log substrate alongside the existing per-plane (data/governance/rotation) stores. Nothing in production reads from it yet - it is dual-written and exercised by its own convergence tests until the per-plane flips land.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `ContextManager` actor, `ContextLock`/`ContextMeta`, cache fields, `Actor::started` (listener spawn ordering), `governance_preflight` / `sign_and_publish_group_op` helpers |
| `src/handlers.rs` | `ContextMessage` dispatch match; module declarations for every handler |
| `src/handlers/execute/` | The method-execution path: signing, storage wiring, governance-position checks, the migration "upgrade gate" |
| `src/cache.rs` | `BoundedCache`, `Evictable` - the shared cap/eviction abstraction for every hot cache |
| `src/config.rs` | `ContextConfig` - the `[context]` node-config section (client signer config + `migration_v2` switch), and `SearchSettings` (`[context.search]`: on/off, commit interval, cache, open-index cap, idle close, audit interval, compaction threshold) |
| `src/search.rs` | Full-text search seams: `changed_entity_ids` (ids a run touched, from its `StorageDelta`), `SearchHostAdapter` (the runtime's `SearchHost`, with the work it did for gas), `NodeContextSource` (the indexer's way into the app's `__calimero_search_*` exports, the state root and the context lock) |
| `src/governance_dag.rs` | `GroupGovernanceApplier` / namespace equivalent - `DeltaApplier` impls bridging `calimero-dag` to `calimero-governance-store` |
| `src/migration_plan.rs` | `UpgradeAction` derivation from embedded ABI manifests (pure, no I/O) |
| `src/hlc_fence.rs` | `fence_decision` / `delta_fence_decision` - buffer-vs-apply decision for schema-mismatched deltas |
| `src/activation.rs` | Per-context "last activated blob" marker (`activated_bytecode()`; `marker == group.bytecode_id` invariant) |
| `src/unified_op_store.rs`, `src/unified_applier.rs`, `src/scope_projection.rs` | The additive unified causal-log substrate (not yet load-bearing); `ScopeProjections::shared_writers_at_cut` resolves a `SharedStorage` cell's writer set at a governance cut from the rotations of the context's group whose signers stood at their own parents, `None` on an incomplete or unreadable one |
| `src/apply_authorizer.rs` | `AtCutAuthorizer` impls: `EphemeralProjectionAuthorizer` (governance apply, folds per op) and `ProjectionAuthorizer` (over the node's maintained projection, for a delegated delta's cited cut). Both hand out `CutStandingReads` (`scope_projection.rs`) — the at-cut source of the one delegated-standing rule in `calimero-governance-store::warrant_admission` — and answer the warrant floor via `ScopeProjections::cut_covers_floor` |
| `src/auto_follow.rs`, `src/account_follow.rs`, `src/self_purge.rs`, `src/tee_subgroup_admit.rs`, `src/rotation_listener.rs`, `src/membership_events.rs` | The background listeners spawned in `Actor::started` |
| `src/account_namespace.rs` | `announce` - the one place a gained/left op is published into this node's account namespace, called by `create_group`, `join_group`, `leave_namespace` and the creation backfill |
| `src/migration_events.rs` | `NodeEvent::GroupMigration` emit helper - resolves the namespace root every migration payload is keyed on |
| `src/error.rs` | `ContextError` - typed errors (`ContextDeleted`, `StateInconsistency`, `StorageError`) |
| `tests/*.rs` | Integration suites: cascade apply/atomicity/concurrency, HLC fencing, op-store reconstruction, projection/membership equivalence |

## Invariants and Gotchas

- **`ApplySignedNamespaceOpRequest` is the one door into a namespace DAG, and it checks before the DAG sees an op.** Gossip, backfill, catch-up and the local publisher all arrive here, so the handler runs `op.validate()` and `op.verify_signature()` first (a failure is an `Err`, never buffered). `NamespaceGovernanceApplier::admit_pending` then decides whether an op with a missing parent may wait: its signer must be certified in the namespace, or the op must be a join that carries its own signer's credential (`pending_standing` in `calimero-governance-store`). A refused op is not buffered and the handler answers `NamespaceApplyOutcome::NotAdmitted`. Pending ops are charged to their signer against `MAX_PENDING_PER_ORIGIN`; joins from not-yet-certified signers share one allowance, and an evicted one is fetched again by sync. Ops expire after `NAMESPACE_PENDING_TTL`. Gossip answers `NotAdmitted` (and `Pending`) by fetching the op's missing ancestors from the sender by id, in at most `MAX_ANCESTRY_ROUNDS` rounds and `MAX_ANCESTRY_OPS` ops, applied parents first; the `NotAdmitted` fetch is limited to one per (namespace, sender) per 30 s. A chain deeper than the round limit is completed only when a later gossip op triggers another fetch. The empty backfill request that sync rounds send returns the same first 500 ops by hash every time, so it does not page through a longer history (a known follow-up). `join_group` orders the join response's catch-up ops parents first before applying them.
- **Lock-gated eviction is a correctness boundary, not hygiene.** Evicting a "live" `contexts` or `namespace_dags` entry would let a new `get_or_fetch_context`/`get_or_create_namespace_dag` mint a *second* `Arc<RwLock>`/`Arc<Mutex>` for the same key, so two concurrent operations would serialize on different locks. `Evictable::is_idle` (strong-count check) is what prevents this - never bypass it for these two caches.
- **Cache-aside fetch-before-evict.** `get_or_fetch_context` checks existence in the datastore *before* touching the cache, so a lookup for a non-existent context never wastes an eviction slot on nothing.
- **Cached `Context` metadata is refreshed, not just inserted, on hit.** `dag_heads`, `root_hash`, and `application_id` are re-read from the DB on every cache hit because they can change out-of-band (network deltas, context upgrades, cascade target-application changes). Skipping this reload was a real bug: deltas would parent onto stale `dag_heads`, or the execute path would run the OLD WASM module against already-migrated state (a borsh "Not all bytes read" panic).
- **`Actor::started` spawn ordering is load-bearing** - see the comment block above `auto_follow::spawn`; don't reorder the listener spawns without re-reading it.
- **`tee_subgroup_admit`/`rotation_listener` call `shutdown()` before `spawn()`** because both handlers are process-global singletons and a bare `spawn` no-ops while a prior instance is still running; on actor restart with a different `Store`/`ContextClient` this is required to rebind rather than leave the old handles live.
- **Governance symbols come from `calimero-governance-store` directly** - import `calimero_governance_store::{MembershipRepository, ...}`, not through this crate. Anything that crate keeps `pub(crate)` is intentionally unreachable.
- **`MemberCapabilities` (in the `config` sub-crate) accepts unknown bits on the wire but truncates at the point of interpretation** - `from_bits` rejects undefined bits (use for operator/API input you want to refuse), `from_bits_truncate` drops them (use when interpreting a stored/received mask). This is forward-compat: an older peer must still be able to decode a governance op a newer peer produced with an extra capability bit.
- **`admit_tee_node` has two policy forms, and only one needs the network.** A list policy is checked in the actor. A signed-release policy (`TeeAdmissionPolicy::release_trust`) is checked after it, in the async half: `calimero_tee_release::fetch_node_release` fetches and Sigstore-verifies the release the TEE named, and the quote must match one of the allowed profiles. A subgroup admission (`account: None`, from `tee_subgroup_admit`) skips that fetch — its record carries no version, and the root admission checked it.
- **A delegated run's writes are never discarded as the executor's.** `internal_execute`'s read-only discard (`executor_is_read_only`) skips delegated runs: the writes are the author's, and the warrant gate decides up front whether they may happen (a `RelayTee` executor relays by role, a `ReadOnlyTee` never, a read-only author never). A role refusal from the gate surfaces as `ExecuteError::DelegatedWriteRefused`, which the server maps to 403 — never a `200` for a dropped write. A TEE node's own JSON-RPC writes are still discarded.
- **A delegated run signs the author's entries as the relay, on their behalf.** Storage stamps a run's placeholders with the run's device as `signer`, and a delegated run's device is the author's, whose key the relay does not hold. `sign_authorized_actions` (`execute/signing.rs`) takes the account a delegated run writes for (execute and `create_context` pass the warrant's author) and restamps each placeholder with `signer` = this node's key and `on_behalf` = that account before signing; it refuses outright to sign an entry whose `signer` is not the signing key. Peers accept those entries only from a `RelayTee` (`calimero_governance_store::on_behalf_standing`), which is narrower than the warrant gate's relay rule, so the relay asks it of its own account before the run: `ExecuteError::DelegatedWriteRefused { reason: ExecutorIsNotARelay }` for a write, `OnBehalfRefusal` for a creation, both 403 at the API.
- **A discarded read-only write is reported, not raised.** The node's read-only role is its effective one (direct or inherited through an Open subgroup; see `governance-store`'s AGENTS.md). When `executor_is_read_only` drops a run's writes, `internal_execute` still returns `Ok` and sets `ExecuteResponse::read_only_write_discarded`: event handlers on a read-only replica come through the same path, and an error there would keep their events queued for replay forever. The server's `execute_request` (JSON-RPC and WebSocket) turns the flag into `ExecutionError::ReadOnlyWriteRefused`, so a client never gets a success for a dropped write.
- **The TEE role comes from the namespace's admission policy mode.** `admit_tee_node` publishes `policy.mode.role()` and re-admits a TEE whose direct row is the other TEE role; `set_tee_admission_policy` publishes the v2 policy op, then a `MemberRoleSet` per direct TEE row in the old role. Only the root conversion is load-bearing: relaying reads a TEE's role at its namespace root row (`warrant_gate::executor_standing`), so a copy in a `Restricted` subgroup this node does not administer — whose conversion is refused and only logged — no longer decides whether the TEE relays there.
- **A TEE is never demoted or promoted out of the TEE roles.** `update_member_role` and `add_group_members` call `MembershipPolicy::require_tee_row_keeps_tee_role` against the target's current row before signing, so the apply's `TeeMemberRoleLocked` refusal (403 at the API) also stops the op locally; `add_group_members` checks the whole batch first so a TEE in it adds nobody.
- **A TEE run is signed over what fired it.** `ContextClient::execute_tee_trigger` takes the `TeeTriggerCause` and sets `ExecuteRequest::tee_trigger`; the execute path signs the delta under `SignatureDomain::Tee` (`tee_delta_signature_payload`), stores the cause beside it (`tee_trigger::record_delta_trigger`) and broadcasts it in the clear. There is no fired-marker event any more: receivers record the marker from the signed cause. A delta cannot be both delegated and TEE-triggered.
- **Whose right to write the execute path checks depends on `ExecuteRequest::write_source`.** A run commits state only if its author may write it. For `WriteSource::Local` (every constructor but one) the author is this node, so a `ReadOnly`/`ReadOnlyTee` member's writes are discarded, and so is a state op from anyone not Admin/Member (the B3 gate, #2382). `WriteSource::RemoteDelta` is set only by `ContextClient::apply_remote_delta`, the node's delta applier: the delta's author was verified and authorized on the receive path, so the gate asks only that this node replicate the context (any role). Do not route an inbound delta through `execute("__calimero_sync_next")` — it is then this node's write, and a read-only replica silently drops it while its DAG records it applied. A handler request marking any other method `RemoteDelta` is refused. A removed member cannot run the context at all: removal deletes its `ContextIdentity` marker, and `execute` returns `Unauthorized` before any gate.
- **A received event runs only a declared handler.** The node dispatches an event's handler through `ContextClient::execute_event_handler`, which sets `ExecuteRequest::event_handler`; that run, and a TEE run whose `tee_trigger` is `TeeTriggerCause::Event`, is refused with `ExecuteError::NotAnEventHandler` unless the method is in the handler set of the blob's `modules` entry (`Method.handler` in the embedded ABI) and has no `__calimero` prefix. The set lives in the same entry as the compiled module, empty when the ABI is absent, so the gate fails closed. A refusal by a blob other than the group's target is `ExecuteError::EventHandlerAwaitsUpgrade` instead: the newer version may declare the method, so the node keeps the event for replay rather than settling it. Do not dispatch a peer-named method through `execute`: it is then an ordinary local call and nothing gates it.
- **A delta's id commits to its events.** `internal_execute` serializes the run's events once (`events_payload`) and passes their `CausalDelta::hash_events` to `compute_id`; the broadcast sends those same bytes, and the hash is kept beside the row (`delta_events::record_events_hash`) so the delta can be served after its events are cleared.
- **Search rows ride the execute batch.** For an app whose module exports `__calimero_search_extract`, `internal_execute` stages one `SearchDirty` row (state root before, root after, changed ids) into the run's own transaction before `storage.commit()`, then notifies the indexer; `before` is read from the committed store under the write lock. Every other path that writes state (snapshot, repair sync, migration) writes no row on purpose: the indexer's root-chain check catches them (see `crates/search/AGENTS.md`). An app without the export pays one `exports_function` lookup.
- **A method the ABI declares a view runs as one even on a cold cache.** Lock selection reads the read-only set from the module cache and falls back to the write lock on a miss (the first call after a restart). The handler re-resolves the declared intent once the module is loaded and runs a declared view read-only either way, so it gets the search handle and the read-only storage wrapper whether or not the cache was warm. The `__calimero_search_*` exports are read-only by name.
- **`ScopeProjections` reads every view without the ops a removal voids.** `walk` hands `cut_ancestry_with_void` the scope's void set (`void_set`, memoized per scope until its log changes and keyed by the `AuthorityBase` read from the store), and `state_of` rebuilds the streaming state without those ops, so `scope_root_for` of a node that applied an op before the removal equals that of a node that never saw it. `op_is_void`, `voided_with` and `group_rows_with` are what `EphemeralProjectionAuthorizer` answers the apply seam's `op_is_void`, `voided_ops` and `group_rows` with; `group_rows_with` answers `None` over a log with a gap, and `forget` drops the per-apply fold. The `AuthorityBase` (the owner and default capability, in no op) is read from the store by `apply_backfill_with_base`, which the node's two backfill sites call, and `authority_base` answers `None` when the store cannot be read.
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

The `Request`/`Response` pair and the `impl Message for Request { type Result = eyre::Result<Response>; }` live in the `primitives` sub-crate (`messages.rs` for context-scoped requests, `group.rs` for group-scoped ones); `src/handlers.rs`'s `ContextMessage` match forwards each variant to `Self::forward_handler`, which is what actually invokes the per-type `Handler` impl. Mutation handlers that touch group governance typically start with `ContextManager::governance_preflight` (resolve signer -> load group meta -> check admin -> resolve signing key) and end with `sign_and_publish_group_op` or the raw `calimero_governance_store::sign_apply_and_publish` call.

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

- **`crates/context/config`** (`calimero-context-config`) - The typed id newtypes (`ContextId`, `ContextGroupId`, `ContextIdentity`, `SignerId`, `BytecodeId`, `ApplicationId`), the invitation wire types (`InvitationFromMember`, `SignedOpenInvitation`, `GroupInvitationFromAdmin`, `SignedGroupOpenInvitation`), `GovernanceParentEdge`, `VisibilityMode`, the `MemberCapabilities` bitset, `MAX_NAMESPACE_DEPTH`, and the `Repr`/`ReprBytes` transmute machinery used to move typed ids across borsh/serde/bs58 boundaries.
- **`crates/context/primitives`** (`calimero-context-client`) - The `ContextClient`/`ContextRegistry` facade, `ContextGuard`/`ContextAtomic` (the per-context lock guard type shared with `calimero-context`), every `*Request`/`*Response` message type and the `ContextMessage` actix envelope, group-related types (`group.rs`), and the local-governance wire types (`SignedGroupOp`, `SignedNamespaceOp`, `AckRouter`).

Part of [crates/](../AGENTS.md).
