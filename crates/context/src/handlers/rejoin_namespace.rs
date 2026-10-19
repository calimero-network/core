//! Following a namespace again with no invitation, one the user removed from
//! this node while the account stayed a member; peers still judge membership.

use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinGroupResponse, RejoinNamespaceRequest};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    AccountNamespaceSet, ForgottenNamespace, ForgottenNamespaces, GroupKeyring, MetaRepository,
    NamespaceRepository, NodeDeviceRepository,
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
use crate::handlers::follow_namespace::follow;
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

/// Follow a removed namespace and wait for its group rows and key to land. The
/// marker is lifted for the follow, which refuses a marked namespace, and put
/// back with the follow undone if the state does not arrive.
async fn restore(
    store: &Store,
    node_client: &NodeClient,
    removed: ForgottenNamespace,
) -> EyreResult<(PublicKey, [u8; 32])> {
    let namespace = removed.namespace_id;
    let forgotten = ForgottenNamespaces::new(store);
    forgotten.clear(&namespace)?;
    let restored = match follow(store, node_client, &namespace).await {
        Ok(identity) => wait_until_landed(store, namespace).await.map(|()| identity),
        Err(err) => Err(err),
    };
    if restored.is_err() {
        undo_follow(store, node_client, &namespace).await;
        forgotten.mark(&namespace, removed.application_id, removed.forgotten_at_ms)?;
    }
    restored
}

async fn wait_until_landed(store: &Store, namespace: ContextGroupId) -> EyreResult<()> {
    let landed = || -> EyreResult<bool> {
        Ok(MetaRepository::new(store).load(&namespace)?.is_some()
            && GroupKeyring::new(store, namespace)
                .load_current_key_record()?
                .is_some())
    };
    let deadline = tokio::time::Instant::now() + LANDING_TIMEOUT;
    while !landed()? {
        if tokio::time::Instant::now() >= deadline {
            return Err(ContextError::NamespaceNotRestored {
                namespace_id: hex::encode(namespace.to_bytes()),
            }
            .into());
        }
        tokio::time::sleep(LANDING_POLL).await;
    }
    Ok(())
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
    use std::sync::Arc;

    use calimero_context_client::group::{CreateGroupRequest, ListNamespacesForApplicationRequest};
    use calimero_governance_store::test_fixtures::test_meta;
    use calimero_store::db::InMemoryDB;

    use super::*;
    use crate::test_support::{actor, paired_device_scoped_to};

    const NS: [u8; 32] = [0x3C; 32];
    const ACCOUNT_NS: [u8; 32] = [0x3D; 32];
    const APP: [u8; 32] = [0x3E; 32];
    const OTHER_APP: [u8; 32] = [0x3F; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// What a peer's pull brings back: the group rows and the group key.
    fn land_state_soon(store: &Store, namespace: ContextGroupId) {
        let store = store.clone();
        actix::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            MetaRepository::new(&store)
                .save(&namespace, &test_meta())
                .expect("the group rows arrive");
            let _key_id = GroupKeyring::new(&store, namespace)
                .store_key(&[0x42; 32])
                .expect("the group key arrives");
        });
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
        land_state_soon(&store, namespace);

        let joined = rejoin(&harness, namespace).await.expect("a member rejoins");

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
