use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{MigrationParams, UpdateApplicationRequest};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::constants::root_storage_key;
use calimero_store::slice::Slice;
use calimero_store::{key, types};
use calimero_utils_actix::global_runtime;
use eyre::bail;
use tracing::{debug, error, info, warn};

use crate::handlers::execute::storage::ContextStorage;
use crate::handlers::utils::StoreContextHost;
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

        let context_meta = self.contexts.get(&context_id).map(|c| c.meta.clone());

        if let Some(ref context) = context_meta {
            if application_id == context.application_id {
                debug!(%context_id, "Application already set, skipping update");
                return ActorResponse::reply(Ok(()));
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

            // Clone values needed for migration
            let datastore = self.datastore.clone();
            let node_client = self.node_client.clone();
            let context_client = self.context_client.clone();
            let migration_params = migration_params.clone();

            // First load the module
            let module_task = self.get_module(application_id);

            let task = module_task.and_then(move |module, act, _ctx| {
                let datastore = datastore.clone();
                let node_client = node_client.clone();
                let context_client = context_client.clone();
                let context_meta = act.contexts.get(&context_id).map(|c| c.meta.clone());
                let application = act.applications.get(&application_id).cloned();

                async move {
                    update_application_with_migration(
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
                    )
                    .await
                }
                .into_actor(act)
            });

            return ActorResponse::r#async(task.map_ok(move |application, act, _ctx| {
                let _ignored = act
                    .applications
                    .entry(application_id)
                    .or_insert(application);

                if let Some(context) = act.contexts.get_mut(&context_id) {
                    context.meta.application_id = application_id;
                }
            }));
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
            let _ignored = act
                .applications
                .entry(application_id)
                .or_insert(application);

            if let Some(context) = act.contexts.get_mut(&context_id) {
                context.meta.application_id = application_id;
            }
        }))
    }
}

pub async fn update_application_id(
    datastore: calimero_store::Store,
    node_client: NodeClient,
    context_client: ContextClient,
    context_id: ContextId,
    context: Option<Context>,
    application_id: ApplicationId,
    application: Option<Application>,
    public_key: PublicKey,
) -> eyre::Result<Application> {
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

    // Task 3.1: Verify AppKey continuity (signerId match)
    verify_appkey_continuity(&datastore, &context, &application_id)?;

    let Some(config_client) = context_client.context_config(&context_id)? else {
        bail!(
            "missing context config parameters for context '{}'",
            context_id
        );
    };

    let external_client = context_client.external_client(&context_id, &config_client)?;

    external_client
        .config()
        .update_application(&public_key, &application)
        .await?;

    let mut handle = datastore.handle();

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(application.id),
            *context.root_hash,
            context.dag_heads.clone(),
        ),
    )?;

    node_client.sync(Some(&context_id), None).await?;

    Ok(application)
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

    // Warn if updating from unsigned to signed (or vice versa)
    if old_signer_id.is_empty() != new_signer_id.is_empty() {
        warn!(
            context_id = %context.id,
            old_has_signer = !old_signer_id.is_empty(),
            new_has_signer = !new_signer_id.is_empty(),
            "Updating between signed and unsigned applications"
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
async fn update_application_with_migration(
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
) -> eyre::Result<Application> {
    let mut context = match context {
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

    // Task 3.1: Verify AppKey continuity (signerId match)
    verify_appkey_continuity(&datastore, &context, &application_id)?;

    // Execute migration if requested
    if let Some(migration_params) = migration {
        info!(
            %context_id,
            %application_id,
            method = %migration_params.method,
            "Executing migration"
        );

        // Task 3.3: Execute migration function via module.run()
        let new_state_bytes = execute_migration(
            &datastore,
            node_client.clone(),
            &context,
            module,
            &migration_params,
        )
        .await?;

        // Task 3.4: Write returned state bytes to root storage key
        write_migration_state(&datastore, &context, &new_state_bytes)?;

        // Update root_hash after migration
        // The new root hash is computed from the migrated state
        let new_root_hash = Hash::new(&new_state_bytes);
        context.root_hash = new_root_hash;

        info!(
            %context_id,
            new_root_hash = %new_root_hash,
            state_size = new_state_bytes.len(),
            "Migration completed successfully"
        );
    }

    // Task 3.5: Update context metadata after migration
    let Some(config_client) = context_client.context_config(&context_id)? else {
        bail!(
            "missing context config parameters for context '{}'",
            context_id
        );
    };

    let external_client = context_client.external_client(&context_id, &config_client)?;

    external_client
        .config()
        .update_application(&public_key, &application)
        .await?;

    let mut handle = datastore.handle();

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(application.id),
            *context.root_hash,
            context.dag_heads.clone(),
        ),
    )?;

    node_client.sync(Some(&context_id), None).await?;

    Ok(application)
}

/// Execute the migration function in the new WASM module.
///
/// The migration function reads old state via `read_raw()` and returns new state bytes.
async fn execute_migration(
    datastore: &calimero_store::Store,
    node_client: NodeClient,
    context: &Context,
    module: calimero_runtime::Module,
    migration_params: &MigrationParams,
) -> eyre::Result<Vec<u8>> {
    let context_id = context.id;
    let method = migration_params.method.clone();

    debug!(
        %context_id,
        method = %method,
        "Preparing to execute migration function"
    );

    // Create storage for the migration execution
    let mut storage = ContextStorage::from(datastore.clone(), context_id);

    // Create host context for membership queries
    let context_host = StoreContextHost {
        store: datastore.clone(),
        context_id,
    };

    // Execute the migration function in a blocking task
    // Migration functions take no parameters - context is accessed via host functions
    let outcome = global_runtime()
        .spawn_blocking(move || {
            module.run(
                context_id,
                // Use a zero executor since migration is not user-initiated
                PublicKey::from([0u8; 32]),
                &method,
                // Empty input - migration functions read old state via read_raw()
                &[],
                &mut storage,
                Some(node_client),
                Some(Box::new(context_host)),
            )
        })
        .await
        .map_err(|e| eyre::eyre!("Migration task failed: {}", e))??;

    // Extract the return value from the outcome
    // Migration functions return serialized new state via value_return()
    let returns = outcome
        .returns
        .map_err(|e| eyre::eyre!("Migration execution failed: {:?}", e))?;

    let Some(return_bytes) = returns else {
        bail!("Migration function did not return any data. Ensure the migration function returns the new state.");
    };

    // The migration function wraps its return in Result<Vec<u8>, Vec<u8>>::Ok(bytes)
    // via `env::value_return(&Ok::<Vec<u8>, Vec<u8>>(output_bytes))`
    // We need to deserialize this Result wrapper
    let new_state_bytes: Result<Vec<u8>, Vec<u8>> = borsh::from_slice(&return_bytes)
        .map_err(|e| eyre::eyre!("Failed to deserialize migration return value: {}", e))?;

    match new_state_bytes {
        Ok(bytes) => {
            debug!(
                %context_id,
                bytes_len = bytes.len(),
                "Migration function returned new state"
            );
            Ok(bytes)
        }
        Err(error_bytes) => {
            let error_msg = String::from_utf8_lossy(&error_bytes);
            bail!("Migration function returned error: {}", error_msg);
        }
    }
}

/// Write migrated state bytes to the root storage key.
fn write_migration_state(
    datastore: &calimero_store::Store,
    context: &Context,
    new_state_bytes: &[u8],
) -> eyre::Result<()> {
    let context_id = context.id;

    // Get the root storage key
    let storage_key = root_storage_key();

    debug!(
        %context_id,
        storage_key = ?storage_key,
        state_size = new_state_bytes.len(),
        "Writing migrated state to root storage key"
    );

    // Write the new state to the context state storage
    let mut handle = datastore.handle();

    // Create the context state key
    let state_key = key::ContextState::new(context_id, storage_key);

    // Convert bytes to ContextState value via Slice
    let slice: Slice<'_> = new_state_bytes.to_vec().into();
    let state_value = types::ContextState::from(slice);

    // Write the new state bytes
    handle.put(&state_key, &state_value)?;

    debug!(
        %context_id,
        "Migrated state written successfully"
    );

    Ok(())
}
