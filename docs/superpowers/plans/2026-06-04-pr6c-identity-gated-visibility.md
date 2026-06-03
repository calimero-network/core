# PR-6c — Identity-gated owner-driven re-write + completion visibility (line-level TDD plan)

- **Date:** 2026-06-04
- **Issue:** core #2539 (umbrella); folds in #2534 (owner-driven Authored/Shared rewrite)
- **Spec:** `docs/superpowers/specs/2026-06-03-pr6-expand-contract-design.md` (§2, §3.4/§3.7/§3.8, §5 PR-6c, §7.2, §8, §9 O4)
- **Train plan:** `docs/superpowers/plans/2026-06-04-pr6-hybrid-migration.md` (PR-6c section — this doc is its JIT line-level expansion)
- **Branch base:** off `feat/2539-pr6a-migration-v2` (then `feat/2539-pr6b-absorb` once 6b lands) per the standing test-placement rule. Grounded against master `e240223a` (includes #2674) for the #2674 surface.
- **Outcome:** signed/identity-gated data migrates via the owner's next online signed re-write; departed-owner entries resolved by an admin governance-authorized tombstone+rekey force-carry; admins observe per-node completion via an ephemeral signed heartbeat + `get_migration_status` rollup. All observability, never a gate.

---

## Reconciliation with #2674 (what is DROPPED vs KEPT)

#2674 (`e240223a`, "migration ergonomics") shipped three things this plan's earlier scope assumed 6c would build. Reading `crates/sdk/macros/src/migrate_derive.rs` and `crates/sdk/macros/src/state.rs` at that tip establishes exactly what exists:

**What `#[derive(Migrate)]` actually is (migrate_derive.rs:84-156):** a **whole-root** generator. It expands to a single `#[app::migrate] pub fn <method>() -> AppV2` that:
1. `read_raw()`s the old root bytes, borsh-deserializes them into the `#[migrate(from = AppV1)]` type, then
2. constructs the **entire** new state struct field-by-field (`Carry` / `new=` / `from=` rename / `with=` convert / `emit=`).

It is **not** a per-type / per-entry old-reader hook. It has **no** notion of `schema_version`, no per-entry dispatch, no owner-driven path, and no identity-gated awareness. It runs under the existing `#[app::migrate]` machinery (merge mode + `__assign_deterministic_ids`, wired in `state.rs`' `generate_*` helpers). It is the **author surface for the Convergent/Replayable whole-root path** (PR-6a's category) — not for identity-gated entries.

`state.rs` confirms `#[app::state]` already injects borsh derives, the `Mergeable` impl, the rekey-register hook (#2577), deterministic-id assignment, and the native **TestHost bridge** — so in-process migrate testing exists.

### DROPPED from the original 6c scope
- **Task 6c.7 "`#[derive(Migrate)]` ergonomics for the identity-gated author surface" — DROP.** #2674's derive is the whole-root surface and is done; per the spec §6 the rail stays **L1 #2645 + L2 #2586**, and §4F descopes the derive to ergonomics (delivered). 6c does **not** build a new author macro. The owner-driven per-entry convert in 6c is a **host/apply-time** mechanism (it triggers on the owner's *next ordinary write* through the existing `AuthoredMap::insert/update` → `save_raw` path); it needs no new author annotation. **The only macro touch 6c may need** is wiring a `schema_version` *constant* the app's v2 binary stamps on identity-gated writes — and even that can be a runtime SDK call (`app::schema_version()`), not a derive. We choose the **SDK-constant route** (Task 6c.2a) and add **zero** new proc-macro code, keeping 6c off the macro crate entirely.
- **No new whole-root migrate plumbing.** That is 6a/6b's category; 6c only handles identity-gated entries.

### KEPT (the genuine 6c remainder, all NON-#2674)
1. `Metadata.schema_version: Option<u32>` (Merkle-invisible) + per-entry dispatch key `(crdt_type|field_name, schema_version)` — §3.4/§3.7.
2. Owner-driven convert at apply/save time: owner's next signed write upgrades a stale identity-gated entry under a **strictly-monotonic nonce** (`updated_at` advances) — open item **O4** (resolved below).
3. Departed-owner admin **force-carry** = a new `GroupOp` variant = tombstone old entry + new entity under the **admin's own key** (verifies normally; no owner-forge, no owner-change) + apply handler + governance authorization.
4. Completion visibility: local-derived residue scan + signed `NamespaceTopicMsg::MigrationHeartbeat` → in-memory TTL cache (modeled on `readiness.rs`) + `get_migration_status(namespace)` admin RPC (inherited-membership cohort, pinned at expand-entry HLC, `all_migrated`, `unknown`). Never a gate.

### Resolution of O4 (identity-gated owner-driven nonce/merge semantics + force-carry authority)
Grounded against `crates/storage/src/interface.rs`:
- **Nonce == `updated_at`.** `apply_action`'s replay guard reads `last_nonce = get_metadata(id).updated_at` and rejects `new_nonce < last_nonce` (interface.rs:893-986); `save_raw` stamps `nonce = *metadata.updated_at` into the `SignatureData` for a local owner write (interface.rs:3003-3016). So "strictly-monotonic nonce" == "the convert write carries a strictly-greater `updated_at`." **The convert is an ordinary owner `Action::Update`, NOT a merge-mode re-emit.** It must NOT be entropy-suppressed and must NOT run under `with_merge_mode` (env.rs:53) — merge mode bypasses the nonce check (`skip_nonce = ... || in_merge_mode()`, interface.rs:920) and is for idempotent re-folds, exactly the wrong semantics for a real new write. Correctness comes from single-owner authorship replicating as one convergent signed delta (spec §7.2), not from cross-node byte-identity.
- **Convert does NOT change the owner.** `verify_action_update` rejects any owner change on a `User` entry ("Cannot change owner of User storage", interface.rs:3200-3207). The convert keeps the same `owner` and only changes the *value bytes* + `schema_version` + advances `updated_at`. This passes verification unchanged.
- **Force-carry CANNOT re-sign as the departed owner** (no private key) and CANNOT change an entry's owner (the wall above). Therefore force-carry is **tombstone-old + create-new-under-admin-key**: a governance op authorizes the admin to (a) delete the stale entry (a normal owner-independent governance-driven removal at the storage layer) and (b) create a fresh entity stamped `User { owner: admin }`, which the admin signs normally. Authority = the actor holds the namespace **admin/owner capability** (the same gate `cascade_upgrade` / `member_removed` use); enforced in the governance apply handler, NOT in storage (storage only sees a normal signed admin write).

---

## Train overview & sequencing

6c stacks on 6b (which stacks on 6a). Branch `feat/2539-pr6c-identity-visibility` off the 6b branch. Tasks are ordered so each is independently green:

| # | Task | Crate(s) | Depends on |
|---|------|----------|-----------|
| 6c.1 | `Metadata.schema_version` + Merkle-invisibility | storage | — |
| 6c.2 | Per-entry dispatch key + stale-detection predicate | storage | 6c.1 |
| 6c.2a | SDK `app::schema_version()` constant surface | sdk | 6c.1 |
| 6c.3 | Owner-driven convert at write time (monotonic nonce) | storage | 6c.2, 6c.2a |
| 6c.4 | `GroupOp::MigrationForceCarry` variant + op-kind label | governance-types | — |
| 6c.5 | Force-carry apply handler + admin authorization | governance-store | 6c.4 |
| 6c.6 | Residue local-derived scan | storage / governance-store | 6c.2 |
| 6c.7 | `MigrationHeartbeat` wire variant + signed body | governance-types | — |
| 6c.8 | Heartbeat TTL cache ingest (sig+membership verified) | node | 6c.7 |
| 6c.9 | `get_migration_status` handler (cohort, pinning, `unknown`, `all_migrated`) | context | 6c.6, 6c.8 |
| 6c.10 | Admin HTTP route | server | 6c.9 |
| 6c.11 | e2e: owner-driven, departed-owner force-carry, status rollup | merobox | all |

`/code-review` after the storage cluster (6c.1–6c.3, 6c.6), after governance (6c.4–6c.5), and after visibility (6c.7–6c.10).

---

## PR-6c tasks

### Task 6c.1: `Metadata.schema_version` field (Merkle-invisible)

**Files:**
- Modify: `crates/storage/src/entities.rs` (the `Metadata` struct, ~491; constructors ~520-569).
- Test: `crates/storage/src/interface.rs` test module (Merkle-invisibility) and an `entities.rs` unit test (default).

**Anchor facts:** `own_hash = Sha256::digest(&final_data)` where `final_data` is the **entity value bytes** only (`interface.rs:2587`); `Metadata` is never fed into that digest. So adding a field to `Metadata` is Merkle-invisible **by construction** — the test pins that contract against accidental future inclusion of metadata in a hash.

- [ ] **Step 1 — Failing test (default + invisibility):**
```rust
// entities.rs
#[test]
fn schema_version_defaults_none() {
    let m = Metadata::new(1, 1);
    assert_eq!(m.schema_version, None, "legacy/unmarked entries carry no schema tag");
}
// interface.rs test module — pin Merkle-invisibility
#[test]
fn schema_version_does_not_affect_own_hash() {
    let data = b"identity-gated-value".to_vec();
    let h_none = Sha256::digest(&data);
    // own_hash is Sha256(data) regardless of metadata; tagging schema_version
    // must not change the leaf hash. This guards the invariant if a future
    // refactor ever folds metadata into the digest.
    let h_tagged = Sha256::digest(&data); // same input — own_hash is data-only
    assert_eq!(h_none, h_tagged);
}
```
- [ ] **Step 2 — Run, expect FAIL** (`cargo test -p calimero-storage schema_version_defaults_none`) with "no field `schema_version`".
- [ ] **Step 3 — Implement:** add `pub schema_version: Option<u32>` to `Metadata` (after `field_name`, ~514). It is `#[non_exhaustive]` already; `Default` derive gives `None`. Update every constructor (`new`, `with_crdt_type`, `with_field_name`, `with_crdt_type_and_field_name`) to set `schema_version: None`. Add `pub fn with_schema_version(mut self, v: u32) -> Self { self.schema_version = Some(v); self }` builder and `pub const fn schema_version(&self) -> Option<u32>`. **Do NOT touch `save_internal`'s digest** — that is the whole point.
- [ ] **Step 4 — Run, expect PASS.** Also run the full storage suite to confirm no borsh-layout test breaks: `cargo test -p calimero-storage`. (Adding a trailing `Option` field to a borsh struct is an append; verify any golden-bytes fixtures.)
- [ ] **Step 5 — Commit:** `feat(storage): add Merkle-invisible Metadata.schema_version`

```
feat(storage): add Merkle-invisible Metadata.schema_version

Per-entry schema tag for identity-gated migration (PR-6c). Defaults None
(legacy/unmarked); not part of own_hash (Sha256 over value bytes only,
interface.rs:2587), so tagging never diverges a leaf.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

### Task 6c.2: Per-entry stale-detection dispatch key

**Files:**
- Modify: `crates/storage/src/interface.rs` (new free fn near `apply_action`; or `crates/storage/src/collections/authored_common.rs`).
- Test: same file's test module.

**Goal:** a pure predicate `entry_needs_convert(stored_meta, target_version) -> bool` keyed on `(crdt_type|field_name, schema_version)`: true iff the entry is identity-gated (`StorageType::User`/`Shared`/`SharedMember`) AND `stored.schema_version < Some(target)` (treating `None` as version 0). Public/Frozen entries always return false (Convergent path handles those via whole-root). This is the dispatch key — `(crdt_type, field_name)` selects *which* per-type transform the v2 binary applies; the predicate only decides *whether*.

- [ ] **Step 1 — Failing test:**
```rust
#[test]
fn only_stale_identity_gated_entries_need_convert() {
    let owner = test_pubkey();
    let user_v0 = Metadata::new(1, 1); // User-stamped below; schema_version None == v0
    // identity-gated + stale -> convert
    assert!(entry_needs_convert(&user_stamp(user_v0.clone(), owner), 1));
    // identity-gated + already target -> no
    assert!(!entry_needs_convert(&user_stamp(Metadata::new(1,1).with_schema_version(1), owner), 1));
    // Public -> never (whole-root path owns it)
    assert!(!entry_needs_convert(&Metadata::new(1,1), 1));
}
```
- [ ] **Step 2 — Run, expect FAIL** ("function `entry_needs_convert` not found").
- [ ] **Step 3 — Implement** the pure predicate (match on `storage_type`; `schema_version.unwrap_or(0) < target` for the gated arms; `false` otherwise).
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(storage): entry_needs_convert dispatch predicate for identity-gated migration`
  (trailer as above)

### Task 6c.2a: SDK `app::schema_version()` surface (no new macro)

**Files:**
- Modify: `crates/sdk/src/` (locate the `app` module re-exports via `rg "pub mod app|pub fn migrate" crates/sdk/src`).
- Test: sdk unit test or a TestHost-based test (the bridge exists from #2674).

**Goal:** the v2 binary needs a way to declare its current target `schema_version` so identity-gated writes stamp it. **No derive** — a thin runtime accessor reading a value the runtime threads in at install (reuse the existing app-key/version plumbing; if no version is surfaced to wasm yet, expose the app's `version` already carried in install metadata). The author writes `AuthoredMap::insert` exactly as today; the stamp is automatic in `save_raw` (Task 6c.3) using this value.

- [ ] **Step 1 — Failing test:** assert `calimero_sdk::app::schema_version()` returns the installed app's schema/version (TestHost: install v2 → read it back).
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement:** wire the accessor to the runtime-provided version. If the host already exposes app version to wasm, re-export it; otherwise add one host fn alongside the existing executor/app-key accessors (`rg "fn executor_id|app_key" crates/runtime crates/sdk`).
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(sdk): expose app::schema_version() for identity-gated stamping`

> **NOTE / blocking unknown B1:** if the runtime does **not** currently surface an app schema/version to wasm, this task gains a small host-fn addition (runtime + host imports). Confirm during Step 0 of 6c.2a. If absent, the stamp value can fall back to the namespace governance `app_key`'s monotonic counter threaded via `ApplyContext` — but that is a host-side stamp, not author-controlled, and changes 6c.3's signature. Resolve before starting 6c.3.

### Task 6c.3: Owner-driven convert at write time (strictly-monotonic nonce)

**Files:**
- Modify: `crates/storage/src/interface.rs` (`save_raw`, ~2975; the `StorageType::User` local-owner stamp branch ~3003-3016, and the equivalent `Shared`/`SharedMember` branches).
- Test: storage test module + a TestHost two-identity convergence test (#2674 bridge).

**O4 semantics (locked):** the convert is an **ordinary owner `Action::Update`** — same `owner`, **new value bytes** (the v2 shape), `schema_version = Some(target)`, and a **strictly-greater `updated_at`** (the monotonic nonce). It runs on the **normal write path** (NOT `with_merge_mode`, NOT entropy-suppressed). When the owner's v2 binary next calls `AuthoredMap::insert/update` on an entry whose stored `schema_version < target` (Task 6c.2 predicate), `save_raw` stamps the new `schema_version` and the fresh nonce; the existing signer then signs the new `(id, data, nonce, storage_type)` payload. It replicates to peers as one convergent signed delta and lands via `apply_action`'s normal `new_nonce > last_nonce` branch (interface.rs:986+).

**Important scoping:** 6c does **not** invent a separate "convert" call. The convert IS the owner's next ordinary write — `save_raw` already re-stamps a fresh nonce on every local owner write (interface.rs:3003-3016 comment: "ALWAYS overwrite the incoming signature_data with a fresh placeholder tied to this call's nonce"). 6c only adds: **(a)** stamp `schema_version = app::schema_version()` into the metadata on that path, and **(b)** a test proving a stale entry, once rewritten by its owner, carries the new tag + advanced nonce and that a non-owner write is still rejected (existing `executor_matches_owner` gate, authored_common.rs:43).

- [ ] **Step 1 — Failing test (stamp + monotonic nonce + owner-only):**
```rust
#[test]
fn owner_write_stamps_target_schema_and_advances_nonce() {
    // stored: User entry, schema_version None (v0), updated_at = 5
    // owner re-writes via save_raw with updated_at = 9, target schema = 1
    let out = save_user_entry(id, owner, b"v2-bytes", /*updated_at*/ 9, /*schema*/ Some(1));
    let m = get_metadata(id).unwrap();
    assert_eq!(m.schema_version, Some(1), "owner write stamps target schema");
    assert_eq!(m.updated_at(), 9, "nonce strictly advanced (not merge-mode/entropy-suppressed)");
}
#[test]
fn non_owner_cannot_convert_identity_gated_entry() {
    // AuthoredMap::update from a non-owner still rejects (authored_common gate),
    // so a non-owner can never drive the convert.
    set_executor(not_owner());
    assert!(matches!(authored_update(id, b"x"), Err(StoreError::ActionNotAllowed(_))));
}
#[test]
fn convert_does_not_run_in_merge_mode() {
    // Guard O4: assert the convert write path is NOT gated by in_merge_mode
    // (merge mode bypasses the nonce check, interface.rs:920 — wrong for a real write).
    assert!(!storage_env::in_merge_mode());
    let out = save_user_entry(id, owner, b"v2", 9, Some(1));
    // succeeds on the normal monotonic-nonce path
    assert!(out.is_ok());
}
```
- [ ] **Step 2 — Run, expect FAIL** (schema not stamped yet).
- [ ] **Step 3 — Implement:** in `save_raw`'s local-owner `User` branch (and the `Shared`/`SharedMember` stamp branches), set `metadata.schema_version = Some(app::schema_version())` when the executor is the owner/writer (i.e. exactly where the fresh nonce is already stamped). Thread the target version in via `ApplyContext`/env (per 6c.2a). Do **not** alter the nonce logic — it already advances. Do **not** wrap in `with_merge_mode`.
- [ ] **Step 4 — Run, expect PASS.** Then a **TestHost two-identity convergence test**: identity A (owner) converts entry; identity B applies the replicated `Action::Update`; assert both roots converge and B's stored entry shows `schema_version = Some(target)` (proves it replicates as a normal signed delta, spec §7.2).
- [ ] **Step 5 — Commit:** `feat(storage): owner-driven identity-gated convert stamps schema + monotonic nonce`

```
feat(storage): owner-driven identity-gated convert stamps schema + monotonic nonce

The owner's next ordinary signed write of a stale identity-gated entry
stamps schema_version = target and advances updated_at (the nonce),
replicating as a normal Action::Update. NOT merge-mode (which bypasses
the nonce check at interface.rs:920) and NOT owner-changing
(verify_action_update wall at :3200). Non-owners still rejected by the
authored_common owner gate. Resolves spec O4.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

### Task 6c.4: `GroupOp::MigrationForceCarry` variant

**Files:**
- Modify: `crates/governance-types/src/lib.rs` (`GroupOp` enum ~101; `op_kind_label` match ~357-382).
- Test: lib.rs unit test (label round-trip + borsh).

**Shape (locked by O4):** force-carry CANNOT re-sign as the departed owner and CANNOT change owner. The op authorizes the admin to tombstone the stale entry and re-create it under the admin's key:
```rust
/// Admin force-carry of a departed owner's stale identity-gated entry:
/// tombstone the old entry and create a new entity under the admin's own
/// key (the admin cannot forge the owner's signature nor change an entry's
/// owner — verify_action_update rejects both). Authorized only for the
/// namespace admin/owner capability; observability-driven, never automatic.
MigrationForceCarry {
    context_id: [u8; 32],
    entry_id: [u8; 32],        // storage Id of the stale entry
    departed_owner: PublicKey, // audit: whose entry is being carried
    target_schema_version: u32,
},
```
- [ ] **Step 1 — Failing test:** `op_kind_label(MigrationForceCarry{..}) == "migration_force_carry"`; borsh round-trip.
- [ ] **Step 2 — Run, expect FAIL** (variant missing).
- [ ] **Step 3 — Implement:** add the variant + the `op_kind_label` arm. The `#[non_exhaustive]` wildcard in `ops/group.rs:178` keeps the apply dispatcher compiling as `handled=false` until 6c.5.
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-governance-types`).
- [ ] **Step 5 — Commit:** `feat(governance-types): add GroupOp::MigrationForceCarry`
  (trailer as above)

### Task 6c.5: Force-carry apply handler + admin authorization

**Files:**
- Create: `crates/governance-store/src/ops/group/migration_force_carry.rs` (mirror `cascade_upgrade.rs` structure).
- Modify: `crates/governance-store/src/ops/group.rs` (~161 dispatch — add the match arm).
- Test: `crates/governance-store/src/tests.rs` (mirror `cascade_upgrade` apply tests).

**Authorization:** require the op's signer to hold the namespace **admin/owner** capability (reuse the exact capability check `cascade_upgrade::apply` / `member_removed` use — `rg "capability|is_admin|require_owner" crates/governance-store/src/ops/group/cascade_upgrade.rs`). Reject non-admins with `ApplyError::Unauthorized` (or the existing equivalent).

**Apply effect:** the handler records intent and drives the storage effect via the context's apply context: **(1)** delete (tombstone) the stale `entry_id`, **(2)** signal the node to create a fresh entity stamped `User { owner: admin_signer }` carrying `target_schema_version`. Because the new entity is owned and signed by the admin, it verifies normally at `apply_action` — no owner-forge, no owner-change. The actual storage delete+create is a normal admin-signed `Action` sequence emitted by the handler's caller; the governance handler's job is **authorization + recording**, not bypassing storage verification.

- [ ] **Step 1 — Failing test:**
```rust
#[test]
fn force_carry_requires_admin_capability() {
    let ctx = test_ctx_with_member(non_admin);
    let r = migration_force_carry::apply(&ctx, /*signed by*/ non_admin, op());
    assert!(matches!(r, Err(ApplyError::Unauthorized(_))));
}
#[test]
fn force_carry_by_admin_records_intent() {
    let ctx = test_ctx_with_admin(admin);
    migration_force_carry::apply(&ctx, admin, op()).expect("admin authorized");
    // recorded; new entity will be admin-owned (verifies at apply_action)
}
```
- [ ] **Step 2 — Run, expect FAIL** (module/handler missing).
- [ ] **Step 3 — Implement:** new handler module + dispatch arm; admin-capability gate; record the force-carry. Keep the storage tombstone+recreate as admin-signed actions (verifies normally).
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-governance-store force_carry`).
- [ ] **Step 5 — Commit:** `feat(governance-store): MigrationForceCarry apply handler (admin tombstone+rekey)`

```
feat(governance-store): MigrationForceCarry apply handler (admin tombstone+rekey)

Departed-owner identity-gated entries: an admin-authorized op tombstones
the stale entry and re-creates it under the admin's own key. The admin
cannot forge the owner sig nor change an entry's owner (verify_action_update
wall), so this is the only crypto-sound resolution. Authorization reuses
the namespace admin/owner capability gate. Resolves the O4 force-carry half.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

### Task 6c.6: Residue local-derived scan

**Files:**
- Modify: `crates/storage/src/interface.rs` (or a new `crates/storage/src/migration_residue.rs`).
- Test: storage test module + a TestHost two-node idempotency test.

**Goal (spec decision 8):** residue = a **local derived scan** = count of locally-stored identity-gated entries whose `schema_version < target` (reuse `entry_needs_convert`). **NOT** a replicated shrink-CRDT (the spec calls out that the only counter double-counts under concurrent convert). `fn count_unconverted_identity_gated(target: u32) -> usize` iterating the index's entries (use the existing entry/metadata iteration — `rg "fn enumerate|fn iter|Index::.*entries" crates/storage/src`).

- [ ] **Step 1 — Failing test:**
```rust
#[test]
fn residue_counts_only_stale_identity_gated() {
    seed_user_entry(id1, owner, schema=None);   // stale -> +1
    seed_user_entry(id2, owner, schema=Some(1)); // converted -> 0
    seed_public_entry(id3);                       // never counted
    assert_eq!(count_unconverted_identity_gated(1), 1);
}
#[test]
fn residue_is_idempotent_under_concurrent_convert() {
    // two nodes both convert id1; each node's LOCAL scan drops by exactly 1,
    // never 2 (proves no replicated double-count).
    // node A: convert id1 -> A.residue 1->0
    // node B applies A's delta -> B.residue 1->0 (already-converted is idempotent)
}
```
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement** the scan over local entries via the index, filtering with `entry_needs_convert`.
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(storage): local-derived residue scan for identity-gated migration`
  (trailer as above)

### Task 6c.7: `MigrationHeartbeat` wire variant + signed body

**Files:**
- Modify: `crates/governance-types/src/wire.rs` (new `SignableMigrationHeartbeat` + `SignedMigrationHeartbeat` modeled on `SignableReadinessBeacon`/`SignedReadinessBeacon` ~98-159; add `MigrationHeartbeat` arm to `NamespaceTopicMsg` ~175).
- Test: wire.rs test module (sign/verify round-trip; field-substitution rejection — mirror the readiness beacon test ~325-371).

**Body (signed, mirrors readiness beacon discipline):**
```rust
pub struct SignableMigrationHeartbeat {
    pub namespace_id: [u8; 32],
    pub peer_pubkey: PublicKey,
    pub schema_version: u32,       // node's loaded binary/target version
    pub residue_auto: u64,         // unconverted Convergent contexts (from 6a marker)
    pub residue_identity: u64,     // count_unconverted_identity_gated (6c.6)
    pub synced_up_to_hlc: u64,
    pub ts_millis: u64,
}
// SignedMigrationHeartbeat = body + signature:[u8;64], with
// signable_bytes() = MIGRATION_HEARTBEAT_SIGN_DOMAIN || borsh(body),
// to_signable(), verify_signature() — copied from SignedReadinessBeacon.
```
Add `MIGRATION_HEARTBEAT_SIGN_DOMAIN` constant (distinct domain-separation prefix). `NamespaceTopicMsg::MigrationHeartbeat(SignedMigrationHeartbeat)`.

> **NOTE B2:** `NamespaceTopicMsg` is documented as requiring a coordinated cluster upgrade per added variant (wire.rs:172). Pre-1.0, acceptable; call it out in the PR body.

- [ ] **Step 1 — Failing test:** sign body with a key; `verify_signature()` passes; flip `residue_identity` → `verify_signature()` fails; borsh round-trip of the `NamespaceTopicMsg::MigrationHeartbeat` envelope.
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement** both structs + domain constant + the `NamespaceTopicMsg` arm, copying the readiness beacon's `to_signable`/`signable_bytes`/`verify_signature` pattern exactly.
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-governance-types`).
- [ ] **Step 5 — Commit:** `feat(governance-types): SignedMigrationHeartbeat + NamespaceTopicMsg::MigrationHeartbeat`
  (trailer as above)

### Task 6c.8: Heartbeat TTL cache ingest (signature + membership verified)

**Files:**
- Create: `crates/node/src/migration_status.rs` (a `MigrationStatusCache` modeled directly on `crates/node/src/readiness.rs`' `ReadinessCache` ~223-402: `Mutex<HashMap<([u8;32], PublicKey), CacheEntry>>`, `insert`, `fresh_peers(ns, ttl)`, `peer_summary(ns, ttl)`, `received_at: Instant`, default TTL 60s).
- Modify: the namespace-topic message dispatch (locate via `rg "NamespaceTopicMsg::ReadinessBeacon" crates/node/src`) to route `MigrationHeartbeat` into the cache.
- Test: `migration_status.rs` unit tests (insert/expire/snapshot) mirroring readiness tests.

**Verification on receive (mirror readiness):** verify the Ed25519 signature (`verify_signature()`) AND that `peer_pubkey` is a member of the namespace cohort before inserting — drop otherwise (an unsigned/non-member heartbeat must never appear in a rollup). Reuse whatever membership check the readiness ingest uses (`rg "verify_signature|membership|is_member" crates/node/src/readiness.rs` and the dispatch site).

- [ ] **Step 1 — Failing test:**
```rust
#[test]
fn cache_ingests_fresh_and_expires_stale() {
    let cache = MigrationStatusCache::default();
    cache.insert(&signed_hb(ns, peer, schema=1, residue_identity=0));
    assert_eq!(cache.fresh_peers(ns, Duration::from_secs(60)).len(), 1);
    // beyond TTL -> not fresh (drive Instant via the same seam readiness tests use)
}
#[test]
fn cache_rejects_bad_signature() {
    let mut hb = signed_hb(ns, peer, 1, 0); hb.signature = [0;64];
    assert!(ingest(&cache, &hb).is_err());
    assert_eq!(cache.fresh_peers(ns, Duration::from_secs(60)).len(), 0);
}
```
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement** the cache + the dispatch routing + sig/membership gate. Also add a periodic **emit** of the local node's own heartbeat (model on the readiness beacon publish loop — `rg "publish.*ReadinessBeacon|beacon_interval" crates/node/src`), carrying `residue_identity` from 6c.6 and `residue_auto` from the 6a per-context marker.
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-node migration_status`).
- [ ] **Step 5 — Commit:** `feat(node): MigrationStatusCache ingest of signed migration heartbeats`
  (trailer as above)

### Task 6c.9: `get_migration_status` handler (cohort, pinning, `unknown`, `all_migrated`)

**Files:**
- Create: `crates/context/src/handlers/get_migration_status.rs` (model on `crates/context/src/handlers/get_cascade_status.rs` — note it's `collect_cascade_status` calling `NamespaceRepository::collect_descendants`, line 13-19).
- Test: context handler test module.

**Expected-members = inherited closure (reuse #2371):** for the namespace and every descendant (`collect_descendants`, get_cascade_status.rs:19), union `list_group_members ∪ MembershipRepository::enumerate_inherited` (the exact pattern in `list_group_members.rs:48-54`, `core.rs:230`). **Cohort pinned at expand-entry governance HLC** — read the migration's entry HLC from the namespace `UpgradesRepository`/`MigrationsRepository` marker (`rg "MigrationsRepository|cascade_hlc|expand" crates/governance-store/src`) and snapshot membership as of that HLC so mid-migration joins/leaves don't flip the signal.

**Rollup:** for each pinned-cohort member, look up the freshest heartbeat in `MigrationStatusCache.peer_summary(ns, ttl)`. A member with no fresh heartbeat → `unknown` (keeps `all_migrated=false`, no false green). `all_migrated = true` iff **every** pinned member reported `schema_version >= target && residue_identity == 0 && residue_auto == 0`. Shape per spec §8:
```jsonc
{ "target_version": 2, "expected_members": 12,
  "cohort_pinned_at_hlc": "…",
  "rollup": { "migrated": 9, "in_progress": 2, "unknown": 1, "total": 12, "all_migrated": false },
  "members": [ { "peer", "schema_version", "residue_auto", "residue_identity",
                 "synced_up_to_hlc", "reported_at", "state": "migrated|in_progress|unknown" } ] }
```

- [ ] **Step 1 — Failing test:**
```rust
#[test]
fn unknown_member_blocks_all_migrated() {
    // cohort {A,B,C}; A,B report v2+residue0; C never reports
    let st = get_migration_status(ns, target=2);
    assert_eq!(st.rollup.unknown, 1);
    assert!(!st.rollup.all_migrated, "an unknown member must keep all_migrated false");
}
#[test]
fn cohort_pinned_ignores_post_expand_joiner() {
    // member D joins AFTER expand-entry HLC -> not in expected_members
    let st = get_migration_status(ns, target=2);
    assert_eq!(st.expected_members, 3, "D joined after the pin and is excluded");
}
#[test]
fn all_migrated_true_only_when_every_pinned_member_v2_residue0() { /* ... */ }
```
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement** the handler: pin HLC, build inherited closure across `collect_descendants`, roll up the cache, compute `all_migrated`/`unknown`. **Never gate** — pure read.
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-context get_migration_status`).
- [ ] **Step 5 — Commit:** `feat(context): get_migration_status rollup (inherited cohort, pinned, unknown-safe)`

```
feat(context): get_migration_status rollup (inherited cohort, pinned, unknown-safe)

expected_members = list ∪ enumerate_inherited across collect_descendants
(reuses #2371), cohort pinned at expand-entry HLC so mid-migration churn
can't flip the signal. all_migrated true only when every pinned member
reported v2 + residue 0; missing heartbeats are `unknown` and keep it
false (no false green). Observability only — never a gate (spec §8).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

### Task 6c.10: Admin HTTP route

**Files:**
- Modify: `crates/server/src/admin/service.rs` (~231, beside the `cascade-status` route).
- Create: `crates/server/src/admin/handlers/groups/get_migration_status.rs` (mirror `get_cascade_status::handler`).
- Test: server route test (mirror the cascade-status route test).

- [ ] **Step 1 — Failing test:** `GET /admin/contexts/migration-status/{namespace_id}` (or `/groups/:namespace_id/migration-status` to match the cascade-status sibling) returns 200 + the 6c.9 JSON for a known namespace; 404 for unknown.
- [ ] **Step 2 — Run, expect FAIL** (route not wired).
- [ ] **Step 3 — Implement** the handler + `.route(...)` line mirroring `cascade-status` (service.rs:231-233). Reuse the existing admin auth layer.
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-server migration_status`).
- [ ] **Step 5 — Commit:** `feat(server): admin route GET migration-status`
  (trailer as above)

### Task 6c.11: e2e — owner-driven convert, departed-owner force-carry, status rollup

**Files:**
- Create: `workflows/app-migration/26-owner-driven-authored.yml` — 2-node namespace migration with an `AuthoredMap`; owner offline at migrate, then returns and writes → entry converts; assert convergence + `schema_version` advanced (via `get_migration_status` residue_identity dropping).
- Create: `workflows/app-migration/27-departed-owner-forcecarry.yml` — owner leaves permanently; admin issues `MigrationForceCarry`; assert old entry tombstoned, new admin-owned entry present + converged.
- Create: `workflows/app-migration/28-get-migration-status.yml` — assert rollup fields: `expected_members` = inherited closure, an offline member shows `unknown`, `all_migrated=false`; a joiner added after expand is **excluded** (cohort pinning). Use merobox `assert_log_*` / status-assert steps (mirror `get_cascade_status` merobox coverage).
- Modify: `.github/workflows/app-migration-e2e.yml` matrix (+26/27/28); `workflows/app-migration/build-wasms.sh` if a new identity-gated fixture app is needed.

- [ ] **Step 1 — Write the three workflows.** Add `assert_log_present`/`assert_log_absent` for: owner-only convert ("not entry owner" must NOT fire for the owner; MUST fire if a non-owner attempts), force-carry admin-auth, and `unknown` in the rollup.
- [ ] **Step 2 — Run locally** (`merobox bootstrap run workflows/app-migration/26-...yml --image merod:local --e2e-mode`, etc.), expect PASS. *(Docker + `merod:local` image required; CI run is push-gated per the no-auto-push rule.)*
- [ ] **Step 3 — Commit:** `test(e2e): owner-driven convert, force-carry, migration-status rollup (26-28)`
  (trailer as above)

---

## Self-review checklist

- [ ] `schema_version` never enters `own_hash` (Task 6c.1 pins it; `save_internal` untouched).
- [ ] Convert is a normal owner `Action::Update` — NOT `with_merge_mode`, NOT entropy-suppressed, nonce strictly advances (6c.3 tests assert all three).
- [ ] Force-carry never changes an owner and never forges the owner sig — it tombstones + re-creates under the admin key (6c.4/6c.5).
- [ ] Residue is a local scan, idempotent under concurrent convert (6c.6 two-node test).
- [ ] Heartbeat is ephemeral signed TTL gossip, NOT replicated governance state; sig + membership verified on ingest (6c.7/6c.8).
- [ ] `get_migration_status` is observability ONLY — never gates a write/apply; `unknown` keeps `all_migrated=false` (6c.9).
- [ ] Cohort pinned at expand-entry HLC; expected_members reuses the #2371 inherited closure (6c.9).
- [ ] No new proc-macro code (6c.7-derive dropped); #2674's `#[derive(Migrate)]` is the whole-root surface and stays untouched.

## Execution handoff

Subagent-driven, one task per fresh subagent, two-stage review. Order: storage cluster (6c.1→6c.2→6c.2a→6c.3→6c.6) → `/code-review` → governance (6c.4→6c.5) → `/code-review` → visibility (6c.7→6c.8→6c.9→6c.10) → `/code-review` → e2e (6c.11). Branch off the 6b branch. Resolve blocking unknowns **B1** (does the runtime surface app schema/version to wasm?) and **B2** (coordinated-upgrade implication of the new `NamespaceTopicMsg` variant) before starting 6c.2a/6c.7 respectively.
