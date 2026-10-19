//! Following a namespace again with no invitation, one the user removed from
//! this node while the account stayed a member; peers still judge membership.

use std::future::Future;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinGroupResponse, RejoinNamespaceRequest};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    AccountNamespaceSet, ForgottenNamespaces, NamespaceRepository, NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_store::Store;
use eyre::{OptionExt, Result as EyreResult};

use crate::account_follow::{reaches, require_reach};
use crate::account_namespace::account_is_member;
use crate::error::ContextError;
use crate::handlers::follow_namespace::follow;
use crate::ContextManager;

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
    let forgotten = ForgottenNamespaces::new(store);
    let removed = forgotten
        .list()?
        .into_iter()
        .find(|entry| entry.namespace_id == namespace);
    let (member_identity, _) = match removed {
        Some(removed) => {
            require_account_still_holds(store, namespace, removed.application_id)?;
            with_removal_lifted(
                store.clone(),
                namespace,
                follow(store, node_client, &namespace),
            )
            .await?
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

/// Run `join` with the user's removal of `namespace` lifted, so every path the
/// join drives treats it as held, and put the removal back if the join fails.
///
/// # Errors
/// The join's failure, or a store failure reading or writing the marker.
async fn with_removal_lifted<T>(
    store: Store,
    namespace: ContextGroupId,
    join: impl Future<Output = EyreResult<T>>,
) -> EyreResult<T> {
    let forgotten = ForgottenNamespaces::new(&store);
    let removed = forgotten
        .list()?
        .into_iter()
        .find(|entry| entry.namespace_id == namespace);
    if removed.is_some() {
        forgotten.clear(&namespace)?;
    }
    let joined = join.await;
    if let (Err(_), Some(removed)) = (&joined, removed) {
        forgotten.mark(&namespace, removed.application_id, removed.forgotten_at_ms)?;
    }
    joined
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

    fn topic() -> String {
        format!("ns/{}", hex::encode(NS))
    }

    /// The marker a member's removal leaves.
    fn removed(store: &Store) -> ContextGroupId {
        let namespace = ContextGroupId::from(NS);
        ForgottenNamespaces::new(store)
            .mark(&namespace, Some(ApplicationId::from(APP)), 7)
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
        assert!(harness.subscribed().is_empty(), "nothing subscribed");
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
        let (harness, gate) = actor::over_holding_subscribe(store.clone(), topic()).await;
        gate.close();
        let namespace = removed(&store);

        let _err = rejoin(&harness, namespace)
            .await
            .expect_err("the swarm refused the topic");

        let marker = ForgottenNamespaces::new(&store)
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
        ForgottenNamespaces::new(&store)
            .mark(
                &ContextGroupId::from([0x40; 32]),
                Some(ApplicationId::from(OTHER_APP)),
                8,
            )
            .expect("another application's removal");

        assert_eq!(forgotten_listed(&harness).await, vec![namespace]);

        let _joined = rejoin(&harness, namespace).await.expect("a member rejoins");

        assert_eq!(forgotten_listed(&harness).await, vec![]);
    }
}
