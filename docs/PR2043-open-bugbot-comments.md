# PR #2043 — Open Bot Review Comments
> **PR**: feat(context): introduce GroupRequest and ContextGroupId types for group management  
> **Source**: https://github.com/calimero-network/core/pull/2043  
> **Scraped**: 2026-03-09 | **Total open threads**: 131 (resolved/outdated excluded)  
> **Bots**: meroreviewer (101 comments), cursor (30 comments)

---

## Summary

| Severity | Count | Description |
|----------|-------|-------------|
| 🔴 HIGH | 8 | Bugs and security issues requiring immediate fix |
| 🟡 MEDIUM | 56 | Code quality, correctness, and design concerns |
| 💡 LOW | 61 | Minor improvements, style, and optimization hints |
| 📝 NITPICK | 6 | Micro-style and cosmetic suggestions |
| **TOTAL** | **131** | |

### Top Files with Open Comments

| File | Open Comments |
|------|--------------|
| `crates/context/config/src/client/env/config/mutate.rs` | 22 |
| `crates/context/config/src/client/env/config/query/near.rs` | 19 |
| `crates/context/src/group_store.rs` | 17 |
| `crates/context/primitives/src/client/external/group.rs` | 11 |
| `crates/client/src/client.rs` | 8 |
| `crates/context/primitives/src/group.rs` | 6 |
| `crates/context/config/src/types.rs` | 6 |
| `crates/context/config/src/client/env/config/requests.rs` | 6 |
| `crates/context/config/src/lib.rs` | 6 |
| `crates/context/src/handlers/execute.rs` | 5 |

---

## 🔁 Repeated Issues (Same Bug, Multiple Locations)

> These issues appear in multiple places in the codebase and should be fixed systematically.

### 🟡 DRY violation: duplicated unsafe slice cast pattern — ×2 occurrences
**Severity:** MEDIUM | **Bot:** meroreviewer

**Locations:**
- [`crates/context/config/src/client/env/config/mutate.rs`:218](https://github.com/calimero-network/core/pull/2043#discussion_r2897080429)
- [`crates/context/config/src/client/env/config/mutate.rs`:226](https://github.com/calimero-network/core/pull/2043#discussion_r2888797947)

**Description:** The identical unsafe pointer cast `unsafe { &*(ptr::from_ref::<[SignerId]>(members) as *const [Repr<SignerId>]) }` appears in both `add_group_members` and `remove_group_members`; extract a helper function.

**Fix:** Create a helper like `fn as_repr_slice<T>(slice: &[T]) -> &[Repr<T>]` with the safety documentation in one place.

### 💡 Nonce not incremented after successful contract call — ×2 occurrences
**Severity:** LOW | **Bot:** cursor

**Locations:**
- [`crates/context/primitives/src/client/external/group.rs`:252](https://github.com/calimero-network/core/pull/2043#discussion_r2883555436)
- [`crates/context/primitives/src/client/external/group.rs`:252](https://github.com/calimero-network/core/pull/2043#discussion_r2904716424)

**Description:** In `with_nonce`, when `f(n).await` succeeds (line 106), the function returns immediately without incrementing `*nonce`. Subsequent operations reuse the stale nonce, which the contract rejects, forcing an extra round-trip to `fetch_nonce` plus a retry on every call after the first. For sequences of group operations (e.g., create → add members → register context), this doubles the number of network 

### 💡 Unsafe pointer cast for slice transmutation — ×2 occurrences
**Severity:** LOW | **Bot:** meroreviewer

**Locations:**
- [`crates/context/config/src/client/env/config/mutate.rs`:223](https://github.com/calimero-network/core/pull/2043#discussion_r2888567567)
- [`crates/context/config/src/client/env/config/mutate.rs`:221](https://github.com/calimero-network/core/pull/2043#discussion_r2888815772)

**Description:** Raw pointer casting from `[SignerId]` to `[Repr<SignerId>]` relies on unverified `#[repr(transparent)]` assumption; a breaking change to `Repr` could cause memory corruption.

**Fix:** Add a compile-time static assertion that `Repr<SignerId>` has the same size and alignment as `SignerId`, or use a safe iterator-based conversion.

---

## 🔴 HIGH Priority (8)

### 1. Group upgrade fails when admin lacks context membership
**File:** [`handlers/upgrade_group.rs`:584](https://github.com/calimero-network/core/pull/2043#discussion_r2857199224) | **Bot:** cursor

Both `propagate_upgrade` and `maybe_lazy_upgrade` pass the group admin's `PublicKey` as the `identity` argument to `ContextClient::update_application`. Internally, `ExternalConfig::update_application` calls `get_identity(&context_id, &public_key)` to retrieve the admin's private key from the context's identity store. If the group admin is not a member of the specific context being upgraded, this l

### 2. LazyOnAccess upgrade status permanently blocks future upgrades
**File:** [`handlers/upgrade_group.rs`:194](https://github.com/calimero-network/core/pull/2043#discussion_r2859752164) | **Bot:** cursor

When `UpgradePolicy::LazyOnAccess` is used, the handler persists `GroupUpgradeStatus::InProgress` (with `completed: 0, failed: 0`) and returns immediately without any mechanism to ever transition this status to `Completed`. Individual lazy upgrades in `maybe_lazy_upgrade` don't update the stored `GroupUpgradeStatus`. This permanently blocks subsequent upgrade requests because `validate_upgrade` re

### 3. Missing prefix guard in `enumerate_all_groups` causes unbounded iteration
**File:** [`src/group_store.rs`:90](https://github.com/calimero-network/core/pull/2043#discussion_r2883579247) | **Bot:** cursor

`enumerate_all_groups` is the only iterator function in `group_store.rs` that does not check the key prefix byte before processing entries. Every other iterator function (e.g., `count_group_admins`, `list_group_members`, `enumerate_group_contexts`, `enumerate_in_progress_upgrades`, `delete_all_group_signing_keys`) explicitly checks `key.as_key().as_bytes()[0] != PREFIX` and breaks when the prefix 

### 4. Lazy upgrade sends actor message to self, causing deadlock
**File:** [`handlers/execute.rs`:241](https://github.com/calimero-network/core/pull/2043#discussion_r2888803134) | **Bot:** cursor

The lazy upgrade path calls `ctx_client.update_application()` from within an actor future of the `ContextManager` actor. `update_application` sends a message back to the same `ContextManager` actor's mailbox. Since Actix actors process messages sequentially, this message won't be processed until the current execute handler completes — but the execute handler is `await`ing the update. This creates 

### 5. Join group bypasses admin check via forced local insertion
**File:** [`handlers/join_group.rs`:116](https://github.com/calimero-network/core/pull/2043#discussion_r2888803159) | **Bot:** cursor

When `needs_chain_sync` is true, the handler forcibly inserts the inviter as a local admin before checking `is_group_admin`. This makes the admin check self-fulfilling — it always passes for the inviter, even if they were demoted on-chain after creating the invitation. When no `signing_key` is provided (skipping contract commit/reveal), a join can succeed with a stale invitation from a revoked adm

### 6. Hardcoded expiration block height ignores actual expiration parameter
**File:** [`handlers/create_group_invitation.rs`:92](https://github.com/calimero-network/core/pull/2043#discussion_r2891308761) | **Bot:** cursor

The `expiration_block_height` is hardcoded to `999_999_999` instead of being derived from the `expiration` parameter. This means the on-chain commitment will accept reveals far beyond the intended expiration window, effectively making all group invitations valid nearly forever on-chain regardless of the `expiration` the admin requested. The `expiration` parameter is only validated client-side in t

### 7. Client-controlled requester parameter enables authorization bypass
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2894949889) | **Bot:** meroreviewer

The `requester` parameter is client-supplied and sent in the request body; if the server trusts this value for authorization decisions instead of deriving identity from authenticated session tokens, attackers can impersonate any user to delete contexts.

> **Fix:** [see code in PR]

### 8. LazyOnAccess migration triggers repeatedly on every execution
**File:** [`handlers/execute.rs`:1239](https://github.com/calimero-network/core/pull/2043#discussion_r2904716415) | **Bot:** cursor

After a `LazyOnAccess` upgrade with a migration method, `meta.migration` in the group metadata is never cleared. The `maybe_lazy_upgrade` condition `*current_application_id == meta.target_application_id && meta.migration.is_none()` only short-circuits when migration is `None`. Since migration is persisted in `save_group_meta` during upgrade but never removed after a successful lazy upgrade, every 

---

## 🟡 MEDIUM Priority (56)

### 1. Primitives crate depends on store crate via From impls
**File:** [`src/group.rs`:339](https://github.com/calimero-network/core/pull/2043#discussion_r2857198655) | **Bot:** meroreviewer

The From<calimero_store::key::*> implementations create a dependency from calimero-context-primitives to calimero-store; primitives crates are typically lower-level and should not depend on storage layers.

> **Fix:** [see code in PR]

### 2. N+1 query pattern in count_group_admins
**File:** [`src/group_store.rs`:165](https://github.com/calimero-network/core/pull/2043#discussion_r2857198783) | **Bot:** meroreviewer

Each member requires a separate `handle.get(&key)` call to read the role, resulting in O(n) database reads for a group with n members; this can be slow for large groups.

> **Fix:** [see code in PR]

### 3. Context re-registration silently removes from old group without authorization
**File:** [`src/group_store.rs`:235](https://github.com/calimero-network/core/pull/2043#discussion_r2857198853) | **Bot:** meroreviewer

When registering a context to a new group, the function automatically deletes the context's index entry from the old group without verifying authorization on that old group. An attacker with admin rights on group B but not group A could move a context from A to B, effectively removing it from A.

> **Fix:** [see code in PR]

### 4. Authorization check missing for cross-group context migration
**File:** [`src/group_store.rs`:266](https://github.com/calimero-network/core/pull/2043#discussion_r2859756028) | **Bot:** meroreviewer

The `register_context_in_group` function automatically removes a context from its previous group when re-registering to a new group, but contains no authorization check; callers must ensure they have permission to unregister from the old group, not just register to the new one.

> **Fix:** [see code in PR]

### 5. Race condition causes spurious errors in count_group_admins
**File:** [`src/group_store.rs`:183](https://github.com/calimero-network/core/pull/2043#discussion_r2859756202) | **Bot:** meroreviewer

Between iterating keys and calling `handle.get(&key)`, a concurrent delete can cause this to return an error instead of gracefully skipping; `enumerate_in_progress_upgrades` (line 414) correctly uses `if let Some()` pattern.

> **Fix:** [see code in PR]

### 6. Race condition causes spurious errors in list_group_members
**File:** [`src/group_store.rs`:232](https://github.com/calimero-network/core/pull/2043#discussion_r2859756400) | **Bot:** meroreviewer

Same iterate-then-get race as count_group_admins; a concurrent delete between key iteration and value fetch will propagate an error instead of skipping the deleted entry.

> **Fix:** [see code in PR]

### 7. Cross-group authorization bypass in context re-registration
**File:** [`src/group_store.rs`:286](https://github.com/calimero-network/core/pull/2043#discussion_r2859882223) | **Bot:** meroreviewer

When re-registering a context to a new group, the function removes it from the old group without authorization check on the old group; caller must ensure they have permission on both groups, not just the target.

> **Fix:** [see code in PR]

### 8. Race condition in register_context_in_group can cause orphaned index entries
**File:** [`src/group_store.rs`:265](https://github.com/calimero-network/core/pull/2043#discussion_r2859882459) | **Bot:** meroreviewer

The read-modify-write sequence (get existing group, delete old index, put new index) is not atomic; concurrent calls for the same context_id with different group_ids can leave orphaned GroupContextIndex entries, causing incorrect count_group_contexts results.

> **Fix:** [see code in PR]

### 9. Missing prefix boundary check in `enumerate_all_groups` iterator
**File:** [`src/group_store.rs`:90](https://github.com/calimero-network/core/pull/2043#discussion_r2883555429) | **Bot:** cursor

`enumerate_all_groups` iterates from a `GroupMeta` seek key but never checks the prefix byte to stop at the key-type boundary. Every other scan function in this file (`count_group_admins`, `list_group_members`, `count_group_members`, `enumerate_group_contexts`, `count_group_contexts`, `enumerate_in_progress_upgrades`, `delete_all_group_signing_keys`) explicitly checks `key.as_key().as_bytes()[0] !

### 10. Unsafe pointer cast relies on unverified Repr<T> transparency invariant
**File:** [`config/mutate.rs`:230](https://github.com/calimero-network/core/pull/2043#discussion_r2883581819) | **Bot:** meroreviewer

The unsafe cast from `[SignerId]` to `[Repr<SignerId>]` assumes `#[repr(transparent)]` on `Repr<T>`, but this invariant isn't enforced at the call site; if `Repr` layout changes, this becomes undefined behavior.

> **Fix:** [see code in PR]

### 11. Unsafe transmute on deserialized network data
**File:** [`query/near.rs`:202](https://github.com/calimero-network/core/pull/2043#discussion_r2883581934) | **Bot:** meroreviewer

Using `mem::transmute` on data received from network (RPC response) relies on the assumption that `Repr<ContextGroupId>` is layout-compatible with `ContextGroupId`; malformed responses could trigger UB if this invariant is violated.

> **Fix:** [see code in PR]

### 12. Unsafe transmute on Vec with undefined repr
**File:** [`query/near.rs`:191](https://github.com/calimero-network/core/pull/2043#discussion_r2888566827) | **Bot:** meroreviewer

Using `mem::transmute` on `Vec<Repr<ContextId>>` to `Vec<ContextId>` is technically undefined behavior because `Vec` has no guaranteed memory layout, even if inner types are layout-compatible.

> **Fix:** [see code in PR]

### 13. Unsafe transmute on Option with undefined repr
**File:** [`query/near.rs`:212](https://github.com/calimero-network/core/pull/2043#discussion_r2888566960) | **Bot:** meroreviewer

Transmuting `Option<Repr<ContextGroupId>>` to `Option<ContextGroupId>` is unsafe; `Option` layout is not guaranteed for non-null-optimized types.

> **Fix:** [see code in PR]

### 14. Vec transmute has undefined behavior
**File:** [`query/near.rs`:198](https://github.com/calimero-network/core/pull/2043#discussion_r2888567126) | **Bot:** meroreviewer

Transmuting `Vec<Repr<T>>` to `Vec<T>` is undefined behavior even with `#[repr(transparent)]` inner type because Vec's layout is not guaranteed; use `into_iter().map(Repr::into_inner).collect()` instead.

> **Fix:** [see code in PR]

### 15. Transmute of Vec with undefined repr risks UB
**File:** [`query/near.rs`:197](https://github.com/calimero-network/core/pull/2043#discussion_r2888567266) | **Bot:** meroreviewer

The transmute between `Vec<Repr<ContextId>>` and `Vec<ContextId>` suppresses `clippy::transmute_undefined_repr`—this is only safe if `Repr<T>` is guaranteed `#[repr(transparent)]` and Vec's internal pointers remain valid, which is fragile to refactoring.

> **Fix:** [see code in PR]

### 16. Option transmute may break niche optimization assumptions
**File:** [`query/near.rs`:235](https://github.com/calimero-network/core/pull/2043#discussion_r2888567404) | **Bot:** meroreviewer

Transmuting `Option<Repr<ContextGroupId>>` to `Option<ContextGroupId>` is risky because Option uses niche optimization; if Repr ever changes its niche properties, this becomes UB.

> **Fix:** [see code in PR]

### 17. Option transmute has undefined behavior
**File:** [`query/near.rs`:235](https://github.com/calimero-network/core/pull/2043#discussion_r2888567719) | **Bot:** meroreviewer

Transmuting `Option<Repr<ContextGroupId>>` to `Option<ContextGroupId>` is undefined behavior; Option's niche optimization may differ between types.

> **Fix:** [see code in PR]

### 18. Repeated .expect() calls violate error handling guidelines
**File:** [`src/client.rs`:1005](https://github.com/calimero-network/core/pull/2043#discussion_r2888576955) | **Bot:** meroreviewer

All 20+ group client methods use `.expect("Mailbox not to be dropped")` which violates the AGENTS.md rule to avoid `.unwrap()`/`.expect()` and use `.map_err()` or `?` instead.

> **Fix:** [see code in PR]

### 19. Unbounded memory allocation with usize::MAX limit
**File:** [`handlers/upgrade_group.rs`:150](https://github.com/calimero-network/core/pull/2043#discussion_r2888577105) | **Bot:** meroreviewer

Multiple calls to `enumerate_group_contexts(..., 0, usize::MAX)` could allocate unbounded memory for groups with many contexts; this pattern appears ~9 times across upgrade/sync handlers.

> **Fix:** [see code in PR]

### 20. Unsafe transmute of Vec with undefined behavior
**File:** [`query/near.rs`:203](https://github.com/calimero-network/core/pull/2043#discussion_r2888797066) | **Bot:** meroreviewer

Using `mem::transmute` on `Vec<Repr<T>>` to `Vec<T>` is undefined behavior per Rust documentation even when inner type is `#[repr(transparent)]`, as Vec's layout is not guaranteed; could cause memory corruption.

> **Fix:** [see code in PR]

### 21. Unsafe transmute of Option with undefined behavior
**File:** [`query/near.rs`:240](https://github.com/calimero-network/core/pull/2043#discussion_r2888797324) | **Bot:** meroreviewer

Same issue as Vec transmute - transmuting `Option<Repr<T>>` to `Option<T>` has no guaranteed layout compatibility.

> **Fix:** [see code in PR]

### 22. Lazy upgrade skips re-fetching module for updated application
**File:** [`handlers/execute.rs`:217](https://github.com/calimero-network/core/pull/2043#discussion_r2888803126) | **Bot:** cursor

The lazy upgrade path calls `ctx_client.update_application()` inside the `lazy_upgrade_task`, then re-fetches the context metadata and loads the module via `get_module(context.application_id)`. However, the `update_application` handler inside the actor invalidates the application cache only after the future completes. Since `lazy_upgrade_task` runs as an actor future and the subsequent `module_tas

### 23. Remove-members admin count check uses wrong comparison operator
**File:** [`handlers/remove_group_members.rs`:64](https://github.com/calimero-network/core/pull/2043#discussion_r2888803149) | **Bot:** cursor

The check `admin_count <= unique_admins_being_removed.len()` prevents removing all admins but also prevents removing admins when exactly one would remain. For example, if there are 2 admins and the request removes 1, the condition `2 <= 1` is false, which is correct. But if there are 2 admins and both are being removed, `2 <= 2` is true, correctly blocking. However, if the requester (who must be a

### 24. Unsafe transmute of Vec with undefined representation
**File:** [`query/near.rs`:198](https://github.com/calimero-network/core/pull/2043#discussion_r2888815251) | **Bot:** meroreviewer

Using `mem::transmute` on `Vec<Repr<ContextId>>` to `Vec<ContextId>` is technically undefined behavior since Vec's layout is not guaranteed; while Repr<T> is #[repr(transparent)], the Vec wrapper is not.

> **Fix:** [see code in PR]

### 25. Unsafe transmute of Option with undefined representation
**File:** [`query/near.rs`:233](https://github.com/calimero-network/core/pull/2043#discussion_r2888815426) | **Bot:** meroreviewer

Transmuting `Option<Repr<ContextGroupId>>` to `Option<ContextGroupId>` relies on Option's internal representation which is not guaranteed; this can cause memory safety issues.

> **Fix:** [see code in PR]

### 26. Transmute of Vec has undefined behavior
**File:** [`query/near.rs`:202](https://github.com/calimero-network/core/pull/2043#discussion_r2889775254) | **Bot:** meroreviewer

Using `mem::transmute` between `Vec<Repr<T>>` and `Vec<T>` is technically undefined behavior even if `Repr<T>` is `#[repr(transparent)]`, since Vec's internal layout is not guaranteed stable.

> **Fix:** [see code in PR]

### 27. No-op role update broadcasts spurious mutation notification
**File:** [`handlers/update_member_role.rs`:69](https://github.com/calimero-network/core/pull/2043#discussion_r2889783485) | **Bot:** cursor

When `current_role == new_role`, the sync closure returns `Ok(())` early without modifying any state. However, execution then falls through to the async block which unconditionally broadcasts a `MemberRoleUpdated` group mutation to all contexts. This sends a spurious notification to peers even though nothing changed, potentially triggering unnecessary sync work on other nodes.

### 28. Signing key required for local-only role update
**File:** [`handlers/update_member_role.rs`:59](https://github.com/calimero-network/core/pull/2043#discussion_r2889783492) | **Bot:** cursor

`update_member_role` calls `require_group_signing_key` which mandates a stored signing key, but the handler never makes any on-chain contract call — it only updates local state and broadcasts a gossip message. This makes the endpoint fail for admins who haven't registered a signing key, even though no signing is performed. The same issue applies to `update_group_settings`.

### 29. Delete context silently skips on-chain group unregistration
**File:** [`handlers/delete_context.rs`:155](https://github.com/calimero-network/core/pull/2043#discussion_r2890409496) | **Bot:** cursor

When deleting a group context, if no signing key is found for the requester in the group signing key store, the on-chain `unregister_context_from_group` call is silently skipped, but the local `unregister_context_from_group` still executes. Unlike other group handlers (e.g., `add_group_members`, `delete_group`, `detach_context_from_group`), this handler lacks the `require_group_signing_key` pre-ch

### 30. Transmuting Vec types is undefined behavior even for layout-compatible elements
**File:** [`query/near.rs`:199](https://github.com/calimero-network/core/pull/2043#discussion_r2890425946) | **Bot:** meroreviewer

Using `mem::transmute` on `Vec<Repr<ContextId>>` to `Vec<ContextId>` is UB because Vec's internal representation is not guaranteed to be identical even when element types are layout-compatible.

> **Fix:** [see code in PR]

### 31. Unsafe pointer cast relies on undocumented repr(transparent) invariant
**File:** [`config/mutate.rs`:219](https://github.com/calimero-network/core/pull/2043#discussion_r2890426339) | **Bot:** meroreviewer

The unsafe cast from `[SignerId]` to `[Repr<SignerId>]` assumes `Repr<T>` is `#[repr(transparent)]`; if this invariant changes without updating this code, it could cause memory corruption or type confusion.

> **Fix:** [see code in PR]

### 32. Transmuting Option<Repr<T>> to Option<T> is unsafe
**File:** [`query/near.rs`:232](https://github.com/calimero-network/core/pull/2043#discussion_r2890426701) | **Bot:** meroreviewer

Same transmute pattern used for `Option<Repr<ContextGroupId>>` relies on Option's niche optimization being identical for both types, which is not guaranteed.

> **Fix:** [see code in PR]

### 33. Delete context silently skips on-chain unregistration without signing key
**File:** [`handlers/delete_context.rs`:155](https://github.com/calimero-network/core/pull/2043#discussion_r2891308768) | **Bot:** cursor

When deleting a group context, if the requester has no stored signing key, the on-chain `unregister_context_from_group` call is silently skipped (the `if let Some(sk)` on line 145 just falls through), but the local `unregister_context_from_group` on line 154 still executes. This creates a divergence where the context is removed locally but remains registered in the group on-chain, leaving orphaned

### 34. Node identity re-evaluated after move into closure captures
**File:** [`handlers/add_group_members.rs`:40](https://github.com/calimero-network/core/pull/2043#discussion_r2891308776) | **Bot:** cursor

`node_identity` is resolved once at line 23 via `self.node_group_identity()`. If `requester` is `Some(pk)` (line 27), `node_identity` could be `None`, making `signing_key` at line 40 also `None`. But the code at line 48 only checks `require_group_signing_key` when `signing_key.is_none()`, and then at line 57 only auto-stores when `signing_key.is_some()`. When `requester` is explicitly provided but

### 35. Delete context passes shadowed requester losing validation
**File:** [`handlers/delete_context.rs`:74](https://github.com/calimero-network/core/pull/2043#discussion_r2894923839) | **Bot:** cursor

Inside the `if let Some(group_id)` block, `requester` is re-bound by shadowing (line 61) as a non-optional `PublicKey` for the admin check. But this shadow is scoped to the block. The `requester` passed to the async `delete_context` function (line 88) is still the original `Option<PublicKey>` from the request. Inside the async function, `requester.and_then(...)` is used to look up the signing key.

### 36. Node identity resolved after requester loses secret key access
**File:** [`handlers/add_group_members.rs`:40](https://github.com/calimero-network/core/pull/2043#discussion_r2894923841) | **Bot:** cursor

When `requester` is provided explicitly (not from node identity), `node_identity` may still be `Some`. The code sets `signing_key = node_sk` (from node identity), which means the signing key used for the contract call belongs to the *node's* identity, not the *requester's* identity. If the requester differs from the node's group identity, the contract call will be signed by a different key than th

### 37. Create group invitation uses hardcoded placeholder expiration height
**File:** [`handlers/create_group_invitation.rs`:92](https://github.com/calimero-network/core/pull/2043#discussion_r2894923843) | **Bot:** cursor

The `expiration_block_height` is hardcoded to `999_999_999` regardless of the caller-supplied `expiration` parameter. This means on-chain invitation commitments never truly expire at the intended time, and the `expiration` field from the request only gates the local timestamp check but has no effect on-chain expiration. The placeholder value may already be in the past on some chains or far in the 

### 38. Join group uses immutable client for commit then reveal
**File:** [`handlers/join_group.rs`:145](https://github.com/calimero-network/core/pull/2043#discussion_r2894923847) | **Bot:** cursor

The `group_client` is bound as immutable (`let group_client = client_result?`) but both `commit_group_invitation` and `reveal_group_invitation` are defined on `&self` (immutable). However, the nonce-managed operations like `commit_group_invitation` call `send` with nonce `0` (no nonce tracking). The real issue is that `commit_group_invitation` and `reveal_group_invitation` both hardcode nonce `0` 

### 39. Unsafe pointer cast relies on undocumented repr guarantee
**File:** [`config/mutate.rs`:229](https://github.com/calimero-network/core/pull/2043#discussion_r2894950083) | **Bot:** meroreviewer

The unsafe cast from `[SignerId]` to `[Repr<SignerId>]` assumes `Repr<T>` is `#[repr(transparent)]` but this is not enforced at compile time; if `Repr` layout changes, this becomes undefined behavior leading to memory corruption.

> **Fix:** [see code in PR]

### 40. Vec transmute may cause UB if Repr layout differs
**File:** [`query/near.rs`:200](https://github.com/calimero-network/core/pull/2043#discussion_r2894950225) | **Bot:** meroreviewer

Transmuting `Vec<Repr<ContextId>>` to `Vec<ContextId>` relies on identical memory layouts including allocator metadata; while the `#[expect]` attribute acknowledges this, it's fragile if `Repr` definition changes.

> **Fix:** [see code in PR]

### 41. Group invitation signatures stored as hex strings without validation
**File:** [`src/types.rs`:680](https://github.com/calimero-network/core/pull/2043#discussion_r2894950555) | **Bot:** meroreviewer

The `inviter_signature` and `invitee_signature` fields are plain strings; without length/format validation during deserialization, oversized or malformed signatures could cause issues during cryptographic verification or enable DoS.

> **Fix:** [see code in PR]

### 42. Using serde_json::Value bypasses type validation for target_application
**File:** [`config/requests.rs`:156](https://github.com/calimero-network/core/pull/2043#discussion_r2894950684) | **Bot:** meroreviewer

Storing `target_application` as `serde_json::Value` bypasses compile-time type checking; malformed or malicious JSON structures could propagate through the system if downstream code doesn't strictly validate the schema.

> **Fix:** [see code in PR]

### 43. Repeated transmute pattern should be abstracted
**File:** [`query/near.rs`:205](https://github.com/calimero-network/core/pull/2043#discussion_r2894950814) | **Bot:** meroreviewer

The `mem::transmute` for converting `Vec<Repr<T>>` to `Vec<T>` is repeated in `GroupContextsRequest::decode` and `ContextGroupRequest::decode`; a shared helper would reduce unsafe surface area.

> **Fix:** [see code in PR]

### 44. Repeated unsafe pointer cast pattern violates DRY
**File:** [`config/mutate.rs`:226](https://github.com/calimero-network/core/pull/2043#discussion_r2894950928) | **Bot:** meroreviewer

The unsafe `ptr::from_ref` cast from `&[SignerId]` to `&[Repr<SignerId>]` is duplicated in both `add_group_members` and `remove_group_members`; extract a helper function to centralize this unsafe code.

> **Fix:** [see code in PR]

### 45. Delete context passes unresolved `requester` to async task
**File:** [`handlers/delete_context.rs`:91](https://github.com/calimero-network/core/pull/2043#discussion_r2897061217) | **Bot:** cursor

When deleting a group context, the `requester` variable is shadowed inside the `if let Some(group_id)` block (line 61) but the original `requester: Option<PublicKey>` is what gets moved into the async task (line 88). For non-group contexts the outer `requester` is passed through fine. But for group contexts, the admin check uses the inner `requester` while the async closure captures the outer `req

### 46. Node identity resolved but used after `requester` shadows it
**File:** [`handlers/add_group_members.rs`:40](https://github.com/calimero-network/core/pull/2043#discussion_r2897061222) | **Bot:** cursor

In `add_group_members`, `node_identity` is consumed to resolve `requester` at line 26-36, but then `node_identity.map(|(_, sk)| sk)` is called again at line 38. If `requester` was provided explicitly (not from `node_identity`), `node_identity` might still be `Some`, and the signing key will be extracted from the *node* identity rather than the requester's key. But if the requester is a different i

### 47. Signing key always from node identity, ignoring explicit requester
**File:** [`handlers/add_group_members.rs`:40](https://github.com/calimero-network/core/pull/2043#discussion_r2897061223) | **Bot:** cursor

Across multiple group handlers, `signing_key` is unconditionally derived from `node_identity.map(|(_, sk)| sk)` — the node's own group secret key. When an explicit `requester` is provided (a different identity than the node's), the local admin check passes for that `requester`, but the contract transaction is signed with the *node's* key. If the node identity differs from the provided requester, t

### 48. Lazy upgrade uses `update_application` with potentially stale executor
**File:** [`handlers/execute.rs`:237](https://github.com/calimero-network/core/pull/2043#discussion_r2897061224) | **Bot:** cursor

In the lazy upgrade path, `update_application` is called with `executor` as the signer, but `executor` is the identity executing the current method call — not necessarily an identity that has permission to update the application on the context config contract. The `update_application` method requires the caller to be a context member with appropriate permissions. If the executor lacks update permi

### 49. Lazy upgrade uses executor instead of proper signing identity
**File:** [`handlers/execute.rs`:237](https://github.com/calimero-network/core/pull/2043#discussion_r2897061225) | **Bot:** cursor

In the lazy upgrade path, `update_application` is called with `executor` as the signer identity. The `executor` is whoever is calling the current method — not necessarily an identity with application-update permissions on the context config contract. If `executor` lacks permissions, the lazy upgrade silently fails on every method call (caught as a warning), adding latency and never actually upgrad

### 50. Create group uses identity_secret as group signing key
**File:** [`handlers/create_context.rs`:473](https://github.com/calimero-network/core/pull/2043#discussion_r2897061227) | **Bot:** cursor

When creating a context within a group, `identity_secret` (the context-specific identity key) is used as the signing key for the group contract call to `register_context_in_group`. However, the group contract expects the caller to be a group admin/member identified by their group signer ID. The `identity_secret` is a freshly generated context identity, not the group admin's signing key. This will 

### 51. Unsafe pointer cast relies on undocumented #[repr(transparent)] assumption
**File:** [`config/mutate.rs`:226](https://github.com/calimero-network/core/pull/2043#discussion_r2897080295) | **Bot:** meroreviewer

The unsafe transmute from `[SignerId]` to `[Repr<SignerId>]` depends on `Repr<T>` being `#[repr(transparent)]`, but this is enforced only by comment—if that invariant is violated, undefined behavior results.

> **Fix:** [see code in PR]

### 52. DRY violation: duplicated unsafe slice cast pattern
**File:** [`config/mutate.rs`:218](https://github.com/calimero-network/core/pull/2043#discussion_r2897080429) | **Bot:** meroreviewer

The identical unsafe pointer cast `unsafe { &*(ptr::from_ref::<[SignerId]>(members) as *const [Repr<SignerId>]) }` appears in both `add_group_members` and `remove_group_members`; extract a helper function.

> **Fix:** [see code in PR]

### 53. Silent config parsing failures in node group identity
**File:** [`src/lib.rs`:142](https://github.com/calimero-network/core/pull/2043#discussion_r2904716420) | **Bot:** cursor

`node_group_identity()` silently returns `None` when the configured group identity has malformed keys — wrong prefix, invalid base58, or wrong byte length. Every caller interprets `None` as "no group identity configured" rather than "identity is misconfigured," producing misleading error messages like "node has no configured group identity" when the identity IS configured but malformed. This makes

### 54. Client-supplied requester identity for context deletion
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2904740425) | **Bot:** meroreviewer

The `delete_context` method now accepts an optional `requester` PublicKey from the caller; if the server trusts this value without independent verification, it creates an authorization bypass where any client could claim to be a different identity.

> **Fix:** [see code in PR]

### 55. Unsafe transmute on Vec with layout assumptions
**File:** [`config/mutate.rs`:382](https://github.com/calimero-network/core/pull/2043#discussion_r2904740551) | **Bot:** meroreviewer

Using `mem::transmute` on `Vec<SignerId>` to `Vec<Repr<SignerId>>` relies on `Repr<T>` being `#[repr(transparent)]` but transmute cannot verify this at compile time; if `Repr` layout changes, this becomes undefined behavior that could corrupt memory.

> **Fix:** [see code in PR]

### 56. Missing #[expect] attribute for Vec transmute
**File:** [`config/mutate.rs`:360](https://github.com/calimero-network/core/pull/2043#discussion_r2904740730) | **Bot:** meroreviewer

The `manage_context_allowlist` function uses `mem::transmute` on Vec without the `#[expect(clippy::transmute_undefined_repr)]` attribute that the rest of the codebase uses for identical operations (see `query/near.rs:65-69`).

> **Fix:** [see code in PR]

---

## 💡 LOW Priority (61)

### 1. Potential TOCTOU race in admin count verification
**File:** [`src/group_store.rs`:149](https://github.com/calimero-network/core/pull/2043#discussion_r2857198932) | **Bot:** meroreviewer

The `require_group_admin` check and subsequent operations are not atomic. A concurrent request could remove the last admin between the check and the operation, potentially leaving a group without admins.

> **Fix:** [see code in PR]

### 2. list_group_members performs value lookup per key
**File:** [`src/group_store.rs`:185](https://github.com/calimero-network/core/pull/2043#discussion_r2857199058) | **Bot:** meroreviewer

Same N+1 pattern as count_group_admins; for paginated results with limit L, this makes L database reads after the key scan.

> **Fix:** [see code in PR]

### 3. Startup upgrade scan reads all upgrade values
**File:** [`src/group_store.rs`:462](https://github.com/calimero-network/core/pull/2043#discussion_r2857199197) | **Bot:** meroreviewer

enumerate_in_progress_upgrades scans all GroupUpgradeKey entries and reads each value to filter by InProgress status; could be slow with many groups.

> **Fix:** [see code in PR]

### 4. Duplicate members inflate admin count in removal check
**File:** [`handlers/remove_group_members.rs`:64](https://github.com/calimero-network/core/pull/2043#discussion_r2857199228) | **Bot:** cursor

In `remove_group_members`, the `admins_being_removed` counter counts every occurrence of an admin in the `members` list, including duplicates. If the same admin identity appears multiple times, the count is inflated, causing the `admin_count <= admins_being_removed` check to incorrectly reject a valid removal request that would actually leave admins remaining.

### 5. JoinGroupRequest should bind joiner_identity to invitation cryptographically
**File:** [`src/group.rs`:201](https://github.com/calimero-network/core/pull/2043#discussion_r2859756574) | **Bot:** meroreviewer

The `joiner_identity` is passed separately from the invitation payload; if the invitation's `invitee_identity` is None (open invitation), any identity could be provided without cryptographic binding.

> **Fix:** [see code in PR]

### 6. register_context_in_group should be atomic
**File:** [`src/group_store.rs`:288](https://github.com/calimero-network/core/pull/2043#discussion_r2859756758) | **Bot:** meroreviewer

The function performs three separate store operations (delete old index, put new index, put ref) without a transaction; a crash between operations could leave orphaned or inconsistent indices.

> **Fix:** [see code in PR]

### 7. ApproveContextRegistration comment implies pre-approval but lacks expiration
**File:** [`src/lib.rs`:166](https://github.com/calimero-network/core/pull/2043#discussion_r2859756967) | **Bot:** meroreviewer

The `ApproveContextRegistration` variant pre-approves a context for registration but doesn't include an expiration field, which could allow stale approvals to be used indefinitely.

> **Fix:** [see code in PR]

### 8. DRY: Repeated iteration pattern across multiple functions
**File:** [`src/group_store.rs`:161](https://github.com/calimero-network/core/pull/2043#discussion_r2859757161) | **Bot:** meroreviewer

The `first.into_iter().chain(iter.keys())` pattern with prefix/group_id boundary checks is duplicated across `count_group_admins`, `list_group_members`, `count_group_members`, `enumerate_group_contexts`, and `count_group_contexts`—consider extracting a helper that takes a closure for the inner loop logic.

> **Fix:** [see code in PR]

### 9. find_local_signing_identity returns any available key for privilege escalation in upgrades
**File:** [`src/group_store.rs`:242](https://github.com/calimero-network/core/pull/2043#discussion_r2859882689) | **Bot:** meroreviewer

This function finds any identity with a private key for group upgrades regardless of its specific permissions within the context; ensure calling code validates that the found identity is actually authorized for the upgrade action.

> **Fix:** [see code in PR]

### 10. Open invitations with optional expiration could persist indefinitely
**File:** [`src/group.rs`:207](https://github.com/calimero-network/core/pull/2043#discussion_r2859882874) | **Bot:** meroreviewer

CreateGroupInvitationRequest allows both invitee_identity and expiration to be None, potentially creating non-expiring open invitations that remain valid forever.

> **Fix:** [see code in PR]

### 11. Nonce not incremented after successful contract call
**File:** [`external/group.rs`:252](https://github.com/calimero-network/core/pull/2043#discussion_r2883555436) | **Bot:** cursor

In `with_nonce`, when `f(n).await` succeeds (line 106), the function returns immediately without incrementing `*nonce`. Subsequent operations reuse the stale nonce, which the contract rejects, forcing an extra round-trip to `fetch_nonce` plus a retry on every call after the first. For sequences of group operations (e.g., create → add members → register context), this doubles the number of network 

### 12. DRY: Duplicate unsafe pointer cast pattern
**File:** [`config/mutate.rs`:230](https://github.com/calimero-network/core/pull/2043#discussion_r2883582078) | **Bot:** meroreviewer

The unsafe slice-to-Repr conversion appears twice (lines 228-233 and 247-248); consider extracting a helper function like `fn as_repr_slice<T>(slice: &[T]) -> &[Repr<T>]` to centralize the safety invariant.

> **Fix:** [see code in PR]

### 13. Using serde_json::Value for target_application may bypass type validation
**File:** [`config/requests.rs`:150](https://github.com/calimero-network/core/pull/2043#discussion_r2883582229) | **Bot:** meroreviewer

Deserializing `target_application` as `serde_json::Value` defers type validation and could allow malformed or unexpected data to propagate if not validated when consumed downstream.

> **Fix:** [see code in PR]

### 14. DRY: Repetitive mutate client construction
**File:** [`external/group.rs`:264](https://github.com/calimero-network/core/pull/2043#discussion_r2883582373) | **Bot:** meroreviewer

Every method repeats `c.sdk_client.mutate::<ContextConfig>(c.protocol.as_str().into(), c.network_id.as_str().into(), c.contract_id.as_str().into())`; extract a `fn mutate_client(&self) -> ...` helper method on `GroupClientInner`.

> **Fix:** [see code in PR]

### 15. Unnecessary clone of Application in async block
**File:** [`external/group.rs`:387](https://github.com/calimero-network/core/pull/2043#discussion_r2883582469) | **Bot:** meroreviewer

target_application.clone() allocates when the Application contains owned data; this occurs on every retry iteration inside with_nonce.

> **Fix:** [see code in PR]

### 16. with_nonce retry loop exits early on first non-nonce error
**File:** [`external/group.rs`:215](https://github.com/calimero-network/core/pull/2043#discussion_r2883582637) | **Bot:** meroreviewer

When the fetched nonce equals the old nonce and an error occurred, the loop returns the error assuming it's not a stale-nonce issue; however, transient network errors could also cause this pattern, wasting remaining retries.

> **Fix:** [see code in PR]

### 17. New group types lack unit tests in this crate
**File:** [`src/group.rs`:1](https://github.com/calimero-network/core/pull/2043#discussion_r2883595741) | **Bot:** meroreviewer

GroupUpgradeStatus, GroupUpgradeInfo, and the request/response types have no tests for serialization or From impls in this module.

> **Fix:** [see code in PR]

### 18. CreateGroupRequest accepts optional signing_key that may be logged or serialized
**File:** [`src/group.rs`:54](https://github.com/calimero-network/core/pull/2043#discussion_r2883595861) | **Bot:** meroreviewer

Private key material (`signing_key: Option<[u8; 32]>`) in request structs could be accidentally logged, serialized, or exposed in error messages if not handled carefully.

> **Fix:** [see code in PR]

### 19. DRY: Boilerplate pattern repeated ~18 times for group operations
**File:** [`src/client.rs`:995](https://github.com/calimero-network/core/pull/2043#discussion_r2883595972) | **Bot:** meroreviewer

Every group operation follows the identical pattern: create oneshot channel, send message variant, await response with expect. Consider a macro or generic helper to reduce duplication.

> **Fix:** [see code in PR]

### 20. DRY: Unsafe Repr slice cast duplicated
**File:** [`config/mutate.rs`:234](https://github.com/calimero-network/core/pull/2043#discussion_r2883596074) | **Bot:** meroreviewer

The unsafe pointer cast from `&[SignerId]` to `&[Repr<SignerId>]` is repeated in both `add_group_members` and `remove_group_members`.

> **Fix:** [see code in PR]

### 21. target_application.clone() called inside retry loop
**File:** [`external/group.rs`:377](https://github.com/calimero-network/core/pull/2043#discussion_r2883596238) | **Bot:** meroreviewer

In set_group_target, target_application.clone() is invoked on every retry iteration; if Application contains heap-allocated data and retries occur, this causes redundant clones.

> **Fix:** [see code in PR]

### 22. Unsafe pointer cast for slice transmutation
**File:** [`config/mutate.rs`:223](https://github.com/calimero-network/core/pull/2043#discussion_r2888567567) | **Bot:** meroreviewer

Raw pointer casting from `[SignerId]` to `[Repr<SignerId>]` relies on unverified `#[repr(transparent)]` assumption; a breaking change to `Repr` could cause memory corruption.

> **Fix:** [see code in PR]

### 23. Retry loop iteration count may exceed intended MAX_RETRIES
**File:** [`external/group.rs`:211](https://github.com/calimero-network/core/pull/2043#discussion_r2888567889) | **Bot:** meroreviewer

The loop `for _ in 0..=retries` runs `retries + 1` times; combined with `MAX_RETRIES + u8::from(nonce.is_none())`, this results in up to 5 attempts when nonce is None (MAX_RETRIES=3), which may be more than expected.

> **Fix:** [see code in PR]

### 24. Signing key stored as plain bytes in memory
**File:** [`external/group.rs`:25](https://github.com/calimero-network/core/pull/2043#discussion_r2888568062) | **Bot:** meroreviewer

The `signing_key: [u8; 32]` is held in plain memory; consider using a zeroizing wrapper to clear sensitive key material when dropped.

> **Fix:** [see code in PR]

### 25. Unsafe slice pointer cast relies on undocumented Repr invariant
**File:** [`config/mutate.rs`:222](https://github.com/calimero-network/core/pull/2043#discussion_r2888568193) | **Bot:** meroreviewer

The pointer cast from `&[SignerId]` to `&[Repr<SignerId>]` assumes identical layout; this invariant should be enforced with a static assertion rather than just a comment.

> **Fix:** [see code in PR]

### 26. Unsafe pointer cast relies on Repr<T> layout guarantee
**File:** [`config/mutate.rs`:219](https://github.com/calimero-network/core/pull/2043#discussion_r2888577263) | **Bot:** meroreviewer

The unsafe cast from `[SignerId]` to `[Repr<SignerId>]` assumes `#[repr(transparent)]` on `Repr<T>`, but this invariant should be enforced at the type definition site with a compile-time assertion.

> **Fix:** [see code in PR]

### 27. Silent key skipping lacks observability
**File:** [`src/iter.rs`:125](https://github.com/calimero-network/core/pull/2043#discussion_r2888577416) | **Bot:** meroreviewer

Keys with mismatched sizes are silently skipped during iteration without any logging or metrics, making it difficult to detect data corruption or bugs in key storage.

> **Fix:** [see code in PR]

### 28. mem::transmute for Vec conversion is fragile
**File:** [`query/near.rs`:195](https://github.com/calimero-network/core/pull/2043#discussion_r2888577573) | **Bot:** meroreviewer

Using `mem::transmute` to convert `Vec<Repr<ContextId>>` to `Vec<ContextId>` depends on identical memory layout; this is correct but fragile if Repr's definition changes.

> **Fix:** [see code in PR]

### 29. Optional requester field may bypass authorization
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2888797586) | **Bot:** meroreviewer

The `delete_context` method accepts an optional `requester` parameter sent to the server; ensure server-side validates this against authenticated identity to prevent deletion by unauthorized parties.

> **Fix:** [see code in PR]

### 30. Unsafe pointer cast relies on undocumented invariant
**File:** [`config/mutate.rs`:223](https://github.com/calimero-network/core/pull/2043#discussion_r2888797757) | **Bot:** meroreviewer

The unsafe slice cast from `&[SignerId]` to `&[Repr<SignerId>]` relies on `Repr<T>` being `#[repr(transparent)]`; this invariant should be enforced at the type definition level.

> **Fix:** [see code in PR]

### 31. DRY violation: duplicated unsafe slice cast pattern
**File:** [`config/mutate.rs`:226](https://github.com/calimero-network/core/pull/2043#discussion_r2888797947) | **Bot:** meroreviewer

The unsafe pointer cast from `[SignerId]` to `[Repr<SignerId>]` is duplicated verbatim in both `add_group_members` and `remove_group_members`; extract to a reusable helper function with centralized safety documentation.

> **Fix:** [see code in PR]

### 32. DRY violation: repeated unsafe transmute pattern for Repr unwrapping
**File:** [`query/near.rs`:194](https://github.com/calimero-network/core/pull/2043#discussion_r2888798388) | **Bot:** meroreviewer

The transmute-with-expect pattern for converting `Vec<Repr<T>>` or `Option<Repr<T>>` to their inner types is repeated in `GroupContextsRequest` and `ContextGroupRequest` decode methods.

> **Fix:** [see code in PR]

### 33. Retry count calculation may be confusing
**File:** [`external/group.rs`:199](https://github.com/calimero-network/core/pull/2043#discussion_r2888798606) | **Bot:** meroreviewer

The expression `MAX_RETRIES + u8::from(nonce.is_none())` combined with `for _ in 0..=retries` results in 4-5 iterations total; if MAX_RETRIES=3 means '3 retries after initial attempt', the loop will execute one extra time.

> **Fix:** [see code in PR]

### 34. Breaking API change without migration path
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2888815608) | **Bot:** meroreviewer

The `delete_context` signature changed to require `Option<PublicKey>` for `requester`, which is a breaking change for existing callers that may not have context about when to provide this value.

> **Fix:** [see code in PR]

### 35. Unsafe pointer cast for slice transmutation
**File:** [`config/mutate.rs`:221](https://github.com/calimero-network/core/pull/2043#discussion_r2888815772) | **Bot:** meroreviewer

While documented as safe due to Repr<T> being transparent, raw pointer casts bypass Rust's type system; a future change to Repr could silently break this invariant.

> **Fix:** [see code in PR]

### 36. Delete context now accepts optional requester without documented authorization model
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2888815933) | **Bot:** meroreviewer

The `delete_context` method now takes an optional `requester` parameter; if `None` bypasses authorization checks server-side, this could allow unauthorized deletions for group-associated contexts.

> **Fix:** [see code in PR]

### 37. Group invitation signatures stored as untyped hex strings
**File:** [`src/types.rs`:677](https://github.com/calimero-network/core/pull/2043#discussion_r2888816070) | **Bot:** meroreviewer

Unlike the existing `Signed` type which uses typed `Signature` and has built-in verification, `SignedGroupRevealPayload` stores signatures as plain `String`; this defers validation to the caller and may allow malformed signatures to propagate.

> **Fix:** [see code in PR]

### 38. Duplicated unsafe pointer cast pattern
**File:** [`config/mutate.rs`:227](https://github.com/calimero-network/core/pull/2043#discussion_r2888816223) | **Bot:** meroreviewer

The unsafe `ptr::from_ref` cast to `Repr<SignerId>` slice is duplicated in both `add_group_members` and `remove_group_members`; consider extracting a helper function like `as_repr_slice<T>()` to centralize this invariant.

> **Fix:** [see code in PR]

### 39. Type erasure loses compile-time safety
**File:** [`config/requests.rs`:154](https://github.com/calimero-network/core/pull/2043#discussion_r2888816441) | **Bot:** meroreviewer

Using `serde_json::Value` for `target_application` in `GroupInfoQueryResponse` loses type safety; consumers must handle parsing errors at runtime. The comment explains the lifetime constraint well, but consider a dedicated owned `ApplicationOwned` type.

> **Fix:** [see code in PR]

### 40. Optional requester parameter in delete_context may allow unauthorized deletion
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2889775476) | **Bot:** meroreviewer

The `delete_context` method now accepts an optional `requester` identity sent in the request body; ensure server-side validation confirms the requester has permission to delete the context.

> **Fix:** [see code in PR]

### 41. Unsafe pointer cast relies on external invariant
**File:** [`config/mutate.rs`:232](https://github.com/calimero-network/core/pull/2043#discussion_r2889775663) | **Bot:** meroreviewer

The safety of casting `&[SignerId]` to `&[Repr<SignerId>]` depends on `Repr<T>` being `#[repr(transparent)]`, which is not enforced at this call site; if Repr's definition changes, this becomes unsound.

> **Fix:** [see code in PR]

### 42. DRY: Repeated unsafe pointer casting pattern
**File:** [`config/mutate.rs`:229](https://github.com/calimero-network/core/pull/2043#discussion_r2889775824) | **Bot:** meroreviewer

The same unsafe `ptr::from_ref` cast to `&[Repr<SignerId>]` is duplicated in both `add_group_members` and `remove_group_members`; this also matches existing patterns in `requests.rs`.

> **Fix:** [see code in PR]

### 43. Invitation payload signature verification not visible in diff
**File:** [`src/types.rs`:677](https://github.com/calimero-network/core/pull/2043#discussion_r2889775987) | **Bot:** meroreviewer

New group invitation types (`SignedGroupOpenInvitation`, `SignedGroupRevealPayload`) include signature fields but the verification logic is not shown; ensure signatures are verified before trusting invitation data.

> **Fix:** [see code in PR]

### 44. New GroupRequest and GroupRequestKind lack doc comments
**File:** [`src/lib.rs`:105](https://github.com/calimero-network/core/pull/2043#discussion_r2889776135) | **Bot:** meroreviewer

The new `GroupRequest` struct and `GroupRequestKind` enum variants are undocumented, making it harder for developers to understand the purpose of each operation.

> **Fix:** [see code in PR]

### 45. Type erasure with serde_json::Value loses compile-time safety
**File:** [`config/requests.rs`:149](https://github.com/calimero-network/core/pull/2043#discussion_r2889776296) | **Bot:** meroreviewer

Using `serde_json::Value` for `target_application` in `GroupInfoQueryResponse` loses type safety; the comment explains the lifetime constraint well, but consumers must handle arbitrary JSON.

> **Fix:** [see code in PR]

### 46. DRY: Duplicated unsafe slice conversion pattern
**File:** [`config/mutate.rs`:226](https://github.com/calimero-network/core/pull/2043#discussion_r2890426131) | **Bot:** meroreviewer

The unsafe `ptr::from_ref` transmutation pattern for converting `&[SignerId]` to `&[Repr<SignerId>]` is duplicated in both `add_group_members` and `remove_group_members`.

> **Fix:** [see code in PR]

### 47. Delete context accepts optional requester without client-side validation
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2890426512) | **Bot:** meroreviewer

The `delete_context` method now accepts an optional `requester` parameter sent directly to the server; ensure the server validates that the authenticated user matches or is authorized to act as the requester.

> **Fix:** [see code in PR]

### 48. Group invitation types lack signature verification methods
**File:** [`src/types.rs`:680](https://github.com/calimero-network/core/pull/2043#discussion_r2890426864) | **Bot:** meroreviewer

The `SignedGroupOpenInvitation` and `SignedGroupRevealPayload` types store signatures as hex strings but no verification logic is visible; ensure signature verification occurs server-side before trusting invitation data.

> **Fix:** [see code in PR]

### 49. GroupRequest and GroupRequestKind could benefit from #[non_exhaustive]
**File:** [`src/lib.rs`:105](https://github.com/calimero-network/core/pull/2043#discussion_r2890427047) | **Bot:** meroreviewer

While `GroupRequestKind` is marked with `#[expect(clippy::exhaustive_enums)]`, `GroupRequest` struct lacks `#[non_exhaustive]` which would allow adding fields without breaking downstream crates.

> **Fix:** [see code in PR]

### 50. Consider enum for role field instead of String
**File:** [`config/requests.rs`:138](https://github.com/calimero-network/core/pull/2043#discussion_r2890427249) | **Bot:** meroreviewer

`GroupMemberQueryEntry.role` uses `String` which defers validation to runtime; an enum would provide compile-time guarantees and better API discoverability.

> **Fix:** [see code in PR]

### 51. GroupRequest and GroupRequestKind lack serialization tests
**File:** [`src/lib.rs`:105](https://github.com/calimero-network/core/pull/2043#discussion_r2894950378) | **Bot:** meroreviewer

New protocol types `GroupRequest` and `GroupRequestKind` are user-facing but have no roundtrip serialization tests to catch breaking changes.

> **Fix:** [see code in PR]

### 52. Enumerate all groups uses wrong prefix boundary check
**File:** [`src/group_store.rs`:71](https://github.com/calimero-network/core/pull/2043#discussion_r2897061234) | **Bot:** cursor

In `enumerate_all_groups`, the iteration break condition checks `key.as_key().as_bytes()[0] >= GROUP_MEMBER_PREFIX` to stop before member keys. However, since the iterator was opened as `iter::<GroupMeta>()`, keys from different prefix families are being compared by their raw first byte. If the store's key layout changes or if `GROUP_MEMBER_PREFIX` isn't immediately after the GroupMeta prefix, thi

### 53. Optional requester in delete_context weakens authorization audit trail
**File:** [`src/client.rs`:510](https://github.com/calimero-network/core/pull/2043#discussion_r2897080068) | **Bot:** meroreviewer

The `requester: Option<PublicKey>` parameter allows deletion without identifying who requested it; if server-side enforcement is missing, this could allow unauthorized deletions.

> **Fix:** [see code in PR]

### 54. Secret salt in GroupInvitationFromAdmin exposed before reveal phase
**File:** [`src/types.rs`:695](https://github.com/calimero-network/core/pull/2043#discussion_r2897080187) | **Bot:** meroreviewer

The `secret_salt` field is included in the `GroupInvitationFromAdmin` struct which is Borsh-serialized; if transmitted to the invitee before commitment, MEV protection is weakened.

> **Fix:** [see code in PR]

### 55. Repeated transmute pattern for Vec<Repr<T>> conversions
**File:** [`query/near.rs`:202](https://github.com/calimero-network/core/pull/2043#discussion_r2897080522) | **Bot:** meroreviewer

The transmute pattern for converting `Vec<Repr<T>>` to `Vec<T>` is now used in multiple decode methods; a centralized helper would improve maintainability and consolidate safety documentation.

> **Fix:** [see code in PR]

### 56. Type safety loss with serde_json::Value for target_application
**File:** [`config/requests.rs`:153](https://github.com/calimero-network/core/pull/2043#discussion_r2897080670) | **Bot:** meroreviewer

Using `serde_json::Value` loses compile-time type checking; consider introducing a dedicated DTO type or documenting runtime validation requirements.

> **Fix:** [see code in PR]

### 57. Nonce not incremented after successful contract call
**File:** [`external/group.rs`:252](https://github.com/calimero-network/core/pull/2043#discussion_r2904716424) | **Bot:** cursor

The `with_nonce` function returns immediately on a successful `f(nonce)` call without updating the stored nonce to `nonce + 1`. Any subsequent operation on the same `ExternalGroupClient` will use the stale (already-consumed) nonce, causing an unnecessary failed contract call before the retry logic fetches a fresh nonce. For sequential group operations (e.g., add members then upgrade), this wastes 

### 58. Unsafe pointer cast relies on Repr transparency invariant
**File:** [`config/mutate.rs`:222](https://github.com/calimero-network/core/pull/2043#discussion_r2904740859) | **Bot:** meroreviewer

The unsafe block casts `&[SignerId]` to `&[Repr<SignerId>]` via raw pointer; while documented as safe due to `Repr` being transparent, this invariant is not compiler-enforced and could silently break.

> **Fix:** [see code in PR]

### 59. Unbounded enumeration may cause high memory usage for large groups
**File:** [`src/group_store.rs`:676](https://github.com/calimero-network/core/pull/2043#discussion_r2904741054) | **Bot:** meroreviewer

enumerate_group_contexts with usize::MAX loads all context IDs into memory. For groups with thousands of contexts, this causes O(n) memory allocation. The same pattern appears in ~15 locations across handlers.

> **Fix:** [see code in PR]

### 60. Vec transmute missing clippy annotation for consistency
**File:** [`config/mutate.rs`:409](https://github.com/calimero-network/core/pull/2043#discussion_r2904741271) | **Bot:** meroreviewer

The transmute on Vec types is technically sound since Repr<T> is #[repr(transparent)], but similar patterns in query/near.rs use #[expect(clippy::transmute_undefined_repr)] to suppress warnings. This is an established codebase pattern but the annotation ensures consistency.

> **Fix:** [see code in PR]

### 61. Unknown fields silently ignored during identity deserialization
**File:** [`src/lib.rs`:525](https://github.com/calimero-network/core/pull/2043#discussion_r2904741463) | **Bot:** meroreviewer

The identity deserializer drops unknown fields with `de::IgnoredAny`; while convenient, this could mask typos in security-sensitive config keys like 'group' being misspelled, leaving the group identity unexpectedly unset.

> **Fix:** [see code in PR]

---

## 📝 NITPICK Priority (6)

### 1. Nit: From impls could use explicit field names
**File:** [`src/group.rs`:287](https://github.com/calimero-network/core/pull/2043#discussion_r2883582747) | **Bot:** meroreviewer

The `From<GroupUpgradeValue> for GroupUpgradeInfo` impl uses implicit field matching; explicit field names improve readability when structs have many fields.

> **Fix:** [see code in PR]

### 2. Nit: Missing doc comment explaining nonce=0 for create_group
**File:** [`external/group.rs`:250](https://github.com/calimero-network/core/pull/2043#discussion_r2883582850) | **Bot:** meroreviewer

The `create_group` method bypasses `with_nonce` and uses nonce=0 directly; a brief comment explaining why (first operation for a new group has no prior nonce) would aid maintainability.

> **Fix:** [see code in PR]

### 3. Nit: ContextGroupId and AppKey have nearly identical implementations
**File:** [`src/types.rs`:240](https://github.com/calimero-network/core/pull/2043#discussion_r2883596374) | **Bot:** meroreviewer

Both types wrap Identity with the same trait implementations and methods; a macro could reduce the ~90 lines of duplication.

> **Fix:** [see code in PR]

### 4. Nit: Complex retry logic could benefit from documentation
**File:** [`external/group.rs`:207](https://github.com/calimero-network/core/pull/2043#discussion_r2888577691) | **Bot:** meroreviewer

The `with_nonce` function has subtle behavior (extra retry when nonce is None, error short-circuit when nonce unchanged) that would benefit from inline comments explaining the intent.

> **Fix:** [see code in PR]

### 5. Nit: ProposalAction variants lack documentation
**File:** [`src/lib.rs`:175](https://github.com/calimero-network/core/pull/2043#discussion_r2888577867) | **Bot:** meroreviewer

New `RegisterInGroup` and `UnregisterFromGroup` variants added to `ProposalAction` lack doc comments explaining when/how they should be used.

> **Fix:** [see code in PR]

### 6. Nit: GroupRequest lacks documentation
**File:** [`src/lib.rs`:108](https://github.com/calimero-network/core/pull/2043#discussion_r2888816639) | **Bot:** meroreviewer

The new `GroupRequest` struct and `GroupRequestKind` enum lack doc comments explaining their purpose and usage, unlike some other request types in this file.

> **Fix:** [see code in PR]

---
