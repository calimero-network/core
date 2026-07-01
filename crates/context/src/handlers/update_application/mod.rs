use std::cell::RefCell;
use std::rc::Rc;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use borsh::BorshDeserialize;
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::MigrationFailureKind;
use calimero_context_client::messages::{MigrationParams, UpdateApplicationRequest};
use calimero_node_primitives::client::NodeClient;
use calimero_prelude::ROOT_STORAGE_ENTRY_ID;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    AppVersionChangedPayload, ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent,
    StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::Event as RuntimeEvent;
use calimero_storage::address::Id;
use calimero_storage::delta::clear_pending_delta;
use calimero_storage::entities::Metadata;
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::error::StorageError;
use calimero_storage::index::{EntityIndex, Index};
use calimero_storage::store::{Key, MainStorage};
use calimero_storage::Interface;
use calimero_store::{key, types};
use calimero_utils_actix::global_runtime;
use either::Either;
use eyre::bail;
use tracing::{debug, error, info, warn};

use crate::handlers::execute::storage::{ContextStorage, ReadOnlyContextStorage};
use crate::ContextManager;

impl Handler<UpdateApplicationRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpdateApplicationRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateApplicationRequest {
            context_id,
            application_id,
            public_key,
            migration,
        }: UpdateApplicationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        info!(
            %context_id,
            %application_id,
            has_migration = migration.is_some(),
            "Handling UpdateApplicationRequest"
        );

        // Authorize the *requester* before touching any state. `update_application`
        // swaps the context's bytecode and can drive a whole-context state
        // migration, so it must be gated to this context's own provisioned
        // identities — the same bar `execute` enforces. Previously the caller
        // `public_key` was threaded straight through to the migration/finalize
        // path and only the new application's signer *continuity* was checked,
        // never the requester, so any key could drive a migration.
        if let Err(err) = authorize_update_application(&self.datastore, &context_id, &public_key) {
            warn!(
                %context_id,
                %application_id,
                %public_key,
                %err,
                "Rejecting unauthorized update_application"
            );
            return ActorResponse::reply(Err(err));
        }

        let context_meta = self.contexts.get(&context_id).map(|c| c.meta.clone());

        // Skip update only when the application ID is unchanged AND no migration is requested.
        // When migration IS requested, the WASM binary may have been replaced under the same
        // application ID (same signing key), so we must proceed to run the migration function.
        if migration.is_none() {
            if let Some(ref context) = context_meta {
                if application_id == context.application_id {
                    // #2060: a same ApplicationId does NOT imply same bytecode for a
                    // signed bundle (its id is version-stable). Only skip when the
                    // context already activated the installed blob; otherwise fall
                    // through to the code-only swap so the new bytecode is loaded and
                    // the activation marker recorded — a bare id compare would silently
                    // drop a code-only upgrade and report success.
                    let installed = self
                        .node_client
                        .get_application(&application_id)
                        .ok()
                        .flatten()
                        .map(|app| *app.blob.bytecode.as_ref());
                    let activated = crate::activation::activated_blob(&self.datastore, &context_id);
                    if same_id_update_is_noop(activated, installed) {
                        debug!(%context_id, "Application already set, installed bytecode already active, and no migration requested; skipping update");
                        return ActorResponse::reply(Ok(()));
                    }
                    debug!(
                        %context_id, %application_id,
                        "same application id but the installed bytecode is not yet active \
                         (code-only bundle upgrade); applying update"
                    );
                }
            }
        }

        let application = self.applications.get(&application_id).cloned();

        // If migration is requested, we need to load the module first
        if let Some(ref migration_params) = migration {
            debug!(
                %context_id,
                %application_id,
                method = %migration_params.method,
                "Migration requested, loading new module"
            );

            // Invalidate the cached application entry BEFORE loading the module.
            // The WASM binary may have been replaced under the same application ID
            // (same signing key, new version), so we must force a fresh fetch from
            // the node's blob storage to get the updated bytecode. The module and
            // read-only caches are blob-keyed (content-addressed) — never stale.
            if self.applications.remove(&application_id).is_some() {
                debug!(
                    %context_id,
                    %application_id,
                    "Invalidated stale cached application before migration module load"
                );
            }

            // Clone values needed for migration
            let datastore = self.datastore.clone();
            let node_client = self.node_client.clone();
            let context_client = self.context_client.clone();
            let migration_params = migration_params.clone();

            let service_name = context_meta.as_ref().and_then(|c| c.service_name.clone());

            // Load the (fresh) module
            let module_task = self.get_module(application_id, service_name);

            let task = module_task.and_then(move |module, act, _ctx| {
                let datastore = datastore.clone();
                let node_client = node_client.clone();
                let context_client = context_client.clone();
                let context_meta = act.contexts.get(&context_id).map(|c| c.meta.clone());
                let application = act.applications.get(&application_id).cloned();
                let migration_v2 = act.config.migration_v2;
                // Hold the per-context write guard across migrate -> check ->
                // commit, mirroring the lazy/execute path. Without it an
                // app-method execute could interleave with this admin-driven
                // migration's read-old -> compute -> commit sequence. The
                // module load above touches no state, so acquiring the guard
                // here (after the load) still serialises the whole
                // state-mutating sequence against a concurrent op.
                let guard_either = act.contexts.get(&context_id).map(|c| c.lock());

                async move {
                    let _guard = match guard_either {
                        Some(Either::Left(guard)) => Some(guard),
                        Some(Either::Right(fut)) => Some(fut.await),
                        None => None,
                    };
                    let result = update_application_with_migration(
                        datastore,
                        node_client,
                        context_client,
                        context_id,
                        context_meta,
                        application_id,
                        application,
                        public_key,
                        Some(migration_params),
                        module,
                        migration_v2,
                    )
                    .await;
                    drop(_guard);
                    result
                }
                .into_actor(act)
            });

            return ActorResponse::r#async(task.map_ok(
                move |(_application, updated_context), act, _ctx| {
                    // Invalidate the cached application row so the next resolution
                    // re-reads it. Blob-keyed module entries are content-addressed
                    // and stay valid.
                    if act.applications.remove(&application_id).is_some() {
                        debug!(%context_id, %application_id, "Invalidated cached application after migration");
                    }

                    if let Some(cached) = act.contexts.get_mut(&context_id) {
                        debug!(
                            %context_id,
                            old_root = ?cached.meta.root_hash,
                            new_root = ?updated_context.root_hash,
                            "Updating cached context after migration"
                        );
                        // service_name is preserved: we only refresh application_id (and state fields).
                        cached.meta.application_id = application_id;
                        cached.meta.root_hash = updated_context.root_hash;
                        cached.meta.dag_heads = updated_context.dag_heads;
                    }
                },
            ));
        }

        // No migration - use existing update path
        let task = update_application_id(
            self.datastore.clone(),
            self.node_client.clone(),
            self.context_client.clone(),
            context_id,
            context_meta,
            application_id,
            application,
            public_key,
        );

        ActorResponse::r#async(task.into_actor(self).map_ok(move |application, act, _ctx| {
            // Insert-if-absent, honouring MAX_CACHED_APPLICATIONS (evicts only
            // when actually adding a new entry); an already-cached entry is
            // left untouched.
            let _ignored = act
                .applications
                .get_or_insert_with(application_id, || application);

            if let Some(context) = act.contexts.get_mut(&context_id) {
                // service_name is preserved: we only update application_id (not service_name).
                context.meta.application_id = application_id;
            }
        }))
    }
}

/// Resolves context and application from optional values or fetches them if missing.
///
/// Returns the resolved context and application, or an error if they don't exist.
fn resolve_context_and_application(
    context_client: &ContextClient,
    node_client: &NodeClient,
    context_id: ContextId,
    context: Option<Context>,
    application_id: ApplicationId,
    application: Option<Application>,
) -> eyre::Result<(Context, Application)> {
    let context = match context {
        Some(context) => context,
        None => {
            let Some(context) = context_client.get_context(&context_id)? else {
                bail!("context '{}' does not exist", context_id);
            };

            context
        }
    };

    let application = match application {
        Some(application) => application,
        None => {
            let Some(application) = node_client.get_application(&application_id)? else {
                bail!("application with id '{}' not found", application_id);
            };

            application
        }
    };

    Ok((context, application))
}

/// Finalizes the application update by updating external config, datastore, and syncing.
///
/// This function performs the common post-update steps:
/// 1. Updates the external config client
/// 2. Writes context metadata to datastore
/// 3. Triggers node sync
async fn finalize_application_update(
    datastore: &calimero_store::Store,
    node_client: &NodeClient,
    context_client: &ContextClient,
    context: &mut Context,
    application: &Application,
) -> eyre::Result<()> {
    let context_id = context.id;

    if context_client.context_config(&context_id)?.is_none() {
        bail!(
            "missing context config parameters for context '{}'",
            context_id
        );
    }

    let mut handle = datastore.handle();

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(application.id),
            *context.root_hash,
            context.dag_heads.clone(),
            context.service_name.as_deref().map(Box::from),
        ),
    )?;

    node_client.sync(Some(&context_id), None).await?;

    Ok(())
}

/// Resolves an application's semver from its `ApplicationMeta` row. `None` when
/// the row is absent (e.g. uninstalled). Labels the `AppVersionChanged` event.
fn application_version(
    datastore: &calimero_store::Store,
    application_id: ApplicationId,
) -> Option<String> {
    match datastore
        .handle()
        .get(&key::ApplicationMeta::new(application_id))
    {
        Ok(meta) => meta.map(|m| m.version.to_string()),
        Err(err) => {
            // Best-effort label: a read fault yields None (same as an absent
            // row), but log it so a persistent store fault isn't fully silent.
            debug!(%err, %application_id, "failed to read ApplicationMeta for version label");
            None
        }
    }
}

/// #2060: a signed bundle's `ApplicationId` is version-stable
/// (`hash(package, signer)`), so a same-id no-migration update can be either a
/// true no-op or a code-only bytecode swap. The early skip is safe ONLY when the
/// context already executes the exact bytecode now installed under that id — its
/// activation marker (`activated`) equals the installed blob. A missing marker
/// or any mismatch means the new bytecode is not yet active, so the update must
/// proceed; an unreadable application row (`installed` is `None`) also proceeds
/// rather than silently skipping.
fn same_id_update_is_noop(activated: Option<[u8; 32]>, installed: Option<[u8; 32]>) -> bool {
    matches!((activated, installed), (Some(a), Some(b)) if a == b)
}

/// Builds an `AppVersionChanged` node event for a context whose application id
/// flipped, or `None` when it is unchanged. The id comparison IS the emit-once
/// dedup (6f.5): a no-op re-apply with the same id emits nothing.
fn app_version_changed_event(
    context_id: ContextId,
    old_application_id: ApplicationId,
    new_application_id: ApplicationId,
    from_version: Option<String>,
    to_version: Option<String>,
) -> Option<NodeEvent> {
    (old_application_id != new_application_id).then_some({
        NodeEvent::Context(ContextEvent {
            context_id,
            payload: ContextEventPayload::AppVersionChanged(AppVersionChangedPayload {
                from_version,
                to_version,
            }),
        })
    })
}

/// Authorize the caller of an `UpdateApplicationRequest`.
///
/// `update_application` runs new bytecode and can migrate the whole context's
/// state, so — like `execute` — it is restricted to a *provisioned local
/// identity* of this context: a stored `ContextIdentity` that carries a private
/// key on this node. A key that is unknown here, or a known-but-remote identity
/// with no private key, is refused before any state is read or written.
///
/// This gates only the external `Handler<UpdateApplicationRequest>` entry. The
/// in-WASM callers of `update_application_id` (the execute path) are already
/// authorized by `execute`'s own identity check and are intentionally not
/// re-gated here.
fn authorize_update_application(
    datastore: &calimero_store::Store,
    context_id: &ContextId,
    caller: &PublicKey,
) -> eyre::Result<()> {
    // This authz check opens its own datastore handle, separate from the handle
    // the downstream business logic uses. That leaves a tiny check-then-use window
    // in which the identity could be deprovisioned between here and the actual
    // update. This is a benign TOCTOU: identity deprovisioning is rare, and the
    // operation re-reads context/identity state downstream, so a race can at worst
    // let one already-in-flight update proceed on a just-revoked identity.
    let handle = datastore.handle();
    let key = calimero_store::key::ContextIdentity::new(*context_id, *caller);
    match handle.get(&key)? {
        Some(identity) if identity.private_key.is_some() => Ok(()),
        // Keep the caller-facing error generic so it can't be used as a
        // membership-enumeration oracle: the reply is returned to the (possibly
        // unauthorized) caller, and interpolating the caller key / context id
        // would confirm whether a given key is a known member. Operators still get
        // the full diagnostic from the `warn!` log at the call site.
        _ => bail!("unauthorized: caller is not a permitted identity for this context"),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "orthogonal args (runtime deps, context identity, crypto keys, module) on a split-brain-critical handler; no cohesive grouping"
)]
pub async fn update_application_id(
    datastore: calimero_store::Store,
    node_client: NodeClient,
    context_client: ContextClient,
    context_id: ContextId,
    context: Option<Context>,
    application_id: ApplicationId,
    application: Option<Application>,
    _public_key: PublicKey,
) -> eyre::Result<Application> {
    let (mut context, application) = resolve_context_and_application(
        &context_client,
        &node_client,
        context_id,
        context,
        application_id,
        application,
    )?;

    // Verify AppKey continuity (signerId match)
    verify_appkey_continuity(&datastore, &context, &application_id)?;

    // Capture the pre-flip app id before finalize persists the new one, so the
    // post-commit AppVersionChanged carries the correct from/to versions.
    let old_application_id = context.application_id;

    finalize_application_update(
        &datastore,
        &node_client,
        &context_client,
        &mut context,
        &application,
    )
    .await?;

    // Post-commit: notify subscribers the application version flipped (skew #2).
    if let Some(event) = app_version_changed_event(
        context_id,
        old_application_id,
        application.id,
        application_version(&datastore, old_application_id),
        application_version(&datastore, application.id),
    ) {
        let _ = node_client.send_event(event);
    }

    // Unified activation marker: code-only updates (this fn — the eager
    // propagator's no-migration route and the lazy code-only finish) count
    // as activations too, or the same-id up-to-date rule would keep reading
    // these contexts as pending. Only recorded when the blob is locally
    // present: a marker hard-binds execution to that bytecode, so naming a
    // missing blob would wedge the context AND stop the lazy retry (the
    // gate reads no-marker as "activation pending").
    let activated = activated_row_blob(&node_client, &application);
    if node_client
        .has_blob(&calimero_primitives::blobs::BlobId::from(activated))
        .unwrap_or(false)
    {
        crate::activation::record_activation(&datastore, &context_id, activated);
    }

    Ok(application)
}

/// The bytecode blob this update activated, read FRESH from the application
/// row. The `Application` passed through these handlers can be a cache
/// snapshot taken before a same-id in-place install moved the row — recording
/// its blob would mark the context as having activated the OLD bytecode.
fn activated_row_blob(node_client: &NodeClient, application: &Application) -> [u8; 32] {
    node_client
        .get_application(&application.id)
        .ok()
        .flatten()
        .map_or(*application.blob.bytecode.as_ref(), |fresh| {
            *fresh.blob.bytecode.as_ref()
        })
}

/// Verifies AppKey continuity by checking that the signerId matches between
/// the currently installed application and the new application.
///
///  An update MUST be accepted only if:
/// - candidate.signerId == installed.signerId, OR
/// - candidate.signerId is permitted by key lineage (future extension), OR
/// - context governance explicitly authorizes a signer switch
///
/// This check MUST occur BEFORE any migration logic executes.
fn verify_appkey_continuity(
    datastore: &calimero_store::Store,
    context: &Context,
    new_application_id: &ApplicationId,
) -> eyre::Result<()> {
    let handle = datastore.handle();

    // Get current application's metadata
    let old_app_key = key::ApplicationMeta::new(context.application_id);
    let Some(old_app_meta) = handle.get(&old_app_key)? else {
        // If no old application exists (new context), allow the update
        debug!(
            context_id = %context.id,
            "No existing application found, skipping AppKey continuity check"
        );
        return Ok(());
    };

    // Get new application's metadata
    let new_app_key = key::ApplicationMeta::new(*new_application_id);
    let Some(new_app_meta) = handle.get(&new_app_key)? else {
        bail!(
            "new application with id '{}' not found in database",
            new_application_id
        );
    };

    // Check signerId continuity
    // Note: Empty signer_id is used as a sentinel for legacy non-bundle applications.
    // We allow updates from/to legacy applications with empty signer_id for backwards compatibility.
    let old_signer_id = old_app_meta.signer_id.as_ref();
    let new_signer_id = new_app_meta.signer_id.as_ref();

    // If both have non-empty signer_ids, they must match
    if !old_signer_id.is_empty() && !new_signer_id.is_empty() && old_signer_id != new_signer_id {
        error!(
            context_id = %context.id,
            old_signer_id = %old_signer_id,
            new_signer_id = %new_signer_id,
            "AppKey continuity violation: signerId mismatch"
        );
        bail!(
            "AppKey continuity violation: signerId mismatch. \
             Cannot update from signerId '{}' to '{}'. \
             The same signing key must be used for application updates.",
            old_signer_id,
            new_signer_id
        );
    }

    // Security: Disallow signed-to-unsigned downgrades
    // Allow unsigned-to-signed upgrades
    if !old_signer_id.is_empty() && new_signer_id.is_empty() {
        error!(
            context_id = %context.id,
            old_signer_id = %old_signer_id,
            "Security downgrade rejected: Cannot update from signed application to unsigned (legacy) application"
        );
        bail!(
            "Security downgrade rejected: Cannot update from signed application (signerId: '{}') \
             to unsigned (legacy) application. \
             Signed-to-unsigned downgrades are disallowed to prevent security vulnerabilities. \
             If you need to use a legacy unsigned application, you must create a new context.",
            old_signer_id
        );
    }

    // Warn if upgrading from unsigned to signed (allowed, but log for audit)
    if old_signer_id.is_empty() && !new_signer_id.is_empty() {
        info!(
            context_id = %context.id,
            new_signer_id = %new_signer_id,
            "Upgrading from unsigned (legacy) to signed application - security improvement"
        );
    }

    debug!(
        context_id = %context.id,
        signer_id = %if old_signer_id.is_empty() { "<unsigned>" } else { old_signer_id },
        "AppKey continuity check passed"
    );

    Ok(())
}

/// Update application with migration execution.
///
/// This function implements the full migration flow:
/// 1. Validates AppKey continuity (signerId match)
/// 2. Loads the NEW application WASM module
/// 3. Executes the migration function
/// 4. Writes returned state bytes to root storage key
/// 5. Updates context metadata and triggers sync
#[allow(
    clippy::too_many_arguments,
    reason = "orthogonal args (runtime deps, context identity, crypto keys, module) on a split-brain-critical handler; no cohesive grouping"
)]
pub(crate) async fn update_application_with_migration(
    datastore: calimero_store::Store,
    node_client: NodeClient,
    context_client: ContextClient,
    context_id: ContextId,
    context: Option<Context>,
    application_id: ApplicationId,
    application: Option<Application>,
    public_key: PublicKey,
    migration: Option<MigrationParams>,
    module: calimero_runtime::Module,
    migration_v2: bool,
) -> eyre::Result<(Application, Context)> {
    let (mut context, application) = resolve_context_and_application(
        &context_client,
        &node_client,
        context_id,
        context,
        application_id,
        application,
    )?;

    // Verify AppKey continuity (signerId match)
    verify_appkey_continuity(&datastore, &context, &application_id)?;

    // Pre-flip app id, captured before finalize persists the new one (for the
    // post-commit AppVersionChanged from/to versions).
    let old_application_id = context.application_id;

    // Set to the v2 module once a migration commits, so we can run
    // `count_my_pending` over the committed v2 state after finalize (6f.8).
    let mut pending_count_module: Option<calimero_runtime::Module> = None;

    // Execute migration if requested
    if let Some(migration_params) = migration {
        'migration: {
            info!(
                %context_id,
                %application_id,
                method = %migration_params.method,
                "Executing migration"
            );

            // Clone the (Arc-backed, cheap) module before `execute_migration` moves
            // its copy into `spawn_blocking`, so the migration_check below can run a
            // second `module.run` on the same already-compiled v2 artifact.
            let check_module = module.clone();
            // A third cheap clone for the post-commit count_my_pending run (6f.8).
            let count_module = check_module.clone();

            // Execute migration function via module.run(). Returns the UNCOMMITTED
            // staging buffer (`storage`) the migrate wrote into, plus the optional
            // transient witness, so the check below sees the produced v2 state and
            // the gate can commit-or-drop the buffer (zero-residue abort).
            let (new_state_bytes, migration_witness, migration_events, migration_logs, storage) =
                match execute_migration(
                    &datastore,
                    node_client.clone(),
                    &context,
                    module,
                    &migration_params,
                    public_key,
                )
                .await
                {
                    Ok(out) => out,
                    Err(err) if err.downcast_ref::<MigrateExportMissing>().is_some() => {
                        // This context's service has no such export — multi-service
                        // bundles record ONE migrate method group-wide, but only the
                        // schema-changing service defines it. Vacuously applied:
                        // proceed as a code-only bytecode swap (and drop any failed
                        // marker a pre-fix attempt persisted).
                        info!(
                            %context_id,
                            method = %migration_params.method,
                            "service does not export the migrate method; applying upgrade code-only"
                        );
                        clear_migration_failed(&datastore, context_id);
                        break 'migration;
                    }
                    Err(err) => {
                        // The migrate apply itself errored (e.g. the v2 wasm trapped).
                        // Record the reason so the heartbeat surfaces this member as
                        // `failed`, then propagate — the context stays on v1.
                        persist_migration_failed(
                            &datastore,
                            context_id,
                            MigrationFailureKind::ApplyFailed,
                        );
                        return Err(err);
                    }
                };

            // Log migration logs
            for log_line in &migration_logs {
                info!(%context_id, migration_log = %log_line, "Migration log");
            }

            // Pre-commit migration_check (migration_v2). Run the app's
            // `__calimero_migration_check` export over the produced v2 bytes while
            // the v1 root is still readable, before `write_migration_state` (the
            // only mutation of the v1 root) and `finalize_application_update`. A
            // failing check makes `commit_or_abort_migration` return early, leaving
            // the committed context on v1 (no byte restore — v1 was never mutated).
            // An app with no check export ⇒ pass ⇒ commits (backwards-compatible).
            // Thread the staging buffer through the check (so `new` reads the
            // produced v2 collections) and back out for the commit/drop decision.
            let (check_passed, storage) = if migration_v2 {
                let (passed, storage) = run_migration_check(
                    node_client.clone(),
                    &context,
                    check_module,
                    &new_state_bytes,
                    migration_witness.as_deref(),
                    public_key,
                    storage,
                )
                .await?;

                if passed {
                    info!(%context_id, "migration_check passed");
                } else {
                    warn!(%context_id, "migration_check failed: logical abort");
                }
                (passed, storage)
            } else {
                // Flag off ⇒ no check ⇒ commit (backwards-compatible).
                (true, storage)
            };

            // Funnel the verdict through the single gate-decision seam. The decision
            // source is pluggable (today: the migration_check verdict; a future
            // canary-subgroup soak would plug in here) without touching
            // `commit_or_abort_migration`.
            let decision = MigrationGateDecision::from_check_result(check_passed);

            // Promote the staging buffer + write the v2 root, or DROP the buffer
            // (zero-residue logical abort). On a failed check this returns
            // `Err(MigrationCheckFailed)` before any live mutation, propagating out
            // and skipping `finalize_application_update`.
            let new_root_hash = commit_or_abort_migration(
                &datastore,
                &mut context,
                &new_state_bytes,
                public_key,
                decision,
                storage,
            )?;

            // Emit migration events to WebSocket clients
            if !migration_events.is_empty() {
                let events_vec: Vec<ExecutionEvent> = migration_events
                    .into_iter()
                    .map(|e| ExecutionEvent {
                        kind: e.kind,
                        data: e.data,
                        handler: e.handler,
                    })
                    .collect();
                let _ = node_client.send_event(NodeEvent::Context(ContextEvent {
                    context_id,
                    payload: ContextEventPayload::StateMutation(
                        StateMutationPayload::with_root_and_events(new_root_hash, events_vec),
                    ),
                }));
            }

            info!(
                %context_id,
                new_root_hash = %new_root_hash,
                state_size = new_state_bytes.len(),
                "Migration completed successfully"
            );

            // The v2 state is committed; schedule the post-finalize authored_remaining
            // recompute (6f.8). Reaching here means the check passed and committed —
            // a failed check returns Err above and never gets here.
            pending_count_module = Some(count_module);
        }
    }

    finalize_application_update(
        &datastore,
        &node_client,
        &context_client,
        &mut context,
        &application,
    )
    .await?;

    // Post-commit: notify subscribers the application version flipped (skew #2).
    if let Some(event) = app_version_changed_event(
        context_id,
        old_application_id,
        application.id,
        application_version(&datastore, old_application_id),
        application_version(&datastore, application.id),
    ) {
        let _ = node_client.send_event(event);
    }

    // Unified activation marker: this context now executes the new app's
    // bytecode (whether a migration committed above or this was a code-only
    // update). The single up-to-date signal for the gate/trigger/rollup.
    crate::activation::record_activation(
        &datastore,
        &context_id,
        activated_row_blob(&node_client, &application),
    );

    // Post-commit: recompute this node's owner's pending-authored count over the
    // committed v2 state and persist it for the heartbeat self-report (6f.8).
    // Best-effort — a missing export / failure leaves the prior value untouched.
    if let Some(module) = pending_count_module {
        if let Some(count) = run_count_my_pending(
            &datastore,
            node_client.clone(),
            context_id,
            module,
            public_key,
        )
        .await
        {
            persist_authored_remaining(&datastore, context_id, count);
        }
    }

    Ok((application, context))
}

/// The decision driving the post-`execute_migration` commit-vs-abort seam.
///
/// Every path that has produced a candidate v2 root funnels its verdict through
/// this enum before reaching [`commit_or_abort_migration`], so the source of the
/// decision is pluggable while the commit/abort mechanics stay fixed. Today the
/// only source is the migration_check verdict (pass ⇒ `Commit`, fail ⇒ `Abort`);
/// a future canary-subgroup soak gate (deferred) would plug in as another source
/// feeding this same enum, without reworking `commit_or_abort_migration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationGateDecision {
    /// Persist the produced v2 root via `write_migration_state` and advance the
    /// context to it.
    Commit,
    /// Logically abort: discard the produced v2 root and leave the still-v1 root
    /// intact (no byte restore — v1 was never mutated).
    Abort,
}

impl MigrationGateDecision {
    /// Map the migration_check verdict onto a gate decision: `true ⇒ Commit`,
    /// `false ⇒ Abort`. This is the single point a future canary-subgroup gate
    /// would replace (or sit beside).
    pub(crate) const fn from_check_result(check_passed: bool) -> Self {
        if check_passed {
            Self::Commit
        } else {
            Self::Abort
        }
    }
}

/// Commit the produced v2 migration root, or perform a logical abort. This is
/// the single seam every post-`execute_migration` commit/abort decision funnels
/// through.
///
/// - [`MigrationGateDecision::Abort`] ⇒ DROP the uncommitted staging buffer and
///   return `Err(MigrationCheckFailed)` without touching the v1 root. The
///   migrate's child-entry writes were buffered in that dropped buffer and never
///   reached the live store, so the v1 root AND its child buckets are intact —
///   a true zero-residue rollback (no byte snapshot/restore needed). The caller
///   propagates this before `finalize_application_update`, so the committed
///   context (root_hash, dag_heads, application_id) stays on v1.
///
/// - [`MigrationGateDecision::Commit`] ⇒ promote the staging buffer (flush the
///   migrate's child-entry writes to the live store), write `new_state_bytes`
///   through `write_migration_state` (the root writer), advance
///   `context.root_hash`/`context.dag_heads` to the new v2 root, and return the
///   new Merkle root hash so the caller can emit events against it.
fn commit_or_abort_migration(
    datastore: &calimero_store::Store,
    context: &mut Context,
    new_state_bytes: &[u8],
    executor_identity: PublicKey,
    decision: MigrationGateDecision,
    storage: ContextStorage,
) -> eyre::Result<Hash> {
    let context_id = context.id;

    if decision == MigrationGateDecision::Abort {
        // Logical abort: DROP the uncommitted staging buffer. Its buffered
        // child-entry writes never reached the live store, so the v1 root AND
        // its child buckets are intact — a true zero-residue rollback (no byte
        // restore needed). The root is also untouched (`write_migration_state`
        // never runs). The caller's `?` propagates this out, skipping
        // `finalize_application_update`.
        //
        // KNOWN GAP (deferred hardening, tracked as "SortedIndex transactional
        // staging"): if the aborted migrate wrote a `SortedMap`/`SortedSet`, its
        // ordered-index rows went straight to the non-synced `Column::SortedIndex`
        // (not this buffer) and survive the drop. The index validity marker is
        // state-store-backed, so it rolls back in lockstep with the entities and
        // (mis)reads as current against the restored v1 state — meaning the
        // ordinary `ensure_index` self-heal does NOT reconcile the residue. The
        // authoritative entity set is still correct (v1); only the ordered-read
        // index can be stale until it is next forced to rebuild. A full fix
        // (index writes inside the migration transaction, or an abort hook that
        // invalidates the touched collections' markers) is deferred.
        drop(storage);
        clear_pending_delta();
        // Record the abort so the heartbeat surfaces this member as `failed`
        // (not a silent in-progress). Cleared if a later migrate commits.
        // The activation marker did not move, so per-context module binding
        // keeps executing the pre-upgrade bytecode — no pin needed.
        persist_migration_failed(datastore, context_id, MigrationFailureKind::CheckAborted);
        return Err(MigrationCheckFailed { context_id }.into());
    }

    // Commit decision: promote the staging buffer — flush the migrate's buffered
    // child-entry writes to the live store — then write the v2 root. The buffer
    // and the root write target the same Arc-backed datastore.
    let _committed_store = storage
        .commit()
        .map_err(|e| eyre::eyre!("Failed to commit migration storage writes: {e}"))?;

    // Write returned state bytes to root storage key
    // This uses the storage layer to properly update both Entry and Index
    let full_hash = write_migration_state(datastore, context, new_state_bytes, executor_identity)?;

    // Update root_hash after migration: full_hash is already the Merkle tree hash from
    // the storage layer; wrap the bytes directly (same as create_context/execute).
    let new_root_hash = Hash::from(full_hash);
    context.root_hash = new_root_hash;

    // Align DAG heads with the new state. Migration does not create a causal delta,
    // so use root_hash as dag_head fallback (same as execute flow when init() creates
    // state without actions). This keeps sync protocol consistent and avoids divergence.
    context.dag_heads = vec![*context.root_hash.as_bytes()];
    debug!(
        %context_id,
        new_root_hash = %new_root_hash,
        "Updated dag_heads to new root after migration"
    );

    // The migrate committed: drop any prior failure marker so a context that
    // recovered (this attempt, or after an earlier abort) stops reporting failed.
    clear_migration_failed(datastore, context_id);

    Ok(new_root_hash)
}

/// Execute the migration function in the new WASM module.
///
/// The migration function reads old state via `read_raw()` and returns new state bytes.
/// Also returns events and logs produced by the migration so the caller can emit them and log them.
async fn execute_migration(
    datastore: &calimero_store::Store,
    node_client: NodeClient,
    context: &Context,
    module: calimero_runtime::Module,
    migration_params: &MigrationParams,
    executor_identity: PublicKey,
) -> eyre::Result<(
    Vec<u8>,
    Option<Vec<u8>>,
    Vec<RuntimeEvent>,
    Vec<String>,
    ContextStorage,
)> {
    let context_id = context.id;
    let method = migration_params.method.clone();

    debug!(
        %context_id,
        method = %method,
        "Preparing to execute migration function"
    );

    let storage = ContextStorage::from(datastore.clone(), context_id);

    // Run the migrate body against `storage` (a `Temporal` buffer). Every
    // `env::storage_write` the migrate makes — `UnorderedMap::insert`,
    // `Vector::push`, sub-entity saves — is buffered in the shadow and is NOT
    // flushed to the live store here. The buffer is returned UNCOMMITTED so the
    // caller can run `migration_check` against it (the check reads the produced
    // v2 collections through the shadow) and then either commit it on a passing
    // check or DROP it on a failing one — a true zero-residue rollback.
    //
    // (Previously this committed immediately, before the check ran, which is
    // why a rejected lossy migrate left v2-shaped child entries as residue under
    // the still-v1 root: the destructive child mutations had already landed.)
    let (outcome, storage) = global_runtime()
        .spawn_blocking(move || {
            let mut storage = storage;
            let outcome = module.run(
                context_id,
                executor_identity,
                &method,
                &[],
                &mut storage,
                None,
                Some(node_client),
            );
            // Migration is delta-less (no causal delta is published). Scrub any
            // pending sync delta the migrate writes pushed into the thread-local
            // DELTA_CONTEXT so it cannot leak into later ops on this pool thread.
            clear_pending_delta();
            (outcome, storage)
        })
        .await
        .map_err(|e| eyre::eyre!("Migration task failed: {}", e))?;
    let outcome = outcome?;

    // Extract the return value from the outcome.
    // `outcome.returns` is `Result<Option<Vec<u8>>, FunctionCallError>` where the
    // Ok/Err discrimination is already handled by the `value_return` host function.
    // The inner `Vec<u8>` is the raw borsh-serialized new state bytes — NOT a
    // borsh-serialized `Result<Vec<u8>, Vec<u8>>`.
    let returns = match outcome.returns {
        // The service module has no such export. Multi-service bundles record
        // ONE migrate method group-wide, but only the schema-changing service
        // defines it — surface a typed error so the caller can treat the
        // migration as vacuous for this context instead of failing it.
        Err(calimero_runtime::errors::FunctionCallError::MethodResolutionError(
            calimero_runtime::errors::MethodResolutionError::MethodNotFound { .. },
        )) => {
            return Err(MigrateExportMissing {
                context_id,
                method: migration_params.method.clone(),
            }
            .into());
        }
        r => r.map_err(|e| eyre::eyre!("Migration execution failed: {:?}", e))?,
    };

    let Some(new_state_bytes) = returns else {
        bail!("Migration function did not return any data. Ensure the migration function returns the new state.");
    };

    debug!(
        %context_id,
        bytes_len = new_state_bytes.len(),
        events_count = outcome.events.len(),
        logs_count = outcome.logs.len(),
        has_witness = outcome.migration_witness.is_some(),
        "Migration function returned new state"
    );

    Ok((
        new_state_bytes,
        outcome.migration_witness,
        outcome.events,
        outcome.logs,
        storage,
    ))
}

/// Error surfaced when a pre-commit `migration_check` rejects (or could not
/// run on) the produced v2 root.
///
/// Surfacing this error from `update_application_with_migration` is the logical
/// abort: the early return happens before `write_migration_state` (the only
/// mutation of the v1 root) and `finalize_application_update`, so the committed
/// context (root_hash, dag_heads, application_id) stays on v1. No byte snapshot
/// or restore — the v1 root is intact because it was never mutated (clean
/// rollback).
#[derive(Debug, thiserror::Error)]
#[error(
    "migration_check failed for context '{context_id}': logical abort — the produced v2 root was \
     discarded and the still-v1 root left intact (no byte restore; v1 was never mutated)"
)]
pub(crate) struct MigrationCheckFailed {
    pub(crate) context_id: ContextId,
}

/// The context's service module does not export the requested migrate method.
/// Multi-service bundles record ONE migrate method group-wide, but only the
/// schema-changing service defines it — for every other service the migration
/// is vacuous and the upgrade proceeds as a code-only bytecode swap.
#[derive(Debug, thiserror::Error)]
#[error("service module does not export migrate method '{method}' for context '{context_id}'")]
pub(crate) struct MigrateExportMissing {
    pub(crate) context_id: ContextId,
    pub(crate) method: String,
}

/// Decode the verdict of a `__calimero_migration_check` invocation from its
/// `outcome.returns`.
///
/// Decision matrix:
/// - **Missing export** (`MethodNotFound`) ⇒ `Ok(true)`: an app that defines no
///   check is never blocked. This keeps the flow backwards-compatible.
/// - **Any other execution error** (a WASM trap, host error, …) ⇒ `Ok(false)`:
///   fail-closed. A check that could not complete must not be treated as a pass.
/// - **`Ok(Some(bytes))`** ⇒ `borsh::from_slice::<bool>(&bytes)`. The verdict
///   bytes are RAW `borsh(bool)` — the macro hands the runtime `borsh(bool)` via
///   `value_return`'s `Ok` branch, mirroring `#[app::migrate]`'s
///   raw-new-state-bytes contract (see `migration.rs`). It is NOT a borsh
///   `Result<bool, Vec<u8>>` envelope, so it must NOT be decoded as one.
/// - **`Ok(None)`** ⇒ a defined check that returned nothing is a contract
///   violation; fail-closed (`Ok(false)`).
///
/// Returns `Err` only when a present check returned bytes that are not a valid
/// borsh `bool` (a genuine, unexpected ABI breakage worth surfacing).
fn decode_migration_check_verdict(
    returns: calimero_runtime::logic::VMLogicResult<
        Option<Vec<u8>>,
        calimero_runtime::errors::FunctionCallError,
    >,
) -> eyre::Result<bool> {
    use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};

    match returns {
        // No `__calimero_migration_check` export ⇒ no check defined ⇒ never
        // block (backwards compatible).
        Err(FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
            ..
        })) => Ok(true),
        // The export exists but trapped / errored. Fail closed: a check that
        // could not run must not pass.
        Err(e) => {
            warn!(error = ?e, "migration_check trapped or errored; failing closed (logical abort)");
            Ok(false)
        }
        Ok(Some(bytes)) => borsh::from_slice::<bool>(&bytes).map_err(|e| {
            eyre::eyre!(
                "migration_check returned undecodable verdict bytes (expected borsh bool): {e}"
            )
        }),
        // A present check must return a verdict; nothing returned is a contract
        // violation. Fail closed.
        Ok(None) => {
            warn!("migration_check returned no verdict; failing closed (logical abort)");
            Ok(false)
        }
    }
}

/// Run the app's `__calimero_migration_check` export over the produced v2 root,
/// returning its verdict (or `Ok(true)` when no check is defined).
///
/// The check is a read-only predicate: it reads the still-v1 old root via
/// `read_raw()` and receives the produced `new_state_bytes` (the same bytes
/// `write_migration_state` would persist) as its `env::input()`. It is run on a
/// throwaway [`ContextStorage`] view that is **never committed**, so any writes
/// it might make are discarded; the lingering pending delta is cleared so it
/// cannot contaminate later operations on the same runtime thread (mirroring
/// `write_migration_state`).
///
/// `module` is a cheap clone (the compiled artifact is `Arc`-backed, see
/// `calimero_runtime::Module`), so this second `module.run` shares the
/// already-compiled module with `execute_migration` rather than re-loading it.
async fn run_migration_check(
    node_client: NodeClient,
    context: &Context,
    module: calimero_runtime::Module,
    new_state_bytes: &[u8],
    witness: Option<&[u8]>,
    executor_identity: PublicKey,
    storage: ContextStorage,
) -> eyre::Result<(bool, ContextStorage)> {
    let context_id = context.id;
    // The check runs against the SAME uncommitted staging buffer the migrate
    // wrote into: `new` (and its collections) reads the produced v2 state
    // through the shadow, while `old` (via `read_raw`) still sees the pristine
    // v1 root (the root is only written on commit). The produced v2 bytes + the
    // optional transient witness are delivered packed as
    // `borsh((new_state_bytes, Option<witness>))` — see the migration_check macro.
    let input = borsh::to_vec(&(new_state_bytes.to_vec(), witness.map(<[u8]>::to_vec)))?;

    let (outcome, storage) = global_runtime()
        .spawn_blocking(move || {
            let mut storage = storage;
            // Run the check through a READ-ONLY view of the staging buffer: the
            // predicate reads the produced v2 state (reads delegate to the
            // buffer's shadow-over-live) while its writes are suppressed, so a
            // misbehaving check cannot contaminate the state we may later commit
            // on a passing verdict (the buffer is reused for the commit).
            let outcome = {
                let mut ro = ReadOnlyContextStorage::new(&mut storage);
                module.run(
                    context_id,
                    executor_identity,
                    "__calimero_migration_check",
                    &input,
                    &mut ro,
                    None,
                    Some(node_client),
                )
            };
            // The check is read-only; scrub any sync delta it pushed into the
            // thread-local DELTA_CONTEXT so it cannot leak into later ops on this
            // pool thread. The buffer is returned UNCOMMITTED — its writes reach
            // the store only if the caller later commits it (on a passing check).
            clear_pending_delta();
            (outcome, storage)
        })
        .await
        .map_err(|e| eyre::eyre!("migration_check task failed: {}", e))?;

    // A host-level runtime error (instantiation / link failure, resource limit)
    // is distinct from `outcome.returns` and must also fail closed: a check that
    // could not even start running is not a pass.
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            warn!(%context_id, error = ?e, "migration_check host runtime error; failing closed (logical abort)");
            return Ok((false, storage));
        }
    };

    Ok((decode_migration_check_verdict(outcome.returns)?, storage))
}

/// Run the app's `count_my_pending` export over the COMMITTED v2 state to read
/// the executor's pending-authored count (the self-reported `authored_remaining`).
/// Runs post-commit against a fresh buffer over the live store, under the
/// applying identity, so `owned_by_me` resolves to this node's owner. Read-only
/// (the export never commits). Best-effort: a missing export (non-authored app),
/// a host error, or a pool-join failure all yield `None`.
async fn run_count_my_pending(
    datastore: &calimero_store::Store,
    node_client: NodeClient,
    context_id: ContextId,
    module: calimero_runtime::Module,
    executor_identity: PublicKey,
) -> Option<u32> {
    let storage = ContextStorage::from(datastore.clone(), context_id);
    let outcome = global_runtime()
        .spawn_blocking(move || {
            let mut storage = storage;
            let outcome = module.run(
                context_id,
                executor_identity,
                "count_my_pending",
                &[],
                &mut storage,
                None,
                Some(node_client),
            );
            // Read-only call: scrub any thread-local delta it might have pushed.
            clear_pending_delta();
            outcome
        })
        .await;

    match outcome {
        // The export returns its u32 count as JSON via value_return. A missing
        // export (non-authored app ⇒ Err(MethodNotFound)), an empty return, or a
        // host error all mean "no count" — report None and leave the prior value.
        Ok(Ok(rt_outcome)) => match rt_outcome.returns {
            Ok(Some(bytes)) => serde_json::from_slice::<u32>(&bytes).ok(),
            _ => None,
        },
        Ok(Err(e)) => {
            debug!(%context_id, error = ?e, "count_my_pending host error; reporting no count");
            None
        }
        Err(e) => {
            debug!(%context_id, error = ?e, "count_my_pending task join failed; reporting no count");
            None
        }
    }
}

/// Persist this node's owner's pending-authored count to the dedicated
/// node-local `ContextAuthoredRemaining` key (read by the migration heartbeat).
/// A single-value put on its own key — NOT folded into `ContextMeta`, so the
/// hot per-write `ContextMeta` rewrite path cannot clobber it and there is no
/// read-modify-write race. Best-effort: a store fault is logged and skipped.
pub(crate) fn persist_authored_remaining(
    datastore: &calimero_store::Store,
    context_id: ContextId,
    authored_remaining: u32,
) {
    let mut handle = datastore.handle();
    let key = key::ContextAuthoredRemaining::new(context_id);
    let value = types::ContextAuthoredRemaining {
        count: authored_remaining,
    };
    if let Err(err) = handle.put(&key, &value) {
        debug!(%context_id, %err, "failed to persist authored_remaining");
    }
}

/// Persist a node-local marker that this context's last migration attempt did
/// not complete, with a categorized reason. Read by the migration heartbeat so
/// the member surfaces as `failed` rather than a silent `in_progress`; cleared
/// by [`clear_migration_failed`] once a later migrate commits. Best-effort.
pub(crate) fn persist_migration_failed(
    datastore: &calimero_store::Store,
    context_id: ContextId,
    kind: MigrationFailureKind,
) {
    let mut handle = datastore.handle();
    let key = key::ContextMigrationFailed::new(context_id);
    let value = types::ContextMigrationFailed { kind: kind.to_u8() };
    if let Err(err) = handle.put(&key, &value) {
        debug!(%context_id, %err, "failed to persist migration_failed marker");
    }
}

/// Drop any persisted migration-failure marker for `context_id` — called when a
/// migration commits so a recovered context stops reporting `failed`. Deleting
/// an absent marker (the common case) is a store no-op, not an error.
pub(crate) fn clear_migration_failed(datastore: &calimero_store::Store, context_id: ContextId) {
    let mut handle = datastore.handle();
    let key = key::ContextMigrationFailed::new(context_id);
    if let Err(err) = handle.delete(&key) {
        debug!(%context_id, %err, "failed to clear migration_failed marker");
    }
}

/// Reference-counted host storage callbacks (read / write / remove).
pub(crate) type ReadFn = Rc<dyn Fn(&Key) -> Option<Vec<u8>>>;
pub(crate) type WriteFn = Rc<dyn Fn(Key, &[u8]) -> bool>;
pub(crate) type RemoveFn = Rc<dyn Fn(&Key) -> bool>;

/// Storage callback closures used by the `calimero-storage` runtime environment.
///
/// These closures bridge the `calimero-storage` [`Key`]-based interface to the
/// underlying `calimero-store` [`key::ContextState`]-based KV store.
pub(crate) struct StorageCallbacks {
    pub(crate) read: ReadFn,
    pub(crate) write: WriteFn,
    pub(crate) remove: RemoveFn,
}

/// Create storage callback closures that route `calimero-storage` operations to the datastore.
///
/// Each callback translates a storage-layer [`Key`] into a context-scoped
/// [`key::ContextState`] and forwards the operation to the store handle.
pub(crate) fn create_storage_callbacks(
    datastore: &calimero_store::Store,
    context_id: ContextId,
) -> StorageCallbacks {
    let read: ReadFn = {
        let handle = datastore.handle();
        let ctx_id = context_id;
        Rc::new(move |key: &Key| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            match handle.get(&state_key) {
                Ok(Some(state)) => Some(state.value.into_boxed().into_vec()),
                Ok(None) => None,
                Err(e) => {
                    error!(
                        %ctx_id,
                        storage_key = ?storage_key,
                        error = ?e,
                        "Storage read failed during migration state write"
                    );
                    None
                }
            }
        })
    };

    let write: WriteFn = {
        let handle_cell: Rc<RefCell<_>> = Rc::new(RefCell::new(datastore.handle()));
        let ctx_id = context_id;
        Rc::new(move |key: Key, value: &[u8]| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            let slice: calimero_store::slice::Slice<'_> = value.to_vec().into();
            let state_value = types::ContextState::from(slice);
            handle_cell
                .borrow_mut()
                .put(&state_key, &state_value)
                .is_ok()
        })
    };

    let remove: RemoveFn = {
        let handle_cell: Rc<RefCell<_>> = Rc::new(RefCell::new(datastore.handle()));
        let ctx_id = context_id;
        Rc::new(move |key: &Key| {
            let storage_key = key.to_bytes();
            let state_key = key::ContextState::new(ctx_id, storage_key);
            handle_cell.borrow_mut().delete(&state_key).is_ok()
        })
    };

    StorageCallbacks {
        read,
        write,
        remove,
    }
}

/// Compute a deterministic [`Metadata`] timestamp from the existing root index.
///
/// Reads the current root entry's [`EntityIndex`] from the store and picks a
/// timestamp strictly greater than any existing `created_at`/`updated_at` value.
/// If the index cannot be read or deserialized, a large deterministic fallback
/// (`u64::MAX / 2`) is used so every node converges on the same value.
fn compute_deterministic_metadata(
    datastore: &calimero_store::Store,
    context_id: ContextId,
) -> Metadata {
    let root_entry_id = Id::new(ROOT_STORAGE_ENTRY_ID);
    let index_key = Key::Index(root_entry_id);
    let storage_key = index_key.to_bytes();
    let state_key = key::ContextState::new(context_id, storage_key);

    let timestamp = match datastore.handle().get(&state_key) {
        Ok(Some(state_data)) => {
            match EntityIndex::try_from_slice(&state_data.value.into_boxed().into_vec()) {
                Ok(existing_index) => {
                    // Use max(existing_updated_at + 1, existing_created_at + 1) to ensure
                    // the new timestamp is strictly greater than any existing timestamp
                    let existing_updated = existing_index.metadata.updated_at();
                    let existing_created = existing_index.metadata.created_at();
                    existing_updated.max(existing_created).saturating_add(1)
                }
                Err(e) => {
                    error!(
                        %context_id,
                        error = ?e,
                        "Failed to deserialize existing index for deterministic timestamp, using fallback"
                    );
                    u64::MAX / 2
                }
            }
        }
        Ok(None) => {
            // No existing metadata — use a large deterministic value so the
            // migration timestamp is newer than any possible prior state.
            u64::MAX / 2
        }
        Err(e) => {
            error!(
                %context_id,
                error = ?e,
                "Failed to read existing index for deterministic timestamp, using fallback"
            );
            u64::MAX / 2
        }
    };

    Metadata::new(timestamp, timestamp)
}

/// Build the full entry byte vector expected by the storage layer.
///
/// The storage layer persists root state entries as `Entry<T> = borsh(T) ++ borsh(Element.id)`.
/// The migration function returns only `borsh(T)` (user data), so we re-append the 32-byte
/// [`ROOT_STORAGE_ENTRY_ID`] suffix so the data round-trips through the normal fetch path
/// (`Root::fetch` → `Collection::get` → `find_by_id::<Entry<T>>`).
fn build_entry_bytes(new_state_bytes: &[u8]) -> Vec<u8> {
    let mut entry_bytes = Vec::with_capacity(new_state_bytes.len() + ROOT_STORAGE_ENTRY_ID.len());
    entry_bytes.extend_from_slice(new_state_bytes);
    entry_bytes.extend_from_slice(&ROOT_STORAGE_ENTRY_ID);
    entry_bytes
}

/// Write migrated state bytes to the root storage key, properly updating both Entry and Index.
///
/// This function uses the `calimero-storage` layer to ensure the Merkle tree Index is
/// updated along with the Entry data. This maintains consistency for the sync protocol.
///
/// Returns the Merkle tree root's `full_hash` (from `Id::root()`), matching the hash
/// computation used by the normal execution flow in `system.rs`.
fn write_migration_state(
    datastore: &calimero_store::Store,
    context: &Context,
    new_state_bytes: &[u8],
    executor_identity: PublicKey,
) -> eyre::Result<[u8; 32]> {
    let context_id = context.id;

    debug!(
        %context_id,
        state_size = new_state_bytes.len(),
        "Writing migrated state via storage layer"
    );

    let callbacks = create_storage_callbacks(datastore, context_id);
    let metadata = compute_deterministic_metadata(datastore, context_id);
    let entry_bytes = build_entry_bytes(new_state_bytes);

    let context_id_bytes: [u8; 32] = *context_id.as_ref();
    let executor_id_bytes: [u8; 32] = *executor_identity.as_ref();
    let runtime_env = RuntimeEnv::new(
        callbacks.read,
        callbacks.write,
        callbacks.remove,
        context_id_bytes,
        executor_id_bytes,
    );

    let root_entry_id = Id::new(ROOT_STORAGE_ENTRY_ID);
    let result = with_runtime_env(runtime_env, || -> Result<_, StorageError> {
        let write_result = (|| -> Result<Option<[u8; 32]>, StorageError> {
            // Pre-flight LWW check. `write_pre_merged_root_state` has a
            // built-in LWW guard: if the locally-stored root entry already
            // has `updated_at > metadata.updated_at`, it silently returns
            // the existing hash and does NOT apply our migrated bytes.
            //
            // For migration that is unsafe — the surrounding
            // `update_application_id` flow still persists the new
            // `application_id` regardless of what happened here, so a
            // silent skip would leave the v2 binary running against
            // un-migrated v1-shaped state (data corruption class). The
            // pre-#2433 `save_raw` path surfaced this case via an
            // `Ok(None)` return; we restore the same safety net here by
            // checking the metadata ourselves and signalling skip as
            // `Ok(None)`, which the outer match arm bails on.
            //
            // First-time migrations (no existing entry yet) skip the
            // guard and proceed normally.
            if let Some(existing_index) = <Index<MainStorage>>::get_index(root_entry_id)? {
                if existing_index.metadata.updated_at > metadata.updated_at {
                    return Ok(None);
                }
            }

            // Capture the intended updated_at before `metadata` is moved
            // into write_pre_merged_root_state below — the post-write
            // verification compares against this.
            let intended_updated_at = metadata.updated_at;

            // Write the migrated root state via the pre-merged primitive
            // introduced by #2465. The caller (this function) is the
            // source of truth for the new bytes — the wasm migrate
            // function already produced fully-resolved v2-shaped state,
            // so there is no host-side merge to dispatch and no app-type
            // entry in the host's merge registry (which lives in the
            // wasm runtime since #2465's host/WASM split). Using
            // `save_raw` here hit `MergeFailure(NoMergeFunctionRegistered)`
            // because save_raw expects a registered Mergeable for
            // root-class entries — that's the #2433 regression this
            // function existed to trigger.
            let _entry_own_hash = Interface::<MainStorage>::write_pre_merged_root_state(
                root_entry_id,
                &entry_bytes,
                metadata,
            )?;

            // Post-write verification closes the TOCTOU window where a
            // racing writer slipped a newer `updated_at` between our
            // pre-flight check above and this call: the primitive's
            // own internal LWW guard would then have silently returned
            // the existing hash without applying our bytes. If that
            // happened the stored `updated_at` will differ from what
            // we passed in. Treat that as a skip and bail.
            if let Some(after_index) = <Index<MainStorage>>::get_index(root_entry_id)? {
                if after_index.metadata.updated_at != intended_updated_at {
                    return Ok(None);
                }
            }

            // Read the Merkle tree root hash — write_pre_merged_root_state
            // returns the *entry node's* full_hash, but the migration
            // caller (system.rs's normal execution flow analogue) needs
            // the tree root hash for ContextMeta.root_hash.
            let root_hash = Index::<MainStorage>::get_hashes_for(Id::root())?
                .map(|(full_hash, _)| full_hash)
                .unwrap_or([0; 32]);

            Ok(Some(root_hash))
        })();

        // The storage-write path pushes sync actions into thread-local
        // DELTA_CONTEXT. This migration path does not emit a delta artifact,
        // so explicitly discard pending actions to avoid contaminating
        // subsequent operations on the same runtime thread.
        clear_pending_delta();

        write_result
    });

    match result {
        Ok(Some(root_hash)) => {
            debug!(
                %context_id,
                root_hash = ?root_hash,
                "Migrated state written successfully with Index update"
            );
            Ok(root_hash)
        }
        Ok(None) => {
            error!(
                %context_id,
                "Migration state write skipped by LWW guard — local root metadata \
                 is newer than the migration's deterministic timestamp"
            );
            bail!(
                "Migration state write was skipped by the LWW guard: local root \
                 metadata has a newer `updated_at` than the migration's. Persisting \
                 the new application_id without writing the migrated bytes would \
                 leave the v2 binary running against v1-shaped state. The migration \
                 must be retried."
            )
        }
        Err(e) => {
            error!(
                %context_id,
                error = ?e,
                "Failed to write migrated state"
            );
            bail!("Failed to write migrated state: {:?}", e)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{Context, ContextId};
    use calimero_primitives::hash::Hash;
    use calimero_store::db::InMemoryDB;
    use calimero_store::{key, types, Store};

    use calimero_primitives::events::{ContextEvent, ContextEventPayload, NodeEvent};

    use calimero_primitives::identity::PublicKey;

    use super::ContextStorage;
    use super::{
        app_version_changed_event, application_version, authorize_update_application,
        same_id_update_is_noop, verify_appkey_continuity,
    };

    /// Write a `ContextIdentity` row exactly as identity provisioning would, so
    /// the authz check sees a member of `context_id`. `private_key` present marks
    /// it a *local* identity (`Some`) vs a known-but-remote one (`None`).
    fn seed_identity(
        store: &Store,
        context_id: ContextId,
        public_key: PublicKey,
        private_key: Option<[u8; 32]>,
    ) {
        let mut handle = store.handle();
        handle
            .put(
                &key::ContextIdentity::new(context_id, public_key),
                &types::ContextIdentity {
                    private_key,
                    sender_key: None,
                },
            )
            .expect("seed identity");
    }

    /// A caller unknown to this context cannot drive an application update /
    /// migration — the request is refused before any state is touched (E3).
    #[test]
    fn authorize_update_application_rejects_unknown_caller() {
        let store = create_test_store();
        let context_id = ContextId::from([1u8; 32]);
        let stranger = PublicKey::from([9u8; 32]);

        assert!(
            authorize_update_application(&store, &context_id, &stranger).is_err(),
            "an identity that is not a member of the context must be refused"
        );
    }

    /// A provisioned local identity (a `ContextIdentity` carrying a private key
    /// on this node) is authorized — the same bar `execute` enforces.
    #[test]
    fn authorize_update_application_accepts_local_member() {
        let store = create_test_store();
        let context_id = ContextId::from([1u8; 32]);
        let member = PublicKey::from([2u8; 32]);
        seed_identity(&store, context_id, member, Some([7u8; 32]));

        assert!(
            authorize_update_application(&store, &context_id, &member).is_ok(),
            "a local provisioned identity must be authorized"
        );
    }

    /// A known-but-remote identity (present in the context but with no private
    /// key on this node) is not a local member and must not drive an update.
    #[test]
    fn authorize_update_application_rejects_remote_identity_without_private_key() {
        let store = create_test_store();
        let context_id = ContextId::from([1u8; 32]);
        let remote = PublicKey::from([3u8; 32]);
        seed_identity(&store, context_id, remote, None);

        assert!(
            authorize_update_application(&store, &context_id, &remote).is_err(),
            "a remote identity without a local private key must be refused"
        );
    }

    /// Membership is scoped per context: a valid local identity in one context
    /// cannot authorize an update to a different context.
    #[test]
    fn authorize_update_application_is_scoped_to_the_context() {
        let store = create_test_store();
        let ctx_a = ContextId::from([1u8; 32]);
        let ctx_b = ContextId::from([2u8; 32]);
        let member = PublicKey::from([4u8; 32]);
        seed_identity(&store, ctx_a, member, Some([7u8; 32]));

        assert!(authorize_update_application(&store, &ctx_a, &member).is_ok());
        assert!(
            authorize_update_application(&store, &ctx_b, &member).is_err(),
            "an identity provisioned in context A must not authorize context B"
        );
    }

    /// Creates a test store with in-memory database.
    fn create_test_store() -> Store {
        let db = InMemoryDB::owned();
        Store::new(Arc::new(db))
    }

    /// Creates a test ApplicationMeta with the given signer_id.
    fn create_app_meta(signer_id: &str) -> types::ApplicationMeta {
        types::ApplicationMeta::new(
            key::BlobMeta::new([1u8; 32].into()),
            1024,
            "file://test.wasm".into(),
            vec![].into(),
            key::BlobMeta::new([2u8; 32].into()),
            types::PackageInfo {
                package: "com.test.app".into(),
                version: "1.0.0".into(),
                signer_id: signer_id.into(),
            },
        )
    }

    /// Creates a test Context with the given application_id.
    fn create_test_context(context_id: ContextId, application_id: ApplicationId) -> Context {
        Context::new(context_id, application_id, Hash::from([0u8; 32]))
    }

    // Test migration succeeds with valid signerId

    #[test]
    fn test_appkey_continuity_passes_with_matching_signer_ids() {
        // Setup: Create store and two applications with the same signerId
        let store = create_test_store();
        let signer_id = "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK";

        // Create old and new application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        // Create application metadata with the same signerId for both
        let old_app_meta = create_app_meta(signer_id);
        let new_app_meta = create_app_meta(signer_id);

        // Store both applications in the database
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        // Create a context that uses the old application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Verify AppKey continuity passes
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_ok(),
            "AppKey continuity check should pass with matching signerIds: {:?}",
            result.err()
        );
    }

    // application_version resolves ApplicationMeta.version (semver) for the emit.
    #[test]
    fn application_version_reads_semver_and_handles_missing() {
        let store = create_test_store();
        let app_id = ApplicationId::from([42u8; 32]);
        store
            .handle()
            .put(
                &key::ApplicationMeta::new(app_id),
                &create_app_meta("signer"),
            )
            .expect("seed app meta");
        assert_eq!(
            application_version(&store, app_id).as_deref(),
            Some("1.0.0")
        );
        assert_eq!(
            application_version(&store, ApplicationId::from([99u8; 32])),
            None
        );
    }

    // No event when the application id did not actually change (6f.5 dedup).
    #[test]
    fn app_version_changed_event_skips_when_unchanged() {
        let id = ApplicationId::from([1u8; 32]);
        let ev = app_version_changed_event(
            ContextId::from([7u8; 32]),
            id,
            id,
            Some("1.0.0".to_owned()),
            Some("1.0.0".to_owned()),
        );
        assert!(ev.is_none(), "no emit when app id unchanged");
    }

    // On a real flip, build the AppVersionChanged event with both versions.
    #[test]
    fn app_version_changed_event_on_flip() {
        let ctx = ContextId::from([7u8; 32]);
        let ev = app_version_changed_event(
            ctx,
            ApplicationId::from([1u8; 32]),
            ApplicationId::from([2u8; 32]),
            Some("1.0.0".to_owned()),
            Some("2.0.0".to_owned()),
        );
        match ev {
            Some(NodeEvent::Context(ContextEvent {
                context_id,
                payload: ContextEventPayload::AppVersionChanged(p),
            })) => {
                assert_eq!(context_id, ctx);
                assert_eq!(p.from_version.as_deref(), Some("1.0.0"));
                assert_eq!(p.to_version.as_deref(), Some("2.0.0"));
            }
            other => panic!("expected AppVersionChanged, got {other:?}"),
        }
    }

    // #2060: the same-id no-migration skip is safe ONLY when the context already
    // executes the exact bytecode now installed under that id. A signed bundle's
    // ApplicationId is version-stable, so a code-only upgrade leaves the id equal
    // while the blob changes — the skip must NOT fire there.
    #[test]
    fn same_id_update_skips_only_when_installed_bytecode_already_active() {
        let blob_v1 = [1u8; 32];
        let blob_v2 = [2u8; 32];

        // Genuine no-op: the context already activated the installed bytecode.
        assert!(same_id_update_is_noop(Some(blob_v1), Some(blob_v1)));

        // #2060: a new bundle replaced the bytecode under the same id; the
        // context still runs the old blob, so the update must proceed.
        assert!(!same_id_update_is_noop(Some(blob_v1), Some(blob_v2)));

        // No activation marker yet — cannot prove up-to-date, so proceed.
        assert!(!same_id_update_is_noop(None, Some(blob_v2)));

        // Installed application row unreadable — proceed conservatively.
        assert!(!same_id_update_is_noop(Some(blob_v1), None));
    }

    #[test]
    fn test_appkey_continuity_passes_for_new_context_without_old_app() {
        // Setup: Create store with only the new application
        let store = create_test_store();
        let new_signer_id = "did:key:z6MkNewSignerKey123456789";

        // Create only the new application ID
        let old_app_id = ApplicationId::from([10u8; 32]); // This won't exist in the store
        let new_app_id = ApplicationId::from([20u8; 32]);

        // Store only the new application
        let new_app_meta = create_app_meta(new_signer_id);
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        // Create a context that references a non-existent old application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Verify AppKey continuity passes (new context case)
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_ok(),
            "AppKey continuity check should pass for new context: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_appkey_continuity_passes_with_empty_signer_ids_legacy_apps() {
        // Setup: Test backward compatibility with legacy unsigned applications
        let store = create_test_store();

        // Create old and new application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        // Create application metadata with empty signerIds (legacy applications)
        let old_app_meta = create_app_meta("");
        let new_app_meta = create_app_meta("");

        // Store both applications in the database
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        // Create a context that uses the old application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Verify AppKey continuity passes (legacy to legacy is allowed)
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_ok(),
            "AppKey continuity check should pass for legacy apps: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_appkey_continuity_passes_when_upgrading_from_unsigned_to_signed() {
        // Setup: Test upgrading from unsigned (legacy) to signed application
        let store = create_test_store();
        let new_signer_id = "did:key:z6MkNewSignerKey123456789";

        // Create old and new application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        // Create old app with empty signerId (legacy) and new app with signerId
        let old_app_meta = create_app_meta("");
        let new_app_meta = create_app_meta(new_signer_id);

        // Store both applications in the database
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        // Create a context that uses the old application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Verify AppKey continuity passes (unsigned to signed is allowed with warning)
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_ok(),
            "AppKey continuity check should pass when upgrading from unsigned to signed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_appkey_continuity_rejects_downgrade_from_signed_to_unsigned() {
        // Setup: Test downgrading from signed to unsigned (legacy) application
        // Security: This is explicitly rejected to prevent security vulnerabilities
        let store = create_test_store();
        let old_signer_id = "did:key:z6MkOldSignerKey123456789";

        // Create old and new application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        // Create old app with signerId and new app without signerId (legacy)
        let old_app_meta = create_app_meta(old_signer_id);
        let new_app_meta = create_app_meta(""); // Empty signerId (legacy)

        // Store both applications in the database
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        // Create a context that uses the old application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Verify AppKey continuity rejects signed-to-unsigned downgrade
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_err(),
            "AppKey continuity check should reject downgrade from signed to unsigned: {result:?}"
        );

        // Verify the error message contains the expected content
        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains("Security downgrade rejected"),
            "Error should mention security downgrade rejection: {error_message}"
        );
        assert!(
            error_message.contains("signed application"),
            "Error should mention signed application: {error_message}"
        );
    }

    #[test]
    fn test_appkey_continuity_passes_when_updating_same_application() {
        // Setup: Test updating to a newer version of the same application (same signerId)
        let store = create_test_store();
        let signer_id = "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK";

        // In practice, same application with different version would have different ApplicationId
        // but same signerId - this is the standard upgrade path
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([11u8; 32]); // Different version = different hash

        // Both versions have the same signerId (same publisher)
        let old_app_meta = create_app_meta(signer_id);
        let new_app_meta = create_app_meta(signer_id);

        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Standard upgrade should pass
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_ok(),
            "Standard application upgrade should pass: {:?}",
            result.err()
        );
    }

    // Test migration rejected with signerId mismatch

    #[test]
    fn test_appkey_continuity_fails_with_mismatched_signer_ids() {
        // Setup: Create store and two applications with different signerIds
        let store = create_test_store();
        let old_signer_id = "did:key:z6MkOldSignerKey123456789";
        let new_signer_id = "did:key:z6MkNewSignerKey987654321";

        // Create old and new application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        // Create application metadata with different signerIds
        let old_app_meta = create_app_meta(old_signer_id);
        let new_app_meta = create_app_meta(new_signer_id);

        // Store both applications in the database
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        // Create a context that uses the old application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Verify AppKey continuity fails
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_err(),
            "AppKey continuity check should fail with mismatched signerIds"
        );

        // Verify the error message contains the expected content
        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains("AppKey continuity violation"),
            "Error should mention AppKey continuity violation: {error_message}"
        );
        assert!(
            error_message.contains("signerId mismatch"),
            "Error should mention signerId mismatch: {error_message}"
        );
    }

    #[test]
    fn test_appkey_continuity_fails_when_new_app_not_found() {
        // Setup: Create store with only the old application
        let store = create_test_store();
        let old_signer_id = "did:key:z6MkOldSignerKey123456789";

        // Create application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]); // This won't exist in the store

        // Store only the old application
        let old_app_meta = create_app_meta(old_signer_id);
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");

        // Create a context that uses the old application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Verify AppKey continuity fails because new app doesn't exist
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_err(),
            "AppKey continuity check should fail when new app not found"
        );

        // Verify the error message mentions the new app not being found
        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains("not found"),
            "Error should mention app not found: {error_message}"
        );
    }

    #[test]
    fn test_appkey_continuity_prevents_hijacking_attempt() {
        // Setup: Simulate an attacker trying to hijack an app with a different signerId
        let store = create_test_store();
        let legitimate_signer = "did:key:z6MkLegitimatePublisher1234567890";
        let attacker_signer = "did:key:z6MkAttackerTryingToHijack999999";

        // Create application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let attacker_app_id = ApplicationId::from([99u8; 32]);

        // Create legitimate app and attacker's app with different signerIds
        let legitimate_app_meta = create_app_meta(legitimate_signer);
        let attacker_app_meta = create_app_meta(attacker_signer);

        // Store both applications
        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &legitimate_app_meta)
            .expect("Failed to store legitimate app meta");
        handle
            .put(
                &key::ApplicationMeta::new(attacker_app_id),
                &attacker_app_meta,
            )
            .expect("Failed to store attacker app meta");

        // Create a context using the legitimate application
        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Attacker tries to update to their malicious application
        let result = verify_appkey_continuity(&store, &context, &attacker_app_id);
        assert!(
            result.is_err(),
            "AppKey continuity check should prevent hijacking attempt"
        );

        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains("signerId mismatch"),
            "Error should indicate signerId mismatch: {error_message}"
        );
    }

    #[test]
    fn test_appkey_continuity_is_case_sensitive() {
        // Security test: Verify that signerId comparison is case-sensitive
        // An attacker should not be able to bypass check by changing case
        let store = create_test_store();
        let legitimate_signer = "did:key:z6MkABCDEFGH123456789";
        let case_modified_signer = "did:key:z6Mkabcdefgh123456789"; // Same but lowercase

        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        let old_app_meta = create_app_meta(legitimate_signer);
        let new_app_meta = create_app_meta(case_modified_signer);

        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Case-modified signerId should be rejected
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_err(),
            "SignerId comparison must be case-sensitive"
        );
    }

    #[test]
    fn test_appkey_continuity_fails_with_similar_looking_signer_ids() {
        // Security test: Verify that similar-looking signerIds are still rejected
        // Attackers might try using visually similar characters
        let store = create_test_store();
        let legitimate_signer = "did:key:z6MkPublisher0123456789";
        let similar_signer = "did:key:z6MkPublisherO123456789"; // 'O' instead of '0'

        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        let old_app_meta = create_app_meta(legitimate_signer);
        let new_app_meta = create_app_meta(similar_signer);

        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Similar-looking signerId should still be rejected
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_err(),
            "Similar-looking signerIds must still be rejected"
        );
    }

    #[test]
    fn test_appkey_continuity_fails_with_whitespace_differences() {
        // Security test: Verify that whitespace variations are rejected
        let store = create_test_store();
        let legitimate_signer = "did:key:z6MkPublisher123";
        let whitespace_signer = "did:key:z6MkPublisher123 "; // Trailing space

        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        let old_app_meta = create_app_meta(legitimate_signer);
        let new_app_meta = create_app_meta(whitespace_signer);

        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // SignerId with whitespace difference should be rejected
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_err(),
            "SignerIds with whitespace differences must be rejected"
        );
    }

    #[test]
    fn test_appkey_continuity_rejects_prefix_attack() {
        // Security test: Attacker tries to use signerId that is a prefix/suffix
        let store = create_test_store();
        let legitimate_signer = "did:key:z6MkPublisher123456789";
        let prefix_signer = "did:key:z6MkPublisher123"; // Prefix of legitimate

        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);

        let old_app_meta = create_app_meta(legitimate_signer);
        let new_app_meta = create_app_meta(prefix_signer);

        let mut handle = store.handle();
        handle
            .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
            .expect("Failed to store old app meta");
        handle
            .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
            .expect("Failed to store new app meta");

        let context_id = ContextId::from([1u8; 32]);
        let context = create_test_context(context_id, old_app_id);

        // Prefix signerId should be rejected
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(result.is_err(), "Prefix signerId attack must be rejected");
    }

    // Test rollback on migration failure

    #[test]
    fn test_no_state_written_when_appkey_continuity_fails() {
        // Setup: Verify that no state changes occur when AppKey continuity check fails
        let store = create_test_store();
        let old_signer_id = "did:key:z6MkOldSignerKey123456789";
        let new_signer_id = "did:key:z6MkNewSignerKey987654321";

        // Create application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);
        let context_id = ContextId::from([1u8; 32]);

        // Create application metadata with different signerIds
        let old_app_meta = create_app_meta(old_signer_id);
        let new_app_meta = create_app_meta(new_signer_id);

        // Store initial context state
        let initial_state = b"initial_state_data";
        let root_key = calimero_storage::constants::root_storage_key();
        let state_key = key::ContextState::new(context_id, root_key);
        let state_value =
            types::ContextState::from(calimero_store::slice::Slice::from(initial_state.to_vec()));

        // Store both applications and initial state
        {
            let mut handle = store.handle();
            handle
                .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
                .expect("Failed to store old app meta");
            handle
                .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
                .expect("Failed to store new app meta");
            handle
                .put(&state_key, &state_value)
                .expect("Failed to store initial state");
        }

        // Create context
        let context = create_test_context(context_id, old_app_id);

        // Attempt update that should fail AppKey continuity check
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(result.is_err(), "AppKey continuity check should fail");

        // Verify the original state is unchanged (no partial writes occurred)
        let handle = store.handle();
        let stored_state: Option<types::ContextState> =
            handle.get(&state_key).expect("Failed to read state");

        assert!(
            stored_state.is_some(),
            "Original state should still exist after failed update"
        );

        let stored_bytes: &[u8] = stored_state.as_ref().unwrap().as_ref();
        assert_eq!(
            stored_bytes, initial_state,
            "State should be unchanged after failed AppKey continuity check"
        );
    }

    #[test]
    fn test_context_meta_unchanged_when_update_fails() {
        // Setup: Verify context metadata is not modified when update fails
        let store = create_test_store();
        let old_signer_id = "did:key:z6MkOldSignerKey123456789";
        let new_signer_id = "did:key:z6MkDifferentSignerKey000000";

        // Create application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let new_app_id = ApplicationId::from([20u8; 32]);
        let context_id = ContextId::from([1u8; 32]);

        // Create application metadata
        let old_app_meta = create_app_meta(old_signer_id);
        let new_app_meta = create_app_meta(new_signer_id);

        // Store initial context metadata
        let original_root_hash = Hash::from([42u8; 32]);
        let context_meta = types::ContextMeta::new(
            key::ApplicationMeta::new(old_app_id),
            *original_root_hash,
            vec![],
            None,
        );

        // Store applications and context metadata
        {
            let mut handle = store.handle();
            handle
                .put(&key::ApplicationMeta::new(old_app_id), &old_app_meta)
                .expect("Failed to store old app meta");
            handle
                .put(&key::ApplicationMeta::new(new_app_id), &new_app_meta)
                .expect("Failed to store new app meta");
            handle
                .put(&key::ContextMeta::new(context_id), &context_meta)
                .expect("Failed to store context meta");
        }

        // Create context
        let context = create_test_context(context_id, old_app_id);

        // Attempt update that should fail
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(result.is_err(), "AppKey continuity check should fail");

        // Verify context metadata is unchanged
        let handle = store.handle();
        let stored_meta: Option<types::ContextMeta> = handle
            .get(&key::ContextMeta::new(context_id))
            .expect("Failed to read context meta");

        assert!(
            stored_meta.is_some(),
            "Context metadata should still exist after failed update"
        );

        let meta = stored_meta.unwrap();
        assert_eq!(
            meta.root_hash, *original_root_hash,
            "Context root_hash should be unchanged after failed update"
        );
    }

    #[test]
    fn test_multiple_failed_updates_preserve_original_state() {
        // Setup: Verify that multiple failed update attempts don't corrupt state
        let store = create_test_store();
        let legitimate_signer = "did:key:z6MkLegitimatePublisher1234567890";

        // Create multiple attacker signerIds
        let attacker_signers = [
            "did:key:z6MkAttacker1111111111111111111111",
            "did:key:z6MkAttacker2222222222222222222222",
            "did:key:z6MkAttacker3333333333333333333333",
        ];

        // Create application IDs
        let old_app_id = ApplicationId::from([10u8; 32]);
        let context_id = ContextId::from([1u8; 32]);

        // Create and store legitimate application
        let legitimate_app_meta = create_app_meta(legitimate_signer);
        {
            let mut handle = store.handle();
            handle
                .put(&key::ApplicationMeta::new(old_app_id), &legitimate_app_meta)
                .expect("Failed to store legitimate app meta");
        }

        // Store initial state
        let original_state = b"precious_application_state";
        let root_key = calimero_storage::constants::root_storage_key();
        let state_key = key::ContextState::new(context_id, root_key);
        {
            let mut handle = store.handle();
            handle
                .put(
                    &state_key,
                    &types::ContextState::from(calimero_store::slice::Slice::from(
                        original_state.to_vec(),
                    )),
                )
                .expect("Failed to store initial state");
        }

        // Create context
        let context = create_test_context(context_id, old_app_id);

        // Simulate multiple failed hijacking attempts
        for (i, attacker_signer) in attacker_signers.iter().enumerate() {
            let attacker_app_id = ApplicationId::from([(100 + i as u8); 32]);
            let attacker_app_meta = create_app_meta(attacker_signer);

            // Store attacker's app
            {
                let mut handle = store.handle();
                handle
                    .put(
                        &key::ApplicationMeta::new(attacker_app_id),
                        &attacker_app_meta,
                    )
                    .expect("Failed to store attacker app meta");
            }

            // Attempt update - should fail
            let result = verify_appkey_continuity(&store, &context, &attacker_app_id);
            assert!(
                result.is_err(),
                "Attempt {} should fail AppKey continuity check",
                i + 1
            );
        }

        // Verify original state is completely intact after all failed attempts
        let handle = store.handle();
        let stored_state: Option<types::ContextState> =
            handle.get(&state_key).expect("Failed to read state");

        assert!(
            stored_state.is_some(),
            "State should still exist after multiple failed attacks"
        );

        let stored_bytes: &[u8] = stored_state.as_ref().unwrap().as_ref();
        assert_eq!(
            stored_bytes, original_state,
            "State should be completely unchanged after multiple failed hijacking attempts"
        );
    }

    /// Characterizes the clean-rollback property the logical abort relies on:
    /// the v1 root entry is never mutated until the migration commits, so a
    /// pre-commit early-return (a logical abort) leaves it fully intact.
    ///
    /// The whole-root migrate path writes the root through exactly one seam —
    /// `write_migration_state`, the sole caller of
    /// `Interface::write_pre_merged_root_state` in this file. Everything before
    /// that is pure computation against a still-v1 store, so "abort" is simply
    /// not reaching that single writer; there is no byte snapshot to restore
    /// because the v1 root was never overwritten.
    ///
    /// This test locks that single-writer invariant at the source level. If a
    /// second root-write site appears, the abort path can no longer guarantee a
    /// clean rollback and this test must be revisited (not merely updated).
    #[test]
    fn clean_rollback_root_written_only_via_write_migration_state() {
        let full = include_str!("mod.rs");
        // Scope to the non-test region so this test's own string literals
        // (which mention the writer by name) don't pollute the count.
        let source = full
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .expect("file should contain a #[cfg(test)] mod tests boundary");

        let write_sites = source.matches("write_pre_merged_root_state(").count();
        assert_eq!(
            write_sites, 1,
            "expected exactly one `write_pre_merged_root_state(` call site (the sole \
             root writer, inside `write_migration_state`) so a pre-commit logical abort \
             leaves the v1 root untouched; found {write_sites}. A new root-write site \
             breaks the clean-rollback guarantee."
        );

        // The single writer must live inside `write_migration_state` — the
        // function the abort path skips. Assert the writer call appears after
        // `fn write_migration_state(` in source order (it is its only caller).
        let writer_fn = source
            .find("fn write_migration_state(")
            .expect("write_migration_state should be defined in this file");
        let write_call = source
            .find("write_pre_merged_root_state(")
            .expect("the root writer call should exist in this file");
        assert!(
            write_call > writer_fn,
            "the sole `write_pre_merged_root_state` call must sit inside \
             `write_migration_state` (the seam a logical abort skips)"
        );
    }

    /// A missing `__calimero_migration_check` export must be treated as a pass
    /// (`Ok(true)`) so apps that do not define a check are never blocked — the
    /// backwards-compatible default.
    #[test]
    fn migration_check_verdict_missing_export_passes() {
        use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};

        let returns: Result<Option<Vec<u8>>, FunctionCallError> = Err(
            FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
                name: "__calimero_migration_check".to_owned(),
            }),
        );

        let verdict =
            super::decode_migration_check_verdict(returns).expect("missing export must not error");
        assert!(
            verdict,
            "a missing migration_check export must pass (Ok(true)) for backwards compatibility"
        );
    }

    /// A present check that returns `borsh(true)` passes; `borsh(false)` fails.
    /// The verdict bytes are raw `borsh(bool)` (mirroring `#[app::migrate]`'s
    /// raw-new-state-bytes contract), NOT a borsh `Result<bool, _>` envelope.
    #[test]
    fn migration_check_verdict_decodes_raw_borsh_bool() {
        use calimero_runtime::errors::FunctionCallError;

        let pass_bytes = borsh::to_vec(&true).expect("serialize true");
        let pass: Result<Option<Vec<u8>>, FunctionCallError> = Ok(Some(pass_bytes));
        assert!(
            super::decode_migration_check_verdict(pass).expect("decode true"),
            "borsh(true) verdict must decode to a passing check"
        );

        let fail_bytes = borsh::to_vec(&false).expect("serialize false");
        let fail: Result<Option<Vec<u8>>, FunctionCallError> = Ok(Some(fail_bytes));
        assert!(
            !super::decode_migration_check_verdict(fail).expect("decode false"),
            "borsh(false) verdict must decode to a failing check"
        );
    }

    /// A WASM trap (non-`MethodNotFound` error) fails closed: the verdict is
    /// `false` so the migration is logically aborted rather than committed on a
    /// check that could not run.
    #[test]
    fn migration_check_verdict_trap_fails_closed() {
        use calimero_runtime::errors::{FunctionCallError, WasmTrap};

        let returns: Result<Option<Vec<u8>>, FunctionCallError> =
            Err(FunctionCallError::WasmTrap(WasmTrap::Unreachable));

        let verdict = super::decode_migration_check_verdict(returns)
            .expect("a trap must be reported as a failing verdict, not a hard error");
        assert!(
            !verdict,
            "a trapping migration_check must fail closed (verdict false ⇒ logical abort)"
        );
    }

    /// The `MigrationCheckFailed` error carries the context id and surfaces a
    /// recognisable "logical abort" message so callers (and e2e log assertions)
    /// can detect the abort.
    #[test]
    fn migration_check_error_is_recognisable() {
        let context_id = ContextId::from([7u8; 32]);
        let err = super::MigrationCheckFailed { context_id };
        let msg = err.to_string();
        assert!(
            msg.contains("migration_check failed"),
            "error message must announce the failed check: {msg}"
        );
        assert!(
            msg.contains("logical abort"),
            "error message must announce the logical abort: {msg}"
        );
    }

    /// Reads the committed root entry's Merkle `full_hash` straight from the
    /// store, the same hash `write_migration_state` derives for
    /// `Context::root_hash`. Returns `None` before any root has been written.
    fn root_full_hash(store: &Store, context_id: ContextId) -> Option<[u8; 32]> {
        use calimero_prelude::ROOT_STORAGE_ENTRY_ID;
        use calimero_storage::address::Id;
        use calimero_storage::index::Index;
        use calimero_storage::store::{Key, MainStorage};

        let read = {
            let handle = store.handle();
            move |key: &Key| -> Option<Vec<u8>> {
                let state_key = key::ContextState::new(context_id, key.to_bytes());
                handle
                    .get(&state_key)
                    .ok()
                    .flatten()
                    .map(|s| s.value.into_boxed().into_vec())
            }
        };
        let noop_write = |_: Key, _: &[u8]| true;
        let noop_remove = |_: &Key| true;

        let env = calimero_storage::env::RuntimeEnv::new(
            std::rc::Rc::new(read),
            std::rc::Rc::new(noop_write),
            std::rc::Rc::new(noop_remove),
            *context_id.as_ref(),
            [0u8; 32],
        );
        let _ = ROOT_STORAGE_ENTRY_ID;
        calimero_storage::env::with_runtime_env(env, || {
            Index::<MainStorage>::get_hashes_for(Id::root())
                .ok()
                .flatten()
                .map(|(full_hash, _)| full_hash)
        })
    }

    /// A failed `migration_check` drives a logical abort through the real
    /// commit/abort seam — and now a TRUE zero-residue rollback: the migrate's
    /// child-entry writes are buffered in the staging `ContextStorage` and
    /// DROPPED on abort, never reaching the live store. Asserts:
    ///   (a) the seam returns `Err(MigrationCheckFailed { context_id })`;
    ///   (b) the committed root entry's `full_hash` is identical to the
    ///       pre-migration v1 hash (v1 root never overwritten);
    ///   (c) the in-flight `Context` (application_id, root_hash, dag_heads) is
    ///       left untouched, so a later `finalize_application_update` would never
    ///       publish the v2 application_id;
    ///   (d) the staged v2 child entry is ABSENT from the live store after the
    ///       abort (zero residue) — and PRESENT after a passing commit.
    #[tokio::test]
    async fn failed_migration_check_logically_aborts() {
        use calimero_runtime::store::Storage as _;

        let store = create_test_store();

        let context_id = ContextId::from([3u8; 32]);
        let app_id_v1 = ApplicationId::from([10u8; 32]);
        let mut context = create_test_context(context_id, app_id_v1);

        let executor = calimero_primitives::identity::PublicKey::from([5u8; 32]);

        // Install a real v1 root entry through the same storage seam the migrate
        // flow uses, so it has a genuine Merkle Index + full_hash to compare
        // against. `write_migration_state` is the sole root writer in this file.
        let v1_bytes = borsh::to_vec(&("v1-state".to_owned())).expect("serialize v1 state");
        let full_hash_v1 = super::write_migration_state(&store, &context, &v1_bytes, executor)
            .expect("install v1 root");
        context.root_hash = Hash::from(full_hash_v1);
        context.dag_heads = vec![full_hash_v1];

        let dag_heads_v1 = context.dag_heads.clone();

        assert_eq!(
            root_full_hash(&store, context_id),
            Some(full_hash_v1),
            "sanity: the v1 root full_hash must be readable before the abort"
        );

        // A deterministic child entry key, as a migrate would write via the
        // buffer. `is_present` reads the LIVE store (a fresh handle), so it only
        // sees writes that were actually promoted — not buffered ones.
        let child_key = key::ContextState::new(context_id, [0xABu8; 32]);
        let is_present = |store: &calimero_store::Store| {
            store
                .handle()
                .get(&child_key)
                .expect("read child entry")
                .is_some()
        };
        let v2_bytes = borsh::to_vec(&("v2-state-much-longer".to_owned())).expect("serialize v2");

        // Reads the node-local migration-failure marker (the heartbeat's source
        // for surfacing a member as `failed`), as its raw discriminant.
        let failed_marker = |store: &calimero_store::Store| {
            store
                .handle()
                .get(&key::ContextMigrationFailed::new(context_id))
                .expect("read failure marker")
                .map(|m| m.kind)
        };

        // --- ABORT: stage a v2 child through the buffer, then drop it. ---
        {
            let mut staging = ContextStorage::from(store.clone(), context_id);
            let _ = staging.set(vec![0xABu8; 32], b"v2-child-staged".to_vec());
            assert!(
                !is_present(&store),
                "the staged child must be buffered (not live) before commit"
            );

            let result = super::commit_or_abort_migration(
                &store,
                &mut context,
                &v2_bytes,
                executor,
                super::MigrationGateDecision::Abort,
                staging,
            );

            // (a) the seam returns the abort error.
            let err = result.expect_err("a failed migration_check must abort with an error");
            let downcast = err.downcast_ref::<super::MigrationCheckFailed>();
            assert!(
                matches!(downcast, Some(super::MigrationCheckFailed { context_id: cid }) if *cid == context_id),
                "abort error must be MigrationCheckFailed carrying the context id: {err}"
            );

            // (b) the committed root entry is byte-for-byte the pre-migration v1 hash.
            assert_eq!(
                root_full_hash(&store, context_id),
                Some(full_hash_v1),
                "v1 root full_hash must be unchanged after a logical abort (no byte mutation)"
            );

            // (c) the in-flight context still points entirely at v1.
            assert_eq!(
                context.application_id, app_id_v1,
                "application_id must NOT be finalized after a logical abort"
            );
            assert_eq!(
                context.root_hash,
                Hash::from(full_hash_v1),
                "context.root_hash must stay on v1 after a logical abort"
            );
            assert_eq!(
                context.dag_heads, dag_heads_v1,
                "context.dag_heads must stay on v1 after a logical abort"
            );

            // (d) ZERO RESIDUE: the staged child never reached the live store.
            assert!(
                !is_present(&store),
                "abort must DROP the staged child entry — zero residue"
            );

            // (e) the abort persists a CheckAborted marker so the heartbeat
            // surfaces this member as `failed` rather than a silent in-progress.
            assert_eq!(
                failed_marker(&store),
                Some(super::MigrationFailureKind::CheckAborted.to_u8()),
                "a logical abort must persist the check-aborted failure marker"
            );
        }

        // --- COMMIT: stage a v2 child through the buffer, then promote it. ---
        {
            let mut staging = ContextStorage::from(store.clone(), context_id);
            let _ = staging.set(vec![0xABu8; 32], b"v2-child-staged".to_vec());

            super::commit_or_abort_migration(
                &store,
                &mut context,
                &v2_bytes,
                executor,
                super::MigrationGateDecision::Commit,
                staging,
            )
            .expect("a passing check must commit");

            // The root advances to the produced v2 bytes...
            assert_ne!(
                root_full_hash(&store, context_id),
                Some(full_hash_v1),
                "a passing check must overwrite the v1 root with the produced v2 bytes"
            );
            assert_eq!(
                context.root_hash,
                root_full_hash(&store, context_id).map(Hash::from).unwrap(),
                "a passing check must advance context.root_hash to the new v2 root"
            );
            // ...and the staged child is flushed to the live store.
            assert!(
                is_present(&store),
                "commit must FLUSH the staged child entry to the live store"
            );

            // ...and the prior abort's failure marker is cleared (self-heal): a
            // context that recovered must stop reporting `failed`.
            assert_eq!(
                failed_marker(&store),
                None,
                "a passing commit must clear the prior failure marker"
            );
        }
    }

    /// The gate-decision seam: every commit/abort decision funnels through one
    /// [`MigrationGateDecision`] so a future canary-subgroup gate (deferred) can
    /// supply the decision later without touching `commit_or_abort_migration`.
    /// Locks two properties:
    ///   (a) the migration_check verdict maps onto the gate decision —
    ///       `true ⇒ Commit`, `false ⇒ Abort` — via `from_check_result`, the
    ///       single point a canary path would replace;
    ///   (b) the seam routes `Commit` to a real root write and `Abort` to the
    ///       logical abort (`Err(MigrationCheckFailed)`, v1 root untouched).
    #[tokio::test]
    async fn migration_gate_decision_maps_verdict_to_commit_or_abort() {
        use super::MigrationGateDecision;

        // (a) the verdict → decision mapping (the seam canary will later own).
        assert!(
            matches!(
                MigrationGateDecision::from_check_result(true),
                MigrationGateDecision::Commit
            ),
            "a passing check must yield Commit"
        );
        assert!(
            matches!(
                MigrationGateDecision::from_check_result(false),
                MigrationGateDecision::Abort
            ),
            "a failing check must yield Abort"
        );

        // (b) the seam routes each decision through `commit_or_abort_migration`.
        let store = create_test_store();
        let context_id = ContextId::from([4u8; 32]);
        let app_id_v1 = ApplicationId::from([11u8; 32]);
        let mut context = create_test_context(context_id, app_id_v1);
        let executor = calimero_primitives::identity::PublicKey::from([6u8; 32]);

        let v1_bytes = borsh::to_vec(&("v1-state".to_owned())).expect("serialize v1 state");
        let full_hash_v1 = super::write_migration_state(&store, &context, &v1_bytes, executor)
            .expect("install v1 root");
        context.root_hash = Hash::from(full_hash_v1);
        context.dag_heads = vec![full_hash_v1];

        let v2_bytes = borsh::to_vec(&("v2-state-much-longer".to_owned())).expect("serialize v2");

        // Abort decision ⇒ logical abort, v1 root untouched.
        let abort = super::commit_or_abort_migration(
            &store,
            &mut context,
            &v2_bytes,
            executor,
            MigrationGateDecision::Abort,
            ContextStorage::from(store.clone(), context_id),
        );
        assert!(
            abort
                .expect_err("Abort must surface the logical-abort error")
                .downcast_ref::<super::MigrationCheckFailed>()
                .is_some(),
            "Abort must map to MigrationCheckFailed (logical abort)"
        );
        assert_eq!(
            root_full_hash(&store, context_id),
            Some(full_hash_v1),
            "Abort must leave the v1 root byte-for-byte unchanged"
        );

        // Commit decision ⇒ root write, v1 root overwritten.
        super::commit_or_abort_migration(
            &store,
            &mut context,
            &v2_bytes,
            executor,
            MigrationGateDecision::Commit,
            ContextStorage::from(store.clone(), context_id),
        )
        .expect("Commit must write the produced v2 root");
        assert_ne!(
            root_full_hash(&store, context_id),
            Some(full_hash_v1),
            "Commit must overwrite the v1 root with the produced v2 bytes"
        );
    }
}
