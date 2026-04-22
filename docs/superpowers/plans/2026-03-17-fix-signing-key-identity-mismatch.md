# Fix: Node Signing Key Overwrites Requester's Stored Key

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Guard the `store_group_signing_key` auto-store calls in `create_group_invitation` and `upgrade_group` so the node's private key is only cached when the requester IS the node's own group identity — preventing a cryptographically invalid `(requester_pubkey → node_privkey)` mapping.

**Architecture:** Both handlers call `node_group_identity()` returning `Option<(node_pk, node_sk)>`, then resolve `requester`. The auto-store currently uses `requester` as the storage key regardless of whether `requester == node_pk`. Adding a single equality guard before each `store_group_signing_key` call is the complete fix — no refactoring needed.

**Tech Stack:** Rust, actix actors, `calimero_store`, `calimero_primitives::identity::PublicKey` (`Copy + PartialEq`)

---

## The Bug

**Location 1:** `crates/context/src/handlers/create_group_invitation.rs:42-50`

```rust
// BUG: stores node_sk under requester regardless of whether requester == node_pk
if let Some((_, node_sk)) = node_identity {
    let _ = group_store::store_group_signing_key(
        &self.datastore,
        &group_id,
        &requester,   // ← could be a different user
        &node_sk,     // ← but this is ALWAYS the node's private key
    );
}
```

**Location 2:** `crates/context/src/handlers/upgrade_group.rs:78-82`

```rust
// BUG: same mismatch — signing_key is always node_sk
if let Some(ref sk) = signing_key {
    let _ =
        group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
}
```

**Impact:** When an authenticated user (whose public key differs from the node's) calls either handler, the store ends up with `requester_pubkey → node_privkey`. In `create_group_invitation`, `inviter_identity` is set to `requester_pk` but the invitation is signed with `node_sk` — the joiner's signature verification (`verify(node_sk_signature, requester_pk)`) will fail. Additionally, any legitimately registered signing key for `requester` is silently overwritten.

**Why the normal path still works:** When `requester` is `None`, it falls back to `node_pk` (L30-40 in create, L33-43 in upgrade), so `requester == node_pk` is always true for that case. The guard does not change behavior for the common single-node scenario.

---

## File Map

| File | Change |
|---|---|
| `crates/context/src/handlers/create_group_invitation.rs` | Add `requester == node_pk` guard around auto-store (L42-50) |
| `crates/context/src/handlers/upgrade_group.rs` | Add `requester == node_pk` guard around auto-store (L78-82) |

---

### Task 1: Fix `create_group_invitation.rs`

**Files:**
- Modify: `crates/context/src/handlers/create_group_invitation.rs:42-50`

- [ ] **Step 1: Apply the guard**

  Current code (L42-50):
  ```rust
  // Auto-store node signing key so it's available for signing the invitation
  if let Some((_, node_sk)) = node_identity {
      let _ = group_store::store_group_signing_key(
          &self.datastore,
          &group_id,
          &requester,
          &node_sk,
      );
  }
  ```

  Replace with:
  ```rust
  // Auto-store node signing key ONLY when the requester IS the node's own identity
  if let Some((node_pk, node_sk)) = node_identity {
      if requester == node_pk {
          let _ = group_store::store_group_signing_key(
              &self.datastore,
              &group_id,
              &requester,
              &node_sk,
          );
      }
  }
  ```

  Key: `node_pk` is now bound instead of discarded with `_`, enabling the equality check. `PublicKey` is `Copy + PartialEq` so `requester == node_pk` compiles without any borrow issues.

- [ ] **Step 2: Verify compilation and style**

  ```bash
  cargo check -p calimero-context
  cargo fmt --check -p calimero-context
  cargo clippy -p calimero-context -- -A warnings
  ```
  Expected: all pass with no output.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/context/src/handlers/create_group_invitation.rs
  git commit -m "fix(context): guard auto-store in create_group_invitation to node identity only"
  ```

---

### Task 2: Fix `upgrade_group.rs`

**Files:**
- Modify: `crates/context/src/handlers/upgrade_group.rs:78-82`

- [ ] **Step 1: Understand the current shape**

  `node_identity` is `Option<(PublicKey, [u8; 32])>`. Both field types are `Copy`, so `node_identity` itself is `Copy` — it can be pattern-matched multiple times without a borrow conflict.

  At L46-47, `signing_key = node_identity.map(|(_, sk)| sk)`. So `signing_key` is `Option<[u8; 32]>` — always the node's SK.

  Current auto-store (L78-82):
  ```rust
  // Auto-store signing key for future use
  if let Some(ref sk) = signing_key {
      let _ =
          group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
  }
  ```

- [ ] **Step 2: Apply the guard**

  Replace with:
  ```rust
  // Auto-store signing key ONLY when the requester IS the node's own identity
  if let (Some(sk), Some((node_pk, _))) = (signing_key, node_identity) {
      if requester == node_pk {
          let _ =
              group_store::store_group_signing_key(&self.datastore, &group_id, &requester, &sk);
      }
  }
  ```

  Note: `signing_key` and `node_identity` are both `Copy` types so moving them into the tuple pattern is fine. `sk` is `[u8; 32]` (copied out), so `&sk` is the correct `&[u8; 32]` argument.

- [ ] **Step 3: Verify compilation and style**

  ```bash
  cargo check -p calimero-context
  cargo fmt --check -p calimero-context
  cargo clippy -p calimero-context -- -A warnings
  ```
  Expected: all pass with no output.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/context/src/handlers/upgrade_group.rs
  git commit -m "fix(context): guard auto-store in upgrade_group to node identity only"
  ```

---

### Task 3: Resolve GitHub review thread

The thread node ID for comment database ID `2947763537` is `PRRT_kwDOLIG5Is506P4T`.

- [ ] **Step 1: Resolve the thread**

  ```bash
  gh api graphql -f query='mutation {
    resolveReviewThread(input: {threadId: "PRRT_kwDOLIG5Is506P4T"}) {
      thread { isResolved }
    }
  }'
  ```
  Expected output: `{"data":{"resolveReviewThread":{"thread":{"isResolved":true}}}}`
