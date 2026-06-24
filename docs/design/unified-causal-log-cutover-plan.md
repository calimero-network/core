# Unified causal log — cutover plan

Status: **blueprint**. The additive foundation is merged in #2775 (the
`op`/`authz`/`projection`/`op-adapter` crates, the scope-isolation property
harness, op coverage, and both halves of the data-write decision proven
fold-equivalent — see `unified-causal-log-p5-decisions.md`). This document
sequences the **cutover**: replacing the three separate per-context op stores
(data DAG, governance DAGs, rotation log) and the `state_hash`/`root_hash`
convergence signal with one op-log + `ScopeState` projection + `scope_root`,
then deleting the old folds.

> **Note on references.** This plan names files by **path + stable symbol**
> (struct/function), not line numbers — the codebase is actively refactored
> between slices, so line numbers would be stale by the time each slice lands.
> Verify the symbol against `HEAD` when starting a slice.

## Ground rules

- **Flag-day, no backwards compat.** A coordinated redeploy; nodes re-bootstrap
  and re-sync. No migration tool, no re-projection. The on-disk format already
  broke in #2745, so re-bootstrap is already required. The operator-facing "wipe
  + re-sync" path reuses the existing `resync_context` admin endpoint
  (`crates/server/src/admin/handlers/context/resync_context.rs`, exposed via the
  client in `crates/context/primitives/src/client/mod.rs`) rather than a bespoke
  one. (This is the §9.7/§9.8 decision in `unified-causal-log-p5-decisions.md`,
  recorded 2026-06-12.)
- **Each behavioral slice is its own PR, gated on a divergence-zero e2e run**
  (merobox / scaffolding-e2e / Sync-regression), run by the maintainer.
- **Author + unit-test in this repo; the maintainer runs CI/e2e.** Slices are
  ordered so each compiles, passes unit tests, and is independently reviewable.
  (This division of labor for the cutover PR series was agreed 2026-06-17; it is
  about *who runs what*, separate from the design decisions above.)

## Why the order is "augment the signal → unify the store → cut the decision → delete"

The security property (a hash-neutral ACL/membership rotation can't be hidden)
is delivered the moment the convergence signal folds ACL + membership in —
**before** the store is unified. So C1 (the `scope_root` signal) ships the
security win early and low-risk. The store unification (C2/C3) is mechanical
plumbing once the projection is authoritative for the signal. The decision cut
(C4) and deletion (C5) are last, when nothing else reads the old folds.

---

## C0 — `scope_root` shadow (pre-C1 de-risk, observe-only)

**Status: in progress (2026-06-24).** A de-risking precursor to C1, chosen over
flipping straight to the new signal — same shadow→flip discipline that carried
F4b/F5. C0 changes **no sync decision**: it computes `scope_root` alongside the
existing `root_hash`, exchanges it, and **logs** when the two signals would
*disagree about convergence* — i.e. `entities_root` (the current `root_hash`)
agrees between two peers but `scope_root` differs. That disagreement is exactly
the hash-neutral ACL/membership rotation the current signal is blind to (the
root cause behind the rotation split-brain family: stale-heads, clear()
tombstone-blindness, #2607 verified-but-divergent). C0 proves the new signal
catches it in real e2e *before* C1 makes it load-bearing.

**Where.** Reuse the #2607 end-of-session convergence re-query in
`crates/node/src/sync/hash_comparison_protocol.rs` (`query_peer_current_root` →
`DagHeadsResponse`). The responder additionally computes and returns its
`scope_root`; the initiator computes its own and compares. **`root_hash` /
`root_hash_verified` keep driving every decision** — `scope_root` is logged, never
acted on.

**Computing a context's `scope_root`.** `entities_root` = the existing storage
Merkle `root_hash` for the context (unchanged, `get_local_root_hash_for_context`
/ `DagHeadsResponse.root_hash`). The ACL + governance come from the **maintained
projection**: resolve `context → group → namespace` (`get_group_for_context` +
`NamespaceRepository::resolve`) to the namespace `ScopeId`, then
`ScopeProjections::scope_root_for(scope, entities_root)` (new, thin) returns
`states.get(scope).map(|s| s.scope_root_with_entities(entities_root))`. `None`
(scope not folded yet) ⇒ skip the shadow comparison for that session — never a
false divergence on a cold projection.

**Wire.** Add `scope_root: Option<Hash>` to `DagHeadsResponse`
(`crates/node/primitives/src/sync/wire.rs`). `Option` so a node that can't
resolve/fold the scope sends `None` and the initiator skips — no false signal.
Safe under the flag-day rule (all nodes redeploy together; e2e runs one build),
and it is the **same field C1 promotes** to the authoritative compare, so the
throwaway is one `Option` unwrap.

**Marker.** Log `scope_root_shadow_divergence` (gate marker, like the F5 planes)
at WARN when `local_entities_root == peer_entities_root && local_scope_root !=
peer_scope_root` (the blind spot), and a quieter `debug!` for the inverse
(`entities` differ — already caught by `root_hash`, expected mid-sync). The e2e
divergence gate greps the WARN marker; a hit on a *converged-entities* scenario
is the proof C0 is looking for, and on a *should-converge* scenario it's a real
bug the old signal hid.

**e2e gate.** A **hash-neutral-rotation canary**: two nodes reach an identical
`entities_root`, one rotates its writer set; assert the shadow fires
`scope_root_shadow_divergence` until the rotation propagates and `acl_hash`
agrees, then stops. Plus: existing concurrent-rotation/governance scenarios show
**zero** shadow divergence at steady state (no false positives). C1 promotes the
signal only once this canary is green.

## C1 — `scope_root`: fold ACL + groups into the convergence signal

**Goal.** Replace the bare entity `root_hash` on the wire and in comparison with
`scope_root`, where

```
scope_root = SHA-256( entities_root ‖ acl_hash ‖ governance_hash )
```

— where each input is a fixed **32-byte** value (`entities_root` is the storage
Merkle root; `acl_hash` and `governance_hash` are SHA-256 outputs). The plain
concatenation is collision-free **only because all three are fixed-width**; this
invariant is load-bearing (see the risk register). Implemented by
`calimero_op::scope_root` and `ScopeState::scope_root_with_entities` (already on
master from #2775).
`entities_root` stays the **existing, proven storage Merkle `root_hash`** — we
do not re-hash entities. `acl_hash` and `governance_hash` (membership + admin +
policy + live subgroups) come from the projection.

This is the kernel security win: a divergent writer set or membership becomes a
divergent root, so sync can never declare "done" while authorization disagrees.

**Touch points (verify symbols against HEAD):**
- Compute: `ScopeState::scope_root_with_entities` is the combiner (already
  landed). C1 makes the projection's `acl_hash`/`governance_hash` available at
  the sync sites — either from a live `ScopeState` (if C2 lands first) or, as
  interim glue, derived on demand from the existing governance store + rotation
  logs.
- Ship: the `root_hash` field on `DagHeadsResponse` carries `scope_root`
  (`crates/node/primitives/src/sync/wire.rs`); likewise the snapshot boundary
  hashes (`SnapshotStreamRequest` / `SnapshotBoundaryResponse`).
- Compare: every local-vs-peer root comparison switches to `scope_root` — the
  protocol selector, level-wise sync, hash-comparison protocol, and the
  post-sync reconciler (all under `crates/node/src/sync/`).
- The HC entity tree-walk itself is unchanged (it still reconciles entities by
  the storage Merkle); only the *top-level convergence decision* uses
  `scope_root`. Divergence localized to acl/groups (equal `entities_root`,
  different `scope_root`) routes to a governance/ACL pull, not an entity walk.

**e2e gate.** Concurrent-rotation + governance scenarios still converge, **and** a
new **hash-neutral-rotation canary** proves a divergent ACL can no longer hide:
add (or extend) a `scaffolding-e2e` scenario where two nodes reach an identical
`entities_root` but a node performs a writer-set rotation, and assert sync stays
active (does not declare "converged") until the rotation has propagated and the
`acl_hash` agrees. This subsumes the #2607 "verified-but-divergent" guard for the
ACL/membership dimension.

## C2 — one `DagStore<Op>` per context (the unified store)

**Goal.** Collapse `DagStore<Vec<Action>>` + `DagStore<SignedGroupOp>` +
`DagStore<SignedNamespaceOp>` + the rotation log into a single `DagStore<Op>` per
context, applied by one `UnifiedApplier` that folds each op into `ScopeState`
(data → storage apply as today; acl/membership/admin → projection state). `Op` is
the `calimero-op` envelope; the `op-adapter` encoders become the *construction*
path (local ops) and the wire decode path (remote ops).

**Touch points (symbols, in `crates/node/src/delta_store.rs` and
`crates/context/src/governance_dag.rs`):**
- New `UnifiedApplier: DeltaApplier<Op>` replacing `ContextStorageApplier`
  (delta_store), `GroupGovernanceApplier` + `NamespaceGovernanceApplier`
  (governance_dag). Data ops still drive `__calimero_sync_next`; acl/membership/
  admin ops drive `ScopeState::apply` plus the storage rotation / membership
  writes that remain authoritative until C5.
- Persistence: one keyspace for `Op` rows keyed `[ContextId ‖ OpId]` (repurpose
  `Column::Delta`). `dag_heads` (in `Column::Config`) now tracks the unified DAG.
  From this slice on, the old governance-op and rotation keyspaces are **no
  longer written** *and no longer read* (C2+ code reads only the unified op-log);
  their on-disk **column removal** is deferred to C5 (with the code that
  references them). So stale old-format rows left by pre-C2 code are **inert** —
  never read, just dead bytes until C5 drops the column. A node that missed the
  flag-day redeploy (e.g. offline during it) has *no* unified op-log and must
  run `resync_context` to rebuild it before it can participate; its stale old
  columns are ignored, not interpreted. There is no path where C2+ code mixes
  old-column data with the unified op-log.
- `load_persisted_deltas` / `persist_cascaded_deltas_and_update_heads`
  (delta_store) operate on `Op`.

**e2e gate.** Full lifecycle (create context, add/remove members, rotate writers,
concurrent writes) converges; partial-replication isolation holds (a non-member
never receives a scope's ops).

## C3 — projection-authoritative reads

**Goal.** Reads that today hit the three stores (writer resolution, membership
status, admin/policy) now read `ScopeState::acl_view_at(parents)`. The old
resolvers (`rotation_log::resolve_local`, governance `acl_view_at`,
`membership_status_at`) become thin shims over the projection, then are removed
in C5. Fold-equivalence for both halves is already proven, so this is a
mechanical swap behind the same call signatures.

**e2e gate.** This slice introduces no new *behavior*, so its gate is a
**regression check** that the swap is transparent — but it must exercise a read
path C2's scenarios don't: add a scenario where membership/writers are resolved
on a **snapshot-replay or DAG-catchup** path (not just live gossip apply), on a
2-node cluster after a concurrent rotation, and assert the projection-served
result matches what the old resolver returned pre-swap. Without that, an
identical-to-C2 gate couldn't distinguish a C3 regression from a passing C2.

## C4 — `authorize` is the decision (the cut)

**Goal.** Replace `authorize_delta_at_edge`
(`crates/node/src/handlers/state_delta/verify.rs`) + `writers_at_authenticated`
(storage) with one `authorize(op, ScopeState::acl_view_at(op.parents))` at every
apply site. This is the single security decision; the two-layer split (membership
gate + per-object writer gate) collapses into one fold.

**Coexistence with the #2763 pull-side gate.** The pull-side membership gate
added on master in #2763 remains the **live** authorization through C1–C3
(C1–C3 change the *signal* and the *store*, not the decision). C4 replaces both
it and `authorize_delta_at_edge` **atomically within this one slice** — there is
never a window where two gates run concurrently with potentially different
outcomes. If C4 is delayed, the #2763 gate simply stays live; nothing regresses.

**e2e gate (the big one):** divergence==0 across concurrent-rotation, governance
add/remove, the snapshot/HC/level paths, **and** an explicit group-remove
scenario that makes the #19 closure *verifiable*: after a group-remove op is
applied, assert that a subsequent op authored in that group's plane is **rejected
by `authorize`** (no authorless plane survives). **C4 does not merge until this
is green.**

## C5 — delete the old folds (~3,500 LOC)

Once nothing reads them, in dependency order:

1. **Persistence layer first** — drop the old on-disk keyspaces no longer written
   since C2: the `Column::Group` op-log rows and the rotation-log keyspace, plus
   the `state_hash` field on `SignedGroupOp`/`SignedNamespaceOp` and
   `compute_group_state_hash` / `snapshot_context_state_hashes`.
2. **Resolver / apply code** — `crates/storage/src/rotation_log.rs`,
   `crates/node/src/sync/rotation_log_reader.rs`,
   `crates/context/src/governance_dag.rs`, `apply_local_signed_group_op`,
   `apply_signed_namespace_op`, `membership_status_at`-as-fold.
3. **The `op-adapter` crate**: delete `crates/op-adapter`, **and** remove it from
   the workspace `members` list in the root `Cargo.toml` and from `deny.toml`
   (otherwise `cargo build` fails on a missing member).

**On losing the equivalence proofs.** The fold-equivalence tests are inherently
*transitional*: they assert "the new projection resolves the same writer set /
membership as the **old resolver**." Once C5 deletes the old resolvers
(`resolve_local`, `membership_status_at`), there is nothing left to compare
against, so those tests retire *with* the code they compare to — keeping them
would not even compile. The durable post-cutover safety net is **not** these
proofs but the **convergence + scope-isolation property harness** in
`calimero-projection` (`testing` feature), which is independent of the old
resolvers and is **not** deleted. Before deleting `op-adapter`, confirm that
harness still covers the properties the equivalence tests were guarding (it
does today: per-scope convergence + non-member isolation); if any unique case is
only in an `op-adapter` test, port it into the projection harness first.

group-remove (#19) closes here structurally.

## P6 (separate epic, after C5)

Collapse `HashComparison` / `Snapshot` / `LevelWise` / `protocol_selector` /
governance catch-up / `rotation_log_reader` into one per-scope sync engine
(head-accumulator → pull-by-ancestry → re-project; Merkle-diff + checkpoint as
strategies), per-shard + membership-gated. This surface grew with the migrations
work (chained catch-up, parent-pull short-circuit, the peer-auth gate) — re-survey
before starting.

## Risk register

- **`entities_root` ≠ projection entities hash.** Resolved by *keeping* the
  storage Merkle as `entities_root` and only folding acl/governance into
  `scope_root` (C1). The projection does not re-hash entity state. (Corollary:
  `scope_root_with_entities` must be called with the storage Merkle root, **not**
  `ScopeState`'s own entity hash — the type system can't distinguish two
  `[u8; 32]`s, so this is a documented caller contract.)
- **`scope_root` concatenation assumes fixed-width inputs.** `SHA-256(a ‖ b ‖ c)`
  is only collision-free because `a`/`b`/`c` are each exactly 32 bytes — there is
  no length prefix or separator. Any future change that makes a component
  variable-length (a truncated/extended hash, a different digest) silently breaks
  collision-resistance. Enforce the widths at the `calimero_op::scope_root`
  boundary (it takes `[u8; 32]` arguments — keep it that way) and re-check this if
  the digest ever changes.
- **Concurrent equal-HLC rotation tiebreak.** `ScopeState` uses `op_id`
  (content-addressed → identical on all nodes → deterministic convergence, proven
  by the harness). The old `resolve_local` signer-digest tiebreak dies with it in
  C5; moot under flag-day (no mixed-version window).
- **No e2e in authoring env.** Every behavioral slice (C1–C4) carries an explicit
  e2e gate run by the maintainer; unit + property tests are the authoring-time
  signal.
