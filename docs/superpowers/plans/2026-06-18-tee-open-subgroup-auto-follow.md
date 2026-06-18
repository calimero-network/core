# TEE Open-Subgroup Auto-Follow (Fix B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** A root-admitted `ReadOnlyTee` (and any inherited Open-subgroup member) actually *replicates* the contexts of Open subgroups it inherits — not just is authorized for them.

**Architecture:** The membership/authorization model is already correct (`check_path` authorizes inherited Open-subgroup membership; `join_context` already resolves + authorizes the inherited path). The only gap is the auto-follow *trigger*: `should_auto_follow_contexts` reads the **direct** `GroupMember` row only, so an inherited-only member (no direct row in the Open subgroup) resolves to `NotAutoFollowing` and `join_context`/subscribe/sync never fire. Fix = make that single gate inheritance-aware: on no direct row, resolve the inheritance anchor via `check_path` and honor the **anchor row's** `auto_follow.contexts` flag.

**Tech Stack:** Rust 1.88.0 (fmt via `rustup run 1.88.0 cargo fmt`). No published-surface change. No mero-tee/mdma contract change.

**Scope:** Fix B only. Fix A (attestation-admitted TEE leave / self_purge-vs-graceful-leave race) and the underlying context split-brain are explicitly OUT — they are a separate design item in the forward-secrecy/reconcile area.

**Verified facts (authoritative — use these):**
- Gate to change: `crates/context/src/auto_follow.rs:477-494` `should_auto_follow_contexts`. Today: `member_value(group_id, member)` → `Some(v) => v.auto_follow.contexts`, `None => false`. The `None` arm is the bug for inherited members.
- Decision path that calls it: `decide_on_context_registered` (`auto_follow.rs:274-290`), invoked from `handle_context_registered` (`:292`). No change needed there.
- Inheritance API: `MembershipRepository::check_path(group_id, identity) -> MembershipPath` (`crates/governance-store/src/membership/core.rs:149`). Returns `MembershipPath::Direct`, `MembershipPath::Inherited { anchor, via_admin }`, or `MembershipPath::None`. `member_value` is at `membership/core.rs:133`.
- The trigger reaches the TEE: `OpEvent::ContextRegistered { group_id=<subgroup>, context_id }` is emitted in the apply path (`crates/governance-store/src/ops/group/context_registered.rs:30`) on every node that applies the op; the TEE applies namespace governance locally, so the event fires.
- `join_context` already authorizes `MembershipPath::Inherited` and resolves the joiner from the namespace-root identity (`crates/context/src/handlers/join_context.rs:144-173`) — so no membership row is written and "Open is free" is preserved.
- E2E truth this builds on: `crates/node/src/local_governance_node_e2e.rs:798` `root_admitted_tee_is_member_of_open_subgroup` proves the TEE `is_member` of the Open subgroup with NO direct row.

**Design decision (locked):** The inheritance fall-through is NOT role-gated to `ReadOnlyTee`. It applies to any inherited Open-subgroup member whose anchor row has `auto_follow.contexts = true`. Rationale: `join_context` already authorizes those same inherited members, so auto-following their contexts is consistent with the Open-inheritance model; a role special-case here would be surprising. Behavior change is gated strictly to the `MembershipPath::Inherited` case — direct members are completely unaffected.

---

### Task 1: Make `should_auto_follow_contexts` inheritance-aware

**Files:**
- Modify: `crates/context/src/auto_follow.rs:477-494`
- Test: `crates/context/src/auto_follow.rs` (`#[cfg(test)] mod tests`, starts `:518`)

- [ ] **Step 1: Write the failing test**

In `mod tests`, add a unit test that seeds: a namespace root group with a direct member row for `self_pk` carrying `CAN_JOIN_OPEN_SUBGROUPS` + `auto_follow.contexts = true`; an Open subgroup child of that root; and **no** direct member row for `self_pk` in the subgroup. Assert `decide_on_context_registered(store, <subgroup_id>, &ctx_id) == ContextRegisteredDecision::Join`.

Reuse the existing seeding helpers in the test module (`seed_self_member` at ~`:640` and whatever seeds groups/subgroups + visibility). The seed must:
- store the root identity so `self_pk_for_group`/`resolve_identity` resolves `self_pk` (mirror existing test setup),
- create the Open subgroup with `VisibilityMode::Open` and parent = root (so `check_path` returns `Inherited { anchor: root, .. }`),
- set the **root** member row's `auto_follow.contexts = true` and `CAN_JOIN_OPEN_SUBGROUPS` capability,
- NOT create a subgroup direct row for `self_pk`.

Also add a negative test: same setup but root row `auto_follow.contexts = false` ⇒ `ContextRegisteredDecision::NotAutoFollowing`.

- [ ] **Step 2: Run the tests, verify they FAIL**

Run: `cd core && cargo test -p calimero-context auto_follow 2>&1 | tail -30`
Expected: the new inherited-Join test FAILS (currently returns `NotAutoFollowing` because `member_value(subgroup, self_pk)` is `None`). The negative test should pass already.

- [ ] **Step 3: Implement the inheritance fall-through**

Rewrite `should_auto_follow_contexts` (`auto_follow.rs:477-494`):

```rust
/// Check if `member` should auto-follow `group_id`'s contexts.
///
/// A direct member uses its own row's `auto_follow.contexts` flag. A member
/// with NO direct row may still be an *inherited* member of an Open subgroup
/// (membership flows down from an anchor ancestor that granted
/// `CAN_JOIN_OPEN_SUBGROUPS`); in that case we honor the **anchor row's**
/// `auto_follow.contexts` flag. This is what lets a root-admitted ReadOnlyTee
/// (which holds no direct row in Open subgroups) replicate their contexts.
fn should_auto_follow_contexts(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> bool {
    let repo = MembershipRepository::new(store);
    match repo.member_value(group_id, member) {
        Ok(Some(v)) => return v.auto_follow.contexts,
        Ok(None) => {} // fall through to inheritance check below
        Err(err) => {
            warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                ?err,
                "auto-follow: failed to read member value"
            );
            return false;
        }
    }

    // No direct row — resolve the inheritance anchor (Open-subgroup chain).
    match repo.check_path(group_id, member) {
        Ok(MembershipPath::Inherited { anchor, .. }) => {
            match repo.member_value(&anchor, member) {
                Ok(Some(v)) => v.auto_follow.contexts,
                Ok(None) => false,
                Err(err) => {
                    warn!(
                        group_id = %hex::encode(group_id.to_bytes()),
                        anchor = %hex::encode(anchor.to_bytes()),
                        ?err,
                        "auto-follow: failed to read anchor member value"
                    );
                    false
                }
            }
        }
        Ok(_) => false,
        Err(err) => {
            warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                ?err,
                "auto-follow: failed to resolve inheritance path"
            );
            false
        }
    }
}
```

Add the `MembershipPath` import to the `use` block at the top of `auto_follow.rs` (it lives in `calimero_governance_store::membership` — match the path already used for `MembershipRepository`).

- [ ] **Step 4: Run the tests, verify they PASS**

Run: `cd core && cargo test -p calimero-context auto_follow 2>&1 | tail -30`
Expected: PASS (both new tests + all existing auto_follow tests).

- [ ] **Step 5: fmt + clippy + commit**

Run: `cd core && rustup run 1.88.0 cargo fmt && cargo clippy -p calimero-context 2>&1 | tail -20`
Then:
```bash
git add crates/context/src/auto_follow.rs docs/superpowers/plans/2026-06-18-tee-open-subgroup-auto-follow.md
git commit -m "fix(auto-follow): replicate Open-subgroup contexts for inherited members

should_auto_follow_contexts only checked the direct GroupMember row, so a
root-admitted ReadOnlyTee (and any inherited Open-subgroup member) never
started replicating Open-subgroup contexts despite being an authorized
inherited member. Resolve the inheritance anchor via check_path and honor
the anchor row's auto_follow.contexts flag. Scoped to MembershipPath::
Inherited; direct members are unaffected."
```

---

### Task 2: E2E regression — TEE auto-joins an Open-subgroup context

**Files:**
- Test: `crates/node/src/local_governance_node_e2e.rs` (sibling to `root_admitted_tee_is_member_of_open_subgroup` at `:798`)

- [ ] **Step 1: Write the test**

Add `root_admitted_tee_auto_follows_open_subgroup_context` modeled on `root_admitted_tee_is_member_of_open_subgroup`. After the TEE is root-admitted and the Open subgroup exists (reuse that test's setup), register a context in the Open subgroup and drive the auto-follow handler (mirror how other e2e tests in this file pump `ContextRegistered`/op-events). Assert the TEE node ends up subscribed to / replicating the context (assert via the same observable those tests use — e.g. context membership/subscription on the TEE node), where before this fix it would not.

- [ ] **Step 2: Run it, verify it PASS (and would have failed pre-fix)**

Run: `cd core && cargo test -p calimero-node root_admitted_tee_auto_follows_open_subgroup_context 2>&1 | tail -30`
Expected: PASS. (Sanity: temporarily stub the gate back to direct-only locally to confirm it fails without the fix, then restore — do NOT commit the stub.)

- [ ] **Step 3: fmt + commit**

Run: `cd core && rustup run 1.88.0 cargo fmt`
```bash
git add crates/node/src/local_governance_node_e2e.rs
git commit -m "test(e2e): TEE auto-follows Open-subgroup contexts after root admission"
```

---

### Final verification

- [ ] `cd core && cargo test -p calimero-context -p calimero-node -p calimero-governance-store 2>&1 | tail -30` — all green.
- [ ] `cd core && cargo check --workspace 2>&1 | tail -5` — clean.
- [ ] `cd core && rustup run 1.88.0 cargo fmt --check` — clean (CI fmt uses 1.88).
