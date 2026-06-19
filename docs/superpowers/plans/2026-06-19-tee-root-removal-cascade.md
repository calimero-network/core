# Scoped ReadOnlyTee Root-Removal Cascade (Fix A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** A namespace-root `MemberRemoved` for a `ReadOnlyTee` member cascades through ALL descendant subgroups (removing its rows + emitting per-subgroup `TeeMemberRemoved`), so a namespace owner can evict a TEE fleet node namespace-wide — including Restricted subgroups created by other members the owner doesn't admin. Normal members (Admin/Member/ReadOnly) keep today's behavior (root removal removes ONLY the root row), preserving the #2256 Restricted-subgroup membership wall.

**Architecture:** Mirror the existing self-`MemberLeft` namespace-leave cascade (`member_left.rs` `is_namespace_leave` block) inside the `MemberRemoved` apply, gated to `(namespace-root removal) AND (removed root-row role == ReadOnlyTee)`. The cascade is computed per-receiver on apply (each node walks its own tree via `collect_descendants`), so the owner never needs to be a member/admin of the subgroup — the subgroup creator's node removes the TEE locally when it applies the one authorized root op. Scoping to ReadOnlyTee is what makes crossing the Restricted wall sound: a TEE's subgroup membership came from namespace-level attestation policy, not the subgroup admin's choice.

**Tech Stack:** Rust 1.88.0 (fmt: `rustup run 1.88.0 cargo fmt`). No public-surface change (`OpEvent::{MemberRemoved,TeeMemberRemoved}` already exist). No mero-tee/mdma impact.

**Branch:** `feat/tee-lifecycle-leave-and-open-replication` (already has Fix B). This adds Fix A on top.

**Verified facts (from design investigation — authoritative):**
- Edit target: `crates/governance-store/src/ops/group/member_removed.rs`. Insert the cascade AFTER `removed_role` is captured (~line 61) and BEFORE the existing root-row mutation (~lines 70-75). The post-apply hash check + root-level event queue (lines ~100-120) stay put (hash invariant holds — cascade only touches descendant ids, not inputs to `compute_group_state_hash(group_id)`; same reasoning as member_left.rs:136-151).
- Template to mirror: `crates/governance-store/src/ops/group/member_left.rs:65-114` (the `is_namespace_leave` block: `collect_descendants` → per-direct-row `(sub, role)` capture → `cascade_remove_member_from_group_tree` + `remove_member` + `DenyListRepository::mark` + `queue_event(MemberRemoved{sub})` + role-gated `queue_event(TeeMemberRemoved{sub})`).
- Gate: `let is_namespace_root = NamespaceRepository::new(store).resolve(group_id)? == *group_id;` then `if is_namespace_root && removed_role == Some(GroupMemberRole::ReadOnlyTee) { ...cascade... }`. `removed_role` already captured at member_removed.rs:61 — reuse it. Namespace-root test is exactly member_left.rs:65's idiom.
- Per-descendant: re-read each descendant's role; only emit `TeeMemberRemoved{sub}` when THAT row's role is `ReadOnlyTee` (mirror member_left.rs:80-92/108-113). Cascade ENTRY is gated on the ROOT role (the security boundary); the per-descendant role gate is for the event only.
- DELIBERATELY OMIT from the mirrored block (do not copy from leave template): the owner-self check (member_left.rs:83-89 `OwnerOwnsSubgroup`) and the per-descendant `ensure_not_last_admin_removal` (member_left.rs:90-91) — a ReadOnlyTee is structurally never owner/admin, so both are dead weight and would mislead.
- Imports to add to member_removed.rs: `NamespaceRepository` (to the `use crate::{…}` group) and `calimero_context_config::types::ContextGroupId` (for the `Vec<(ContextGroupId, GroupMemberRole)>` local), mirroring member_left.rs:8,10.
- Authorization (NO change): root op `require_manage_members(signer)` at member_removed.rs:24-25 passes for root admin O; removing a ReadOnlyTee is not blocked (`require_admin_to_remove_admin`, `OwnerImmuneFromRemoval`, `ensure_not_last_admin_removal` are all inert for a non-admin non-owner TEE). The cascade does NOT re-check per-subgroup admin (consequence of the authorized root op, like member_left).
- Emit-after-persist (NO change): `ctx.queue_event` (context.rs:67-69, `GroupApplyCtx::pending_events`) is the same sink member_left/member_removed already use; drained post-persist at lib.rs:1394-1397 and namespace/governance.rs:1386-1388, dropped on the dedup branch (replay-safe). Queue cascade events BEFORE the existing root events (mirror member_left ordering) so the evicted TEE's per-subgroup self_purge events resolve before namespace finalization.
- #2726 reconcile (NO logic change): `namespace_needs_reconcile` (self_purge.rs:664-684) is root-only, never consults descendants → harmless no-op with or without residue. BUT amend the now-stale doc-comment at self_purge.rs:631-637 and :671-676 (it claims root removal removes ONLY the root row) to note the TEE-cascade split.
- self_purge idempotency (NO change): extra per-subgroup `TeeMemberRemoved` events → `purge_subgroup_for_self` (idempotent deletes of already-absent rows) → no error/spam. Safe.

---

### Task 1: Scoped ReadOnlyTee root-removal cascade + emission tests

**Files:**
- Modify: `crates/governance-store/src/ops/group/member_removed.rs`
- Modify (comment only): `crates/context/src/self_purge.rs` (doc-block ~631-637 and ~671-676)
- Test: `crates/governance-store/src/tests.rs` (next to existing emission tests ~6167-6265)

- [ ] **Step 1: Write the two failing tests** (TDD)

In `tests.rs`, next to `member_removed_op_emits_tee_event_for_readonly_tee_role` (~6171) and `member_removed_op_does_not_emit_tee_event_for_regular_member` (~6223), add:

**Test A — `member_removed_root_readonly_tee_cascades_into_restricted_subgroup`:** Build `ns_gid` (root) ─`NamespaceRepository::nest`─ `subgroup`; set `subgroup` `VisibilityMode::Restricted` via `CapabilitiesRepository::set_subgroup_visibility`. Admin O at root (use `sample_meta_with_admin`/the existing meta helper + `MetaRepository::save`). Add `tee_pk` as `GroupMemberRole::ReadOnlyTee` at BOTH `ns_gid` and `subgroup` via `MembershipRepository::add_member` directly (bypasses the attestation-only op guard — see tests.rs:6186/6292). `op_events::subscribe()` BEFORE apply. Sign `dummy_member_removed_op(tee_pk)` targeting `ns_gid` signed by O's sk; `apply_local_signed_group_op(&store, &op)`. Assert: `role_of(&subgroup,&tee_pk)` is `None` (cascade removed it); `role_of(&ns_gid,&tee_pk)` is `None`; `DenyListRepository::is_denied(&subgroup,&tee_pk)`; `count_removed_events_for(rx, subgroup.to_bytes(), tee_pk) == (1,1)`; root pair `(1,1)`.

**Test B — `member_removed_root_regular_member_does_not_cascade`:** Same tree; add `member_pk` as `GroupMemberRole::Member` at BOTH `ns_gid` and `subgroup`. Subscribe; apply root `dummy_member_removed_op(member_pk)` signed by O. Assert: `role_of(&ns_gid,&member_pk)` is `None` (root row removed — today's behavior); `role_of(&subgroup,&member_pk) == Some(Member)` (**subgroup preserved — Restricted wall holds**); `count_removed_events_for(rx, subgroup.to_bytes(), member_pk) == (0,0)`; root pair `(1,0)`.

Reuse existing helpers: `count_removed_events_for` (~6140), `test_store`, the meta helper, `nest`, `set_subgroup_visibility`, `add_member`, `dummy_member_removed_op`, `apply_local_signed_group_op`, `compute_state_hash`. Study tests.rs:6171 and 6223 and match their idiom exactly.

- [ ] **Step 2: Run, verify FAIL**

Run: `cd core && cargo test -p calimero-governance-store member_removed_root 2>&1 | tail -30`
Expected: Test A FAILS (subgroup row still present, no subgroup events — no cascade yet). Test B PASSES already (current behavior = no cascade).

- [ ] **Step 3: Implement the cascade**

In `member_removed.rs`, add imports (`NamespaceRepository` to the `use crate::{…}` group; `use calimero_context_config::types::ContextGroupId;`). After `removed_role` is captured (~line 61) and before the root-row mutation (~line 70), insert:

```rust
// A namespace-root removal of a ReadOnlyTee evicts it namespace-wide: the
// TEE's presence in any subgroup came from namespace-level attestation
// policy (tee_subgroup_admit), not the subgroup admin's choice, so root
// authority extends to it. Cascade per-receiver like a self-MemberLeft
// namespace-leave; scoped to ReadOnlyTee so normal-member Restricted
// membership autonomy (#2256) is untouched. Queue cascade events BEFORE
// the root events (below) so the evicted node's per-subgroup self_purge
// resolves before namespace finalization.
let is_namespace_root = NamespaceRepository::new(store).resolve(group_id)? == *group_id;
if is_namespace_root && removed_role == Some(GroupMemberRole::ReadOnlyTee) {
    let descendants = NamespaceRepository::new(store).collect_descendants(group_id)?;
    let mut direct: Vec<(ContextGroupId, GroupMemberRole)> = Vec::new();
    for sub in &descendants {
        if let Some(role) = MembershipRepository::new(store).role_of(sub, member)? {
            direct.push((*sub, role));
        }
    }
    for (sub, role) in &direct {
        cascade_remove_member_from_group_tree(store, sub, member)?;
        MembershipRepository::new(store).remove_member(sub, member)?;
        DenyListRepository::new(store).mark(sub, member)?;
        ctx.queue_event(crate::op_events::OpEvent::MemberRemoved {
            group_id: sub.to_bytes(),
            member: *member,
        });
        if *role == GroupMemberRole::ReadOnlyTee {
            ctx.queue_event(crate::op_events::OpEvent::TeeMemberRemoved {
                group_id: sub.to_bytes(),
                member: *member,
            });
        }
    }
}
```

Verify the exact names of `cascade_remove_member_from_group_tree`, `DenyListRepository`, `role_of`, `collect_descendants`, `to_bytes`, and the `OpEvent` variants against member_left.rs and adjust to match its actual calls verbatim. Do NOT add the owner-self or last-admin checks.

- [ ] **Step 4: Amend the stale #2726 doc-comment**

In `self_purge.rs`, update the doc-block (~631-637) and inline comment (~671-676) that assert "a namespace-root `MemberRemoved` apply removes ONLY the root `GroupMember` row" — add: for a **ReadOnlyTee** root removal the apply now cascades and removes descendant rows in-band (no residue); for non-TEE removals descendant rows still survive as residue; either way `namespace_needs_reconcile` is correct because it only consults the root row. Comment-only; no logic change.

- [ ] **Step 5: Run, verify PASS**

Run: `cd core && cargo test -p calimero-governance-store member_removed 2>&1 | tail -30` (both new tests + existing member_removed tests green). Then `cargo test -p calimero-governance-store 2>&1 | grep "test result:" | tail`.

- [ ] **Step 6: fmt + clippy + commit**

`cd core && rustup run 1.88.0 cargo fmt && cargo clippy -p calimero-governance-store 2>&1 | tail -20`
```
git add crates/governance-store/src/ops/group/member_removed.rs crates/context/src/self_purge.rs crates/governance-store/src/tests.rs docs/superpowers/plans/2026-06-19-tee-root-removal-cascade.md
git commit -m "feat(governance): cascade root MemberRemoved into subgroups for ReadOnlyTee

A namespace-root MemberRemoved of a ReadOnlyTee now cascades through all
descendant subgroups (removing its rows + emitting per-subgroup
TeeMemberRemoved), mirroring the self-MemberLeft namespace-leave cascade.
This lets a namespace owner evict a TEE fleet node namespace-wide,
including Restricted subgroups created by other members the owner does
not admin — sound because a TEE's subgroup membership derives from
namespace-level attestation policy, not the subgroup admin. Scoped to
ReadOnlyTee; normal-member Restricted autonomy (#2256) is unchanged."
```

---

### Task 2: Broader regression check

- [ ] `cd core && cargo test -p calimero-governance-store -p calimero-context -p calimero-node 2>&1 | grep -E "test result:|error" | tail -30` — all green (esp. existing member_left/member_removed/self_purge/#2726 reconcile tests).
- [ ] `cd core && rustup run 1.88.0 cargo fmt --check` — clean.
- [ ] `cd core && cargo check --workspace 2>&1 | tail -5` — clean.
