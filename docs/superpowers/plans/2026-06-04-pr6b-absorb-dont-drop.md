# PR-6b — Absorb-Don't-Drop (Straggler Safety) — Line-Level TDD Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Branch this work off `feat/2539-pr6a-migration-v2` (PR-6a), per the standing test-placement rule (PR-6b depends on the `migration_v2` flag + `should_block` gate from 6a).

**Goal:** No straggler delta is ever silently dropped. A node offline across a migration window (including one still running the **v1 binary**) is *absorbed* on reconnect: its original signed bytes are persisted durably, then replayed **verbatim** through the receiver's current wasm `__calimero_sync_next` once the receiver's binary advances — never translated (translating bytes breaks each `Action`'s signature). Coverage spans the gossip fence **and** the HashComparison / LevelSync / snapshot sync-repair paths. The fence keys on the node's **locally-loaded binary/reader version**, not the replicated `GroupMeta.app_key`.

**Spec:** `docs/superpowers/specs/2026-06-03-pr6-expand-contract-design.md` — read §2, §3 decision 5/6, §5 "PR-6b", §7 invariants 3, §9 O2/O3, §12 6b file-touch map first. This plan honors the 8-lens adversarial-review fixes folded into that spec verbatim.

**Train context:** 6a (DONE on branch) removed the cluster-wide write-freeze behind `migration_v2`; **6b is the safety net that makes no-freeze safe**, and is the gate on flipping `migration_v2` default-on (see Task 6b.8). Stack: 6a → **6b** → 6c → 6d.

**Tech Stack:** Rust workspace (`crates/{store,governance-store,node,node/primitives,context}`), borsh, RocksDB columns, wasmtime, merobox e2e (`workflows/app-migration/`). #2674's `TestHost` (`crates/sdk/src/testing.rs`, `migrate`/`read_raw`/`call_as`) is available for unit-testing the O2 replay-then-refold convergence; it does NOT overlap the node/store work.

---

## Grounding — confirmed anchors (path:line, current tree)

| Concern | Anchor |
|---|---|
| Pure fence rule | `crates/context/src/hlc_fence.rs:20` `should_fence(delta_app_key, ctx_app_key, delta_hlc, cascade_hlc)` |
| Store-aware fence | `crates/context/src/hlc_fence.rs:31` `delta_is_fenced` — resolves `meta.app_key` (the **replicated** key, the O3 bug) + `cascade_hlc` |
| Fence DROP site | `crates/node/src/handlers/state_delta/mod.rs:661-678` — `if delta_is_fenced(..) { record_delta_outcome("fenced_stale_schema"); return Ok(()); }` (the silent drop 6b replaces) |
| `BufferedDelta` shape | `crates/node/primitives/src/delta_buffer.rs:87-142` — `#[derive(Debug, Clone)]` only, **NO Borsh**; carries `source_peer: libp2p::PeerId`, `governance_position: Option<GovernancePosition>`, `producing_app_key: Option<[u8;32]>`, every replay field |
| Buffered-delta build at fence path | `crates/node/src/handlers/state_delta/mod.rs:719-734` (the snapshot-sync buffer construction we mirror for absorb) |
| Replay path (verbatim) | `crates/node/src/delta_store.rs:352-380` — `StorageDelta::CausalActions { actions: delta.payload, .. }` → `context_client.execute(.., "__calimero_sync_next", artifact, ..)` |
| Signature breaks on translate | `crates/storage/src/action.rs:164` `payload_for_signing()` hashes `(id, data, storage_type, sig_data)` — any byte rewrite invalidates it. Confirms "replay verbatim, no translate." |
| Recovery-scan model | `crates/governance-store/src/upgrades.rs:50-67` `enumerate_in_progress` (+ `collect_keys_with_prefix`) — the template for `enumerate_pending` |
| Store `Column` enum | `crates/store/src/db.rs:17-42` (`#[non_exhaustive]`, `EnumIter`, `AsRefStr`) — add `AbsorbBuffer` here |
| Key pattern (prefix‖components) | `crates/store/src/key/group/mod.rs:20-54, 390-428` — `GroupPrefix=U1`, `GroupIdComponent=U32`, `GenericArray::from([PREFIX]).concat(..)`; `AsKeyParts::column()` selects the CF |
| Metric helper | `crates/node/src/node_metrics.rs:538` `record_delta_outcome(outcome: &str)` over `DeltaApplyLabels { outcome: String }` (single label) |
| Wire leaf type (no schema today) | `crates/node/primitives/src/sync/hash_comparison.rs:290` `TreeLeafData`, `:333` `LeafMetadata { crdt_type, hlc_timestamp, created_at, version, authorization }` — **carries NO app-schema/binary version** |
| Sync apply (LWW store of leaf) | `crates/node/src/sync/helpers.rs:227` `apply_leaf_with_crdt_merge(context_id, leaf)` |
| Loaded app resolution | `crates/context/src/handlers/execute/mod.rs:101` `current_application_id = context.meta.application_id` (the **loaded** app; the O3 "loaded reader version" source) vs `:2290` `resolve_producing_app_key` → `GroupMeta.app_key` |

**Key blocker surfaced by grounding (resolved in Task 6b.1):** `BufferedDelta` is NOT Borsh-serializable (`PeerId`, `GovernancePosition` don't derive cleanly). Persisting it requires an explicit serializable mirror (`AbsorbRecord`) with a hand-written `from(&BufferedDelta)` / `into_buffered()` using `PeerId::to_bytes()/from_bytes()`. Do NOT attempt `#[derive(Borsh)]` on `BufferedDelta` itself.

---

## O2 / O3 resolutions (explicit, locked here)

**O2 — folding an absorbed v1 delta into a v2 context = replay-verbatim-then-deterministic-refold.**
The absorbed delta's bytes are the v1-schema signed `Action`s the straggler authored. We do NOT translate them (that breaks `payload_for_signing`, `action.rs:164`). Instead: on binary advance, we re-feed the **original** `StorageDelta::CausalActions` artifact (reconstructed byte-identically from the `AbsorbRecord`) through `__calimero_sync_next` (`delta_store.rs:373`). The **now-loaded v2 wasm** applies those actions via its own `Mergeable::merge` against the v2 root — i.e. the v2 binary is the only thing that interprets the bytes, and it does so under the existing deterministic merge-mode machinery. Convergence holds because: (a) every replayed action is causally ordered by its preserved `hlc`/`parents`; (b) the v2 root is the deterministic whole-root rebuild from PR-6a, so all nodes that absorb the same straggler delta re-derive a byte-identical v2 root (whole-root refold). There is **no version-aware host-side merge** and **no per-delta translation table** — the host stays schema-agnostic. (Identity-gated entries are out of scope for 6b; they cannot be replayed by a non-owner and are handled by 6c's owner-driven re-write.)

**O3 — fence on the LOADED binary/reader version, not `GroupMeta.app_key`.**
Under LazyOnAccess the governance `GroupMeta.app_key` advances to v2 for *all* members at cascade-apply, but each node's wasm binary swaps **lazily** (on next execute via `maybe_lazy_upgrade`, `execute/mod.rs:2202`). So `delta_is_fenced` resolving `meta.app_key` (`hlc_fence.rs:41`) is wrong: a node still on the v1 binary sees `ctx_app_key = v2` and would *fail to fence* (or fence the wrong way) and could be fed unreadable v2 bytes. **Resolution:** introduce `loaded_reader_app_key(store, context_id) -> Option<[u8;32]>` derived from the **loaded** application (`context.meta.application_id` → its blob/app_key), thread it as a new `loaded_app_key` parameter into `should_fence`/`delta_is_fenced` and into the sync-apply gate. The fence/buffer decision becomes: *the receiver lacks a reader for the incoming schema* ⟺ `incoming_app_key != loaded_app_key`. When that holds, **buffer** (absorb), never drop, never store. `should_fence` keeps its pure shape; we add the loaded-key argument and the call sites pass the loaded key instead of `meta.app_key`. (`GroupMeta.app_key` is retained only to know the *target*, used by the drain to decide when the binary has caught up.)

---

## Task list (each TDD: failing test → minimal impl → cargo → commit)

- **6b.1** `AbsorbRecord` serializable mirror + `Column::AbsorbBuffer` + `AbsorbBufferKey` (`ctx‖app_key‖delta_id`).
- **6b.2** `AbsorbRepository` (save/load/delete/enumerate_pending) + unit round-trip + prefix-scan tests.
- **6b.3** `loaded_reader_app_key` resolver + thread loaded key into `should_fence`/`delta_is_fenced` (O3).
- **6b.4** Replace the gossip-fence silent drop with absorb-persist (`state_delta/mod.rs:661-678`); metric label `absorbed_for_migration`; idempotent via `delta_id` key.
- **6b.5** Drain-on-advance: replay original bytes verbatim through `__calimero_sync_next`; delete on success; idempotent re-drain (O2).
- **6b.6** Startup recovery scan wires `enumerate_pending` into node boot (mirror `enumerate_in_progress` recovery).
- **6b.7** Sync-repair coverage: add `schema_app_key` to `LeafMetadata`/`TreeLeafData`; `apply_leaf_with_crdt_merge` declines+buffers a leaf whose schema ≠ loaded reader.
- **6b.8** Flip `migration_v2` default ON + add live merobox e2e (writes-available, straggler-absorbed, v1-binary-not-corrupted).

---

## Task 6b.1 — `AbsorbRecord` mirror + `Column::AbsorbBuffer` + key type

**Files:**
- Modify: `crates/store/src/db.rs:42` (add `AbsorbBuffer` to the `Column` enum).
- Create: `crates/store/src/key/absorb.rs` (`AbsorbBufferKey`, prefix `0x4A`). Grounding-confirmed: `group/mod.rs` prefixes run a **contiguous `0x20`–`0x3C`** (`GROUP_META_PREFIX=0x20` … `GROUP_LOCAL_GOV_NONCE_WINDOW_PREFIX=0x3C`), so `ABSORB_BUFFER_PREFIX = 0x4A` is free. Because `AbsorbBuffer` is its **own Column/CF** (not `Column::Group`), the prefix byte only has to be distinct *within that CF*, so collision risk is nil — but keep `0x4A` for grep-ability.
- Modify: `crates/store/src/key.rs` (or the key `mod.rs`) to `pub mod absorb;`.
- Create: `crates/governance-store/src/absorb_record.rs` (`AbsorbRecord` — Borsh-serializable mirror of `BufferedDelta`).

- [ ] **Step 1 — Failing test (key round-trip):** in `crates/store/src/key/absorb.rs` `#[cfg(test)]`, assert `AbsorbBufferKey::new(ctx, app_key, delta_id)` round-trips its three 32-byte components and reports `Column::AbsorbBuffer`:
```rust
#[test]
fn absorb_key_round_trips_three_components() {
    let k = AbsorbBufferKey::new([1; 32], [2; 32], [3; 32]);
    assert_eq!(k.context_id(), [1; 32]);
    assert_eq!(k.producing_app_key(), [2; 32]);
    assert_eq!(k.delta_id(), [3; 32]);
    assert_eq!(AbsorbBufferKey::column(), Column::AbsorbBuffer);
}
```
- [ ] **Step 2 — Run, expect FAIL** (`cargo test -p calimero-store absorb_key`) — type/variant missing.
- [ ] **Step 3 — Implement:** add `AbsorbBuffer` to `Column` (`db.rs:42`). Define `AbsorbBufferKey(Key<(AbsorbPrefix, ContextIdComp, AppKeyComp, DeltaIdComp)>)` modeled on `GroupUpgradeKey` (`group/mod.rs:392`): `AbsorbPrefix: U1`, three `U32` components, built via `GenericArray::from([ABSORB_BUFFER_PREFIX]).concat(..).concat(..).concat(..)`. Impl `AsKeyParts` (`column() => Column::AbsorbBuffer`) + `FromKeyParts` (`Infallible`) + the three accessor slices + `Debug`.
- [ ] **Step 4 — Failing test (record mirror):** in `absorb_record.rs`, assert `AbsorbRecord` borsh-round-trips and converts to/from a `BufferedDelta` losslessly (PeerId via `to_bytes`):
```rust
#[test]
fn absorb_record_round_trips_buffered_delta() {
    let bd = sample_buffered_delta();              // mirrors delta_buffer::tests::make_test_delta
    let rec = AbsorbRecord::from_buffered(&bd);
    let bytes = borsh::to_vec(&rec).unwrap();
    let back = AbsorbRecord::try_from_slice(&bytes).unwrap().into_buffered().unwrap();
    assert_eq!(back.id, bd.id);
    assert_eq!(back.source_peer, bd.source_peer);  // PeerId survived to_bytes/from_bytes
    assert_eq!(back.producing_app_key, bd.producing_app_key);
    assert_eq!(back.delta_signature, bd.delta_signature);
}
```
- [ ] **Step 5 — Run, expect FAIL.**
- [ ] **Step 6 — Implement `AbsorbRecord`:** a `#[derive(BorshSerialize, BorshDeserialize)]` struct holding every `BufferedDelta` field, with `source_peer: Vec<u8>` (`PeerId::to_bytes()`), `governance_position` borsh'd if it derives else a flattened mirror, `hlc` via its existing borsh repr (`HybridTimestamp`). `from_buffered(&BufferedDelta)` and `into_buffered() -> eyre::Result<BufferedDelta>` (PeerId parse can fail → `Result`). Do **not** add Borsh to `BufferedDelta` itself.
- [ ] **Step 7 — Run, expect PASS** (`cargo test -p calimero-store absorb_key && cargo test -p calimero-governance-store absorb_record`).
- [ ] **Step 8 — Commit:** `feat(store): AbsorbBuffer column + key + AbsorbRecord serializable mirror`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

## Task 6b.2 — `AbsorbRepository` (save/load/delete/enumerate_pending)

**Files:**
- Create: `crates/governance-store/src/absorb.rs` (repository).
- Modify: `crates/governance-store/src/lib.rs` (`pub mod absorb; pub use absorb::AbsorbRepository;`).

- [ ] **Step 1 — Failing test:** mirror `upgrades.rs:107-166`. Round-trip + delete + `enumerate_pending(context)` filters to one context:
```rust
#[test]
fn enumerate_pending_returns_only_this_context() {
    let store = test_store();
    let repo = AbsorbRepository::new(&store);
    let ctx_a = ContextId::from([0xAA; 32]);
    let ctx_b = ContextId::from([0xBB; 32]);
    repo.save(&ctx_a, [9; 32], &sample_record([1; 32])).unwrap();
    repo.save(&ctx_a, [9; 32], &sample_record([2; 32])).unwrap();
    repo.save(&ctx_b, [9; 32], &sample_record([3; 32])).unwrap();
    let pending = repo.enumerate_pending(&ctx_a).unwrap();
    assert_eq!(pending.len(), 2);
}

#[test]
fn save_is_idempotent_on_delta_id() {
    let store = test_store();
    let repo = AbsorbRepository::new(&store);
    let ctx = ContextId::from([0xAA; 32]);
    repo.save(&ctx, [9; 32], &sample_record([1; 32])).unwrap();
    repo.save(&ctx, [9; 32], &sample_record([1; 32])).unwrap(); // same delta_id key overwrites
    assert_eq!(repo.enumerate_pending(&ctx).unwrap().len(), 1);
}
```
- [ ] **Step 2 — Run, expect FAIL** (`cargo test -p calimero-governance-store absorb`).
- [ ] **Step 3 — Implement:** `AbsorbRepository<'a> { store }` with:
  - `save(&self, ctx, producing_app_key, rec) -> Result<()>` → `handle.put(&AbsorbBufferKey::new(ctx.into(), app_key, rec.id), rec)` (idempotent because the `delta_id` is in the key).
  - `load(&self, ctx, app_key, delta_id) -> Result<Option<AbsorbRecord>>`.
  - `delete(&self, ctx, app_key, delta_id) -> Result<()>`.
  - `enumerate_pending(&self, ctx) -> Result<Vec<(([u8;32],[u8;32]), AbsorbRecord)>>` — `collect_keys_with_prefix` (`upgrades.rs:51`) over `ABSORB_BUFFER_PREFIX ‖ ctx`, returning `((app_key, delta_id), rec)`. (The prefix-scan needs a per-context lower bound: build the seek key from `prefix‖ctx‖0..`; add a `collect_keys_with_prefix`-style helper taking a longer prefix, or filter the full-column scan on `key.context_id() == ctx` — match whichever `collect_keys_with_prefix` signature exists.)
  - `enumerate_all_contexts(&self) -> Result<Vec<ContextId>>` for the startup scan (distinct contexts with pending absorbs).
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(governance-store): AbsorbRepository save/load/delete/enumerate_pending`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

## Task 6b.3 — Loaded-reader-version resolver + thread into the fence (O3)

**Files:**
- Modify: `crates/context/src/hlc_fence.rs` (`should_fence` + `delta_is_fenced` signatures).
- Modify: `crates/context/src/handlers/execute/mod.rs` (or a small `crates/context/src/loaded_reader.rs`) — add `loaded_reader_app_key`.
- Modify: `crates/node/src/handlers/state_delta/mod.rs:662` (the `delta_is_fenced` call).

- [ ] **Step 1 — Failing test (pure rule, loaded key):** add a parameter so the rule fences when the **incoming** schema differs from the **loaded reader**, independent of `ctx_app_key`/`meta.app_key`:
```rust
#[test]
fn fences_when_incoming_differs_from_loaded_reader() {
    // delta produced under v1; node still on v1 binary, ctx target advanced to v2.
    // Must NOT fence-drop; must signal "buffer" because reader can read v1.
    assert_eq!(fence_decision([1;32], /*loaded*/[1;32], /*target*/[2;32], hlc_after_zero(), Some(zero())), FenceDecision::Apply);
    // delta produced under v2; node still on v1 reader → cannot read → BUFFER.
    assert_eq!(fence_decision([2;32], /*loaded*/[1;32], /*target*/[2;32], hlc_after_zero(), Some(zero())), FenceDecision::Buffer);
}
```
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement:** introduce `enum FenceDecision { Apply, Buffer, Drop }` (replaces the bare `bool`, but keep `should_fence` as a thin compat shim if other callers exist — `rg should_fence crates/`). New rule:
  - incoming `== loaded_app_key` → `Apply` (readable now).
  - incoming `!= loaded_app_key` AND it is a post-cascade stale-schema case → `Buffer` (absorb; the readable-later case).
  - the legacy non-migration fence (`cascade_hlc == None`) → `Apply` (unchanged; never fenced).
  `loaded_reader_app_key(store, context_id)`: resolve `context.meta.application_id` (`execute/mod.rs:101`) and map to its app_key/blob_id; fall back to `GroupMeta.app_key` only when no context meta. `delta_is_fenced` → `delta_fence_decision`, now resolving `loaded_app_key` instead of `meta.app_key` at `hlc_fence.rs:41`, and returning `FenceDecision`.
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-context hlc_fence`). Keep the four existing `hlc_fence` tests green (adapt them to `FenceDecision`).
- [ ] **Step 5 — Commit:** `feat(context): fence on loaded reader version, return FenceDecision (O3)`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

## Task 6b.4 — Absorb instead of silent-drop at the gossip fence

**Files:**
- Modify: `crates/node/src/handlers/state_delta/mod.rs:661-678`.
- Modify: `crates/node/src/node_metrics.rs:538` area — accept the `absorbed_for_migration` outcome label (no schema change; it's a string label, just a new value).

- [ ] **Step 1 — Failing test:** integration-style test in `state_delta` test module: given a context past a cascade boundary and a delta whose `producing_app_key` is **not the loaded reader** (Buffer decision), the handler persists an `AbsorbRecord` (not drop) and records `record_delta_outcome("absorbed_for_migration")`. Assert via `AbsorbRepository::enumerate_pending(ctx).len() == 1` and no error returned.
```rust
#[test]
fn buffer_decision_persists_absorb_record_not_drop() {
    // arrange store+context with cascade_hlc set, loaded reader = v1, delta app_key = v2
    handle_state_delta(msg_with_app_key([2;32])).unwrap();
    let pending = AbsorbRepository::new(&store).enumerate_pending(&ctx).unwrap();
    assert_eq!(pending.len(), 1, "v2 delta to a v1-reader node must be absorbed, not dropped");
}
```
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement:** at `:661`, switch on `delta_fence_decision(..)`:
  - `Apply` → fall through (current behavior).
  - `Drop` → keep `record_delta_outcome("fenced_stale_schema"); return Ok(())` (non-migration fences still drop).
  - `Buffer` → build a `BufferedDelta` exactly as at `:719-734`, `AbsorbRepository::save(ctx, producing_app_key, AbsorbRecord::from_buffered(&bd))`, `record_delta_outcome("absorbed_for_migration")`, `return Ok(())`. Idempotent: the `delta_id` is the key, so a re-delivered straggler delta overwrites rather than duplicates.
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(node): absorb stale-schema deltas to AbsorbBuffer instead of dropping`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

## Task 6b.5 — Drain-on-advance: replay original bytes verbatim (O2)

**Files:**
- Create: `crates/node/src/handlers/absorb_drain.rs` (or extend `delta_store.rs`).
- Modify: `crates/context/src/handlers/execute/mod.rs:2202` `maybe_lazy_upgrade` success path — after a context's binary advances (lazy upgrade ran), trigger the absorb drain for that context.

- [ ] **Step 1 — Failing test (TestHost convergence, O2):** using `crates/sdk/src/testing.rs` `TestHost` (#2674), prove replay-verbatim-then-refold converges: install v1 state, capture a v1-authored delta's bytes, `migrate` to v2, replay the captured bytes via the v2 reader, and assert the resulting root equals the root of a node that received the same delta pre-migration then migrated (`assert_migrate_converges`). This pins O2's "no translation, deterministic refold."
```rust
#[test]
fn absorbed_v1_delta_refolds_into_v2_root_deterministically() {
    // host_a: v1 -> migrate v2 -> replay straggler bytes
    // host_b: v1 + apply straggler bytes -> migrate v2
    assert_eq!(host_a.root_hash(), host_b.root_hash());
}
```
- [ ] **Step 2 — Failing test (drain mechanics):** unit test that `drain_absorbed(ctx, store, context_client)` loads each pending `AbsorbRecord`, reconstructs the **byte-identical** `StorageDelta::CausalActions` artifact, calls `__calimero_sync_next` (mock/`TestHost`), and `delete`s the record on success; a record whose schema still ≠ loaded reader is **left in place** (re-drained later).
- [ ] **Step 3 — Run, expect FAIL.**
- [ ] **Step 4 — Implement `drain_absorbed`:** `enumerate_pending(ctx)` → for each `((app_key, delta_id), rec)`: skip if `app_key != loaded_reader_app_key(ctx)` (binary hasn't caught up); else reconstruct the exact artifact from the record (`StorageDelta::CausalActions { actions: rec.payload, delta_id: rec.id, delta_hlc: rec.hlc, effective_writers }`, mirroring `delta_store.rs:352-358` — resolve `effective_writers` the same way), feed through the existing `__calimero_sync_next` execute (`delta_store.rs:373`), then `delete(ctx, app_key, delta_id)`. **No translation of `rec.payload`** (preserves `payload_for_signing`, `action.rs:164`). Idempotent: re-running after a crash mid-drain just re-replays survivors (replay is convergent, delete-after-success). Hook the drain call into the `maybe_lazy_upgrade` success site (`execute/mod.rs:2202` caller) so a binary advance immediately drains.
- [ ] **Step 5 — Run, expect PASS** (`cargo test -p calimero-node absorb_drain` + the sdk TestHost test).
- [ ] **Step 6 — Commit:** `feat(node): drain absorbed deltas by verbatim replay on binary advance (O2)`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

## Task 6b.6 — Startup recovery scan

**Files:**
- Modify: node boot path that already calls `UpgradesRepository::enumerate_in_progress` for crash recovery (`rg "enumerate_in_progress" crates/node crates/server` to locate the startup site).

- [ ] **Step 1 — Failing test:** seed an `AbsorbRecord` directly into the store, run the recovery routine, assert it enqueues a drain for that context (or, if the reader hasn't advanced, leaves it pending and logs it). Assert no panic, idempotent across two recovery calls.
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement:** at the existing crash-recovery site, after the upgrades scan, call `AbsorbRepository::enumerate_all_contexts()` and, per context, attempt `drain_absorbed` (records whose schema ≠ loaded reader stay pending until the binary advances). Mirror the `enumerate_in_progress` recovery shape exactly.
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(node): startup recovery scan drains pending AbsorbBuffer`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

## Task 6b.7 — Sync-repair coverage (HashComparison / LevelSync / snapshot)

**The wire gap (requirement 4):** `TreeLeafData`/`LeafMetadata` (`hash_comparison.rs:290,333`) carry `crdt_type, hlc_timestamp, created_at, version, authorization` but **NO app-schema/binary version**. A v1-binary receiver currently has no way to know an incoming leaf was authored under v2 → it would LWW-store unreadable v2 bytes (`apply_leaf_with_crdt_merge`, `helpers.rs:227`). We must add a `schema_app_key: Option<[u8;32]>` to `LeafMetadata` and have senders stamp it.

**Files:**
- Modify: `crates/node/primitives/src/sync/hash_comparison.rs:333` (`LeafMetadata` + builder).
- Modify: leaf senders — `get_local_tree_node` / `collect_leaves_recursive` in `hash_comparison.rs` + `hash_comparison_protocol.rs` (stamp `schema_app_key = loaded reader`), plus `level_sync.rs` and `snapshot.rs` senders.
- Modify: `crates/node/src/sync/helpers.rs:227` `apply_leaf_with_crdt_merge` (decline+buffer on schema mismatch).

- [ ] **Step 1 — Failing test (wire field):** assert `LeafMetadata::new(..).with_schema_app_key([7;32]).schema_app_key == Some([7;32])` and that `schema_app_key` defaults `None` (legacy peers). Borsh round-trip of `TreeLeafData` preserves it.
- [ ] **Step 2 — Failing test (apply gate):** `apply_leaf_with_crdt_merge` given a leaf with `schema_app_key = Some(v2)` while the loaded reader is v1 returns a `Buffer`/decline outcome and writes an `AbsorbRecord`-equivalent (or returns a typed `LeafOutcome::Buffered`) **instead of** storing the leaf. A matching/`None` schema applies as today.
```rust
#[test]
fn leaf_with_future_schema_is_buffered_not_stored() {
    let leaf = leaf_with_schema([2;32]); // v2
    // loaded reader = v1
    let outcome = apply_leaf_with_crdt_merge_gated(ctx, &leaf, /*loaded*/[1;32]).unwrap();
    assert!(matches!(outcome, LeafOutcome::Buffered));
    assert!(Index::get_index(Id::new(leaf.key)).unwrap().is_none(), "must not persist unreadable bytes");
}
```
- [ ] **Step 3 — Run, expect FAIL.**
- [ ] **Step 4 — Implement:**
  - Add `schema_app_key: Option<[u8;32]>` to `LeafMetadata` (`#[borsh]`, defaulted `None`) + `with_schema_app_key`. **Merkle-invisible:** confirm it does NOT enter any leaf/own hash (it's transport metadata only) — grep the hash sites in `helpers.rs`/`interface.rs` to be sure it isn't folded into `own_hash`.
  - Senders stamp `schema_app_key = loaded_reader_app_key(ctx)` when building leaves.
  - `apply_leaf_with_crdt_merge` (rename internal to `_gated`, take `loaded_app_key`): if `leaf.metadata.schema_app_key` is `Some(k)` and `k != loaded_app_key`, persist via `AbsorbRepository` (reusing the absorb path — a leaf-shaped absorb, or a thin `BufferedDelta`-equivalent) and return `LeafOutcome::Buffered`; otherwise apply as today. The HC DFS / LevelSync / snapshot callers (`hash_comparison_protocol.rs`, `level_sync.rs`, `snapshot.rs`) must propagate the `Buffered` outcome (decline + continue) rather than treating it as applied — so a v1 node syncing a v2 subtree never corrupts.
- [ ] **Step 5 — Run, expect PASS** (`cargo test -p calimero-node sync::helpers && cargo test -p calimero-node-primitives leaf`).
- [ ] **Step 6 — Commit:** `feat(node): carry schema_app_key on sync leaves; buffer future-schema leaves`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

## Task 6b.8 — Flip `migration_v2` default ON + live merobox e2e

**Rationale:** Per the train flag-flip rule, `migration_v2` may go default-on once 6a (no-freeze) **and** 6b (absorb safety net) have landed. At default-on, the new scenarios run natively in the existing app-migration matrix — **no merobox feature flag needed**.

**Files:**
- Modify: the `migration_v2` default (`crates/context/...ContextManagerConfig` — set from 6a) flip `false` → `true`; remove/relax any merod external-config gating that forced it off.
- Create: `workflows/app-migration/24-straggler-absorbed.yml`, `25-v1binary-not-corrupted.yml` (writes-available is already scenario `23` from 6a — re-run it under default-on).
- Modify: `.github/workflows/app-migration-e2e.yml` matrix (+ `24`, `25`).

- [ ] **Step 1 — Failing test (default flip):** update `migration_v2_flag_defaults_off` (6a.1) to `migration_v2_flag_defaults_on` asserting `true`; run flag-off characterization tests that must now exercise the new default. Run, expect FAIL until the default is flipped.
- [ ] **Step 2 — Implement:** flip the default; ensure 6a's `should_block(true, InProgress)==false` path is now the live path.
- [ ] **Step 3 — Author `24-straggler-absorbed.yml`:** 2-node namespace migration; node B offline across the **entire** migration window; B authors a v1 write while offline (or holds an un-acked v1 delta); B reconnects → assert via `assert_log_present "absorbed_for_migration"` (or the absorb metric) and that B's write **converges** into the v2 root on both nodes (`assert_log_absent "Dropping state delta"`).
- [ ] **Step 4 — Author `25-v1binary-not-corrupted.yml`:** a node pinned on the **v1 binary** syncs (HashComparison) with v2 nodes; assert `assert_log_absent` of any deserialization/`Cannot change StorageType`/panic, assert the v2 leaves were **buffered** (`assert_log_present "Buffered"`/absorb metric), and that after the v1 node lazily upgrades it converges. Models on `21-reads-available-during-upgrade.yml`.
- [ ] **Step 5 — Run locally:** `merobox bootstrap run workflows/app-migration/24-straggler-absorbed.yml --image merod:local --e2e-mode` (and `25`), expect PASS. Re-run the full `00..23` matrix to confirm default-on doesn't regress.
- [ ] **Step 6 — Commit:** `feat(context): default migration_v2 on; e2e straggler-absorbed + v1-binary-not-corrupted (24,25)`
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```

---

## Verification gate (run before declaring 6b complete)

- [ ] `cargo test -p calimero-store -p calimero-governance-store -p calimero-context -p calimero-node -p calimero-node-primitives` — all green.
- [ ] `cargo test -p calimero-context hlc_fence` — the four original fence tests adapted to `FenceDecision`, all green.
- [ ] `cargo clippy --workspace --all-targets` — no new warnings (the `#[non_exhaustive] Column` `EnumIter`/`AsRefStr` may force a match-arm update at every `Column` exhaustive match — `rg "Column::SortedIndex =>"`/`match col` to find them).
- [ ] merobox `24`, `25`, `23` PASS locally under default-on; `00..22` unchanged.
- [ ] Grep guard: `rg 'return Ok\(\)' crates/node/src/handlers/state_delta/mod.rs` — confirm the migration-fence branch no longer silent-drops (only the non-migration `Drop` arm does).

## Self-review / coverage map

- Req 1 (durable column + repo + recovery) → 6b.1, 6b.2, 6b.6.
- Req 2 (replace drop with persist-original; verbatim replay; metric `absorbed_for_migration`; idempotent via delta_id) → 6b.4, 6b.5.
- Req 3 (fence on loaded binary version, O3) → 6b.3 (+ threaded into 6b.4, 6b.7).
- Req 4 (sync-repair coverage; wire leaf has no schema today → add `schema_app_key`) → 6b.7.
- Req 5 (flip default on; live e2e, no merobox feature) → 6b.8.
- O2 (replay-verbatim-then-deterministic-refold) → resolved §"O2/O3" + 6b.5 (TestHost convergence test).
- O3 (loaded-binary-version threading) → resolved §"O2/O3" + 6b.3.

## Blocking unknowns (resolve at task entry, not now)

1. **`loaded_reader_app_key` source-of-truth — RESOLVED during grounding.** `context.meta.application_id` is version-*stable*; the **app_key** (schema discriminator) is `blob_id(loaded bytecode)`. It IS cheaply available off the store: `app_key = *app_meta.bytecode.blob_id().as_ref()` is exactly how `GroupMeta.app_key` is produced (`crates/context/src/handlers/upgrade_group.rs:174,327,983`), and `ApplicationMeta` carries `bytecode: key::BlobMeta` (`crates/store/src/types/application.rs:20`). So `loaded_reader_app_key(store, ctx) = ApplicationMeta(context.meta.application_id).bytecode.blob_id()` — no extra marker row, no `get_module` round-trip. (Fallback to `GroupMeta.app_key` only if the loaded `ApplicationMeta` row is missing.) Verify `BlobMeta::blob_id()` is reachable from the context crate at task entry.
2. **`GovernancePosition` Borsh.** Verify it derives Borsh for `AbsorbRecord`; if not, flatten it in the mirror (it's `Option`, legacy `None` is common).
3. **Leaf-absorb shape.** 6b.7 buffers a *leaf* (not a full delta). Decide whether to (a) reuse `AbsorbRecord` with a leaf-vs-delta tag, or (b) re-request the leaf's originating delta on binary advance. Leaning (a) — store the leaf bytes + `schema_app_key` and re-apply via `apply_leaf_with_crdt_merge_gated` once the reader advances; finalize at 6b.7 entry.
4. **`collect_keys_with_prefix` signature** — confirm it accepts a >1-byte prefix (need `prefix‖ctx`) for `enumerate_pending`; if it only takes the 1-byte column prefix, filter on `key.context_id()`.
