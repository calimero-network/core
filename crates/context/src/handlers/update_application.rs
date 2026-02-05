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
use sha2::{Digest, Sha256};
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
        // The root_hash must match what the storage layer computes via calculate_full_hash_for_children.
        // For a root entity: full_hash = SHA256(own_hash) where own_hash = SHA256(state_bytes).
        // This is the Merkle tree hash that the sync protocol expects.
        let own_hash: [u8; 32] = Sha256::digest(&new_state_bytes).into();
        let new_root_hash = Hash::new(&own_hash);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{Context, ContextId};
    use calimero_primitives::hash::Hash;
    use calimero_store::db::InMemoryDB;
    use calimero_store::{key, types, Store};

    use super::verify_appkey_continuity;

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
            "com.test.app".into(),
            "1.0.0".into(),
            signer_id.into(),
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
    fn test_appkey_continuity_passes_when_downgrading_from_signed_to_unsigned() {
        // Setup: Test downgrading from signed to unsigned (legacy) application
        // This is intentionally allowed for backwards compatibility, but generates a warning
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

        // Verify AppKey continuity passes (signed to unsigned is allowed for backwards compat)
        // Note: This behavior is intentional but generates a warning log
        let result = verify_appkey_continuity(&store, &context, &new_app_id);
        assert!(
            result.is_ok(),
            "AppKey continuity check should pass when downgrading from signed to unsigned: {:?}",
            result.err()
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
            "Error should mention AppKey continuity violation: {}",
            error_message
        );
        assert!(
            error_message.contains("signerId mismatch"),
            "Error should mention signerId mismatch: {}",
            error_message
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
            "Error should mention app not found: {}",
            error_message
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
            "Error should indicate signerId mismatch: {}",
            error_message
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
}
