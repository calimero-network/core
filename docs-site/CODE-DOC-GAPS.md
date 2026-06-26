# Code → Docs gap audit

**Status:** §0 (contradictions) ✅ fixed · §1 (P1 subsystems) ✅ documented · §2 (P2) ✅ documented · §3 (P3) ⏳ open.

Findings from a six-subsystem code audit (runtime/SDK/ABI, storage, sync,
governance/context, networking, server/auth/config). Each was grep-verified
against `src/content/docs/`. Ordered by priority. `file:line` anchors included.

## 0. Docs that are WRONG (contradict the code) — fix first

- **`minRuntimeVersion` "doesn't exist".** `protocol/upgrades.mdx:50` has an Aside saying no such field exists. It IS a required manifest field, enforced at install (bails if `min_runtime_version > current`). `crates/node/primitives/src/bundle/mod.rs:95`, `.../client/application/bundle.rs:148`.
- **Snapshot boundary "negotiation".** Docs imply the requested cutoff is honored; it's ignored dead code — you always get the responder's current live root. `crates/node/src/sync/snapshot.rs:90,119,437`.
- **Receive-path "key-pending buffer".** `receive-path.mdx` describes an indefinite wait for a missing key; actual path is a bounded 3s poll then bail to gossip/heartbeat. `crates/node/src/handlers/state_delta/crypto.rs:26`.
- **Key-delivery seeds namespace admin.** `key-rotation.mdx:230` says the responder identity seeds the admin; replaced by the all-zeros placeholder-admin bootstrap (deliverer no longer trusted). `crates/governance-store/src/lib.rs:134`.
- **AppKey terminology collision.** `glossary.mdx` defines `app_key` = bytecode blob_id; code's `AppKey = (package, signerId)`. Reconcile. `crates/primitives/src/application.rs:220`.
- **Minor:** `upgrades.mdx:118` Convergent row omits `SortedMap`/`SortedSet` (code classifies them Convergent).

## 1. Big undocumented subsystems (P1)

- **App signing + `.mpk` bundle format.** JCS-canonical manifest, `ed25519`, `did:key` signerId, signed vs dev-unsigned install split, multi-service `services[]` + `--service`. Entirely undocumented. → `protocol/upgrades.mdx` + `build/advanced-sdk.mdx` (+ a bundle-format page).
- **Auto-follow machinery.** Dangling cross-refs ("see Governance") point at content that doesn't exist. The two flags, event-driven auto-join, backfill (1000 cap), 20 joins/min token bucket, inherited Open-anchor rule, `leave_context` opt-out. This is how membership becomes data replication ("joined but nothing synced"). `crates/context/src/auto_follow.rs`. → new section in `protocol/governance.mdx`.
- **Authored-data migration is per-owner.** Whole-root `#[app::migrate]` does NOT convert `Authored*` entries; an auto-generated signed `migrate_my_entries` RPC must be called by each owner. Silent-correctness footgun. `crates/sdk/macros/src/state.rs:962`. → `build/migrations.mdx`.
- **A "hot" peer can never serve a snapshot.** Any write during transfer aborts it (`InvalidBoundary`) → a node bootstrapping against a busy peer retries forever. `crates/node/src/sync/snapshot.rs:152,214`. → `operate/troubleshooting.mdx`.
- **Networking reconnect story.** Gossipsub mesh tuning (2/4/8, not libp2p defaults) coupled to the readiness gate; `flood_publish=true`; per-overlay demand-driven rendezvous (not one global ns); persistent peer-address cache (24h TTL, on-disk); liveness ping reaping (3 fails → force close). `crates/network/...`. → `protocol/networking.mdx` + `operate/networking.mdx`.
- **HTTP client contract.** CORS exposes `x-auth-error/user/permissions`, 401 `token_expired`; WS auth at upgrade + `?token=` fallback; SSE skip-on-disconnect + `Last-Event-ID` + 24h session TTL. Integrator-blocking. `crates/server/...`. → `operate/admin-api.mdx`.
- **`/metrics` is unauthenticated even with embedded auth on.** Security-relevant. `crates/server/src/service_mounts.rs:62`. → `operate/observability.mdx`/`security.mdx`.
- **Startup version check phones home to GitHub** (~10% of starts, no opt-out). Air-gapped operators. `crates/merod/src/version.rs:40`. → `operate/merod.mdx`.

## 2. Important missing details (P2)

- **Sync numeric caps** (no sync-caps section in `limits.mdx`): uninitialized delta buffer 10k FIFO; governance drain 16 attempts then drop; LevelWise/HashComparison caps (per-level, per-request truncation, per-session). `delta_buffer.rs`, `level_sync.rs`. → `protocol/limits.mdx`.
- **Sync observability**: periodic-sync backoff `2^min(fail,8)`→256s (distinct from reconcile 30s→30min); readiness FSM 5 tiers + constants; `sync_status` phases (Syncing/Idle/BackingOff/WaitingForPeers). → `operate/observability.mdx` + `protocol/sync.mdx`.
- **Upgrade internals**: the upgrade gate freezes writes mid-eager-upgrade (reads from pre-migration root, side-effecting calls refused); activation-marker ladder replay (multi-version catch-up hops v1→v2→v3); HLC fence is absorb-don't-drop and keys on the loaded reader, not the target. `crates/context/...`. → `protocol/upgrades.mdx`.
- **Storage marker columns** (operator debugging of stuck upgrades): `ContextMigrationFailed` (kind 1/2), `ContextExecutingBlob` vs `ContextActivatedBlob`, `ContextResyncRequested`, `ApplicationPreviousBlob`. `crates/store/src/types/context.rs`. → `protocol/storage.mdx` + `operate/runbooks.mdx`.
- **Snapshot apply semantics**: full REPLACE (local-only keys deleted); bypasses `apply_action` (no nonce/CRDT — sound only for fresh/force); never ships rotation logs (Auxiliary rejected, rebuilt from delta replay). `snapshot.rs`. → `protocol/sync-internals.mdx`/`security-model.mdx`.
- **KMS supply-chain**: Sigstore (Rekor/Fulcio) release-policy verification pinned to a GitHub identity; `USE_ENV_POLICY` air-gap toggle; `MERO_KMS_RELEASE_TAG`/`_VERSION` pinning. `crates/merod/src/kms_policy.rs`. → `operate/security.mdx`.
- **Startup self-heal sweeps**: redrive-stranded-governance-ops (key-delivery race, fixpoint, 64 passes); eager-upgrade propagator re-spawn on restart; self-purge completion sweep. `crates/context/...`. → `protocol/governance.mdx`.
- **JS runtime surface**: `js_crdt_*`/`js_user_storage_*` host fns with the `-1`/register error ABI; the JS storage-bridge fns (`persist_root_state`, `apply_storage_delta`) that make JS produce a valid root. `crates/runtime/.../js_collections.rs`. → `build/host-functions.mdx`/`protocol/execution.mdx`.
- **Tombstone GC loop** (12h, distinct from `dag_compaction`). `crates/node/src/gc.rs`. → `operate/config.mdx`.
- **Two-layer signature model**: envelope sig (domain `b"calimero/delta/1"`) vs per-action sigs. → `protocol/security-model.mdx`.
- **Membership peer-scoring "why"** (cold-start-safe: only ≥0 scores, all thresholds ≤0). → `protocol/networking.mdx`.

## 3. Long tail (P3) — selected

- Temporal staging + one-WriteBatch cross-CF atomicity; rotation-log-as-children; runtime deterministic-id rekey + `DeleteRef` tombstone; Merkle-invisible `schema_version` tag (storage internals).
- `tracing` integration + WARN default + `env::set_log_level`; method-arg JSON contract (`deny_unknown_fields`); reserved `__calimero` prefix; `#[app::view]` post-exec enforcement; state-lint rejects interior mutability; `#[app::private]` alias blind spot (storage internals / sdk).
- ABI in `calimero_abi_v1` custom section, hashed with bytecode; downgrade lint fail-closed; guarded wrappers normalize to SharedStorage (ABI).
- Beacon debounce/anti-abuse; wedge-watchdog +10s; targeted `context sync` syncs all; deferred root-merge through WASM; crash-recovery sync marker; `force` resync disables I5+fence (sync internals).
- Auth secret auto-rotation (24h/48h grace); challenge TTL 300s + client-key rotation; `node_url` bound to Host header (auth).
- Specialized-node-invite wire protocol; AutoNAT v2 reachability state machine; blob discovery via custom Kad records not providers; `crates/network/PROTOCOLS.md` is stale in several places.
