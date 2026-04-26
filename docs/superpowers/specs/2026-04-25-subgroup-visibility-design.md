# Subgroup Visibility — Move `Open`/`Restricted` from contexts to subgroups

**Issue:** [calimero-network/core#2256](https://github.com/calimero-network/core/issues/2256)
**Date:** 2026-04-25
**Status:** Draft for review
**Branch:** `feat/subgroup-visibility-2256`

## Summary

Today the `VisibilityMode { Open, Restricted }` enum is documented as "visibility mode for a context within a group" and stored as `default_visibility` on group metadata. In practice it is dead code in the join path: `join_context` never consults it, the `CAN_JOIN_OPEN_CONTEXTS` capability bit that pairs with it is also never checked, and the existing comment in `join_context.rs:132-134` explicitly acknowledges the gap:

> ```text
> // Group membership already verified above. All contexts in a group
> // a member has access to are joinable. Restricted access is handled
> // at the subgroup level (admin must explicitly add member to the subgroup).
> ```

This design relocates visibility from the (unused) context level to the subgroup level, where it actually makes sense. After this change:

- A subgroup is `Open` or `Restricted`.
- Members of a parent group are automatically members of any `Open` child subgroup (and transitively, any contexts inside it) **provided they hold the `CAN_JOIN_OPEN_SUBGROUPS` capability bit** at the parent group level. Restricted subgroups remain explicit-membership only.
- `default_visibility` is removed entirely. The `CAN_JOIN_OPEN_CONTEXTS` capability bit is renamed to `CAN_JOIN_OPEN_SUBGROUPS` (same bit slot, `1 << 2`, retired meaning replaced with the new one). It is granted by default to new members, so admins use it as a per-member deny-list ("revoke from this user to keep them out of Open subgroups even though they're in the parent").
- One visibility primitive on the subgroup, one capability bit on the member. Both must align for inherited membership.

## Goals

1. Single visibility knob, attached to subgroups, with real enforcement.
2. Parent-namespace members with the `CAN_JOIN_OPEN_SUBGROUPS` capability get transparent access to `Open` subgroups (and their contexts) without per-member `add_group_members` cascades.
3. The `Restricted` setting acts as a wall in the parent-walk: inheritance stops at the first `Restricted` ancestor.
4. Per-member deny-list: admins can revoke `CAN_JOIN_OPEN_SUBGROUPS` from a specific member to block their inheritance into Open subgroups while leaving them in the parent.
5. App-level workarounds (mero-drive's `useInheritCascade`, namespace-wide member fan-out) become unnecessary for the common case.
6. Battleships' invite-link flow benefits directly: a host marks the namespace's gameplay subgroup as `Open`, and any second player who joins the namespace (and gets default capabilities, including `CAN_JOIN_OPEN_SUBGROUPS`) gains context access immediately, with no per-context invite step.

## Non-goals

- **Admin chain inheritance.** `is_group_admin` already does NOT walk parents (despite what issue #2256 implies); cascading admin authority is a separate, security-sensitive change and is out of scope here.
- **Per-context override.** There is no per-context visibility flag today and we are not adding one. Visibility lives at one level.
- **Migration.** Treated as greenfield. Pre-existing on-disk `default_visibility` values are unreachable after the storage-key rename; admins re-set via the new API if they want `Open`.
- **Capability cascade.** `get_member_capabilities` continues to return only direct capability rows. Inherited members on `Open` subgroups get whatever default caps the subgroup grants on first effective read; we do not synthesize inherited cap rows on the fly.

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│ Namespace (root group)                                           │
│  members: alice (admin),                                         │
│           bob   [caps: CAN_JOIN_OPEN_SUBGROUPS],                 │
│           carol [caps: CAN_JOIN_OPEN_SUBGROUPS revoked]          │
│  subgroup_visibility: (n/a — root)                               │
│                                                                  │
│  ├── Subgroup: "general"   subgroup_visibility = Open            │
│  │    direct members: alice                                      │
│  │    effective members: alice, bob   ← walked + cap-checked     │
│  │    (carol excluded: cap revoked at namespace level)           │
│  │    contexts: chat-001, chat-002                               │
│  │       → bob can join_context(chat-001) without invite         │
│  │       → carol cannot, despite "general" being Open            │
│  │                                                               │
│  └── Subgroup: "leadership"  subgroup_visibility = Restricted    │
│       direct members: alice                                      │
│       effective members: alice                                   │
│       contexts: planning-001                                     │
│          → bob and carol both CANNOT join_context(planning-001)  │
└──────────────────────────────────────────────────────────────────┘
```

Walk semantics for `check_group_membership(group, identity)`:

1. If `identity` is a direct member of `group` → `true`.
2. Else read `group.subgroup_visibility`. If `Restricted` (or unset) → `false`. Stop.
3. Else (`Open`), look up `group`'s parent. If no parent → `false`.
4. If `identity` is a direct member of the parent (the **anchor**), check `identity`'s capabilities at the parent:
   - If admin of parent → `true` (admin override, matches existing `is_group_admin_or_has_capability` pattern).
   - Else if `CAN_JOIN_OPEN_SUBGROUPS` bit is set → `true`.
   - Else → `false` (admin has explicitly revoked the cap).
5. Otherwise, recurse step 2 with the parent group as `current`.

The first `Restricted` ancestor terminates the walk. The walk also terminates at the namespace root (which has no parent) — namespace-level membership is the source of truth.

**Anchor-level cap check:** the cap is evaluated exactly once per call, at the level where the direct membership row is found. Walking bottom-up, the deepest direct membership wins — that group's cap value decides whether inheritance applies. This matches "most-specific membership decides" intuition.

`subgroup_visibility` set on the root group itself is a no-op: there is no parent to inherit from. The setting is only meaningful on non-root groups. We do not reject the op for the root, but we do not act on it either; this keeps the API uniform.

## Data model

### Storage key (rename + clean cut)

`crates/store/src/key/group/mod.rs`

| Old | New |
|---|---|
| `GroupDefaultVis` | `GroupSubgroupVis` |
| `GroupDefaultVisValue { mode: u8 }` | `GroupSubgroupVisValue { mode: u8 }` |

Byte-prefix changes with the rename, so any pre-existing values become unreachable. This is intentional — the old field was cosmetic, so there is nothing functional to migrate.

`mode` encoding stays:
- `0` → `VisibilityMode::Open`
- `1` → `VisibilityMode::Restricted`
- absent (no key) → treated as `Restricted` at read sites

### Group store layer

`crates/context/src/group_store/capabilities.rs`

```rust
// Replaces get_default_visibility / set_default_visibility / delete_default_visibility.
pub fn get_subgroup_visibility(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<VisibilityMode>;  // returns Restricted when key absent

pub fn set_subgroup_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    mode: VisibilityMode,
) -> EyreResult<()>;

pub fn delete_subgroup_visibility(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<()>;
```

Note the typed signature: `VisibilityMode` instead of raw `u8`. The `u8` was a workaround for the old governance op shape and is no longer needed at the public surface.

### `VisibilityMode` enum

`crates/context/config/src/lib.rs:123-128` — enum stays, doc comment changes:

```rust
/// Visibility mode for a subgroup within its parent group.
///
/// `Open`     → parent-group members are inherited as members of this subgroup
///              (and, transitively, of any contexts it contains).
/// `Restricted` → membership requires an explicit add_group_members call.
///
/// The walk in `check_group_membership` stops at the first `Restricted`
/// ancestor; a `Restricted` subgroup is a wall regardless of what sits above.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VisibilityMode {
    Open,
    Restricted,
}
```

## Behavior change

### `check_group_membership` walks parents on `Open`, gated by cap at anchor

`crates/context/src/group_store/membership.rs:118-124`

```rust
pub fn check_group_membership(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    if has_direct_member(store, group_id, identity)? {
        return Ok(true);
    }

    // Walk parent chain only when THIS subgroup is Open. A Restricted
    // subgroup is a membership wall — direct membership is required.
    let mut current = *group_id;
    loop {
        if get_subgroup_visibility(store, &current)? != VisibilityMode::Open {
            return Ok(false);
        }
        let Some(parent) = get_parent_group(store, &current)? else {
            return Ok(false);
        };
        if has_direct_member(store, &parent, identity)? {
            // Anchor found. Admins always inherit; non-admins need the cap.
            if is_group_admin(store, &parent, identity)? {
                return Ok(true);
            }
            let caps = get_member_capability(store, &parent, identity)?.unwrap_or(0);
            return Ok(caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS != 0);
        }
        current = parent;
    }
}
```

Performance: O(depth-of-tree) `RocksDB` reads per check, plus one cap read at the anchor. Group hierarchies are shallow in practice (namespace → subgroup → leaf), so this is small and cacheable.

### `join_context` — no handler change, comment update

`crates/context/src/handlers/join_context.rs:132-140`

The handler already calls `check_group_membership` and bails on `false`. The behavior change happens automatically once the function above walks parents. The misleading comment gets corrected:

```rust
// Group membership covers both direct members and parent-chain members
// inherited through Open subgroups. Restricted subgroups still require
// an explicit add_group_members call by an admin.
if !group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
    bail!("identity is not a member of the group");
}
```

### `CAN_JOIN_OPEN_CONTEXTS` → `CAN_JOIN_OPEN_SUBGROUPS`

The bit slot `1 << 2` is reused with new semantics. The constant is renamed, the documentation is rewritten, the grant sites stay (the bit is still default-on), and the display sites get the new label.

`crates/context/config/src/lib.rs:137`:

```rust
/// Permits a parent-group member to be inherited as a member of any
/// `Open` subgroup beneath them. Granted by default; admins revoke
/// per-member when they want a specific user kept out of Open
/// subgroups even though they remain in the parent.
pub const CAN_JOIN_OPEN_SUBGROUPS: u32 = 1 << 2;
```

**Default-on policy preserved:**

- `crates/context/src/handlers/create_group.rs:126` — default capabilities for the creator group still include this bit (renamed reference).
- `crates/context/src/handlers/store_group_meta.rs:69-78` — fallback grant before `DefaultCapabilitiesSet` arrives still includes this bit (renamed reference).
- `crates/context/src/group_store/membership.rs` `add_group_member_with_keys` — already grants `default_capabilities` to non-admin members on add; nothing to change here, the new bit flows through automatically.

**Display renamed (not removed):**

- `crates/meroctl/src/output/groups.rs:452` — `"CAN_JOIN_OPEN_CONTEXTS"` → `"CAN_JOIN_OPEN_SUBGROUPS"`.
- `crates/meroctl/src/cli/group/members.rs:338` — same.
- `crates/meroctl/src/cli/group/settings.rs:88` — same.

The other `MemberCapabilities::*` bits are unaffected.

## API surface

### Governance op (`SetDefaultVisibility` → `SetSubgroupVisibility`)

`crates/context/primitives/src/group.rs`

```rust
// Replaces SetDefaultVisibilityRequest.
#[derive(Debug, Serialize, Deserialize)]
pub struct SetSubgroupVisibilityRequest {
    pub group_id: ContextGroupId,
    pub subgroup_visibility: VisibilityMode,
}

impl Message for SetSubgroupVisibilityRequest { ... }
```

`StoreDefaultVisibilityRequest` (the local-store-only sibling for applying received gossip) → `StoreSubgroupVisibilityRequest`. Same shape, renamed.

`crates/context/primitives/src/messages.rs` — `ContextMessage::SetDefaultVisibility` variant → `SetSubgroupVisibility`.

`crates/context/src/handlers/set_default_visibility.rs` → `set_subgroup_visibility.rs`. Logic identical: validate admin, emit governance op, return.

`crates/context/src/handlers/store_default_visibility.rs` → `store_subgroup_visibility.rs`. Same.

`crates/context/src/handlers.rs` — module declarations and the `match` arm in `handle()` updated.

`crates/context/primitives/src/client/mod.rs:1265-1267` — re-exports renamed.

### JSON-RPC admin endpoint

`crates/server/src/admin/handlers/groups/set_default_visibility.rs` → `set_subgroup_visibility.rs`.

`crates/server/src/admin/handlers/groups/get_group_info.rs:47` — response field renamed `default_visibility` → `subgroup_visibility`.

`crates/server/primitives/src/admin/mod.rs:1689` and `:2406` — request/response struct fields renamed; validator at `:2414` updated.

Routes registered via the admin router get the new path (the existing `set-default-visibility` URL is removed; new path `set-subgroup-visibility`).

### CLI (`meroctl`)

`crates/meroctl/src/cli/group/settings.rs`:
- Subcommand `set-default-visibility` → `set-subgroup-visibility`.
- Display: `default_visibility:` row in `meroctl group settings show` → `subgroup_visibility:`.
- Help text updated to describe parent-chain inheritance.

### Group info response shape

`crates/context/src/handlers/get_group_info.rs:39-57` — replace `default_visibility` field with `subgroup_visibility`. Returns `VisibilityMode` (typed in the response struct via serde rename to lowercase string for the wire).

## Apps consumer payoff

### mero-drive

After this lands, mero-drive can delete:
- `app/src/hooks/useInheritCascade.ts` — the manual cascade.
- The eager `add_group_members` fan-out in `useFolderOperations.create()` for "Inherit" folders.
- The "not a member" error fallback in `useFolderPermissions`.

Replace per-folder `visibility: 'Inherit' | 'Restricted'` with calling `SetSubgroupVisibility` once at folder creation. Late joiners get folder access automatically.

### Battleships invite-link

The host's namespace contains a gameplay subgroup. With `subgroup_visibility = Open` (and the joining player getting `CAN_JOIN_OPEN_SUBGROUPS` by default at namespace add), any player who accepts the namespace invite link gets immediate context access for the game session — no per-context invite, no membership cascade. Hosts can revoke the cap from a specific player to keep them out of the gameplay subgroup while leaving them in the namespace lobby.

## Testing strategy

Unit tests in `crates/context/src/group_store/tests.rs`:

1. `check_membership_direct_member` — direct member of any group passes (covers the existing path).
2. `check_membership_open_subgroup_inherits_parent_with_default_cap` — parent member with default caps (which include `CAN_JOIN_OPEN_SUBGROUPS`) returns `true` for `Open` child without explicit row.
3. `check_membership_restricted_subgroup_does_not_inherit` — parent member returns `false` for `Restricted` child even with cap set.
4. `check_membership_restricted_wall_blocks_grandparent_inheritance` — three-level chain (`namespace → restricted_mid → open_leaf`); namespace member returns `false` for `open_leaf` because the walk stops at the `Restricted` middle.
5. `check_membership_open_chain_walks_to_root` — three-level chain (`namespace → open_mid → open_leaf`); namespace member with cap returns `true` for `open_leaf`.
6. `check_membership_unset_visibility_treated_as_restricted` — group with no `subgroup_visibility` key behaves like `Restricted`.
7. `check_membership_open_subgroup_blocked_when_cap_revoked` — parent member whose `CAN_JOIN_OPEN_SUBGROUPS` was explicitly cleared returns `false` for `Open` child.
8. `check_membership_open_subgroup_admin_inherits_without_cap` — parent admin returns `true` for `Open` child even when the cap bit is cleared (admin override).
9. `check_membership_anchor_cap_check_uses_deepest_direct_membership` — identity has direct rows on both `namespace` and intermediate `mid`, with cap set at `namespace` but cleared at `mid`. For `open_leaf` under `mid`, returns `false` (deepest anchor wins).
10. `set_subgroup_visibility_admin_only` — non-admin call rejected; admin call persists.
11. `set_subgroup_visibility_round_trip` — set then get returns the stored value.
12. `default_capabilities_include_can_join_open_subgroups` — newly added non-admin member gets the bit set automatically.

Integration tests (existing `join_context` scenarios extended):

13. `join_open_subgroup_context_as_namespace_member` — namespace member with default caps can `join_context` for a context inside an `Open` subgroup without being added to that subgroup.
14. `join_open_subgroup_context_blocked_when_cap_revoked` — same scenario after admin revokes `CAN_JOIN_OPEN_SUBGROUPS` returns the existing "identity is not a member of the group" error.
15. `join_restricted_subgroup_context_blocked_until_added` — same scenario with `Restricted` returns the existing error; works after explicit `add_group_members`.

E2E (`apps/e2e-kv-store/workflows/`): a new workflow `group-subgroup-visibility-inheritance.yml` that boots two nodes, has node A create a namespace + `Open` subgroup + context, has node B join only the namespace, and asserts node B can `join_context` and execute a method on the inner context. Naming follows the existing `group-*.yml` pattern in that directory.

`★ Insight ─────────────────────────────────────`
- Test 4 is the most important on the visibility axis — it pins down the "Restricted = wall" semantic. Without it, an admin could accidentally leak grandparent access by flipping a leaf to `Open`.
- Test 6 protects the read default for visibility. If the default ever silently flipped to `Open`, every existing-but-unset subgroup would suddenly become a public room.
- Tests 7 and 9 are the load-bearing ones for the cap axis. Test 7 confirms the cap actually denies (not just decorates the member row); Test 9 nails down which membership row's cap is consulted when there's ambiguity. Without 9, a future reader could justify either "shallowest" or "deepest" anchor and ship the wrong one.
- Test 8 ensures the admin-override doesn't silently disappear if someone refactors `is_group_admin` later — admins must keep transparently inheriting access.
`─────────────────────────────────────────────────`

## Files touched (summary)

**Crates with code changes:**
- `crates/store/src/key/group/mod.rs` — rename storage key.
- `crates/store/src/types/group.rs` — rename `PredefinedEntry` impl.
- `crates/store/src/key.rs` — rename re-export.
- `crates/context/config/src/lib.rs` — `VisibilityMode` doc; rename `CAN_JOIN_OPEN_CONTEXTS` → `CAN_JOIN_OPEN_SUBGROUPS` (same bit slot, new doc).
- `crates/context/src/group_store/capabilities.rs` — typed get/set/delete fns.
- `crates/context/src/group_store/membership.rs` — parent-walk in `check_group_membership`.
- `crates/context/src/group_store/mod.rs` — rename re-exports + wrappers.
- `crates/context/src/group_store/group_settings.rs` — wrapper rename.
- `crates/context/src/group_store/namespace_governance.rs` — handle renamed op variant.
- `crates/context/src/handlers.rs` — module decls + match arm.
- `crates/context/src/handlers/set_default_visibility.rs` → `set_subgroup_visibility.rs` — rename + retype.
- `crates/context/src/handlers/store_default_visibility.rs` → `store_subgroup_visibility.rs` — rename.
- `crates/context/src/handlers/get_group_info.rs` — field rename.
- `crates/context/src/handlers/create_group.rs` — rename constant reference (still grants the bit by default).
- `crates/context/src/handlers/store_group_meta.rs` — rename constant reference in fallback grant (still grants by default).
- `crates/context/src/handlers/join_context.rs` — comment update only.
- `crates/context/primitives/src/group.rs` — request structs renamed; field rename in `GroupInfo`.
- `crates/context/primitives/src/messages.rs` — `ContextMessage` variant rename.
- `crates/context/primitives/src/client/mod.rs` — re-exports.
- `crates/server/src/admin/handlers/groups/set_default_visibility.rs` → `set_subgroup_visibility.rs`.
- `crates/server/src/admin/handlers/groups/get_group_info.rs` — response field rename.
- `crates/server/primitives/src/admin/mod.rs` — request/response field rename + validator.
- `crates/server/src/admin/router.rs` (or equivalent) — route path rename.
- `crates/meroctl/src/cli/group/settings.rs` — subcommand + display rename.
- `crates/meroctl/src/cli/group/members.rs` — rename `CAN_JOIN_OPEN_CONTEXTS` row label.
- `crates/meroctl/src/cli/group/settings.rs` — rename in default-caps display block.
- `crates/meroctl/src/output/groups.rs` — rename `CAN_JOIN_OPEN_CONTEXTS` row label.

**Tests:**
- `crates/context/src/group_store/tests.rs` — add tests 1-8 above; rename existing `set_and_get_default_visibility` test.
- `crates/context/src/handlers/tests/...` (if join_context has handler tests) — add tests 9-10.
- `apps/e2e-kv-store/workflows/group-subgroup-visibility-inheritance.yml` — new E2E workflow.

## Open questions

None — design is greenfield (no migration), single approach (B), all surface renames committed to.

## Definition of done

- `cargo fmt --check` passes.
- `cargo clippy -- -A warnings` passes.
- `cargo test` passes (including new tests 1-10).
- `cargo deny check licenses sources` passes (no new deps expected).
- E2E workflow `subgroup-visibility-inheritance.yml` runs green.
- Issue #2256 referenced in the PR description with "closes" syntax.
