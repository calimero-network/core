# calimero-projection - Deterministic ScopeState Projection

Folds a scope's unified op-log into `ScopeState` - the one materialized view of values, ACL, and group membership, and the one convergence root.

## Package Identity

- **Crate**: `calimero-projection`
- **Entry**: `src/lib.rs`
- **Key deps**: `calimero-op` (`Op`/`OpPayload`/`ScopeId`/`scope_root` - the log this crate folds), `calimero-authz` (`AclView`/`SubgroupEdge` - the authorization view this crate produces), `calimero-context-config` (`ContextGroupId`), `calimero-storage` (`Id`, `OpMask`, `HybridTimestamp`), `sha2` (root hashing)
- **Feature**: `testing` - exposes the `testing` module (convergence + isolation property harness) for reuse outside this crate's own tests; also auto-enabled under `#[cfg(test)]`

## Commands

```bash
# Build
cargo build -p calimero-projection

# Test (default feature set)
cargo test -p calimero-projection

# Test including the testing-harness module
cargo test -p calimero-projection --all-features

# Test a single case
cargo test -p calimero-projection projection_is_order_independent -- --nocapture
```

## Mental Model: One Fold, One Root

A **scope** (`calimero_op::ScopeId`) is a replication + convergence domain: a context, a subgroup, the root governance scope. Every change inside it - a data write, a writer-set rotation, a membership change, an admin/policy/subgroup-tree edit - is one `Op` in that scope's causal log (see `calimero-op`). `ScopeState` is the single deterministic projection of that log: fold every op, in any order, deduped by id, and you get the same state and the same `ScopeState::root()`.

Determinism comes from per-slot **last-writer-wins** keyed on a `Stamp = (HybridTimestamp, generation, op_id)`, compared as a tuple:
- `hlc` dominates - orders ops that carry a real clock (data + ACL planes).
- `generation` breaks ties when `hlc` is equal, which is *always* the case for the governance plane (its ops are stamped `hlc = 0`). It is the op's causal depth (`1 + max(parent generation)`) within the cut being resolved, so a causally-later op (e.g. a re-add after a remove) wins instead of losing to an arbitrary content-hash tie-break.
- `op_id` is the final tie-break for genuinely concurrent ops (equal `hlc` and `generation`), so every node picks the same winner regardless of arrival order.

Two ways to fold, for two different purposes:
- **`apply` / `from_ops`** (streaming, no ancestry context) stamps every op with `generation = 0`. Convergent and order-independent, but not causally authoritative for governance - use it as a sync convergence signal, never as the authorization answer.
- **`acl_view_at(log, parents)`** (cut-aware) walks the causal ancestry of `parents`, computes real per-op generations, and folds only that ancestry. This is the **causal-honor** view `calimero_authz::authorize` decides against: a pre-revocation write resolves against the pre-revocation ACL even on a node that already applied the revocation.

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `ScopeState` | struct (`Clone, Debug, Default`) | The projection: values, ACL, group membership, admin/policy/subgroups, capabilities |
| `ScopeState::from_ops(ops)` | fn | Fold a set of ops into a fresh state (order-independent) |
| `ScopeState::apply(op)` | fn | Streaming fold of one op at `generation = 0` |
| `ScopeState::apply_with_generation(op, generation)` | fn | Fold one op with an explicit causal generation |
| `ScopeState::acl_view()` | fn | Current `AclView` (whole state, generation-0 semantics) |
| `ScopeState::acl_view_at(log, parents)` | fn (assoc) | Causal-honor `AclView` at the cut named by `parents`, folding only their transitive ancestry in `log` |
| `ScopeState::cut_ancestry_complete(log, parents)` | fn (assoc) | `true` iff `log` contains the *complete* ancestry of `parents` - the over-grant guard for `acl_view_at` |
| `ScopeState::root()` | fn | `scope_root(entities_hash, acl_hash, governance_hash)` - the whole-projection convergence root |
| `ScopeState::scope_root_with_entities(entities_root)` | fn | Same root, but with an externally supplied (storage-layer Merkle) entities root |
| `testing::ReplicaView` | struct (feature `testing`) | One replica's `member_of` set + per-scope roots, for the property harness |
| `testing::simulate(seed, membership, ops)` | fn | Partial-replication delivery simulation: each replica folds only its member scopes, in a seeded-shuffled order |
| `testing::check(views)` | fn | Checks convergence + isolation over a simulation result, `Err` naming the first violation |
| `testing::assert_converges_and_isolates(seed, membership, ops)` | fn | `simulate` + `check`, panicking on violation - the one-call property-test entry point |

`ScopeState`'s fields are all private; the only way out is `acl_view()` / `acl_view_at()` (authorization) and `root()` / `scope_root_with_entities()` (convergence hash) - there is no raw accessor for entities, ACL, or groups.

## Relationship to calimero-op / calimero-op-adapter / calimero-authz

- **calimero-op** defines the log this crate folds: `Op` (scope, parents, author, hlc, payload, signature), `OpPayload` (the append-only, exhaustively-matched enum of all four planes), and the `scope_root` combining function. `calimero-projection` computes the three component hashes (`entities_hash`, `acl_hash`, `governance_hash`) that `scope_root` combines - this crate owns the hashing, `calimero-op` only owns the combinator.
- **calimero-op-adapter** is the transitional bridge that encodes today's per-plane operation types (`Action`, `RotationLogEntry`, `GroupOp`, `RootOp`) into `OpPayload`, so the unified projection can be proven fold-equivalent with the current per-plane resolvers before those are retired.
- **calimero-authz** consumes this crate's output: `AclView` (and `SubgroupEdge`) is authz's own type, but it is only ever populated by `ScopeState::acl_view()` / `acl_view_at()`. `calimero_authz::authorize(op, acl_at_cut)` decides against that view; this crate never authorizes anything itself, and authz never walks the DAG itself.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Everything: `Stamp`, `wins`/`lww_set`, `SubgroupSlot`, `ScopeState` and all its methods, `role_byte`, and all unit tests |
| `src/testing.rs` | The convergence + scope-isolation property harness (`ReplicaView`, `simulate`, `check`, `assert_converges_and_isolates`), gated behind `#[cfg(any(test, feature = "testing"))]` |

## Invariants and Gotchas

- **`apply`/`from_ops` is convergent, not causally authoritative.** Because it stamps every op with `generation = 0`, a governance add -> remove -> re-add chain (all `hlc = 0`) tie-breaks by `op_id`, which can leave a member resolved as absent while `acl_view_at` (real generations) resolves them present. Use the streaming fold only as a sync convergence signal; use `acl_view_at` as the authorization answer.
- **`acl_view_at`'s precondition is the caller's responsibility.** `log` must contain every same-scope ancestor of `parents`. A missing ancestor is silently skipped - correct for a legitimately out-of-slice cross-scope parent edge, but a missing same-scope ancestor yields a silently truncated, possibly-stale view. The live apply path guarantees this by buffering an op until its parents are present; anything else computing an authoritative grant off a partial log must check `cut_ancestry_complete` first and defer to live (not override its reject) when it returns `false`.
- **Empty groups and dead subgroups must not perturb the root.** `MemberRemoved` drops a group's map entry once it's empty, and `governance_hash` skips empty groups and non-live subgroups - so "group never existed" and "all members removed" hash identically, and a phantom empty entry can never split two nodes that reached the same state via different op orders. The per-member LWW clock is retained regardless, so a later re-add still has to beat the removal's stamp.
- **A join is not a role write.**
  An invitation join (`MemberJoinedWithDevice` with a non-TEE role) never replaces a standing membership, because the apply skips a join by an account that already holds a row.
  `member_clock` is stamped only by adds, role changes and removals; a join takes effect only when no add stands, that is when the member is absent or its latest write is a removal the join beats, and among several such joins the earliest stamp wins.
  The fold keeps the joins that beat `member_clock` so the result is a function of the op set: a removal that arrives late still finds the join that follows it.
  No fixed-size summary does that for every history, since any join may become the earliest one above a later removal, so `member_joins` keeps three things per `(group, member)` (`MemberJoins`), each a function of the op set alone.
  `standing` is the latest `MAX_STANDING_JOINS + 1` joins above the clock: the latest, because a removal only ever spends joins from the earliest end, which makes "the latest N above the clock" the same set in every arrival order; the earliest N is not.
  `first` is the earliest join ever folded for the slot and `least` is the least role any join of the slot ever carried (`invited_rank`: `ReadOnly` below `Member` below `Admin`); both cover joins the clock already beats, and no add, removal or leave clears them.
  They cannot be "the earliest join above the clock": a removal that folds before the joins and one that folds after them would then keep different joins, and the root would split by arrival order.
  For the same reason `first` survives an add that beats it; it stops mattering then, because the clock never drops back below it.
  While no add stands the role is: `first`'s, if the clock is below it (no removal has spent the first join, so repeat joins alone never change a role, however many); else the earliest standing join's, if at most `MAX_STANDING_JOINS` stand (or absent if none does); else `least`.
  That last case is the only inexact one, and its condition is order-free: the member's latest add, role change, removal or leave in the group is a removal or a leave (a namespace leave included), its stamp is above the member's first join of the group, and more than `MAX_STANDING_JOINS` of the member's joins of the group are above it.
  `least` is at most the role of the true earliest join above the removal, so the fold can lower a role there and never raise it, and it is exact whenever every join of the member in the group carried one role.
  A join that arrives below the clock still updates `first` and `least`, so the fold reads the role again after it.
  A TEE admission folds as an add and enters neither.
  The history that stays inexact: a member who left after its first join, came back, repeated the join more than `MAX_STANDING_JOINS` times with no admin write since, and at some point joined with a lower role than the one it came back with.
  No bounded fold avoids such a case: joins folded before a leave that may land between any two of them would need every join's role kept.
  "Earliest" is causal only in `acl_view_at`, whose stamps carry causal depth; in the streaming fold clockless joins are ordered by op id, so `from_ops` can resolve the later of two chained joins (`without_a_clock_the_cut_keeps_the_first_join_and_the_stream_the_lower_id`).
  The streaming state is what `ScopeProjections::scope_root_for` hashes, and sync compares that root between peers: it agrees across nodes holding the same ops, and it is not the membership authorization reads.
  The device half of a repeat join links its device as before.
  A join is never void, so one concurrent with an admin's removal of the joiner can stand at a cut where the apply ends with the member removed.
  A TEE admission (a TEE role) still folds as a plain last-writer-wins `MemberAdded`; so does a join whose credential does not bind, which the apply refuses.
  That matches the apply where the admission meets no row or a TEE row, which re-attesting in the other mode converts.
  It does not match where the member holds a non-TEE row: the apply leaves an admin's decision alone (`admit_or_convert_tee_member`), and the fold replaces the role with the TEE one.
  Matching it would make an admission a third kind of write, kept like a join, and the `GroupOp` form of the admission reaches the fold as a bare `MemberAdded` that cannot be told from an admin's.
- **A namespace leave reaches every group of the scope.**
  `MemberLeft` is a removal from its group, and when that group is the scope's root (its id is the scope id) the fold also removes the member from every other group at the leave's stamp, as the apply deletes a namespace leaver's direct row in every subgroup.
  The latest such leave of each member is kept in `namespace_left` and folded into a `(group, member)` slot as one more removal: when the leave folds, for every slot that holds a membership write or a relay seat, and ahead of any later write to a slot.
  Each slot therefore resolves over its own ops plus that removal, in any arrival order, and an add or a join stamped after the leave stands.
  No subgroup tree is consulted, because every other group of a namespace scope descends from its root.
  A leave of a subgroup reaches no other group, and neither does an admin's `MemberRemoved` of the root.
  That last one matches the apply for an ordinary member and not for a TEE: the apply cascades a root TEE eviction onto the subgroups, and the fold does not.
  A capability grant in a group where the member holds no membership write and no seat is not ended by the leave.
- **A reparent/visibility-set op asserts existence.** `SubgroupReparented` and `SubgroupVisibilitySet` both LWW-set `exists = true` on the target slot, so a mutation that folds before its `SubgroupCreated` doesn't transiently hide a live subgroup. A later `SubgroupDeleted` still wins by its higher stamp - the assertion only fills the create gap, it never resurrects a deletion.
- **`scope_root_with_entities` vs `root`: do not swap the entities root.** `entities_root` passed in MUST be the storage layer's Merkle root, not this projection's own `entities_hash()` - they are different hash functions over different structures, and the type system can't distinguish two `[u8; 32]`s. Passing the wrong one produces a valid-looking but semantically wrong root. Use `root()` when you want the projection's own entity hash end to end; use `scope_root_with_entities` only to fold authorization onto the storage layer's root.
- **`OpPayload::Noop` folds to nothing.** It exists purely so an ancestry walk can traverse through a graph-only node (e.g. an op this replica can't decrypt) to reach ops behind it.
- **Role bytes are explicit, not the enum discriminant** (`role_byte`), so the governance root stays invariant across a refactor that reorders `GroupMemberRole` variants.
- **The fold is not an authorization boundary, but some arms carry security properties anyway.** The dividing line is whether a precondition is a function of the op *alone*. The **account** plane's rules are (a genesis hashes to the id it claims, a certificate is signed by the account root, ownership is a field comparison), so they are enforced **in the fold** — and must be, because `from_ops` and the sync convergence path fold raw logs without ever calling `authorize`. A precondition enforced only in the authz layer is not a precondition: `AccountKeysRotated` used to absorb into any account's capped epoch slot, so a stranger could evict a victim's real rotation by key order and freeze their chain at a superseded root key, convergently and invisibly. Two kinds of arm are deliberate exceptions — `DeviceRevoked` (two legitimate authors, one of which is an at-cut admin question) and `DeviceDescoped` (its op-local rule is the root signature on a statement the payload does not carry, so it is checked in `calimero-op-adapter`, where the proof is, at the one point a payload can enter the log at all), plus every data/ACL/governance arm (all relational: was the author a writer/member/admin *at this cut*) — and those are safe only because the live apply path authorizes before appending. Adding an arm with an op-local precondition? Enforce it here as well as in authz; the duplication is the point. `apply_with_generation`'s docs carry the full table, and `tests/account_plane.rs::no_unauthorized_op_writes_another_accounts_plane_state` folds unauthorized ops raw and asserts no cross-account state is written.
- **A shared cell's writer set is not part of any view.** `ScopeState::shared_writer_steps(walked, group, context, cell, signer_of)` collects one cell's `SharedWritersRotated` steps from a walked cut, each with the cell's steps in its own causal past (one pass over the walk) and its signer's account from `signer_of`. It drops a step with an empty new set or a non-cell id, any step that does not rest on the cell's genesis set (from the set its id commits to, or from one another such step left), and any whose signer does not hold `ADMIN` in `prior` or has no standing; it returns `None` past `MAX_STEPS_PER_CELL` such steps, since dropping some could roll a rotation back. `calimero-context`'s `shared_writers_at_cut` checks each signer's standing and runs `calimero_storage::shared_writers::fold`; nothing else resolves it, so no reader can mistake "not computed" for "genesis stands".
- **A scope narrowing is applied when the view is read, not when it folds.** `DeviceDescoped` records a per-`(account, device)` floor and a link records the highest scope epoch it was made under; `live_devices` drops a binding at or below that floor, alongside the root-key supersession check that lives there for the same reason. Doing either at fold time would read whatever had folded so far, which is how a floor comparison becomes order-dependent.
- **A member's capability grant ends with its membership.** The fold keeps `member_caps` across a `MemberRemoved`, and the view drops any grant stamped before the member's latest removal, as the live path deletes the grant on removal. A role change keeps the grant, so the comparison is against the removal's stamp, not the member row's.
- **Void ops are excluded from every at-cut view, and from the streaming state of `ScopeProjections`.** (`ScopeState::apply`, `from_ops` and `root()` still fold every op they are given.) `ScopeState::void_ops(log, base)` (`src/void.rs`) returns the ops with no authority: an op of a signer that some removal of it (a `MemberRemoved`, a demotion to a non-admin role of an account that held `Admin`, a `DeviceRevoked`) neither precedes nor follows, where the removal reaches the op's group or an ancestor. A `MemberCapabilitySet` for another member is a capability revocation of the bits the member held at its cut and does not keep, when its author was an admin of the group there; it voids only the member's concurrent ops in that same group whose capability (`capability_by_payload`, or `Acting::capability` for an op the projection models nothing about) is among those bits, and only when the member was not an admin at the op's own cut (`was_admin`). A grant takes nothing away. Past `MAX_FOLD_WORK` a revocation takes every bit it does not keep and a victim counts as no admin. It is a function of the op set, so it is order-independent, and `CutAncestry` carries it so `acl_view_from_ancestry` folds none of those ops while still computing causal depth through them. Exempt: the namespace owner (`AuthorityBase::root`), and an op that is itself a removal of the account that removed its signer (a mutual removal; both stand). Only an effective (non-void) removal voids, which makes the set a fixed point of a rule that is not monotone. The search starts from the empty set, and when it cycles the void set is the union of the cycle (three admins each removing the next: every removal void, nobody removed). The void cascades through grants: an op is void when its signer held its authority through a grant a void op made (`MemberAdded`, `MemberCapabilitySet`, a subgroup's creating admin, `AdminChanged`) and holds none without it. `DefaultCapabilitiesSet` and structural ops by a void signer are left out of views but not cascaded from. Work is bounded (`MAX_FOLD_WORK`); past it the remaining cascade candidates are void and an unjudged device revocation counts for nothing. `void_ops_judging` also judges `held`, ops the projection models nothing about (a standalone key rotation), with the group each acted in. An op built without an author (its signer's binding was already gone) is attributed from the log's own device links. A device revocation counts as a removal only if its author was an admin of the root group at its own cut (a subgroup admin's ejection, which the apply scopes to that group, does not), because the apply logs a revocation it refused; a self-service one rests on a root-signed proof the log does not carry, so it does not count, and the check reads no device binding, which a refused link can poison. It is also voidable, and an admin removed while revoking its remover's device is removed while that device stays revoked. A role change counts as a demotion only if its author was an admin too, because the apply logs a no-op such as a TEE admitted over an existing member. Removals are judged in causal order, so a spent work bound does not follow arrival order. Not covered: an invitation signed by a removed admin, a delegated op (its author is the relay), and a removal the node holds only as an unreadable op.
- **Property-test the fold with `testing::assert_converges_and_isolates`**, not ad hoc unit assertions, when changing anything in the fold or in how ops are delivered: it re-checks both convergence (same op-set, any order, same root) and isolation (a non-member never computes a root for a scope it wasn't delivered) over randomized workloads and delivery orders.

Part of [crates/](../AGENTS.md).
