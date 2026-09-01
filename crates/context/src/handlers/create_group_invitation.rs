use actix::{ActorResponse, Handler, Message, WrapFuture as _};
use calimero_context_client::group::{CreateGroupInvitationRequest, CreateGroupInvitationResponse};
use calimero_context_config::types::{
    GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::{MembershipRepository, MetaRepository, MetadataRepository};
use calimero_primitives::identity::PrivateKey;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::ContextManager;

/// Format this node's confirmed external addresses as multiaddrs a joiner can
/// dial.
///
/// Split from the lookup so the shape is testable without a live swarm: the
/// peer id is appended because a bare address does not say who answers at it,
/// and a joiner cannot resolve that for itself — the identity-to-peer map is
/// exactly what it does not have yet.
fn dialable_self_addrs(
    local_peer_id: &impl std::fmt::Display,
    external_addrs: &[impl std::fmt::Display],
) -> Vec<String> {
    external_addrs
        .iter()
        .map(|addr| format!("{addr}/p2p/{local_peer_id}"))
        .collect()
}

/// Where the invitation's admitters can be reached, best-effort.
///
/// Two sources, because the two cases fail differently. This node answers for
/// itself out of the swarm's confirmed external addresses, which are current by
/// construction. Every other admitter is answered out of the durable caches,
/// which is the only thing that makes a *delegated* invitation usable at all:
/// `CAN_INVITE_MEMBERS` lets a member mint invitations it may not admit, so the
/// admitters are other nodes and the joiner has no way to find them.
///
/// Only `Multiaddr` endpoints are produced. A `Url` hint would need a node to
/// know the base URL its own admin API is served on, and nothing records that —
/// so a keyholder with no node still supplies its own.
///
/// Best-effort throughout: the caches expire, so an admitter not seen lately is
/// simply not hinted. That is a joiner with one fewer address to try, never a
/// wrong one — the signed `admitters` list still decides who may answer.
async fn resolve_admitter_addrs(
    node_client: &calimero_node_primitives::client::NodeClient,
    datastore: &calimero_store::Store,
    signed: &SignedGroupOpenInvitation,
    signer_account: calimero_account::AccountId,
) -> Vec<String> {
    let group_id = signed.invitation.group_id;
    let mut addrs = Vec::new();

    // This node, if it is one of the admitters. `external_addrs` rather than
    // `listen_addrs`: the latter includes `0.0.0.0` and loopback, which are not
    // addresses anybody else can dial, and the former already carries the
    // relay-circuit form a NAT'd node is reachable on.
    if signed.invitation.admitters.contains(&signer_account) {
        let status = node_client.network_status().await;
        addrs.extend(dialable_self_addrs(
            &status.local_peer_id,
            &status.external_addrs,
        ));
    }

    // Every other admitter, through the two durable caches. An account is not
    // an address: it fans out to the signing keys its live devices hold, each
    // of which may have been seen on some peer, each of which may have a cached
    // address. Any link missing just means no hint for that admitter.
    let others: Vec<_> = signed
        .invitation
        .admitters
        .iter()
        .filter(|account| **account != signer_account)
        .copied()
        .collect();

    if !others.is_empty() {
        let by_account = calimero_governance_store::AccountBindingRepository::new(datastore)
            .live_devices_by_account(&group_id)
            .unwrap_or_default();
        let identities: Vec<_> = others
            .iter()
            .filter_map(|account| by_account.get(account))
            .flat_map(|devices| devices.iter().map(|d| d.sign_pk))
            .collect();

        if !identities.is_empty() {
            for (peer, addr) in node_client
                .peer_addrs_for_identities(group_id, identities)
                .await
            {
                addrs.push(format!("{addr}/p2p/{peer}"));
            }
        }
    }

    let mut seen = std::collections::BTreeSet::new();
    addrs.retain(|addr| seen.insert(addr.clone()));

    if addrs.is_empty() {
        // Said out loud, because the alternative is minting a credential that
        // silently cannot be redeemed: a joiner holding a hintless invitation
        // has to already know where to knock, and for a keyholder with no node
        // that is the case direct admission exists to remove.
        tracing::warn!(
            group_id = ?group_id,
            admitters = signed.invitation.admitters.len(),
            "invitation minted with no admitter hints: no admitter has a known \
             address, so the joiner needs one out of band"
        );
    }
    addrs
}

impl Handler<CreateGroupInvitationRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupInvitationRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupInvitationRequest {
            group_id,
            expiration_timestamp,
            admitters,
            admitter_addrs,
        }: CreateGroupInvitationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let (signer, node_sk) = match self.resolve_signer(&group_id) {
            Ok(pair) => pair,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = self.datastore.clone();

        let result = (|| -> eyre::Result<_> {
            let meta = MetaRepository::new(&datastore)
                .load(&group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            let signer_account = crate::member_account::require(&datastore, &group_id, &signer)?;
            MembershipRepository::new(&datastore).require_admin_or_capability(
                &group_id,
                &signer_account,
                MemberCapabilities::CAN_INVITE_MEMBERS.bits(),
                "create group invitation",
            )?;

            // An invitation nobody restricted is claimable by broadcast, which
            // publishes it to every subscriber of the namespace topic. Callers
            // that say nothing get the group's admins and TEE nodes rather than
            // that, so the exposed case stops being the one you reach by
            // omission.
            let admitters = if admitters.is_empty() {
                let defaulted =
                    calimero_governance_store::NamespaceMembershipService::default_admitters(
                        &datastore, &group_id,
                    )?;
                // A group always has at least one admin — losing the last one is
                // refused by `ensure_not_last_admin_removal` and its demotion
                // counterpart — so an empty result here is not a group without
                // admins, it is a store that disagrees with an invariant.
                //
                // Refused rather than defaulted through. An empty admitter list is
                // the one value that means "claimable by broadcast", so carrying
                // on would answer an inconsistency by minting the least
                // restricted credential the system can express, at exactly the
                // moment there is reason to trust it least.
                if defaulted.is_empty() {
                    eyre::bail!(
                        "refusing to mint an invitation for a group with no admin and no TEE node: \
                         every group is supposed to have an admin, so this is an inconsistent \
                         store rather than a group to issue an unrestricted invitation for"
                    );
                }

                // Deliberately NOT the inviter. `CAN_INVITE_MEMBERS` grants
                // creating an invitation, not seeing it through: admission stays
                // with admins and TEE nodes, so a delegated inviter cannot
                // complete a membership on its own.
                //
                // The cost is reachability, not authority. A non-admin inviter
                // mints invitations it cannot admit, so the invitee has to reach
                // an admin or TEE node rather than whoever handed it the
                // invitation — which is what `admitter_addrs` is for. This node
                // can only hint the admitters it can address, and itself is the
                // one it always can; see the hint pass below for what that
                // leaves uncovered.
                defaulted
            } else {
                admitters
            };

            let private_key = PrivateKey::from(node_sk);

            let mut rng = rand::thread_rng();
            let invitation_nonce: [u8; 32] = rng.gen();

            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_secs();
            // Clamped, not merely defaulted: a caller asking for longer is a
            // caller extending how long a leaked bearer credential stays
            // redeemable, and the request is not theirs to grant.
            let requested = expiration_timestamp
                .unwrap_or(calimero_context_config::types::MAX_INVITATION_VALIDITY_SECS)
                .min(calimero_context_config::types::MAX_INVITATION_VALIDITY_SECS);
            let expiration_timestamp: u64 = now_secs + requested;

            let inviter_signer_id = SignerId::from(*signer);

            let invitation = GroupInvitationFromAdmin {
                inviter_identity: inviter_signer_id,
                group_id,
                expiration_timestamp,
                invitation_nonce,
                invited_role: 1, // Member
                admitters,
            };

            let invitation_bytes = borsh::to_vec(&invitation)
                .map_err(|e| eyre::eyre!("failed to serialize invitation: {e}"))?;
            let hash = Sha256::digest(&invitation_bytes);
            let signature = private_key
                .sign(&hash)
                .map_err(|e| eyre::eyre!("signing failed: {e}"))?;
            let inviter_signature = hex::encode(signature.to_bytes());

            let group_name = MetadataRepository::new(&datastore)
                .group_metadata(&group_id)?
                .and_then(|r| r.name);

            Ok((
                SignedGroupOpenInvitation {
                    inviter_account: Some(signer_account),
                    invitation,
                    inviter_signature,
                    // Carry the real application_id so the joiner can
                    // pre-populate GroupMetaValue correctly. Without this,
                    // joiners would write target_application_id = ZERO
                    // and compute_group_state_hash would diverge from
                    // the inviter's view persistently.
                    application_id: Some(*meta.target_application_id.as_ref()),
                    // Carry the real bytecode_id (already derived from
                    // blob_id(app_meta.bytecode) at create_group time)
                    // so the joiner's pre-populated GroupMetaValue
                    // matches the originator's. Without this the
                    // joiner's bytecode_id seeds to [0u8; 32] and any
                    // CascadeUpgrade op the joiner
                    // applies silently skips the subtree — divergence
                    // between originator and joiner.
                    bytecode_id: Some(meta.bytecode_id),
                    admitter_addrs,
                },
                group_name,
                signer_account,
            ))
        })();

        let (signed_invitation, group_name, signer_account) = match result {
            Ok(v) => v,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        // No commitment publishing needed — the signed invitation is a
        // self-contained bearer credential. The joiner will present it
        // in a RootOp::MemberJoined on the namespace topic.
        //
        // Hints are attached AFTER signing, which is what makes this an async
        // tail rather than a restructure: they sit outside the signature, so
        // nothing here can change what was signed above.
        let node_client = self.node_client.clone();
        let datastore_for_hints = self.datastore.clone();
        ActorResponse::r#async(
            async move {
                let mut signed_invitation = signed_invitation;
                if signed_invitation.admitter_addrs.is_empty() {
                    signed_invitation.admitter_addrs = resolve_admitter_addrs(
                        &node_client,
                        &datastore_for_hints,
                        &signed_invitation,
                        signer_account,
                    )
                    .await;
                }
                Ok(CreateGroupInvitationResponse {
                    invitation: signed_invitation,
                    group_name,
                })
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::dialable_self_addrs;

    #[test]
    fn an_address_carries_the_peer_id_it_alone_does_not() {
        // A joiner cannot turn a bare address into "who answers there" — the
        // identity-to-peer map is precisely what it has not synced yet. So the
        // peer id travels in the hint or the hint is unusable.
        let addrs = dialable_self_addrs(&"12D3KooWTEST", &["/ip4/10.0.0.1/tcp/2528"]);
        assert_eq!(
            addrs,
            vec!["/ip4/10.0.0.1/tcp/2528/p2p/12D3KooWTEST".to_owned()]
        );
    }

    #[test]
    fn a_relay_circuit_address_is_hinted_like_any_other() {
        // The NAT'd case, and the one worth pinning: a node behind NAT is
        // reachable only through its relay reservation, so dropping circuit
        // addresses would leave exactly the nodes that most need a hint
        // advertising nothing.
        let addrs = dialable_self_addrs(
            &"12D3KooWSELF",
            &["/ip4/1.2.3.4/tcp/4001/p2p/12D3KooWRELAY/p2p-circuit"],
        );
        assert_eq!(
            addrs,
            vec!["/ip4/1.2.3.4/tcp/4001/p2p/12D3KooWRELAY/p2p-circuit/p2p/12D3KooWSELF".to_owned()]
        );
    }

    #[test]
    fn no_external_address_yields_no_hint_rather_than_a_useless_one() {
        let addrs = dialable_self_addrs(&"12D3KooWSELF", &[] as &[String]);
        assert!(
            addrs.is_empty(),
            "a node with no confirmed external address has nothing dialable to offer"
        );
    }
}
