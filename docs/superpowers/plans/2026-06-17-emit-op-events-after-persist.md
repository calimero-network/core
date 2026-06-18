# Emit OpEvents After Op-Log Persist (#2770) — Implementation Plan (PR-1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop emitting governance `OpEvent`s before the op-log entry is persisted — collect events during apply and flush them only after the op-log append succeeds, on both the group-op path and the namespace RootOp path.

**Architecture:** Mirror the existing `GroupApplyCtx.divergence` out-parameter: each apply context grows a `pending_events: Vec<OpEvent>` sink + a `queue_event` helper. Per-op handlers push instead of calling `op_events::notify`. After the op-log persists (append branch only), the apply functions drain the sink and notify. Incremental + green-throughout: an unconverted site keeps emitting before persist (pre-existing behavior), a converted site emits after — both still fire, so the tree compiles and existing tests pass at every step.

**Tech Stack:** Rust (toolchain 1.88.0 — fmt via `rustup run 1.88.0 cargo fmt`), `calimero-governance-store`, `tokio::sync::broadcast` op-events, `cargo test`.

**Scope guardrails:**
- Spec: `docs/superpowers/specs/2026-06-17-tee-lifecycle-correctness-design.md` (PR-1).
- `GroupKeyDelivered` (`governance.rs:1026-1032`) stays a direct `notify_op_event` — off the op-append path, latency-sensitive `join_group` wake. **Do not touch it.**
- Drain **only on the append branch**, never on the content-hash dedup early-return (that's the replay path — stops replay re-emits, the intended net-positive semantic change).
- Keep emit **inside** the synchronous apply call (drain before the apply fn returns) — do NOT move emit to the caller, or the subscribe-then-apply-then-drain tests break.
- Out of scope: removing the `tee_subgroup_admit` wake-then-reread (follow-up once PR-1 + #2772 land); PR-2 (#2726), PR-3 (#2771).
- No AI attribution in commits. No published-surface change (`calimero-server-primitives`/`calimero-tee-attestation`), no mero-tee rev bump.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/governance-store/src/ops/group/context.rs` | Modify | `GroupApplyCtx`: add `pending_events` + `queue_event`. |
| `crates/governance-store/src/lib.rs` | Modify | `apply_group_op_mutations` returns events; `apply_local_signed_group_op` drains after persist; `emit_auto_follow_set_if_enabled` → returns `Option<OpEvent>`. |
| `crates/governance-store/src/namespace/governance.rs` | Modify | `apply_group_op_inner` drains after persist; `apply_signed_op`/`apply_root_op` collect + drain RootOp events after `store_operation`. |
| `crates/governance-store/src/ops/group/{member_added,member_joined_via_tee_attestation,member_removed,member_left,member_set_auto_follow,context_registered}.rs` | Modify | Push group events into the ctx sink. |
| `crates/governance-store/src/ops/namespace/context.rs` | Modify | `NamespaceApplyCtx`: add `pending_events` + `queue_event`. |
| `crates/governance-store/src/ops/namespace.rs` | Modify | `dispatch_root_op` + handlers → `&mut NamespaceApplyCtx`. |
| `crates/governance-store/src/ops/namespace/{group_created,group_reparented,member_joined}.rs` | Modify | Push namespace events into the ctx sink. |
| `crates/governance-store/src/namespace/membership.rs` | Modify | `NamespaceMembershipService` member-joined returns the auto-follow event instead of notifying. |
| `crates/governance-store/src/tests.rs` | Modify | New behavioral tests (race-gone, no-re-emit-on-replay, namespace event-after-reorder). |

---

## Task 1: Group-path plumbing (sink + drain, no sites converted yet)

Lays the group infrastructure with zero behavior change (the sink stays empty until Task 2).

**Files:** `ops/group/context.rs`, `lib.rs`.

- [ ] **Step 1: Add the sink to `GroupApplyCtx`**

In `crates/governance-store/src/ops/group/context.rs`, add a field after `divergence` and a method in the impl:

```rust
    /// Op-events queued during apply, flushed by the caller AFTER the
    /// op-log entry is persisted (see #2770). Mirrors `divergence`'s
    /// out-parameter pattern. Handlers MUST `queue_event` rather than
    /// calling `op_events::notify` directly, or they reintroduce the
    /// emit-before-persist race.
    pub(crate) pending_events: Vec<crate::op_events::OpEvent>,
```

In `new(...)`, add `pending_events: Vec::new(),` to the struct literal. Add the method to `impl<'a> GroupApplyCtx<'a>`:

```rust
    pub(crate) fn queue_event(&mut self, event: crate::op_events::OpEvent) {
        self.pending_events.push(event);
    }
```

- [ ] **Step 2: Return the events from `apply_group_op_mutations`**

In `crates/governance-store/src/lib.rs`, change `apply_group_op_mutations` (currently `lib.rs:1203-1212`) to:

```rust
fn apply_group_op_mutations(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    op: &GroupOp,
) -> EyreResult<(bool, Option<DivergenceReport>, Vec<crate::op_events::OpEvent>)> {
    let mut ctx = ops::group::GroupApplyCtx::new(store, group_id, signer);
    let handled = ops::group::dispatch(&mut ctx, op)?;
    Ok((handled, ctx.divergence, ctx.pending_events))
}
```

- [ ] **Step 3: Drain at the `lib.rs` flush site (append branch only)**

In `apply_local_signed_group_op`, update the destructure (currently `let (handled, _divergence) = apply_group_op_mutations(...)?;` at `lib.rs:1272`) to capture the events, and drain them after the append. Keep the dedup early-return un-drained:

```rust
    let (handled, _divergence, pending_events) =
        apply_group_op_mutations(store, &group_id, &op.signer, &op.op)?;
```

In the dedup early-return branch (`if op_log_contains_content_hash(...)? { store_nonce_window(...)?; return Ok(()); }` at ~1313-1316) — leave it exactly as-is (do NOT drain; this is the replay path). After `persist_group_governance_progress(...)?` (~1340-1348) and before the final `Ok(())`, add:

```rust
    // #2770: flush events only after the op-log entry is durably appended.
    for event in pending_events {
        crate::op_events::notify(event);
    }
    Ok(())
```

- [ ] **Step 4: Drain at the `governance.rs` group flush site (inside the append block)**

In `crates/governance-store/src/namespace/governance.rs` `apply_group_op_inner`, update the destructure (currently `let (handled, divergence) = apply_group_op_mutations(...)?;` at ~1275) to:

```rust
        let (handled, divergence, pending_events) =
            apply_group_op_mutations(self.store, group_id, signer, op)?;
```

Inside the existing `if handled { ... if !already_logged { ... persist_group_op_log_entry(...)?; } }` block, immediately AFTER `persist_group_op_log_entry(...)?` (~1365-1367) and still inside the `if !already_logged` body, add:

```rust
                // #2770: flush after the op-log append; a re-received op
                // (already_logged) drops its queued events (no re-emit).
                for event in pending_events {
                    crate::op_events::notify(event);
                }
```

> NOTE: `pending_events` is moved into the `if !already_logged` block, so the `already_logged` (replay) and `!handled` paths drop it without notifying — exactly the desired no-re-emit-on-replay. Confirm the borrow checker is happy (the vec is consumed once); if `pending_events` is referenced after the block elsewhere, it isn't here. The `divergence` return (`Ok(divergence)`) is unchanged.

- [ ] **Step 5: Verify it compiles + existing tests green (no behavior change yet)**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store`
Expected: PASS. The sink is empty (no sites converted), so events still fire from the old `notify` calls in the handlers; this task only adds dormant plumbing.

- [ ] **Step 6: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-governance-store
git add crates/governance-store/src/ops/group/context.rs crates/governance-store/src/lib.rs crates/governance-store/src/namespace/governance.rs
git commit -m "refactor(governance-store): add group-apply event sink + post-persist drain (#2770)"
```

---

## Task 2: Convert the direct group emit sites + behavioral tests

Move the 8 direct group emit sites from `notify` to `ctx.queue_event` (the auto-follow helper is Task 3). Each converted event now fires after persist. Add the race-gone + no-re-emit tests.

**Files:** `ops/group/{member_added,member_joined_via_tee_attestation,member_removed,member_left,member_set_auto_follow,context_registered}.rs`; `tests.rs`.

- [ ] **Step 1: Write the failing behavioral test (race-gone)**

In `crates/governance-store/src/tests.rs`, add a test that proves the op-log entry is visible when the event fires. Model it on the existing `end_to_end_event_fires_after_op_apply` (~5488) — subscribe, apply an op, drain the event with a deadline — but in the event handler, assert the op-log already contains the op:

```rust
#[test]
fn member_added_event_fires_after_op_log_append() {
    // #2770: when MemberAdded fires, the op-log entry must already be persisted.
    let (store, gid, _admin_pk, admin_sk, member_pk) = seed(&mut OsRng); // reuse the existing seed helper
    let mut rx = crate::op_events::subscribe();

    let op = SignedGroupOp::sign(
        &admin_sk, gid.to_bytes(), vec![], [0u8; 32], 1,
        GroupOp::MemberAdded { member: member_pk, role: GroupMemberRole::Member },
    ).unwrap();
    let content_hash = op.content_hash().unwrap();
    apply_local_signed_group_op(&store, &op).unwrap();

    // Drain to the MemberAdded for our (gid, member) within a deadline.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        assert!(std::time::Instant::now() < deadline, "MemberAdded never fired");
        if let Ok(crate::op_events::OpEvent::MemberAdded { group_id, member, .. }) = rx.try_recv() {
            if group_id == gid.to_bytes() && member == member_pk {
                // The load-bearing assertion: op-log already has the entry.
                assert!(
                    crate::local_state::op_log_contains_content_hash(&store, &gid, &content_hash).unwrap(),
                    "op-log entry must be persisted before MemberAdded fires (#2770)"
                );
                break;
            }
        }
    }
}
```

> NOTE: match the EXACT names of the existing `seed` helper + `SignedGroupOp::sign` arg order used by neighbors in `tests.rs` (the dossier shows `end_to_end_event_fires_after_op_apply` ~5488 and `member_added_emits_synthesized_auto_follow_set` ~5595 — copy their fixture + sign pattern). If `op_log_contains_content_hash` is module-private, use the in-crate path it's defined at (`crate::local_state::op_log_contains_content_hash` or wherever `tests.rs` can reach it).

- [ ] **Step 2: Run it — fails today (event fires before persist)**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store member_added_event_fires_after_op_log_append -- --nocapture`
Expected: FAIL — the assertion inside the handler fails (event currently fires before the op-log append).

- [ ] **Step 3: Convert the 8 direct group emit sites**

Each `crate::op_events::notify(crate::op_events::OpEvent::X { ... });` becomes `ctx.queue_event(crate::op_events::OpEvent::X { ... });` (handlers already hold `&mut ctx`). Convert verbatim:

- `ops/group/member_added.rs:43` — `MemberAdded { group_id: group_id.to_bytes(), member: *member, role: role.clone() }`.
- `ops/group/member_joined_via_tee_attestation.rs:43` — `TeeMemberAdmitted { group_id: group_id.to_bytes(), member: *member }`.
- `ops/group/member_removed.rs:107` — `MemberRemoved { group_id: group_id.to_bytes(), member: *member }`; and `:116` — `TeeMemberRemoved { group_id: group_id.to_bytes(), member: *member }`.
- `ops/group/member_left.rs:104` (loop) — `MemberRemoved { group_id: sub.to_bytes(), member: *member }`; `:109` — `TeeMemberRemoved { group_id: sub.to_bytes(), member: *member }`; `:159` — `MemberRemoved { group_id: group_id.to_bytes(), member: *member }`; `:166` — `TeeMemberRemoved { group_id: group_id.to_bytes(), member: *member }`. **Preserve insertion order** (MemberRemoved before TeeMemberRemoved is contractual) — `Vec` push order does this naturally.
- `ops/group/member_set_auto_follow.rs:40` — `AutoFollowSet { group_id: group_id.to_bytes(), member: *target, contexts: *auto_follow_contexts, subgroups: *auto_follow_subgroups }`.
- `ops/group/context_registered.rs:30` — `ContextRegistered { group_id: group_id.to_bytes(), context_id: *context_id }`. **Leave `crate::registration_notify::notify(*context_id);` at `:29` untouched** (different channel).

Some handlers reference `store`/`group_id` directly; they also have `ctx` in scope (the `&mut GroupApplyCtx` param). Use `ctx.queue_event(...)`.

- [ ] **Step 4: Run the new test + existing op-event tests**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store member_added_event_fires_after_op_log_append`
Expected: PASS (event now fires after persist).
Run: `rustup run 1.88.0 cargo test -p calimero-governance-store` — all existing op-event tests still green.

- [ ] **Step 5: Add the no-re-emit-on-replay test**

```rust
#[test]
fn replayed_group_op_does_not_re_emit() {
    // #2770: re-applying an already-logged op must NOT re-fire its event.
    let (store, gid, _admin_pk, admin_sk, member_pk) = seed(&mut OsRng);
    let op = SignedGroupOp::sign(
        &admin_sk, gid.to_bytes(), vec![], [0u8; 32], 1,
        GroupOp::MemberAdded { member: member_pk, role: GroupMemberRole::Member },
    ).unwrap();
    apply_local_signed_group_op(&store, &op).unwrap(); // first apply emits

    let mut rx = crate::op_events::subscribe(); // subscribe AFTER first apply
    apply_local_signed_group_op(&store, &op).unwrap(); // replay (already logged)

    // No MemberAdded for (gid, member) should arrive in a short window.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    while std::time::Instant::now() < deadline {
        if let Ok(crate::op_events::OpEvent::MemberAdded { group_id, member, .. }) = rx.try_recv() {
            assert!(
                !(group_id == gid.to_bytes() && member == member_pk),
                "replay must not re-emit MemberAdded (#2770)"
            );
        }
    }
}
```

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store replayed_group_op_does_not_re_emit`
Expected: PASS.

- [ ] **Step 6: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-governance-store
git add crates/governance-store/src/ops/group/ crates/governance-store/src/tests.rs
git commit -m "refactor(governance-store): queue group op-events for post-persist flush (#2770)"
```

---

## Task 3: AutoFollowSet helper returns the event (group callers)

`emit_auto_follow_set_if_enabled` does a post-mutation read and conditionally notifies. Change it to RETURN the event so callers push into their sink (decouples it from ctx type — needed because it's also called on the namespace path in Task 5).

**Files:** `lib.rs`; `ops/group/{member_added,member_joined_via_tee_attestation}.rs`.

- [ ] **Step 1: Refactor the helper to return `Option<OpEvent>`**

In `crates/governance-store/src/lib.rs`, replace `emit_auto_follow_set_if_enabled` (1158-1194) with a builder that returns the event (keep the best-effort read-error → `None` swallow; rename to reflect it no longer notifies):

```rust
pub(crate) fn build_auto_follow_set_if_enabled(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<crate::op_events::OpEvent>> {
    let value = match MembershipRepository::new(store).member_value(group_id, member) {
        Ok(Some(v)) => v,
        Ok(None) => {
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()), %member,
                "post-apply read found no member row — skipping auto-follow emission"
            );
            return Ok(None);
        }
        Err(err) => {
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()), %member, ?err,
                "post-apply read failed — skipping auto-follow emission"
            );
            return Ok(None);
        }
    };
    if value.auto_follow.contexts {
        Ok(Some(crate::op_events::OpEvent::AutoFollowSet {
            group_id: group_id.to_bytes(),
            member: *member,
            contexts: true,
            subgroups: value.auto_follow.subgroups,
        }))
    } else {
        Ok(None)
    }
}
```

- [ ] **Step 2: Update the two group callers to push into the sink**

In `ops/group/member_added.rs:59` and `ops/group/member_joined_via_tee_attestation.rs:56`, replace `emit_auto_follow_set_if_enabled(store, group_id, member)?;` with:

```rust
    if let Some(event) = crate::build_auto_follow_set_if_enabled(ctx.store(), ctx.group_id(), member)? {
        ctx.queue_event(event);
    }
```

(Use `ctx.store()` / `ctx.group_id()` accessors — they exist on `GroupApplyCtx`. Adjust the path to `build_auto_follow_set_if_enabled` to however these modules reference crate-root fns — neighbors already call `emit_auto_follow_set_if_enabled` so use the same path with the new name.)

- [ ] **Step 3: Leave the namespace caller temporarily compiling**

`crates/governance-store/src/namespace/membership.rs:71` also calls the old fn. To keep the tree green until Task 5 fully refactors that path, update it minimally to use the new builder + notify directly (it still emits before persist on the namespace path for now — acceptable transient, fixed in Task 5):

```rust
    if let Some(event) = crate::build_auto_follow_set_if_enabled(self.store, &group_id, member)? {
        crate::op_events::notify(event);
    }
```

- [ ] **Step 4: Build + existing auto-follow test green**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store member_added_emits_synthesized_auto_follow_set`
Expected: PASS (auto-follow events still fire — now after persist on the group path).
Run: `rustup run 1.88.0 cargo test -p calimero-governance-store` — green.

- [ ] **Step 5: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-governance-store
git add crates/governance-store/src/lib.rs crates/governance-store/src/ops/group/ crates/governance-store/src/namespace/membership.rs
git commit -m "refactor(governance-store): auto-follow-set returns event for sink (#2770)"
```

---

## Task 4: Namespace-path plumbing (sink + drain, no sites converted yet)

Thread a sink through the RootOp path. Dormant until Task 5.

**Files:** `ops/namespace/context.rs`, `ops/namespace.rs`, all namespace handler signatures, `namespace/governance.rs`.

- [ ] **Step 1: Add the sink to `NamespaceApplyCtx`**

In `crates/governance-store/src/ops/namespace/context.rs`, add a field + method (it has no `divergence` analog today):

```rust
// in the struct:
    pending_events: Vec<crate::op_events::OpEvent>,
// in new(): pending_events: Vec::new(),
// in impl:
    pub(crate) fn queue_event(&mut self, event: crate::op_events::OpEvent) {
        self.pending_events.push(event);
    }
    pub(crate) fn take_events(&mut self) -> Vec<crate::op_events::OpEvent> {
        std::mem::take(&mut self.pending_events)
    }
```

- [ ] **Step 2: Flip `dispatch_root_op` + all namespace handlers to `&mut`**

In `crates/governance-store/src/ops/namespace.rs`, change `dispatch_root_op(ctx: &NamespaceApplyCtx<'_>, ...)` to `ctx: &mut NamespaceApplyCtx<'_>`. Then change EVERY per-op handler signature it calls to take `&mut NamespaceApplyCtx<'_>`: `group_created::apply`, `group_deleted::apply`, `group_reparented::apply`, `admin_changed::apply`, `policy_updated::apply`, `member_joined::apply`, `member_joined_open::apply` (all in `ops/namespace/*.rs`). Handlers that don't emit just take `&mut` and ignore it — required because they share the one dispatch signature.

- [ ] **Step 3: Make `apply_root_op` build a mutable ctx and return the events**

In `crates/governance-store/src/namespace/governance.rs`, change `apply_root_op` (1405-1453) to return the collected events:

```rust
    fn apply_root_op(&self, op: &SignedNamespaceOp, root: &RootOp)
        -> EyreResult<Vec<crate::op_events::OpEvent>> {
        // ... existing staleness telemetry ...
        let mut ctx =
            super::super::ops::namespace::NamespaceApplyCtx::new(self.store, self.namespace_id);
        super::super::ops::namespace::dispatch_root_op(&mut ctx, op, root)?;
        Ok(ctx.take_events())
    }
```

- [ ] **Step 4: Collect + drain in `apply_signed_op` after `store_operation`**

In `apply_signed_op`, capture the events from the Root arm and flush them after the durable append. Change the `NamespaceOp::Root(root) => { self.apply_root_op(op, root)?; }` arm (~240) to bind the events into a function-scoped vec declared before the `match`:

```rust
        let mut root_events: Vec<crate::op_events::OpEvent> = Vec::new();
        match &op.op {
            NamespaceOp::Root(root) => {
                root_events = self.apply_root_op(op, root)?;
            }
            // ... other arms unchanged ...
        }
```

Then after `self.store_operation(op)?;` (~425) and before `Ok(result)`:

```rust
        // #2770: flush RootOp-path events only after the namespace op is appended.
        for event in root_events {
            crate::op_events::notify(event);
        }
        Ok(result)
```

> NOTE: confirm the exact shape of the `match &op.op` arms in `apply_signed_op` (the dossier shows the Root arm; other arms like `Group { .. }` go through `decrypt_and_apply_group_op` and produce no namespace-ctx events, so `root_events` stays empty for them — correct). Keep the existing `?` error propagation.

- [ ] **Step 5: Compiles + existing namespace tests green (no behavior change)**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store`
Expected: PASS. Sink is empty (no namespace sites converted yet); events still fire from the old `notify_op_event` calls.

- [ ] **Step 6: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-governance-store
git add crates/governance-store/src/ops/namespace/ crates/governance-store/src/ops/namespace.rs crates/governance-store/src/namespace/governance.rs
git commit -m "refactor(governance-store): add namespace-apply event sink + post-persist drain (#2770)"
```

---

## Task 5: Convert the namespace emit sites + behavioral test

Move the 3 namespace emit sites to the sink. Add the namespace event-after-reorder test.

**Files:** `ops/namespace/{group_created,group_reparented,member_joined}.rs`, `namespace/membership.rs`, `tests.rs`.

- [ ] **Step 1: Convert `SubgroupCreated` and `SubgroupReparented`**

- `ops/namespace/group_created.rs:121` — replace `notify_op_event(OpEvent::SubgroupCreated { namespace_id, parent_group_id: parent_id, child_group_id: group_id });` with `ctx.queue_event(OpEvent::SubgroupCreated { namespace_id, parent_group_id: parent_id, child_group_id: group_id });` (handler now holds `&mut ctx`).
- `ops/namespace/group_reparented.rs:22` — replace `notify_op_event(OpEvent::SubgroupReparented { namespace_id: ctx.namespace_id(), old_parent_group_id: old_parent.to_bytes(), new_parent_group_id: new_parent_id, child_group_id });` with `ctx.queue_event(OpEvent::SubgroupReparented { ... same fields ... });`.

- [ ] **Step 2: Convert the MemberJoined auto-follow path**

`member_joined::apply` (`ops/namespace/member_joined.rs`) delegates to `NamespaceMembershipService` (`namespace/membership.rs`), which currently builds + notifies the auto-follow event (the Task-3 transient). Refactor so the service RETURNS the event and `member_joined::apply` pushes it into `ctx`:

- In `crates/governance-store/src/namespace/membership.rs`, change the member-joined method (around `:62-72`) to return `EyreResult<Option<crate::op_events::OpEvent>>`: replace the `if let Some(event) = ... { crate::op_events::notify(event); } Ok(())` (the Task-3 transient) with `Ok(crate::build_auto_follow_set_if_enabled(self.store, &group_id, member)?)` (return it; the `add_member` call above stays).
- In `ops/namespace/member_joined.rs`, where it calls the service's member-joined method, capture the returned `Option` and push: `if let Some(event) = service.apply_member_joined(...)? { ctx.queue_event(event); }`. (Match the actual method/var names in the file; `member_joined_open.rs` if it shares the path needs the same treatment — check it.)

> NOTE: read `ops/namespace/member_joined.rs` + `namespace/membership.rs` to get the exact method name + call shape. The goal: the auto-follow event for a namespace MemberJoined ends up in the RootOp sink (flushed after `store_operation`), not notified inline.

- [ ] **Step 3: Write the namespace event-after-reorder test**

In `tests.rs`, mirror an existing `SubgroupCreated`-observing test (namespace/tests has the apply harness) — apply a `RootOp::GroupCreated` and assert the `SubgroupCreated` event still arrives AND the namespace op-log already contains the op when it fires:

```rust
#[test]
fn subgroup_created_event_fires_after_namespace_op_persist() {
    // #2770: SubgroupCreated must still be observed, and the namespace op
    // must already be in the log when it fires.
    // (Use the existing namespace apply-harness: build a namespace, subscribe,
    //  apply a signed RootOp::GroupCreated, drain SubgroupCreated within a
    //  deadline, and assert NamespaceOpLogService::contains_op(delta_id) is true
    //  inside the handler. Copy the harness/seed from the existing namespace
    //  tests that exercise GroupCreated apply.)
}
```

> Fill the body from the existing namespace-apply test pattern (the harness that signs + applies a `SignedNamespaceOp` and the `op_events::subscribe()` + drain idiom). The load-bearing assertion: `NamespaceOpLogService::new(&store, ns_id).contains_op(delta_id).unwrap()` is true at the moment `SubgroupCreated` fires.

- [ ] **Step 4: Run the namespace tests**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store subgroup_created_event_fires_after_namespace_op_persist -- --nocapture`
Expected: PASS.
Run: `rustup run 1.88.0 cargo test -p calimero-governance-store` — all green (incl. existing SubgroupCreated/SubgroupReparented/MemberJoined tests after the reorder).

- [ ] **Step 5: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-governance-store
git add crates/governance-store/src/ops/namespace/ crates/governance-store/src/namespace/membership.rs crates/governance-store/src/tests.rs
git commit -m "refactor(governance-store): queue namespace op-events for post-persist flush (#2770)"
```

---

## Final verification

- [ ] **Whole-affected-crate + dependents (the subscribers live in calimero-context/node)**

```bash
rustup run 1.88.0 cargo fmt --all -- --check
rustup run 1.88.0 cargo test -p calimero-governance-store -p calimero-context -p calimero-node
```
Expected: fmt clean (1.88 rustfmt); all green. The `calimero-context` (`auto_follow`, `self_purge`) + `calimero-node` (e2e) consumers must still pass — they tolerate the timing change (idempotent + lossy-tolerant).

- [ ] **Scope self-check:** only `calimero-governance-store` files changed; `GroupKeyDelivered` (`governance.rs:1026-1032`) untouched (still a direct `notify_op_event`); no `calimero-server-primitives`/`calimero-tee-attestation` change. Add a PR/changelog note: "OpEvents now emit after the op-log append; replays no longer re-emit (idempotent subscribers unaffected)."

- [ ] **Grep for stragglers:** `grep -rn "op_events::notify\|notify_op_event" crates/governance-store/src/ops crates/governance-store/src/namespace/membership.rs` — confirm the only remaining direct calls are `GroupKeyDelivered` (intentional) and none in the converted handlers.
