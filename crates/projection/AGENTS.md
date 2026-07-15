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
- **A reparent/visibility-set op asserts existence.** `SubgroupReparented` and `SubgroupVisibilitySet` both LWW-set `exists = true` on the target slot, so a mutation that folds before its `SubgroupCreated` doesn't transiently hide a live subgroup. A later `SubgroupDeleted` still wins by its higher stamp - the assertion only fills the create gap, it never resurrects a deletion.
- **`scope_root_with_entities` vs `root`: do not swap the entities root.** `entities_root` passed in MUST be the storage layer's Merkle root, not this projection's own `entities_hash()` - they are different hash functions over different structures, and the type system can't distinguish two `[u8; 32]`s. Passing the wrong one produces a valid-looking but semantically wrong root. Use `root()` when you want the projection's own entity hash end to end; use `scope_root_with_entities` only to fold authorization onto the storage layer's root.
- **`OpPayload::Noop` folds to nothing.** It exists purely so an ancestry walk can traverse through a graph-only node (e.g. an op this replica can't decrypt) to reach ops behind it.
- **Role bytes are explicit, not the enum discriminant** (`role_byte`), so the governance root stays invariant across a refactor that reorders `GroupMemberRole` variants.
- **Property-test the fold with `testing::assert_converges_and_isolates`**, not ad hoc unit assertions, when changing anything in the fold or in how ops are delivered: it re-checks both convergence (same op-set, any order, same root) and isolation (a non-member never computes a root for a scope it wasn't delivered) over randomized workloads and delivery orders.

Part of [crates/](../AGENTS.md).
