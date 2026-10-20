//! Following a namespace again with no invitation, one the user removed from
//! this node while the account stayed a member; peers still judge membership.

use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::AccountId;
use calimero_context_client::group::{JoinGroupResponse, RejoinNamespaceRequest};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::{notify, OpEvent};
use calimero_governance_store::{
    member_account_in_namespace, AccountNamespaceSet, ForgottenNamespace, ForgottenNamespaces,
    GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::NamespaceParticipation;
use calimero_store::Store;
use eyre::{OptionExt, Result as EyreResult};
use tracing::warn;

use crate::account_follow::{reaches, require_reach};
use crate::account_namespace::account_is_member;
use crate::error::ContextError;
use crate::handlers::follow_namespace::{follow, follow_marked};
use crate::ContextManager;

const LANDING_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 30 }); // bounds one restore request
const LANDING_POLL: Duration = Duration::from_millis(100);

impl Handler<RejoinNamespaceRequest> for ContextManager {
    type Result = ActorResponse<Self, <RejoinNamespaceRequest as Message>::Result>;

    fn handle(
        &mut self,
        RejoinNamespaceRequest { namespace_id }: RejoinNamespaceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();
        let node_client = self.node_client.clone();
        ActorResponse::r#async(
            async move { rejoin(&store, &node_client, namespace_id).await }.into_actor(self),
        )
    }
}

async fn rejoin(
    store: &Store,
    node_client: &NodeClient,
    namespace: ContextGroupId,
) -> EyreResult<JoinGroupResponse> {
    let (member_identity, _) = match ForgottenNamespaces::new(store).get(&namespace)? {
        Some(removed) => {
            require_account_still_holds(store, namespace, removed.application_id)?;
            restore(store, node_client, removed).await?
        }
        None if account_is_member(store, namespace)? => {
            require_reach(store, &namespace)?;
            follow(store, node_client, &namespace).await?
        }
        None => {
            return Err(ContextError::NamespaceNotFound {
                namespace_id: hex::encode(namespace.to_bytes()),
            }
            .into())
        }
    };

    let devices = NodeDeviceRepository::new(store);
    let member_account = match devices.get()? {
        Some(device) => device.account,
        None => devices
            .account_root()?
            .ok_or_eyre("this node holds neither a device nor an account root")?
            .account(),
    };
    Ok(JoinGroupResponse {
        group_id: namespace,
        member_identity,
        member_account,
    })
}

/// Follow a removed namespace with its marker held, so only the replay of this node's
/// own ops runs, and wait until it is a member again; the follow is undone otherwise.
async fn restore(
    store: &Store,
    node_client: &NodeClient,
    removed: ForgottenNamespace,
) -> EyreResult<(PublicKey, [u8; 32])> {
    let namespace = removed.namespace_id;
    let restored = match follow_marked(store, node_client, &namespace).await {
        Ok(identity) => wait_until_restored(store, namespace)
            .await
            .map(|account| (identity, account)),
        Err(err) => Err(err),
    };
    match restored {
        Ok((identity, account)) => {
            ForgottenNamespaces::new(store).clear(&namespace)?;
            // Auto-follow refused the namespace's contexts while it was marked.
            notify(OpEvent::AutoFollowSet {
                group_id: namespace.to_bytes(),
                member: account,
                contexts: true,
                subgroups: false,
            });
            Ok(identity)
        }
        Err(err) => {
            undo_follow(store, node_client, &namespace).await;
            Err(err)
        }
    }
}

/// The group rows and key are back and this node's own key speaks for a member.
async fn wait_until_restored(store: &Store, namespace: ContextGroupId) -> EyreResult<AccountId> {
    let restored = || -> EyreResult<Option<AccountId>> {
        if MetaRepository::new(store).load(&namespace)?.is_none()
            || GroupKeyring::new(store, namespace)
                .load_current_key_record()?
                .is_none()
        {
            return Ok(None);
        }
        let Some((identity, _)) = NamespaceRepository::new(store).identity(&namespace)? else {
            return Ok(None);
        };
        let Some(account) = member_account_in_namespace(store, &namespace, &identity)? else {
            return Ok(None);
        };
        Ok(MembershipRepository::new(store)
            .is_member(&namespace, &account)?
            .then_some(account))
    };
    let deadline = tokio::time::Instant::now() + LANDING_TIMEOUT;
    loop {
        if let Some(account) = restored()? {
            return Ok(account);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ContextError::NamespaceNotRestored {
                namespace_id: hex::encode(namespace.to_bytes()),
            }
            .into());
        }
        tokio::time::sleep(LANDING_POLL).await;
    }
}

async fn undo_follow(store: &Store, node_client: &NodeClient, namespace: &ContextGroupId) {
    if let Err(err) = node_client
        .unsubscribe_namespace(namespace.to_bytes())
        .await
    {
        warn!(?namespace, %err, "failed to unsubscribe a namespace whose restore failed");
    }
    if let Err(err) = store
        .handle()
        .delete(&NamespaceParticipation::new(namespace.to_bytes()))
    {
        warn!(?namespace, %err, "failed to drop the participation of a namespace whose restore failed");
    }
}

/// Where this node follows an account namespace, its set must still name
/// `namespace` (a sibling's leave drops it) and this device's scope must reach it.
fn require_account_still_holds(
    store: &Store,
    namespace: ContextGroupId,
    marked: Option<ApplicationId>,
) -> EyreResult<()> {
    // Participation, not the id: a holder derives its account namespace before
    // any pairing creates it, and an uncreated one names nothing.
    let participating = NamespaceRepository::new(store).participating_namespaces()?;
    let Some(account_namespace) = NodeDeviceRepository::new(store)
        .account_namespace()?
        .filter(|namespace| participating.contains(namespace))
    else {
        return Ok(());
    };
    let Some(application) =
        AccountNamespaceSet::new(store, account_namespace).contains(namespace)?
    else {
        return Err(ContextError::NotANamespaceMember {
            namespace_id: hex::encode(namespace.to_bytes()),
        }
        .into());
    };
    if !reaches(store, namespace, application.or(marked))? {
        return Err(ContextError::DeviceOutOfScope {
            group_id: hex::encode(namespace.to_bytes()),
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use calimero_context_client::group::{CreateGroupRequest, ListNamespacesForApplicationRequest};
    use calimero_governance_store::test_fixtures::test_meta;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_store::db::InMemoryDB;

    use super::*;
    use crate::test_support::{actor, enrol, paired_device_scoped_to};

    const NS: [u8; 32] = [0x3C; 32];
    const ACCOUNT_NS: [u8; 32] = [0x3D; 32];
    const APP: [u8; 32] = [0x3E; 32];
    const OTHER_APP: [u8; 32] = [0x3F; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// What a peer's pull brings back: the group rows, the key and this node's
    /// own membership. The flag says the removal was still marked when it landed.
    fn land_state_soon(store: &Store, namespace: ContextGroupId) -> Arc<AtomicBool> {
        let store = store.clone();
        let marked = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&marked);
        actix::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            seen.store(
                ForgottenNamespaces::new(&store)
                    .is_forgotten(&namespace)
                    .expect("read the marker"),
                Ordering::SeqCst,
            );
            MetaRepository::new(&store)
                .save(&namespace, &test_meta())
                .expect("the group rows arrive");
            let _key_id = GroupKeyring::new(&store, namespace)
                .store_key(&[0x42; 32])
                .expect("the group key arrives");
            let (identity, _) = NamespaceRepository::new(&store)
                .identity(&namespace)
                .expect("read this node's identity")
                .expect("the follow gave this node an identity");
            let account = enrol(&store, &namespace, &identity);
            MembershipRepository::new(&store)
                .add_member(&namespace, &account, GroupMemberRole::Member)
                .expect("this node's account is a member");
        });
        marked
    }

    fn topic() -> String {
        format!("ns/{}", hex::encode(NS))
    }

    /// The marker a member's removal leaves.
    fn removed(store: &Store) -> ContextGroupId {
        let namespace = ContextGroupId::from(NS);
        ForgottenNamespaces::new(store)
            .mark(&namespace, Some(ApplicationId::from(APP)), 7, None)
            .expect("record the removal");
        namespace
    }

    /// A device paired into an account whose set names `named`, scoped to `scope`.
    fn a_paired_device(store: &Store, named: Option<ContextGroupId>, scope: &[ApplicationId]) {
        let account_namespace = ContextGroupId::from(ACCOUNT_NS);
        let _device = paired_device_scoped_to(store, &account_namespace, scope);
        if let Some(namespace) = named {
            AccountNamespaceSet::new(store, account_namespace)
                .record(namespace, Some(ApplicationId::from(APP)))
                .expect("the account holds the namespace");
        }
    }

    async fn rejoin(
        harness: &actor::Harness,
        namespace: ContextGroupId,
    ) -> EyreResult<JoinGroupResponse> {
        harness
            .manager
            .send(RejoinNamespaceRequest {
                namespace_id: namespace,
            })
            .await
            .expect("the manager answers")
    }

    fn refusal(err: &eyre::Report) -> Option<&ContextError> {
        err.downcast_ref::<ContextError>()
    }

    fn still_removed(store: &Store, namespace: ContextGroupId, harness: &mut actor::Harness) {
        assert!(ForgottenNamespaces::new(store)
            .is_forgotten(&namespace)
            .expect("read the marker"));
        assert!(harness.live_topics().is_empty(), "nothing subscribed");
        assert!(!NamespaceRepository::new(store)
            .participating_namespaces()
            .expect("read participation")
            .contains(&namespace));
    }

    /// A node with no account namespace has only the marker, which the removal
    /// writes for a member alone.
    #[actix::test]
    async fn a_removed_namespace_is_followed_again_and_its_marker_lifted() {
        let store = store();
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");
        let mut harness = actor::over(store.clone()).await;
        let namespace = removed(&store);
        let marked_until_landed = land_state_soon(&store, namespace);

        let joined = rejoin(&harness, namespace).await.expect("a member rejoins");

        assert!(
            marked_until_landed.load(Ordering::SeqCst),
            "the removal stays marked until the state lands"
        );
        assert_eq!(joined.group_id, namespace);
        assert!(!ForgottenNamespaces::new(&store)
            .is_forgotten(&namespace)
            .expect("read the marker"));
        assert_eq!(harness.subscribed(), vec![topic()]);
        assert!(NamespaceRepository::new(&store)
            .participating_namespaces()
            .expect("read participation")
            .contains(&namespace));
    }

    /// Auto-follow refused the namespace's contexts while it was marked, so the
    /// restore announces that this member follows once it is lifted.
    #[actix::test]
    async fn a_restored_namespace_has_its_contexts_followed_again() {
        let store = store();
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");
        let harness = actor::over(store.clone()).await;
        let namespace = removed(&store);
        let mut events = calimero_governance_store::op_events::subscribe();
        let _marked = land_state_soon(&store, namespace);

        let _joined = rejoin(&harness, namespace).await.expect("a member rejoins");

        let announced = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await {
                    Ok(OpEvent::AutoFollowSet {
                        group_id,
                        contexts: true,
                        ..
                    }) if group_id == namespace.to_bytes() => return true,
                    Ok(_) => continue,
                    Err(_) => return false,
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(announced, "the restore re-announces auto-follow");
    }

    #[actix::test]
    async fn a_device_whose_account_still_names_the_namespace_rejoins_it() {
        let store = store();
        let namespace = removed(&store);
        land_state_soon(&store, namespace);
        a_paired_device(&store, Some(namespace), &[ApplicationId::from(APP)]);
        let mut harness = actor::over(store.clone()).await;

        let _joined = rejoin(&harness, namespace).await.expect("a member rejoins");

        assert!(harness.subscribed().contains(&topic()));
    }

    /// A sibling's leave drops the namespace from the account's set.
    #[actix::test]
    async fn a_namespace_the_account_no_longer_names_is_refused_and_stays_removed() {
        let store = store();
        let namespace = removed(&store);
        a_paired_device(&store, None, &[]);
        let mut harness = actor::over(store.clone()).await;

        let err = rejoin(&harness, namespace).await.expect_err("not a member");

        assert!(
            matches!(
                refusal(&err),
                Some(ContextError::NotANamespaceMember { .. })
            ),
            "got: {err:#}"
        );
        still_removed(&store, namespace, &mut harness);
        assert!(harness.subscribed().is_empty(), "never asked to subscribe");
    }

    #[actix::test]
    async fn a_device_scoped_to_another_application_is_refused() {
        let store = store();
        let namespace = removed(&store);
        a_paired_device(&store, Some(namespace), &[ApplicationId::from(OTHER_APP)]);
        let mut harness = actor::over(store.clone()).await;

        let err = rejoin(&harness, namespace).await.expect_err("out of scope");

        assert!(
            matches!(refusal(&err), Some(ContextError::DeviceOutOfScope { .. })),
            "got: {err:#}"
        );
        still_removed(&store, namespace, &mut harness);
        assert!(harness.subscribed().is_empty(), "never asked to subscribe");
    }

    #[actix::test]
    async fn a_namespace_never_joined_is_not_found() {
        let store = store();
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");
        let mut harness = actor::over(store.clone()).await;
        let namespace = ContextGroupId::from(NS);

        let err = rejoin(&harness, namespace)
            .await
            .expect_err("nothing to rejoin");

        assert!(
            matches!(refusal(&err), Some(ContextError::NamespaceNotFound { .. })),
            "got: {err:#}"
        );
        assert!(harness.subscribed().is_empty());
    }

    /// The subscribe failing after the marker was lifted puts it back.
    #[actix::test]
    async fn a_rejoin_whose_subscribe_fails_leaves_the_namespace_removed() {
        let store = store();
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");
        let (mut harness, gate) = actor::over_holding_subscribe(store.clone(), topic()).await;
        gate.close();
        let namespace = removed(&store);

        let _err = rejoin(&harness, namespace)
            .await
            .expect_err("the swarm refused the topic");

        still_removed(&store, namespace, &mut harness);
        assert_unchanged_marker(&store);
    }

    /// The follow worked but a peer never brought the state: the removal stands,
    /// the topic is let go again, and the restore can be tried again.
    #[actix::test]
    async fn a_rejoin_whose_state_never_lands_leaves_the_namespace_removed() {
        let store = store();
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");
        let harness = actor::over(store.clone()).await;
        let namespace = removed(&store);

        let err = rejoin(&harness, namespace)
            .await
            .expect_err("nothing arrived");

        assert!(
            matches!(
                refusal(&err),
                Some(ContextError::NamespaceNotRestored { .. })
            ),
            "got: {err:#}"
        );
        assert_unchanged_marker(&store);
        assert!(harness.live_topics().is_empty(), "the topic is let go");
        assert!(!NamespaceRepository::new(&store)
            .participating_namespaces()
            .expect("read participation")
            .contains(&namespace));
    }

    fn assert_unchanged_marker(store: &Store) {
        let marker = ForgottenNamespaces::new(store)
            .list()
            .expect("list the removals");
        assert_eq!(marker.len(), 1);
        assert_eq!(marker[0].application_id, Some(ApplicationId::from(APP)));
        assert_eq!(marker[0].forgotten_at_ms, 7);
    }

    #[actix::test]
    async fn rejoining_a_namespace_this_node_is_in_is_a_no_op_success() {
        let store = store();
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");
        let harness = actor::over(store.clone()).await;
        let created = harness
            .manager
            .send(CreateGroupRequest {
                salt: None,
                bytecode_id: None,
                application_id: None,
                name: None,
                parent_group_id: None,
                restricted: true,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");

        let joined = rejoin(&harness, created.group_id)
            .await
            .expect("already a member");

        assert_eq!(joined.group_id, created.group_id);
    }

    async fn forgotten_listed(harness: &actor::Harness) -> Vec<ContextGroupId> {
        harness
            .manager
            .send(ListNamespacesForApplicationRequest {
                application_id: ApplicationId::from(APP),
                offset: 0,
                limit: usize::MAX,
            })
            .await
            .expect("the manager answers")
            .expect("the listing")
            .into_iter()
            .filter(|summary| summary.forgotten)
            .map(|summary| summary.namespace_id)
            .collect()
    }

    #[actix::test]
    async fn the_application_lists_a_removed_namespace_until_it_is_restored() {
        let store = store();
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");
        let harness = actor::over(store.clone()).await;
        let namespace = removed(&store);
        land_state_soon(&store, namespace);
        ForgottenNamespaces::new(&store)
            .mark(
                &ContextGroupId::from([0x40; 32]),
                Some(ApplicationId::from(OTHER_APP)),
                8,
                None,
            )
            .expect("another application's removal");

        assert_eq!(forgotten_listed(&harness).await, vec![namespace]);

        let _joined = rejoin(&harness, namespace).await.expect("a member rejoins");

        assert_eq!(forgotten_listed(&harness).await, vec![]);
    }
}
