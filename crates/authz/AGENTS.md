# calimero-authz - Unified Causal-Log Authorization

The single security boundary for the unified causal log: one fold, [`authorize`], deciding whether an op's author had authority at the op's own causal cut.

## Package Identity

- **Crate**: `calimero-authz`
- **Entry**: `src/lib.rs` (crate docs + the flat re-export facade; the decision itself lives in the modules below)
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
| `AclView::is_member_at_cut(group, author, root, default_cap_base)` | fn | Membership at the cut: direct, group-admin, or inherited over an open-subgroup chain. Defined as `member_path_at_cut(..) != None` |
| `AclView::is_authorized_admin(group, author, root)` | fn | Admin authority at the cut: group admin, root admin, or admin of an ancestor over the open chain |
| `AclView::member_path_at_cut(...)` | fn | The walk itself; returns the role-bearing `MemberPathAtCut` for enumeration callers. `is_member_at_cut` is this with the role discarded |
| `AclView::capability(group, member)` | fn | Effective capability bitmask: member override, else group default, else `0` |
| `AclView::is_owner(author, object)` | fn | Owner = holds `OpMask::ADMIN` on `object` (confers writer-set rotation rights) |
| `AclView::is_group_admin(author, group)` | fn | Folded group admin (subgroup creator / `Admin`-role holder) |
| `AclView::is_root_admin(author)` | fn | Is `author` the scope's `root_admin` at the cut |
| `MemberPathAtCut` | enum | `None` / `Direct { role }` / `Inherited { anchor, via_admin }` - how `author` reaches membership |
| `SubgroupEdge` | struct | `{ parent: ScopeId, restricted: bool }` - a live subgroup's tree position + visibility at the cut |
| `Rejected` | enum (`ThisError`) | `NotPermitted { entity, required }` / `NotOwner` / `NotGroupAdmin` / `NotRootAdmin` - one rejection type for every plane |
| `AccountBinding` | struct | `{ epoch, root_pk }` - an account's resolved root key at the cut |
| `DeviceBinding` | struct | `{ account, sign_pk, kem_pk, device_epoch, key_epoch }` - a device's binding in force at the cut |
| `AclView::admit_device_link(genesis, chain, cert)` | fn | The single definition of when a `DeviceLinked` credential takes effect; the at-cut half, shared with the projection's fold |
| `fold_device_link(devices, revoked, genesis, chain, cert)` | fn | The order-independent half of the above - every rule more folding cannot change. Stays a free function: its caller is the fold, which is *building* the state a view is read from and has no `AclView` to pass |
| `AclView::admit_key_rotation(handoff)` | fn | When an `AccountKeysRotated` handoff continues the chain in force at the cut |

`AclView::open_ancestors` is the crate-private climb all three inheritance questions share (yields the Open-reachable ancestors, nearest first, bounded by `MAX_NAMESPACE_DEPTH`, stopping at a `restricted` edge). Three constants also shape the rules above but are **crate-private**, so they are pinned by name here rather than importable: `DEFAULT_MEMBER_MASK` (`WRITE|DELETE`, what a plain member holds on a non-restricted entity), `CAN_JOIN_OPEN_SUBGROUPS` (mirrors `MemberCapabilities`, gates inherited membership), `MAX_NAMESPACE_DEPTH` (bound on the inheritance walk, sourced from `calimero-context-config`).

## How the pieces interact

**One op, one decision.** The view arrives already resolved — this crate never
walks the DAG — and the answer is a `Result`, never a mutation:

```text
   calimero-projection                    calimero-authz
   ───────────────────                    ──────────────
   ScopeState (the folded op log)
        │ acl_view_at(op.parents)
        ▼
     AclView ─────────────────────────────▶ authorize(op, view)
     Op ──────────────────────────────────▶      │
                                                 │
                                      stage 1 ───┤ check_device_speaks_for_author
                                                 │   revoked? bound? same account?
                                                 │   same key?  (DeviceLinked and
                                                 │   MemberJoinedWithDevice skip it)
                                                 ▼
                                      stage 2 ───┤ match op.payload
                                                 ├── data ────▶ may()
                                                 ├── owner ───▶ is_owner()
                                                 ├── group ───▶ is_group_admin()
                                                 ├── root ────▶ is_root_admin()
                                                 ├── account ─▶ view.admit_device_link()
                                                 │              view.admit_key_rotation()
                                                 │
                                        Ok(()) ◀─┴─▶ Rejected
```

**The rule that is deliberately shared.** The link rule has two callers in
two crates, and that is the whole point — a rule stated twice is how one node
authorizes an op its peer folds differently, which is a `scope_root` divergence:

```text
   authorize (this crate)              projection's fold (calimero-projection)
   decides at ONE fixed cut            walks ops ONE AT A TIME
        │                                          │
        ▼                                          │
   AclView::admit_device_link                      │
        │  = fold_device_link + the supersession   │
        │    check, which needs a cut to mean      │
        ▼    anything                              ▼
   fold_device_link ◀──────────────────────────────┘
        the order-independent half: a tombstone is never removed, a device is
        never un-assigned, an epoch only rises — so any fold order agrees

   Supersession stays out of the fold on purpose: against a partial fold it would
   read whichever epoch happened to arrive first, making admission depend on
   delivery order. The fold records the link and filters superseded ones when the
   view is read, once the final epoch is known.
```

**Module map.** Dependencies run one way, and the arrows are exactly the
`use crate::` graph:

```text
   authorize.rs ──▶ admission.rs ──┐
        │                          ├──▶ view.rs        (AclView + flat predicates)
        └──────────────────────────┘         ▲
                                             │ second `impl AclView`
                             inheritance.rs ─┘         (the subgroup-tree walk)

   error.rs ◀── every fallible path in all of the above returns Rejected
```

`inheritance.rs` adds a second `impl AclView` block rather than living in
`view.rs`: the three walking questions are ~200 lines that share one loop shape
and differ only in what counts as success, and their differences are load-bearing
(see the Invariants below). Nothing else in the crate depends on it — `authorize`
never asks the inheritance questions itself; `crates/context` is what calls them.

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
- `crates/context` (`scope_projection.rs`, `apply_authorizer.rs`) is the consumer: it filters its candidate set with `is_member_at_cut` and then resolves each survivor's role with `member_path_at_cut`, so those two agreeing is load-bearing - which is why one is now defined in terms of the other. It calls `AclView::is_authorized_admin` / `is_member_at_cut` / `member_path_at_cut` directly (bypassing the `authorize` top-level match) wherever it needs a raw authority check outside the op-apply path, and re-exports `MemberPathAtCut` variants into its own `AtCutMembershipPath`.
- `calimero-governance-store` is a separate, legacy governance path; it depends on `calimero-op` (for `unified_op_decode`) but not on `calimero-authz` - it is not part of the unified-log authorization flow this crate guards.

## JIT Index

```bash
# Find the authorization fold itself
rg -n "pub fn authorize" src/authorize.rs

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

Every public item is re-exported flat from `src/lib.rs`, so `calimero_authz::AclView` works regardless of which module it lives in - the modules are private and exist for readability, not as API surface.

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Crate docs (the WHY), module declarations, and the flat `pub use` facade |
| `src/authorize.rs` | `authorize`, `required_mask_for`, `check_data`, `check_op_is_the_certified_device` (the possession half both credential arms share), and the `check_device_speaks_for_author` precondition |
| `src/view.rs` | `AclView` + `AccountBinding` / `DeviceBinding` / `SubgroupEdge`, `DEFAULT_MEMBER_MASK`, and the single-lookup predicates |
| `src/inheritance.rs` | The shared subgroup-tree climb (`open_ancestors`) and the three questions over it: `is_member_at_cut`, `is_authorized_admin`, `member_path_at_cut`, `MemberPathAtCut` |
| `src/admission.rs` | `AclView::admit_device_link`, `AclView::admit_key_rotation`, `fold_device_link` - the rules shared with the projection's fold |
| `src/error.rs` | `Rejected` |
| `src/tests.rs` | Declares the test tree; every test in the crate lives under `src/tests/` |
| `src/tests/<module>.rs` | Tests for the module of the same name, reaching it through `crate::` paths |
| `src/tests/support.rs` | Shared fixtures (`op_with`, `bind_account`, `bind_test_devices`, `view_with_writer`, `inheritance_view`, `membership_view`) |
| `src/tests/inheritance_climb.rs` | The pre-refactor walks, kept verbatim as oracles, swept over every small tree against the shared climb |

## Invariants and Gotchas

- **`required_mask_for` returns `Option`, not `OpMask::NONE`, on purpose**: the empty mask is contained by *every* mask, so a `NONE` requirement fed into `AclView::may` would authorize anyone. `None` makes that misuse a type error instead of a silent bypass. `authorize` itself doesn't call `required_mask_for` - each data arm inlines its literal mask so there's no `Option::unwrap` that could panic or silently fall through if the arms ever drift. That leaves the payload→mask mapping written twice, so `required_mask_for_agrees_with_the_mask_authorize_enforces` pins the two together by comparing the helper's answer against the mask `authorize` reports refusing on. The helper currently has **no callers outside this crate**; the drift test is what earns it its keep.
- **`DEFAULT_MEMBER_MASK` excludes `ADMIN` deliberately**: any scope member can `Put`/`Delete` a non-restricted entity, but rotating its writer set (`SetWriters`) always requires an explicit ownership grant. A single compromised member can wipe default data but can't lock others out of it.
- **Ownership == holding `OpMask::ADMIN`** (`is_owner` is literally `may(author, object, ADMIN)`). If owner ever needs to diverge from admin/writer capability, this is the one place to change.
- **The inheritance walk is at-cut, not live**: a membership the cut revokes is not granted, even if the revocation is later in wall-clock time than when the op was authored - this is the whole reason the walk is duplicated here instead of reusing a live-store membership check. See the `inherited_membership_requires_open_chain_and_cap` test's "THE over-auth case" for the scenario this guards against.
- **The three inheritance questions share one climb, and only their success conditions differ**: `AclView::open_ancestors` is the loop; `member_path_at_cut` checks the direct row before the admin carve-out (so a stored role wins when an identity is both a stored member and the genesis admin, matching live's `list` semantics), and `is_member_at_cut` is now literally that walk with the role discarded. The order that once differed between them only ever decided which *role* was reported, never *whether* the author was a member - `src/tests/inheritance_climb.rs` keeps the pre-refactor walks as independent oracles and sweeps every small tree to hold that. Do not refactor those oracles to share code with the crate; a duplicate is the whole point of an oracle.
- **`is_authorized_admin` is the one place the GLOBAL root admin counts**: admin *authority* reaches every group, but membership does not - the root admin is a member of a subgroup only over the open chain, never of a Restricted one. `is_membership_admin` is therefore deliberately narrower than the admin predicate `is_authorized_admin` builds.
- **"Account unknown" and "wrong epoch" are separate rotation refusals**: `RotationAccountUnknown` means nothing has linked a device of the account into this scope, `RotationNotContinuous { expected, found }` means it is known and the handoff starts at the wrong epoch. They shared one variant until they were split; `calimero-governance-store`'s `BindingRejected::RotationNotContinuous` still conflates the same two causes on the legacy governance path.
- **`root: Option<(ContextGroupId, PublicKey)>` is the one un-folded fact**: the namespace's genesis admin has no governance op (it's set at backfill), so every membership/admin function takes it as an explicit out-of-band parameter rather than expecting it in `AclView`.
- **An open-subgroup self-join (`MemberJoinedOpen`) is never folded as a direct row**: it's deliberately re-derived by the inheritance walk each time, so removing the anchor ancestor correctly revokes it - folding it as a direct membership would make it survive the anchor's removal.
- **Restricted edges are a hard wall**: hitting one anywhere in the walk stops it immediately, even if an admin sits further up the chain past the wall.

Part of [crates/](../AGENTS.md).
