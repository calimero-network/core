# calimero-authz - Unified Causal-Log Authorization

The single security boundary for the unified causal log: one fold, [`authorize`], deciding whether an op's author had authority at the op's own causal cut.

## Package Identity

- **Crate**: `calimero-authz`
- **Entry**: `src/lib.rs` (the whole crate - one file, no submodules; roughly half of it is `#[cfg(test)]`)
- **Key deps**: `calimero-op` (`Op`, `OpPayload`, `ScopeId`), `calimero-context-config` (`ContextGroupId`, `MemberCapabilities`), `calimero-primitives` (`PublicKey`, `GroupMemberRole`), `calimero-storage` (`address::Id`, `entities::OpMask`), `thiserror`

## Commands

```bash
# Build
cargo build -p calimero-authz

# Test (all)
cargo test -p calimero-authz

# Test a single case
cargo test -p calimero-authz inherited_membership_requires_open_chain_and_cap -- --nocapture
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `authorize(op, acl_at_cut)` | fn | The one decision: matches `op.payload`, returns `Ok(())` or a plane-specific `Rejected` |
| `required_mask_for(payload)` | fn | Maps a data `OpPayload` (`Put`/`Delete`) to the `OpMask` it needs; `None` for non-data payloads |
| `AclView` | struct | The authorization-relevant slice of projected state at a causal cut - what `authorize` decides against |
| `AclView::may(author, entity, required)` | fn | Data-plane check: explicit per-object ACL if one exists, else default-write-by-membership |
| `AclView::is_scope_member(author)` | fn | Is `author` in any group in this view (backs default-write) |
| `AclView::is_member_at_cut(group, author, root, default_cap_base)` | fn | Membership at the cut: direct, group-admin, or inherited over an open-subgroup chain |
| `AclView::is_authorized_admin(group, author, root)` | fn | Admin authority at the cut: group admin, root admin, or admin of an ancestor over the open chain |
| `AclView::member_path_at_cut(...)` | fn | Same walk as `is_member_at_cut`, returns the role-bearing `MemberPathAtCut` for enumeration callers |
| `AclView::capability(group, member)` | fn | Effective capability bitmask: member override, else group default, else `0` |
| `AclView::is_owner(author, object)` | fn | Owner = holds `OpMask::ADMIN` on `object` (confers writer-set rotation rights) |
| `AclView::is_group_admin(author, group)` | fn | Folded group admin (subgroup creator / `Admin`-role holder) |
| `AclView::is_root_admin(author)` | fn | Is `author` the scope's `root_admin` at the cut |
| `MemberPathAtCut` | enum | `None` / `Direct { role }` / `Inherited { anchor, via_admin }` - how `author` reaches membership |
| `SubgroupEdge` | struct | `{ parent: ScopeId, restricted: bool }` - a live subgroup's tree position + visibility at the cut |
| `Rejected` | enum (`ThisError`) | `NotPermitted { required: OpMask }` / `NotOwner` / `NotGroupAdmin` / `NotRootAdmin` - one rejection type for every plane |
| `DEFAULT_MEMBER_MASK` | const | `OpMask::WRITE.union(OpMask::DELETE)` - what a plain scope member holds on a non-restricted entity |
| `CAN_JOIN_OPEN_SUBGROUPS` | const | Mirrors `MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits()` - the cap gating inherited membership |
| `MAX_NAMESPACE_DEPTH` | const | `16` - bound on the subgroup-inheritance walk |

## Mental Model: the Authorization Fold

`authorize(op, acl_at_cut)` is a single `match` over `op.payload` (`OpPayload` lives in `calimero-op`). Each arm maps to exactly one authority plane:

| `OpPayload` variant(s) | Authority required | Rejection on failure |
| --- | --- | --- |
| `Put { entity, .. }` | `AclView::may(author, entity, OpMask::WRITE)` | `NotPermitted { required: WRITE }` |
| `Delete { entity }` | `AclView::may(author, entity, OpMask::DELETE)` | `NotPermitted { required: DELETE }` |
| `SetWriters { object, .. }` | `AclView::is_owner(author, object)` (holds `ADMIN` on `object`) | `NotOwner` |
| `MemberAdded` / `MemberRemoved { group, .. }` | `AclView::is_group_admin(author, group)` | `NotGroupAdmin` |
| `SubgroupVisibilitySet { scope, .. }` | `is_group_admin(author, scope-as-group)` | `NotGroupAdmin` |
| `DefaultCapabilitiesSet` / `MemberCapabilitySet { group, .. }` | `is_group_admin(author, group)` | `NotGroupAdmin` |
| `AdminChanged` / `PolicyUpdated` / `SubgroupCreated` / `SubgroupReparented` / `SubgroupDeleted` | `AclView::is_root_admin(author)` | `NotRootAdmin` |
| `Noop` | always `Ok(())` - a graph-only node, mutates nothing | - |

**Causal-honor semantics** is the reason this crate exists as a separate decision from live state: an op is authorized against the ACL/membership *as of its own causal parents*, never the receiver's current state. A write authored before a revocation stays valid regardless of the order a receiver later observes the revocation (the forward-only property). This crate never walks the DAG to get there - the caller (`calimero-projection`'s `ScopeState::acl_view_at(op.parents)`) resolves the `AclView`; `authorize` is a pure, unit-testable decision over that already-resolved value.

**Two-tier data authorization** (`AclView::may`): a **restricted** entity (one with an explicit per-object ACL entry) is authoritative - only listed writers with a sufficient mask pass, even for scope members. A **non-restricted** entity has no explicit ACL, so `default-write = membership`: any scope member gets `DEFAULT_MEMBER_MASK` (`WRITE`+`DELETE`, deliberately **not** `ADMIN`). This matches a shared key-value store where membership is the write boundary, while still letting an app narrow specific objects behind an explicit ACL grant.

**Inheritance walk** (`is_member_at_cut` / `is_authorized_admin` / `member_path_at_cut`): three functions walk the same `subgroups` parent chain (bounded by `MAX_NAMESPACE_DEPTH`), stopping at a `restricted` edge (a visibility wall). A group admin reached anywhere on the open chain grants immediately; a plain member only inherits through the *first* direct-member ancestor, and only if that ancestor's effective capability includes `CAN_JOIN_OPEN_SUBGROUPS`. `is_authorized_admin` is admin-only (no membership-only success); `is_member_at_cut` grants on either path; `member_path_at_cut` returns the same decision as a role-bearing enum for enumeration/listing callers.

## Relation to calimero-op / calimero-projection

- `calimero-op` defines the shared vocabulary this crate matches on: `Op`, `OpPayload`, `ScopeId`. `calimero-authz` takes `Op` as an opaque input and never constructs one.
- `calimero-projection` (`ScopeState::acl_view_at`) is the *only* producer of `AclView` in the real system: it folds the op log up to a causal cut into the `acl` / `groups` / `root_admin` / `default_caps` / `member_caps` / `subgroups` / `group_admin` maps this crate reads. This crate deliberately has no code path that reads a live store - swapping in a synthetic `AclView` (as the unit tests do) fully exercises the decision logic.
- `crates/context` (`scope_projection.rs`, `apply_authorizer.rs`) is the consumer: it calls `AclView::is_authorized_admin` / `is_member_at_cut` / `member_path_at_cut` directly (bypassing the `authorize` top-level match) wherever it needs a raw authority check outside the op-apply path, and re-exports `MemberPathAtCut` variants into its own `AtCutMembershipPath`.
- `calimero-governance-store` is a separate, legacy governance path; it depends on `calimero-op` (for `unified_op_decode`) but not on `calimero-authz` - it is not part of the unified-log authorization flow this crate guards.

## JIT Index

```bash
# Find the authorization fold itself
rg -n "pub fn authorize" src/lib.rs

# Find OpPayload's variants (defined in calimero-op, matched here)
rg -n "pub enum OpPayload" -A45 ../op/src/lib.rs

# Find OpMask's bit definitions
rg -n "impl OpMask" -A40 ../storage/src/entities.rs

# Find the AclView producer (the only place a real AclView is built)
rg -n "fn acl_view_at" ../projection/src/lib.rs

# Find every direct AclView method call outside this crate
rg -n "calimero_authz::" ../context/src/
```

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Everything: `AclView`, `MemberPathAtCut`, `SubgroupEdge`, `Rejected`, `authorize`, `required_mask_for`, and all tests |

## Invariants and Gotchas

- **`required_mask_for` returns `Option`, not `OpMask::NONE`, on purpose**: the empty mask is contained by *every* mask, so a `NONE` requirement fed into `AclView::may` would authorize anyone. `None` makes that misuse a type error instead of a silent bypass. `authorize` itself doesn't call `required_mask_for` - each data arm inlines its literal mask so there's no `Option::unwrap` that could panic or silently fall through if the arms ever drift; the function stays as the public helper for external callers.
- **`DEFAULT_MEMBER_MASK` excludes `ADMIN` deliberately**: any scope member can `Put`/`Delete` a non-restricted entity, but rotating its writer set (`SetWriters`) always requires an explicit ownership grant. A single compromised member can wipe default data but can't lock others out of it.
- **Ownership == holding `OpMask::ADMIN`** (`is_owner` is literally `may(author, object, ADMIN)`). If owner ever needs to diverge from admin/writer capability, this is the one place to change.
- **The inheritance walk is at-cut, not live**: a membership the cut revokes is not granted, even if the revocation is later in wall-clock time than when the op was authored - this is the whole reason the walk is duplicated here instead of reusing a live-store membership check. See the `inherited_membership_requires_open_chain_and_cap` test's "THE over-auth case" for the scenario this guards against.
- **`is_member_at_cut` vs `is_authorized_admin` vs `member_path_at_cut` walk order differs by necessity**: `member_path_at_cut` checks the direct row before the admin carve-out (so a stored role wins when an identity is both a stored member and the genesis admin, matching live's `list` semantics); `is_member_at_cut` only needs a bool so its order doesn't matter. Don't unify them without re-checking both call sites.
- **`root: Option<(ContextGroupId, PublicKey)>` is the one un-folded fact**: the namespace's genesis admin has no governance op (it's set at backfill), so every membership/admin function takes it as an explicit out-of-band parameter rather than expecting it in `AclView`.
- **An open-subgroup self-join (`MemberJoinedOpen`) is never folded as a direct row**: it's deliberately re-derived by the inheritance walk each time, so removing the anchor ancestor correctly revokes it - folding it as a direct membership would make it survive the anchor's removal.
- **Restricted edges are a hard wall**: hitting one anywhere in the walk stops it immediately, even if an admin sits further up the chain past the wall.

Part of [crates/](../AGENTS.md).
