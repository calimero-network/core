use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::AdmitTeeNodeRequest;
use calimero_context_client::local_governance::{AckRouter, GroupOp, NamespaceOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use tracing::{info, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    GroupKeyring, MembershipRepository, NamespaceRepository, SigningKeysRepository,
};

/// Publish a `RootOp::KeyDelivery` wrapping the namespace group key for
/// `member`, signed with the verifier's namespace identity (`signer_sk`).
///
/// The TEE-attestation join op is an encrypted `NamespaceOp::Group` that
/// only the verifier can apply, so the admitted node can't decrypt its own
/// membership until it holds the key. The verifier (which holds the key)
/// proactively delivers it here — one-shot, admin-initiated. The joiner-side
/// pull (`recover_missing_group_keys`) is the durable fallback. Idempotent
/// on the recipient: a duplicate `KeyDelivery` for a key already held is a
/// no-op (`store_key` keys by content).
async fn deliver_group_key_to_member(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &AckRouter,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    member: &PublicKey,
) -> eyre::Result<()> {
    let namespace_id = NamespaceRepository::new(store).resolve(group_id)?;

    let Some((_key_id, group_key)) = GroupKeyring::new(store, *group_id).load_current_key()? else {
        // The verifier admitted the node against this namespace's policy but
        // holds no group key for it — there is nothing to deliver. This should
        // not happen for a namespace owner/admin, but bail loudly rather than
        // silently leave the admitted node un-bootstrappable.
        eyre::bail!("verifier has no group key for namespace; cannot deliver to admitted TEE node");
    };

    let envelope =
        GroupKeyring::wrap_for_member(signer_sk, member, &group_id.to_bytes(), &group_key)?;

    let delivery_op = NamespaceOp::Root(RootOp::KeyDelivery {
        group_id: group_id.to_bytes().into(),
        envelope,
    });

    // Target only the admitted member's ack for delivery confirmation.
    // Best-effort: an unformed mesh downgrades readiness rather than failing,
    // and the announcer re-announces (re-admit) to retry; the pull also
    // recovers it.
    let report = calimero_governance_store::sign_and_publish_namespace_op(
        store,
        node_client,
        ack_router,
        namespace_id.to_bytes().into(),
        signer_sk,
        delivery_op,
        Some(vec![*member]),
    )
    .await?;

    info!(
        group_id = %hex::encode(group_id.to_bytes()),
        %member,
        acked = report.acked_by.len(),
        elapsed_ms = report.elapsed_ms,
        "published KeyDelivery for admitted TEE node"
    );

    Ok(())
}

impl Handler<AdmitTeeNodeRequest> for ContextManager {
    type Result = ActorResponse<Self, <AdmitTeeNodeRequest as Message>::Result>;

    fn handle(
        &mut self,
        AdmitTeeNodeRequest {
            group_id,
            member,
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            is_mock,
        }: AdmitTeeNodeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_namespace_identity(&group_id);

        let requester = match node_identity {
            Some((pk, _)) => pk,
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "node has no configured group identity for TEE admission"
                )))
            }
        };

        let node_sk = node_identity.map(|(_, sk)| sk);

        let policy = match calimero_governance_store::read_tee_admission_policy(
            &self.datastore,
            &group_id,
        ) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "no TeeAdmissionPolicy set for group"
                )))
            }
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        if is_mock && !policy.accept_mock {
            return ActorResponse::reply(Err(eyre::eyre!(
                "mock attestation rejected by group policy"
            )));
        }

        if policy.allowed_mrtd.is_empty() {
            return ActorResponse::reply(Err(eyre::eyre!(
                "TEE admission policy has empty allowed_mrtd — at least one MRTD must be specified"
            )));
        }
        if !policy.allowed_mrtd.iter().any(|a| a == &mrtd) {
            return ActorResponse::reply(Err(eyre::eyre!("MRTD not in policy allowlist")));
        }
        if !policy.allowed_tcb_statuses.is_empty()
            && !policy.allowed_tcb_statuses.iter().any(|a| a == &tcb_status)
        {
            return ActorResponse::reply(Err(eyre::eyre!("TCB status not in policy allowlist")));
        }
        if !policy.allowed_rtmr0.is_empty() && !policy.allowed_rtmr0.iter().any(|a| a == &rtmr0) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR0 not in policy allowlist")));
        }
        if !policy.allowed_rtmr1.is_empty() && !policy.allowed_rtmr1.iter().any(|a| a == &rtmr1) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR1 not in policy allowlist")));
        }
        if !policy.allowed_rtmr2.is_empty() && !policy.allowed_rtmr2.iter().any(|a| a == &rtmr2) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR2 not in policy allowlist")));
        }
        if !policy.allowed_rtmr3.is_empty() && !policy.allowed_rtmr3.iter().any(|a| a == &rtmr3) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR3 not in policy allowlist")));
        }

        // Direct-row check: TEE admission writes the node's direct
        // membership row + signing key. An inherited match via the
        // Open-subgroup chain (#2256) does not mean the node already has
        // its own direct row, and skipping the write here would leave
        // the TEE without a per-node row that subsequent direct-membership
        // operations expect.
        match MembershipRepository::new(&self.datastore).has_direct_member(&group_id, &member) {
            Ok(true) => return ActorResponse::reply(Ok(())),
            Ok(false) => {}
            Err(e) => return ActorResponse::reply(Err(e)),
        }

        match calimero_governance_store::is_quote_hash_used(&self.datastore, &group_id, &quote_hash)
        {
            Ok(true) => {
                return ActorResponse::reply(Err(eyre::eyre!("TEE attestation quote already used")))
            }
            Ok(false) => {}
            Err(e) => return ActorResponse::reply(Err(e)),
        }

        if let Some(ref sk) = node_sk {
            if let Err(err) =
                SigningKeysRepository::new(&self.datastore).store_key(&group_id, &requester, sk)
            {
                tracing::warn!(?group_id, %requester, error = %err, "Failed to persist group signing key");
            }
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let effective_signing_key = node_sk.or_else(|| {
            SigningKeysRepository::new(&self.datastore)
                .get_key(&group_id, &requester)
                .ok()
                .flatten()
        });

        ActorResponse::r#async(
            async move {
                let sk =
                    PrivateKey::from(effective_signing_key.ok_or_else(|| {
                        eyre::eyre!("no signing key available for TEE admission")
                    })?);
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    GroupOp::MemberJoinedViaTeeAttestation {
                        member,
                        quote_hash,
                        mrtd,
                        rtmr0,
                        rtmr1,
                        rtmr2,
                        rtmr3,
                        tcb_status,
                        role: GroupMemberRole::ReadOnlyTee,
                    },
                )
                .await?;
                report.observe("admit_tee_node", "MemberJoinedViaTeeAttestation");

                info!(%member, ?group_id, "TEE node admitted via attestation");

                // Deliver the namespace group key to the freshly-admitted
                // TEE node. A `MemberJoinedViaTeeAttestation` op is an
                // encrypted `NamespaceOp::Group` only the verifier can apply,
                // so the node can't decrypt its own membership until it holds
                // the key. The verifier holds it and signs with its namespace
                // identity, so it publishes the `KeyDelivery` directly (the
                // node picks it up off the DAG/gossip when it pulls the
                // namespace governance DAG after fleet-join). Best-effort —
                // the joiner-side pull recovers it if this delivery is missed.
                if let Err(err) = deliver_group_key_to_member(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    &member,
                )
                .await
                {
                    warn!(
                        %member,
                        ?group_id,
                        ?err,
                        "TEE admission succeeded but KeyDelivery to the admitted node \
                         failed — the node recovers it via the joiner-side pull, or on \
                         re-admission."
                    );
                }

                // Auto-follow flags for the admitted TEE member are
                // published by the member itself in `fleet_join.rs` after
                // it observes admission — signed with its own namespace
                // identity, which satisfies `MemberSetAutoFollow`'s
                // admin-or-self authorization rule. The verifier (this
                // handler) has neither admin authority nor the member's
                // signing key, so it can't do it here.

                Ok(())
            }
            .into_actor(self),
        )
    }
}
