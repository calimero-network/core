use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{MigrationParams, UpdateApplicationRequest};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::PublicKey;
use calimero_runtime::store::{Key as RuntimeKey, Storage, Value as RuntimeValue};
use calimero_store::{key, types};
use calimero_utils_actix::global_runtime;
use eyre::{bail, WrapErr};

use futures_util::io::Cursor;

// Get access to execution logic via `ContextStorage`.
use super::execute::storage::ContextStorage;

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
        let context_meta = self.contexts.get(&context_id).map(|c| c.meta.clone());

        if let Some(ref context) = context_meta {
            // If the app ID is the same and no migration is provided, ignore.
            if application_id == context.application_id && migration.is_none() {
                return ActorResponse::reply(Ok(()));
            }
        }

        let application = self.applications.get(&application_id).cloned();

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let context_client = self.context_client.clone();

        let task = async move {
            let result: eyre::Result<Application> = async {
                // If application is not in cache, try fetching it from the node.
                let application = match application {
                    Some(app) => app,
                    None => node_client
                        .get_application(&application_id)?
                        .ok_or_else(|| {
                            eyre::eyre!("application with id '{}' not found", application_id)
                        })?,
                };

                let compiled_blob_id = application.blob.compiled;

                // Try to load precompiled module from the blobstore.
                let module_result = if let Some(compiled_bytes) =
                    node_client.get_blob_bytes(&compiled_blob_id, None).await?
                {
                    unsafe {
                        calimero_runtime::Engine::headless()
                            .from_precompiled(&compiled_bytes)
                            .ok()
                    }
                } else {
                    None
                };

                let module = match module_result {
                    Some(m) => m,
                    None => {
                        // Handle bundle extraction.
                        let bytecode = node_client
                            .get_application_bytes(&application_id)
                            .await?
                            .ok_or_else(|| eyre::eyre!("Application bytecode not found"))?;

                        // Recompile the module.
                        let new_module = calimero_runtime::Engine::default().compile(&bytecode)?;

                        // Cache compiled blob for optimizations.
                        // It's ok to ignore errors here as we won't just have an optimization
                        // in case of error.
                        if let Ok(bytes) = new_module.to_bytes() {
                            let compiled_cursor = Cursor::new(bytes);
                            let _ = node_client.add_blob(compiled_cursor, None, None).await;
                        }

                        new_module
                    }
                };

                update_application_with_migration(
                    datastore,
                    node_client,
                    context_client,
                    context_id,
                    context_meta,
                    application_id,
                    Some(application),
                    public_key,
                    migration,
                    module,
                )
                .await
            }
            .await;

            result
        };

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

pub async fn update_application_with_migration(
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

    // If migration is requested, we first execute it locally using the new code (module)
    // against the current storage.
    let mut storage_updates = None;

    if let Some(params) = migration {
        // TODO: show migration params?
        tracing::info!(
            %context_id,
            method = %params.method,
            new_app_id = %application_id,
            "Executing atomic migration"
        );

        // Prepare storage view
        let storage = ContextStorage::from(datastore.clone(), context_id);

        let (execution_result, mut execution_storage) = global_runtime()
            .spawn_blocking(move || {
                let mut execution_storage = storage;

                // Execute using the new module (`application_id`)
                // This allows the new code to read old state via `env::storage_read()`
                let res = module.run(
                    context_id,
                    // Executor is the updater
                    public_key,
                    &params.method,
                    &params.payload,
                    &mut execution_storage,
                    Some(node_client),
                    None,
                );

                (res, execution_storage)
            })
            .await?;

        let outcome = execution_result?;

        if outcome.returns.is_err() {
            bail!("Migration failed with runtime error: {:?}", outcome.returns);
        }

        // If the client provided the flag to write the return value back to the state key
        if params.write_return_to_state_key.is_some() {
            let return_val_opt = outcome
                .returns
                .as_ref()
                .map_err(|e| eyre::eyre!("Migration function failed: {:?}", e))?;

            let new_state_bytes = return_val_opt.clone().ok_or_else(|| {
                eyre::eyre!("Migration function returned None/Void but write-back was requested")
            })?;

            tracing::info!(
                %context_id,
                state_size = new_state_bytes.len(),
                "Writing migration result to root state storage"
            );

            // Ignore the provided user's storage key, use the same storage key
            // that is defined in the `storage` crate.
            let root_key_bytes = calimero_storage::constants::root_storage_key();

            let key_ref = RuntimeKey::from(root_key_bytes.to_vec());
            let value_ref = RuntimeValue::from(new_state_bytes);

            // Inject the write directly into the storage transaction
            execution_storage.set(key_ref, value_ref);
        }

        // Save the storage updates, but don't commit yet.
        storage_updates = Some(execution_storage);
        tracing::info!(%context_id, "Migration execution successful, proceeding to external update");
    }

    // Perform an external context config update.
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
        .await
        .wrap_err("Failed to update application in external config")?;

    // Commit the migration state changes (if any)
    if let Some(storage_to_commit) = storage_updates {
        // This flushes the changes to RocksDB
        let _ = storage_to_commit.commit()?;
        tracing::debug!(%context_id, "Committed migration state changes");
    }

    // Update Context Metadata (Point to new App ID)
    let mut handle = datastore.handle();

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(application.id),
            *context.root_hash,
            context.dag_heads.clone(),
        ),
    )?;

    // Perform the sync
    context_client.sync_context_config(context_id, None).await?;

    Ok(application)
}
