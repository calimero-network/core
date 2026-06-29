use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::{ApplySignedNamespaceOpRequest, NamespaceApplyOutcome};
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
        let dag = self.get_or_create_namespace_dag(&namespace_id);
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
        let feed_store = self.datastore.clone();
        let scope_projections = Arc::clone(&self.scope_projections);

        ActorResponse::r#async(
            async move {
                let mut dag = dag.lock().await;
                let outcome = dag.add_delta_with_outcome(delta, &applier).await;

                // Fold into the shadow projection only on a real apply, mirroring
                // the divergence-outbox gating below. A poisoned lock is ignored:
                // the projection is not yet authoritative, so it must never break
                // the governance apply path.
                if matches!(outcome, Ok(AddDeltaOutcome::Applied)) {
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
                            &feed_store,
                            namespace_id,
                            calimero_context_config::types::ContextGroupId::from(*group_id),
                            key_id,
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
                    let shadow_op = crate::scope_projection::op_from_namespace_op(
                        &signed_op,
                        decrypted.as_ref(),
                        delta_id,
                        delta_hlc,
                        &delta_parents,
                    );

                    {
                        // The member this op touches (for the per-member
                        // shadow-compare), if it's a membership op.
                        let membership = match &shadow_op.payload {
                            calimero_op::OpPayload::MemberAdded { group, member, .. }
                            | calimero_op::OpPayload::MemberRemoved { group, member } => {
                                Some((*group, *member))
                            }
                            _ => None,
                        };

                        // ONE lock acquisition: ingest, then read the just-applied
                        // member's projected role so the compare reflects exactly
                        // this op (no TOCTOU window between ingest and read). A
                        // poisoned lock skips feed+compare with a warning rather
                        // than affecting the governance apply path.
                        let (fed, projected) = match scope_projections.write() {
                            Ok(mut projections) => {
                                projections.ingest_op(&shadow_op);
                                // Resolve at THIS op's own causal cut (its id),
                                // so a re-add after a remove reflects the
                                // causally-latest state rather than the non-causal
                                // `states` snapshot (governance ops share hlc=0).
                                let role = membership.and_then(|(g, m)| {
                                    projections.role_at_cut(
                                        &shadow_op.scope,
                                        &g,
                                        &m,
                                        &[shadow_op.id],
                                    )
                                });
                                (true, role)
                            }
                            Err(err) => {
                                tracing::warn!(%err, "scope-projections lock poisoned; skipping unified-op shadow feed/compare");
                                (false, None)
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
                        if fed {
                            if let Some((group, member)) = membership {
                                let live = calimero_governance_store::MembershipRepository::new(
                                    &compare_store,
                                )
                                .role_of(&group, &member)
                                .ok()
                                .flatten();
                                if live.is_some() && projected != live {
                                    tracing::warn!(
                                        marker = "unified_projection_divergence",
                                        plane = "membership",
                                        ?group,
                                        %member,
                                        ?projected,
                                        ?live,
                                        "unified-op projection disagrees with live membership resolver"
                                    );
                                }
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
                        if fed {
                            if let Some((auth_group, req)) =
                                apply_auth_requirement(&signed_op, decrypted.as_ref())
                            {
                                let verdict = match scope_projections.read() {
                                    Ok(projections) => match req {
                                        ApplyAuthReq::Admin => projections.is_admin_at_cut(
                                            &feed_store,
                                            auth_group,
                                            &signed_op.signer,
                                            &delta_parents,
                                        ),
                                        ApplyAuthReq::AdminOrCap(bits) => projections
                                            .is_admin_or_capability_at_cut(
                                                &feed_store,
                                                auth_group,
                                                &signed_op.signer,
                                                bits,
                                                &delta_parents,
                                            ),
                                    },
                                    Err(_) => None,
                                };
                                if verdict == Some(false) {
                                    tracing::warn!(
                                        marker = "unified_projection_divergence",
                                        plane = "governance-auth",
                                        group_id = ?auth_group,
                                        signer = %signed_op.signer,
                                        "projection would reject a governance op the live resolver authorized"
                                    );
                                }
                            }
                        }
                    }
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
                if matches!(outcome, Ok(AddDeltaOutcome::Applied))
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
                    Ok(AddDeltaOutcome::Applied) => {
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
            let ns_root = ContextGroupId::from(signed.namespace_id);
            match root {
                RootOp::AdminChanged { .. }
                | RootOp::PolicyUpdated { .. }
                | RootOp::GroupReparented { .. } => Some((ns_root, ApplyAuthReq::Admin)),
                RootOp::GroupCreated { parent_id, .. } => Some((
                    ContextGroupId::from(*parent_id),
                    ApplyAuthReq::AdminOrCap(Cap::CAN_CREATE_SUBGROUP),
                )),
                // GroupDeleted authorizes the subgroup OWNER or a
                // `CAN_DELETE_SUBGROUP` holder at the root, NOT only the root
                // admin — owner authority isn't in this admin/cap model, so skip.
                _ => None,
            }
        }
        NamespaceOp::Group { group_id, .. } => {
            let group = ContextGroupId::from(*group_id);
            match decrypted? {
                GroupOp::MemberAdded { .. } | GroupOp::MemberRemoved { .. } => {
                    Some((group, ApplyAuthReq::AdminOrCap(Cap::MANAGE_MEMBERS)))
                }
                GroupOp::SubgroupVisibilitySet { .. } => {
                    Some((group, ApplyAuthReq::AdminOrCap(Cap::CAN_MANAGE_VISIBILITY)))
                }
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
