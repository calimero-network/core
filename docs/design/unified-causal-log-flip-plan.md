# Unified causal log — the cutover flip (do NOT merge until e2e-green)

This branch carries the **decision flip**: making the unified-op projection the
authoritative answer to "is this author permitted to write at its governance
cut", then deleting the old gates. It is the security boundary, it is flag-day,
and per the cutover plan it **must not merge until a divergence-zero e2e run is
green** (a merobox/scaffolding run the maintainer executes — it cannot be run in
the authoring environment).

The safe, additive groundwork is already on master / in #2783:
- the `op`/`authz`/`projection`/`op-adapter` crates + isolation harness (#2775);
- the live per-scope projection fed from governance + raw rotations, the
  feed-verification shadow-compare, and **op-log retention + causal-cut
  `acl_view_at`** (#2783).

What remains here, in order, each its own commit and the last two gated:

## F1 — bridge: share the projection node↔context (additive)
The projection lives in `ContextManager` (context crate); the data-write
decision is orchestrated in `authorize_delta_at_edge` (`node`/verify.rs). Share
one `Arc<Mutex<ScopeProjections>>`:
- `run.rs` creates it, passes it to `ContextManager::with_scope_projections(..)`
  (a new builder that replaces the internally-created registry) **and** stores
  it on `NodeState`.
- The governance/ACL feeds then populate the shared registry; the node decision
  site can read it.
Pure plumbing, nothing decides against it yet — zero behavioral risk.

## F2 — persistence + startup backfill (prerequisite, additive)
The projection is in-memory and only fills from feeds since process start, so a
just-restarted node has an empty projection. Authorizing against an empty
projection = deny = broken. Before the flip, the op-log must be **persisted**
and **rebuilt on startup** (replay the retained ops, like `load_persisted_deltas`
seeds the data DAG). Until F2, the projection can only be a *shadow* (act on the
live decision), never authoritative.

## F3 — decision-site shadow-compare (additive, acts on live)
At `authorize_delta_at_edge`, also compute the projection's verdict via
`ScopeProjections::acl_view_at(scope, governance_dag_heads)` +
`calimero_authz::authorize`, and compare to the live `acl_view_at`/`writers_at`
decision behind the `unified_projection_divergence` marker. **Still act on the
live decision.** This is the literal authorize-vs-live compare; its e2e output
(zero divergence) is the gate for F4.

## F3.5 — feed the encrypted membership plane (prerequisite for F4)
The shadow-compare surfaced the one membership source the projection does not
yet fold: the **encrypted `GroupOp` plane** (admin-push `MemberAdded`,
`MemberRemoved`, `MemberRoleSet`, `MemberJoinedViaTeeAttestation`,
`TransferOwnership`, …). Those ops ride the namespace DAG encrypted, so neither
the post-apply Store read nor the startup backfill can decrypt them — the
projection's op-fold misses every admin-added member, and the live decision
(which decrypts via the keyring) authorizes them. The shadow only stops
false-flagging these because `member_at_cut` currently honors genesis-admin and
inherited carve-outs; admin-push members are a *real* op-fold gap, not a
carve-out.

Closing it has three parts:
- **received live feed** — ✅ done. The namespace apply handler decrypts an
  applied `NamespaceOp::Group` (read-only, via `decrypt_group_op` — no re-run of
  the mutation) and folds the cleartext `GroupOp` membership variant via
  `op_from_group_op`, at the **carrying namespace delta's** id/hlc/parents so the
  op lands on the DAG node a `governance_dag_heads` cut names. Covers every
  non-originating node (in `frozen-rga`, node-2 + node-3).
- **originator local feed** — the node that *emits* an admin-push op applies the
  cleartext `GroupOp` locally (via `sign_apply_and_publish`) without going
  through the decrypt path, so its own projection doesn't see the member. Feed it
  from the group-op-emitting context handlers (or a shared post-publish hook),
  using the published namespace op's `op_hash` as the id for alignment.
- **cold/backfill** — a restarted node re-reads encrypted ops it cannot decrypt,
  so backfill must seed the encrypted plane from the **materialized** membership
  (the `GroupMember` rows the live applier wrote), mirroring the live resolver's
  own `heads_equal` fast-path. At-cut historical resolution for encrypted ops is
  then approximate in exactly the way the live resolver's materialized path is.

Until all three land, the e2e divergence step is **informational** (reports
planes + counts, does not fail); it flips back to a hard gate as the last step
here.

## F4 — the flip (gated: do not merge until divergence==0 e2e)
Replace `authorize_delta_at_edge` + `writers_at_authenticated` with the single
`authorize(op, projection.acl_view_at(parents))` decision. Subsumes the #2763
pull-side membership gate (a non-member's ops never authorize). Atomic within
the slice — no window where two gates run with different outcomes.

## F5 — delete the old folds (~3,500 LOC), after F4 soaks
`rotation_log.rs`/`rotation_log_reader.rs`, `governance_dag.rs`,
`apply_local_signed_group_op`/`apply_signed_namespace_op`-as-fold,
`membership_status_at`, the `state_hash` field + `compute_group_state_hash`, and
the `op-adapter` crate (its job — proving equivalence — is done). Persistence
columns retired. group-remove (#19) closes here structurally. The durable
post-cutover safety net is the projection's convergence + isolation property
harness, not the (now-deleted) equivalence proofs.

See `unified-causal-log-cutover-plan.md` for the full rationale.
