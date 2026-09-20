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
use calimero_governance_store::{
    AccountNamespaceSet, MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_governance_types::bounds::MAX_DEVICE_SCOPE_APPLICATIONS;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{debug, info, warn};

use crate::error::ContextError;
use crate::handlers::pair_device_complete::signing_identity;
use crate::handlers::relink_device::resolve_device;
use crate::ContextManager;

/// The applications a request asks for: the empty `Only` refused, a list longer
/// than a statement can carry refused, and repeats dropped in the order given.
fn requested_applications(scope: ScopeRequest) -> EyreResult<Vec<ApplicationId>> {
    let ScopeRequest::Only(applications) = scope else {
        return Ok(Vec::new());
    };
    if applications.is_empty() {
        return Err(ContextError::ScopeReplacementEmpty.into());
    }
    if applications.len() > MAX_DEVICE_SCOPE_APPLICATIONS {
        return Err(ContextError::ScopeReplacementTooLarge {
            limit: MAX_DEVICE_SCOPE_APPLICATIONS,
        }
        .into());
    }
    let mut named = Vec::with_capacity(applications.len());
    for application in applications {
        if !named.contains(&application) {
            named.push(application);
        }
    }
    Ok(named)
}

/// Refuse an application no namespace of this account targets.
///
/// Accepting one is an empty scope in all but name: it reaches nothing and
/// descopes the device everywhere, which is not what naming an application says.
fn refuse_unknown_applications(
    store: &Store,
    account_namespace: ContextGroupId,
    applications: &[ApplicationId],
) -> EyreResult<()> {
    if applications.is_empty() {
        return Ok(());
    }
    let known: Vec<_> = AccountNamespaceSet::new(store, account_namespace)
        .namespaces()?
        .into_iter()
        .filter_map(|(_namespace, application)| application)
        .collect();
    for application in applications {
        if !known.contains(application) {
            return Err(ContextError::ScopeReplacementUnknownApplication {
                application: application.to_string(),
            }
            .into());
        }
    }
    Ok(())
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
/// the descope is what writes the scope floor there. Never the account namespace.
///
/// Paired with the application each targets here, which the op carries so every
/// replica decides the narrowing against one reading of it.
fn namespaces_left_behind(
    store: &Store,
    namespaces: &[ContextGroupId],
    account_namespace: ContextGroupId,
    applications: &[ApplicationId],
) -> EyreResult<Vec<(ContextGroupId, Option<ApplicationId>)>> {
    let meta = MetaRepository::new(store);
    let mut left = Vec::new();
    for namespace in namespaces {
        if *namespace == account_namespace {
            continue;
        }
        let application = meta.load(namespace)?.map(|meta| meta.target.application_id);
        if !scope_covers(applications, application) {
            left.push((*namespace, application));
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
        if let Err(err) =
            refuse_unknown_applications(&store, root.account_namespace(), &applications)
        {
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
                for (namespace, application) in &left {
                    let op = GroupOp::AccountDeviceDescoped {
                        account,
                        device,
                        application: *application,
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
                        // One failure must not withhold the narrowing from the
                        // rest; nothing re-drives it, so a repeat is the repair.
                        Err(err) => {
                            warn!(
                                ?err, ?namespace, %device,
                                "rescope: a namespace did not take the narrowing; the rest continue"
                            );
                            outcomes.push((*namespace, BindOutcome::Failed));
                        }
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
                    if left.iter().any(|(left, _)| *left == namespace) {
                        continue;
                    }
                    outcomes.push((namespace, outcome));
                }

                debug!(%account, %device, ?applications, ?outcomes, "replaced a device's scope");
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
        AccountBindingRepository, AccountDeviceRegistry, AccountNamespaceSet, GroupKeyring,
        MembershipRepository,
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

    /// A list longer than a scope statement may carry could never be signed into
    /// one, so it is refused here rather than as "the replacement was not recorded".
    #[test]
    fn naming_more_applications_than_a_statement_carries_is_refused() {
        let named = vec![app(APP_ONE); MAX_DEVICE_SCOPE_APPLICATIONS + 1];
        let refused = requested_applications(ScopeRequest::Only(named))
            .expect_err("an oversized replacement");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::ScopeReplacementTooLarge { .. })
        ));
    }

    /// Repeats say nothing the first mention did not, and the statement they are
    /// signed into is what every peer compares.
    #[test]
    fn a_repeated_application_is_named_once_in_the_order_given() {
        let named = requested_applications(ScopeRequest::Only(vec![
            app(APP_TWO),
            app(APP_ONE),
            app(APP_TWO),
        ]))
        .expect("a replacement naming one application twice");
        assert_eq!(named, vec![app(APP_TWO), app(APP_ONE)]);
    }

    /// An application no namespace of the account targets reaches nothing, so
    /// accepting it would be an empty scope under another name.
    #[actix::test]
    async fn naming_an_application_this_account_has_no_namespace_for_is_refused() {
        let store = a_holder_of_two_namespaces();
        let harness = actor::over(store.clone()).await;
        let _namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let device = certify_device(&store, 0x39, &[]);

        let refused = harness
            .manager
            .send(RescopeDeviceRequest {
                device,
                scope: ScopeRequest::Only(vec![app([0x5F; 32])]),
            })
            .await
            .expect("the manager answers")
            .expect_err("the account takes part in no namespace of it");

        assert!(
            matches!(
                refused.downcast_ref::<ContextError>(),
                Some(ContextError::ScopeReplacementUnknownApplication { .. })
            ),
            "got: {refused}"
        );
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

    /// The floor is written where nothing is bound too, so a racing sibling link
    /// is refused on every replica, not only where a binding happened to be seen.
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
    /// publish its own withdrawal - for `all` as much as for `only`.
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

    /// Scope epochs only rise, so a device at the last one has nothing to mint:
    /// a typed refusal, never a `200` reporting a replacement that never happened.
    #[actix::test]
    async fn a_device_at_the_last_scope_epoch_is_refused() {
        let store = a_holder_of_two_namespaces();
        let harness = actor::over(store.clone()).await;
        let account_namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let device = certify_device(&store, 0x36, &[]);
        let registry = AccountDeviceRegistry::new(&store, account_namespace);
        let known = registry.device(device).expect("read").expect("row");
        let root = NodeDeviceRepository::new(&store)
            .account_root()
            .expect("read")
            .expect("the holder holds a root");
        assert!(registry
            .record(
                &known.proof,
                &crate::test_support::device_scope(
                    root.signing_key(),
                    &known.proof.statement,
                    &[],
                    u32::MAX
                ),
            )
            .expect("put the device at the last epoch"));

        let refused = harness
            .manager
            .send(RescopeDeviceRequest {
                device,
                scope: ScopeRequest::Only(vec![app(APP_ONE)]),
            })
            .await
            .expect("the manager answers")
            .expect_err("there is no epoch left to mint");

        assert!(
            matches!(
                refused.downcast_ref::<ContextError>(),
                Some(ContextError::ScopeEpochExhausted { .. })
            ),
            "got: {refused}"
        );
    }

    /// A replacement the registry never took must not answer with the new scope:
    /// the statement is the only durable record of it.
    #[actix::test]
    async fn a_replacement_the_registry_did_not_record_is_an_error() {
        let store = a_holder_of_two_namespaces();
        let device = certify_device(&store, 0x34, &[]);
        // The set names the application, so the request gets past validation and
        // fails where this test is aiming.
        AccountNamespaceSet::new(
            &store,
            NodeDeviceRepository::new(&store)
                .account_namespace()
                .expect("read")
                .expect("the holder names one"),
        )
        .record(ContextGroupId::from(NS_ONE), Some(app(APP_ONE)))
        .expect("record the namespace the account takes part in");

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
