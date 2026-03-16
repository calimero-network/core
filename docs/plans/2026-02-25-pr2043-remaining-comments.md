# PR #2043 — Remaining Review Comments: Triage & Plan

**Date**: 2026-02-25
**PR**: https://github.com/calimero-network/core/pull/2043
**Follows**: `2026-02-25-pr2043-review-fixes.md` (Phases 1–3 complete)

---

## Triage Summary

After Phases 1–3 the remaining 47 open threads were reviewed. 46 fall into one of three
excluded categories. 1 is a genuine architectural issue requiring a fix.

---

## Excluded — Already Fixed

| Thread | Comment | Reason |
|--------|---------|--------|
| PRRT_kwDOLIG5Is5vhmVT | Asymmetric API between RegisterInGroup and UnregisterFromGroup | `UnregisterFromGroup` now has `group_id` field (lib.rs:216-218) |
| PRRT_kwDOLIG5Is5v96mD | `group_id` silently ignored in `create_context` | `register_context_in_group` called at create_context.rs:443 |
| PRRT_kwDOLIG5Is5v96lg | Group message handlers drop outcome channels | All handlers fully implemented; actor responds to every message |
| PRRT_kwDOLIG5Is5v96mr | Stub handlers silently drop group management requests | Same as above |
| PRRT_kwDOLIG5Is5v96nD | `GroupUpgradeStatus::RolledBack` uses unbounded String | `RolledBack` variant was removed; replaced with auto-retry |
| PRRT_kwDOLIG5Is5wm-yY | Invitation expiration only validated at creation, not join | join_group.rs:48-57 validates expiration at join time |
| PRRT_kwDOLIG5Is5vutRF | UpgradePolicy and GroupMemberRole appear unused | Both types are now used throughout the group implementation |
| PRRT_kwDOLIG5Is5vutST | YAGNI: types not mentioned in PR scope | Same — now fully used |

---

## Excluded — Design Decision / Accepted Risk

| Thread | Comment | Reason |
|--------|---------|--------|
| PRRT_kwDOLIG5Is5vjnb3 | Inconsistent member identity type (SignerId vs PublicKey) | Groups use on-chain `SignerId`; Contexts use local `PublicKey`. Different protocols by design |
| PRRT_kwDOLIG5Is5vvLcU / PRRT_kwDOLIG5Is5v96nh | `GroupMemberRole` re-exported from key module | Acceptable coupling; callers import via `key::`. Plan item #15 |
| PRRT_kwDOLIG5Is5vvLiI / PRRT_kwDOLIG5Is5weOA0 / PRRT_kwDOLIG5Is5wk6yA / PRRT_kwDOLIG5Is5wm-4y | DRY: `ContextGroupId` and `AppKey` near-identical | 45 lines of acceptable duplication. Plan item #18 |
| PRRT_kwDOLIG5Is5vvLaY / PRRT_kwDOLIG5Is5vvLgW | Key prefix uniqueness not cross-module verified | `distinct_prefixes` test verifies within module; 33-byte group keys cannot collide with 32-byte context keys. Plan item #20 |
| PRRT_kwDOLIG5Is5vvLeb | Group uses different identity type than Context | Design decision — same as #vjnb3 |
| PRRT_kwDOLIG5Is5vutTX | `GroupMemberRole` lacks `#[non_exhaustive]` | Acceptable variation. Plan item #10 |
| PRRT_kwDOLIG5Is5wDKQv / PRRT_kwDOLIG5Is5weNzX | Requester not cryptographically verified at handler level | Transport-layer auth (JWT/signed request) handles this. Plan item #32/#42 |
| PRRT_kwDOLIG5Is5wDKRX / PRRT_kwDOLIG5Is5weN1y / PRRT_kwDOLIG5Is5wk6rm / PRRT_kwDOLIG5Is5wm-zz / PRRT_kwDOLIG5Is5wm-vc | Non-deterministic group ID (`rand`) | Groups are node-local admin objects, not consensus objects. Plan item #33 |
| PRRT_kwDOLIG5Is5wDKTw / PRRT_kwDOLIG5Is5weN7U | Non-deterministic `created_at` timestamp | Groups are node-local; timestamp is metadata, not a consensus field. Plan item #36 |
| PRRT_kwDOLIG5Is5wDKNE / PRRT_kwDOLIG5Is5wDKNw | Missing authorization check in read handlers | Admin API is behind HTTP auth middleware. Consistent with `get_context` pattern |
| PRRT_kwDOLIG5Is5wnAYz | Concurrent upgrades bypass validation race | Actix actor processes messages sequentially — no concurrent handling |
| PRRT_kwDOLIG5Is5wm-rH / PRRT_kwDOLIG5Is5wm-tW | Non-atomic context creation + registration | Actix serialises all accesses; cross-operation atomicity is an accepted best-effort risk |
| PRRT_kwDOLIG5Is5wm-wy | `register_context_in_group` lacks group existence check | Caller (`create_context.rs`) verifies group exists before calling. Enforcing in the helper is redundant |
| PRRT_kwDOLIG5Is5weN5E | Group membership check insufficient for context creation | Correct by design — membership alone gates context creation |
| PRRT_kwDOLIG5Is5wm-1M | `expect()` on `receiver.await` could panic | Pre-existing established pattern across entire `ContextClient` (lines 126, 280, 409, …). Not introduced by group work |
| PRRT_kwDOLIG5Is5wm-2K | Unbatched store writes when adding members | Optimisation, not a correctness issue |
| PRRT_kwDOLIG5Is5weOC7 / PRRT_kwDOLIG5Is5wk6zA | Unnecessary `role.clone()` | `GroupMemberRole` does not implement `Copy` — clone is required |
| PRRT_kwDOLIG5Is5wk6xa | Repetitive boilerplate in `ContextClient` group methods | Refactoring suggestion; no functional impact |

---

## Excluded — Documentation / Testing Scope

| Thread(s) | Comment |
|-----------|---------|
| PRRT_kwDOLIG5Is5vjndt / PRRT_kwDOLIG5Is5vkoDh / PRRT_kwDOLIG5Is5vkoFl | Missing doc comments (GroupRequestKind, ContextGroupId, AppKey) |
| PRRT_kwDOLIG5Is5vjnfz / PRRT_kwDOLIG5Is5vkoBN | No serialization tests for new request types |
| PRRT_kwDOLIG5Is5vutUu / PRRT_kwDOLIG5Is5vutTX | No unit tests for ContextGroupId / AppKey |
| PRRT_kwDOLIG5Is5wDKO1 / PRRT_kwDOLIG5Is5weNww | No unit tests for group_store operations |

---

## Phase 4 — Architecture: Store Type Leakage (Low)

### Problem

`calimero_store::key::GroupUpgradeValue` (a storage layer type) is imported directly into
two files in the primitives crate:

```
crates/context/primitives/src/client.rs:19   use calimero_store::key::GroupUpgradeValue;
crates/context/primitives/src/group.rs:8     use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
```

This creates a dependency `context-primitives → calimero-store`, breaking the expected
layering where primitives are a thin message-passing boundary that should not depend on
storage internals. If `GroupUpgradeValue` ever changes (e.g. new borsh fields), it breaks
the API between the actor and its callers.

**Impact**: Architectural coupling; no runtime correctness impact today.

---

### Fix

Define a dedicated response type in `context/primitives/src/group.rs` that mirrors the
fields needed by API consumers, and translate from the store type inside the actor handler.

#### Task 4.1 — Define `GroupUpgradeInfo` in primitives

**File**: `crates/context/primitives/src/group.rs`

```rust
/// Snapshot of an in-progress or completed group upgrade, returned by the API.
/// Mirrors the storage-layer GroupUpgradeValue without a direct store dependency.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupUpgradeInfo {
    pub from_revision:        u32,
    pub to_revision:          u32,
    pub migration:            Option<Vec<u8>>,
    pub initiated_at:         u64,
    pub initiated_by:         PublicKey,
    pub status:               GroupUpgradeStatus,
}
```

Remove the `use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};` import and
re-export `GroupUpgradeStatus` from primitives (or duplicate it — it has no store-specific
logic). Update:
- `GroupInfoResponse.active_upgrade: Option<GroupUpgradeInfo>`
- `GetGroupUpgradeStatusRequest::Result = eyre::Result<Option<GroupUpgradeInfo>>`

#### Task 4.2 — Translate in handlers

**File**: `crates/context/src/handlers/get_group_upgrade_status.rs`
**File**: `crates/context/src/handlers/get_group_info.rs`

Add a `From<GroupUpgradeValue> for GroupUpgradeInfo` impl (or inline the mapping) so the
handlers translate the store type before returning it to the actor message system.

#### Task 4.3 — Remove store import from primitives

**Files**: `crates/context/primitives/src/client.rs`, `crates/context/primitives/src/group.rs`

Remove `use calimero_store::key::GroupUpgradeValue;` and update all type references.

---

## Verification

```bash
cargo check --workspace
cargo fmt --check
cargo clippy -- -A warnings
cargo test -p calimero-context
cargo test -p calimero-context-primitives
```

After Phase 4:
- `calimero-context-primitives` must have **no dependency** on `calimero-store` for this feature.
