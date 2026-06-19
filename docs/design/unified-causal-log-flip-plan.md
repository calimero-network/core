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

## F3.5 — namespace-wide governance resolution (prerequisite for F4)
The shadow-compare surfaced TWO problems, the second deeper than it first looked.

**1. The encrypted `GroupOp` plane wasn't folded.** Admin-push `MemberAdded` /
`MemberRemoved` / `MemberRoleSet` / `MemberJoinedViaTeeAttestation` ride the
namespace DAG encrypted, so the projection couldn't see admin-added members while
the live decision (which decrypts via the keyring) authorized them.

**2. The bigger one — the per-scope log fragmented the namespace-wide cut.** The
live system keeps ONE governance DAG per namespace, interleaving namespace-root
ops and every group's membership ops on a single parent chain, and a data write
cites namespace-wide `governance_dag_heads`. The projection keyed its op-log
per-group, so `acl_view_at(group_scope, heads)` truncated the ancestry walk at
the first node that wasn't in that group's log (a namespace-root op, another
group's op, or an unmodeled op) — orphaning every membership op behind it. Even
a member couldn't see *itself* once any later cross-scope op became the head.

The fix addresses both at the root:
- **namespace-scoped governance** — all of a namespace's governance ops fold into
  one log keyed by `ScopeId::from(namespace_id)`; membership for a group is read
  from the folded view's `groups[group]`. `member_at_cut` resolves the namespace
  and walks the whole namespace ancestry (mirroring the live resolver's
  `prefix_walk_membership`), so the cut never truncates.
- **graph-complete ancestry** — EVERY namespace op becomes a node, even ones the
  projection doesn't model (out-of-model Root ops, undecryptable group ops): they
  fold as `OpPayload::Noop` but keep the parent chain unbroken so the walk can
  pass through them. `op_from_namespace_op` always returns a node.
- **encrypted plane folded** — the namespace apply handler and the backfill walk
  both decrypt an applied/persisted `NamespaceOp::Group` (read-only, via
  `decrypt_group_op` — no re-run of the mutation) and fold its membership variant
  at the carrying namespace delta's id/parents. Backfill decrypts the ops for
  groups this node belongs to, so cold-start and the op-emitting originator are
  covered once the namespace is (re)walked.

Residual (narrow): an originator that emits a *new* group op AFTER its namespace
was already backfilled won't live-fold it (its own ops don't pass through the
namespace apply handler, and backfill runs once). Re-walk-on-new-head or a
post-publish feed closes it; tracked here. Until divergence is zero across e2e,
the divergence step stays **informational**; it flips back to a hard gate as the
last step here.

## F4 — the flip (gated: do not merge until divergence==0 e2e)
Once divergence reached zero across e2e (the hard gate), the flip lands in two
sub-steps so the security boundary moves under continuous validation rather than
in one leap.

**F4a — projection as a load-bearing co-authorizer (DONE).** At the data-write
edge's `Authorized` arm, the projection's `member_at_cut` must now CONCUR: a
`Some(false)` DENIES the write (rejects) instead of merely logging the divergence
marker. This only ANDs with the live authorize, so it can never grant a write
live rejected (the still-unvalidated permissive direction) — it cannot
over-authorize. It can only additionally reject, and with forward divergence at
zero a wrong denial would both fail an e2e convergence scenario and trip the hard
gate. `None` (no projection answer) defers to live. RwLock makes this a cheap
concurrent read; the prior `Mutex` + an `if let`-scrutinee lock self-deadlocked
(fixed).

**F4b — sole authority (next).** Validate the inverse/permissive direction
(projection authorizing where live rejects) with a reject-arm cross-check — now
safe as a refresh-free RwLock read — and, once zero, replace
`authorize_delta_at_edge` + `writers_at_authenticated` outright with
`authorize(op, projection.acl_view_at(parents))` as the single decision. Subsumes
the #2763 pull-side membership gate.

## F5 — delete the old folds (~3,500 LOC), after F4 soaks
`rotation_log.rs`/`rotation_log_reader.rs`, `governance_dag.rs`,
`apply_local_signed_group_op`/`apply_signed_namespace_op`-as-fold,
`membership_status_at`, the `state_hash` field + `compute_group_state_hash`, and
the `op-adapter` crate (its job — proving equivalence — is done). Persistence
columns retired. group-remove (#19) closes here structurally. The durable
post-cutover safety net is the projection's convergence + isolation property
harness, not the (now-deleted) equivalence proofs.

See `unified-causal-log-cutover-plan.md` for the full rationale.
