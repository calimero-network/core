use calimero_governance_store::{
    GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository,
};
use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinContextRequest, JoinContextResponse};
use calimero_context_client::local_governance::KeyEnvelope;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextConfigParams;
use eyre::bail;
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

use calimero_governance_store::registration_notify;

use crate::ContextManager;

/// Overall budget for the context→group mapping to land locally after a
/// `sync_known_namespaces` kick. Dominated by peer-discovery in the cold
/// case (`Mesh low` / no peers); the normal case wakes within a few ms as
/// soon as `registration_notify::notify` fires from the apply path.
const GROUP_LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Fallback poll interval in case the notifier channel lags (burst of
/// registrations overflowing the broadcast capacity). Lag is handled by
/// re-reading the datastore; this bounds how long a lagged receiver
/// waits before that recheck.
const FALLBACK_POLL: Duration = Duration::from_millis(200);

/// Ceiling for the exponential backoff on fallback namespace re-syncs. The
/// datastore is still rechecked every [`FALLBACK_POLL`] (cheap, local), but the
/// network re-sync kicked from a poll tick backs off from [`FALLBACK_POLL`] up
/// to this cap so a single unresolved join doesn't fire a full namespace sync
/// every 200ms for its whole 30s budget.
const MAX_RESYNC_BACKOFF: Duration = Duration::from_secs(5);

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest { context_id }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        let ack_router = std::sync::Arc::clone(&self.ack_router);
        ActorResponse::r#async(
            async move {
                let mut group_id = calimero_governance_store::get_group_for_context(&datastore, &context_id)?;
                if group_id.is_none() {
                    // Subscribe BEFORE kicking sync so we cannot miss a signal
                    // that fires between the sync returning and us starting to
                    // wait. All messages sent after this point are delivered.
                    let mut rx = registration_notify::subscribe();

                    warn!(
                        %context_id,
                        "context->group mapping missing locally; syncing known namespaces"
                    );
                    sync_known_namespaces(&datastore, &node_client).await;

                    // Mapping may have landed synchronously during sync (creator's
                    // own apply, or a sync that completed and applied inline).
                    group_id = calimero_governance_store::get_group_for_context(&datastore, &context_id)?;

                    if group_id.is_none() {
                        let deadline = tokio::time::Instant::now() + GROUP_LOOKUP_TIMEOUT;
                        let started = tokio::time::Instant::now();
                        // Exponential backoff for the fallback re-sync. Seeded
                        // from the initial `sync_known_namespaces` above.
                        let mut resync_backoff = FALLBACK_POLL;
                        let mut last_resync = tokio::time::Instant::now();
                        loop {
                            // Race the notifier against a short poll interval: if
                            // the channel lagged (bursty traffic), we still catch
                            // the mapping via the periodic datastore recheck.
                            let recv = tokio::time::timeout(FALLBACK_POLL, rx.recv()).await;
                            match recv {
                                Ok(Ok(cid)) if cid == context_id => {
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    if group_id.is_some() {
                                        info!(
                                            %context_id,
                                            elapsed_ms = started.elapsed().as_millis() as u64,
                                            "resolved context->group mapping via registration signal"
                                        );
                                        break;
                                    }
                                }
                                Ok(Ok(_)) => {
                                    // Signal for a different context — keep waiting.
                                }
                                Ok(Err(RecvError::Lagged(skipped))) => {
                                    warn!(
                                        %context_id,
                                        skipped,
                                        "registration_notify lagged; falling back to datastore poll"
                                    );
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    if group_id.is_some() {
                                        break;
                                    }
                                }
                                Ok(Err(RecvError::Closed)) => {
                                    // Channel sender dropped; final datastore check then bail.
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    break;
                                }
                                Err(_elapsed) => {
                                    // Poll tick — recheck the datastore (cheap,
                                    // local) every interval, but throttle the
                                    // network namespace re-sync with exponential
                                    // backoff so an unresolved join doesn't
                                    // re-sync every 200ms for its whole budget.
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    if group_id.is_some() {
                                        break;
                                    }
                                    let now = tokio::time::Instant::now();
                                    if now.duration_since(last_resync) >= resync_backoff {
                                        sync_known_namespaces(&datastore, &node_client).await;
                                        last_resync = tokio::time::Instant::now();
                                        resync_backoff =
                                            (resync_backoff * 2).min(MAX_RESYNC_BACKOFF);
                                    }
                                }
                            }
                            if tokio::time::Instant::now() >= deadline {
                                break;
                            }
                        }
                    }
                }

                let group_id =
                    group_id.ok_or_else(|| eyre::eyre!("context does not belong to any group"))?;

                // Resolve joiner identity from node namespace identity.
                let (joiner_identity, _) = NamespaceRepository::new(&datastore)
                    .resolve_identity(&group_id)?
                    .ok_or_else(|| {
                            eyre::eyre!(
                            "node has no namespace identity for this group; join the group first"
                        )
                        })?;

                // Group membership covers both direct members and parent-chain
                // members inherited through `Open` subgroups (gated by the
                // `CAN_JOIN_OPEN_SUBGROUPS` capability at the anchor parent).
                // `Restricted` subgroups still require an explicit
                // `add_group_members` call by an admin.
                if MetaRepository::new(&datastore).load(&group_id)?.is_none() {
                    bail!("group not found");
                }
                let joiner_account =
                    crate::member_account::require(&datastore, &group_id, &joiner_identity)?;
                let membership_path =
                    MembershipRepository::new(&datastore).check_path(&group_id, &joiner_account)?;
                let mut was_inherited = false;
                match membership_path {
                    calimero_governance_store::MembershipPath::None => {
                        // A paired device is a member of NOTHING by design — its
                        // right to take part comes from the account its certificate
                        // binds it to. So the same fallback the authorization path
                        // uses applies here: is this key the `sign_pk` of a live
                        // device whose account a member endorsed?
                        //
                        // Without it a paired device gets scope keys and the right
                        // to author and still cannot follow a context, because
                        // following one means writing the keyless identity marker
                        // that makes this node "own" an identity there. The symptom
                        // is a bare "no owned identity found for this context" from
                        // the RPC layer, which names neither accounts nor devices.
                        let account = calimero_governance_store::member_account_for_device_key(
                            &datastore,
                            &group_id,
                            &joiner_identity,
                        )?;
                        let Some(account) = account else {
                            bail!(
                                "identity is not a member of the group, nor a live device of an \
                                 account that one endorsed"
                            );
                        };
                        info!(
                            target: "calimero::audit::group_membership",
                            group_id = %hex::encode(group_id.to_bytes()),
                            %joiner_identity,
                            %account,
                            %context_id,
                            "context join authorized as a device of a member's account"
                        );
                    }
                    calimero_governance_store::MembershipPath::Direct => {}
                    calimero_governance_store::MembershipPath::Inherited { anchor, via_admin } => {
                        // Audit trail: inherited members do not appear in
                        // `list_group_members` for the subgroup, so emit a
                        // structured log so admins can reconstruct who has
                        // access via the parent-walk inheritance path
                        // (issue #2256).
                        info!(
                            target: "calimero::audit::group_membership",
                            subgroup_id = %hex::encode(group_id.to_bytes()),
                            anchor_parent = %hex::encode(anchor.to_bytes()),
                            %joiner_identity,
                            %context_id,
                            via_admin,
                            "context join authorized via inherited subgroup membership"
                        );
                        was_inherited = true;
                    }
                }

                let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;
                let ns_identity = NamespaceRepository::new(&datastore).identity(&ns_id)?
                    .ok_or_else(|| eyre::eyre!("namespace identity not found"))?;
                let (_pk, sk_bytes) = ns_identity;

                let zero_app = calimero_primitives::application::ApplicationId::from([0u8; 32]);
                let config = if !context_client.has_context(&context_id)? {
                    let app_id = MetaRepository::new(&datastore).load(&group_id)?
                        .map(|meta| meta.target_application_id)
                        .filter(|id| *id != zero_app);

                    // Read service_name from the dedicated context service name key,
                    // written during ContextRegistered governance application.
                    let svc_name = calimero_governance_store::get_context_service_name(&datastore, &context_id)?;

                    Some(ContextConfigParams {
                        application_id: app_id,
                        application_revision: 0,
                        members_revision: 0,
                        service_name: svc_name,
                    })
                } else {
                    None
                };

                let _ignored = context_client
                    .sync_context_config(context_id, config)
                    .await?;

                {
                    let mut handle = datastore.handle();
                    // Keyless membership marker. `joiner_identity` is the node's
                    // namespace identity, so its private key is resolved live from
                    // the namespace identity at read time rather than copied here
                    // (see `resolve_owned_namespace_signer`). Peers already write
                    // keyless marker rows for members; this makes the owner match.
                    handle.put(
                        &calimero_store::key::ContextIdentity::new(context_id, joiner_identity),
                        &calimero_store::types::ContextIdentity { private_key: None },
                    )?;

                    // Clear any leave-tombstone written by a previous
                    // `leave_context` for this `(member, context)` pair —
                    // explicit rejoin means the user is opting back in, so
                    // auto-follow should not see the marker on future events.
                    let marker_key = calimero_store::key::ContextLeftMarker::new(
                        context_id,
                        joiner_identity,
                    );
                    if let Err(err) = handle.delete(&marker_key) {
                        warn!(
                            %context_id,
                            ?err,
                            "join_context: failed to clear leave marker — \
                             auto-follow may continue to skip this context until cleared"
                        );
                    }
                }

                node_client.subscribe(&context_id).await?;
                node_client.sync(Some(&context_id), None).await?;

                info!(
                    ?group_id,
                    ?context_id,
                    %joiner_identity,
                    "joined context via group membership"
                );

                // Inherited members (Open subgroup, joined via the
                // CAN_JOIN_OPEN_SUBGROUPS parent-walk) don't get a
                // `KeyDelivery` from any admin — admin never called
                // `add_group_members` for them. Without it they never
                // receive the group key (into their GroupKeyring), which
                // means they can't decrypt state-DAG messages and others
                // can't decrypt theirs. Publish `RootOp::MemberJoinedOpen`
                // signed by us so any peer holding the key responds with a
                // `KeyDelivery` (same mechanism `MemberJoined` uses for
                // invitation-based joins).
                //
                // Direct members are explicit-add via
                // `add_group_members` and already get a `KeyDelivery`
                // emitted alongside their `MemberAdded` — skip this
                // path for them.
                if was_inherited {
                    let signer_sk = calimero_primitives::identity::PrivateKey::from(sk_bytes);

                    // Deterministic key acquisition. This is the ONLY path by
                    // which an inherited joiner obtains the subgroup key:
                    // `MemberJoinedOpen` emits no `KeyDelivery` (the only
                    // emitters are `add_group_members`, `admit_tee_node` and
                    // `pair_device_complete`), so without this fetch the joiner
                    // stays keyless indefinitely. `join_subgroup_inheritance`
                    // already uses this direct-stream fetch (#2357); a context
                    // join needs the same certainty.
                    //
                    // Missing it fails silently and asymmetrically: state
                    // deltas still decrypt, because their lookup falls back to
                    // the namespace keyring by `key_id`, but ephemeral presence
                    // resolves strictly through `load_current_key_record` on
                    // the context's own group (rotation-as-eviction depends on
                    // that), so every presence message is dropped with no trace
                    // at default log level.
                    //
                    // Best-effort, like the publish below: the join has already
                    // happened locally, and the gossip round-trip remains as
                    // the fallback. Skipped when the key is already local, so
                    // repeat joins cost nothing.
                    let key_already_local =
                        match GroupKeyring::new(&datastore, group_id).load_current_key() {
                            Ok(k) => k.is_some(),
                            Err(err) => {
                                warn!(
                                    ?err,
                                    ?group_id,
                                    "join_context: keyring read failed -- attempting the \
                                     direct key fetch anyway"
                                );
                                false
                            }
                        };
                    if !key_already_local {
                        let fetched = match node_client
                            .request_open_subgroup_join(
                                ns_id.to_bytes(),
                                group_id.to_bytes(),
                                joiner_identity,
                            )
                            .await
                        {
                            Ok(bytes) => borsh::from_slice::<KeyEnvelope>(&bytes)
                                .map_err(|e| {
                                    eyre::eyre!("decode KeyEnvelope from peer response: {e}")
                                })
                                .and_then(|envelope| {
                                    GroupKeyring::unwrap_for_recipient(
                                        &signer_sk,
                                        &group_id.to_bytes(),
                                        None,
                                        &envelope,
                                    )
                                })
                                .and_then(|group_key| {
                                    crate::group_key_pull::adopt_pulled_group_key(
                                        &datastore,
                                        ns_id.to_bytes().into(),
                                        group_id,
                                        &group_key,
                                    )
                                }),
                            Err(err) => Err(err),
                        };
                        match fetched {
                            Ok(_key_id) => info!(
                                ?group_id,
                                %joiner_identity,
                                "join_context: fetched the inherited subgroup key directly"
                            ),
                            Err(err) => warn!(
                                ?err,
                                ?group_id,
                                %joiner_identity,
                                "join_context: direct subgroup-key fetch failed -- the joiner \
                                 has no subgroup key; presence and any subgroup-encrypted op \
                                 will be dropped until an admin re-adds them"
                            ),
                        }
                    }
                    // NOT `?`. By this point the join has already happened locally —
                    // the identity marker is written, the leave tombstone cleared,
                    // and the context subscribed and synced. Propagating here would
                    // report the whole join as failed while leaving the node a member
                    // of it, which is a worse state than the one it is reporting.
                    //
                    // A credential this node cannot build is the same class of
                    // problem as a publish it cannot complete, and is handled the
                    // same way a few lines below: warn and skip the publish. The
                    // joiner then waits on key delivery exactly as it did before
                    // joins carried a credential at all.
                    match crate::join_credential::build(&datastore, &ns_id, &joiner_identity) {
                    Err(err) => warn!(
                        ?err,
                        %joiner_identity,
                        %context_id,
                        "join_context: could not build the join credential — skipping the \
                         MemberJoinedOpen publish; the join stands, but key delivery to this \
                         inherited joiner waits for an admin to add_group_members them"
                    ),
                    Ok(join_account) => {
                    let op = calimero_context_client::local_governance::NamespaceOp::Root(
                        calimero_context_client::local_governance::RootOp::MemberJoinedOpen {
                            member: join_account.statement.account,
                            group_id: group_id.to_bytes().into(),
                            account: join_account,
                        },
                    );
                    if let Err(e) = calimero_governance_store::sign_apply_and_publish_namespace_op(
                        &datastore,
                        &node_client,
                        &ack_router,
                        ns_id.to_bytes().into(),
                        &signer_sk,
                        op,
                    )
                    .await
                    {
                        warn!(
                            ?e,
                            %joiner_identity,
                            %context_id,
                            "failed to publish MemberJoinedOpen — key delivery to this \
                             inherited joiner will be skipped; messages will appear local-only \
                             until an admin explicitly add_group_members the joiner"
                        );
                    }
                    }
                    }
                }

                Ok(JoinContextResponse {
                    context_id,
                    member_public_key: joiner_identity,
                })
            }
            .into_actor(self),
        )
    }
}

/// The namespaces this node should sync when resolving a context->group
/// mapping: exactly the distinct namespace roots it takes part in.
///
/// A join can only ultimately succeed in a namespace the node takes part in (the
/// join later requires `NamespaceRepository::resolve_identity` for the context's
/// owning group, which answers `None` where it does not), so this is the tightest
/// plausibly-owning set. It
/// deliberately does NOT enumerate every known group and resolve each to its
/// root — that re-synced a namespace once per subgroup and wasted fan-out and
/// network syncs on namespaces the node can never join into. `participating_namespaces`
/// is keyed by namespace id, so the result is already distinct.
fn namespaces_to_sync(datastore: &calimero_store::Store) -> eyre::Result<Vec<ContextGroupId>> {
    NamespaceRepository::new(datastore).participating_namespaces()
}

async fn sync_known_namespaces(
    datastore: &calimero_store::Store,
    node_client: &calimero_node_primitives::client::NodeClient,
) {
    let namespaces = match namespaces_to_sync(datastore) {
        Ok(namespaces) => namespaces,
        Err(err) => {
            warn!(error = ?err, "failed to enumerate namespace identities for sync");
            return;
        }
    };

    for namespace in namespaces {
        let namespace_id = namespace.to_bytes();
        if let Err(err) = node_client.subscribe_namespace(namespace_id).await {
            warn!(?namespace, error = ?err, "failed to subscribe namespace during join_context");
        }
        if let Err(err) = node_client.sync_namespace(namespace_id).await {
            warn!(?namespace, error = ?err, "failed to sync namespace during join_context");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{MetaRepository, NamespaceRepository};
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::namespaces_to_sync;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn store_identity(store: &Store, namespace: &ContextGroupId) {
        let sk = PrivateKey::from([0x33; 32]);
        NamespaceRepository::new(store)
            .store_identity(namespace, &sk.public_key(), sk.as_bytes())
            .expect("store identity");
    }

    fn save_meta(store: &Store, group: &ContextGroupId) {
        let pk = PublicKey::from([0xAB; 32]);
        MetaRepository::new(store)
            .save(
                group,
                &GroupMetaValue {
                    bytecode_id: [0x11; 32],
                    target_application_id: ApplicationId::from([0xCC; 32]),
                    created_at: 1_700_000_000,
                    admin_identity: crate::test_support::account_for(&pk),
                    owner_identity: crate::test_support::account_for(&pk),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save meta");
    }

    #[test]
    fn syncs_only_identity_holding_namespaces_not_every_group() {
        let store = store();

        // Two namespace roots the node holds an identity in.
        let ns_a = ContextGroupId::from([0xA0; 32]);
        let ns_b = ContextGroupId::from([0xB0; 32]);
        store_identity(&store, &ns_a);
        store_identity(&store, &ns_b);

        // A subgroup under A. The OLD code enumerated every group and resolved
        // each back to its root, so A would have been synced once per subgroup;
        // the identity-keyed set has no such duplication.
        let sub_a = ContextGroupId::from([0xA1; 32]);
        NamespaceRepository::new(&store)
            .nest(&ns_a, &sub_a)
            .expect("nest subgroup under A");
        save_meta(&store, &sub_a);

        // A root group the node has META for but NO identity in. The OLD
        // `enumerate_all` would have synced it; the join can never succeed there
        // (no identity), so the narrowed set must EXCLUDE it.
        let group_c = ContextGroupId::from([0xC0; 32]);
        save_meta(&store, &group_c);

        let mut got = namespaces_to_sync(&store).expect("namespaces_to_sync");
        got.sort();
        let mut want = vec![ns_a, ns_b];
        want.sort();

        assert_eq!(
            got, want,
            "must sync exactly the identity-holding namespace roots (A, B), \
             excluding the no-identity group C and without duplicating A per subgroup"
        );
    }

    #[test]
    fn no_identities_yields_empty() {
        let store = store();
        save_meta(&store, &ContextGroupId::from([0xC0; 32]));
        assert!(
            namespaces_to_sync(&store)
                .expect("namespaces_to_sync")
                .is_empty(),
            "a node with meta but no identities syncs nothing"
        );
    }
}
