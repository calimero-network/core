use calimero_governance_store::SigningKeysRepository;
use calimero_governance_store::{MembershipRepository, MetaRepository, UpgradesRepository};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorFutureExt, ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_context_client::group::{UpgradeGroupRequest, UpgradeGroupResponse};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_client::messages::MigrationParams;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{self, GroupUpgradeStatus, GroupUpgradeValue};
use calimero_wasm_abi::downgrade::identity_downgrades;
use calimero_wasm_abi::schema::Manifest;
use eyre::bail;
use tracing::{debug, error, info, warn};

use crate::ContextManager;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

impl Handler<UpgradeGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpgradeGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpgradeGroupRequest {
            group_id,
            target_application_id,
            requester,
            cascade,
        }: UpgradeGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_namespace_identity(&group_id);

        // Resolve requester: use provided value or fall back to node group identity
        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        // Resolve signing_key from node identity key
        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        // Cascade path: emit `GroupOp::CascadeUpgrade` and dispatch one
        // `propagate_upgrade` per descendant subgroup whose current
        // `app_key` matches the signed group's current `app_key`.
        //
        // The single-group branch below stays bit-identical for
        // `cascade = false` (the historical default).
        //
        // The cascade flow bypasses the single-group `validate_upgrade`
        // preamble because (a) the signed group on a cascade is often a
        // namespace root with no contexts of its own, and (b) cascade
        // dispatches one propagator per matched descendant rather than
        // one canary against a single context list.
        if cascade {
            return dispatch_cascade(
                self,
                group_id,
                target_application_id,
                requester,
                signing_key,
                node_identity,
            );
        }

        // --- Synchronous validation ---
        let preamble = match validate_upgrade(
            &self.datastore,
            &group_id,
            &target_application_id,
            &requester,
            signing_key.is_some(),
        ) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let UpgradePreamble {
            canary_context_id,
            total_contexts,
            upgrade_policy,
            from_version,
            to_version,
            current_application_id,
            current_app_key,
        } = preamble;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Auto-store signing key ONLY when the requester IS the node's own identity
        if let (Some(sk), Some((node_pk, _))) = (signing_key, node_identity) {
            if requester == node_pk {
                let _ = SigningKeysRepository::new(&self.datastore)
                    .store_key(&group_id, &requester, &sk);
            }
        }

        // Build contract call if signing_key is available (or from stored key)
        let effective_signing_key = signing_key.or_else(|| {
            SigningKeysRepository::new(&self.datastore)
                .get_key(&group_id, &requester)
                .ok()
                .flatten()
        });
        let app_meta_for_contract = match (|| {
            let handle = self.datastore.handle();
            let key = key::ApplicationMeta::new(target_application_id);
            handle
                .get(&key)?
                .ok_or_else(|| eyre::eyre!("target application not found"))
        })() {
            Ok(meta) => Some(meta),
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        // --- LazyOnAccess: update target and return without canary/propagator ---
        // Contexts will be upgraded individually on their next execution.
        // Launching a propagator would race with the lazy mechanism and could
        // invoke migration functions twice on the same context.
        //
        // Save the upgrade record as Completed immediately — InProgress serves
        // no purpose for lazy upgrades (no propagator runs) and would
        // permanently block future upgrades since nothing transitions it out.
        if matches!(upgrade_policy, UpgradePolicy::LazyOnAccess) {
            let datastore = self.datastore.clone();
            let ack_router_for_lazy = Arc::clone(&ack_router);
            let target_blob_bytes: Option<[u8; 32]> = app_meta_for_contract
                .as_ref()
                .map(|m| *m.bytecode.blob_id().as_ref());
            return ActorResponse::r#async(
                async move {
                    // Resolve the upgrade from the apps' embedded ABIs (every
                    // service of the bundle); the wire carries the one
                    // resolved method for receivers' lazy actuation.
                    let migration = {
                        let target_blob = target_blob_bytes
                            .ok_or_else(|| eyre::eyre!("target application not found"))?;
                        resolve_upgrade_from_abis(&node_client, current_app_key, target_blob)
                            .await?
                    };
                    let migration_bytes = migration.as_ref().map(|m| m.method.as_bytes().to_vec());
                    let has_migration = migration.is_some();
                    // L1 identity-downgrade gate: a migration upgrade may not strip
                    // identity from a top-level state field. Runs BEFORE any group op
                    // is emitted so a forbidden downgrade never reaches the network.
                    // Fail-open when either app lacks an embedded ABI section.
                    if has_migration {
                        let old = resolve_pre_upgrade_schema(
                            &node_client,
                            current_app_key,
                            &current_application_id,
                        )
                        .await;
                        let new =
                            resolve_embedded_schema(&node_client, &target_application_id).await;
                        verify_no_identity_downgrade(old.as_ref(), new.as_ref())?;
                    }
                    {
                        let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                            eyre::eyre!(
                                "local group upgrade requires a signing key for the requester"
                            )
                        })?);
                        let app_meta = app_meta_for_contract
                            .as_ref()
                            .ok_or_else(|| eyre::eyre!("target application not found"))?;
                        let app_key = *app_meta.bytecode.blob_id().as_ref();
                        let report = calimero_governance_store::sign_apply_and_publish(
                            &datastore,
                            &node_client,
                            &ack_router_for_lazy,
                            &group_id,
                            &sk,
                            GroupOp::TargetApplicationSet {
                                app_key,
                                target_application_id,
                            },
                        )
                        .await?;
                        report.observe("upgrade_group", "TargetApplicationSet");
                        // Unconditional: a code-only upgrade (migration None)
                        // must CLEAR any method left by an earlier migration
                        // upgrade — a stale Some(method) makes the same-id
                        // lazy trigger take the migration arm and short-circuit
                        // on its applied marker, so the new bytecode would
                        // never activate.
                        let report = calimero_governance_store::sign_apply_and_publish(
                            &datastore,
                            &node_client,
                            &ack_router_for_lazy,
                            &group_id,
                            &sk,
                            GroupOp::GroupMigrationSet {
                                migration: migration_bytes.clone(),
                            },
                        )
                        .await?;
                        report.observe("upgrade_group", "GroupMigrationSet");
                    }

                    let mut meta = MetaRepository::new(&datastore)
                        .load(&group_id)?
                        .ok_or_else(|| eyre::eyre!("group not found"))?;
                    meta.target_application_id = target_application_id;
                    meta.migration = migration_bytes.clone();
                    MetaRepository::new(&datastore).save(&group_id, &meta)?;

                    // LazyOnAccess: contexts upgrade individually on demand; there is no single
                    // "all done" moment, so completed_at is None.
                    let completed_status = GroupUpgradeStatus::Completed { completed_at: None };

                    let upgrade_value = GroupUpgradeValue {
                        from_version,
                        to_version,
                        migration: migration_bytes,
                        initiated_at: now,
                        initiated_by: requester,
                        status: completed_status.clone(),
                        cascade_hlc: None,
                        cascade_seq: None,
                    };

                    UpgradesRepository::new(&datastore).save(&group_id, &upgrade_value)?;

                    info!(
                        ?group_id,
                        %target_application_id,
                        "LazyOnAccess upgrade target set; contexts will upgrade on next access"
                    );

                    let contexts = calimero_governance_store::enumerate_group_contexts(
                        &datastore,
                        &group_id,
                        0,
                        usize::MAX,
                    )?;

                    // Announce target app blob on DHT for each group context so
                    // peer nodes can discover and fetch it during group sync.
                    if let Some(ref app_meta) = app_meta_for_contract {
                        let blob_id = app_meta.bytecode.blob_id();
                        for context_id in &contexts {
                            if let Err(err) = node_client
                                .announce_blob_to_network(&blob_id, context_id, app_meta.size)
                                .await
                            {
                                warn!(%err, "failed to announce target app blob");
                            }
                        }
                    }

                    Ok(UpgradeGroupResponse {
                        group_id,
                        status: completed_status.into(),
                    })
                }
                .into_actor(self),
            );
        }

        // --- Persist InProgress BEFORE the async canary ---
        // This prevents a concurrent UpgradeGroupRequest from passing
        // validate_upgrade while the canary is still running.
        let initial_status = GroupUpgradeStatus::InProgress {
            total: total_contexts as u32,
            completed: 0,
            failed: 0,
        };

        let upgrade_value = GroupUpgradeValue {
            from_version,
            to_version,
            // Non-lazy upgrades are code-only by construction: the canary
            // guard below rejects any target that declares a migration.
            migration: None,
            initiated_at: now,
            initiated_by: requester,
            status: initial_status.clone(),
            cascade_hlc: None,
            cascade_seq: None,
        };

        if let Err(err) = UpgradesRepository::new(&self.datastore).save(&group_id, &upgrade_value) {
            return ActorResponse::reply(Err(err.into()));
        }

        // --- Async: run canary upgrade ---
        let context_client = self.context_client.clone();
        let datastore_for_canary = self.datastore.clone();
        let datastore = self.datastore.clone();

        let canary_signer = match calimero_governance_store::find_local_signing_identity(
            &self.datastore,
            &canary_context_id,
        ) {
            Ok(Some(s)) => s,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "no local signing identity for canary context {canary_context_id}"
                )))
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let target_blob_info = app_meta_for_contract
            .as_ref()
            .map(|m| (m.bytecode.blob_id(), m.size));
        let ack_router_for_canary = Arc::clone(&ack_router);
        let target_blob_bytes: Option<[u8; 32]> = app_meta_for_contract
            .as_ref()
            .map(|m| *m.bytecode.blob_id().as_ref());
        let canary_task = async move {
            // Guard: the target may DECLARE a migration in its ABI.
            // Migrations only run under LazyOnAccess — proceeding here
            // (Automatic/Coordinated) would swap bytecode under live state
            // without running the declared migration. Also rejects
            // missing-edge / multi-hop targets.
            if let Some(target_blob) = target_blob_bytes {
                if let Some(declared) =
                    resolve_upgrade_from_abis(&node_client, current_app_key, target_blob).await?
                {
                    eyre::bail!(
                        "target app declares migration '{}' in its ABI; migrations only \
                         run under the LazyOnAccess upgrade policy",
                        declared.method
                    );
                }
            }
            {
                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!("local group upgrade requires a signing key for the requester")
                })?);
                let app_meta = app_meta_for_contract
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("target application not found"))?;
                let app_key = *app_meta.bytecode.blob_id().as_ref();
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore_for_canary,
                    &node_client,
                    &ack_router_for_canary,
                    &group_id,
                    &sk,
                    GroupOp::TargetApplicationSet {
                        app_key,
                        target_application_id,
                    },
                )
                .await?;
                report.observe("upgrade_group", "TargetApplicationSet");
                // Unconditional — see the LazyOnAccess site: clearing a stale
                // method on code-only upgrades keeps the same-id lazy trigger
                // out of the migration arm.
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore_for_canary,
                    &node_client,
                    &ack_router_for_canary,
                    &group_id,
                    &sk,
                    GroupOp::GroupMigrationSet { migration: None },
                )
                .await?;
                report.observe("upgrade_group", "GroupMigrationSet");
            }

            context_client
                .update_application(
                    &canary_context_id,
                    &target_application_id,
                    &canary_signer,
                    None,
                )
                .await
        }
        .into_actor(self);

        let group_id_clone = group_id;
        let context_client_for_propagator = self.context_client.clone();
        let datastore_for_propagator = self.datastore.clone();
        let node_client_for_gossip = self.node_client.clone();
        let datastore_for_gossip = self.datastore.clone();

        ActorResponse::r#async(canary_task.map(
            move |canary_result, act, ctx| match canary_result {
                Err(err) => {
                    error!(
                        ?group_id,
                        canary=%canary_context_id,
                        ?err,
                        "canary upgrade failed, aborting group upgrade"
                    );
                    // Clean up the InProgress record so the group can be retried
                    if let Err(cleanup_err) =
                        UpgradesRepository::new(&datastore).delete(&group_id_clone)
                    {
                        error!(
                            ?group_id,
                            ?cleanup_err,
                            "failed to clean up upgrade record after canary failure"
                        );
                    }
                    Err(eyre::eyre!(
                        "canary upgrade failed on context {canary_context_id}: {err}"
                    ))
                }
                Ok(()) => {
                    info!(
                        ?group_id,
                        canary=%canary_context_id,
                        "canary upgrade succeeded, proceeding with group upgrade"
                    );

                    // Update group's target_application_id
                    let mut meta = MetaRepository::new(&datastore)
                        .load(&group_id_clone)?
                        .ok_or_else(|| eyre::eyre!("group not found after canary"))?;

                    meta.target_application_id = target_application_id;
                    MetaRepository::new(&datastore).save(&group_id_clone, &meta)?;

                    // Update InProgress status (canary = 1 completed)
                    let status = GroupUpgradeStatus::InProgress {
                        total: total_contexts as u32,
                        completed: 1,
                        failed: 0,
                    };

                    update_upgrade_status(&datastore, &group_id_clone, status.clone())?;

                    // Gossip upgrade notification to peers
                    if let Ok(contexts) = calimero_governance_store::enumerate_group_contexts(
                        &datastore_for_gossip,
                        &group_id_clone,
                        0,
                        usize::MAX,
                    ) {
                        let nc = node_client_for_gossip;
                        ctx.spawn(
                            async move {
                                if let Some((blob_id, blob_size)) = target_blob_info {
                                    for context_id in &contexts {
                                        if let Err(err) = nc
                                            .announce_blob_to_network(
                                                &blob_id, context_id, blob_size,
                                            )
                                            .await
                                        {
                                            warn!(
                                                %err,
                                                "failed to announce target app blob"
                                            );
                                        }
                                    }
                                }
                            }
                            .into_actor(act),
                        );
                    }

                    // Spawn propagator for remaining contexts
                    if total_contexts > 1 {
                        act.active_propagators.insert(group_id_clone);

                        let propagator = propagate_upgrade(
                            context_client_for_propagator,
                            datastore_for_propagator,
                            group_id_clone,
                            target_application_id,
                            None,
                            Some(canary_context_id),
                            1, // canary already upgraded
                        );
                        ctx.spawn(propagator.into_actor(act).map(move |_, act, _| {
                            act.active_propagators.remove(&group_id_clone);
                        }));
                    } else {
                        // Only one context (the canary) — mark completed
                        let completed_at = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let completed_status = GroupUpgradeStatus::Completed {
                            completed_at: Some(completed_at),
                        };
                        update_upgrade_status(
                            &datastore,
                            &group_id_clone,
                            completed_status.clone(),
                        )?;

                        return Ok(UpgradeGroupResponse {
                            group_id: group_id_clone,
                            status: completed_status.into(),
                        });
                    }

                    Ok(UpgradeGroupResponse {
                        group_id: group_id_clone,
                        status: status.into(),
                    })
                }
            },
        ))
    }
}

/// L1 identity-downgrade gate. Refuse a migration upgrade that strips identity
/// from a top-level state field (e.g. AuthoredMap -> UnorderedMap). Fail-OPEN
/// (allow, with a warning) when either schema is unavailable — apps built before
/// ABI embedding have no `calimero_abi_v1` section and cannot be checked.
///
/// Scope: callers run this gate only for migration upgrades (`has_migration`).
/// A code-only upgrade does not transform existing state — the new bytecode
/// reads the old data in place, so an incompatible top-level type change fails
/// to deserialize rather than silently re-interpreting identity-gated entries as
/// plain. Extending the gate to code-only app swaps is a possible defence-in-depth
/// follow-up (tracked under #2587).
fn verify_no_identity_downgrade(
    old: Option<&Manifest>,
    new: Option<&Manifest>,
) -> eyre::Result<()> {
    let (Some(old), Some(new)) = (old, new) else {
        tracing::warn!(
            "L1 identity-downgrade gate: skipped (one side has no embedded ABI / legacy app); allowing upgrade"
        );
        return Ok(());
    };
    if let Some(d) = identity_downgrades(old, new).into_iter().next() {
        tracing::warn!(
            field = %d.field, from = %d.from, to = %d.to,
            "identity downgrade forbidden: refusing migration upgrade that strips authorship/writer-ACL"
        );
        eyre::bail!(
            "identity downgrade forbidden: field '{}' {} -> {} strips authorship/writer-ACL network-wide \
             (use owner-driven rewrite; see #2534)",
            d.field, d.from, d.to
        );
    }
    Ok(())
}

/// Read a context application's embedded state schema, or None if unavailable
/// (no blob, no embedded section, or a read error — all fail-open).
/// "From" side of the L1 gate. An in-place (same-id) bundle install leaves
/// the application row already on the NEW wasm — comparing row-to-row would
/// diff a manifest against itself and wave a downgrade through. The group's
/// `app_key` is the bytecode the group actually runs before this upgrade
/// (stamped by the previous upgrade / group creation), so read the "from"
/// ABI from that blob; fall back to the row when the blob is unreadable
/// (legacy randomly-seeded app_key → fail-open, same as before).
async fn resolve_pre_upgrade_schema(
    node_client: &calimero_node_primitives::client::NodeClient,
    current_app_key: [u8; 32],
    current_application_id: &ApplicationId,
) -> Option<Manifest> {
    if current_app_key != [0u8; 32] {
        let blob = calimero_primitives::blobs::BlobId::from(current_app_key);
        match node_client.application_bytes_from_blob(&blob, None).await {
            Ok(Some(bytes)) => return calimero_wasm_abi::embed::read_embedded_state_schema(&bytes),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    %current_application_id, error = %err,
                    "L1 gate: failed to read current app_key bytecode; falling back to the row"
                );
            }
        }
    }
    resolve_embedded_schema(node_client, current_application_id).await
}

/// Emit-side ABI resolution for an upgrade: scan EVERY service in the target
/// bundle, plan each against the same service in the group's current
/// bytecode, and aggregate. The wire carries one method (the first declared
/// edge for this hop, deterministic in bundle service order); receivers
/// re-resolve per context at actuation, so services without an edge activate
/// code-only by construction.
///
/// Errors are the contract from the design's decision table: a service that
/// needs a migration but declares no edge, a group more than one hop behind
/// (chaining not shipped yet), or a version-changing upgrade whose ABIs are
/// unreadable.
async fn resolve_upgrade_from_abis(
    node_client: &calimero_node_primitives::client::NodeClient,
    current_app_key: [u8; 32],
    target_blob: [u8; 32],
) -> eyre::Result<Option<MigrationParams>> {
    use calimero_primitives::blobs::BlobId;
    use calimero_wasm_abi::embed::read_embedded_state_schema;

    use crate::migration_plan::{plan_upgrade, UpgradeAction};

    let target_blob_id = BlobId::from(target_blob);
    let services = node_client
        .bundle_service_names(&target_blob_id)
        .await?
        .ok_or_else(|| eyre::eyre!("target bytecode blob not available locally"))?;

    let current_blob_id = (current_app_key != [0u8; 32]).then(|| BlobId::from(current_app_key));

    // Service inventory of the CURRENT bundle, fetched once: a service present
    // in the target but absent here is genuinely NEW (no old data → code-only
    // for it). Detecting that by error-kind from the per-service bytes fetch
    // would conflate "not in bundle" with real I/O failures.
    let current_services: Option<Vec<Option<String>>> = match current_blob_id {
        Some(blob) => node_client.bundle_service_names(&blob).await.ok().flatten(),
        None => None,
    };

    /// Whether a target manifest declares anything migration-relevant: a
    /// state version past 1 or any migration edge. When it does, an
    /// unresolvable CURRENT side must reject the upgrade rather than guess —
    /// code-only-by-default here is exactly the silent new-code-on-old-state
    /// failure this resolution exists to prevent.
    fn target_declares_migration(m: &Manifest) -> bool {
        m.state_version_or_default() > 1 || !m.migrations.is_empty()
    }

    let mut resolved_method: Option<String> = None;
    for service in &services {
        let target_abi = match node_client
            .application_bytes_from_blob(&target_blob_id, service.as_deref())
            .await
        {
            Ok(Some(bytes)) => read_embedded_state_schema(&bytes),
            _ => None,
        };

        // Resolve the CURRENT side, distinguishing the safe unknowns from the
        // dangerous ones:
        //  * service absent from the old bundle → genuinely NEW service, no
        //    old data to migrate → code-only for it;
        //  * old blob missing locally / no embedded ABI / unseeded app_key →
        //    the from-version is UNKNOWABLE. Only safe when the target
        //    declares no migration at all; otherwise reject.
        // A service the old bundle never had cannot need a data migration.
        let new_service = current_services
            .as_ref()
            .is_some_and(|svcs| !svcs.contains(service));
        let current_abi = match current_blob_id {
            Some(_) if new_service => target_abi.clone(),
            Some(blob) => match node_client
                .application_bytes_from_blob(&blob, service.as_deref())
                .await
            {
                Ok(Some(bytes)) => match read_embedded_state_schema(&bytes) {
                    Some(m) => Some(m),
                    None => {
                        if target_abi.as_ref().is_some_and(target_declares_migration) {
                            eyre::bail!(
                                "service '{}': the currently-running bytecode has no embedded \
                                 ABI, but the target declares a state migration — cannot \
                                 determine the from-version safely. Rebuild the previous \
                                 version with the current SDK",
                                service.as_deref().unwrap_or("<single>"),
                            );
                        }
                        target_abi.clone()
                    }
                },
                Ok(None) => {
                    if target_abi.as_ref().is_some_and(target_declares_migration) {
                        eyre::bail!(
                            "service '{}': the currently-running bytecode blob is not \
                             available locally, but the target declares a state migration — \
                             cannot determine the from-version safely. Fetch the previous \
                             version first",
                            service.as_deref().unwrap_or("<single>"),
                        );
                    }
                    target_abi.clone()
                }
                // Not "service missing" (that's `new_service` above) — a
                // real read failure. Same unknowable-from rule as the other
                // arms: only safe when the target declares no migration.
                Err(err) => {
                    if target_abi.as_ref().is_some_and(target_declares_migration) {
                        eyre::bail!(
                            "service '{}': failed to read the currently-running bytecode \
                             ({err}), and the target declares a state migration — cannot \
                             determine the from-version safely",
                            service.as_deref().unwrap_or("<single>"),
                        );
                    }
                    target_abi.clone()
                }
            },
            // Group with no blob-derived app_key recorded: the from-version
            // is unknowable — same rule as above.
            None => {
                if target_abi.as_ref().is_some_and(target_declares_migration) {
                    eyre::bail!(
                        "service '{}': this group has no recorded bytecode blob, but the \
                         target declares a state migration — cannot determine the \
                         from-version safely",
                        service.as_deref().unwrap_or("<single>"),
                    );
                }
                target_abi.clone()
            }
        };

        match plan_upgrade(current_abi.as_ref(), target_abi.as_ref()) {
            Ok(UpgradeAction::CodeOnly) => {}
            Ok(UpgradeAction::Downgrade { from, to }) => {
                eyre::bail!(
                    "service '{}' declares state v{to}, OLDER than the running v{from} —                      schema downgrades are not supported",
                    service.as_deref().unwrap_or("<single>"),
                );
            }
            Ok(UpgradeAction::Migrate { method, from, to }) => {
                info!(
                    service = service.as_deref().unwrap_or("<single>"),
                    %method, from, to,
                    "upgrade resolves a declared migration edge from the app ABI"
                );
                match &resolved_method {
                    None => resolved_method = Some(method),
                    Some(existing) if *existing == method => {}
                    Some(existing) => {
                        // The wire carries ONE method; running it on every
                        // context (with missing-export = vacuous) is only
                        // sound when all migrating services agree on the
                        // name. Distinct names need per-service actuation —
                        // reject rather than silently dropping one.
                        eyre::bail!(
                            "services declare DIFFERENT migration methods for this release \
                             ('{existing}' vs '{method}'); migrating multiple services with \
                             distinct methods in one release is not supported yet — use the \
                             same method name in each service, or split the release",
                        );
                    }
                }
            }
            Ok(UpgradeAction::MissingEdge { from, to }) => {
                eyre::bail!(
                    "service '{}' declares state v{to} but no migration edge from v{from} — \
                     rebuild the app with a #[derive(app::Migrate)] edge for this hop",
                    service.as_deref().unwrap_or("<single>"),
                );
            }
            Ok(UpgradeAction::Behind { from, to }) => {
                eyre::bail!(
                    "service '{}' is {} versions behind the target (v{from} -> v{to}); \
                     chained upgrades are not supported yet — upgrade one version at a time",
                    service.as_deref().unwrap_or("<single>"),
                    to - from,
                );
            }
            // Only reachable when the TARGET build has no embedded ABI (the
            // current-side unknowns all reject above when the target declares
            // a migration). A target without an ABI declares nothing —
            // code-only is the only sound action for it.
            Err(err) => {
                warn!(
                    service = service.as_deref().unwrap_or("<single>"),
                    %err,
                    "target build has no embedded ABI; proceeding code-only"
                );
            }
        }
    }

    Ok(resolved_method.map(|method| MigrationParams { method }))
}

async fn resolve_embedded_schema(
    node_client: &calimero_node_primitives::client::NodeClient,
    application_id: &ApplicationId,
) -> Option<Manifest> {
    match node_client
        .get_application_bytes(application_id, None)
        .await
    {
        Ok(Some(bytes)) => calimero_wasm_abi::embed::read_embedded_state_schema(&bytes),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(
                %application_id, error = %err,
                "L1 gate: failed to fetch application bytes; treating as no embedded ABI (fail-open)"
            );
            None
        }
    }
}

struct UpgradePreamble {
    canary_context_id: ContextId,
    total_contexts: usize,
    upgrade_policy: UpgradePolicy,
    from_version: String,
    to_version: String,
    /// The group's CURRENT target application id (before this upgrade), used by
    /// the L1 identity-downgrade gate as the "old" schema source.
    current_application_id: ApplicationId,
    /// The group's CURRENT blob-derived app key (the bytecode the group runs
    /// before this upgrade). For same-id (bundle) upgrades the application
    /// row may already hold the NEW wasm, so the gate must read the "from"
    /// ABI from this blob rather than the row.
    current_app_key: [u8; 32],
}

fn validate_upgrade(
    datastore: &calimero_store::Store,
    group_id: &ContextGroupId,
    target_application_id: &ApplicationId,
    requester: &PublicKey,
    has_raw_signing_key: bool,
) -> eyre::Result<UpgradePreamble> {
    // 1. Group must exist
    let meta = MetaRepository::new(datastore)
        .load(group_id)?
        .ok_or_else(|| eyre::eyre!("group not found"))?;

    // 2. Requester must be admin
    MembershipRepository::new(datastore).require_admin(group_id, requester)?;

    // 3. Verify node holds the key (skip if raw key was provided)
    if !has_raw_signing_key {
        SigningKeysRepository::new(datastore).require_key(group_id, requester)?;
    }

    // 4. No active upgrade in progress
    if let Some(existing) = UpgradesRepository::new(datastore).load(group_id)? {
        if matches!(existing.status, GroupUpgradeStatus::InProgress { .. }) {
            bail!("an upgrade is already in progress for this group");
        }
    }

    // 5. Target must differ from current. Same id no longer implies no-op:
    //    bundle ids are version-stable (hash(package, signer)), so a new
    //    version moves only the bytecode blob — compare the group's recorded
    //    app_key against the target row's blob before rejecting. The row read
    //    races a concurrent in-place install (installs bypass this actor);
    //    the worst case either way is a rejected retry or a no-op upgrade op,
    //    never state corruption, so the window is accepted.
    if meta.target_application_id == *target_application_id {
        let target_blob = datastore
            .handle()
            .get(&key::ApplicationMeta::new(*target_application_id))?
            .map(|app| *app.bytecode.blob_id().as_ref());
        let bytecode_unchanged = target_blob.is_none_or(|blob| blob == meta.app_key);
        if bytecode_unchanged {
            bail!("group is already targeting this application");
        }
    }

    // 6. Group must have contexts
    let contexts =
        calimero_governance_store::enumerate_group_contexts(datastore, group_id, 0, usize::MAX)?;
    if contexts.is_empty() {
        bail!("group has no contexts to upgrade");
    }

    // 7. Select canary (first context, deterministic order)
    let canary_context_id = contexts[0];

    // 8. Read current and target application versions from ApplicationMeta.
    //    Use the group's current target_application_id as the "from" version — NOT the
    //    canary context's application. For LazyOnAccess, the canary may have already been
    //    lazily upgraded on its last execute, making its app_id == new target_application_id,
    //    which would produce from_version == to_version.
    let handle = datastore.handle();

    let from_version = handle
        .get(&key::ApplicationMeta::new(meta.target_application_id))?
        .map_or_else(|| "unknown".to_owned(), |app| String::from(app.version));

    let to_version = handle
        .get(&key::ApplicationMeta::new(*target_application_id))?
        .map_or_else(|| "unknown".to_owned(), |app| String::from(app.version));

    Ok(UpgradePreamble {
        canary_context_id,
        total_contexts: contexts.len(),
        upgrade_policy: meta.upgrade_policy.clone(),
        from_version,
        to_version,
        current_application_id: meta.target_application_id,
        current_app_key: meta.app_key,
    })
}

/// Maximum number of automatic retry rounds for failed context upgrades.
const MAX_AUTO_RETRIES: u32 = 3;

/// Base delay between retry rounds (doubles each round: 5s, 10s, 20s).
const RETRY_BASE_DELAY_SECS: u64 = 5;

pub(crate) async fn propagate_upgrade(
    context_client: calimero_context_client::client::ContextClient,
    datastore: calimero_store::Store,
    group_id: ContextGroupId,
    target_application_id: ApplicationId,
    migration: Option<MigrationParams>,
    skip_context: Option<ContextId>,
    initial_completed: u32,
) {
    let contexts = match calimero_governance_store::enumerate_group_contexts(
        &datastore,
        &group_id,
        0,
        usize::MAX,
    ) {
        Ok(c) => c,
        Err(err) => {
            error!(
                ?group_id,
                ?err,
                "failed to enumerate contexts for propagation"
            );
            return;
        }
    };

    // Use actual enumerated count as the authoritative total so that
    // contexts added/removed since the upgrade started are reflected.
    let total_contexts = contexts.len();

    // Build the list of contexts to upgrade (excluding the canary)
    let mut pending: Vec<ContextId> = contexts
        .into_iter()
        .filter(|cid| skip_context.map_or(true, |skip| *cid != skip))
        .collect();

    // If the canary was removed from the group between the initial upgrade
    // and this enumeration, it won't appear in the list and shouldn't count
    // toward completed — otherwise completed can exceed total.
    let canary_in_group = pending.len() < total_contexts;
    let mut completed: u32 = if canary_in_group {
        initial_completed
    } else {
        0
    };
    let mut failed: u32;
    let mut attempt: u32 = 0;

    loop {
        let mut next_pending = Vec::new();
        failed = 0;

        for context_id in &pending {
            // Skip contexts already running the target application to avoid
            // re-executing migrations on retry/recovery paths.
            match context_client.get_context(context_id) {
                Ok(Some(ctx))
                    if ctx.application_id == target_application_id && migration.is_none() =>
                {
                    completed += 1;
                    debug!(
                        ?group_id,
                        %context_id,
                        "context already on target application, skipping"
                    );
                    // Persist progress
                    let status = GroupUpgradeStatus::InProgress {
                        total: total_contexts as u32,
                        completed,
                        failed,
                    };
                    if let Err(err) = update_upgrade_status(&datastore, &group_id, status) {
                        error!(?group_id, ?err, "failed to persist upgrade progress");
                    }
                    continue;
                }
                _ => {}
            }

            let migrate_method = migration.as_ref().map(|m| m.method.clone());

            let signer = match calimero_governance_store::find_local_signing_identity(
                &datastore, context_id,
            ) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    warn!(
                        ?group_id,
                        %context_id,
                        "no local signing identity for context, skipping upgrade"
                    );
                    failed += 1;
                    next_pending.push(*context_id);
                    continue;
                }
                Err(err) => {
                    warn!(
                        ?group_id,
                        %context_id,
                        ?err,
                        "failed to look up local signing identity, skipping upgrade"
                    );
                    failed += 1;
                    next_pending.push(*context_id);
                    continue;
                }
            };

            match context_client
                .update_application(context_id, &target_application_id, &signer, migrate_method)
                .await
            {
                Ok(()) => {
                    completed += 1;
                    // The update_application handler records the per-context
                    // activation marker itself, so a subsequent LazyOnAccess
                    // read sees this context as already activated.
                    debug!(
                        ?group_id,
                        %context_id,
                        completed,
                        total = total_contexts,
                        attempt,
                        "context upgraded successfully"
                    );
                }
                Err(err) => {
                    failed += 1;
                    next_pending.push(*context_id);
                    warn!(
                        ?group_id,
                        %context_id,
                        ?err,
                        failed,
                        attempt,
                        "context upgrade failed"
                    );
                }
            }

            // Persist progress after each context
            let status = GroupUpgradeStatus::InProgress {
                total: total_contexts as u32,
                completed,
                failed,
            };

            if let Err(err) = update_upgrade_status(&datastore, &group_id, status) {
                error!(?group_id, ?err, "failed to persist upgrade progress");
            }
        }

        // All succeeded — no retry needed
        if next_pending.is_empty() {
            break;
        }

        attempt += 1;

        // Exhausted retry attempts
        if attempt > MAX_AUTO_RETRIES {
            warn!(
                ?group_id,
                failed = next_pending.len(),
                attempts = attempt,
                "exhausted auto-retry attempts, remaining failures left as InProgress"
            );
            break;
        }

        // Exponential backoff before retrying
        let delay_secs = RETRY_BASE_DELAY_SECS * (1 << (attempt - 1));
        info!(
            ?group_id,
            failed = next_pending.len(),
            attempt,
            delay_secs,
            "retrying failed context upgrades after delay"
        );
        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;

        // Reset failed count for next round and retry only the failures
        pending = next_pending;
    }

    // Final status
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let final_status = if failed == 0 {
        GroupUpgradeStatus::Completed {
            completed_at: Some(now),
        }
    } else {
        // Keep as InProgress with the final counts so manual retry can pick it up
        GroupUpgradeStatus::InProgress {
            total: total_contexts as u32,
            completed,
            failed,
        }
    };

    if let Err(err) = update_upgrade_status(&datastore, &group_id, final_status) {
        error!(?group_id, ?err, "failed to persist final upgrade status");
    }

    info!(
        ?group_id,
        completed,
        failed,
        total = total_contexts,
        attempts = attempt + 1,
        "group upgrade propagation finished"
    );
}

pub(crate) fn update_upgrade_status(
    datastore: &calimero_store::Store,
    group_id: &ContextGroupId,
    status: GroupUpgradeStatus,
) -> eyre::Result<()> {
    if let Some(mut upgrade) = UpgradesRepository::new(datastore).load(group_id)? {
        upgrade.status = status;
        UpgradesRepository::new(datastore).save(group_id, &upgrade)?;
    }
    Ok(())
}

/// Cascade variant of the upgrade-group flow.
///
/// Emits a single [`GroupOp::CascadeUpgrade`] signed by the requester,
/// then spawns one [`propagate_upgrade`] per descendant subgroup whose
/// current `app_key` matches the signed group's current `app_key`.
/// The atomic op carries `target_application_id`, `app_key`, `migration`,
/// and `cascade_hlc` in one unit, eliminating the out-of-order apply bug
/// of the legacy two-op path.
///
/// `cascade_hlc` IS stamped here at the initiator (once, deterministically)
/// via `calimero_storage::env::hlc_timestamp()`, and is recorded as the
/// fence boundary on every matched descendant's pre-spawn upgrade record.
/// Every peer that applies the gossiped op records the same fence value.
///
/// The walk used to enumerate matched descendants runs BEFORE the
/// cascade op is published locally — by the time the apply arm runs,
/// matched descendants' `GroupMeta.app_key` has been rewritten to the
/// new `app_key`, so a post-publish walk against the old predicate
/// would find zero matches. Capturing the descendant list synchronously
/// before publish is the simplest mechanism that respects the apply
/// arm's own mutation.
fn dispatch_cascade(
    actor: &mut ContextManager,
    group_id: ContextGroupId,
    target_application_id: ApplicationId,
    requester: PublicKey,
    signing_key: Option<[u8; 32]>,
    node_identity: Option<(PublicKey, [u8; 32])>,
) -> ActorResponse<ContextManager, eyre::Result<UpgradeGroupResponse>> {
    // --- Lightweight cascade validation ---
    // Cascade bypasses `validate_upgrade` because that helper requires
    // the signed group to have at least one context (for canary
    // selection). Namespace roots used as cascade entry-points often
    // hold no contexts of their own, only descendant subgroups. We
    // re-implement the subset of checks that do apply: group exists,
    // requester is admin, signing key is available, no concurrent
    // upgrade in progress on the signed group, and target differs.
    let meta = match MetaRepository::new(&actor.datastore).load(&group_id) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return ActorResponse::reply(Err(eyre::eyre!("group not found")));
        }
        Err(err) => return ActorResponse::reply(Err(err)),
    };

    if let Err(err) =
        MembershipRepository::new(&actor.datastore).require_admin(&group_id, &requester)
    {
        return ActorResponse::reply(Err(err));
    }

    if let Some(existing) = match UpgradesRepository::new(&actor.datastore).load(&group_id) {
        Ok(v) => v,
        Err(err) => return ActorResponse::reply(Err(err)),
    } {
        if matches!(existing.status, GroupUpgradeStatus::InProgress { .. }) {
            return ActorResponse::reply(Err(eyre::eyre!(
                "an upgrade is already in progress for this group"
            )));
        }
    }

    // Same-id is a no-op only when the bytecode also matches — bundle ids
    // are version-stable (hash(package, signer)), so a new version moves
    // only the blob. Mirrors `validate_upgrade` rule 5; this duplicate
    // originally kept the id-only comparison and rejected every same-id
    // bundle cascade as "already targeting".
    if meta.target_application_id == target_application_id {
        let target_blob = {
            let handle = actor.datastore.handle();
            match handle.get(&key::ApplicationMeta::new(target_application_id)) {
                Ok(row) => row.map(|app| *app.bytecode.blob_id().as_ref()),
                Err(err) => return ActorResponse::reply(Err(err.into())),
            }
        };
        if target_blob.is_none_or(|blob| blob == meta.app_key) {
            return ActorResponse::reply(Err(eyre::eyre!(
                "group is already targeting this application and no migration was requested"
            )));
        }
    }

    // Resolve target application meta (for the new app_key + blob announce).
    let app_meta = {
        let handle = actor.datastore.handle();
        let key = key::ApplicationMeta::new(target_application_id);
        match handle.get(&key) {
            Ok(Some(m)) => m,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!("target application not found")));
            }
            Err(err) => return ActorResponse::reply(Err(err.into())),
        }
    };
    let new_app_key = *app_meta.bytecode.blob_id().as_ref();
    let target_blob_info = (app_meta.bytecode.blob_id(), app_meta.size);
    let to_version: String = String::from(app_meta.version.clone());

    let from_app_key = meta.app_key;
    let from_version = {
        let handle = actor.datastore.handle();
        handle
            .get(&key::ApplicationMeta::new(meta.target_application_id))
            .ok()
            .flatten()
            .map_or_else(|| "unknown".to_owned(), |app| String::from(app.version))
    };

    // Auto-store signing key when requester == node identity, mirroring
    // the single-group path so subsequent cascade ops on the same group
    // don't need an explicit key.
    if let (Some(sk), Some((node_pk, _))) = (signing_key, node_identity) {
        if requester == node_pk {
            if let Err(err) =
                SigningKeysRepository::new(&actor.datastore).store_key(&group_id, &requester, &sk)
            {
                warn!(
                    target: "calimero::cascade",
                    ?err,
                    ?group_id,
                    "failed to auto-store signing key for cascade — next cascade on this group will require explicit key"
                );
            }
        }
    }

    // Resolve the signing key once (prefer caller-passed key, fall back
    // to the stored per-requester key) and validate the result with a
    // single `ok_or_else`. The prior split — `require_group_signing_key`
    // only when `signing_key.is_none()`, then `.or(...)` + later
    // `ok_or_else` inside `publish_task` — could fall through validation
    // when `signing_key` was `Some` but the stored key was absent,
    // surfacing as a less clear failure deep in publish.
    let effective_signing_key = match signing_key {
        Some(sk) => sk,
        None => match calimero_governance_store::SigningKeysRepository::new(&actor.datastore)
            .get_key(&group_id, &requester)
        {
            Ok(Some(sk)) => sk,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "local group upgrade requires a signing key for the requester"
                )));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        },
    };

    // --- Capture matched descendants BEFORE emitting the cascade op ---
    // After `sign_apply_and_publish` runs, the apply arm rewrites
    // `GroupMeta.app_key` on matched descendants to `new_app_key`, so a
    // post-publish walk against `from_app_key` would find zero matches.
    let matched_descendants = match calimero_governance_store::cascade::walk_for_predicate(
        &actor.datastore,
        group_id,
        from_app_key,
    ) {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| e.matched)
            .map(|e| e.group_id)
            .collect::<Vec<_>>(),
        Err(err) => return ActorResponse::reply(Err(err)),
    };

    if matched_descendants.is_empty() {
        return ActorResponse::reply(Err(eyre::eyre!(
            "cascade walk matched no descendants (signed group's app_key may have \
             already been migrated by a concurrent cascade)"
        )));
    }

    // Migration-policy gate for cascade. Each matched descendant runs the
    // migrate under its OWN policy on receivers (`maybe_lazy_upgrade` reads the
    // descendant's group meta, not the signed root's), so the gate is
    // per-descendant — not the signed group's policy. Collected here (sync);
    // the publish task resolves the migration from ABIs and checks these
    // policies before emitting `CascadeUpgrade`.
    let descendant_policies = {
        let mut descendant_policies = Vec::with_capacity(matched_descendants.len());
        for gid in &matched_descendants {
            match MetaRepository::new(&actor.datastore).load(gid) {
                Ok(Some(m)) => descendant_policies.push((*gid, m.upgrade_policy)),
                Ok(None) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "matched cascade descendant {gid:?} has no group meta"
                    )))
                }
                Err(err) => return ActorResponse::reply(Err(err)),
            }
        }
        descendant_policies
    };

    info!(
        ?group_id,
        %target_application_id,
        matched = matched_descendants.len(),
        "cascade upgrade: matched descendants enumerated"
    );

    // Snapshot per-descendant context totals BEFORE emission so the
    // pre-spawn `GroupUpgradeValue` write carries the right `total` for
    // status accounting. Descendant counts can shift between snapshot
    // and propagator-enumeration; the propagator re-enumerates and uses
    // its own count as authoritative (see `propagate_upgrade`), so any
    // brief mismatch here is harmless.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut pre_spawn_totals = Vec::with_capacity(matched_descendants.len());
    for gid in &matched_descendants {
        let total = match calimero_governance_store::MetadataRepository::new(&actor.datastore)
            .count_contexts(gid)
        {
            Ok(c) => c as u32,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        pre_spawn_totals.push(total);
    }

    let datastore = actor.datastore.clone();
    let node_client = actor.node_client.clone();
    let ack_router = Arc::clone(&actor.ack_router);
    let context_client = actor.context_client.clone();

    let datastore_for_publish = datastore.clone();
    let node_client_for_publish = node_client.clone();
    let ack_router_for_publish = Arc::clone(&ack_router);

    // The signed group's CURRENT target application id (before this cascade) is
    // the "old" schema source for the L1 identity-downgrade gate. The cascade
    // op rewrites every matched descendant from `from_app_key` to the new app,
    // so a single gate check on the signed group's app pair covers the family.
    let current_application_id = meta.target_application_id;
    let current_app_key_for_gate = meta.app_key;

    // Stamp the cascade_hlc ONCE at the initiator so every receiver
    // applies the same fence boundary (Task 3 apply handler stores this
    // value verbatim; Task 4 carries it on the wire via CascadeUpgrade).
    let cascade_hlc = calimero_storage::env::hlc_timestamp();

    let publish_task = async move {
        // Resolve the migration from the bundle's embedded ABIs (all
        // services) and re-run the per-descendant policy gate that the sync
        // section could not (resolution is async).
        let migration = {
            let resolved =
                resolve_upgrade_from_abis(&node_client_for_publish, from_app_key, new_app_key)
                    .await?;
            if resolved.is_some() {
                ensure_cascade_migration_policies_supported(&descendant_policies)?;
            }
            resolved
        };
        let migration_bytes_for_publish = migration.as_ref().map(|m| m.method.as_bytes().to_vec());
        let has_migration = migration.is_some();
        // L1 identity-downgrade gate: refuse a migration cascade that strips
        // identity from a top-level state field. Runs BEFORE the CascadeUpgrade
        // op is emitted. Fail-open when either app lacks an embedded ABI section.
        if has_migration {
            let old = resolve_pre_upgrade_schema(
                &node_client_for_publish,
                current_app_key_for_gate,
                &current_application_id,
            )
            .await;
            let new =
                resolve_embedded_schema(&node_client_for_publish, &target_application_id).await;
            verify_no_identity_downgrade(old.as_ref(), new.as_ref())?;
        }

        let sk = PrivateKey::from(effective_signing_key);

        let report = calimero_governance_store::sign_apply_and_publish(
            &datastore_for_publish,
            &node_client_for_publish,
            &ack_router_for_publish,
            &group_id,
            &sk,
            GroupOp::CascadeUpgrade {
                from_app_key,
                app_key: new_app_key,
                target_application_id,
                migration: migration_bytes_for_publish.clone(),
                cascade_hlc,
            },
        )
        .await?;
        report.observe("upgrade_group", "CascadeUpgrade");

        Ok::<_, eyre::Report>(migration)
    }
    .into_actor(actor);

    ActorResponse::r#async(publish_task.map(move |publish_result, act, ctx| {
        let migration = publish_result?;
        let migration_bytes = migration.as_ref().map(|m| m.method.as_bytes().to_vec());

        // After successful publish + local apply, spawn one propagator
        // per matched descendant. Each propagator re-enumerates its
        // group's contexts on entry and drives `update_application` per
        // context, exactly like the single-group path.
        for (gid, total) in matched_descendants.iter().zip(pre_spawn_totals.iter()) {
            // Carry forward the expand-entry governance position the
            // `CascadeUpgrade` apply just stamped onto this descendant's record
            // (`cascade_upgrade::apply`, run during the local apply above). This
            // per-descendant write replaces that `Completed` record with an
            // `InProgress` one to drive the propagator; without preserving
            // `cascade_seq` the migration-status rollup would lose its sequence
            // pin on the initiator.
            let cascade_seq = UpgradesRepository::new(&datastore)
                .load(gid)
                .ok()
                .flatten()
                .and_then(|v| v.cascade_seq);
            // Per-descendant `GroupUpgradeValue` so the propagator's
            // `update_upgrade_status` writes hit a live record. Same
            // shape the single-group canary path uses.
            let upgrade_value = GroupUpgradeValue {
                from_version: from_version.clone(),
                to_version: to_version.clone(),
                migration: migration_bytes.clone(),
                initiated_at: now,
                initiated_by: requester,
                status: GroupUpgradeStatus::InProgress {
                    total: *total,
                    completed: 0,
                    failed: 0,
                },
                cascade_hlc: Some(cascade_hlc),
                cascade_seq,
            };
            if let Err(err) = UpgradesRepository::new(&datastore).save(gid, &upgrade_value) {
                error!(
                    ?gid,
                    ?err,
                    "failed to save per-descendant upgrade record; skipping propagator spawn"
                );
                continue;
            }

            // Re-skip if a propagator is already running for this group
            // (e.g. from a prior in-flight upgrade that finished after
            // the validation check above). The active_propagators set
            // is the in-process race guard.
            if act.active_propagators.contains(gid) {
                warn!(
                    ?gid,
                    "propagator already running for cascade descendant; skipping spawn"
                );
                continue;
            }

            spawn_propagator_for(
                act,
                ctx,
                *gid,
                target_application_id,
                migration.clone(),
                context_client.clone(),
                datastore.clone(),
            );
        }

        // Best-effort blob announce so peers can fetch the target app
        // bytecode during their own context sync. Mirrors the gossip
        // step in the single-group canary path.
        let nc_for_announce = node_client.clone();
        let datastore_for_announce = datastore.clone();
        let descendants_for_announce = matched_descendants.clone();
        let (blob_id, blob_size) = target_blob_info;
        ctx.spawn(
            async move {
                for gid in &descendants_for_announce {
                    let contexts = match calimero_governance_store::enumerate_group_contexts(
                        &datastore_for_announce,
                        gid,
                        0,
                        usize::MAX,
                    ) {
                        Ok(c) => c,
                        Err(err) => {
                            warn!(
                                ?gid,
                                ?err,
                                "failed to enumerate descendant contexts for blob announce"
                            );
                            continue;
                        }
                    };
                    for context_id in &contexts {
                        if let Err(err) = nc_for_announce
                            .announce_blob_to_network(&blob_id, context_id, blob_size)
                            .await
                        {
                            warn!(%err, "failed to announce target app blob");
                        }
                    }
                }
            }
            .into_actor(act),
        );

        // Initial status snapshot for the signed group itself (which is
        // also in `matched_descendants` since the walk always includes
        // the root and it always matches `from_app_key`). The
        // per-descendant propagators write their own statuses; we
        // surface the signed group's initial status here for the RPC
        // response.
        let signed_status = match UpgradesRepository::new(&datastore).load(&group_id) {
            Ok(Some(v)) => v.status,
            _ => GroupUpgradeStatus::InProgress {
                total: pre_spawn_totals.first().copied().unwrap_or(0),
                completed: 0,
                failed: 0,
            },
        };

        Ok(UpgradeGroupResponse {
            group_id,
            status: signed_status.into(),
        })
    }))
}

/// Insert into `active_propagators`, spawn `propagate_upgrade` for the
/// given group, and arrange the post-completion removal — used by the
/// cascade dispatch loop. Mirrors the inline pattern in the single-
/// group canary handler at L398-411 of `handle`, factored out so the
/// cascade loop and any future caller can share one spawn shape.
fn spawn_propagator_for(
    actor: &mut ContextManager,
    ctx: &mut <ContextManager as actix::Actor>::Context,
    group_id: ContextGroupId,
    target_application_id: ApplicationId,
    migration: Option<MigrationParams>,
    context_client: calimero_context_client::client::ContextClient,
    datastore: calimero_store::Store,
) {
    actor.active_propagators.insert(group_id);
    let propagator = propagate_upgrade(
        context_client,
        datastore,
        group_id,
        target_application_id,
        migration,
        None, // cascade has no per-descendant canary to skip
        0,    // initial_completed: 0 — no contexts pre-migrated
    );
    ctx.spawn(propagator.into_actor(actor).map(move |_, act, _| {
        act.active_propagators.remove(&group_id);
    }));
}

/// Reject a migration-carrying upgrade under any policy other than
/// [`UpgradePolicy::LazyOnAccess`].
///
/// Only `LazyOnAccess` triggers the receiver-side migrate: a receiver runs
/// the migration via `maybe_lazy_upgrade`, which early-returns for any
/// non-`LazyOnAccess` policy (`execute/mod.rs`). Under `Automatic` a receiver
/// swaps its application pointer to the new bytecode but never runs the
/// migrate, so v2 wasm reads v1 state bytes and panics with a silent borsh
/// "Not all bytes read". Catch that combination loudly here, before any group
/// op is emitted, rather than letting it corrupt state on every receiver.
///
/// Code-only upgrades (`has_migration == false`) stay allowed under every
/// policy.
/// Cascade migration-policy gate.
///
/// Call this only when the cascade carries a migration — the caller gates on
/// that before loading each descendant's meta, so no work is done for code-only
/// cascades (hence, unlike the single-group variant, this takes no
/// `has_migration` flag and assumes a migration is present).
///
/// A cascade fans out to every matched descendant, and on receivers each
/// descendant runs the migrate under its OWN policy — `maybe_lazy_upgrade` reads
/// the *descendant's* group meta, not the signed root's. So the gate is
/// per-descendant: reject if any matched descendant is not `LazyOnAccess`. The
/// signed (root) group's own policy is irrelevant here (it is often a
/// context-less namespace root carrying the default `Automatic`, which would
/// otherwise both false-reject all-Lazy cascades and false-pass a non-Lazy
/// descendant straight into silent corruption).
fn ensure_cascade_migration_policies_supported(
    descendants: &[(ContextGroupId, UpgradePolicy)],
) -> eyre::Result<()> {
    for (group_id, policy) in descendants {
        if !matches!(policy, UpgradePolicy::LazyOnAccess) {
            bail!(
                "cascade migration is only supported when every matched descendant uses the \
                 LazyOnAccess upgrade policy; descendant group {group_id:?} uses {policy:?}, \
                 which would swap the application without running the migration on its receivers \
                 (silent state corruption). Set that subgroup's upgrade policy to LazyOnAccess \
                 before migrating."
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_cascade_migration_policies_supported;
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::UpgradePolicy;

    fn gid(b: u8) -> ContextGroupId {
        ContextGroupId::from([b; 32])
    }

    fn dg_manifest(fields: &str) -> calimero_wasm_abi::schema::Manifest {
        serde_json::from_str(&format!(
            r#"{{"schema_version":"wasm-abi/1","types":{{"Root":{{"kind":"record","fields":[{fields}]}}}},"methods":[],"events":[],"state_root":"Root"}}"#
        )).unwrap()
    }
    const DG_AUTH: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"authored_map"}}"#;
    const DG_PLAIN: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"unordered_map"}}"#;

    #[test]
    fn gate_refuses_identity_downgrade() {
        let err = super::verify_no_identity_downgrade(
            Some(&dg_manifest(DG_AUTH)),
            Some(&dg_manifest(DG_PLAIN)),
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(s.contains("identity downgrade forbidden"), "{s}");
        assert!(s.contains("wiki"), "{s}");
    }
    #[test]
    fn gate_allows_carry_through() {
        assert!(super::verify_no_identity_downgrade(
            Some(&dg_manifest(DG_AUTH)),
            Some(&dg_manifest(DG_AUTH))
        )
        .is_ok());
    }
    #[test]
    fn gate_fails_open_when_schema_absent() {
        assert!(super::verify_no_identity_downgrade(None, Some(&dg_manifest(DG_PLAIN))).is_ok());
        assert!(super::verify_no_identity_downgrade(Some(&dg_manifest(DG_AUTH)), None).is_ok());
    }

    #[test]
    fn cascade_migration_all_lazy_descendants_is_allowed() {
        let descendants = [
            (gid(1), UpgradePolicy::LazyOnAccess),
            (gid(2), UpgradePolicy::LazyOnAccess),
        ];
        ensure_cascade_migration_policies_supported(&descendants)
            .expect("cascade migration with all-LazyOnAccess descendants must be allowed");
    }

    #[test]
    fn cascade_migration_rejected_when_any_descendant_not_lazy() {
        // A single non-Lazy descendant (root policy is irrelevant) must reject.
        let descendants = [
            (gid(1), UpgradePolicy::LazyOnAccess),
            (gid(2), UpgradePolicy::Automatic),
        ];
        let err = ensure_cascade_migration_policies_supported(&descendants)
            .expect_err("a non-LazyOnAccess matched descendant must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("LazyOnAccess"),
            "error should name the required policy, got: {msg}"
        );
    }

    #[test]
    fn cascade_migration_empty_descendants_is_allowed() {
        ensure_cascade_migration_policies_supported(&[])
            .expect("an empty descendant set is vacuously allowed");
    }
}
