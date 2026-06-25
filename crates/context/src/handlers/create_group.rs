use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{CreateGroupRequest, CreateGroupResponse};
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_context_config::types::AppKey;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::key::GroupMetaValue;
use calimero_store::types::ApplicationMeta as ApplicationMetaValue;
use calimero_store::Store;
use rand::Rng;
use tracing::{info, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository, MetadataRepository,
    SigningKeysRepository,
};

impl Handler<CreateGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupRequest {
            group_id,
            app_key,
            application_id,
            upgrade_policy,
            name,
            parent_group_id,
            restricted,
        }: CreateGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let group_id = group_id.unwrap_or_else(|| {
            let bytes: [u8; 32] = rand::thread_rng().gen();
            bytes.into()
        });

        if let Ok(Some(_)) = MetaRepository::new(&self.datastore).load(&group_id) {
            return ActorResponse::reply(Err(eyre::eyre!("group '{group_id:?}' already exists")));
        }

        let namespace_anchor_group_id = parent_group_id.as_ref().unwrap_or(&group_id);
        let (namespace_id, admin_identity, sk_bytes, _sender) =
            match self.get_or_create_namespace_identity(namespace_anchor_group_id) {
                Ok(result) => result,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to resolve namespace identity: {err}"
                    )))
                }
            };

        let signing_key = Some(sk_bytes);

        // Subgroups inherit target_application_id from the parent (namespace root owns the app).
        let effective_application_id = if let Some(ref parent_id) = parent_group_id {
            let parent_meta = match MetaRepository::new(&self.datastore).load(parent_id) {
                Ok(Some(m)) => m,
                _ => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "parent group '{parent_id:?}' not found"
                    )));
                }
            };
            // Authorization. Namespace-root admins may create a subgroup at
            // any depth. A non-admin namespace member may create one *directly
            // under the namespace root* if they hold `CAN_CREATE_SUBGROUP`
            // (honored only at root level — see the capability's doc and
            // `execute_group_created`, which re-checks this on every peer).
            let is_namespace_admin = match MembershipRepository::new(&self.datastore)
                .is_admin(&namespace_id, &admin_identity)
            {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
            if !is_namespace_admin {
                if *parent_id != namespace_id {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "creating a subgroup under non-root parent '{parent_id:?}' requires \
                         namespace admin (delegated nested-subgroup creation is not yet supported)"
                    )));
                }
                if let Err(err) =
                    calimero_governance_store::PermissionChecker::new(&self.datastore, *parent_id)
                        .require_can_create_subgroup(&admin_identity)
                {
                    return ActorResponse::reply(Err(err));
                }
            }
            parent_meta.target_application_id
        } else {
            application_id
        };

        let app_meta = match load_app_meta(&self.datastore, &effective_application_id) {
            Ok(m) => m,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Derive app_key from the resolved application's bytecode blob_id
        // when the caller didn't provide one. This is the same value that
        // `set_target_application` (upgrade_group's apply path) writes after
        // an upgrade, so the cascade predicate (from_app_key == descendant
        // app_key) walks into freshly-created subgroups without needing a
        // pre-cascade alignment upgrade. A randomly-seeded app_key, which
        // is what this used to do, made every cascade silently skip the
        // descendant subtree.
        //
        // A caller-provided app_key pins the group to a specific version;
        // it is verified inside the async block below (blob present locally
        // + manifest package matches the row's package).
        let row_blob = *app_meta.bytecode.blob_id().as_ref();
        let app_package = app_meta.package.clone();
        let requested_app_key = app_key;

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        // Auto-store signing key for future use (group is about to be created with
        // admin_identity as the first admin, so store it keyed to that identity)
        if let Some(ref sk) = signing_key {
            let _ = SigningKeysRepository::new(&self.datastore).store_key(
                &group_id,
                &admin_identity,
                sk,
            );
        }

        ActorResponse::r#async(
            async move {
                let app_key = match requested_app_key {
                    Some(requested) => {
                        verify_requested_app_key(&node_client, &requested, row_blob, &app_package)
                            .await?;
                        requested
                    }
                    None => AppKey::from(row_blob),
                };

                // Local cache write
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                let meta = GroupMetaValue {
                    app_key: app_key.to_bytes(),
                    target_application_id: effective_application_id,
                    upgrade_policy,
                    created_at: now,
                    admin_identity,
                    // Creator is the initial Owner. Transferable via
                    // `GroupOp::TransferOwnership`.
                    owner_identity: admin_identity,
                    migration: None,
                    auto_join: true,
                };

                MetaRepository::new(&datastore).save(&group_id, &meta)?;
                MembershipRepository::new(&datastore).add_member(
                    &group_id,
                    &admin_identity,
                    GroupMemberRole::Admin,
                )?;

                // Set default capabilities so new members can be inherited
                // into Open subgroups beneath this group.
                CapabilitiesRepository::new(&datastore).set_default_capabilities(
                    &group_id,
                    calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
                )?;

                // Generate and store the group encryption key.
                let group_key: [u8; 32] = rand::thread_rng().gen();
                let key_id = GroupKeyring::new(&datastore, group_id).store_key(&group_key)?;
                tracing::debug!(
                    ?group_id,
                    key_id = %hex::encode(key_id),
                    "stored initial group key"
                );

                if let Some(ref n) = name {
                    // Seed the group's initial metadata record locally, stamped
                    // with the creator's identity / wall-clock — not the
                    // zero-value `Default` (which would surface through the API
                    // as misleading provenance). Like under the former alias,
                    // this is a local seed; later `GroupOp::GroupMetadataSet`
                    // ops replicate and supersede it. The name is validated
                    // here too — the seed bypasses the op-apply validator.
                    match calimero_primitives::metadata::validate_metadata_payload(
                        Some(n),
                        &std::collections::BTreeMap::new(),
                    ) {
                        Ok(()) => MetadataRepository::new(&datastore).set_group(
                            &group_id,
                            &calimero_primitives::metadata::MetadataRecord {
                                name: name.clone(),
                                updated_at: calimero_governance_store::now_millis(),
                                updated_by: admin_identity,
                                ..Default::default()
                            },
                        )?,
                        Err(e) => {
                            warn!(?group_id, reason = %e, "ignoring invalid group name on create")
                        }
                    }
                }

                // In the namespace model, group hierarchy is tracked in the
                // namespace DAG (RootOp::GroupCreated), not via parent refs.
                if let Err(err) = node_client
                    .subscribe_namespace(namespace_id.to_bytes())
                    .await
                {
                    warn!(
                        ?err,
                        namespace_id=%hex::encode(namespace_id.to_bytes()),
                        "failed to subscribe to namespace before publishing governance ops"
                    );
                }

                let signer_sk = PrivateKey::from(sk_bytes);
                // Strict-tree refactor: GroupCreated is now an atomic
                // create+nest op. It ONLY applies to subgroups — the namespace
                // root itself has no parent by definition.
                //
                // #2474: root creation (parent_group_id is None) now emits a
                // replayable `RootOp::NamespaceCreated { founder }` GENESIS op so
                // a bootstrapping replica derives the founding admin/owner
                // authoritatively from the synced DAG instead of TOFU-seeding it
                // from the KeyDelivery signer. This is the FIRST op in the
                // namespace DAG (nonce 0, no parents — the head record is empty
                // for a brand-new namespace), signed+published via the same path
                // subgroup GroupCreated uses. It self-authorizes on apply
                // (genesis establishes authority; see
                // `ops/namespace/namespace_created.rs`). Previously root creation
                // emitted NO op and the founder lived only in the creator's local
                // GroupMeta, which is exactly the gap #2474 closes.
                if let Some(parent_id) = parent_group_id {
                    let create_op = NamespaceOp::Root(RootOp::GroupCreated {
                        group_id: group_id.to_bytes(),
                        parent_id: parent_id.to_bytes(),
                        restricted,
                    });
                    match calimero_governance_store::sign_apply_and_publish_namespace_op(
                        &datastore,
                        &node_client,
                        &ack_router,
                        namespace_id.to_bytes(),
                        &signer_sk,
                        create_op,
                    )
                    .await
                    {
                        Ok(report) => {
                            report.observe("create_group", "GroupCreated");
                            // C3 Stage 1: the local author wrote GroupCreated to the
                            // gov-DAG; land it in the unified op-store too so the
                            // op-store stays a complete mirror (by construction).
                            crate::scope_projection::ScopeProjections::persist_namespace_head_ops(
                                &datastore,
                                namespace_id.to_bytes(),
                            );
                        }
                        Err(e) => {
                            // Subgroup GroupCreated intentionally keeps warn-and-
                            // continue: unlike the namespace-ROOT genesis below, a
                            // subgroup's authoritative state is recoverable by re-
                            // applying the (idempotent) GroupCreated op, and a missing
                            // subgroup op does not strand the namespace founder.
                            tracing::warn!(?e, "failed to publish GroupCreated on namespace DAG");
                        }
                    }
                } else {
                    let genesis_op = NamespaceOp::Root(RootOp::NamespaceCreated {
                        founder: admin_identity,
                    });
                    match calimero_governance_store::sign_apply_and_publish_namespace_op(
                        &datastore,
                        &node_client,
                        &ack_router,
                        namespace_id.to_bytes(),
                        &signer_sk,
                        genesis_op,
                    )
                    .await
                    {
                        Ok(report) => {
                            report.observe("create_group", "NamespaceCreated");
                            crate::scope_projection::ScopeProjections::persist_namespace_head_ops(
                                &datastore,
                                namespace_id.to_bytes(),
                            );
                        }
                        Err(e) => {
                            // An `Err` here is, by contract, a LOCAL APPLY failure —
                            // never a publish/transport failure (#2474 reviewer batch 5).
                            //
                            // `sign_apply_and_publish_namespace_op` is apply-FIRST and
                            // publish-BEST-EFFORT: it `?`-propagates only the local DAG
                            // mutation (sign/hash/`apply_signed_op`), while EVERY
                            // publish/transport error — including the normal cold-start
                            // `NoPeersSubscribedToTopic` — is caught internally and
                            // downgraded to a `Degraded` `Ok(report)`. So an `Err` here
                            // already means the genesis op did NOT apply to our own
                            // store; there is no no-peers case to special-case (a
                            // namespace created offline / on a single node still gets
                            // `Ok` because apply succeeds and the publish is swallowed).
                            // The assert pins that contract so a future change that
                            // started surfacing publish errors trips loudly in test/dev
                            // instead of silently rolling back a perfectly-good genesis.
                            debug_assert!(
                                !calimero_network_primitives::client::is_no_peers_subscribed_error(
                                    &e
                                ),
                                "sign_apply_and_publish is apply-first/publish-best-effort; \
                                 Err must mean a local apply failure, not no-peers"
                            );

                            // FATAL for namespace-ROOT creation (#2474): the genesis op
                            // is what makes the founder authoritative on the DAG. A LOCAL
                            // APPLY failure means the namespace would exist locally with
                            // correct meta but NO genesis on the DAG — and a backfilling
                            // replica would fall back to the broken TOFU seed
                            // (`seed_bootstrap_admin_if_absent`), pinning the wrong admin.
                            // That is exactly the production bug this PR fixes, so a true
                            // apply failure MUST fail the create.
                            //
                            // ROLLBACK (#2474 reviewer batch 3): the local root rows were
                            // already written above (the `GroupMetaValue`, the founder
                            // Admin member row, the default caps, the group encryption
                            // key, the auto-stored signing key, and the optional name
                            // metadata seed). Leaving them behind on a genesis-apply
                            // failure would strand an orphaned root: the top-of-handler
                            // "group already exists" guard would then make every retry
                            // with the same group id fail PERMANENTLY (unrecoverable
                            // without store surgery), while the DAG carries no genesis.
                            // calimero-store has no atomic multi-key write, so we undo
                            // each write explicitly, mirroring the writes above, before
                            // returning Err. After this the namespace is cleanly ABSENT
                            // and a retry with the same group id flows through the normal
                            // create path again. Each delete is idempotent; we log (not
                            // propagate) any delete error so a partial rollback can't mask
                            // the original apply failure, and so the most useful error
                            // (the genesis apply failure) is the one surfaced.
                            if let Err(re) = MetaRepository::new(&datastore).delete(&group_id) {
                                warn!(?re, ?group_id, "rollback: failed to delete root meta");
                            }
                            if let Err(re) = MembershipRepository::new(&datastore)
                                .remove_member(&group_id, &admin_identity)
                            {
                                warn!(
                                    ?re,
                                    ?group_id,
                                    "rollback: failed to delete founder member row"
                                );
                            }
                            if let Err(re) =
                                CapabilitiesRepository::new(&datastore).delete_default(&group_id)
                            {
                                warn!(?re, ?group_id, "rollback: failed to delete default caps");
                            }
                            if let Err(re) =
                                GroupKeyring::new(&datastore, group_id).delete_key_by_id(&key_id)
                            {
                                warn!(?re, ?group_id, "rollback: failed to delete group key");
                            }
                            if let Err(re) = SigningKeysRepository::new(&datastore)
                                .delete_key(&group_id, &admin_identity)
                            {
                                warn!(?re, ?group_id, "rollback: failed to delete signing key");
                            }
                            if name.is_some() {
                                if let Err(re) =
                                    MetadataRepository::new(&datastore).delete_group(&group_id)
                                {
                                    warn!(
                                        ?re,
                                        ?group_id,
                                        "rollback: failed to delete group name metadata"
                                    );
                                }
                            }
                            return Err(eyre::eyre!(
                                "failed to apply NamespaceCreated genesis on namespace DAG; \
                                 aborting namespace-root creation and rolling back local \
                                 root rows so a retry with the same group id succeeds \
                                 (genesis must be atomic with root creation, #2474): {e}"
                            ));
                        }
                    }
                }

                info!(?group_id, ?parent_group_id, %admin_identity, "group created");

                Ok(CreateGroupResponse { group_id })
            }
            .into_actor(self),
        )
    }
}

fn load_app_meta(
    datastore: &Store,
    application_id: &calimero_primitives::application::ApplicationId,
) -> eyre::Result<ApplicationMetaValue> {
    let handle = datastore.handle();
    let key = calimero_store::key::ApplicationMeta::new(*application_id);
    handle
        .get(&key)?
        .ok_or_else(|| eyre::eyre!("application '{application_id}' not found"))
}

/// A caller-chosen `app_key` must point at locally-present bytecode of the
/// SAME package as the group's application row — otherwise the group would
/// bind to bytecode the node cannot execute, or to another app entirely.
async fn verify_requested_app_key(
    node_client: &calimero_node_primitives::client::NodeClient,
    app_key: &AppKey,
    row_blob: [u8; 32],
    expected_package: &str,
) -> eyre::Result<()> {
    let key_bytes = app_key.to_bytes();
    if key_bytes == [0u8; 32] {
        eyre::bail!("app_key must not be zero");
    }
    if key_bytes == row_blob {
        return Ok(()); // the row's own blob is trivially valid
    }
    let blob_id = calimero_primitives::blobs::BlobId::from(key_bytes);
    if !node_client.has_blob(&blob_id)? {
        eyre::bail!("app_key blob '{blob_id}' is not present locally; install that version first");
    }
    let Some(manifest) = node_client.bundle_manifest_for_blob(&blob_id).await? else {
        eyre::bail!("app_key blob '{blob_id}' is not an application bundle");
    };
    if manifest.package != expected_package {
        eyre::bail!(
            "app_key blob '{blob_id}' belongs to package '{}', expected '{expected_package}'",
            manifest.package
        );
    }
    Ok(())
}
