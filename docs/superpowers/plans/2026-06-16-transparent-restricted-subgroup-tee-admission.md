# Transparent Restricted-Subgroup TEE Admission — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a namespace-level TEE HA replica automatically gain a `ReadOnlyTee` membership row **and** the per-subgroup key for every **Restricted** subgroup of the namespace, so it can decrypt and serve those subgroups' reads — without manual per-subgroup configuration.

**Architecture:** A new event subscriber `tee_subgroup_admit` in `calimero-context` (mirrors the existing `self_purge` subscriber) is spawned from `ContextManager::started`. It listens on the process-wide `op_events` broadcast for two events and reacts **only on a node that already holds the subgroup's key** (the subgroup's creator/member): on `SubgroupCreated` (a new Restricted subgroup → admit the namespace's existing root-level `ReadOnlyTee` members into it) and on `TeeMemberAdmitted` at the namespace root (a new TEE member joined → admit it into the Restricted subgroups this node holds keys for). Both reuse the member's already-verified verdict, read back from the namespace-root op log by a new `tee_admission_record` helper, and call the **existing** `admit_tee_node`, which writes the `ReadOnlyTee` row and delivers the per-subgroup key (delivery succeeds because the acting node holds the key). **Open** subgroups are skipped — a root-admitted TEE node already reads them via inherited membership + the namespace key. The existing joiner-side recovery pull remains the fallback for the edge where no key-holder is online when the event fires.

**Tech Stack:** Rust (toolchain 1.88.0, run `rustup run 1.88.0 cargo fmt`), Cargo workspace; `tokio::sync::broadcast` op-events; `actix` `ContextManager`; `cargo test` (unit + in-crate e2e harness); `borsh`.

**Scope guardrails (do NOT violate):**
- No change to `read_tee_admission_policy` (subgroup admission reuses the namespace-root policy).
- No subgroup id bound into the attestation quote → no change to the published `calimero-tee-attestation` API → **no mero-tee rev bump**.
- No mdma change: this is entirely verifier/key-holder-side inside core's DAG; `should_join`/`confirm` stay namespace-keyed.
- `deliver_group_key_to_member` and `admit_tee_node` are **reused unchanged**.
- Out of scope (Phase 2): node-resident periodic liveness heartbeat, verdict cache, richer self-heal, metrics, `trusted_anchors` sync-preference for subgroups.

**Source of truth for the design:** `proposal.md` §12d (workspace root) / mdma#163.

---

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `core/crates/governance-store/src/tee.rs` | Modify | Add `TeeAdmissionRecord` + `tee_admission_record(...)` — read back a member's stored verified verdict from a group's op log. |
| `core/crates/governance-store/src/lib.rs` | Modify (only if needed) | Re-export `TeeAdmissionRecord`/`tee_admission_record` if `tee` items aren't already public there. |
| `core/crates/context/src/tee_subgroup_admit.rs` | Create | The new subscriber: pure `admit_trigger`, the spawn/run/shutdown loop, and the two store-touching handlers. |
| `core/crates/context/src/lib.rs` | Modify | `mod tee_subgroup_admit;` + `tee_subgroup_admit::spawn(...)` in `started()`. |
| `core/crates/governance-store/src/namespace/tests.rs` | Modify | Integration test: recovery pull serves a `ReadOnlyTee` member (fallback proof). |
| `core/crates/node/src/local_governance_node_e2e.rs` | Modify | Three e2e tests: admit-on-subgroup-create, admit-on-member-admit, and the `CAN_JOIN_OPEN_SUBGROUPS` precondition check. |

---

## Task 1: `tee_admission_record` verdict read-back helper

Reconstructs the data needed to re-issue `admit_tee_node` for a subgroup, from the stored `MemberJoinedViaTeeAttestation` op. `is_tee_admitted_identity` already does the scan but returns only `bool`; this returns the fields.

**Files:**
- Modify: `core/crates/governance-store/src/tee.rs`
- Test: same file, `#[cfg(test)]` module (add if absent).

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `tee.rs` (create the module if it doesn't exist; use the crate's existing `test_fixtures::test_store` + `MetaRepository`/`MembershipRepository`/op-append helpers already used by sibling tests in this crate):

```rust
#[cfg(test)]
mod admission_record_tests {
    use super::*;
    use crate::test_fixtures::test_store;
    use crate::{MetaRepository, NamespaceRepository};
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    #[test]
    fn record_returns_stored_verdict_for_admitted_member() {
        let mut rng = OsRng;
        let store = test_store();

        let ns_id = [0x21u8; 32];
        let ns_gid = ContextGroupId::from(ns_id);

        let admin_sk = PrivateKey::random(&mut rng);
        let admin_pk = admin_sk.public_key();
        let tee_pk = PrivateKey::random(&mut rng).public_key();

        // Seed the namespace identity + admin so the op-log append is well-formed.
        NamespaceRepository::new(&store)
            .store_identity(&ns_gid, &admin_pk, &admin_sk, &[0u8; 32])
            .unwrap();

        // Append a real MemberJoinedViaTeeAttestation op to the namespace-root log.
        let op = SignedGroupOp::sign(
            &admin_sk,
            ns_id,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberJoinedViaTeeAttestation {
                member: tee_pk,
                quote_hash: [0xABu8; 32],
                mrtd: "mrtd-x".to_owned(),
                rtmr0: "r0".to_owned(),
                rtmr1: "r1".to_owned(),
                rtmr2: "r2".to_owned(),
                rtmr3: "r3".to_owned(),
                tcb_status: "UpToDate".to_owned(),
                role: GroupMemberRole::ReadOnlyTee,
            },
        )
        .unwrap();
        crate::append_signed_group_op_for_test(&store, &op).unwrap(); // use the crate's existing op-append test helper

        let rec = tee_admission_record(&store, &ns_gid, &tee_pk)
            .unwrap()
            .expect("record present");
        assert_eq!(rec.quote_hash, [0xABu8; 32]);
        assert_eq!(rec.mrtd, "mrtd-x");
        assert_eq!(rec.tcb_status, "UpToDate");
        assert_eq!(rec.role, GroupMemberRole::ReadOnlyTee);

        // Unknown member → None.
        let other = PrivateKey::random(&mut rng).public_key();
        assert!(tee_admission_record(&store, &ns_gid, &other).unwrap().is_none());
    }
}
```

> NOTE: `append_signed_group_op_for_test` is a stand-in for whatever the crate already uses to write an op to a group's log in tests (the same mechanism `is_tee_admitted_identity`'s neighbors use — grep `tee.rs`/`tests.rs` for how existing tests seed `read_op_log_after` entries; reuse that exact helper name). If none exists, append via the same path `apply_local_signed_group_op` uses.

- [ ] **Step 2: Run the test, verify it fails**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store record_returns_stored_verdict -- --nocapture`
Expected: FAIL — `cannot find function tee_admission_record` / `cannot find type TeeAdmissionRecord`.

- [ ] **Step 3: Implement the helper**

Add to `tee.rs` (mirror `is_tee_admitted_identity` at `tee.rs:96-114`, capturing fields instead of `..`):

```rust
/// The verified verdict stored when a TEE node was admitted, read back from a
/// group's op log. Used to re-issue admission into a subgroup reusing the
/// already-verified measurements (no fresh quote needed) — the group-invariant
/// measurement verdict is reusable; liveness was proven at the original admission.
#[derive(Clone, Debug)]
pub struct TeeAdmissionRecord {
    pub quote_hash: [u8; 32],
    pub mrtd: String,
    pub rtmr0: String,
    pub rtmr1: String,
    pub rtmr2: String,
    pub rtmr3: String,
    pub tcb_status: String,
    pub role: GroupMemberRole,
}

/// Returns the stored TEE-admission verdict for `identity` in `group_id`, or
/// `None` if it was never admitted there. Scans `group_id` directly (does NOT
/// resolve to the namespace root), so pass the group whose log holds the
/// admission — for root admissions that is the namespace id.
pub fn tee_admission_record(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<TeeAdmissionRecord>> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;
    for (_seq, bytes) in &entries {
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) {
            if let GroupOp::MemberJoinedViaTeeAttestation {
                member,
                quote_hash,
                mrtd,
                rtmr0,
                rtmr1,
                rtmr2,
                rtmr3,
                tcb_status,
                role,
            } = op.op
            {
                if member == *identity {
                    return Ok(Some(TeeAdmissionRecord {
                        quote_hash,
                        mrtd,
                        rtmr0,
                        rtmr1,
                        rtmr2,
                        rtmr3,
                        tcb_status,
                        role,
                    }));
                }
            }
        }
    }
    Ok(None)
}
```

Add `use calimero_primitives::context::GroupMemberRole;` to `tee.rs` imports if not already present.

- [ ] **Step 4: Run the test, verify it passes**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store record_returns_stored_verdict`
Expected: PASS.

- [ ] **Step 5: Confirm public visibility, then commit**

Confirm `tee_admission_record` + `TeeAdmissionRecord` are reachable from `calimero-context` (check how `is_tee_admitted_identity` is re-exported — likely `calimero_governance_store::tee::...` or a top-level re-export in `governance-store/src/lib.rs`; add the same re-export for the new items).

```bash
git add core/crates/governance-store/src/tee.rs core/crates/governance-store/src/lib.rs
git commit -m "feat(governance-store): add tee_admission_record verdict read-back"
```

---

## Task 2: Subscriber module skeleton + pure `admit_trigger`

Create the module mirroring `self_purge.rs` (the leaner subscriber). This task lands only the spawn/run/shutdown plumbing and the **pure** event→intent function with its unit test. Handlers are stubs that do nothing yet.

**Files:**
- Create: `core/crates/context/src/tee_subgroup_admit.rs`
- Modify: `core/crates/context/src/lib.rs` (add `mod tee_subgroup_admit;` near the other `mod self_purge;` / `mod auto_follow;` lines — do **not** wire `spawn` yet).
- Test: `tee_subgroup_admit.rs` `#[cfg(test)]`.

- [ ] **Step 1: Write the failing test**

Create `core/crates/context/src/tee_subgroup_admit.rs` containing only the imports, the `AdmitTrigger` enum, the pure `admit_trigger`, and this test (no spawn/handlers yet):

```rust
#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use calimero_governance_store::op_events::OpEvent;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    #[test]
    fn maps_only_subgroup_created_and_tee_admitted() {
        let mut rng = OsRng;
        let member = PrivateKey::random(&mut rng).public_key();

        assert_eq!(
            admit_trigger(&OpEvent::SubgroupCreated {
                namespace_id: [1u8; 32],
                parent_group_id: [2u8; 32],
                child_group_id: [3u8; 32],
            }),
            Some(AdmitTrigger::NewSubgroup {
                namespace_id: [1u8; 32],
                child_group_id: [3u8; 32],
            })
        );

        assert_eq!(
            admit_trigger(&OpEvent::TeeMemberAdmitted {
                group_id: [4u8; 32],
                member,
            }),
            Some(AdmitTrigger::NewTeeMember {
                group_id: [4u8; 32],
                member,
            })
        );

        // Unrelated events are ignored.
        assert_eq!(
            admit_trigger(&OpEvent::MemberAdded {
                group_id: [5u8; 32],
                member,
                role: GroupMemberRole::Member,
            }),
            None
        );
        assert_eq!(
            admit_trigger(&OpEvent::TeeMemberRemoved {
                group_id: [6u8; 32],
                member,
            }),
            None
        );
    }
}
```

- [ ] **Step 2: Run the test, verify it fails**

Run: `rustup run 1.88.0 cargo test -p calimero-context maps_only_subgroup_created -- --nocapture`
Expected: FAIL — module/type/function not found.

- [ ] **Step 3: Implement the skeleton + pure function**

Top of `core/crates/context/src/tee_subgroup_admit.rs` (mirror `self_purge.rs:67-121`):

```rust
//! Transparent per-subgroup TEE admission (Phase 1, Restricted subgroups).
//!
//! Runs on a node that holds a subgroup's group key (its creator/member).
//! Reacts to two governance events and admits the namespace's entitled TEE
//! member(s) into Restricted subgroups, reusing the verified verdict from the
//! namespace-root op log. Open subgroups are skipped (a root-admitted TEE node
//! reads them via inherited membership + the namespace key). See proposal.md §12d.

use std::sync::Mutex;

use calimero_context_client::client::ContextClient;
use calimero_context_primitives::group::AdmitTeeNodeRequest;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::tee::tee_admission_record;
use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, NamespaceRepository,
};
use calimero_primitives::context::{ContextGroupId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tokio::task::AbortHandle;
use tracing::{debug, error, info, warn};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AdmitTrigger {
    /// A new subgroup was created; admit the namespace's existing root TEE members.
    NewSubgroup {
        namespace_id: [u8; 32],
        child_group_id: [u8; 32],
    },
    /// A TEE member was admitted somewhere; if it was the namespace root, fan it
    /// out into the Restricted subgroups this node holds keys for.
    NewTeeMember {
        group_id: [u8; 32],
        member: PublicKey,
    },
}

/// Pure event→intent mapping. The store-touching filtering (Restricted-ness,
/// key ownership, root-vs-subgroup, idempotency) happens in the handlers.
pub(crate) fn admit_trigger(event: &OpEvent) -> Option<AdmitTrigger> {
    match event {
        OpEvent::SubgroupCreated {
            namespace_id,
            child_group_id,
            ..
        } => Some(AdmitTrigger::NewSubgroup {
            namespace_id: *namespace_id,
            child_group_id: *child_group_id,
        }),
        OpEvent::TeeMemberAdmitted { group_id, member } => Some(AdmitTrigger::NewTeeMember {
            group_id: *group_id,
            member: *member,
        }),
        _ => None,
    }
}

struct HandleState {
    abort: AbortHandle,
}

static HANDLE: Mutex<Option<HandleState>> = Mutex::new(None);

pub fn spawn(store: Store, context_client: ContextClient) {
    let mut slot = HANDLE.lock().expect("tee-subgroup-admit HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("tee-subgroup-admit handler already running; skipping re-spawn");
        return;
    }
    let abort = tokio::spawn(async move {
        run(store, context_client).await;
    })
    .abort_handle();
    *slot = Some(HandleState { abort });
}

pub fn shutdown() {
    if let Some(state) = HANDLE.lock().expect("tee-subgroup-admit HANDLE poisoned").take() {
        state.abort.abort();
    }
}

async fn run(store: Store, context_client: ContextClient) {
    let mut rx = op_events::subscribe();
    info!("tee-subgroup-admit handler started");
    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "tee-subgroup-admit subscriber lagged; dropped events are recovered \
                     by the next SubgroupCreated/TeeMemberAdmitted or the joiner-side key pull"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                info!("tee-subgroup-admit op-event channel closed; handler exiting");
                break;
            }
        };

        match admit_trigger(&event) {
            Some(AdmitTrigger::NewSubgroup {
                namespace_id,
                child_group_id,
            }) => handle_new_subgroup(&store, &context_client, namespace_id, child_group_id).await,
            Some(AdmitTrigger::NewTeeMember { group_id, member }) => {
                handle_new_tee_member(&store, &context_client, group_id, member).await
            }
            None => {}
        }
    }
}

// Stubs — implemented in Tasks 3 and 4.
async fn handle_new_subgroup(
    _store: &Store,
    _context_client: &ContextClient,
    _namespace_id: [u8; 32],
    _child_group_id: [u8; 32],
) {
}

async fn handle_new_tee_member(
    _store: &Store,
    _context_client: &ContextClient,
    _group_id: [u8; 32],
    _member: PublicKey,
) {
}
```

Add `mod tee_subgroup_admit;` to `core/crates/context/src/lib.rs` next to the existing `mod self_purge;`.

> NOTE: confirm the exact import paths against the crate (the verbatim recon shows `self_purge.rs` imports `calimero_governance_store::op_events::{self, OpEvent}`, `calimero_store::Store`, `calimero_primitives::identity::PublicKey`). `CapabilitiesRepository`/`GroupKeyring` may be re-exported at `calimero_governance_store::` top level or under submodules — match how `admit_tee_node.rs` imports them. `ContextGroupId` is `calimero_context_config::types::ContextGroupId` in some crates and re-exported elsewhere — match `self_purge.rs`'s path.

- [ ] **Step 4: Run the test, verify it passes**

Run: `rustup run 1.88.0 cargo test -p calimero-context maps_only_subgroup_created`
Expected: PASS. (Dead-code warnings for the stub handlers are fine; they're filled next.)

- [ ] **Step 5: Commit**

```bash
git add core/crates/context/src/tee_subgroup_admit.rs core/crates/context/src/lib.rs
git commit -m "feat(context): tee-subgroup-admit subscriber skeleton + pure admit_trigger"
```

---

## Task 3: `handle_new_subgroup` (create-time path) + spawn wiring

When a Restricted subgroup is created **on this node** (so this node holds its freshly-minted key), admit every existing namespace-root `ReadOnlyTee` member into it.

**Files:**
- Modify: `core/crates/context/src/tee_subgroup_admit.rs` (replace the `handle_new_subgroup` stub; add a shared `admit_member_into_subgroup` helper).
- Modify: `core/crates/context/src/lib.rs` (add `tee_subgroup_admit::spawn(...)` in `started()`).

- [ ] **Step 1: Write the failing e2e test** (full runtime; lives with the other e2e in the node crate, added in Task 5 — here we only implement code). For this task, implement against the existing unit harness by asserting the handler is wired; the behavioral proof is Task 5's e2e. Write a minimal smoke unit test that `spawn` is idempotent:

```rust
#[cfg(test)]
mod spawn_tests {
    use super::*;

    #[tokio::test]
    async fn spawn_is_idempotent() {
        // Two spawns must not start two loops. We can't easily observe the task,
        // but a second spawn must early-return without panicking.
        // (Construct ContextClient/Store via the crate's existing test fixture if
        // available; otherwise this is covered by the e2e in Task 5 and this test
        // may be omitted — do NOT fake a Store.)
    }
}
```

> If the `calimero-context` crate has no lightweight `ContextClient` fixture, **omit this unit test** — `spawn` idempotency is structurally identical to `self_purge::spawn` (already proven) and the behavior is covered by Task 5's e2e. Do not stand up a fake actor just to test the guard.

- [ ] **Step 2: Implement `handle_new_subgroup` + the shared admit helper**

Replace the `handle_new_subgroup` stub and add the shared helper:

```rust
/// Admit `member` (an entitled TEE identity) into `subgroup`, reusing the
/// verdict recorded at its namespace-root admission. Idempotent and best-effort:
/// logs and continues on any error. Delivery of the per-subgroup key happens
/// inside `admit_tee_node` and succeeds because the caller holds the key.
async fn admit_member_into_subgroup(
    store: &Store,
    context_client: &ContextClient,
    namespace_gid: &ContextGroupId,
    subgroup_gid: &ContextGroupId,
    member: &PublicKey,
) {
    // Idempotency: skip if already a direct member of the subgroup.
    match MembershipRepository::new(store).has_direct_member(subgroup_gid, member) {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            error!(?e, "tee-subgroup-admit: has_direct_member check failed");
            return;
        }
    }

    let record = match tee_admission_record(store, namespace_gid, member) {
        Ok(Some(r)) => r,
        Ok(None) => {
            // Member is not a root TEE member (or its admission op isn't local
            // yet). Nothing to reuse; the key pull / a later event will recover.
            return;
        }
        Err(e) => {
            error!(?e, "tee-subgroup-admit: reading admission record failed");
            return;
        }
    };

    if record.role != GroupMemberRole::ReadOnlyTee {
        return;
    }

    let req = AdmitTeeNodeRequest {
        group_id: *subgroup_gid,
        member: *member,
        quote_hash: record.quote_hash,
        mrtd: record.mrtd,
        rtmr0: record.rtmr0,
        rtmr1: record.rtmr1,
        rtmr2: record.rtmr2,
        rtmr3: record.rtmr3,
        tcb_status: record.tcb_status,
        // Production TEE admissions are real; the op-log record carries no
        // is_mock flag. Mock-quote test paths admit via the allowlisted mock
        // MRTD regardless (accept_mock + allowed_mrtd), so false is correct here.
        is_mock: false,
    };

    if let Err(e) = context_client.admit_tee_node(req).await {
        error!(?e, "tee-subgroup-admit: admit_tee_node into subgroup failed (key pull is the fallback)");
    }
}

async fn handle_new_subgroup(
    store: &Store,
    context_client: &ContextClient,
    namespace_id: [u8; 32],
    child_group_id: [u8; 32],
) {
    let namespace_gid = ContextGroupId::from(namespace_id);
    let child_gid = ContextGroupId::from(child_group_id);

    // Only act for Restricted subgroups — Open subgroups are already readable
    // by a root-admitted TEE node (inherited membership + namespace key).
    match CapabilitiesRepository::new(store).is_open_chain_to_namespace(&child_gid, &namespace_gid) {
        Ok(true) => return,   // Open → skip
        Ok(false) => {}       // Restricted → proceed
        Err(e) => {
            error!(?e, "tee-subgroup-admit: open-chain check failed");
            return;
        }
    }

    // Only the key-holder (the creator) can deliver the per-subgroup key.
    match GroupKeyring::new(store, child_gid).load_current_key() {
        Ok(Some(_)) => {}     // we hold the key → we can admit + deliver
        Ok(None) => return,   // not the key-holder → leave it to the creator / pull
        Err(e) => {
            error!(?e, "tee-subgroup-admit: load_current_key failed");
            return;
        }
    }

    // Admit every existing root-level ReadOnlyTee member into the new subgroup.
    let members = match MembershipRepository::new(store).list(&namespace_gid, 0, usize::MAX) {
        Ok(m) => m,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: listing root members failed");
            return;
        }
    };
    for (member, role) in members {
        if role == GroupMemberRole::ReadOnlyTee {
            admit_member_into_subgroup(store, context_client, &namespace_gid, &child_gid, &member)
                .await;
        }
    }
}
```

- [ ] **Step 3: Wire `spawn` into `started()`**

In `core/crates/context/src/lib.rs`, immediately after the `self_purge::spawn(...)` call (recon shows it at `lib.rs:757`), add:

```rust
        // Transparent per-subgroup TEE admission (proposal.md §12d, Phase 1).
        // Reacts to SubgroupCreated / TeeMemberAdmitted to admit entitled TEE
        // members into Restricted subgroups this node holds keys for. Open
        // subgroups need no admission. Mirrors the self_purge listener pattern.
        tee_subgroup_admit::spawn(self.datastore.clone(), self.context_client.clone());
```

- [ ] **Step 4: Build + run existing tests**

Run: `rustup run 1.88.0 cargo test -p calimero-context`
Expected: PASS (compiles; no regressions). Behavioral proof comes in Task 5.

- [ ] **Step 5: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-context
git add core/crates/context/src/tee_subgroup_admit.rs core/crates/context/src/lib.rs
git commit -m "feat(context): admit root TEE members into new Restricted subgroups"
```

---

## Task 4: `handle_new_tee_member` (join-into-existing path)

When a TEE node is admitted at the namespace **root**, fan it out into the Restricted subgroups this node holds keys for. Guards against the self-trigger loop (subgroup admissions also fire `TeeMemberAdmitted`).

**Files:**
- Modify: `core/crates/context/src/tee_subgroup_admit.rs` (replace the `handle_new_tee_member` stub).

- [ ] **Step 1: Implement the handler**

```rust
async fn handle_new_tee_member(
    store: &Store,
    context_client: &ContextClient,
    group_id: [u8; 32],
    member: PublicKey,
) {
    let group_gid = ContextGroupId::from(group_id);

    // Resolve the namespace root. React ONLY to root admissions — a subgroup
    // admission (which this very handler causes) also fires TeeMemberAdmitted;
    // ignoring non-root admissions prevents an infinite fan-out loop.
    let namespace_gid = match NamespaceRepository::new(store).resolve(&group_gid) {
        Ok(ns) => ns,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: namespace resolve failed");
            return;
        }
    };
    if namespace_gid != group_gid {
        return; // subgroup admission echo → ignore
    }

    // Enumerate Restricted subgroups of the namespace that this node holds keys
    // for, and admit the new member into each.
    let descendants = match NamespaceRepository::new(store).collect_descendants(&namespace_gid) {
        Ok(d) => d,
        Err(e) => {
            error!(?e, "tee-subgroup-admit: collect_descendants failed");
            return;
        }
    };

    let caps = CapabilitiesRepository::new(store);
    for sub in descendants {
        // Restricted only.
        match caps.is_open_chain_to_namespace(&sub, &namespace_gid) {
            Ok(true) => continue,  // Open → skip
            Ok(false) => {}
            Err(e) => {
                error!(?e, "tee-subgroup-admit: open-chain check failed (descendant)");
                continue;
            }
        }
        // Only if we hold this subgroup's key.
        match GroupKeyring::new(store, sub).load_current_key() {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(e) => {
                error!(?e, "tee-subgroup-admit: load_current_key failed (descendant)");
                continue;
            }
        }
        admit_member_into_subgroup(store, context_client, &namespace_gid, &sub, &member).await;
    }
}
```

> NOTE: confirm `NamespaceRepository::resolve(&gid) -> EyreResult<ContextGroupId>` returns the namespace **root** id (recon: `admit_tee_node` uses exactly this to resolve the namespace), and `collect_descendants(&gid) -> EyreResult<Vec<ContextGroupId>>` returns all nested subgroups excluding the root (recon: `namespace/core.rs:207`). If `collect_descendants` is private, use the public `list_children` recursively or expose `collect_descendants` (it's already used internally).

- [ ] **Step 2: Build**

Run: `rustup run 1.88.0 cargo build -p calimero-context`
Expected: compiles clean.

- [ ] **Step 3: Run crate tests**

Run: `rustup run 1.88.0 cargo test -p calimero-context`
Expected: PASS.

- [ ] **Step 4: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-context
git add core/crates/context/src/tee_subgroup_admit.rs
git commit -m "feat(context): fan new root TEE member into existing Restricted subgroups"
```

---

## Task 5: e2e — admit-on-subgroup-create

Proves: with a TEE node already admitted at the namespace root, creating a **Restricted** subgroup on the same node gives the TEE node a direct `ReadOnlyTee` row in that subgroup.

**Files:**
- Modify: `core/crates/node/src/local_governance_node_e2e.rs`

- [ ] **Step 1: Write the failing test**

Add (mirrors `ns_announce_admits_announcer_as_read_only_tee_member` at `:583` and reuses `boot_test_node`, `provision_tee_owner`, `mock_quote_bytes`, `announce_network_event`, `wait_until`):

```rust
#[tokio::test]
async fn restricted_subgroup_created_admits_existing_tee_member() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x93u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // 1) Admit a TEE node at the namespace root via the announce path.
    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Bu8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");

    let admitted_root = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&ns_gid, &tee_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;
    assert!(admitted_root, "TEE node must be admitted at the namespace root first");

    // 2) Create a RESTRICTED subgroup on this node (this node mints + holds its key).
    let sub_gid = create_restricted_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;

    // 3) The subscriber must admit the root TEE member into the new subgroup.
    let admitted_sub = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&sub_gid, &tee_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;
    assert!(
        admitted_sub,
        "TEE node must gain a ReadOnlyTee row in the Restricted subgroup after creation"
    );

    // 4) And it must hold the subgroup key (delivered by the creator).
    assert!(
        calimero_context::group_store::GroupKeyring::new(&node.store, sub_gid)
            .load_current_key()
            .expect("load key")
            .is_some(),
        "subgroup must have a current key on this (creator) node"
    );
}
```

> `create_restricted_subgroup` is a new test helper — implement it next to `provision_tee_owner` using the production `ContextClient` create-group path (so the key is minted and `SubgroupCreated` fires). Inspect `core/crates/context/src/handlers/create_group.rs` for the exact `ContextClient` method and request type; the subgroup must be **Restricted** (visibility != Open) and nested under `ns_gid`. Return its `ContextGroupId`.

- [ ] **Step 2: Run, verify it fails**

Run: `rustup run 1.88.0 cargo test -p calimero-node restricted_subgroup_created_admits -- --nocapture`
Expected: FAIL — `create_restricted_subgroup` undefined (then, once defined, FAIL on the subgroup-admit assertion until Tasks 3 is active — which it is, so it should pass; if it fails on key delivery, see Task 3 NOTE on `is_mock`).

- [ ] **Step 3: Implement `create_restricted_subgroup` helper**

Write the helper using the real create-group entrypoint. Pseudostructure (fill from `create_group.rs`):

```rust
async fn create_restricted_subgroup(
    node: &TestNode,
    parent_ns: &ContextGroupId,
    admin_pk: &PublicKey,
    rng: &mut OsRng,
) -> ContextGroupId {
    // Use node.context_client to create a RESTRICTED subgroup under parent_ns,
    // signed by admin_pk's namespace identity. Return the new ContextGroupId.
    // The create handler mints the subgroup GroupKeyring key (create_group.rs:180-181)
    // and applies RootOp::GroupCreated, which fires OpEvent::SubgroupCreated.
    todo!("call the real ContextClient create-group method with Restricted visibility")
}
```

Replace the `todo!` with the actual call once the signature is confirmed. Do **not** hand-roll the op — go through `context_client` so the key is minted and the event fires.

- [ ] **Step 4: Run, verify it passes**

Run: `rustup run 1.88.0 cargo test -p calimero-node restricted_subgroup_created_admits`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add core/crates/node/src/local_governance_node_e2e.rs
git commit -m "test(node): e2e Restricted subgroup creation admits existing TEE member"
```

---

## Task 6: e2e — admit-on-member-admit (join-into-existing)

Proves: when a Restricted subgroup already exists (this node holds its key) and a TEE node is then admitted at the root, the subscriber fans it into the existing subgroup.

**Files:**
- Modify: `core/crates/node/src/local_governance_node_e2e.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn tee_admitted_after_restricted_subgroup_exists_is_fanned_in() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x94u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // 1) Restricted subgroup exists FIRST (no TEE member yet).
    let sub_gid = create_restricted_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;

    // 2) Now admit a TEE node at the root.
    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Cu8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");

    // 3) The TEE node must end up admitted into the pre-existing subgroup.
    let fanned_in = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&sub_gid, &tee_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;
    assert!(
        fanned_in,
        "root TEE admission must fan into the pre-existing Restricted subgroup"
    );
}
```

- [ ] **Step 2: Run, verify it fails** (then passes once Task 4 is in)

Run: `rustup run 1.88.0 cargo test -p calimero-node tee_admitted_after_restricted_subgroup_exists -- --nocapture`
Expected: PASS (Task 4 implements the fan-in). If it FAILS, debug the root-resolution guard (`namespace_gid != group_gid`) and `collect_descendants` scope.

- [ ] **Step 3: Commit**

```bash
git add core/crates/node/src/local_governance_node_e2e.rs
git commit -m "test(node): e2e root TEE admission fans into existing Restricted subgroup"
```

---

## Task 7: Integration test — recovery pull serves a `ReadOnlyTee` member (fallback proof)

The fan-in relies on a key-holder being online; the documented fallback is the joiner-side pull. The existing roundtrip test (`tests.rs:3444`) uses role `Member`. This task proves the responder authz also serves a `ReadOnlyTee` member (closing the "runtime-unverified for ReadOnlyTee" gap at the unit/crypto layer).

**Files:**
- Modify: `core/crates/governance-store/src/namespace/tests.rs`

- [ ] **Step 1: Write the failing test** (clone `responder_delivery_round_trips_key_to_joiner_cross_store` verbatim, changing only the member role and the test name):

```rust
#[test]
fn responder_delivery_round_trips_key_to_read_only_tee_joiner() {
    use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedNamespaceOp};
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let namespace_id = [0xF2u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let subgroup_id = [0xF3u8; 32];
    let subgroup_gid = ContextGroupId::from(subgroup_id);
    let group_key = [0x6Du8; 32];

    let joiner_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let joiner_sk = PrivateKey::from(joiner_sk_bytes);
    let joiner_pk = joiner_sk.public_key();
    let responder_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let responder_sk = PrivateKey::from(responder_sk_bytes);
    let responder_pk = responder_sk.public_key();

    let responder_store = test_store();
    NamespaceRepository::new(&responder_store)
        .store_identity(&ns_gid, &responder_pk, &responder_sk_bytes, &[0u8; 32])
        .unwrap();
    MetaRepository::new(&responder_store)
        .save(&ns_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    MetaRepository::new(&responder_store)
        .save(&subgroup_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    NamespaceRepository::new(&responder_store)
        .nest(&ns_gid, &subgroup_gid)
        .unwrap();
    // The ONLY change vs the Member test: the joiner holds ReadOnlyTee.
    MembershipRepository::new(&responder_store)
        .add_member(&subgroup_gid, &joiner_pk, GroupMemberRole::ReadOnlyTee)
        .unwrap();
    GroupKeyring::new(&responder_store, subgroup_gid)
        .store_key(&group_key)
        .unwrap();

    let (envelope_bytes, responder_identity) =
        build_group_key_delivery(&responder_store, namespace_id, subgroup_id, joiner_pk).unwrap();
    assert!(
        !envelope_bytes.is_empty(),
        "responder must deliver the subgroup key to a ReadOnlyTee member"
    );
    assert_eq!(responder_identity, responder_pk);

    let joiner_store = test_store();
    NamespaceRepository::new(&joiner_store)
        .store_identity(&ns_gid, &joiner_pk, &joiner_sk_bytes, &[0u8; 32])
        .unwrap();
    let buffered = SignedNamespaceOp::sign(
        &responder_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: subgroup_id,
            key_id: GroupKeyring::key_id_for(&group_key),
            encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    NamespaceOpLogService::new(&joiner_store, namespace_id)
        .store_signed_operation(&buffered)
        .unwrap();

    apply_received_group_key(
        &joiner_store,
        namespace_id,
        subgroup_id,
        &envelope_bytes,
        responder_identity,
    )
    .unwrap();

    assert_eq!(
        GroupKeyring::new(&joiner_store, subgroup_gid)
            .load_current_key()
            .unwrap()
            .map(|(_, k)| k),
        Some(group_key)
    );
    assert!(namespace_groups_awaiting_key(&joiner_store, namespace_id)
        .unwrap()
        .is_empty());
}
```

- [ ] **Step 2: Run, verify it passes immediately** (it tests existing, unchanged code — the point is to lock in `ReadOnlyTee` coverage)

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store responder_delivery_round_trips_key_to_read_only_tee_joiner`
Expected: PASS. **If it FAILS**, the recovery responder gates on role somewhere — STOP and report; that would mean the fallback is unsound for `ReadOnlyTee` and Phase 1 needs a key-holder-push-only design.

- [ ] **Step 3: Commit**

```bash
git add core/crates/governance-store/src/namespace/tests.rs
git commit -m "test(governance-store): key-recovery pull serves a ReadOnlyTee member"
```

---

## Task 8: Verify the `CAN_JOIN_OPEN_SUBGROUPS` precondition (Open-is-free)

The "Open subgroups need no admission" property holds only if the root `ReadOnlyTee` row carries `CAN_JOIN_OPEN_SUBGROUPS`. Prove it; if absent, fix at root admission.

**Files:**
- Test: `core/crates/node/src/local_governance_node_e2e.rs`
- Possible fix: `core/crates/governance-store/src/membership/policy.rs` (default caps) or `core/crates/context/src/handlers/admit_tee_node.rs`.

- [ ] **Step 1: Write the test**

```rust
#[tokio::test]
async fn root_admitted_tee_is_member_of_open_subgroup() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x95u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // Admit a TEE node at root.
    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Du8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(libp2p::PeerId::random(), &topic, quote_bytes, tee_pk, nonce))
        .await
        .expect("deliver announce");
    assert!(
        wait_until(|| calimero_context::group_store::MembershipRepository::new(&node.store)
            .is_member(&ns_gid, &tee_pk).unwrap_or(false)).await,
        "TEE node admitted at root"
    );

    // Create an OPEN subgroup.
    let open_sub = create_open_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;

    // The root TEE node must be a member-by-inheritance of the Open subgroup,
    // WITHOUT any direct row in it.
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&node.store)
            .has_direct_member(&open_sub, &tee_pk)
            .unwrap(),
        "no direct row expected in the Open subgroup"
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .is_member(&open_sub, &tee_pk)
            .unwrap(),
        "root TEE node must be an inherited member of the Open subgroup \
         (requires CAN_JOIN_OPEN_SUBGROUPS on the root row)"
    );
}
```

> `create_open_subgroup` mirrors `create_restricted_subgroup` but with **Open** visibility.

- [ ] **Step 2: Run**

Run: `rustup run 1.88.0 cargo test -p calimero-node root_admitted_tee_is_member_of_open_subgroup -- --nocapture`
Expected: PASS. **If the `is_member` assertion FAILS**, the root `ReadOnlyTee` admission does not grant `CAN_JOIN_OPEN_SUBGROUPS` → proceed to Step 3.

- [ ] **Step 3 (only if Step 2 failed): Grant the capability at root TEE admission**

Inspect where `add_member`/`add_member_with_keys` assigns default capabilities (`membership/policy.rs:130-143`) and confirm the namespace `default_capabilities` include `CAN_JOIN_OPEN_SUBGROUPS` for the `ReadOnlyTee` role. If not, add it specifically for the row written by `MemberJoinedViaTeeAttestation` apply (find the apply arm that materializes the `ReadOnlyTee` row and ensure its caps include `CAN_JOIN_OPEN_SUBGROUPS`). Re-run Step 2 to green. Keep the change minimal and TEE-scoped — do not broaden caps for other roles.

- [ ] **Step 4: Commit**

```bash
git add core/crates/node/src/local_governance_node_e2e.rs core/crates/governance-store/src/membership/policy.rs
git commit -m "test(node): root TEE admission grants Open-subgroup read access (CAN_JOIN_OPEN_SUBGROUPS)"
```

---

## Final verification

- [ ] **Workspace check + targeted tests**

```bash
rustup run 1.88.0 cargo fmt --all -- --check
rustup run 1.88.0 cargo check --workspace
rustup run 1.88.0 cargo test -p calimero-governance-store -p calimero-context -p calimero-node
```
Expected: fmt clean (CI fmt uses the 1.88 rustfmt), check clean, all new tests green.

- [ ] **Self-check against scope guardrails:** confirm no edits touched `read_tee_admission_policy`, the `calimero-tee-attestation` crate, `deliver_group_key_to_member`, `admit_tee_node`'s signature, or any mdma/mero-tee file.

---

## Known residuals (documented, not Phase-1 bugs)

- **Offline key-holder at event time:** if no node holding a Restricted subgroup's key is online/subscribed when `SubgroupCreated`/`TeeMemberAdmitted` fires, the TEE member's row may be written without immediate key delivery; the joiner-side recovery pull (Task 7-proven authz) self-heals once a key-holder is meshed. `op_events` is a live broadcast (not replayed on sync), so a key-holder that was offline at fire time will not re-trigger — the pull is the only recovery for that case.
- **`is_mock` not in the op-log record:** reconstruction uses `is_mock: false` (Task 3). If a future real-vs-mock distinction must survive into subgroup admission, add an `is_mock` field to `MemberJoinedViaTeeAttestation` (`governance-types/src/lib.rs:253`) and store it at `admit_tee_node.rs:202`.
- **Deferred to Phase 2:** node-resident periodic liveness heartbeat, verdict cache, `trusted_anchors` sync-source preference for subgroups, metrics/observability.
