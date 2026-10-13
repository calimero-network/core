//! `FollowGroupRequest` handler - follow a namespace this account was added to.
//!
//! The join without the invitation. An admin placed this node's account at the
//! namespace root with `MemberAdded`, so the row is the authority: an anchor
//! serves the bundle on it (`NamespaceFollowRequest`), and nothing is published
//! that admits anyone. No `MemberJoined*` op either, which leaves this node's
//! own key bound to no account on every peer, so the follow publishes its own
//! device link under the anchor's endorsement of the account.
//!
//! Otherwise `join_group.rs` step for step, through the helpers the two share:
//! seed the namespace if unknown, subscribe, ask, hold the key provisionally,
//! apply the bundle, pull, settle the key against the namespace's anchors.
//! Then the follow's own check, before anything is recorded for the account:
//! the synced state must hold a direct root row for this account, or the
//! follow is refused and what it seeded is taken down again.

use std::sync::Arc;
use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::{AccountId, AccountMemberEndorsement};
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::{FollowGroupRequest, FollowGroupResponse};
use calimero_context_client::local_governance::{AckRouter, KeyEnvelope};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::subscribe as subscribe_op_events;
use calimero_governance_store::{
    account_for_group, AccountBindingRepository, GroupKeyring, MembershipRepository,
    MetaRepository, NamespaceRepository,
};
use calimero_node_primitives::client::{NamespaceJoinExhausted, NodeClient};
use calimero_node_primitives::join_bundle::JoinBundle;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use tracing::{info, warn};

use super::join_group::{
    apply_join_bundle, auto_join_bundle_contexts, await_group_key, install_join_key,
    settle_join_key, JoinKey, KeyVoucher, ProvisionalKey,
};
use crate::account_namespace::AccountNamespaceChange;
use crate::error::ContextError;
use crate::ContextManager;

/// How long a follow waits for the namespace topic's mesh before asking, as a
/// join does.
const NAMESPACE_MESH_GRACE: Duration = Duration::from_secs(2);

/// The most keys a refused follow removes; it installs one.
const MAX_KEYS_TORN_DOWN: usize = 8;

impl Handler<FollowGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <FollowGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        FollowGroupRequest {
            namespace_id,
            peer_addrs,
            application_id,
            bytecode_id,
        }: FollowGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Read before the identity call below notes participation, so a refused
        // follow knows whether that note was its own to take back.
        let was_participating =
            match NamespaceRepository::new(&self.datastore).participates_in(&namespace_id) {
                Ok(participating) => participating,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
        let (resolved, identity, sk_bytes) =
            match self.get_or_create_namespace_identity(&namespace_id) {
                Ok(resolved) => resolved,
                Err(err) => {
                    return ActorResponse::reply(Err(
                        err.wrap_err("failed to resolve namespace identity for follow")
                    ));
                }
            };
        // A known subgroup resolves to its namespace. The bundle a follow is
        // served is the root's, and the row it rests on is a root row.
        if resolved != namespace_id {
            return ActorResponse::reply(Err(eyre::eyre!(
                "{namespace_id:?} is a subgroup of {resolved:?}; a follow names a namespace root"
            )));
        }

        let follow = Follow {
            datastore: self.datastore.clone(),
            node_client: self.node_client.clone(),
            context_client: self.context_client.clone(),
            ack_router: Arc::clone(&self.ack_router),
            key_wait: self.config.key_delivery_fallback_wait,
            group_id: namespace_id,
            identity,
            sk: PrivateKey::from(sk_bytes),
            peer_addrs,
            application_id,
            bytecode_id,
            was_participating,
        };
        ActorResponse::r#async(async move { follow.run().await }.into_actor(self))
    }
}

/// One follow, with everything it needs off the actor.
struct Follow {
    datastore: Store,
    node_client: NodeClient,
    context_client: ContextClient,
    ack_router: Arc<AckRouter>,
    key_wait: Duration,
    group_id: ContextGroupId,
    identity: PublicKey,
    sk: PrivateKey,
    peer_addrs: Vec<String>,
    application_id: ApplicationId,
    bytecode_id: Option<[u8; 32]>,
    was_participating: bool,
}

/// What a follow left behind before it knew whether it would keep it.
#[derive(Clone, Copy)]
struct Seeded {
    /// The meta row was written by this follow.
    meta: bool,
    /// A key of the namespace was held before this follow.
    key_held_before: bool,
}

/// A served bundle, applied, with its key settled and this account confirmed a
/// member at the namespace root.
struct Admitted {
    bundle: JoinBundle,
}

impl Follow {
    fn refused(&self, reason: &'static str) -> eyre::Report {
        ContextError::FollowRefused {
            namespace_id: hex::encode(self.group_id.to_bytes()),
            reason,
        }
        .into()
    }

    async fn run(self) -> eyre::Result<FollowGroupResponse> {
        let datastore = &self.datastore;
        let group_id = self.group_id;

        // The credential the responder names this account by, and the one the
        // link published below carries. Idempotent, like the join's.
        let credential = crate::join_credential::build(datastore, &group_id, &self.identity)
            .map_err(|e| {
                e.wrap_err(
                    "follow: could not build the account credential the responder needs to \
                     name this account",
                )
            })?;
        let account = credential.statement.account;

        // Already in, keyed and bound: nothing to ask anyone.
        if self.already_follows(account)? {
            info!(?group_id, "namespace already followed; nothing to do");
            return self.response(true);
        }

        let seeded = self.seed()?;
        let admitted = match self.admit(&credential, account).await {
            Ok(admitted) => admitted,
            Err(err) => {
                self.tear_down(seeded).await;
                return Err(err);
            }
        };

        // Bind this node's own key here now, rather than when its link op
        // applies: the join does the same, for the same reason (a row keyed by
        // the account that this node's key does not yet resolve to).
        if let Err(rejected) = AccountBindingRepository::new(datastore).apply_link(
            &group_id,
            &credential.genesis,
            &credential.chain,
            &credential.statement,
            calimero_governance_store::JOIN_SCOPE_EPOCH,
        )? {
            warn!(
                ?group_id,
                identity = %self.identity,
                ?rejected,
                "follower's own device credential was refused locally"
            );
        }

        let key_ready = self.await_key().await?;

        if key_ready {
            // Without its own link logged this node's key resolves to nothing on
            // its peers, so the follow is not reported done. Membership, key and
            // local binding stay: the next follow, which runs again because the
            // marker is absent, completes it.
            if let Err(err) = self.link_own_device(&credential, &admitted.bundle).await {
                warn!(?group_id, %err, "could not log this node's own device link");
                return Err(self.refused("device binding not published"));
            }
            // Every device this account already certified belongs here too, as
            // after a join. After the own link: their links are endorsed by this
            // node's key, which peers resolve only once that link has applied.
            let _bound = calimero_governance_store::bind_known_devices(
                datastore,
                &self.node_client,
                &self.ack_router,
                &group_id,
                &self.sk,
            )
            .await;
        }

        crate::account_namespace::announce(
            datastore,
            &self.node_client,
            &self.ack_router,
            group_id,
            AccountNamespaceChange::Gained,
            "follow_group",
        )
        .await;

        auto_join_bundle_contexts(
            datastore,
            &self.context_client,
            &self.node_client,
            group_id,
            self.identity,
            &admitted.bundle,
        )
        .await?;

        if let Err(e) = self.node_client.sync(None, None).await {
            warn!(?e, "failed to trigger global sync after follow");
        }

        info!(
            ?group_id,
            identity = %self.identity,
            key_ready,
            "followed a namespace this account was added to"
        );
        self.response(key_ready)
    }

    /// A direct root row for `account`, this node's key bound to it, a key held
    /// and its own device link logged: a follow already completed here. The
    /// local binding alone is not enough, since it tells peers nothing.
    fn already_follows(&self, account: AccountId) -> eyre::Result<bool> {
        let datastore = &self.datastore;
        let group_id = self.group_id;
        Ok(MetaRepository::new(datastore).load(&group_id)?.is_some()
            && MembershipRepository::new(datastore).has_direct_member(&group_id, &account)?
            && calimero_governance_store::member_account_in_namespace(
                datastore,
                &group_id,
                &self.identity,
            )? == Some(account)
            && GroupKeyring::new(datastore, group_id)
                .load_current_key()?
                .is_some()
            && calimero_governance_store::own_link_logged(datastore, &group_id, &self.identity)?)
    }

    /// Seed the namespace's meta when it is unknown here, as the join does from
    /// its invitation: a placeholder admin (governance names the real one) and
    /// the application the caller named, which `compute_group_state_hash` reads.
    fn seed(&self) -> eyre::Result<Seeded> {
        let datastore = &self.datastore;
        let group_id = self.group_id;
        let key_held_before = GroupKeyring::new(datastore, group_id)
            .load_current_key()?
            .is_some();
        if MetaRepository::new(datastore).load(&group_id)?.is_some() {
            return Ok(Seeded {
                meta: false,
                key_held_before,
            });
        }
        let placeholder = calimero_governance_store::placeholder_admin_identity();
        let meta = calimero_store::key::GroupMetaValue {
            target: calimero_store::key::GroupTarget {
                application_id: self.application_id,
                // Zero self-heals from the group's next `ContextRegistered` or
                // target op, as an invitation without one does.
                bytecode_id: self.bytecode_id.unwrap_or_default(),
                ..Default::default()
            },
            admin_identity: placeholder,
            owner_identity: placeholder,
            migration: None,
            created_at: 0,
            auto_join: true,
        };
        MetaRepository::new(datastore).save(&group_id, &meta)?;
        Ok(Seeded {
            meta: true,
            key_held_before,
        })
    }

    /// Ask for the bundle, apply it and settle its key; then require that the
    /// synced state holds this account's direct root row.
    async fn admit(
        &self,
        credential: &calimero_context_client::local_governance::JoinAccountCredential,
        account: AccountId,
    ) -> eyre::Result<Admitted> {
        let datastore = &self.datastore;
        let group_id = self.group_id;
        let namespace_id = group_id.to_bytes();

        let _ = self.node_client.subscribe_namespace(namespace_id).await;
        tokio::time::sleep(NAMESPACE_MESH_GRACE).await;

        // Unlike the join, no fallback to an empty bundle: the join can still
        // publish a membership the invitation authorises, while a follow has
        // nothing to publish until an anchor has served it.
        let bundle = match self
            .node_client
            .request_namespace_follow(
                namespace_id,
                self.peer_addrs.clone(),
                self.identity,
                borsh::to_vec(credential)?,
            )
            .await
        {
            Ok(bundle) => bundle,
            Err(err) => {
                let Some(reason) = follow_refusal(&err) else {
                    return Err(err.wrap_err("follow: no peer served the namespace"));
                };
                warn!(?group_id, %err, reason, "every peer asked declined the follow");
                return Err(self.refused(reason));
            }
        };
        if !bundle.has_key() {
            // The requester skips keyless answers, so this is a responder that
            // answered outside the protocol.
            return Err(self.refused("no anchor answered"));
        }

        let envelope: KeyEnvelope = borsh::from_slice(&bundle.key_envelope_bytes)
            .map_err(|e| eyre::eyre!("failed to deserialize key envelope: {e}"))?;
        let mut provisional = ProvisionalKey::new(datastore.clone(), group_id);
        let mut key_state = install_join_key(
            datastore,
            namespace_id,
            group_id,
            &self.sk,
            &envelope,
            KeyVoucher::NamespaceAnchors,
        )?;
        if let JoinKey::Provisional(key_id) = key_state {
            provisional.arm(key_id);
        }

        apply_join_bundle(
            datastore,
            &self.context_client,
            &self.node_client,
            namespace_id,
            group_id,
            &bundle,
        )
        .await?;

        // The bundle's ops are what make the sender recognisable as an anchor.
        key_state = settle_join_key(
            datastore,
            namespace_id,
            group_id,
            &self.sk,
            &envelope,
            KeyVoucher::NamespaceAnchors,
            key_state,
        )?;
        provisional.disarm();
        if key_state == JoinKey::Refused {
            warn!(
                ?group_id,
                sender = %envelope.sender,
                "no key kept from the follow response: its sender is not an anchor of the \
                 namespace"
            );
        }

        if !MembershipRepository::new(datastore).has_direct_member(&group_id, &account)? {
            return Err(self.refused("not a member"));
        }
        Ok(Admitted { bundle })
    }

    /// Whether a usable key is held, waiting up to the join's deadline for one
    /// to land when it is not.
    async fn await_key(&self) -> eyre::Result<bool> {
        let keyring = GroupKeyring::new(&self.datastore, self.group_id);
        if keyring.load_current_key()?.is_some() {
            return Ok(true);
        }
        let mut rx = subscribe_op_events();
        let ready = await_group_key(
            &self.datastore,
            self.group_id,
            self.identity,
            &mut rx,
            self.key_wait,
        )
        .await?;
        if !ready {
            warn!(
                group_id = ?self.group_id,
                waited_secs = self.key_wait.as_secs(),
                "followed, but no group key arrived; it is pending"
            );
        }
        Ok(ready)
    }

    /// Publish this node's own device link, endorsed by the anchor that served
    /// the follow, and mark it logged. An `Err` means nothing was logged, so the
    /// follow is not complete: the marker stays absent and the next follow runs
    /// again and asks for a fresh endorsement.
    async fn link_own_device(
        &self,
        credential: &calimero_context_client::local_governance::JoinAccountCredential,
        bundle: &JoinBundle,
    ) -> eyre::Result<()> {
        let bytes = bundle
            .admitter_endorsement_bytes
            .as_deref()
            .ok_or_else(|| eyre::eyre!("the anchor sent no endorsement of this account"))?;
        let endorsement = borsh::from_slice::<AccountMemberEndorsement>(bytes)
            .map_err(|err| eyre::eyre!("the anchor's endorsement did not decode: {err}"))?;
        calimero_governance_store::link_own_device(
            &self.datastore,
            &self.node_client,
            &self.ack_router,
            &self.group_id,
            &self.sk,
            credential,
            endorsement,
        )
        .await
    }

    /// Take down what a follow seeded for a namespace it did not get into: the
    /// meta, the participation it noted, the subscription and any key it
    /// installed. Ops the bundle applied stay in the log; nothing reads them
    /// without the meta, and a later follow or join applies them again as no-ops.
    async fn tear_down(&self, seeded: Seeded) {
        if !seeded.meta {
            return;
        }
        let datastore = &self.datastore;
        let group_id = self.group_id;
        if let Err(err) = MetaRepository::new(datastore).delete(&group_id) {
            warn!(?group_id, %err, "could not remove the meta a refused follow seeded");
        }
        if !seeded.key_held_before {
            let keyring = GroupKeyring::new(datastore, group_id);
            for _ in 0..MAX_KEYS_TORN_DOWN {
                match keyring.load_current_key() {
                    Ok(Some((key_id, _))) => {
                        if let Err(err) = keyring.delete_key_by_id(&key_id) {
                            warn!(?group_id, %err, "could not remove a refused follow's key");
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        warn!(?group_id, %err, "could not read a refused follow's keys");
                        break;
                    }
                }
            }
        }
        if !self.was_participating {
            if let Err(err) = NamespaceRepository::new(datastore).forget_participation(&group_id) {
                warn!(?group_id, %err, "could not drop a refused follow's participation");
            }
            if let Err(err) = self
                .node_client
                .unsubscribe_namespace(group_id.to_bytes())
                .await
            {
                warn!(?group_id, %err, "could not unsubscribe a refused follow");
            }
        }
    }

    fn response(&self, key_ready: bool) -> eyre::Result<FollowGroupResponse> {
        Ok(FollowGroupResponse {
            namespace_id: self.group_id,
            member_identity: self.identity,
            member_account: account_for_group(&self.datastore, &self.group_id)?,
            key_ready,
        })
    }
}

/// The refusal an exhausted follow amounts to, counted over distinct peers.
///
/// A membership refusal stands only when every peer that declined gave one: a
/// peer that answered keyless had found the account a member (it checks that
/// first), so beside it a refusal is a peer behind on governance, and the
/// outcome is that no anchor answered. Among membership refusals the removal
/// wins, being the more specific.
fn follow_refusal(err: &eyre::Report) -> Option<&'static str> {
    let exhausted = err.downcast_ref::<NamespaceJoinExhausted>()?;
    let peers = exhausted.rejected_peers;
    if peers == 0 {
        return None;
    }
    let count = |reason: &str| exhausted.refusals.iter().filter(|r| *r == reason).count();
    let (removed, not_member) = (count("removed"), count("not a member"));
    let membership = removed + not_member;
    if membership == peers {
        Some(if removed > 0 {
            "removed"
        } else {
            "not a member"
        })
    } else if exhausted.unusable_answers > 0 && exhausted.unusable_answers + membership == peers {
        Some("no anchor answered")
    } else if exhausted.opaque_errors == peers {
        Some("peer does not support follow")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_governance_store::{AccountNamespaceSet, NodeDeviceRepository};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_store::db::InMemoryDB;

    use super::*;
    use crate::handlers::ensure_account_namespace::ensure_account_namespace;
    use crate::test_support::{actor, certify_device, enrol};

    const APP: [u8; 32] = [0xF1; 32];
    const NS: [u8; 32] = [0xF2; 32];
    const KEY: [u8; 32] = [0x99; 32];

    /// A node whose account an admin may have added to `NS`, before it has
    /// followed: its identity minted but noted nowhere, and nothing of the
    /// namespace held. The anchor is an admin of `NS` this node can resolve once
    /// the bundle has applied, which in these tests it already can.
    struct World {
        store: Store,
        group: ContextGroupId,
        follower: PublicKey,
        account: AccountId,
        anchor: PrivateKey,
    }

    fn world(seated: bool) -> World {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let group = ContextGroupId::from(NS);
        let account = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("hold an account root, as an initialised node does")
            .account();
        let follower = NamespaceRepository::new(&store)
            .provision_node_identity()
            .expect("the node's signing identity");
        let anchor = PrivateKey::from([0xF4; 32]);
        let anchor_account = enrol(&store, &group, &anchor.public_key());
        let members = MembershipRepository::new(&store);
        members
            .add_member(&group, &anchor_account, GroupMemberRole::Admin)
            .expect("the anchor's row");
        if seated {
            members
                .add_member(&group, &account, GroupMemberRole::Member)
                .expect("the admin's MemberAdded of this account");
        }
        World {
            store,
            group,
            follower,
            account,
            anchor,
        }
    }

    impl World {
        /// What an anchor answers a follow with: the namespace key wrapped by
        /// `sender` for this node, and the anchor's endorsement of its account.
        fn bundle_from(&self, sender: &PrivateKey) -> JoinBundle {
            let envelope = GroupKeyring::wrap_for_member(sender, &self.follower, &NS, &KEY)
                .expect("wrap the key");
            let mut bundle = JoinBundle::empty();
            bundle.key_envelope_bytes = borsh::to_vec(&envelope).expect("borsh the envelope");
            bundle.admitter_endorsement_bytes = Some(
                borsh::to_vec(
                    &AccountMemberEndorsement::sign(&self.anchor, self.account)
                        .expect("the anchor vouches"),
                )
                .expect("borsh the endorsement"),
            );
            bundle
        }

        fn request(&self) -> FollowGroupRequest {
            FollowGroupRequest {
                namespace_id: self.group,
                peer_addrs: Vec::new(),
                application_id: ApplicationId::from(APP),
                bytecode_id: None,
            }
        }

        fn held(&self) -> Option<[u8; 32]> {
            GroupKeyring::new(&self.store, self.group)
                .load_current_key()
                .expect("read the keyring")
                .map(|(_id, key)| key)
        }
    }

    async fn follow(harness: &actor::Harness, world: &World) -> eyre::Result<FollowGroupResponse> {
        harness
            .manager
            .send(world.request())
            .await
            .expect("the manager answers")
    }

    /// A follow takes the anchor's key, and reports itself ready.
    #[actix::test]
    async fn a_follow_installs_the_anchors_key_and_reports_it_ready() {
        let w = world(true);
        let harness =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&w.anchor))).await;

        let followed = follow(&harness, &w).await.expect("the follow completes");

        assert!(followed.key_ready);
        assert_eq!(followed.namespace_id, w.group);
        assert_eq!(followed.member_identity, w.follower);
        assert_eq!(followed.member_account, w.account);
        assert_eq!(w.held(), Some(KEY));
        assert_eq!(
            calimero_governance_store::member_account_in_namespace(&w.store, &w.group, &w.follower)
                .expect("resolve the key"),
            Some(w.account),
            "this node's key resolves to its account here"
        );
    }

    /// The sibling of the join's announcement: the account's other devices learn
    /// of a namespace it followed into.
    #[actix::test]
    async fn a_follow_records_the_namespace_in_the_account_namespace() {
        let w = world(true);
        let harness =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&w.anchor))).await;
        let account_namespace = ensure_account_namespace(&w.store, &harness.context_client)
            .await
            .expect("the ensure runs")
            .expect("the holder creates its account namespace");

        let _followed = follow(&harness, &w).await.expect("the follow completes");

        assert!(
            AccountNamespaceSet::new(&w.store, account_namespace)
                .contains(w.group)
                .expect("read the set")
                .is_some(),
            "the namespace the follow gained has to reach the account's other devices"
        );
    }

    /// The sibling of the join's auto-bind: a device the account certified
    /// before the follow is carried into the namespace it followed.
    #[actix::test]
    async fn a_follow_carries_this_accounts_devices_into_the_namespace() {
        let w = world(true);
        let device = certify_device(&w.store, 0xD6, &[]);
        let harness =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&w.anchor))).await;

        let _followed = follow(&harness, &w).await.expect("the follow completes");

        assert!(
            AccountBindingRepository::new(&w.store)
                .is_device_linked(&w.group, device)
                .expect("read the bindings"),
            "the device this account already certified has to be bound in the namespace \
             the follow gained"
        );
    }

    /// A namespace whose root holds no row for this account is refused as
    /// `not a member` once synced, and the follow leaves nothing behind: no meta,
    /// no key, no participation, no subscription.
    #[actix::test]
    async fn a_follow_without_a_root_row_is_refused_and_leaves_nothing() {
        let w = world(false);
        let mut harness =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&w.anchor))).await;

        let err = follow(&harness, &w).await.expect_err("no row, no follow");

        assert!(
            matches!(
                err.downcast_ref::<ContextError>(),
                Some(ContextError::FollowRefused {
                    reason: "not a member",
                    ..
                })
            ),
            "got: {err:#}"
        );
        assert!(
            MetaRepository::new(&w.store)
                .load(&w.group)
                .expect("read the meta")
                .is_none(),
            "the meta the follow seeded is taken down"
        );
        assert_eq!(w.held(), None, "the key it installed is taken down");
        assert!(
            !NamespaceRepository::new(&w.store)
                .participates_in(&w.group)
                .expect("read the participation"),
            "the participation it noted is taken back"
        );
        let topic = format!("ns/{}", hex::encode(NS));
        assert!(
            !harness.live_topics().contains(&topic),
            "the namespace topic is left again: {:?}",
            harness.live_topics()
        );
        assert!(
            harness.subscribed().contains(&topic),
            "control: the follow did subscribe before it was refused"
        );
    }

    /// A second follow of a namespace already followed asks nobody and reports
    /// the key it holds: it completes on a node no peer answers.
    #[actix::test]
    async fn a_second_follow_is_a_no_op_reporting_the_held_key() {
        let w = world(true);
        let answered =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&w.anchor))).await;
        let _first = follow(&answered, &w)
            .await
            .expect("the first follow completes");
        drop(answered);
        let unanswered = actor::over(w.store.clone()).await;

        let second = follow(&unanswered, &w)
            .await
            .expect("the second follow asks nobody, so nobody need answer");

        assert!(second.key_ready);
        assert_eq!(second.member_account, w.account);
        assert_eq!(w.held(), Some(KEY));
    }

    /// The peer that answered is any node on the namespace topic; only an anchor
    /// of the namespace may hand this node its key. Held provisionally while the
    /// bundle applies, it is dropped once the applied state shows the sender is
    /// none.
    #[actix::test]
    async fn a_key_from_a_sender_that_is_no_anchor_is_dropped_after_apply() {
        let w = world(true);
        let member = PrivateKey::from([0xF5; 32]);
        let member_account = enrol(&w.store, &w.group, &member.public_key());
        MembershipRepository::new(&w.store)
            .add_member(&w.group, &member_account, GroupMemberRole::Member)
            .expect("a plain member's row");
        let harness =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&member))).await;

        let followed = follow(&harness, &w)
            .await
            .expect("the follow is recorded; only the key is pending");

        assert!(!followed.key_ready, "no key was kept, so none is ready");
        assert_eq!(w.held(), None, "the plain member's key is not installed");
    }

    /// The follow logs its own `AccountDeviceLinked`, signed by this node and
    /// carrying its own certificate under the anchor's endorsement, and that op
    /// alone binds this node's key on a peer that knew nothing of it.
    #[actix::test]
    async fn a_follow_logs_its_own_link_and_a_peer_binds_its_key_from_it() {
        use calimero_context_client::local_governance::{GroupOp, NamespaceOp};
        use calimero_governance_store::NamespaceOpLogService;

        let w = world(true);
        let harness =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&w.anchor))).await;
        let _followed = follow(&harness, &w).await.expect("the follow completes");

        let logged = NamespaceOpLogService::new(&w.store, NS.into())
            .collect_signed_group_ops_for_group(NS)
            .expect("read the op log");
        let own_link = logged
            .into_iter()
            .map(|entry| entry.signed_op)
            .find(|op| {
                op.signer == w.follower
                    && matches!(
                        &op.op,
                        NamespaceOp::Group { encrypted, .. }
                            if matches!(
                                GroupKeyring::decrypt_op(&KEY, encrypted),
                                Ok(GroupOp::AccountDeviceLinked { .. })
                            )
                    )
            })
            .expect("the follow logged a device link signed by this node");
        let NamespaceOp::Group { encrypted, .. } = &own_link.op else {
            unreachable!("matched as a group op above");
        };
        let Ok(GroupOp::AccountDeviceLinked {
            cert, endorsement, ..
        }) = GroupKeyring::decrypt_op(&KEY, encrypted)
        else {
            unreachable!("matched as a device link above");
        };
        assert_eq!(cert.sign_pk, w.follower, "its own certificate");
        assert_eq!(cert.account, w.account);
        assert_eq!(
            endorsement.member,
            w.anchor.public_key(),
            "the anchor's endorsement"
        );
        assert_eq!(endorsement.account, w.account);

        // A peer: the anchor bound, the admin's row for this account, the key,
        // and no binding for this node's key.
        let peer = Store::new(Arc::new(InMemoryDB::owned()));
        let anchor_account = enrol(&peer, &w.group, &w.anchor.public_key());
        let members = MembershipRepository::new(&peer);
        members
            .add_member(&w.group, &anchor_account, GroupMemberRole::Admin)
            .expect("the anchor's row");
        members
            .add_member(&w.group, &w.account, GroupMemberRole::Member)
            .expect("the admin's MemberAdded");
        let _key_id = GroupKeyring::new(&peer, w.group)
            .store_key(&KEY)
            .expect("the peer holds the key");
        assert_eq!(
            calimero_governance_store::member_account_in_namespace(&peer, &w.group, &w.follower)
                .expect("resolve the key"),
            None,
            "control: the peer cannot resolve this node's key before the link"
        );

        calimero_governance_store::apply_signed_namespace_op(&peer, &own_link)
            .expect("the peer applies the link");

        assert_eq!(
            calimero_governance_store::member_account_in_namespace(&peer, &w.group, &w.follower)
                .expect("resolve the key"),
            Some(w.account),
            "the peer resolves this node's key to its account"
        );
    }

    /// A follow whose own link cannot be logged is not reported done, and the
    /// next follow is not a no-op: it asks again and completes.
    #[actix::test]
    async fn a_follow_without_an_endorsement_is_not_done_and_the_next_one_completes_it() {
        let w = world(true);
        let mut unendorsed = w.bundle_from(&w.anchor);
        unendorsed.admitter_endorsement_bytes = None;
        let first = actor::over_answering_joins(w.store.clone(), Some(unendorsed)).await;

        let err = follow(&first, &w)
            .await
            .expect_err("no endorsement, no own link, no completed follow");
        assert!(
            matches!(
                err.downcast_ref::<ContextError>(),
                Some(ContextError::FollowRefused {
                    reason: "device binding not published",
                    ..
                })
            ),
            "got: {err:#}"
        );
        assert!(
            !calimero_governance_store::own_link_logged(&w.store, &w.group, &w.follower)
                .expect("read the marker")
        );
        drop(first);

        let second =
            actor::over_answering_joins(w.store.clone(), Some(w.bundle_from(&w.anchor))).await;
        let followed = follow(&second, &w)
            .await
            .expect("the retry asks again and completes");

        assert!(followed.key_ready);
        assert!(
            calimero_governance_store::own_link_logged(&w.store, &w.group, &w.follower)
                .expect("read the marker"),
            "the retry logged the link the first follow could not"
        );
    }

    fn exhausted(
        refusals: &[&str],
        unusable_answers: usize,
        opaque_errors: usize,
        rejected_peers: usize,
    ) -> eyre::Report {
        NamespaceJoinExhausted {
            label: "follow",
            attempts: 32,
            last_rejection: None,
            last_connect_err: None,
            rejected_peers,
            refusals: refusals.iter().map(|r| (*r).to_owned()).collect(),
            unusable_answers,
            opaque_errors,
        }
        .into()
    }

    /// How an exhausted follow reads to its caller.
    #[test]
    fn an_exhausted_follow_is_named_by_what_every_peer_said() {
        assert_eq!(
            follow_refusal(&exhausted(&["not a member", "removed"], 0, 0, 2)),
            Some("removed")
        );
        assert_eq!(
            follow_refusal(&exhausted(&["not a member", "not a member"], 0, 0, 2)),
            Some("not a member")
        );
        assert_eq!(
            follow_refusal(&exhausted(&["removed"], 1, 0, 2)),
            Some("no anchor answered"),
            "a peer that served keyless found the account a member, so a lone \
             refusal beside it does not decide"
        );
        assert_eq!(
            follow_refusal(&exhausted(&["not a member"], 0, 0, 2)),
            None,
            "one peer's word with another peer unaccounted for is passed through"
        );
        assert_eq!(
            follow_refusal(&exhausted(&[], 3, 0, 3)),
            Some("no anchor answered")
        );
        assert_eq!(
            follow_refusal(&exhausted(&[], 0, 2, 2)),
            Some("peer does not support follow")
        );
        assert_eq!(
            follow_refusal(&exhausted(&[], 1, 1, 2)),
            None,
            "a mix says nothing typed; the exhaustion is passed through"
        );
        assert_eq!(follow_refusal(&eyre::eyre!("no mesh peers")), None);
    }
}
