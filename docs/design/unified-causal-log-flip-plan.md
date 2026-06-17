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
