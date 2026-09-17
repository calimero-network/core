//! `RescopeDeviceRequest` handler - replace a device's scope, the counterpart of
//! the add-only `relink_device`.
//!
//! Order is load-bearing: the replacement is recorded in the account namespace
//! first, then the namespaces it left, then the ones it gained.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::{scope_covers, DeviceId};
use calimero_context_client::group::{
    BindOutcome, RescopeDeviceRequest, RescopeDeviceResponse, ScopeRequest,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MetaRepository, NamespaceRepository, NodeDeviceRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{info, warn};

use crate::error::ContextError;
use crate::handlers::pair_device_complete::signing_identity;
use crate::handlers::relink_device::resolve_device;
use crate::ContextManager;

/// The applications a request asks for, with the empty `Only` refused.
fn requested_applications(scope: ScopeRequest) -> EyreResult<Vec<ApplicationId>> {
    match scope {
        ScopeRequest::All => Ok(Vec::new()),
        ScopeRequest::Only(applications) if applications.is_empty() => {
            Err(ContextError::ScopeReplacementEmpty.into())
        }
        ScopeRequest::Only(applications) => Ok(applications),
    }
}

/// Refuse the device this node runs as while it holds the account root.
///
/// That device signs every scope statement, so narrowing it would have it publish
/// its own withdrawal: here rather than in the route, so no caller can skip it.
fn refuse_the_root_holders_own_device(store: &Store, device: DeviceId) -> EyreResult<()> {
    let devices = NodeDeviceRepository::new(store);
    if devices.holder_root()?.is_some()
        && devices.get()?.is_some_and(|held| held.device() == device)
    {
        return Err(ContextError::ScopeReplacementHoldsTheRoot {
            device: device.to_string(),
        }
        .into());
    }
    Ok(())
}

/// Every participating namespace `applications` no longer reaches, bound or not:
/// the descope is what writes the scope floor there, so a racing sibling link or
/// a replayed stale one is refused at apply on every replica.
///
/// Never the account namespace: that is where the device's certificate and every
/// scope statement live, and no scope names it.
fn namespaces_left_behind(
    store: &Store,
    namespaces: &[ContextGroupId],
    account_namespace: ContextGroupId,
    applications: &[ApplicationId],
) -> EyreResult<Vec<ContextGroupId>> {
    let meta = MetaRepository::new(store);
    let mut left = Vec::new();
    for namespace in namespaces {
        if *namespace == account_namespace {
            continue;
        }
        let application = meta.load(namespace)?.map(|meta| meta.target.application_id);
        if !scope_covers(applications, application) {
            left.push(*namespace);
        }
    }
    Ok(left)
}

impl Handler<RescopeDeviceRequest> for ContextManager {
    type Result = ActorResponse<Self, <RescopeDeviceRequest as Message>::Result>;

    fn handle(
        &mut self,
        RescopeDeviceRequest { device, scope }: RescopeDeviceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();

        let applications = match requested_applications(scope) {
            Ok(applications) => applications,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let (root, mut cached) = match resolve_device(&store, device) {
            Ok(target) => target,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        if let Err(err) = refuse_the_root_holders_own_device(&store, device) {
            return ActorResponse::reply(Err(err));
        }
        // Signed before anything is published: the descopes below carry it, and so
        // does every link the re-bind makes, so all three name one statement.
        cached.scope = match crate::account_namespace::next_device_scope(
            &store,
            Some(root.account_namespace()),
            &root,
            &cached.proof,
            &applications,
        ) {
            Ok(scope) => scope,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let account = root.account();
        let account_namespace = root.account_namespace();

        let namespaces = match NamespaceRepository::new(&store).participating_namespaces() {
            Ok(namespaces) => namespaces,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let signer_sk = match signing_identity(&store, &namespaces) {
            Ok(identity) => PrivateKey::from(identity),
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                // The statement is the only durable record of the replacement and
                // what every later bind is judged against.
                if !crate::account_namespace::publish_device_certified(
                    &store,
                    &node_client,
                    &ack_router,
                    account_namespace,
                    &signer_sk,
                    &cached,
                    "rescope_device",
                )
                .await
                {
                    eyre::bail!(
                        "the replacement scope for {device} was not recorded in the \
                         account namespace"
                    );
                }

                let left =
                    namespaces_left_behind(&store, &namespaces, account_namespace, &applications)?;
                let mut outcomes = Vec::with_capacity(namespaces.len());
                for namespace in &left {
                    let op = GroupOp::AccountDeviceDescoped {
                        account,
                        device,
                        scope: Box::new(cached.scope.clone()),
                    };
                    match calimero_governance_store::withdraw_device_in(
                        &store,
                        &node_client,
                        &ack_router,
                        namespace,
                        &signer_sk,
                        device,
                        op,
                    )
                    .await
                    {
                        Ok(key_rotated) => {
                            outcomes.push((*namespace, BindOutcome::Descoped { key_rotated }));
                        }
                        // One namespace failing must not withhold the narrowing
                        // from the rest; the caller sees which ones landed.
                        Err(err) => warn!(
                            ?err, ?namespace, %device,
                            "rescope: a namespace did not take the narrowing; the rest continue"
                        ),
                    }
                }

                let bound = calimero_governance_store::bind_device_everywhere(
                    &store,
                    &node_client,
                    &ack_router,
                    &namespaces,
                    &signer_sk,
                    &cached,
                )
                .await;
                for (namespace, outcome) in bound {
                    if left.contains(&namespace) {
                        continue;
                    }
                    outcomes.push((namespace, outcome));
                }

                info!(%account, %device, ?applications, ?outcomes, "replaced a device's scope");
                Ok(RescopeDeviceResponse::new(
                    account,
                    device,
                    applications,
                    outcomes,
                ))
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        AccountBindingRepository, AccountDeviceRegistry, GroupKeyring, MembershipRepository,
    };
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};

    use super::*;
    use crate::handlers::ensure_account_namespace::ensure_account_namespace;
    use crate::test_support::{actor, certify_device};

    const APP_ONE: [u8; 32] = [0x11; 32];
    const APP_TWO: [u8; 32] = [0x22; 32];
    const NS_ONE: [u8; 32] = [0xA1; 32];
    const NS_TWO: [u8; 32] = [0xA2; 32];

    fn app(id: [u8; 32]) -> ApplicationId {
        ApplicationId::from(id)
    }

    /// A node holding its own account and taking part in two namespaces, one per
    /// application, each with the membership and key a publish here needs.
    fn a_holder_of_two_namespaces() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let devices = NodeDeviceRepository::new(&store);
        let _root = devices
            .provision_account_root()
            .expect("a node that ran `merod init` holds a root");
        for (namespace, application) in [(NS_ONE, APP_ONE), (NS_TWO, APP_TWO)] {
            let namespace = ContextGroupId::from(namespace);
            let (_ns, node_pk, _sk) = NamespaceRepository::new(&store)
                .participate_in(&namespace)
                .expect("this node's identity here");
            let _held = devices
                .ensure_enrolled(&namespace)
                .expect("mint this node's own device");
            let account = crate::test_support::enrol_holder(&store, &namespace, &node_pk);
            MetaRepository::new(&store)
                .save(
                    &namespace,
                    &GroupMetaValue {
                        target: GroupTarget {
                            application_id: app(application),
                            bytecode_id: [0xAA; 32],
                            ..Default::default()
                        },
                        created_at: 1_700_000_000,
                        admin_identity: account,
                        owner_identity: account,
                        migration: None,
                        auto_join: true,
                    },
                )
                .expect("save the namespace metadata");
            MembershipRepository::new(&store)
                .add_member(
                    &namespace,
                    &account,
                    calimero_primitives::context::GroupMemberRole::Admin,
                )
                .expect("be an admin here");
            let _key_id = GroupKeyring::new(&store, namespace)
                .store_key(&[0x42; 32])
                .expect("hold the scope key");
        }
        store
    }

    fn is_bound(store: &Store, namespace: [u8; 32], device: DeviceId) -> bool {
        AccountBindingRepository::new(store)
            .is_device_linked(&namespace.into(), device)
            .expect("read the bindings")
    }

    /// An empty `Only` means every application on the wire, so it must never be
    /// reachable: the narrowest-looking request would be the widest one.
    #[test]
    fn naming_no_application_at_all_is_refused() {
        let refused =
            requested_applications(ScopeRequest::Only(vec![])).expect_err("an empty replacement");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::ScopeReplacementEmpty)
        ));
    }

    /// The narrowing publishes only where the new scope stops reaching, and the
    /// registry row every other device reads is what moves.
    #[actix::test]
    async fn narrowing_descopes_only_the_namespace_the_new_scope_left() {
        let store = a_holder_of_two_namespaces();
        let harness = actor::over(store.clone()).await;
        let account_namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let device = certify_device(&store, 0x31, &[]);
        let _relinked = harness
            .manager
            .send(calimero_context_client::group::RelinkDeviceRequest {
                device,
                applications: vec![],
            })
            .await
            .expect("the manager answers")
            .expect("bound everywhere first");
        assert!(is_bound(&store, NS_ONE, device));
        assert!(is_bound(&store, NS_TWO, device));

        let narrowed = harness
            .manager
            .send(RescopeDeviceRequest {
                device,
                scope: ScopeRequest::Only(vec![app(APP_ONE)]),
            })
            .await
            .expect("the manager answers")
            .expect("the rescope runs");

        assert!(is_bound(&store, NS_ONE, device), "the covered one stays");
        assert!(
            !is_bound(&store, NS_TWO, device),
            "the uncovered one is unbound"
        );
        assert!(
            !AccountBindingRepository::new(&store)
                .is_revoked(&NS_TWO.into(), device)
                .expect("read the tombstones"),
            "narrowing must leave no tombstone; that is what makes it reversible"
        );
        assert!(narrowed.outcomes.contains(&(
            ContextGroupId::from(NS_TWO),
            BindOutcome::Descoped { key_rotated: true },
        )));
        assert!(
            !narrowed.outcomes.iter().any(|(namespace, outcome)| {
                *namespace == ContextGroupId::from(NS_ONE)
                    && matches!(outcome, BindOutcome::Descoped { .. })
            }),
            "a namespace the new scope still reaches is never published into; got: {:?}",
            narrowed.outcomes
        );
        let recorded = AccountDeviceRegistry::new(&store, account_namespace)
            .device(device)
            .expect("read")
            .expect("the rescope recorded the device");
        assert_eq!(recorded.applications(), vec![app(APP_ONE)]);
    }

    /// A descoped device is not re-bound by the fan-out that runs right after,
    /// and a later widening re-binds it - which a revocation could never allow.
    #[actix::test]
    async fn a_narrowed_device_stays_unbound_until_a_widening_brings_it_back() {
        let store = a_holder_of_two_namespaces();
        let harness = actor::over(store.clone()).await;
        let _namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let device = certify_device(&store, 0x32, &[]);
        let _relinked = harness
            .manager
            .send(calimero_context_client::group::RelinkDeviceRequest {
                device,
                applications: vec![],
            })
            .await
            .expect("the manager answers")
            .expect("bound everywhere first");

        let _narrowed = harness
            .manager
            .send(RescopeDeviceRequest {
                device,
                scope: ScopeRequest::Only(vec![app(APP_ONE)]),
            })
            .await
            .expect("the manager answers")
            .expect("narrowed");
        assert!(!is_bound(&store, NS_TWO, device));

        let widened = harness
            .manager
            .send(RescopeDeviceRequest {
                device,
                scope: ScopeRequest::All,
            })
            .await
            .expect("the manager answers")
            .expect("widened");

        assert!(is_bound(&store, NS_TWO, device), "the id was never spent");
        assert!(widened.applications.is_empty());
        assert!(widened.outcomes.contains(&(
            ContextGroupId::from(NS_TWO),
            BindOutcome::Linked {
                key_delivered: true
            },
        )));
    }

    /// The floor is written where nothing is bound too, which is what refuses a
    /// racing sibling link or a replayed stale one on every replica - rather than
    /// only where this node happened to see a binding at narrowing time.
    #[actix::test]
    async fn a_namespace_the_device_was_never_bound_in_still_takes_the_floor() {
        let store = a_holder_of_two_namespaces();
        let harness = actor::over(store.clone()).await;
        let account_namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        // Certified but never linked anywhere: NS_TWO holds no binding for it.
        let device = certify_device(&store, 0x35, &[]);
        assert!(!is_bound(&store, NS_TWO, device));

        let narrowed = harness
            .manager
            .send(RescopeDeviceRequest {
                device,
                scope: ScopeRequest::Only(vec![app(APP_ONE)]),
            })
            .await
            .expect("the manager answers")
            .expect("the rescope runs");

        let registry = AccountDeviceRegistry::new(&store, account_namespace);
        let recorded = registry
            .device(device)
            .expect("read")
            .expect("the rescope recorded the device");
        let epoch = recorded.scope.statement.scope_epoch;
        let account = NodeDeviceRepository::new(&store)
            .account_root()
            .expect("read")
            .expect("the holder holds a root")
            .account();
        let bindings = AccountBindingRepository::new(&store);
        assert_eq!(
            bindings
                .scope_floor(&NS_TWO.into(), account, device)
                .expect("read the floor"),
            Some(epoch),
            "the uncovered namespace has to record the replacement's epoch"
        );
        assert!(narrowed.outcomes.contains(&(
            ContextGroupId::from(NS_TWO),
            BindOutcome::Descoped { key_rotated: false },
        )));

        // A link made under the scope in force before the replacement, landing
        // after it: the floor is what refuses it.
        assert!(matches!(
            bindings
                .apply_link(
                    &NS_TWO.into(),
                    &recorded.proof.genesis,
                    &recorded.proof.chain,
                    &recorded.proof.statement,
                    epoch - 1,
                )
                .expect("read"),
            Err(calimero_governance_store::BindingRejected::ScopeNarrowed { .. })
        ));
    }

    /// The holder's own device signs every scope statement, so narrowing it would
    /// publish its own withdrawal. Refused before anything is signed, for `all`
    /// as much as for `only`: neither has anything to replace.
    #[actix::test]
    async fn the_device_holding_the_account_root_cannot_be_rescoped() {
        let store = a_holder_of_two_namespaces();
        let harness = actor::over(store.clone()).await;
        let account_namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let own = NodeDeviceRepository::new(&store)
            .get()
            .expect("read")
            .expect("the holder runs as a device")
            .device();
        let account = NodeDeviceRepository::new(&store)
            .account_root()
            .expect("read")
            .expect("the holder holds a root")
            .account();
        let registry = AccountDeviceRegistry::new(&store, account_namespace);
        let before = registry
            .device(own)
            .expect("read")
            .expect("the holder recorded its own device")
            .scope
            .statement
            .scope_epoch;

        for scope in [ScopeRequest::Only(vec![app(APP_ONE)]), ScopeRequest::All] {
            let refused = harness
                .manager
                .send(RescopeDeviceRequest { device: own, scope })
                .await
                .expect("the manager answers")
                .expect_err("the holder's own device is never rescoped");
            assert!(
                matches!(
                    refused.downcast_ref::<ContextError>(),
                    Some(ContextError::ScopeReplacementHoldsTheRoot { .. })
                ),
                "got: {refused}"
            );
        }

        assert_eq!(
            registry
                .device(own)
                .expect("read")
                .expect("still recorded")
                .scope
                .statement
                .scope_epoch,
            before,
            "a refusal must not mint a scope epoch"
        );
        let bindings = AccountBindingRepository::new(&store);
        for namespace in [NS_ONE, NS_TWO] {
            assert_eq!(
                bindings
                    .scope_floor(&namespace.into(), account, own)
                    .expect("read the floor"),
                None,
                "a refusal must publish no descope"
            );
        }
    }

    /// A replacement the registry never took must not answer with the new scope:
    /// the statement is the only durable record of it.
    #[actix::test]
    async fn a_replacement_the_registry_did_not_record_is_an_error() {
        let store = a_holder_of_two_namespaces();
        let device = certify_device(&store, 0x34, &[]);

        // Nothing created the account namespace here, so this node is not an
        // admin of the one its root names and the certified op's apply refuses.
        let harness = actor::over(store.clone()).await;
        let refused = harness
            .manager
            .send(RescopeDeviceRequest {
                device,
                scope: ScopeRequest::Only(vec![app(APP_ONE)]),
            })
            .await
            .expect("the manager answers")
            .expect_err("the replacement scope reached no registry");

        assert!(
            refused.to_string().contains(&device.to_string()),
            "the refusal has to name the device; got: {refused}"
        );
    }
}
