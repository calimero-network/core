# PR-3 — Atomic Cascade Op + Sticky `cascade_hlc` + HLC Fence + Status RPC — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the two-op cascade upgrade with a single atomic `CascadeUpgrade` GroupOp carrying a deterministic `cascade_hlc`; persist that `cascade_hlc` stickily per matched descendant group; stamp each state delta with its producing `app_key` and **fence** (drop) stale-schema deltas that arrive after a cascade; generalize the local write-gate; and add a read-only `get_cascade_status` RPC.

**Architecture:** Builds on the merged cascade train (PR-1 #2477, PR-2 #2493) and #2507's determinism work. (1) A wire change to `GroupOp` (schema v6→v7): one `CascadeUpgrade { from_app_key, app_key, target_application_id, migration, cascade_hlc }` op replaces the ordered `CascadeGroupMigrationSet`+`CascadeTargetApplicationSet` pair — removing the receiver apply-order bug (xilosada review item #3 on #2507) and giving every node an *identical* `cascade_hlc` (stamped once by the initiator) to record. (2) A second wire change adds `producing_app_key: Option<[u8;32]>` to the state-delta broadcast: the sender stamps the blob-derived app key it executed under; a receiver that has already applied the cascade drops any delta whose producing key differs from its current target key **and** whose HLC is past the recorded `cascade_hlc` boundary. Both wire changes assume lockstep merod deployment (the same assumption #2507's `app_key` GroupOp addition made).

**Tech Stack:** Rust (`crates/governance-types`, `crates/governance-store`, `crates/context`, `crates/node`, `crates/store`, `crates/storage`), merobox e2e workflows (`workflows/app-migration/`), GitHub Actions (`app-migration-e2e.yml`).

---

## Source-of-truth & scope notes

- **Roadmap:** `docs/superpowers/plans/2026-05-28-migration-remaining-work.md` §A. This plan details §A0–A5 **and** §A3 (the HLC fence), which the roadmap nominally split out — it is included here per explicit direction.
- **The cited design spec does not exist.** The roadmap references `docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md`; only `2026-05-13-opaque-leaf-sync-design.md` and `2026-05-16-namespace-governance-anti-entropy-design.md` are present. The fence predicate below is taken from the roadmap §A3 text, not a spec file.
- **Why `app_key`, not `application_id`:** `ApplicationId = hash(package, signer)` is **stable across versions** (confirmed: `GroupUpgradeValue` doc comment + `crates/primitives/src/application.rs`). `app_key = *app_meta.bytecode.blob_id().as_ref()` **changes on every upgrade** (confirmed: `create_group.rs:114`, `join_group.rs:123`). Only `app_key` is a usable schema-version boundary.
- **Wire compatibility:** borsh is not self-describing and does **not** skip unknown bytes (the #2507 "Not all bytes read" class). Both wire additions (the `CascadeUpgrade` GroupOp variant, the `producing_app_key` delta field) require all merod nodes to run the new binary — lockstep, matching the precedent the cascade `app_key` GroupOp set. If non-lockstep merod rolling upgrades ever become a requirement, migrate the delta change to a `BroadcastMessage::StateDeltaV2` variant; not needed now.
- **Line numbers drift.** Every `file.rs:NNN` below was confirmed in the worktree `/Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation/` at plan time. Re-`grep` the symbol at execution and trust the symbol over the number.
- **Branch:** `feat/cascade-atomic-op-fence-status`. Work in the worktree, not the main checkout.
- **Commit trailer (project convention):** end every commit message with
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## File structure

**Modify:**
- `crates/store/src/key/group/mod.rs` — sticky `cascade_hlc: Option<HybridTimestamp>` on `GroupUpgradeValue` (`:1311`) + backward-compat borsh (currently derived `:1310`).
- `crates/store/Cargo.toml`, `crates/governance-types/Cargo.toml` — ensure `calimero-storage` dep (for `HybridTimestamp`).
- `crates/governance-types/src/lib.rs` — `GroupOp::CascadeUpgrade` variant (`:90`–`308`); bump `SIGNED_GROUP_OP_SCHEMA_VERSION` 6→7 (`:81`).
- `crates/governance-store/src/ops/group.rs` — dispatch arm + `mod cascade_upgrade;` (`:55`–`167`).
- `crates/context/src/handlers/upgrade_group.rs` — `dispatch_cascade` emits one `CascadeUpgrade` (`:1011`–`1056`); stamp `cascade_hlc`; pre-spawn record carries it (`:1069`–`1092`).
- `crates/context/src/handlers/execute/mod.rs` — generalize write-gate (`:128`–`214`); resolve + thread `producing_app_key` into the broadcast call (`:852`–`868`).
- `crates/context/src/lib.rs` — `pub mod hlc_fence;`.
- `crates/context/src/handlers.rs` — register `get_cascade_status`.
- `crates/node/primitives/src/sync/snapshot.rs` — `producing_app_key: Option<[u8;32]>` on `BroadcastMessage::StateDelta` (`:669`–`719`).
- `crates/node/primitives/src/client.rs` — `broadcast(...)` gains the param + writes it into the message (`:491`–`538`).
- `crates/node/src/handlers/state_delta/mod.rs` — `StateDeltaMessage` field + destructure (`:240`–`254`); fence check in `apply_authorized_state_delta` after the envelope-sig check (`:587`+).

**Create:**
- `crates/governance-store/src/ops/group/cascade_upgrade.rs` — atomic apply handler.
- `crates/context/src/hlc_fence.rs` — pure `should_fence` + store-aware `delta_is_fenced`.
- `crates/context/src/handlers/get_cascade_status.rs` — read-only RPC.
- `crates/context/tests/cascade_atomic_apply.rs`, `cascade_status_rpc.rs`, `hlc_fence.rs` — integration tests.

**Workflows:**
- `workflows/app-migration/01-namespace-cascade-migration.yml` — receiver applies the single op; both layers converge.
- (Stretch, merobox-gated) `workflows/app-migration/06-fence-rejects-straggler.yml`.

---

## Task 1 — Sticky `cascade_hlc` field on `GroupUpgradeValue`

**Files:**
- Modify: `crates/store/src/key/group/mod.rs:1309-1322`
- Modify: `crates/store/Cargo.toml`
- Test: `crates/store/src/key/group/mod.rs` (`#[cfg(test)] mod cascade_hlc_borsh_tests`)

Rationale: `GroupUpgradeValue` derives borsh; appending a field breaks deserialization of pre-existing records (no trailing bytes). Hand-write `BorshDeserialize` to treat a missing trailing field as `None`. `LazyOnAccess` writes straight to `Completed`, so `cascade_hlc` must be a top-level sticky field, **not** inside `InProgress`.

- [ ] **Step 1: Confirm `HybridTimestamp` dep on the store crate**

Run: `grep -n "calimero-storage" /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation/crates/store/Cargo.toml`
If absent, add under `[dependencies]` (match the form siblings use): `calimero-storage = { workspace = true }`.

- [ ] **Step 2: Write the failing borsh round-trip test**

Add at the bottom of `crates/store/src/key/group/mod.rs`:

```rust
#[cfg(all(test, feature = "borsh"))]
mod cascade_hlc_borsh_tests {
    use borsh::{to_vec, BorshDeserialize, BorshSerialize};
    use calimero_storage::logical_clock::HybridTimestamp;

    use super::{GroupUpgradeStatus, GroupUpgradeValue};
    use crate::key::PrimitivePublicKey;

    fn sample(cascade_hlc: Option<HybridTimestamp>) -> GroupUpgradeValue {
        GroupUpgradeValue {
            from_version: "1.0.0".to_owned(),
            to_version: "2.0.0".to_owned(),
            migration: Some(vec![1, 2, 3]),
            initiated_at: 1_700_000_000,
            initiated_by: PrimitivePublicKey::from([7u8; 32]),
            status: GroupUpgradeStatus::Completed { completed_at: None },
            cascade_hlc,
        }
    }

    #[test]
    fn roundtrips_with_populated_cascade_hlc() {
        let value = sample(Some(HybridTimestamp::zero()));
        let bytes = to_vec(&value).unwrap();
        let decoded = GroupUpgradeValue::try_from_slice(&bytes).unwrap();
        assert_eq!(decoded.cascade_hlc, Some(HybridTimestamp::zero()));
        assert_eq!(decoded.to_version, "2.0.0");
    }

    #[test]
    fn old_format_without_field_decodes_as_none() {
        let mut legacy = Vec::new();
        "1.0.0".to_owned().serialize(&mut legacy).unwrap();
        "2.0.0".to_owned().serialize(&mut legacy).unwrap();
        Some(vec![1u8, 2, 3]).serialize(&mut legacy).unwrap();
        1_700_000_000u64.serialize(&mut legacy).unwrap();
        PrimitivePublicKey::from([7u8; 32]).serialize(&mut legacy).unwrap();
        (GroupUpgradeStatus::Completed { completed_at: None }).serialize(&mut legacy).unwrap();

        let decoded = GroupUpgradeValue::try_from_slice(&legacy).unwrap();
        assert_eq!(decoded.cascade_hlc, None);
    }
}
```

- [ ] **Step 3: Run, expect FAIL**

Run: `cd /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation && cargo test -p calimero-store cascade_hlc_borsh_tests`
Expected: FAIL — no field `cascade_hlc`.

- [ ] **Step 4: Add the field + hand-written backward-compatible borsh**

Replace the struct (`:1309-1322`); keep `BorshSerialize` derived, hand-write `BorshDeserialize`:

```rust
#[derive(Clone, Debug)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize))]
pub struct GroupUpgradeValue {
    pub from_version: String,
    pub to_version: String,
    pub migration: Option<Vec<u8>>,
    pub initiated_at: u64,
    pub initiated_by: PrimitivePublicKey,
    pub status: GroupUpgradeStatus,
    /// Sticky cascade fence boundary: the HLC the originating `CascadeUpgrade`
    /// op was stamped with, identical on every node that applied it. `None` for
    /// non-cascade upgrades and pre-existing records. NEVER cleared once set
    /// (survives `Completed`) — the boundary the state-delta HLC fence reads.
    pub cascade_hlc: Option<HybridTimestamp>,
}

#[cfg(feature = "borsh")]
impl borsh::BorshDeserialize for GroupUpgradeValue {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let from_version = String::deserialize_reader(reader)?;
        let to_version = String::deserialize_reader(reader)?;
        let migration = Option::<Vec<u8>>::deserialize_reader(reader)?;
        let initiated_at = u64::deserialize_reader(reader)?;
        let initiated_by = PrimitivePublicKey::deserialize_reader(reader)?;
        let status = GroupUpgradeStatus::deserialize_reader(reader)?;
        let cascade_hlc = match Option::<HybridTimestamp>::deserialize_reader(reader) {
            Ok(v) => v,
            Err(e) if e.kind() == borsh::io::ErrorKind::UnexpectedEof => None,
            Err(e) => return Err(e),
        };
        Ok(Self { from_version, to_version, migration, initiated_at, initiated_by, status, cascade_hlc })
    }
}
```

Add near the file's import group: `use calimero_storage::logical_clock::HybridTimestamp;`.

- [ ] **Step 5: Run, expect PASS**

Run: `cargo test -p calimero-store cascade_hlc_borsh_tests`
Expected: PASS.

- [ ] **Step 6: Fix every `GroupUpgradeValue { … }` literal**

Run: `grep -rn "GroupUpgradeValue {" /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation/crates`
Add `cascade_hlc: None` to each (the cascade path gets a real value in Task 4). Then `cargo build -p calimero-store -p calimero-context -p calimero-governance-store` and fix any `missing field cascade_hlc`.

- [ ] **Step 7: Commit**

```bash
git add crates/store/src/key/group/mod.rs crates/store/Cargo.toml crates/context/src/handlers/upgrade_group.rs
git commit -m "feat(store): add sticky cascade_hlc to GroupUpgradeValue with backward-compat borsh"
```

---

## Task 2 — `CascadeUpgrade` GroupOp variant + schema bump

**Files:**
- Modify: `crates/governance-types/src/lib.rs:81,90-308`
- Modify: `crates/governance-store/src/ops/group.rs:55-167` (stub arm)

- [ ] **Step 1: Confirm current state**

Run: `grep -n "SIGNED_GROUP_OP_SCHEMA_VERSION\|CascadeTargetApplicationSet\|CascadeGroupMigrationSet" /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation/crates/governance-types/src/lib.rs`
Expected: `= 6`; cascade variants near `:293-307`.

- [ ] **Step 2: Add variant + bump schema**

```rust
pub const SIGNED_GROUP_OP_SCHEMA_VERSION: u8 = 7;
```
After the existing `CascadeGroupMigrationSet` variant, add:
```rust
    /// Atomic namespace cascade upgrade. Applies target_application_id, app_key,
    /// and migration in a SINGLE op per matched descendant (the
    /// `from_app_key == descendant.app_key` walk predicate), so receivers cannot
    /// reproduce the out-of-order apply bug. `cascade_hlc` is stamped once by the
    /// initiator so every node records an identical fence boundary. Lockstep
    /// wire addition (schema v7).
    CascadeUpgrade {
        from_app_key: [u8; 32],
        app_key: [u8; 32],
        target_application_id: ApplicationId,
        migration: Option<Vec<u8>>,
        cascade_hlc: HybridTimestamp,
    },
```
Add `use calimero_storage::logical_clock::HybridTimestamp;` (and the `calimero-storage` dep in `crates/governance-types/Cargo.toml` if `grep -n calimero-storage crates/governance-types/Cargo.toml` shows none).

- [ ] **Step 3: Temporary dispatch stub**

In `crates/governance-store/src/ops/group.rs` after the `CascadeGroupMigrationSet` arm:
```rust
        GroupOp::CascadeUpgrade { .. } => eyre::bail!("CascadeUpgrade apply not yet implemented"),
```

- [ ] **Step 4: Build**

Run: `cargo build -p calimero-governance-types -p calimero-governance-store`
Expected: compiles (add explicit `CascadeUpgrade` arms to any other exhaustive `GroupOp` match the compiler flags).

- [ ] **Step 5: Commit**

```bash
git add crates/governance-types/src/lib.rs crates/governance-types/Cargo.toml crates/governance-store/src/ops/group.rs
git commit -m "feat(governance-types): add atomic CascadeUpgrade GroupOp variant, bump schema v6->v7"
```

---

## Task 3 — Atomic `CascadeUpgrade` apply (sets target+app_key+migration, records sticky cascade_hlc)

**Files:**
- Create: `crates/governance-store/src/ops/group/cascade_upgrade.rs`
- Modify: `crates/governance-store/src/ops/group.rs`
- Test: `crates/context/tests/cascade_atomic_apply.rs`

Reference verbatim (copy real accessor names from): `crates/governance-store/src/ops/group/cascade_target_application_set.rs:9-130`, `cascade_group_migration_set.rs:8-83`.

- [ ] **Step 1: Characterize the latent bug (RED proof against the existing two-op design)**

Before building the fix, prove the bug is real in shipped code. Create `crates/context/tests/cascade_atomic_apply.rs` with the `empty_store`/`meta`/`create_group`/`APP_KEY_1/2`/`app_id_1/2` helpers from `crates/context/tests/cascade_apply_walk.rs:13-70`, then write a test that applies **today's two ops in the buggy order** (target-set first, which rewrites `app_key` so the later migration-set predicate matches nothing) and asserts the DESIRED outcome (migration set):

```rust
/// CHARACTERIZATION of the xilosada #2507 review-item-#3 apply-order bug.
/// Delivering CascadeTargetApplicationSet BEFORE CascadeGroupMigrationSet
/// rewrites every descendant's app_key away from `from_app_key`, so the
/// migration-set predicate then matches nothing and `migration` is dropped.
/// This assertion FAILS today (migration == None) — that red is the proof.
/// It is replaced by the atomic-op test below; see Step 9 for its disposition.
#[test]
fn two_op_reverse_delivery_drops_migration_characterization() {
    let mut rng = rand::rngs::OsRng;
    let admin_sk = calimero_primitives::identity::PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let store = empty_store();
    let r = ContextGroupId::from([0x70; 32]);
    create_group(&store, &r, admin_pk, APP_KEY_1, app_id_1());

    // Buggy order: target-set first.
    apply_local_signed_group_op(&store, &SignedGroupOp::sign(
        &admin_sk, r.to_bytes(), vec![], [0u8; 32], 1,
        GroupOp::CascadeTargetApplicationSet { from_app_key: APP_KEY_1, app_key: APP_KEY_2, target_application_id: app_id_2() },
    ).unwrap()).unwrap();
    apply_local_signed_group_op(&store, &SignedGroupOp::sign(
        &admin_sk, r.to_bytes(), vec![], [0u8; 32], 2,
        GroupOp::CascadeGroupMigrationSet { from_app_key: APP_KEY_1, migration: Some(b"migrate_v2".to_vec()) },
    ).unwrap()).unwrap();

    let m = MetaRepository::new(&store).load(&r).unwrap().unwrap();
    assert_eq!(m.migration, Some(b"migrate_v2".to_vec()), "BUG: migration dropped by reverse delivery");
}
```

- [ ] **Step 2: Run, expect RED (the proof)**

Run: `cargo test -p calimero-context --test cascade_atomic_apply two_op_reverse_delivery_drops_migration_characterization`
Expected: **FAIL** — `migration` is `None`. Capture this output verbatim for the PR description (it is the evidence the latent bug is live). Do NOT fix the two-op path; the atomic op makes it unreachable.

- [ ] **Step 3: Write the failing atomic-op test (the permanent regression guard)**

Add to the same file the assertion that the **single atomic op** sets everything regardless of order:

```rust
use calimero_context::group_store::{
    apply_local_signed_group_op, MetaRepository, NamespaceRepository, UpgradesRepository,
};
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_storage::logical_clock::HybridTimestamp;

#[test]
fn cascade_upgrade_atomic_op_sets_target_app_key_and_migration_and_records_cascade_hlc() {
    let mut rng = rand::rngs::OsRng;
    let admin_sk = calimero_primitives::identity::PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let store = empty_store();

    let r = ContextGroupId::from([0x70; 32]);
    let r_b = ContextGroupId::from([0xB1; 32]);
    let r_b_b1 = ContextGroupId::from([0xB2; 32]);
    create_group(&store, &r, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &r_b, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &r_b_b1, admin_pk, APP_KEY_1, app_id_1());
    NamespaceRepository::new(&store).nest(&r, &r_b).unwrap();
    NamespaceRepository::new(&store).nest(&r_b, &r_b_b1).unwrap();

    let fence = HybridTimestamp::zero();
    let op = SignedGroupOp::sign(
        &admin_sk, r.to_bytes(), vec![], [0u8; 32], 1,
        GroupOp::CascadeUpgrade {
            from_app_key: APP_KEY_1,
            app_key: APP_KEY_2,
            target_application_id: app_id_2(),
            migration: Some(b"migrate_v2".to_vec()),
            cascade_hlc: fence,
        },
    ).expect("sign CascadeUpgrade");

    apply_local_signed_group_op(&store, &op).expect("atomic cascade applies");

    for gid in [&r, &r_b, &r_b_b1] {
        let m = MetaRepository::new(&store).load(gid).unwrap().expect("meta");
        assert_eq!(m.app_key, APP_KEY_2);
        assert_eq!(m.target_application_id, app_id_2());
        assert_eq!(m.migration, Some(b"migrate_v2".to_vec()));
        let up = UpgradesRepository::new(&store).load(gid).unwrap().expect("upgrade record");
        assert_eq!(up.cascade_hlc, Some(fence));
    }
}
```

- [ ] **Step 4: Run, expect FAIL**

Run: `cargo test -p calimero-context --test cascade_atomic_apply cascade_upgrade_atomic_op`
Expected: FAIL — stub bails.

- [ ] **Step 5: Implement the atomic apply**

Create `crates/governance-store/src/ops/group/cascade_upgrade.rs`. Open the two existing modules and copy the exact `GroupApplyCtx` accessors, `crate::cascade::walk_for_predicate` call, the `MANAGE_APPLICATION` prescan, and `GroupSettingsService::{set_target_application,set_group_migration}` shapes — the `/* … */` below MUST be replaced with those real lines:

```rust
use calimero_governance_types::ApplicationId;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
use eyre::Result as EyreResult;

use crate::ops::group::GroupApplyCtx;
use crate::UpgradesRepository;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    from_app_key: &[u8; 32],
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
    migration: &Option<Vec<u8>>,
    cascade_hlc: HybridTimestamp,
) -> EyreResult<()> {
    let store = /* ctx store handle, per existing modules */;
    let signer = /* ctx.signer, per existing modules */;
    let group_id = /* ctx.group_id, per existing modules */;

    let entries = crate::cascade::walk_for_predicate(store, *group_id, *from_app_key)?;
    /* MANAGE_APPLICATION prescan over matched entries — copy verbatim, bail on first failure */;

    for entry in entries.iter().filter(|e| e.matched) {
        let gid = entry.group_id;
        /* GroupSettingsService::set_target_application(signer, *app_key, *target_application_id, gid) */;
        /* GroupSettingsService::set_group_migration(signer, migration.clone(), gid) */;

        let repo = UpgradesRepository::new(store);
        let mut value = repo.load(&gid)?.unwrap_or_else(|| GroupUpgradeValue {
            from_version: String::new(),
            to_version: String::new(),
            migration: migration.clone(),
            initiated_at: 0,
            initiated_by: signer,
            status: GroupUpgradeStatus::Completed { completed_at: None },
            cascade_hlc: None,
        });
        value.cascade_hlc = Some(cascade_hlc); // sticky: only ever set
        repo.save(&gid, &value)?;

        tracing::info!(?gid, "CascadeUpgrade: applied");
    }
    Ok(())
}
```

Wire the real dispatch arm (replace the Task 2 stub) in `crates/governance-store/src/ops/group.rs`:
```rust
        GroupOp::CascadeUpgrade { from_app_key, app_key, target_application_id, migration, cascade_hlc } =>
            cascade_upgrade::apply(ctx, from_app_key, app_key, target_application_id, migration, *cascade_hlc)?,
```
Add `mod cascade_upgrade;` alongside `mod cascade_target_application_set;`.

- [ ] **Step 6: Run, expect PASS (atomic op green)**

Run: `cargo test -p calimero-context --test cascade_atomic_apply cascade_upgrade_atomic_op`
Expected: PASS — this is the permanent regression guard going green. The characterization test from Step 1 is still red (the two-op path is unchanged); dispose of it in Step 9.

- [ ] **Step 7: Add a reverse-delivery convergence test**

Append a `#[tokio::test]` mirroring `crates/context/tests/cascade_concurrent_safety.rs:116-282` (DagStore + applier) delivering the op to a replica whose parent ops arrive out of order; assert each descendant ends with target+app_key+migration set AND identical `cascade_hlc`. Single op ⇒ ordering can't split the mutations.

Run: `cargo test -p calimero-context --test cascade_atomic_apply`
Expected: PASS (atomic + tokio tests; characterization still red until Step 9).

- [ ] **Step 8: Keep the two legacy variants (wire-compat)**

`grep -rn "CascadeTargetApplicationSet\|CascadeGroupMigrationSet" crates --include=*.rs`. Mark both variants `#[deprecated(note = "use CascadeUpgrade")]`; leave their apply arms in place for one release. Deletion is a follow-up once all nodes emit `CascadeUpgrade`.

- [ ] **Step 9: Dispose of the characterization test (keep CI green)**

The Step 1 characterization is red and the two-op apply path stays deprecated-but-present, so it would stay red. Convert it to documentation: add `#[ignore = "documents the pre-CascadeUpgrade two-op apply-order bug (xilosada #2507 item #3); cascade no longer emits these ops — see cascade_upgrade.rs"]` above its `#[test]`, and flip its assertion to the OBSERVED-buggy outcome so an accidental un-ignore is still meaningful:
```rust
    // two-op reverse delivery drops migration (the bug); asserted as the
    // observed behavior so this stays a faithful record, not a flaky red.
    assert_eq!(m.migration, None, "two-op reverse delivery drops migration (documented bug)");
```
Quote the original red run output in the PR description as the live-bug evidence.

- [ ] **Step 10: Commit**

```bash
git add crates/governance-store/src/ops/group/cascade_upgrade.rs crates/governance-store/src/ops/group.rs crates/context/tests/cascade_atomic_apply.rs
git commit -m "feat(governance-store): atomic CascadeUpgrade apply sets target+app_key+migration and records sticky cascade_hlc"
```

---

## Task 4 — Emit `CascadeUpgrade` from `dispatch_cascade` (stamp cascade_hlc once)

**Files:**
- Modify: `crates/context/src/handlers/upgrade_group.rs:1011-1056, 1069-1092`
- Test: extend `crates/context/tests/cascade_apply_walk.rs`

- [ ] **Step 1: Find the actor HLC accessor**

Run: `grep -n "hlc_timestamp\|HybridTimestamp\|next_hlc\|env::hlc" /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation/crates/context/src/handlers/upgrade_group.rs crates/context/src/handlers/execute/mod.rs`
Use the same accessor the execute/state-delta path uses to produce a real (non-zero) HLC for the cascade moment.

- [ ] **Step 2: Failing test**

Add to `cascade_apply_walk.rs` a test asserting the cascade emission produces a single `GroupOp::CascadeUpgrade` with non-zero `cascade_hlc`, and that after apply every matched descendant has `migration.is_some()` AND `cascade_hlc.is_some()`.

Run: `cargo test -p calimero-context --test cascade_apply_walk`
Expected: FAIL.

- [ ] **Step 3: Replace the two-op emit with one op**

In `dispatch_cascade` (`:1011-1056`), delete both legacy emits + the ORDER-MATTERS comment; compute `let cascade_hlc = /* accessor from Step 1 */;` once before the emit and the pre-spawn loop; emit:
```rust
        let report = calimero_governance_store::sign_apply_and_publish(
            &datastore_for_publish, &node_client_for_publish, &ack_router_for_publish,
            &group_id, &sk,
            GroupOp::CascadeUpgrade {
                from_app_key,
                app_key: new_app_key,
                target_application_id,
                migration: migration_bytes_for_publish.clone(),
                cascade_hlc,
            },
        ).await?;
        report.observe("upgrade_group", "CascadeUpgrade");
```
In the pre-spawn `GroupUpgradeValue` write (`:1069-1092`), set `cascade_hlc: Some(cascade_hlc)`.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p calimero-context --test cascade_apply_walk --test cascade_atomic_apply`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/handlers/upgrade_group.rs crates/context/tests/cascade_apply_walk.rs
git commit -m "feat(context): emit single atomic CascadeUpgrade op stamped with deterministic cascade_hlc"
```

---

## Task 5 — Generalize the local upgrade write-gate

**Files:**
- Modify: `crates/context/src/handlers/execute/mod.rs:128-214`
- Test: `crates/context/tests/cascade_apply_walk.rs`

- [ ] **Step 1: Read the gate**

Run: `sed -n '120,220p' /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation/crates/context/src/handlers/execute/mod.rs`
Confirm the `if !is_state_op { … InProgress … ExecuteError::UpgradeInProgress { group_id } }` block.

- [ ] **Step 2: Failing test**

Add a unit test on an extracted decision fn: `fn upgrade_blocks_write(status: &GroupUpgradeStatus) -> bool` returns true for `InProgress`, false for `Completed`.

Run: `cargo test -p calimero-context upgrade_blocks_write`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
/// A group upgrade in `InProgress` pauses ALL writes (calls and state ops).
fn upgrade_blocks_write(status: &GroupUpgradeStatus) -> bool {
    matches!(status, GroupUpgradeStatus::InProgress { .. })
}
```
Apply the check to state-ops too (drop the `!is_state_op` exclusion for the InProgress branch), returning `ExecuteError::UpgradeInProgress { group_id }`.

- [ ] **Step 4: Run, expect PASS**; **Step 5: Commit**

```bash
git add crates/context/src/handlers/execute/mod.rs crates/context/tests/cascade_apply_walk.rs
git commit -m "feat(context): refuse state-op writes (not just calls) while a context upgrade is InProgress"
```

---

## Task 6 — `get_cascade_status` RPC (actor handler **and** admin HTTP exposure)

**Files:**
- Create: `crates/context/src/handlers/get_cascade_status.rs` (actor handler)
- Modify: `crates/context/src/handlers.rs`; the context-client request-type crate (where `GetGroupUpgradeStatusRequest` lives)
- Create: `crates/server/src/admin/handlers/groups/get_cascade_status.rs` (HTTP handler)
- Modify: `crates/server/src/admin/handlers/groups.rs` (`pub mod get_cascade_status;`), `crates/server/src/admin/service.rs:228` (route), `crates/server/primitives/src/admin/mod.rs:1921` (`GetCascadeStatusApiResponse`)
- (Optional) `crates/meroctl/src/cli/...` + `crates/meroctl/src/output/groups.rs` — CLI command
- Test: `crates/context/tests/cascade_status_rpc.rs`

Reference the FULL exposure stack of the sibling RPC (mirror each layer):
`crates/context/src/handlers/get_group_upgrade_status.rs` → `crates/server/src/admin/handlers/groups/get_group_upgrade_status.rs` → `crates/server/src/admin/service.rs:228` (route registration) → `crates/server/primitives/src/admin/mod.rs:1921` (`GetGroupUpgradeStatusApiResponse`). Without the server layer, **no external client (calimero-client-py, meroctl, merobox) can reach the RPC** — only the internal actor can. Locate the request type: `grep -rn "struct GetGroupUpgradeStatusRequest" crates`.

- [ ] **Step 1: Define request/response types** (mirror `GetGroupUpgradeStatusRequest` derives + `Message` impl; reuse its status view DTO):
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetCascadeStatusRequest { pub namespace_id: ContextGroupId }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CascadeStatusEntry {
    pub group_id: ContextGroupId,
    pub status: GroupUpgradeStatusView, // the exact type get_group_upgrade_status maps into
    pub cascade_hlc: Option<HybridTimestamp>,
}
impl actix::Message for GetCascadeStatusRequest {
    type Result = eyre::Result<Vec<CascadeStatusEntry>>;
}
```

- [ ] **Step 2: Failing integration test** (`crates/context/tests/cascade_status_rpc.rs`): build `r`/`r_b`/`r_b_b1`, apply a `CascadeUpgrade` (as Task 3 Step 1), call `collect_cascade_status(&store, &r)`, assert 3 entries each with `cascade_hlc == Some(fence)`.

Run: `cargo test -p calimero-context --test cascade_status_rpc` → FAIL.

- [ ] **Step 3: Implement**
```rust
pub fn collect_cascade_status(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
) -> eyre::Result<Vec<CascadeStatusEntry>> {
    let mut groups = vec![*namespace_id];
    groups.extend(NamespaceRepository::new(store).collect_descendants(namespace_id)?);
    let repo = UpgradesRepository::new(store);
    let mut out = Vec::with_capacity(groups.len());
    for gid in groups {
        if let Some(v) = repo.load(&gid)? {
            out.push(CascadeStatusEntry { group_id: gid, status: v.status.clone().into(), cascade_hlc: v.cascade_hlc });
        }
    }
    Ok(out)
}
```
Add the `Handler` impl (membership-gate like `get_group_upgrade_status.rs:18-23`), `pub mod get_cascade_status;` in `handlers.rs`, a `ContextMessage::GetCascadeStatus` variant + dispatch arm.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p calimero-context --test cascade_status_rpc` → PASS.

- [ ] **Step 5: Expose via the admin HTTP API (so external clients can reach it)**

Mirror `get_group_upgrade_status`'s server layer exactly:
- Add `GetCascadeStatusApiResponse { data: Vec<CascadeStatusEntry> }` to `crates/server/primitives/src/admin/mod.rs` (next to `GetGroupUpgradeStatusApiResponse` at `:1921`).
- Create `crates/server/src/admin/handlers/groups/get_cascade_status.rs` mirroring `groups/get_group_upgrade_status.rs` (`:6-36`): take `namespace_id` from the path, call `context_client.get_cascade_status(GetCascadeStatusRequest { namespace_id })`, wrap in `GetCascadeStatusApiResponse`.
- Register `pub mod get_cascade_status;` in `crates/server/src/admin/handlers/groups.rs` and the route in `crates/server/src/admin/service.rs` (next to `:228` `get(groups::get_group_upgrade_status::handler)`).
- (Optional, recommended) add a `meroctl` subcommand mirroring `crates/meroctl/src/cli/group/upgrade.rs` + `output/groups.rs` so operators can query it.

Run: `cargo build -p calimero-server -p calimero-server-primitives -p meroctl` → compiles.

- [ ] **Step 6: Commit**

```bash
git add crates/context/src/handlers/get_cascade_status.rs crates/context/src/handlers.rs crates/context/tests/cascade_status_rpc.rs crates/server crates/meroctl
git commit -m "feat(context,server): get_cascade_status RPC + admin HTTP route reporting per-descendant status + cascade_hlc"
```

---

## Task 7 — Wire: stamp `producing_app_key` on the state delta

**Files:**
- Modify: `crates/node/primitives/src/sync/snapshot.rs:669-719` (`BroadcastMessage::StateDelta`)
- Modify: `crates/node/primitives/src/client.rs:491-538` (`broadcast`)
- Modify: `crates/node/src/handlers/state_delta/mod.rs:240-254` (`StateDeltaMessage`)
- Modify: `crates/context/src/handlers/execute/mod.rs:282-384, 852-868` (resolve + thread)

- [ ] **Step 1: Add the wire field**

In `BroadcastMessage::StateDelta` add `producing_app_key: Option<[u8; 32]>` (after `key_id`). In `StateDeltaMessage` (`:240`) add the same field, and add it to the destructure at `:597-611`.

- [ ] **Step 2: Add the `broadcast` param**

In `crates/node/primitives/src/client.rs`, give `broadcast(...)` a new param `producing_app_key: Option<[u8; 32]>` and set it in the constructed `BroadcastMessage::StateDelta { … producing_app_key }`.

- [ ] **Step 3: Extract a testable `resolve_producing_app_key` helper + thread it**

In `execute/mod.rs`, add a small store-aware helper (mirrors the `verify_position_group_id_matches_context` seam already in `state_delta/mod.rs` — a guard fn that's unit-tested via `register_context_in_group`, not the async actor):

```rust
/// The blob-derived app key the sender is executing under — `GroupMeta.app_key`
/// for the context's owning group (None for non-group contexts). Stamped onto
/// the broadcast so receivers can fence stale-schema deltas after a cascade.
fn resolve_producing_app_key(
    datastore: &calimero_store::Store,
    context_id: &calimero_primitives::context::ContextId,
) -> eyre::Result<Option<[u8; 32]>> {
    let Some(gid) = calimero_governance_store::get_group_for_context(datastore, context_id)? else {
        return Ok(None);
    };
    Ok(calimero_governance_store::MetaRepository::new(datastore)
        .load(&gid)?
        .map(|m| m.app_key))
}
```
Call it where the owning group is already resolved (~`:282`), capture the `Option<[u8;32]>` into the broadcast closure, and pass it to `.broadcast(...)` at `:852-868`.

- [ ] **Step 4: Unit-test the helper (sender-stamp coverage)**

Add an in-crate `#[cfg(test)]` test mirroring `group_id_check_tests` in `state_delta/mod.rs:1786-1809` (`fresh_store()` = `Store::new(Arc::new(InMemoryDB::owned()))`, then `calimero_governance_store::register_context_in_group` + a `MetaRepository::save` of a `GroupMetaValue { app_key: APP_KEY_2, .. }`):

```rust
#[test]
fn resolve_producing_app_key_returns_group_meta_app_key() {
    let store = fresh_store();
    let context_id = ContextId::from([0xF1; 32]);
    let group_id = ContextGroupId::from([0xF2; 32]);
    calimero_governance_store::register_context_in_group(&store, &group_id, &context_id).unwrap();
    MetaRepository::new(&store)
        .save(&group_id, &group_meta_with_app_key([0x22; 32])) // helper like cascade_apply_walk.rs `meta`
        .unwrap();
    assert_eq!(resolve_producing_app_key(&store, &context_id).unwrap(), Some([0x22; 32]));
}

#[test]
fn resolve_producing_app_key_none_for_non_group_context() {
    let store = fresh_store();
    let context_id = ContextId::from([0xF3; 32]); // never registered to a group
    assert_eq!(resolve_producing_app_key(&store, &context_id).unwrap(), None);
}
```
(Reuse/clone the `GroupMetaValue` builder from `crates/context/tests/cascade_apply_walk.rs`'s `meta` helper for `group_meta_with_app_key`.)

- [ ] **Step 5: Build + run, expect PASS**

Run: `cargo build -p calimero-node -p calimero-node-primitives -p calimero-context && cargo test -p calimero-context resolve_producing_app_key`
Expected: compiles (every `BroadcastMessage::StateDelta`/`StateDeltaMessage` construction + destructure now names `producing_app_key` — fix each surfaced site, pass `None` where the producing key isn't known); both helper tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/node/primitives/src/sync/snapshot.rs crates/node/primitives/src/client.rs crates/node/src/handlers/state_delta/mod.rs crates/context/src/handlers/execute/mod.rs
git commit -m "feat(node): stamp producing app_key on state-delta broadcast (lockstep wire add) + helper test"
```

---

## Task 8 — Pure `should_fence` + store-aware `delta_is_fenced`

**Files:**
- Create: `crates/context/src/hlc_fence.rs`; Modify `crates/context/src/lib.rs` (`pub mod hlc_fence;`)
- Test: inline `#[cfg(test)] mod tests` + `crates/context/tests/hlc_fence.rs`

- [ ] **Step 1: Failing unit tests** (inline in `hlc_fence.rs`):
```rust
#[cfg(test)]
mod tests {
    use super::should_fence;
    use calimero_storage::logical_clock::HybridTimestamp;

    fn hlc(n: u64) -> HybridTimestamp { /* construct a HybridTimestamp ordered by n; see logical_clock.rs API */ }

    #[test] fn fences_stale_schema_delta_after_boundary() {
        assert!(should_fence([1; 32], [2; 32], hlc(10), Some(hlc(5))));
    }
    #[test] fn does_not_fence_matching_app_key() {
        assert!(!should_fence([2; 32], [2; 32], hlc(10), Some(hlc(5))));
    }
    #[test] fn does_not_fence_at_or_before_boundary() {
        assert!(!should_fence([1; 32], [2; 32], hlc(5), Some(hlc(5))));
        assert!(!should_fence([1; 32], [2; 32], hlc(4), Some(hlc(5))));
    }
    #[test] fn does_not_fence_without_boundary() {
        assert!(!should_fence([1; 32], [2; 32], hlc(10), None));
    }
}
```
(For `hlc(n)`: confirm the `HybridTimestamp` construction API in `crates/storage/src/logical_clock.rs:134-178`; if only `zero()` + ordering helpers exist, build two distinct ordered values and parametrize the asserts accordingly rather than by integer.)

Run: `cargo test -p calimero-context hlc_fence` → FAIL (module absent).

- [ ] **Step 2: Implement**
```rust
//! State-delta HLC fence (roadmap §3.4): drop a delta that was produced under a
//! different app schema than the context now targets AND is newer than the
//! recorded cascade boundary. A `None` boundary never fences.

use calimero_governance_store::{get_group_for_context, MetaRepository, UpgradesRepository};
use calimero_primitives::context::ContextId;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::Store;

/// Pure two-condition rule. `cascade_hlc == None` ⇒ never fence.
#[must_use]
pub fn should_fence(
    delta_app_key: [u8; 32],
    ctx_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
    cascade_hlc: Option<HybridTimestamp>,
) -> bool {
    matches!(cascade_hlc, Some(boundary) if delta_app_key != ctx_app_key && delta_hlc > boundary)
}

/// Store-aware wrapper: resolves the context's current app_key + cascade_hlc and
/// applies `should_fence`. Non-group contexts / missing meta ⇒ never fence.
pub fn delta_is_fenced(
    store: &Store,
    context_id: &ContextId,
    producing_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
) -> eyre::Result<bool> {
    let Some(gid) = get_group_for_context(store, context_id)? else { return Ok(false) };
    let Some(meta) = MetaRepository::new(store).load(&gid)? else { return Ok(false) };
    let cascade_hlc = UpgradesRepository::new(store).load(&gid)?.and_then(|v| v.cascade_hlc);
    Ok(should_fence(producing_app_key, meta.app_key, delta_hlc, cascade_hlc))
}
```
Add `pub mod hlc_fence;` to `crates/context/src/lib.rs`.

- [ ] **Step 3: Run unit tests, expect PASS**

Run: `cargo test -p calimero-context hlc_fence` → PASS.

- [ ] **Step 4: Integration test for the wrapper** (`crates/context/tests/hlc_fence.rs`): build a group on `APP_KEY_2` with a `GroupUpgradeValue { cascade_hlc: Some(boundary), .. }` and a registered context (use `cascade_apply_walk.rs` helpers + the context-registration repo). Assert: a delta with `producing_app_key = APP_KEY_1`, `hlc > boundary` ⇒ `delta_is_fenced == true`; `producing_app_key = APP_KEY_2` ⇒ false; `hlc <= boundary` ⇒ false; a group with `cascade_hlc: None` ⇒ false.

Run: `cargo test -p calimero-context --test hlc_fence` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/hlc_fence.rs crates/context/src/lib.rs crates/context/tests/hlc_fence.rs
git commit -m "feat(context): pure should_fence + store-aware delta_is_fenced for the HLC fence"
```

---

## Task 9 — Apply the fence in the state-delta receive path (drop + warn + metric)

**Files:**
- Modify: `crates/node/src/handlers/state_delta/mod.rs` (after the envelope-sig check, ~`:642`)

Use the existing **drop pattern** (`warn! … return Ok(())`) at `:626-642` / `:657-666`; do NOT `bail!` (an `Err` risks marking the delta failed/zombie or wedging the actor).

- [ ] **Step 1: Confirm the datastore accessor**

Run: `grep -n "datastore()\|\.context\.datastore\|node_clients.context" /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation/crates/node/src/handlers/state_delta/mod.rs`
Confirm the `Store` handle reachable here (the explorer used `node_clients.context.datastore()`).

- [ ] **Step 2: Insert the fence**

After the envelope-signature verification block and before the delta is added to the DAG (`~:642`, ahead of the `CausalDelta` construction at `:804`):
```rust
    if let Some(producing_app_key) = producing_app_key {
        if calimero_context::hlc_fence::delta_is_fenced(
            node_clients.context.datastore(),
            &context_id,
            producing_app_key,
            hlc,
        )? {
            warn!(
                %context_id,
                %author_id,
                delta_id = ?delta_id,
                producing_app_key = %hex::encode(producing_app_key),
                "Dropping state delta — HLC fence: stale schema after cascade migration"
            );
            // metric: increment a fenced-delta counter (mirror the nearest existing
            // counter in this file; grep `metrics::` / `counter!` here for the macro in use).
            return Ok(());
        }
    }
```
Ensure `producing_app_key` is bound from the destructure (Task 7 Step 1) and `hex` is available (it is used elsewhere in the crate; add the dep if `cargo build` complains).

- [ ] **Step 3: Receive-path drop test (mirrors `group_id_check_tests`)**

Add to the existing `#[cfg(test)] mod tests` in `state_delta/mod.rs` (the one at `:1720` housing `group_id_check_tests`) a sibling `fence_drop_tests` submodule. It exercises the exact guard the receive path calls (`delta_is_fenced`) on realistic on-store state, via the same `fresh_store()` + `register_context_in_group` setup the existing guard test uses — no async actor needed. This is the ungated stand-in for the merobox `06` straggler e2e:

```rust
mod fence_drop_tests {
    use std::sync::Arc;

    use calimero_context::hlc_fence::delta_is_fenced;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{register_context_in_group, MetaRepository, UpgradesRepository};
    use calimero_primitives::context::ContextId;
    use calimero_storage::logical_clock::HybridTimestamp;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
    use calimero_store::Store;

    const APP_V1: [u8; 32] = [0x11; 32];
    const APP_V2: [u8; 32] = [0x22; 32];

    // `group_meta(app_key)` reuses the `meta` builder from
    // crates/context/tests/cascade_apply_walk.rs; `hlc_after_zero()` builds a
    // HybridTimestamp strictly greater than `zero()` per logical_clock.rs API.
    fn cascaded_store(boundary: Option<HybridTimestamp>) -> (Store, ContextId) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let context_id = ContextId::from([0xF1; 32]);
        let group_id = ContextGroupId::from([0xF2; 32]);
        register_context_in_group(&store, &group_id, &context_id).unwrap();
        MetaRepository::new(&store).save(&group_id, &group_meta(APP_V2)).unwrap(); // current target = v2
        if let Some(b) = boundary {
            UpgradesRepository::new(&store)
                .save(&group_id, &GroupUpgradeValue {
                    from_version: "1".into(),
                    to_version: "2".into(),
                    migration: None,
                    initiated_at: 0,
                    initiated_by: [0u8; 32].into(),
                    status: GroupUpgradeStatus::Completed { completed_at: None },
                    cascade_hlc: Some(b),
                })
                .unwrap();
        }
        (store, context_id)
    }

    #[test]
    fn drops_stale_v1_delta_after_cascade() {
        let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
        assert!(delta_is_fenced(&store, &ctx, APP_V1, hlc_after_zero()).unwrap());
    }

    #[test]
    fn keeps_current_v2_delta() {
        let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
        assert!(!delta_is_fenced(&store, &ctx, APP_V2, hlc_after_zero()).unwrap());
    }

    #[test]
    fn keeps_delta_when_no_cascade_recorded() {
        let (store, ctx) = cascaded_store(None);
        assert!(!delta_is_fenced(&store, &ctx, APP_V1, hlc_after_zero()).unwrap());
    }
}
```

- [ ] **Step 4: Build + run, expect PASS**

Run: `cargo build -p calimero-node && cargo test -p calimero-node fence_drop_tests`
Expected: compiles; the three drop tests PASS (and existing node tests stay green).

- [ ] **Step 5: Commit**

```bash
git add crates/node/src/handlers/state_delta/mod.rs
git commit -m "feat(node): HLC fence drops stale-schema state deltas after cascade migration + guard tests"
```

---

## Task 10 — Verification + e2e + C2 doc

**Files:**
- Modify: `workflows/app-migration/01-namespace-cascade-migration.yml`; doc comments on `pending_upgrade_target` + `CascadeUpgrade`.
- (Stretch) Create: `workflows/app-migration/06-fence-rejects-straggler.yml`.

- [ ] **Step 1: Full suite**

Run:
```bash
cd /Users/beast/Developer/Calimero/core/.worktrees/fix-app-key-creation
cargo test -p calimero-store -p calimero-governance-types -p calimero-governance-store -p calimero-context -p calimero-node
```
Expected: all PASS. Fix any non-exhaustive-match / missing-field fallout.

- [ ] **Step 2: Document the gate ↔ fence division (roadmap §C2)**

On `pending_upgrade_target` (`grep -rn pending_upgrade_target crates`): note it covers the **active-upgrade window** (declines all context-state sync while `application_id != target`); the **sticky HLC fence** covers the **post-`Completed` long tail** (late stragglers/offline writers) via `cascade_hlc`. They cover disjoint time windows ⇒ no double-rejection; the fence's `None`-boundary bypass means a context that hasn't applied the cascade never fences (the coarse gate/lazy-upgrade handles it). On `CascadeUpgrade`, note the lockstep wire assumption (schema v7) and the `producing_app_key` lockstep delta add.

- [ ] **Step 3: Strengthen workflow 01**

Update the receiver log assertion to the atomic op (`min_matches` = matched descendants = 2):
```yaml
  - name: Assert atomic cascade apply log on node 2 (receiver)
    type: assert_log_present
    nodes: [app-migration-cascade-2]
    patterns: ["CascadeUpgrade: applied"]
    min_matches: 2
```
Keep the existing `json_assert` (both layers, both nodes == `app_v2`).

- [ ] **Step 4: (Stretch) Fence-rejection workflow**

If a merobox primitive can pause/offline a node across a cascade (`grep -rn "stop_node\|pause\|offline\|set_node_clock_offset" $(python -c "import merobox,os;print(os.path.dirname(merobox.__file__))" 2>/dev/null)` or check the merobox step catalogue): author `06-fence-rejects-straggler.yml` — node 2 goes offline, node 1 cascades v1→v2, node 2 returns and attempts a v1 write, assert node 1 logs the `HLC fence` drop. If no primitive exists, SKIP and note it as a merobox#255 follow-up (do not fabricate a step type). Log the skip in the PR description.

- [ ] **Step 5: Run workflow 01 (if docker+merobox local)**

Run: `merobox bootstrap run workflows/app-migration/01-namespace-cascade-migration.yml --image merod:local --e2e-mode --verbose`
Expected: PASS. Otherwise rely on the `app-migration-e2e` CI matrix.

- [ ] **Step 6: Commit + push + PR**

```bash
git add workflows/app-migration crates
git commit -m "test(e2e): atomic CascadeUpgrade apply assertion; document gate vs HLC fence"
git push -u origin feat/cascade-atomic-op-fence-status
gh pr create --title "feat: atomic CascadeUpgrade op + cascade_hlc + HLC fence + get_cascade_status" --body-file <authored>
```
PR body: single chosen approach (no A/B/C, no spec doc in-branch); call out the two lockstep wire additions; link the xilosada #2507 review item the atomic op closes; note the fence's `None`-bypass / disjoint-window reasoning vs the coarse gate; note workflow `06` status (shipped or merobox-gated follow-up). Ready-for-review unless CI is concretely red.

---

## Cross-repo dependency chain (core → calimero-client-py → merobox)

**Required for PR-3 (core) to merge: NOTHING downstream.** Tasks 1–10 are Rust + workflow `01` (already-shipped steps). PR-3 is independently mergeable and CI-green. The downstream repos are a **follow-up** that turns on the e2e for the new surface.

**The chain (each step's client calls the layer above through HTTP):** merobox steps call core RPCs **through** `calimero_client_py` — `merobox/setup.py` pins `calimero-client-py>=0.6.11` and steps `from calimero_client_py import create_client`. So py client is a **required middle link**, not optional:

```
core actor handler + admin HTTP route (Task 6)
        │
        ▼
calimero-client-py: add get_cascade_status() binding   (new release, e.g. 0.6.17)
        │
        ▼
merobox#255: get_cascade_status / assert_cascade_complete steps (depends on the py method)
        │
        ▼
core CI: bump merobox pin, land gated 06 workflow + status e2e
```

**Ordering — does the py client come first?** **No.** For a *new* RPC the endpoint must exist in core first (you can't bind a client to a route that doesn't exist), so the order is strictly **core → py client → merobox**. The "client-first" pattern you may recall (task #52, the `cascade: true` field) was for an *existing* endpoint (`upgrade_group`) gaining a field — and even there core's `cascade: bool` API type (`server/primitives/src/admin/mod.rs:1881`) had to land for the client to send it. **#2507 left no py-client debt**: its only client surface (`cascade: true`) already shipped (task #52; core's `upgrade_group.rs:45 cascade: req.cascade` is present). The *only* new client surface is `get_cascade_status`, introduced by PR-3 — so its py-client method is part of *this* train, downstream of Task 6.

**Already shipped (no work):** `upgrade_group` with `cascade: true` (merobox 0.6.23 / py client); `assert_log_present` / `assert_log_absent` (merobox#243); `json_assert`, `wait_for_sync`, `call`, `create_namespace`/`join_*`, `get_group_info`.

**calimero-client-py — new work:** add a `get_cascade_status(namespace_id)` method wrapping the Task 6 admin route (mirror its existing `upgrade_group`/`get_group_upgrade_status` bindings), cut a release. **Depends on PR-3's server route.**

**merobox#255 — new step types (depend on the py release):**
1. **`get_cascade_status`** — wraps the py method.
2. **`assert_cascade_complete`** — asserts the status map is all-`Completed` (surfaces any `Failed`).
3. **A straggler/offline primitive for the fence e2e (`06`)** — the real need is `stop_node` / `start_node` (a node MISSES the cascade, returns on v1, authors a stale write). The roadmap's `set_node_clock_offset` alone is insufficient. Scenario: node-2 offline → node-1 cascades v1→v2 → node-2 back online (still v1) → node-2 writes (`hlc` now > `cascade_hlc`) → node-1 drops it → assert the fence-drop log. **Independent of core/py; buildable in parallel.** If merobox already exposes node stop/start, only the workflow YAML is needed.

**After downstream ships:** bump the merobox pin in core CI (`.github/actions/setup-merobox`), land the gated `06` workflow + a `get_cascade_status`/`assert_cascade_complete` e2e (roadmap §A6) as a core follow-up (tracks open task #51 / #2494).

## Out of scope (tracked elsewhere)

- **§A6 workflow `06`** if no merobox offline/clock primitive exists yet (merobox#255, §D) — best-effort in Task 10 Step 4, else follow-up.
- **`get_cascade_status` / `assert_cascade_complete` merobox step types** (§D) — different repo.
- **§C1** (reject migrate under `Automatic`/`Coordinated`) — small independent PR; complements Task 5.
- **§E** (close/trim #2494), **§B (PR-4)**, **§C3/§C4/§C5/§C6** — separate workstreams.
- **Deleting the legacy `CascadeTargetApplicationSet`/`CascadeGroupMigrationSet` variants** — wire-compat follow-up once all nodes emit `CascadeUpgrade`.

## Self-review

- **§A0 atomic op:** Tasks 2–4 (single op, reverse-order test, replaces two-op emit). ✓
- **§A1 sticky storage:** Task 1 (field + back-compat borsh + roundtrip). ✓
- **§A2 record on apply:** Task 3 Step 3 — per matched descendant, sticky, deterministic (carried on the op, stamped once). Recorded per-GROUP (not per-context) since `GroupUpgradeValue` is group-keyed and a group's contexts share an app — deliberate deviation from the roadmap's "per context" wording. ✓
- **§A3 fence:** Tasks 7 (wire), 8 (pure + wrapper, two-condition rule), 9 (drop+warn+metric in the receive path). Boundary (`==` ⇒ false), match-bypass, None-bypass all unit-tested. ✓
- **§A4 write-gate:** Task 5. ✓
- **§A5 status RPC:** Task 6. ✓
- **§C2:** Task 10 Step 2 (disjoint windows, no double-rejection). ✓
- **Placeholders:** `/* … */` in Task 3 Step 3 and the `hlc(n)` constructor in Task 8 are flagged "copy the real signature/API from the named file" — the accessor names live in files the executor opens. Every other step has concrete code + commands.
- **Type consistency:** `cascade_hlc: Option<HybridTimestamp>` (stored, Task 1) vs `cascade_hlc: HybridTimestamp` (on the wire op — always present on a cascade, Task 2) wrapped `Some(..)` on record (Task 3) and surfaced in the status entry (Task 6); `producing_app_key: Option<[u8;32]>` consistent across wire (Task 7), `should_fence`/`delta_is_fenced` (Task 8 — `[u8;32]` after the `if let Some` unwrap), and the receive-path call (Task 9). ✓
