use std::borrow::Cow;
use std::collections::btree_map;
// Removed: NonZeroUsize (replaced with CausalDelta)
use std::time::Instant;

use actix::{
    ActorFuture, ActorFutureExt, ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture,
};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{
    ExecuteError, ExecuteEvent, ExecuteRequest, ExecuteResponse,
};
use calimero_context_primitives::{ContextAtomic, ContextAtomicKey};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
};
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::Outcome;
use calimero_store::{key, types, Store};
use calimero_utils_actix::global_runtime;
use either::Either;
use eyre::{bail, WrapErr};
use futures_util::future::TryFutureExt;
use futures_util::io::Cursor;
use memchr::memmem;
use tokio::sync::OwnedMutexGuard;
use tracing::{debug, error, info, warn};

use crate::metrics::ExecutionLabels;
use crate::ContextManager;

pub mod storage;

use storage::ContextStorage;

impl Handler<ExecuteRequest> for ContextManager {
    type Result = ActorResponse<Self, <ExecuteRequest as Message>::Result>;

    fn handle(
        &mut self,
        ExecuteRequest {
            context: context_id,
            executor,
            method,
            payload,
            aliases,
            atomic,
        }: ExecuteRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        info!(
            %context_id,
            method,
            "Executing method in context"
        );
        debug!(
            %context_id,
            %executor,
            method,
            aliases = ?aliases,
            payload_len = payload.len(),
            atomic = %match atomic {
                None => "no",
                Some(ContextAtomic::Lock) => "acquire",
                Some(ContextAtomic::Held(_)) => "yes",
            },
            "Execution request details"
        );

        let context = match self.get_or_fetch_context(&context_id) {
            Ok(Some(context)) => context,
            Ok(None) => return ActorResponse::reply(Err(ExecuteError::ContextNotFound)),
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        };

        let is_state_op = "__calimero_sync_next" == method;

        if !is_state_op && *context.meta.root_hash == [0; 32] {
            return ActorResponse::reply(Err(ExecuteError::Uninitialized));
        }

        let (guard, is_atomic) = match atomic {
            None => (context.lock(), false),
            Some(ContextAtomic::Lock) => (context.lock(), true),
            Some(ContextAtomic::Held(ContextAtomicKey(guard))) => (Either::Left(guard), true),
        };

        let external_config = match self.context_client.context_config(&context_id) {
            Ok(Some(external_config)) => external_config,
            Ok(None) => {
                error!(%context_id, "missing context config for context");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        };

        let sender_key = match self.context_client.get_identity(&context_id, &executor) {
            Ok(Some(ContextIdentity {
                private_key: Some(_),
                sender_key: Some(sender_key),
                ..
            })) => sender_key,
            Ok(_) => {
                return ActorResponse::reply(Err(ExecuteError::Unauthorized {
                    context_id,
                    public_key: executor,
                }))
            }
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        };

        let payload =
            match substitute_aliases_in_payload(&self.node_client, context_id, payload, &aliases) {
                Ok(payload) => payload,
                Err(err) => {
                    error!(%err, "failed to execute request");

                    return ActorResponse::reply(Err(err));
                }
            };

        let guard_task = async move {
            match guard {
                Either::Left(guard) => guard,
                Either::Right(task) => task.await,
            }
        }
        .into_actor(self);

        let context_task = guard_task.map(move |guard, act, _ctx| {
            let Some(context) = act.get_or_fetch_context(&context_id)? else {
                bail!("context '{context_id}' deleted before we could execute");
            };

            Ok((guard, context.meta.clone()))
        });

        let module_task = context_task.and_then(move |(guard, context), act, _ctx| {
            act.get_module(context.application_id)
                .map_ok(move |module, _act, _ctx| (guard, context, module))
        });

        let execution_count = self.metrics.as_ref().map(|m| m.execution_count.clone());
        let execution_duration = self.metrics.as_ref().map(|m| m.execution_duration.clone());

        let execute_task = module_task.and_then(move |(guard, mut context, module), act, _ctx| {
            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();

            async move {
                let old_root_hash = context.root_hash;

                let start = Instant::now();

                let (outcome, causal_delta) = internal_execute(
                    datastore,
                    &node_client,
                    &context_client,
                    module,
                    &guard,
                    &mut context,
                    executor,
                    method.clone().into(),
                    payload.into(),
                    is_state_op,
                )
                .await?;

                let duration = start.elapsed().as_secs_f64();
                let status = outcome
                    .returns
                    .is_ok()
                    .then_some("success")
                    .unwrap_or("failure");

                // Update execution count metrics
                if let Some(execution_count) = execution_count {
                    let _ignored = execution_count
                        .clone()
                        .get_or_create(&ExecutionLabels {
                            context_id: context_id.to_string(),
                            method: method.clone(),
                            status: status.to_owned(),
                        })
                        .inc();
                }

                // Update execution duration metrics
                if let Some(execution_duration) = execution_duration {
                    let _ignored = execution_duration
                        .clone()
                        .get_or_create(&ExecutionLabels {
                            context_id: context_id.to_string(),
                            method: method.clone(),
                            status: status.to_owned(),
                        })
                        .observe(duration);
                }

                info!(
                    %context_id,
                    method,
                    status,
                    "Method execution completed"
                );
                debug!(
                    %context_id,
                    %executor,
                    method,
                    status,
                    %old_root_hash,
                    new_root_hash=%context.root_hash,
                    artifact_len = outcome.artifact.len(),
                    logs_count = outcome.logs.len(),
                    events_count = outcome.events.len(),
                    xcalls_count = outcome.xcalls.len(),
                    "Execution outcome details"
                );

                Ok((guard, context, outcome, causal_delta))
            }
            .into_actor(act)
        });

        let external_task =
            execute_task.and_then(move |(guard, context, outcome, causal_delta), act, _ctx| {
                if let Some(cached_context) = act.contexts.get_mut(&context_id) {
                    debug!(
                        %context_id,
                        old_root = ?cached_context.meta.root_hash,
                        new_root = ?context.root_hash,
                        is_state_op,
                        "Updating cached context root_hash"
                    );
                    cached_context.meta.root_hash = context.root_hash;
                } else {
                    debug!(%context_id, is_state_op, "Context not in cache, will be fetched from DB next time");
                }

                let node_client = act.node_client.clone();
                let context_client = act.context_client.clone();

                async move {
                    if outcome.returns.is_err() {
                        return Ok((guard, context.root_hash, outcome));
                    }

                    info!(
                        %context_id,
                        %executor,
                        is_state_op,
                        artifact_empty = outcome.artifact.is_empty(),
                        events_count = outcome.events.len(),
                        xcalls_count = outcome.xcalls.len(),
                        "Execution outcome details"
                    );

                    // Log events with handlers for debugging
                    // NOTE: Handlers are NEVER executed on the node that produces the events/diffs.
                    // Handlers are only executed on receiving nodes during network sync to avoid
                    // infinite loops and ensure proper distributed execution.
                    for event in &outcome.events {
                        if let Some(handler_name) = &event.handler {
                            info!(
                                %context_id,
                                event_kind = %event.kind,
                                handler_name = %handler_name,
                                "Event emitted with handler (will be executed on receiving nodes)"
                            );
                        }
                    }

                    // Process cross-context calls
                    // NOTE: XCalls are executed locally on the current node after the main execution completes.
                    // This allows contexts to communicate by calling functions on other contexts.
                    for xcall in &outcome.xcalls {
                        let target_context_id = ContextId::from(xcall.context_id);

                        info!(
                            %context_id,
                            target_context = ?target_context_id,
                            function = %xcall.function,
                            params_len = xcall.params.len(),
                            "Processing cross-context call"
                        );

                        // Find an owned member of the target context to execute as
                        // We need to use a member that has permissions on the target context
                        use futures_util::TryStreamExt;
                        let members: Vec<_> = context_client
                            .get_context_members(&target_context_id, Some(true))
                            .try_collect()
                            .await
                            .unwrap_or_default();

                        let Some((target_executor, _is_owned)) = members.first() else {
                            error!(
                                %context_id,
                                target_context = ?target_context_id,
                                function = %xcall.function,
                                "No owned members found for target context"
                            );
                            continue;
                        };

                        let target_executor = *target_executor;

                        info!(
                            %context_id,
                            target_context = ?target_context_id,
                            target_executor = ?target_executor,
                            "Found owned member for target context"
                        );

                        // Execute the cross-context call with the target context's member
                        let xcall_result = context_client
                            .execute(
                                &target_context_id,
                                &target_executor,
                                xcall.function.clone(),
                                xcall.params.clone(),
                                vec![],
                                None,
                            )
                            .await;

                        match xcall_result {
                            Ok(_) => {
                                info!(
                                    %context_id,
                                    target_context = ?target_context_id,
                                    function = %xcall.function,
                                    "Cross-context call executed successfully"
                                );
                            }
                            Err(err) => {
                                error!(
                                    %context_id,
                                    target_context = ?target_context_id,
                                    function = %xcall.function,
                                    ?err,
                                    "Cross-context call failed"
                                );
                            }
                        }
                    }

                    // Broadcast state deltas to other nodes when:
                    // 1. It's not a state synchronization operation (is_state_op = false)
                    // 2. AND there's a state change artifact (non-empty artifact)
                    //
                    // This ensures that:
                    // - State changes are broadcast when there are actual state changes
                    // - State synchronization operations don't trigger broadcasts (prevents loops)
                    // - Events are still broadcast via WebSocket regardless of state changes
                    if !(is_state_op || outcome.artifact.is_empty()) {
                        info!(
                            %context_id,
                            %executor,
                            is_state_op,
                            artifact_empty = outcome.artifact.is_empty(),
                            events_count = outcome.events.len(),
                            has_delta = causal_delta.is_some(),
                            "Broadcasting state delta and events to other nodes"
                        );

                        if let Some(ref the_delta) = causal_delta {
                            // Serialize events if any were emitted
                            let events_data = if outcome.events.is_empty() {
                                info!(
                                    %context_id,
                                    %executor,
                                    "No events to serialize"
                                );
                                None
                            } else {
                                let events_vec: Vec<ExecutionEvent> = outcome
                                    .events
                                    .iter()
                                    .map(|e| ExecutionEvent {
                                        kind: e.kind.clone(),
                                        data: e.data.clone(),
                                        handler: e.handler.clone(),
                                    })
                                    .collect();
                                let serialized = serde_json::to_vec(&events_vec)?;
                                info!(
                                    %context_id,
                                    %executor,
                                    events_count = events_vec.len(),
                                    serialized_len = serialized.len(),
                                    "Serializing events for broadcast"
                                );
                                Some(serialized)
                            };

                            node_client
                                .broadcast(
                                    &context,
                                    &executor,
                                    &sender_key,
                                    outcome.artifact.clone(),
                                    the_delta.id,
                                    the_delta.parents.clone(),
                                    events_data,
                                )
                                .await?;
                        }
                    }

                    let external_client =
                        context_client.external_client(&context_id, &external_config)?;

                    let proxy_client = external_client.proxy();

                    for (proposal_id, actions) in &outcome.proposals {
                        let actions = borsh::from_slice(actions)?;

                        let proposal_id = proposal_id.rt().expect("infallible conversion");

                        proxy_client
                            .propose(&executor, &proposal_id, actions)
                            .await?;
                    }

                    for proposal_id in &outcome.approvals {
                        let proposal_id = proposal_id.rt().expect("infallible conversion");

                        proxy_client.approve(&executor, &proposal_id).await?;
                    }

                    Ok((guard, context.root_hash, outcome))
                }
                .map_err(|err| {
                    error!(
                    ?err,
                    "execution succeeded, but an error occurred while performing external actions"
                );

                    err
                })
                .into_actor(act)
            });

        let task = external_task
            .map_err(|err, _act, _ctx| {
                err.downcast::<ExecuteError>().unwrap_or_else(|err| {
                    debug!(?err, "an error occurred while executing request");
                    ExecuteError::InternalError
                })
            })
            .map_ok(
                move |(guard, root_hash, outcome), _act, _ctx| ExecuteResponse {
                    returns: outcome.returns.map_err(Into::into),
                    logs: outcome.logs,
                    events: outcome
                        .events
                        .into_iter()
                        .map(|e| ExecuteEvent {
                            kind: e.kind,
                            data: e.data,
                            handler: e.handler,
                        })
                        .collect(),
                    root_hash,
                    artifact: outcome.artifact,
                    atomic: is_atomic.then_some(ContextAtomicKey(guard)),
                },
            );

        ActorResponse::r#async(task)
    }
}

impl ContextManager {
    pub fn get_module(
        &self,
        application_id: ApplicationId,
    ) -> impl ActorFuture<Self, Output = eyre::Result<calimero_runtime::Module>> + 'static {
        let blob_task = async {}.into_actor(self).map(move |_, act, _ctx| {
            let blob = match act.applications.entry(application_id) {
                btree_map::Entry::Vacant(vacant) => {
                    let Some(app) = act.node_client.get_application(&application_id)? else {
                        bail!(ExecuteError::ApplicationNotInstalled { application_id });
                    };

                    vacant.insert(app).blob
                }
                btree_map::Entry::Occupied(occupied) => occupied.into_mut().blob,
            };

            Ok(blob)
        });

        let module_task = blob_task.and_then(move |mut blob, act, _ctx| {
            let node_client = act.node_client.clone();

            async move {
                if let Some(compiled) = node_client.get_blob_bytes(&blob.compiled, None).await? {
                    let module =
                        unsafe { calimero_runtime::Engine::headless().from_precompiled(&compiled) };

                    match module {
                        Ok(module) => return Ok((module, None)),
                        Err(err) => {
                            debug!(
                                ?err,
                                %application_id,
                                blob_id=%blob.compiled,
                                "failed to load precompiled module, recompiling.."
                            );
                        }
                    }
                }

                debug!(
                    %application_id,
                    blob_id=%blob.compiled,
                    "no usable precompiled module found, compiling.."
                );

                let Some(bytecode) = node_client.get_blob_bytes(&blob.bytecode, None).await? else {
                    bail!(ExecuteError::ApplicationNotInstalled { application_id });
                };

                let module = calimero_runtime::Engine::default().compile(&bytecode)?;

                let compiled = Cursor::new(module.to_bytes()?);

                let (blob_id, _ignored) = node_client.add_blob(compiled, None, None).await?;

                blob.compiled = blob_id;

                node_client.update_compiled_app(&application_id, &blob_id)?;

                Ok((module, Some(blob)))
            }
            .into_actor(act)
        });

        module_task
            .map_ok(move |(module, blob), act, _ctx| {
                if let Some(blob) = blob {
                    if let Some(app) = act.applications.get_mut(&application_id) {
                        app.blob = blob;
                    }
                }

                module
            })
            .map_err(|err, _act, _ctx| {
                error!(?err, "failed to initialize module for execution");

                err
            })
    }
}

async fn internal_execute(
    datastore: Store,
    node_client: &NodeClient,
    _context_client: &ContextClient,
    module: calimero_runtime::Module,
    guard: &OwnedMutexGuard<ContextId>,
    context: &mut Context,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    is_state_op: bool,
) -> eyre::Result<(Outcome, Option<calimero_storage::delta::CausalDelta>)> {
    let storage = ContextStorage::from(datastore, context.id);

    let (outcome, storage) = execute(
        guard,
        module,
        executor,
        method.clone(),
        input,
        storage,
        node_client.clone(),
    )
    .await?;

    debug!(
        context_id = %context.id,
        method = %method,
        is_state_op,
        has_root_hash = outcome.root_hash.is_some(),
        artifact_len = outcome.artifact.len(),
        events_count = outcome.events.len(),
        returns_ok = outcome.returns.is_ok(),
        "WASM execution completed"
    );

    if outcome.returns.is_err() {
        warn!(
            context_id = %context.id,
            method = %method,
            error = ?outcome.returns,
            "WASM execution returned error"
        );
        return Ok((outcome, None));
    }

    'fine: {
        if outcome.root_hash.is_some() && outcome.artifact.is_empty() {
            debug!(
                context_id = %context.id,
                has_root_hash = true,
                artifact_empty = true,
                is_state_op,
                "Outcome has root hash but empty artifact - checking mitigation"
            );

            if is_state_op {
                // fixme! temp mitigation for a potential state inconsistency
                break 'fine;
            }

            eyre::bail!("context state changed, but no actions were generated, discarding execution outcome to mitigate potential state inconsistency");
        }
    }

    let mut causal_delta = None;

    // Always update root_hash if present (even if storage is empty)
    // This is critical for state_ops like __calimero_sync_next where actions
    // are applied inside WASM but storage appears empty
    if let Some(root_hash) = outcome.root_hash {
        debug!(
            context_id = %context.id,
            old_root = ?context.root_hash,
            new_root = ?root_hash,
            is_state_op,
            storage_empty = storage.is_empty(),
            "Updating context root_hash after execution"
        );
        context.root_hash = root_hash.into();

        // Commit storage and persist metadata
        let store = storage.commit()?;

        // Create causal delta for non-state ops with non-empty artifacts
        if !is_state_op && !outcome.artifact.is_empty() {
            // Create causal delta with DAG metadata
            use calimero_storage::delta::{CausalDelta, StorageDelta};

            // Extract actions from artifact for DAG persistence
            let actions = match borsh::from_slice::<StorageDelta>(&outcome.artifact) {
                Ok(StorageDelta::Actions(actions)) => actions,
                Ok(_) => {
                    warn!("Unexpected StorageDelta variant, using empty actions");
                    vec![]
                }
                Err(e) => {
                    warn!(
                        ?e,
                        "Failed to deserialize artifact for DAG, using empty actions"
                    );
                    vec![]
                }
            };

            // Use current DAG heads as parents for this new delta
            let parents = if context.dag_heads.is_empty() {
                // Genesis case: parent is the zero hash
                vec![[0u8; 32]]
            } else {
                // Normal case: parents are current DAG heads
                context.dag_heads.clone()
            };

            let timestamp = calimero_storage::env::time_now();
            let delta_id = CausalDelta::compute_id(&parents, &actions, timestamp);

            let delta = CausalDelta {
                id: delta_id, // Computed from delta content (parents + actions + timestamp)
                parents,
                actions, // ✅ Real actions extracted from artifact!
                timestamp,
            };

            // Update context's DAG heads to this new delta
            context.dag_heads = vec![delta.id];

            causal_delta = Some(delta);
        } else if !is_state_op {
            // No delta created (empty artifact), but state changed
            // Use root_hash as dag_head fallback to enable sync
            // This happens when init() creates state but doesn't generate actions
            if context.dag_heads.is_empty() {
                warn!(
                    context_id = %context.id,
                    root_hash = ?root_hash,
                    artifact_empty = outcome.artifact.is_empty(),
                    "State changed but no delta created - using root_hash as dag_head fallback"
                );
                context.dag_heads = vec![root_hash];
            }
        }

        // CRITICAL: Always persist context metadata when root_hash changes
        // This ensures receiving nodes update their state after applying deltas
        let mut handle = store.handle();

        debug!(
            context_id = %context.id,
            root_hash = ?context.root_hash,
            dag_heads_count = context.dag_heads.len(),
            is_state_op,
            "Persisting context metadata to database"
        );

        handle.put(
            &key::ContextMeta::new(context.id),
            &types::ContextMeta::new(
                key::ApplicationMeta::new(context.application_id),
                *context.root_hash,
                context.dag_heads.clone(),
            ),
        )?;

        // Also persist the delta itself for serving to peers who request it
        if let Some(ref delta) = causal_delta {
            let serialized_actions = borsh::to_vec(&delta.actions)?;

            handle.put(
                &key::ContextDagDelta::new(context.id, delta.id),
                &types::ContextDagDelta {
                    delta_id: delta.id,
                    parents: delta.parents.clone(),
                    actions: serialized_actions,
                    timestamp: delta.timestamp,
                    applied: true,
                },
            )?;

            debug!(
                context_id = %context.id,
                delta_id = ?delta.id,
                "Persisted delta to database for future requests"
            );
        }

        debug!(
            context_id = %context.id,
            root_hash = ?context.root_hash,
            dag_heads_count = context.dag_heads.len(),
            is_state_op,
            "Context metadata persisted successfully"
        );
    }

    // Emit state mutation to WebSocket clients (frontends) if there are events or state changes
    // Note: This is separate from node-to-node DAG broadcast (lines 408-419)
    if !outcome.events.is_empty() || outcome.root_hash.is_some() {
        let new_root = outcome
            .root_hash
            .map(|h| h.into())
            .unwrap_or((*context.root_hash).into());

        let events_vec = outcome
            .events
            .iter()
            .map(|e| ExecutionEvent {
                kind: e.kind.clone(),
                data: e.data.clone(),
                handler: e.handler.clone(),
            })
            .collect();

        node_client.send_event(NodeEvent::Context(ContextEvent {
            context_id: context.id,
            payload: ContextEventPayload::StateMutation(
                StateMutationPayload::with_root_and_events(new_root, events_vec),
            ),
        }))?;
    }

    Ok((outcome, causal_delta))
}

pub async fn execute(
    context: &OwnedMutexGuard<ContextId>,
    module: calimero_runtime::Module,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    mut storage: ContextStorage,
    node_client: NodeClient,
) -> eyre::Result<(Outcome, ContextStorage)> {
    let context_id = **context;

    // Run WASM execution in blocking context
    global_runtime()
        .spawn_blocking(move || {
            let outcome = module.run(
                context_id,
                executor,
                &method,
                &input,
                &mut storage,
                Some(node_client),
            )?;
            Ok((outcome, storage))
        })
        .await
        .wrap_err("failed to receive execution response")?
}

fn substitute_aliases_in_payload(
    node_client: &NodeClient,
    context_id: ContextId,
    payload: Vec<u8>,
    aliases: &[Alias<PublicKey>],
) -> Result<Vec<u8>, ExecuteError> {
    if aliases.is_empty() {
        return Ok(payload);
    }

    // todo! evaluate a byte-version of calimero_server[build]::replace
    // todo! ref: https://github.com/calimero-network/core/blob/6deb2db81a65e0b5c86af9fe2950cf9019ab61af/crates/server/build.rs#L139-L175

    let mut result = Vec::with_capacity(payload.len());
    let mut remaining = &payload[..];

    for alias in aliases {
        let needle_str = format!("{{{alias}}}");
        let needle = needle_str.into_bytes();

        while let Some(pos) = memmem::find(remaining, &needle) {
            result.extend_from_slice(&remaining[..pos]);

            let public_key = node_client
                .resolve_alias(*alias, Some(context_id))
                .map_err(|_| ExecuteError::InternalError)?
                .ok_or_else(|| ExecuteError::AliasResolutionFailed { alias: *alias })?;

            result.extend_from_slice(public_key.as_str().as_bytes());

            remaining = &remaining[pos + needle.len()..];
        }
    }

    result.extend_from_slice(remaining);

    Ok(result)
}
