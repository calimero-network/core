use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::{ApplySignedNamespaceOpRequest, NamespaceApplyOutcome};
use calimero_context_config::types::ContextGroupId;
use calimero_dag::AddDeltaOutcome;

use crate::governance_dag::{signed_namespace_op_to_delta, NamespaceGovernanceApplier};
use crate::{ContextManager, NAMESPACE_DAG_PRUNE_RETAIN, NAMESPACE_DAG_PRUNE_THRESHOLD};

impl Handler<ApplySignedNamespaceOpRequest> for ContextManager {
    type Result = ActorResponse<Self, <ApplySignedNamespaceOpRequest as Message>::Result>;

    fn handle(
        &mut self,
        ApplySignedNamespaceOpRequest { op }: ApplySignedNamespaceOpRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let namespace_id = op.namespace_id;
        let dag = self.get_or_create_namespace_dag(namespace_id.as_bytes());
        let datastore = self.datastore.clone();
        // Separate handle for the shadow-compare (the one above is moved into
        // the applier).
        let compare_store = self.datastore.clone();

        let delta = match signed_namespace_op_to_delta(&op) {
            Ok(d) => d,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        let applier = NamespaceGovernanceApplier::new(datastore);

        // Shadow unified-op projection (additive — nothing reads it yet): fold the
        // op this governance delta represents into its namespace's projection,
        // but only if the DAG actually applies it. Capture the delta coordinates
        // (id/hlc/parents) and the signed op before the DAG call consumes `delta`;
        // EVERY applied op becomes a node (membership ops with their payload, the
        // rest as `Noop`) so the namespace-wide ancestry stays unbroken.
        let delta_id = delta.id;
        let delta_hlc = delta.hlc;
        let delta_parents = delta.parents.clone();
        let signed_op = op;
        let scope_projections = Arc::clone(&self.scope_projections);

        ActorResponse::r#async(
            async move {
                let mut dag = dag.lock().await;
                let outcome = dag.add_delta_with_outcome(delta, &applier).await;

                // Fold into the shadow projection only on a real apply, mirroring
                // the divergence-outbox gating below. A poisoned lock is ignored:
                // the projection is not yet authoritative, so it must never break
                // the governance apply path.
                if let Ok(applied) = &outcome {
                    shadow_fold_applied(
                        &compare_store,
                        &scope_projections,
                        &dag,
                        AppliedOp {
                            signed: &signed_op,
                            id: delta_id,
                            hlc: delta_hlc,
                            parents: &delta_parents,
                        },
                        applied,
                    );
                }

                // Bound this namespace's in-memory governance-DAG history. A hot
                // namespace that never gets evicted from `namespace_dags` would
                // otherwise retain every applied op for the process lifetime.
                // Done under the held DAG lock (the apply path is the only
                // writer), and only after `Applied` advanced the frontier.
                //
                // Gate on the *applied* count, not the total `delta_count()`
                // (which also counts pending deltas). `prune_to_recent` only
                // prunes applied, non-head history, so triggering off a large
                // *pending* backlog (e.g. a partition with missing parents)
                // would re-walk the whole DAG on every apply while pruning
                // nothing. Applied history advancing past the threshold is the
                // only thing this prune can actually act on; once it fires it
                // drops applied back to the retain window, so the next prune is
                // ~RETAIN..THRESHOLD applies away (a built-in hysteresis band).
                //
                // Lossless for peers: applied ops are durably persisted and the
                // backfill responder serves from RocksDB, not this DAG — so the
                // pruned ids are discarded, NOT deleted from disk.
                if matches!(outcome, Ok(AddDeltaOutcome::Applied { .. }))
                    && dag.stats().applied_deltas > NAMESPACE_DAG_PRUNE_THRESHOLD
                {
                    let pruned = dag.prune_to_recent(NAMESPACE_DAG_PRUNE_RETAIN);
                    if !pruned.is_empty() {
                        tracing::debug!(
                            namespace = ?namespace_id,
                            pruned = pruned.len(),
                            retained = dag.stats().applied_deltas,
                            "pruned applied governance-DAG history (durable op-log retained)"
                        );
                    }
                }

                // Read-and-clear the applier's divergence outbox after
                // the DAG call returns. The outbox is populated by the
                // applier's `apply` impl when `MemberRemoved` /
                // `MemberLeft` verify reports a state-hash mismatch.
                // Only meaningful on `Applied` — `Pending` / `Duplicate`
                // don't run the apply path.
                let divergence = applier.take_divergence();
                match outcome {
                    Ok(AddDeltaOutcome::Applied { .. }) => {
                        Ok(NamespaceApplyOutcome::Applied { divergence })
                    }
                    Ok(AddDeltaOutcome::Pending) => Ok(NamespaceApplyOutcome::Pending),
                    Ok(AddDeltaOutcome::Duplicate) => Ok(NamespaceApplyOutcome::Duplicate),
                    Err(e) => Err(eyre::eyre!("namespace DAG apply error: {e}")),
                }
            }
            .into_actor(self),
        )
    }
}

/// The SIGNER-authority an apply gate requires, for the apply-auth projection
/// shadow (F5 #28). `Admin` = the live `require_admin`/`require_namespace_admin`
/// gate; `AdminOrCap(bits)` = `is_authorized_with_capability` (admin OR the bit).
enum ApplyAuthReq {
    Admin,
    AdminOrCap(u32),
}

/// Map a just-applied governance op to the (group, signer-authority) the live
/// apply gate enforced — so the shadow can ask whether the projection agrees the
/// SIGNER was authorized at the op's parent cut.
///
/// Returns `None` (shadow skips) for variants whose authority is NOT the signer's
/// group-admin/capability: `MemberJoined` (an admin-signed invitation — authority
/// is the inviter's signature, not the joiner-signer), `MemberJoinedOpen` (an
/// inheritance proof on the joiner, not an admin gate), `MemberLeft` (self-
/// authored), `KeyDelivery`/`Noop`/metadata/context-registration ops. These are
/// covered by later, more specific shadows; the conservative subset here is the
/// unambiguous admin/capability gates.
fn apply_auth_requirement(
    signed: &calimero_context_client::local_governance::SignedNamespaceOp,
    decrypted: Option<&calimero_context_client::local_governance::GroupOp>,
) -> Option<(calimero_context_config::types::ContextGroupId, ApplyAuthReq)> {
    use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::MemberCapabilities as Cap;

    match &signed.op {
        NamespaceOp::Root(root) => {
            let ns_root = ContextGroupId::from(signed.namespace_id.to_bytes());
            match root {
                RootOp::AdminChanged { .. }
                | RootOp::PolicyUpdated { .. }
                | RootOp::GroupReparented { .. } => Some((ns_root, ApplyAuthReq::Admin)),
                RootOp::GroupCreated { parent_id, .. } => Some((
                    *parent_id,
                    ApplyAuthReq::AdminOrCap(Cap::CAN_CREATE_SUBGROUP.bits()),
                )),
                // GroupDeleted authorizes the subgroup OWNER or a
                // `CAN_DELETE_SUBGROUP` holder at the root, NOT only the root
                // admin — owner authority isn't in this admin/cap model, so skip.
                _ => None,
            }
        }
        NamespaceOp::Group { group_id, .. } => {
            let group = *group_id;
            match decrypted? {
                GroupOp::MemberAdded { .. } | GroupOp::MemberRemoved { .. } => {
                    Some((group, ApplyAuthReq::AdminOrCap(Cap::MANAGE_MEMBERS.bits())))
                }
                GroupOp::SubgroupVisibilitySet { .. } => Some((
                    group,
                    ApplyAuthReq::AdminOrCap(Cap::CAN_MANAGE_VISIBILITY.bits()),
                )),
                GroupOp::MemberRoleSet { .. }
                | GroupOp::MemberCapabilitySet { .. }
                | GroupOp::DefaultCapabilitiesSet { .. } => Some((group, ApplyAuthReq::Admin)),
                // TransferOwnership gates on the current OWNER identity
                // (`meta.owner_identity`), not group admin — outside this model.
                _ => None,
            }
        }
        // `NamespaceOp` is `#[non_exhaustive]`; an unknown future op authorizes
        // nothing here (secure default — no admin/cap requirement is granted).
        _ => None,
    }
}

/// The `(group, member)` a folded op moves, and which kind of op moved it — or
/// `None` for an op that touches no direct membership row.
///
/// A JOIN belongs here as much as an admin-push add does. `MemberJoinedWithDevice`
/// is what every invitation join folds to (membership and the joiner's device
/// credential as one indivisible fact), and the projection folds its membership
/// through the same `fold_member_added` an add uses — so the two planes are just
/// as comparable. Leaving this arm out meant no invitation join was ever compared,
/// on any scenario, for as long as the shadow has existed: the gate has been
/// checking admin-push adds and removals only, which is why a whole e2e suite
/// concluded 15 comparisons.
///
/// `MemberJoinedOpen` is deliberately absent and is not the same omission: an
/// open-subgroup self-join is a PROOF of inheritance, live writes no direct row
/// for it, and the projection models it as inherited too. There is nothing to
/// compare.
fn membership_touched(
    payload: &calimero_op::OpPayload,
) -> Option<(
    calimero_context_config::types::ContextGroupId,
    calimero_account::AccountId,
    &'static str,
)> {
    match payload {
        calimero_op::OpPayload::MemberAdded { group, member, .. } => Some((*group, *member, "add")),
        calimero_op::OpPayload::MemberJoinedWithDevice { group, member, .. } => {
            Some((*group, *member, "join"))
        }
        calimero_op::OpPayload::MemberRemoved { group, member } => {
            Some((*group, *member, "remove"))
        }
        _ => None,
    }
}

/// Everything a membership comparison needed to be conclusive, as the fold found
/// it.
struct ComparePremise {
    /// The op reached the projection (the lock was not poisoned).
    fed: bool,
    /// Every op in the cut's ancestry is decrypted, so the folded membership is
    /// final rather than provisional.
    decoded: bool,
    /// The live plane holds a DIRECT row for this member. Inherited and
    /// open-join members have none, and the projection modelling them as direct
    /// is an expected difference between the planes, not a disagreement.
    has_live_row: bool,
    /// The op's cut reaches everything live has applied, so both planes are
    /// describing the same history.
    at_frontier: bool,
    /// What the two planes said, valid only if the premise above holds.
    agrees: bool,
}

/// What a membership comparison DID, in one word, for the coverage gate.
///
/// The gate reads these to tell "the planes agreed" from "no comparison was
/// possible" — a distinction the absence of a divergence marker cannot make, and
/// without which the gate can go quiet while still reporting green.
///
/// `agree` and `diverged` are the only CONCLUSIVE answers, and they are reachable
/// only with the whole premise satisfied. Everything else names the reason the
/// comparison could not be made: `skipped_*` for a refusal, `no_live_row` for the
/// expected model difference (which is not a refusal — there was nothing to
/// compare against).
fn compare_result(premise: ComparePremise) -> &'static str {
    let ComparePremise {
        fed,
        decoded,
        has_live_row,
        at_frontier,
        agrees,
    } = premise;
    match (fed, decoded, has_live_row, at_frontier) {
        (false, ..) => "skipped_unfed",
        (true, false, ..) => "skipped_undecoded",
        (true, true, false, _) => "no_live_row",
        (true, true, true, false) => "skipped_stale_cut",
        (true, true, true, true) if agrees => "agree",
        (true, true, true, true) => "diverged",
    }
}

/// One delta the DAG just applied, as the shadow fold needs to see it: the
/// signed op plus the delta coordinates the fold keys and orders it by.
///
/// A struct rather than four positional parameters because two of the four are
/// `[u8; 32]`-shaped and one is a slice of the same, and the ordering mistake
/// that makes possible is silent — an op folded under a neighbour's id.
struct AppliedOp<'a> {
    signed: &'a calimero_governance_types::SignedNamespaceOp,
    id: [u8; 32],
    hlc: calimero_storage::logical_clock::HybridTimestamp,
    parents: &'a [[u8; 32]],
}

/// Fold everything an apply just applied into the shadow projection.
///
/// The DAG applies the delta it was handed AND every pending delta that one
/// unblocked, so this folds all of them: the primary op first, then the drained
/// cascade in the order it applied. Folding only the primary is what let the
/// projection trail the live store — a drained child reached the live store and
/// never the projection, leaving every later at-cut read of that scope answered
/// from a log missing an op the store had.
///
/// A no-op unless the apply actually applied something. The live governance
/// frontier is read once here and shared by every op folded, since they all
/// landed before it was read.
fn shadow_fold_applied(
    store: &calimero_store::Store,
    scope_projections: &std::sync::RwLock<crate::scope_projection::ScopeProjections>,
    dag: &calimero_dag::DagStore<calimero_governance_types::SignedNamespaceOp>,
    primary: AppliedOp<'_>,
    outcome: &AddDeltaOutcome,
) {
    if !outcome.is_applied() {
        return;
    }
    // Read AFTER the apply advanced it. The shadow compares an at-cut projection
    // answer against a live row, and those describe the same history only while
    // the cut reaches this frontier — see the `at_frontier` gate in
    // `shadow_fold_and_compare`. `None` (head unreadable) leaves that
    // unestablished, so the compare is skipped rather than run on an unknown
    // premise.
    let live_frontier = calimero_governance_store::NamespaceDagService::new(
        store,
        primary.signed.namespace_id,
    )
    .read_head_record()
    .map(|head| head.parent_hashes)
    .map_err(|err| {
        tracing::debug!(%err, "unified-op shadow: governance head unreadable; skipping compare");
    })
    .ok();
    let frontier = live_frontier.as_deref();

    shadow_fold_and_compare(store, scope_projections, primary, frontier);

    for cascaded_id in outcome.cascaded() {
        let Some(cascaded) = dag.get_delta(cascaded_id) else {
            // Applied-then-gone inside one call is not a state the DAG produces;
            // say so rather than fold a guess.
            tracing::warn!(
                delta_id = %hex::encode(cascaded_id),
                "unified-op shadow: drained delta absent from the DAG; not folded"
            );
            continue;
        };
        shadow_fold_and_compare(
            store,
            scope_projections,
            AppliedOp {
                signed: &cascaded.payload,
                id: cascaded.id,
                hlc: cascaded.hlc,
                parents: &cascaded.parents,
            },
            frontier,
        );
    }
}

/// Fold one just-applied governance op into the shadow projection, then compare
/// what the projection says against the live resolver.
///
/// Called once per delta the DAG actually applied — see
/// [`shadow_fold_applied`], which enumerates them.
///
/// `live_frontier` is live's governance head set, read once by
/// [`shadow_fold_applied`] and shared by every op folded in that call. `None`
/// means it could not be read, which leaves the comparison premise
/// unestablished — see the `at_frontier` gate below.
fn shadow_fold_and_compare(
    store: &calimero_store::Store,
    scope_projections: &std::sync::RwLock<crate::scope_projection::ScopeProjections>,
    op: AppliedOp<'_>,
    live_frontier: Option<&[[u8; 32]]>,
) {
    let AppliedOp {
        signed: signed_op,
        id: delta_id,
        hlc: delta_hlc,
        parents: delta_parents,
    } = op;
    let ns_id = signed_op.namespace_id;
    // For an encrypted `NamespaceOp::Group` that just applied,
    // decrypt its cleartext membership op (the key is present —
    // the live apply already used it). Read-only decrypt; never
    // re-runs the mutation. A `Root` op or an undecryptable group
    // op yields `None` → the node folds as `Noop` (still recorded
    // so the ancestry walk can pass through it).
    let decrypted = match &signed_op.op {
        calimero_governance_types::NamespaceOp::Group {
            group_id,
            key_id,
            encrypted,
            ..
        } => calimero_governance_store::decrypt_group_op(
            store,
            ns_id,
            *group_id,
            key_id.as_bytes(),
            encrypted,
        )
        .map_err(|err| {
            tracing::warn!(%err, "unified-op shadow: group-op decrypt failed; folded as Noop");
        })
        .ok()
        .flatten(),
        calimero_governance_types::NamespaceOp::Root(_) => None,
        // `NamespaceOp` is `#[non_exhaustive]`; an unknown future
        // op has nothing to decrypt and folds as `Noop`.
        _ => None,
    };
    // Resolved from the store, not derived from the key: the op
    // has just applied, and it could not have unless the signer's
    // binding was present — so every node agrees on this value.
    let signer_binding = calimero_governance_store::signer_binding_for(
        store,
        &ContextGroupId::from(ns_id.to_bytes()),
        &signed_op.signer,
    );
    let shadow_op = calimero_governance_store::op_from_namespace_op_with_binding(
        signed_op,
        decrypted.as_ref(),
        signer_binding,
        delta_id,
        delta_hlc,
        delta_parents,
    );

    {
        // The member this op touches (for the per-member
        // shadow-compare), if it's a membership op.
        let membership = membership_touched(&shadow_op.payload);

        // ONE lock acquisition: ingest, then read the just-applied
        // member's projected role so the compare reflects exactly
        // this op (no TOCTOU window between ingest and read). A
        // poisoned lock skips feed+compare with a warning rather
        // than affecting the governance apply path.
        let (fed, projected, decoded, at_frontier) = match scope_projections.write() {
            Ok(mut projections) => {
                projections.ingest_op(&shadow_op);
                // Does THIS op's cut — the one `role` is resolved
                // at, just below — reach everything live has
                // applied? Asked after the ingest, so the ordinary
                // in-order apply (the op IS live's head) covers the
                // frontier and the compare is meaningful.
                let at_frontier = live_frontier.is_some_and(|heads| {
                    projections.cut_covers_frontier(&shadow_op.scope, &[shadow_op.id()], heads)
                });
                // Resolve at THIS op's own causal cut (its id),
                // so a re-add after a remove reflects the
                // causally-latest state rather than the non-causal
                // `states` snapshot (governance ops share hlc=0).
                let role = membership.and_then(|(g, m, _)| {
                    projections.role_at_cut(&shadow_op.scope, &g, &m, &[shadow_op.id()])
                });
                // Is this cut's whole history present AND decoded?
                // If an ancestor is still an undecrypted `Noop`,
                // the folded membership is provisional and a
                // mismatch against live is a decrypt-feed lag, not
                // a fold-logic bug — same causal cut as `role`.
                let decoded = membership.is_some_and(|_| {
                    projections.cut_ancestry_decoded(&shadow_op.scope, &[shadow_op.id()])
                });
                (true, role, decoded, at_frontier)
            }
            Err(err) => {
                tracing::warn!(%err, "scope-projections lock poisoned; skipping unified-op shadow feed/compare");
                (false, None, false, false)
            }
        };

        // Shadow-compare (additive, log-only). Per-member (not
        // full-set) so a partially-fed projection — e.g. right
        // after restart — can't false-positive. Conservative gate:
        // only flag when the live resolver has a DIRECT row the
        // projection disagrees with; skip inherited / open-join
        // members the live system doesn't store directly but the
        // projection models as direct (an expected model
        // difference, not a feed bug). The projection's first
        // reader — the precursor to authorizing against it.
        //
        // Also require the cut's ancestry to be fully DECODED. When
        // an ancestor is still an undecrypted `Noop` — the window
        // between a member re-join and the `KeyDelivery` that lets
        // this node decrypt the encrypted `MemberAdded` (forward
        // secrecy rotates the group key on departure, so a re-add
        // rides a key epoch the rejoiner briefly lacks) — live
        // already has the materialized row while the projection is
        // still holding the add encrypted. That is a decrypt-feed
        // lag that the late-decrypt log upgrade heals on its own,
        // not a fold-logic divergence. Skipping it keeps the shadow
        // sharp for real disagreements, which surface only once the
        // history is fully readable.
        //
        // And require this op's CUT to cover live's governance
        // frontier. The role is resolved at the op's own cut while
        // `live` answers about now, so an op applied out of causal
        // order — the author's own op returning through the feed
        // after it published a newer one, a partition heal replaying
        // old history — makes the two answer different questions.
        // That is what fired here: the projection said `Member` at
        // the cut of an add while live said `Admin`, because the
        // promotion that followed the add had already applied. Both
        // correct, a divergence for neither. Folding more ops does
        // not fix it — the cut is what excludes the promotion — so
        // this is a real precondition, not a lag to wait out.
        if let Some((group, member, op_kind)) = membership {
            // Both sides now name the same principal: `member`
            // comes off the op payload as an account, and the
            // live rows are keyed by account. This used to
            // invert the account back into a member key by
            // scanning the live rows, because the two planes
            // disagreed about what a member IS.
            let live = (fed && decoded)
                .then(|| {
                    calimero_governance_store::MembershipRepository::new(store)
                        .role_of(&group, &member)
                        .ok()
                        .flatten()
                })
                .flatten();

            // Every membership op this node folds reports what the comparison
            // DID, not only when it failed. Without that a gate reading these
            // logs cannot tell "the planes agreed" from "no comparison was
            // possible": both look like the absence of a divergence marker, so
            // the gate can go quiet — across every scenario at once — while
            // still reporting green. The counts are the coverage.
            //
            // `skipped_*` results are the honest refusals: a comparison whose
            // premise does not hold says so rather than guessing. `no_live_row`
            // is the expected model difference (inherited and open-join members
            // the live plane stores no direct row for), not a refusal.
            let result = compare_result(ComparePremise {
                fed,
                decoded,
                has_live_row: live.is_some(),
                at_frontier,
                agrees: projected == live,
            });
            // INFO, not debug, and that is load-bearing: the coverage gate reads
            // these lines, so emitting them at a level a scenario's `log_level`
            // can filter out would put the gate back to trusting silence — the
            // exact failure it exists to catch. Two of 86 scenarios already
            // override the level, and a governance membership op is rare enough
            // (67 lines across a whole e2e suite) that INFO costs nothing.
            tracing::info!(
                marker = "unified_projection_compare",
                plane = "membership",
                result,
                op_kind,
                ?group,
                %member,
                ?projected,
                ?live,
                "unified-op membership comparison"
            );

            // Disagreement is only a DIVERGENCE if both planes were answering
            // the same question. Off the frontier they were not, which the
            // `skipped_stale_cut` result above records without failing a gate.
            if result == "diverged" {
                tracing::warn!(
                    marker = "unified_projection_divergence",
                    plane = "membership",
                    op_kind,
                    ?group,
                    %member,
                    ?projected,
                    ?live,
                    "unified-op projection disagrees with live membership resolver"
                );
            }
        }

        // APPLY-AUTH shadow (F5 #28, log-only): the op just APPLIED,
        // so the LIVE resolver authorized its signer. Would the
        // projection authorize the signer too — at the op's PARENT
        // cut (state EXCLUDING this op, the correct cut to authorize
        // against)? `Some(false)` = the projection would REJECT what
        // live accepted (under-auth — safe, but a real divergence to
        // investigate); `None` = ancestry not fully folded → skip.
        // The reverse (projection accepts what live rejected) is
        // unobservable here — a live-rejected op never reaches this
        // fold. Resolved at the parent cut (independent of the
        // just-ingested op), so it runs OUTSIDE the write lock under
        // a brief read lock — no store I/O while the apply path's
        // ingest is blocked.
        //
        // Same frontier premise as the membership compare above: a
        // projection that holds less than live under-authorizes for
        // that reason alone, so `Some(false)` would name the lag
        // rather than a real disagreement.
        if fed {
            if let Some((auth_group, req)) = apply_auth_requirement(signed_op, decrypted.as_ref()) {
                let verdict = match scope_projections.read() {
                    Ok(projections) => match req {
                        ApplyAuthReq::Admin => projections.is_admin_at_cut(
                            store,
                            auth_group,
                            &signed_op.signer,
                            delta_parents,
                        ),
                        ApplyAuthReq::AdminOrCap(bits) => projections
                            .is_admin_or_capability_at_cut(
                                store,
                                auth_group,
                                &signed_op.signer,
                                bits,
                                delta_parents,
                            ),
                    },
                    Err(_) => None,
                };
                if verdict == Some(false) {
                    // Same frontier premise, same reporting split
                    // as the membership compare above.
                    if at_frontier {
                        tracing::warn!(
                            marker = "unified_projection_divergence",
                            plane = "governance-auth",
                            group_id = ?auth_group,
                            signer = %signed_op.signer,
                            "projection would reject a governance op the live resolver authorized"
                        );
                    } else {
                        tracing::debug!(
                            plane = "governance-auth",
                            group_id = ?auth_group,
                            signer = %signed_op.signer,
                            "projection behind live's governance frontier; \
                             under-auth at the parent cut is the lag, not a disagreement"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use calimero_dag::{ApplyError, CausalDelta, DagStore, DeltaApplier};
    use calimero_governance_types::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_op::ScopeId;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use core::num::NonZeroU128;

    use super::{
        compare_result, membership_touched, shadow_fold_applied, AppliedOp, ComparePremise,
    };
    use crate::scope_projection::ScopeProjections;

    /// Drives the DAG's ordering machinery without a governance apply: this test
    /// is about which deltas the fold SEES, and a real applier would make the
    /// setup about signature and membership validity instead.
    struct NoopApplier;

    #[async_trait::async_trait]
    impl DeltaApplier<SignedNamespaceOp> for NoopApplier {
        async fn apply(&self, _delta: &CausalDelta<SignedNamespaceOp>) -> Result<(), ApplyError> {
            Ok(())
        }
    }

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(ns),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    /// A minimal well-formed envelope. The payload folds as a graph node; what
    /// matters here is that the op carries the namespace the fold scopes by.
    fn envelope(namespace: [u8; 32], parents: Vec<[u8; 32]>) -> SignedNamespaceOp {
        SignedNamespaceOp {
            version: 1,
            namespace_id: namespace.into(),
            parent_op_hashes: parents,
            signer: PublicKey::from([7u8; 32]),
            nonce: 0,
            op: NamespaceOp::Root(RootOp::PolicyUpdated {
                policy_bytes: Vec::new(),
            }),
            signature: [0u8; 64],
        }
    }

    /// Which payloads the gate can compare, pinned — because the answer was
    /// silently wrong for as long as the shadow existed. A join carries
    /// membership exactly as an add does and folds through the same
    /// `fold_member_added`, so omitting it did not make joins unfoldable, only
    /// unchecked: an entire e2e suite concluded 15 comparisons, none of them a
    /// join.
    #[test]
    fn a_join_is_as_comparable_as_an_add() {
        use calimero_account::AccountId;
        use calimero_context_config::types::ContextGroupId;
        use calimero_op::OpPayload;
        use calimero_primitives::context::GroupMemberRole;

        let group = ContextGroupId::from([3u8; 32]);
        let member = AccountId::from([9u8; 32]);

        let add = OpPayload::MemberAdded {
            group,
            member,
            role: GroupMemberRole::Member,
        };
        assert_eq!(membership_touched(&add), Some((group, member, "add")));

        let remove = OpPayload::MemberRemoved { group, member };
        assert_eq!(membership_touched(&remove), Some((group, member, "remove")));

        // The arm that was missing. Built through the same helper the fold uses,
        // so a change to the payload's shape shows up here rather than silently
        // dropping the arm again.
        let genesis = crate::test_support::credential(&PublicKey::from([4u8; 32]));
        let join = OpPayload::MemberJoinedWithDevice {
            group,
            member,
            role: GroupMemberRole::Member,
            genesis: genesis.genesis,
            chain: genesis.chain.clone(),
            cert: genesis.statement,
        };
        assert_eq!(
            membership_touched(&join),
            Some((group, member, "join")),
            "an invitation join moves a direct membership row and must be compared",
        );

        // An op that moves no direct row stays out: a `Noop` carries nothing, and
        // an open-subgroup self-join is a proof of inheritance that live writes no
        // row for, so there is nothing to compare against.
        assert_eq!(membership_touched(&OpPayload::Noop), None);
    }

    /// Exhaustive over the premise, because the property that matters is a
    /// negative one: a comparison may report itself CONCLUDED only when every
    /// part of its premise held. An edit that let a stale-cut or undecoded
    /// comparison count as `agree` would restore the silence the coverage gate
    /// exists to break — and it would do so while every scenario stayed green.
    #[test]
    fn only_a_complete_premise_yields_a_conclusive_comparison() {
        const KNOWN: [&str; 6] = [
            "agree",
            "diverged",
            "skipped_unfed",
            "skipped_undecoded",
            "skipped_stale_cut",
            "no_live_row",
        ];

        for bits in 0..32u8 {
            let (fed, decoded, has_live_row, at_frontier, agrees) = (
                bits & 1 != 0,
                bits & 2 != 0,
                bits & 4 != 0,
                bits & 8 != 0,
                bits & 16 != 0,
            );
            let result = compare_result(ComparePremise {
                fed,
                decoded,
                has_live_row,
                at_frontier,
                agrees,
            });

            assert!(
                KNOWN.contains(&result),
                "{result} is not a result the coverage gate knows how to count; a \
                 new label has to be taught to the gate, not just returned here",
            );
            assert_eq!(
                matches!(result, "agree" | "diverged"),
                fed && decoded && has_live_row && at_frontier,
                "conclusive only with the whole premise: {result} for fed={fed} \
                 decoded={decoded} live_row={has_live_row} at_frontier={at_frontier}",
            );
            if matches!(result, "agree" | "diverged") {
                assert_eq!(result == "agree", agrees, "verdict must follow the planes");
            }
        }
    }

    /// The apply path must fold every delta the DAG applied, not just the one it
    /// was handed. Two children arrive before their parent and sit pending; the
    /// parent's apply drains both, and all three have to reach the projection —
    /// otherwise an at-cut read of this scope answers from a log missing ops the
    /// live store holds.
    #[tokio::test]
    async fn folds_the_deltas_the_apply_drained_not_just_the_one_handed_in() {
        let ns = [0x44u8; 32];
        let scope = ScopeId::from(ns);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let projections = RwLock::new(ScopeProjections::new());
        let mut dag = DagStore::new([0u8; 32]);
        let applier = NoopApplier;

        let (id1, id2, id3) = ([0xD1u8; 32], [0xD2u8; 32], [0xD3u8; 32]);
        let op1 = envelope(ns, vec![]);
        let op2 = envelope(ns, vec![id1]);
        let op3 = envelope(ns, vec![id2]);

        // Children first: both park in the pending buffer.
        let out3 = dag
            .add_delta_with_outcome(CausalDelta::new(id3, vec![id2], op3, hlc(3)), &applier)
            .await
            .expect("add id3");
        let out2 = dag
            .add_delta_with_outcome(CausalDelta::new(id2, vec![id1], op2, hlc(2)), &applier)
            .await
            .expect("add id2");
        assert!(out3.is_pending() && out2.is_pending());

        // The parent arrives and takes the chain with it.
        let outcome = dag
            .add_delta_with_outcome(
                CausalDelta::new(id1, vec![], op1.clone(), hlc(1)).into_genesis(),
                &applier,
            )
            .await
            .expect("add id1");
        assert_eq!(
            outcome.cascaded(),
            &[id2, id3],
            "the apply drained both children",
        );

        shadow_fold_applied(
            &store,
            &projections,
            &dag,
            AppliedOp {
                signed: &op1,
                id: id1,
                hlc: hlc(1),
                parents: &[],
            },
            &outcome,
        );

        let folded = projections.read().expect("projection lock");
        // Asserted per id, NOT through the coverage walk: that walk counts an id
        // cited by a folded op as reached whether or not the op itself is
        // present, so it would pass with the drained deltas still missing.
        for id in [id1, id2, id3] {
            assert!(
                folded.has_folded(&scope, &id),
                "delta {} applied but was never folded",
                hex::encode(id),
            );
        }
    }
}
