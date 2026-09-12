use calimero_governance_store::{
    account_for_group, CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository,
    MetadataRepository, ReentryRepository,
};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_client::messages::NamespaceApplyOutcome;
use calimero_node_primitives::join_bundle::JoinBundle;
use calimero_primitives::context::{ContextConfigParams, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::key;
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

use crate::ContextManager;
use calimero_governance_store::op_events::subscribe as subscribe_op_events;
use calimero_governance_store::op_events::OpEvent;

const NAMESPACE_MESH_GRACE: Duration = Duration::from_secs(2);

// Maximum time `join_group` waits on the gossip-fallback path for a
// `KeyDelivery` op addressed to the joiner now lives on
// [`ContextManagerConfig::key_delivery_fallback_wait`] so operators can
// override it without source patches. Default is preserved (5s).

impl Handler<JoinGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupRequest {
            invitation,
            group_name,
        }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let group_id = invitation.invitation.group_id;
        let invited_role = invitation.invitation.invited_role;
        let expiration = invitation.invitation.expiration_timestamp;
        let now_secs = calimero_governance_store::now_secs();
        if expiration != 0 && now_secs > expiration {
            return ActorResponse::reply(Err(eyre::eyre!("invitation expired")));
        }

        let (ns_id, joiner_identity, sk_bytes) =
            match self.get_or_create_namespace_identity(&group_id) {
                Ok(result) => result,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to resolve namespace identity for join: {err}"
                    )));
                }
            };

        let namespace_id = ns_id.to_bytes();
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let context_client = self.context_client.clone();
        let key_delivery_fallback_wait = self.config.key_delivery_fallback_wait;

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(sk_bytes);
                let role = match invited_role {
                    0 => GroupMemberRole::Admin,
                    2 => GroupMemberRole::ReadOnly,
                    _ => GroupMemberRole::Member,
                };

                // Verify the invitation's inviter signature before it seeds any
                // local trust. `admin_identity` written in Phase 1 becomes the
                // local root of trust for this namespace's beacon / ack /
                // heartbeat verification and `is_admin`, so a forged invitation
                // must not reach the seed below.
                calimero_governance_store::NamespaceMembershipService::verify_open_invitation_signature(
                    &invitation,
                )?;

                // -------------------------------------------------------
                // Phase 1: Set up local state.
                // -------------------------------------------------------

                if MetaRepository::new(&datastore).load(&group_id)?.is_none() {
                    // From the invitation ENVELOPE, and optional there: it is a
                    // bootstrap hint, and it is treated as one — recorded in a
                    // node-local row below, never seeded as this group's admin.
                    // See the seeding site for why "genesis repairs it anyway"
                    // is not true.
                    let inviter_account = invitation.inviter_account;
                    // The invitation carries the application id so joiners
                    // pre-populate `GroupMetaValue` with the real value:
                    // `target_application_id` is part of
                    // `compute_group_state_hash`, so a placeholder here
                    // diverges the joiner's hash from the originator's.
                    //
                    // An invitation without it is refused rather than
                    // defaulted. Every current producer sets it
                    // (`create_group_invitation`), so absence means the
                    // invitation was minted or re-serialized by something that
                    // dropped the field — and joining on a zero id trades a
                    // clear failure here for a state-hash divergence later.
                    let Some(application_id) = invitation.application_id else {
                        return Err(eyre::eyre!(
                            "invitation for group {group_id:?} carries no application_id;                              refusing to join on a placeholder that would diverge                              compute_group_state_hash from the inviter's"
                        ));
                    };
                    let target_application_id =
                        calimero_primitives::application::ApplicationId::from(application_id);
                    // Prefer the invitation's `bytecode_id` field when present.
                    // When it is missing (e.g. an older Python client on
                    // the wire deserialized the invitation against a
                    // pre-`bytecode_id` `SignedGroupOpenInvitation` and
                    // silently dropped the unknown field on its
                    // re-serialize), re-derive locally using the SAME
                    // algorithm the originator used in `create_group`
                    // (`blob_id(app_meta.bytecode)`), which is
                    // deterministic across nodes that hold the same
                    // application bytecode. Final fallback to zero
                    // applies only when neither the invitation nor a
                    // locally-installed application can produce a value
                    // — in that case the existing self-heal on the next
                    // governance op still recovers, just one
                    // gossip-round later.
                    let bytecode_id = invitation.bytecode_id.unwrap_or_else(|| {
                        let handle = datastore.handle();
                        let key = calimero_store::key::ApplicationMeta::new(target_application_id);
                        match handle.get(&key) {
                            Ok(Some(app_meta)) => *app_meta.bytecode.blob_id().as_ref(),
                            _ => [0u8; 32],
                        }
                    });
                    // The placeholder, never `invitation.inviter_account`.
                    //
                    // That hint rides in the UNSIGNED envelope —
                    // `inviter_signature` covers the inner
                    // `GroupInvitationFromAdmin` only — so anything that can
                    // relay an invitation can choose it. Seeding it made it this
                    // node's `admin_identity`, the local root of trust for
                    // `is_admin` and for beacon, ack and heartbeat verification.
                    //
                    // The field's own docs called that safe because genesis
                    // overwrites a wrong value. It does not: `namespace_created`
                    // keys its established-check on
                    // `admin_identity != placeholder` and is a no-op once one is
                    // set, so an attacker-chosen account would have been
                    // permanent on this node rather than reconciled.
                    //
                    // The head start the hint exists for is kept, in the
                    // node-local bootstrap row written below — which confers no
                    // authority and stops being read the moment a real admin
                    // exists. This mirrors `join_namespace` in `calimero-node`,
                    // the other entry point into this same cold-start window.
                    let seeded_admin = calimero_governance_store::placeholder_admin_identity();
                    let meta = calimero_store::key::GroupMetaValue {
                        // Coordinates stay unset until governance names them.
                        target: calimero_store::key::GroupTarget {
                            application_id: target_application_id,
                            bytecode_id,
                            ..Default::default()
                        },
                        admin_identity: seeded_admin,
                        owner_identity: seeded_admin,
                        migration: None,
                        created_at: 0,
                        auto_join: true,
                    };
                    MetaRepository::new(&datastore).save(&group_id, &meta)?;

                    // The hint goes to its own node-local row, not to a
                    // membership row.
                    //
                    // Recorded at all because a joiner that has applied no DAG
                    // ops verifies nothing it receives — including the inviter's
                    // readiness beacons, which are the trigger that would fetch
                    // the state ending that condition. `namespace_accounts`
                    // admits this hint while the admin is still the placeholder,
                    // which is exactly the cold-start window and no longer.
                    //
                    // An earlier version wrote an Admin MEMBERSHIP row from it
                    // instead. That row outlives the window — nothing retracts
                    // it once genesis names the real admin — so an unsigned
                    // field chosen by whoever relayed the invitation became a
                    // durable grant.
                    if let Some(hint) = inviter_account {
                        MembershipRepository::new(&datastore)
                            .set_bootstrap_inviter(group_id.to_bytes().into(), hint)?;
                    }
                }

                // -------------------------------------------------------
                // Phase 2: Subscribe to namespace topic, wait for mesh
                //          formation, then get everything we need via a
                //          single direct stream request to a mesh peer.
                // -------------------------------------------------------

                let _ = node_client.subscribe_namespace(namespace_id).await;
                tokio::time::sleep(NAMESPACE_MESH_GRACE).await;

                let invitation_bytes = borsh::to_vec(&invitation)
                    .map_err(|e| eyre::eyre!("failed to serialize invitation: {e}"))?;

                // A thin / still-forming namespace mesh at join time must not
                // strand the joiner. If no mesh peer can serve the direct join
                // bundle within the discovery budget, fall back to an empty
                // bundle and proceed: the invitation signature is already
                // verified above, so the joiner still records local membership
                // (below) and publishes `MemberJoinedAt`. That keeps
                // `list_group_*` from 500ing with "node is not a member", and
                // the group key, contexts, and governance ops catch up via the
                // gossip `KeyDelivery` fallback and namespace sync once a peer
                // becomes reachable — no manual re-join. This follows the same
                // "advance locally regardless of transport readiness" invariant
                // the `MemberJoinedAt` publish further below already relies on.
                // Previously this was a hard `?`, so a join attempted before the
                // mesh formed aborted before writing the joiner's membership row
                // and left the node not-a-member (the reported symptom).
                // Built BEFORE the request, not after: the responder needs it to
                // name this joiner's account, and the deny-list gate it feeds
                // runs before any state is served. The same credential is
                // published with `MemberJoinedAt` further below.
                let joiner_credential_bytes = match crate::join_credential::build(
                    &datastore,
                    &namespace_id.into(),
                    &joiner_identity,
                ) {
                    Ok(credential) => borsh::to_vec(&*credential)?,
                    Err(e) => {
                        // Without one the responder cannot name us, and a
                        // responder on this version refuses rather than guess.
                        return Err(e.wrap_err(
                            "join: could not build the account credential the responder \
                             needs to authorize this join",
                        ));
                    }
                };

                let join_result = match node_client
                    .request_namespace_join(
                        namespace_id,
                        invitation_bytes,
                        joiner_identity,
                        joiner_credential_bytes,
                    )
                    .await
                {
                    Ok(bundle) => bundle,
                    Err(e) => {
                        warn!(
                            ?e,
                            ?group_id,
                            "direct namespace-join request found no reachable mesh peer; \
                             recording local membership and relying on gossip/sync catch-up"
                        );
                        JoinBundle::empty()
                    }
                };

                // Unwrap and store the group key.
                //
                // The join response wins over whatever the keyring already
                // holds, and that is a deliberate reversal (#3891). This used to
                // skip on "a key is already present", which reads as harmless
                // caution and is the step that makes a wrong key PERMANENT: the
                // response is authenticated and addressed to this joiner for
                // this group, so it is better evidence of the group's key than
                // an entry of unknown provenance that happens to be in the
                // keyring. Preferring the local one meant a key planted before
                // the join could never be corrected — the membership-driven pull
                // reports only *keyless* groups, so nothing re-drives a node
                // holding the wrong key, and a delivery may no longer replace a
                // held key (#3887). This was the last correction path and it was
                // declining to correct.
                if !join_result.has_key() {
                    warn!("join response contained no group key");
                } else {
                    let envelope: calimero_context_client::local_governance::KeyEnvelope =
                        borsh::from_slice(&join_result.key_envelope_bytes)
                            .map_err(|e| eyre::eyre!("failed to deserialize key envelope: {e}"))?;

                    let group_key = GroupKeyring::unwrap_for_recipient(
                        &sk,
                        &group_id.to_bytes(),
                        None,
                        &envelope,
                    )?;
                    let offered_key_id = GroupKeyring::key_id_for(&group_key);
                    let held_key_id = GroupKeyring::new(&datastore, group_id)
                        .load_current_key()?
                        .map(|(key_id, _)| key_id);

                    match join_key_action(held_key_id, offered_key_id) {
                        JoinKeyAction::AlreadyHeld => {
                            info!(?group_id, "join response carried the group key already held");
                        }
                        // Worth shouting about. Either something planted a key
                        // for this group before the join, or the group rotated
                        // and this node is behind. Both resolve the same way --
                        // take the authenticated one -- but an operator should
                        // see that a displacement happened.
                        JoinKeyAction::Displace { held_key_id } => {
                            warn!(
                                ?group_id,
                                held_key_id = %hex::encode(held_key_id),
                                adopted_key_id = %hex::encode(offered_key_id),
                                "the join response's group key differs from the one already held; \
                                 adopting the join response's key and displacing the local one, \
                                 which was not attested by this join"
                            );
                            let _ = crate::group_key_pull::adopt_pulled_group_key(
                                &datastore,
                                namespace_id.into(),
                                group_id,
                                &group_key,
                            )?;
                        }
                        JoinKeyAction::Seed => {
                            let _ = crate::group_key_pull::adopt_pulled_group_key(
                                &datastore,
                                namespace_id.into(),
                                group_id,
                                &group_key,
                            )?;
                            info!("received group key via direct join response");
                        }
                    }
                }

                // Issue #2256 / PR #2368: write the namespace's
                // `default_capabilities` from the join bundle BEFORE
                // applying the catch-up governance ops below.
                //
                // The catch-up batch contains the `MemberJoined` ops of
                // OTHER members who joined before us. Each one runs
                // `add_group_member` → `add_group_member_with_keys`,
                // which materializes that member's per-member
                // capability row by copying `default_capabilities` — but
                // ONLY if `default_capabilities` is already set in the
                // local store. If we set it afterwards (the previous
                // ordering), every member applied during catch-up was
                // recorded with NO per-member capability row, so a later
                // `MemberJoinedOpen` from them failed
                // `check_group_membership_path` with "no membership
                // path" (their `CAN_JOIN_OPEN_SUBGROUPS` bit was never
                // materialized on this node). Setting it first fixes
                // that for the catch-up apply AND the `sync_namespace`
                // pull below.
                //
                // A `DefaultCapabilitiesSet` op inside the catch-up
                // batch still wins: it is applied after this write and
                // overwrites the bundle value (authoritative-op-wins).
                // The `is_none()` guard keeps this a no-op when a prior
                // op already set the value.
                if CapabilitiesRepository::new(&datastore).default_capabilities(&group_id)?.is_none() {
                    CapabilitiesRepository::new(&datastore).set_default_capabilities(&group_id, join_result.default_capabilities, )?;
                }

                // Apply governance ops so the local DAG is up to date.
                //
                // Note on dropped divergence reports: the `divergence`
                // field of `NamespaceApplyOutcome::Applied` is not
                // routed to the reconcile-via-anchor path from here.
                // This is acceptable for the join-response replay
                // path specifically: the joiner is rebuilding state
                // from a snapshot the inviter assembled, so a
                // divergence between the snapshot and a signed
                // `MemberRemoved` / `MemberLeft` claim it carries
                // would indicate inviter-side inconsistency rather
                // than the partition-window scenario reconcile is
                // designed to heal. The next signed op the joiner
                // sees post-join takes the gossip-receive path,
                // which does route divergence to reconcile — so the
                // recovery path is not permanently closed. Wiring
                // reconcile in from here would require plumbing the
                // node-side `SyncManager` into the context crate,
                // which the layering prohibits.
                let mut any_applied = false;
                for op_bytes in &join_result.governance_ops {
                    if let Ok(op) = borsh::from_slice::<SignedNamespaceOp>(op_bytes) {
                        match context_client.apply_signed_namespace_op(op).await {
                            Ok(NamespaceApplyOutcome::Applied { .. }) => {
                                any_applied = true;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!(?e, "failed to apply governance op from join response");
                            }
                        }
                    }
                }
                // FSM notify after the batch — closes the gap where a
                // joiner that catches up exclusively via the direct join
                // response never tells the readiness FSM about its DAG
                // advance. Same pattern as the receive-path notifies in
                // `crates/node/src/handlers/network_event/namespace.rs`.
                if any_applied {
                    node_client.notify_namespace_op_applied(namespace_id);
                }

                // Pull any governance ops published during (or just before) the
                // join window that weren't in the join response snapshot.
                // Direct stream request — does not depend on gossip delivery.
                if let Err(e) = node_client.sync_namespace(namespace_id).await {
                    warn!(
                        ?e,
                        "failed to trigger post-join namespace governance pull (non-fatal)"
                    );
                }

                // Add the joiner as a direct member of the namespace. The
                // call reads `default_capabilities` from the local store
                // (populated before the catch-up apply above) and assigns
                // the bit set to the new member. Idempotent on the
                // *direct*-row check — the
                // inheritance-aware `check_group_membership` would
                // wrongly skip the add when the joiner already inherits
                // membership from a parent namespace, leaving them
                // without the direct row that subsequent direct lookups
                // (removal, capability writes, list_group_members) need.
                // The joiner's own account, taken from the credential it is about
                // to publish.
                let joiner_credential = crate::join_credential::build(
                    &datastore,
                    &namespace_id.into(),
                    &joiner_identity,
                )?;
                let joiner_account = joiner_credential.statement.account;

                // Bind the joiner's own key to that account locally, now, rather
                // than waiting for the `MemberJoined` op below to apply.
                //
                // The row written just below is keyed by the ACCOUNT, while every
                // later read resolves this node's KEY to whatever account it can
                // look up. Publishing the credential is what normally supplies
                // that link, so a joiner whose op does not apply locally — an
                // invitation whose inviter it cannot resolve offline, which is
                // every joiner that has not yet synced the inviter's binding —
                // ends up holding a row it cannot match itself against. It reads
                // as "not a member of the group it just joined", from its own
                // store, with the row sitting right there.
                //
                // Same fix, same reason, as the founder's binding at namespace
                // genesis: the node that mints a credential is the one node that
                // must not depend on an op round-trip to believe it.
                let bindings =
                    calimero_governance_store::AccountBindingRepository::new(&datastore);
                if let Err(rejected) = bindings.apply_link(
                    &namespace_id.into(),
                    &joiner_credential.genesis,
                    &joiner_credential.chain,
                    &joiner_credential.statement,
                )? {
                    warn!(
                        ?group_id,
                        %joiner_identity,
                        ?rejected,
                        "joiner's own device credential was refused locally; membership \
                         will not resolve until a peer's binding arrives"
                    );
                }
                if !MembershipRepository::new(&datastore)
                    .has_direct_member(&group_id, &joiner_account)?
                {
                    // Do not materialize a local row for an identity that may not
                    // re-enter this group. This is the last writer of a direct row
                    // that isn't already gated, and the apply path leans on that:
                    // `apply_member_joined` checks re-entry only for identities
                    // WITHOUT a row (so re-applying the `MemberJoinedAt` op — via
                    // gossip re-delivery, sync backfill, or replay — stays
                    // idempotent), which is sound precisely because no blocked
                    // identity can obtain a row out of band.
                    //
                    // A voluntary leaver returning with a freshly issued
                    // invitation passes here, which is the point — it is the same
                    // check `MemberJoined` apply will make in a moment. Someone an
                    // admin removed does not, and their join fails locally with a
                    // clear reason rather than stalling on peers that reject the
                    // op they were about to publish.
                    ReentryRepository::new(&datastore).require_invitation_admits(
                        &group_id,
                        &joiner_account,
                        invitation.invitation.invitation_nonce,
                    )?;
                    MembershipRepository::new(&datastore)
                        .add_member(&group_id, &joiner_account, role)?;
                } else {
                    info!(
                        ?group_id,
                        %joiner_identity,
                        "group member already recorded locally, skipping add_group_member"
                    );
                }

                // The joiner needs the group key to decrypt subsequent
                // group ops. Two delivery paths converge here:
                //
                //   1. Direct stream (`request_namespace_join` above).
                //      Authoritative when the served peer holds the key.
                //   2. Gossip `KeyDelivery` from any admin who applies our
                //      `MemberJoined` op below — Phase 9.1 already targets
                //      the recipient via `required_signers` so the admin
                //      knows whether *we* acked.
                //
                // If path 1 didn't deliver a key, we fall through to path 2
                // here and block `join_group` until either a `KeyDelivery`
                // for our identity arrives or `key_delivery_fallback_wait`
                // elapses. Subscribing BEFORE publishing MemberJoined
                // closes the race where an admin could see our op,
                // immediately publish KeyDelivery, and have us miss it
                // before subscribing.
                let needs_key_wait =
                    GroupKeyring::new(&datastore, group_id).load_current_key()?.is_none();
                let mut op_event_rx = if needs_key_wait {
                    Some(subscribe_op_events())
                } else {
                    None
                };

                // Announce membership on the namespace DAG. Apply + store it
                // locally regardless of transport readiness so a later
                // `MemberJoinedOpen` can causally parent onto this op; publish
                // is best-effort on top of that.
                // Built here rather than after key delivery: nothing in it publishes
                // or encrypts, so the joiner's account travels WITH the membership
                // it belongs to. That is what closes the window a separate
                // `AccountDeviceLinked` left open.
                let join_account = crate::join_credential::build(&datastore, &namespace_id.into(), &joiner_identity)?;

                // No endorsement, no join. The peer that served the exchange
                // either was not named in the invitation's `admitters` or was
                // never reached at all, and neither leaves anything worth
                // publishing: every peer refuses an unendorsed join at apply,
                // so emitting one would trade a clear failure here for a
                // membership that silently never materialises anywhere.
                //
                // This is the shape of the guarantee. `CAN_INVITE_MEMBERS` mints
                // invitations without being able to complete them, and that only
                // holds if reaching an admitter is a requirement rather than a
                // preference.
                let Some(endorsement_bytes) = join_result.admitter_endorsement_bytes.as_ref()
                else {
                    return Err(eyre::eyre!(
                        "join could not be endorsed: no admitter named by this invitation was \
                         reached, so the membership cannot be authorised"
                    ));
                };
                let admitter_endorsement = Box::new(
                    borsh::from_slice::<calimero_governance_types::AdmitterEndorsement>(
                        endorsement_bytes,
                    )
                    .map_err(|e| eyre::eyre!("admitter endorsement did not decode: {e}"))?,
                );

                let join_root = RootOp::MemberJoinedAt {
                    member: join_account.statement.account,
                    signed_invitation: invitation,
                    joined_at: now_secs,
                    account: join_account,
                };

                // Sealed when this node already holds the namespace key, which on
                // the ordinary path it does: the bundle above carried the key and
                // it was stored before we got here (see the unwrap near the top of
                // this handler). Sealing keeps off the namespace topic the one
                // thing a cleartext join tells every non-member — which account
                // joined which group, and when.
                //
                // `root_op_is_sealable` still says no for this variant, and that
                // is not a contradiction: it answers for the variant, which has
                // publishers that hold no key (a browser client signing offline
                // never does), and it has to answer the same on every node. This
                // asks the narrower question the publisher can actually answer,
                // "do I hold the key right now", so a keyed joiner seals and an
                // unkeyed one still joins.
                //
                // The unkeyed joiner is no longer answered with a cleartext
                // publish. Its key does arrive from a `KeyDelivery` an admin
                // publishes on SEEING this op, so it genuinely cannot seal for
                // itself -- but the admitter can, and since #3804 the joiner has
                // already reached one to get the endorsement above. So the
                // fallback is the relay below, not the disclosure (#3904).
                // Which key seals it depends on what this joiner was actually
                // given. A namespace-root invitation delivers the namespace key,
                // so the namespace-key seal applies. A SUBGROUP-targeted one
                // delivers that subgroup's key and never the namespace's — so
                // the namespace-key seal finds nothing, and before #3858 the
                // join went out in the clear, telling every peer on the
                // namespace topic which account joined which group and when.
                //
                // `seal_root_op_for_group_if_keyed` resolves the covering group
                // through `key_covering_group`, so a Restricted chain seals
                // under the subgroup and an Open chain under the namespace —
                // never under a key row nothing encrypts to (#3859).
                let seal_attempt = if group_id.to_bytes() == namespace_id {
                    calimero_governance_store::seal_root_op_if_keyed(
                        &datastore,
                        namespace_id.into(),
                        &join_root,
                    )
                } else {
                    calimero_governance_store::seal_root_op_for_group_if_keyed(
                        &datastore,
                        group_id,
                        &join_root,
                    )
                };
                let sealed_op = match seal_attempt {
                    Ok(Some(sealed)) => Some(sealed),
                    Ok(None) => None,
                    Err(e) => {
                        // Not a reason to publish in the clear. A seal this node
                        // could not perform is exactly the case the relay below
                        // handles, and the alternative is the disclosure.
                        warn!(?e, ?group_id, "could not seal the join locally; relaying it instead");
                        None
                    }
                };

                match sealed_op {
                    // Handed in rather than embedded: the endorsement rides the
                    // envelope, outside this node's signature, so it is attached
                    // after signing and before the local apply.
                    Some(member_joined_op) => {
                        match calimero_governance_store::sign_apply_and_publish_namespace_op_returning_op(
                            &datastore,
                            &node_client,
                            &ack_router,
                            namespace_id.into(),
                            &sk,
                            member_joined_op,
                            Some(admitter_endorsement),
                        )
                        .await
                        {
                            Ok((report, signed)) if report.acked_by.is_empty() => {
                                // Reached no peer; retry when a namespace peer next subscribes.
                                node_client.queue_membership_republish(namespace_id, signed);
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!(?e, "failed to apply/publish MemberJoined locally (non-fatal)")
                            }
                        }
                    }
                    // No key to seal under, so this node cannot publish the join
                    // itself without putting it on the namespace topic in the
                    // clear — telling every peer which account joined which
                    // group and when. Hand it to a keyholder instead, starting
                    // with the admitter that endorsed this very join, since that
                    // is the one peer already known reachable. The seal it
                    // applies carries the joiner's own signature inside, which is
                    // what peers check at apply, so relaying grants nothing
                    // (#3904).
                    //
                    // Applied locally first and NOT published: governance ops are
                    // locally authoritative, and a later `MemberJoinedOpen` needs
                    // this op on the local DAG to parent onto.
                    None => {
                        info!(
                            ?group_id,
                            "no key to seal this join under; relaying it to the admitter to be \
                             sealed and published"
                        );
                        let signed =
                            calimero_governance_store::sign_and_apply_namespace_op_without_publish(
                                &datastore,
                                &node_client,
                                namespace_id.into(),
                                &sk,
                                NamespaceOp::Root(join_root),
                                Some(admitter_endorsement),
                            )
                            .map_err(|e| {
                                // Fatal, unlike the publish path's warn above,
                                // because there is nothing to relay if the op was
                                // never signed. The reachable case is a
                                // SUBGROUP-targeted invitation whose subgroup key
                                // never arrived: the apply refuses that join in
                                // the clear (#3858), and it used to surface two
                                // steps later as a key-delivery timeout.
                                eyre::eyre!(
                                    "could not sign and apply this join locally, so there is \
                                     nothing to relay: {e:#}"
                                )
                            })?;

                        let signed_op_bytes = borsh::to_vec(&signed).map_err(|e| {
                            eyre::eyre!("could not encode the join for relay: {e}")
                        })?;

                        // An `Err` fails the join. Falling back to a cleartext
                        // publish is what this path removes, and a fallback that
                        // quietly re-opens the disclosure would make "sealed" and
                        // "leaked" the same silence.
                        node_client
                            .relay_sealed_join(
                                calimero_node_primitives::client::RelaySealedJoinParams {
                                    namespace_id,
                                    admitter_peer: join_result.admitter_peer,
                                    joiner_public_key: joiner_identity,
                                    signed_op_bytes,
                                },
                            )
                            .await
                            .map_err(|e| {
                                eyre::eyre!(
                                    "join could not be published: this node holds no key to seal \
                                     it under and no admitter would relay it, and publishing it \
                                     in the clear would disclose the membership: {e:#}"
                                )
                            })?;
                    }
                }

                if let Some(rx) = op_event_rx.as_mut() {
                    let deadline = Instant::now() + key_delivery_fallback_wait;
                    loop {
                        // Re-check the store on every iteration: the apply
                        // path emits the event AFTER `store_group_key`, so
                        // any successful unwrap is observable as an
                        // already-stored key by the time we get woken.
                        // This also catches races where an event was
                        // dropped via `RecvError::Lagged` before we got to
                        // it (broadcast channel is process-wide and other
                        // tasks may flood it).
                        //
                        // Soft-fail on transient store errors: a single
                        // failed read should not abort the join — the loop
                        // already retries on every event tick and the
                        // deadline branch handles permanent failure. This
                        // mirrors the `RecvError::Lagged` arm's "we'll
                        // observe it next tick" semantics.
                        match GroupKeyring::new(&datastore, group_id).load_current_key() {
                            Ok(Some(_)) => {
                                info!(
                                    ?group_id,
                                    "group key acquired via gossip KeyDelivery fallback"
                                );
                                break;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(
                                    ?group_id,
                                    ?e,
                                    "transient store error during KeyDelivery wait — retrying"
                                );
                            }
                        }
                        let now = Instant::now();
                        if now >= deadline {
                            // Phase 12 (#2237) deferred this from a typed
                            // `Err` to a `warn!` + Ok-no-key. Restoring the
                            // typed-error contract: callers must be able to
                            // distinguish "joined and ready" from "joined
                            // but unusable yet". Returning Err here means
                            // the admin endpoint surfaces a failure that
                            // clients can retry, instead of clients
                            // proceeding to write to a context whose
                            // group key has not yet arrived.
                            return Err(eyre::eyre!(
                                "KeyDelivery timed out for group {group_id:?}: \
                                 no group key arrived within {}s via the gossip fallback path; \
                                 join cannot proceed without a usable group key",
                                key_delivery_fallback_wait.as_secs()
                            ));
                        }
                        let remaining = deadline - now;
                        match tokio::time::timeout(remaining, rx.recv()).await {
                            Ok(Ok(OpEvent::GroupKeyDelivered {
                                group_id: g,
                                recipient,
                            })) if g == group_id.to_bytes() && recipient == joiner_identity => {
                                // Loop back to re-read the store — the
                                // emitter publishes AFTER store_group_key
                                // succeeded, so the next iteration's
                                // `load_current_group_key` will hit and
                                // break cleanly.
                                continue;
                            }
                            Ok(Ok(_)) => continue, // unrelated event
                            Ok(Err(RecvError::Lagged(_))) => {
                                // Broadcast capacity exceeded — relevant
                                // events may have been dropped. Re-check
                                // store; if the key arrived during the
                                // overflow window we'll observe it on the
                                // next iteration and break cleanly.
                                continue;
                            }
                            Ok(Err(RecvError::Closed)) => {
                                // The static `op_events::NOTIFIER` cannot
                                // be dropped at runtime today, so this
                                // branch is functionally unreachable. If
                                // a future refactor changes that, surface
                                // a typed Err for the same reason the
                                // deadline branch above does — joining
                                // without a usable group key would leave
                                // the caller in the "joined but unusable"
                                // condition this PR's typed-error contract
                                // is meant to prevent. `break` here would
                                // bypass the deadline check and fall
                                // through to `Ok(JoinGroupResponse)`;
                                // `continue` would tight-loop on a
                                // permanently-closed channel until the
                                // deadline fires. Returning Err is the
                                // only outcome consistent with the
                                // contract.
                                return Err(eyre::eyre!(
                                    "KeyDelivery channel closed before group key arrived for {group_id:?}: \
                                     join cannot proceed without a usable group key"
                                ));
                            }
                            Err(_) => continue, // timeout slice — outer deadline check handles exit
                        }
                    }
                }

                // Every device this account already certified belongs here too.
                // Pairing bound them wherever this node took part at the time, and
                // this namespace was not one of them - so without this the paired
                // device would silently never see it.
                //
                // Deliberately after the key wait: the binding is an encrypted
                // group op and the delivery is that same key wrapped, so neither is
                // possible without it. Nothing here can fail the join.
                let _bound = calimero_governance_store::bind_known_devices(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &namespace_id.into(),
                    &sk,
                )
                .await;

                // -------------------------------------------------------
                // Phase 3: Auto-join contexts from the response.
                // -------------------------------------------------------

                // Seed a local display-name hint from the invitation *only* if
                // nothing has synced yet — a replicated `GroupOp::GroupMetadataSet`
                // is authoritative and must not be clobbered by a possibly-stale
                // invitation value. Stamp with this node's identity / wall-clock
                // rather than the zero-value `Default` so the provenance fields
                // aren't misleading.
                if group_name.is_some()
                    && calimero_primitives::metadata::validate_metadata_payload(
                        group_name.as_deref(),
                        &std::collections::BTreeMap::new(),
                    )
                    .is_ok()
                    && MetadataRepository::new(&datastore).group_metadata(&group_id)?.is_none()
                {
                    MetadataRepository::new(&datastore).set_group(&group_id, &calimero_primitives::metadata::MetadataRecord {
                            name: group_name.clone(), data: std::collections::BTreeMap::new(), updated_at: calimero_governance_store::now_millis(), updated_by: joiner_identity,
                        }, )?;
                }

                let contexts = &join_result.context_ids;

                if let Some(meta) = MetaRepository::new(&datastore).load(&group_id)? {
                    if meta.auto_join {
                        info!(
                            ?group_id,
                            context_count = contexts.len(),
                            "auto-join: contexts from direct join response"
                        );
                        // Batch all ContextIdentity writes in a single handle to
                        // avoid per-context mutex acquisition overhead.
                        {
                            let mut handle = datastore.handle();
                            for context_id in contexts {
                                let ci_key =
                                    key::ContextIdentity::new(*context_id, joiner_identity);
                                if !handle.has(&ci_key)? {
                                    // Keyless membership marker — `joiner_identity`
                                    // is the node's namespace identity, whose key is
                                    // resolved live at read time rather than copied.
                                    handle.put(
                                        &ci_key,
                                        &calimero_store::types::ContextIdentity { private_key: None },
                                    )?;
                                }
                            }
                        }

                        // Register the context-under-group mapping
                        // (`ContextGroupRef`) directly from the bundle's
                        // `context_ids`. The bundle's `governance_ops`
                        // normally include a `ContextRegistered` op
                        // that would write the same mapping on apply,
                        // but the op list can be an incomplete snapshot
                        // — missing the op leaves the mapping unwritten
                        // and `get_group_for_context` returns `None`.
                        // Idempotent with the governance-op path.
                        for context_id in contexts {
                            if let Err(err) = calimero_governance_store::register_context_in_group(
                                &datastore, &group_id, context_id,
                            ) {
                                warn!(
                                    %context_id,
                                    ?err,
                                    "failed to register context under group during join"
                                );
                            }
                        }

                        for context_id in contexts {
                            let config = if !context_client.has_context(context_id)? {
                                let zero_app =
                                    calimero_primitives::application::ApplicationId::from(
                                        [0u8; 32],
                                    );
                                let app_id = join_result.application_id;
                                let resolved = if app_id != zero_app {
                                    Some(app_id)
                                } else {
                                    MetaRepository::new(&datastore).load(&group_id)?
                                        .map(|m| m.target.application_id)
                                        .filter(|id| *id != zero_app)
                                };
                                let svc_name =
                                    calimero_governance_store::get_context_service_name(&datastore, context_id)?;
                                Some(ContextConfigParams {
                                    application_id: resolved,
                                    application_revision: 0,
                                    members_revision: 0,
                                    service_name: svc_name,
                                })
                            } else {
                                None
                            };

                            if let Err(e) = context_client
                                .sync_context_config(*context_id, config)
                                .await
                            {
                                warn!(%context_id, ?e, "failed to sync context config");
                            }
                            if let Err(e) = node_client.subscribe(context_id).await {
                                warn!(%context_id, ?e, "failed to subscribe to context");
                            }
                            if let Err(e) = node_client.sync(Some(context_id), None).await {
                                warn!(%context_id, ?e, "failed to trigger context sync");
                            }
                        }
                    }
                }

                if let Err(e) = node_client.sync(None, None).await {
                    warn!(?e, "failed to trigger global sync after join");
                }

                info!(
                    ?group_id,
                    namespace_id = %hex::encode(namespace_id),
                    %joiner_identity,
                    "member joined group via direct request-response"
                );

                // Resolved after the join has applied, so a joiner that enrolled
                // as part of it reports the account it actually writes as rather
                // than the stand-in its key would have derived a moment earlier.
                let member_account = account_for_group(&datastore, &group_id)?;

                Ok(JoinGroupResponse {
                    group_id,
                    member_identity: joiner_identity,
                    member_account,
                })
            }
            .into_actor(self),
        )
    }
}

/// What to do with the group key a join response carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinKeyAction {
    /// Nothing held for this group: store it.
    Seed,
    /// The key held is the one offered. Storing it again is a no-op.
    AlreadyHeld,
    /// A DIFFERENT key is held. Take the join response's and displace it.
    Displace { held_key_id: [u8; 32] },
}

/// Decide between the key a join response carried and the one already held.
///
/// The join response wins, and that is a deliberate reversal (#3891). This
/// decision used to be "if any key is present, skip" — which reads as harmless
/// caution and is the step that makes a wrong key PERMANENT. The response is
/// authenticated and addressed to this joiner for this group, so it is better
/// evidence of the group's key than an entry of unknown provenance that happens
/// to be in the keyring.
///
/// It was also the last correction path left. The membership-driven pull reports
/// only *keyless* groups (`groups_member_but_keyless`), so nothing re-drives a
/// node holding the wrong key; and a delivery may no longer replace a held key
/// (#3887). So a key planted before the join could never be corrected, and this
/// function was the thing declining to correct it.
///
/// Split out from the handler so the decision is testable without mocking the
/// join transport: it is the security-relevant half, and the rest of that block
/// is unwrapping and storing.
fn join_key_action(held_key_id: Option<[u8; 32]>, offered_key_id: [u8; 32]) -> JoinKeyAction {
    match held_key_id {
        None => JoinKeyAction::Seed,
        Some(held) if held == offered_key_id => JoinKeyAction::AlreadyHeld,
        Some(held) => JoinKeyAction::Displace { held_key_id: held },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::{
        ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use calimero_governance_store::AccountBindingRepository;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::test_support::{actor, certify_device};

    /// A join response's key displaces an unattested held key (#3891).
    ///
    /// The reversal, and the case a poisoned node depends on. Before this,
    /// holding *any* key meant the join response's was skipped — so a key
    /// planted before the join stayed, and nothing else could correct it: the
    /// membership-driven pull enumerates only keyless groups, and #3887 stopped
    /// a delivery from replacing a held key. The join response is authenticated
    /// and addressed to this joiner for this group, so it is the better
    /// evidence.
    #[test]
    fn a_join_responses_key_displaces_a_different_held_key() {
        let planted = [0xAA; 32];
        let authentic = [0xBB; 32];
        assert_eq!(
            join_key_action(Some(planted), authentic),
            JoinKeyAction::Displace {
                held_key_id: planted
            },
            "the join response wins over a key of unknown provenance, and the displaced \
             id is carried out so the log can name it"
        );
    }

    /// Re-joining is idempotent and quiet.
    ///
    /// This is what keeps the reversal from being noisy: a retry, or a second
    /// join of a group whose key is already correct, must not report a
    /// displacement, because storing the same key again changes nothing.
    #[test]
    fn a_join_response_carrying_the_held_key_is_a_no_op() {
        let key_id = [0xCC; 32];
        assert_eq!(
            join_key_action(Some(key_id), key_id),
            JoinKeyAction::AlreadyHeld
        );
    }

    /// The ordinary first join: nothing held, so seed it.
    #[test]
    fn a_join_response_seeds_when_no_key_is_held() {
        assert_eq!(join_key_action(None, [0xDD; 32]), JoinKeyAction::Seed);
    }

    const APP: [u8; 32] = [0xD1; 32];
    const GROUP: [u8; 32] = [0xD2; 32];

    /// An invitation to `group`, signed the way `create_group_invitation` signs
    /// one. The inviter is nobody this node knows, which is the state a joiner
    /// is genuinely in: its `MemberJoinedAt` cannot apply until the inviter's
    /// binding syncs, and the auto-bind must not depend on that.
    fn an_invitation(group: ContextGroupId) -> SignedGroupOpenInvitation {
        let inviter_sk = PrivateKey::from([0xD3; 32]);
        let invitation = GroupInvitationFromAdmin {
            inviter_identity: SignerId::from(*inviter_sk.public_key().digest()),
            group_id: group,
            expiration_timestamp: 0,
            invitation_nonce: [0xD4; 32],
            invited_role: 1,
            admitters: vec![calimero_account::AccountId::from([0xD7; 32])],
        };
        let signature = inviter_sk
            .sign(&Sha256::digest(
                borsh::to_vec(&invitation).expect("borsh the invitation"),
            ))
            .expect("sign the invitation");
        SignedGroupOpenInvitation {
            invitation,
            inviter_signature: hex::encode(signature.to_bytes()),
            inviter_account: None,
            application_id: Some(APP),
            bytecode_id: Some([0xD5; 32]),
            admitter_addrs: Vec::new(),
        }
    }

    /// The sibling of the creation's auto-bind: a namespace joined after a
    /// pairing is one the paired device was never bound in, so without this the
    /// join succeeds and that device silently never sees the group.
    ///
    /// The devices come from the account namespace's registry, so this holds on
    /// any device of the account, not only the one that did the certifying.
    #[actix::test]
    async fn joining_a_namespace_carries_this_accounts_devices_into_it() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let group = ContextGroupId::from(GROUP);
        // The scope key a join normally takes from its bundle. Seeded because
        // there is no peer to serve one here, and the auto-bind runs after the
        // key wait precisely so it can wrap the delivery from it.
        let _key_id = GroupKeyring::new(&store, group)
            .store_key(&[0x42; 32])
            .expect("hold the scope key");
        let device = certify_device(&store, 0xD6, &[]);

        // A peer that answers the join, because the endorsement it carries is
        // what authorises the membership — the auto-bind asserted below runs
        // only on a join that got that far.
        let mut bundle = calimero_node_primitives::join_bundle::JoinBundle::empty();
        bundle.admitter_endorsement_bytes = Some(
            borsh::to_vec(
                &calimero_governance_types::AdmitterEndorsement::sign(
                    &PrivateKey::from([0xD3; 32]),
                    &GROUP,
                    &calimero_account::AccountId::from([0xD7; 32]),
                    &[0xD4; 32],
                )
                .expect("sign the endorsement"),
            )
            .expect("borsh the endorsement"),
        );

        let harness = actor::over_answering_joins(store.clone(), Some(bundle)).await;
        let _joined = harness
            .manager
            .send(JoinGroupRequest {
                invitation: an_invitation(group),
                group_name: None,
            })
            .await
            .expect("the manager answers")
            .expect("the join runs");

        assert!(
            AccountBindingRepository::new(&store)
                .is_device_linked(&group, device)
                .expect("read the bindings"),
            "the device this account already certified has to be bound in the \
             namespace the join just gained"
        );
    }
}
