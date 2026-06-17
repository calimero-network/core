# Unified causal log — cutover plan (P5 migration, core#2716)

Status: **blueprint**. The additive foundation is complete and merged/landing
in PR #2775 (crates `op`/`authz`/`projection`/`op-adapter`, the property
harness, op coverage, and both halves of the data-write decision proven
fold-equivalent — see `unified-causal-log-p5-decisions.md`). This document
sequences the **cutover**: replacing the three separate per-context op stores
(data DAG, governance DAGs, rotation log) and the `state_hash`/`root_hash`
convergence signal with one op-log + `ScopeState` projection + `scope_root`,
then deleting the old folds.

## Ground rules

- **§9.8 flag-day, no backwards compat.** A coordinated redeploy; nodes
  re-bootstrap and re-sync. No migration tool, no re-projection. The on-disk
  format already broke at #2745, so re-bootstrap is already required. Lean on
  the migrations-v2 `resync_context` client method for the operator-facing
  "wipe + re-sync" path rather than building a bespoke one.
- **Each behavioral slice is its own PR, gated on a divergence-zero e2e run**
  (merobox / scaffolding-e2e / Sync-regression), run by the maintainer. The
  cutover slice (C4) does not merge until that signal is green.
- **Author + unit-test in this repo; the maintainer runs CI/e2e** (ratified
  2026-06-17). Slices are ordered so each compiles, passes unit tests, and is
  independently reviewable.

## Why the order is "augment the signal → unify the store → cut the decision → delete"

The security property of #2716 (a hash-neutral ACL/membership rotation can't be
hidden) is delivered the moment the convergence signal folds ACL + membership
in — **before** the store is unified. So C1 (the `scope_root` signal) ships the
security win early and low-risk. The store unification (C2/C3) is mechanical
plumbing once the projection is authoritative for the signal. The decision cut
(C4) and deletion (C5) are last, when nothing else reads the old folds.

---

## C1 — `scope_root`: fold ACL + groups into the convergence signal

**Goal.** Replace the bare entity `root_hash` on the wire and in comparison
with `scope_root = H(entities_root ‖ acl_hash ‖ groups_root)` (the combiner
already in `calimero_op::scope_root`). `entities_root` stays the **existing,
proven storage Merkle `root_hash`** — we do not re-hash entities. `acl_hash`
and `groups_root` come from the projection's `AclView` / `ScopeState`.

This is the kernel security win: a divergent writer set or membership becomes a
divergent root, so sync can never declare "done" while authorization disagrees.

**Touch points (from the blast-radius survey):**
- Compute: a pure node-side `compute_scope_root(root_hash, &AclView)` helper
  (additive, unit-tested first — slice C1a, below).
- Ship: `DagHeadsResponse.root_hash` → carry `scope_root`
  (`crates/node/primitives/src/sync/wire.rs`). Also `SnapshotStreamRequest`
  /`SnapshotBoundaryResponse` boundary hashes.
- Compare: `protocol_selector.rs:~142`, `level_sync.rs:~587`,
  `hash_comparison.rs:~309`, `reconciler.rs:~388` — every local-vs-peer
  root comparison switches to `scope_root`.
- The HC entity tree-walk itself is unchanged (it still reconciles entities by
  the storage Merkle); only the *top-level convergence decision* uses
  `scope_root`. Divergence localized to acl/groups (entities_root equal but
  scope_root differs) routes to a governance/ACL pull, not an entity tree-walk.

**e2e gate:** concurrent-rotation + governance scenarios converge;
hash-neutral-rotation canary now *fails to hide* (a divergent writer set keeps
sync alive until reconciled). This subsumes the #2607 "verified-but-divergent"
guard for the ACL/membership dimension.

**C1a (verifiable now, additive, no wire change):** the pure
`compute_scope_root` helper + tests proving a hash-neutral ACL change moves the
root. Lands in PR #2775 or this branch without an e2e gate (no behavior change).

## C2 — one `DagStore<Op>` per context (the unified store)

**Goal.** Collapse `DagStore<Vec<Action>>` + `DagStore<SignedGroupOp>` +
`DagStore<SignedNamespaceOp>` + the rotation log into a single
`DagStore<Op>` per context, applied by one `UnifiedApplier` that folds each op
into `ScopeState` (data → storage apply as today; acl/membership/admin →
projection state). `Op` is the `crates/op` envelope; the per-plane encoders in
`op-adapter` become the *construction* path (local ops) and the wire decode
path (remote ops).

**Touch points:**
- New `UnifiedApplier: DeltaApplier<Op>` replacing `ContextStorageApplier`
  (`delta_store.rs:245`), `GroupGovernanceApplier` + `NamespaceGovernanceApplier`
  (`governance_dag.rs:16/67`). Data ops still drive `__calimero_sync_next`;
  acl/membership/admin ops drive `ScopeState::apply` + the storage rotation /
  membership writes that remain authoritative until C5.
- Persistence: one keyspace for `Op` rows keyed `[ContextId ‖ OpId]` (repurpose
  `Column::Delta`; retire the governance-op and rotation keyspaces). `dag_heads`
  in `Column::Config` now tracks the unified DAG.
- `load_persisted_deltas` / `persist_cascaded_deltas_and_update_heads`
  (`delta_store.rs:1260/2818`) operate on `Op`.

**e2e gate:** full lifecycle (create context, add/remove members, rotate
writers, concurrent writes) converges; partial-replication isolation holds
(Invariant 0 — non-member never receives a scope's ops).

## C3 — projection-authoritative reads

**Goal.** Reads that today hit the three stores (writer resolution, membership
status, admin/policy) now read `ScopeState::acl_view_at(parents)`. The old
resolvers (`rotation_log::resolve_local`, governance `acl_view_at`,
`membership_status_at`) become thin shims over the projection, then are removed
in C5. (Fold-equivalence for both halves is already proven, so this is a
mechanical swap behind the same call signatures.)

**e2e gate:** same scenarios as C2; writer/membership decisions unchanged.

## C4 — `authorize` is the decision (the cut)

**Goal.** Replace `authorize_delta_at_edge` (`verify.rs:60`) +
`writers_at_authenticated` (storage) with one
`authorize(op, ScopeState::acl_view_at(op.parents))` at every apply site. This
is the single security decision; the two-layer split (membership gate +
per-object writer gate) collapses into one fold. The new pull-side membership
gate added on master by #2763 is subsumed (a non-member's ops never authorize).

**e2e gate (the big one):** divergence==0 across concurrent-rotation,
governance add/remove, group-remove (closes #19 for free — no authorless
plane), and the snapshot/HC/level paths. **C4 does not merge until this is
green**, per the ratified gate.

## C5 — delete the old folds (~3,500 LOC)

Once nothing reads them:
- `crates/storage/src/rotation_log.rs`, `crates/node/src/sync/rotation_log_reader.rs`
- `crates/context/src/governance_dag.rs`, `apply_local_signed_group_op`,
  `apply_signed_namespace_op`, `membership_status_at`-as-fold
- `state_hash` field on `SignedGroupOp`/`SignedNamespaceOp` +
  `compute_group_state_hash` + `snapshot_context_state_hashes`
- the `op-adapter` crate itself (its job — proving equivalence — is done)
- `Column::Group` op rows / rotation keyspaces

group-remove (#19) closes here structurally.

## P6 (separate epic, after C5)

Collapse `HashComparison` / `Snapshot` / `LevelWise` / `protocol_selector` /
governance catch-up / `rotation_log_reader` into one per-scope sync engine
(head-accumulator → pull-by-ancestry → re-project; Merkle-diff + checkpoint as
strategies), per-shard + membership-gated (Invariant 0). The survey shows this
surface grew with migrations-v2 (chained catch-up, parent-pull short-circuit,
the peer-auth gate) — re-survey before starting.

## Risk register

- **entities_root ≠ projection entities hash.** Resolved by *keeping* the
  storage Merkle as `entities_root` and only folding acl/groups into
  `scope_root` (C1). The projection does not re-hash entity state.
- **Concurrent equal-HLC rotation tiebreak.** `ScopeState` uses `op_id`
  (content-addressed → identical on all nodes → deterministic convergence,
  proven by the harness). The old `resolve_local` signer-digest tiebreak dies
  with it in C5; moot under flag-day (no mixed-version window).
- **No e2e in authoring env.** Every behavioral slice (C1 wire-up, C2–C4)
  carries an explicit e2e gate run by the maintainer; unit + property tests are
  the authoring-time signal.
