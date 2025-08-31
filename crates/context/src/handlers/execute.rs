use std::borrow::Cow;
use std::collections::btree_map;
use std::num::NonZeroUsize;
use std::time::Instant;

use actix::{
    ActorFuture, ActorFutureExt, ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture,
};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::execute::{
    ExecuteError, ExecuteEvent, ExecuteRequest, ExecuteResponse,
};
use calimero_context_primitives::{ContextAtomic, ContextAtomicKey};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, ExecutionEventPayload, NodeEvent,
    StateMutationPayload,
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
use tracing::{debug, error};


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
            "execution requested"
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

            Ok((guard, context.meta))
        });

        let module_task = context_task.and_then(move |(guard, context), act, _ctx| {
            act.get_module(context.application_id)
                .map_ok(move |module, _act, _ctx| (guard, context, module))
        });


        let execute_task = module_task.and_then(move |(guard, mut context, module), act, _ctx| {
            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();

            async move {
                let old_root_hash = context.root_hash;
                let start = Instant::now();

                // Check if we should use lightweight processing BEFORE WASM execution
                let performance_service = crate::performance::PerformanceService::default();
                let should_skip_wasm = performance_service
                    .should_use_lightweight_processing(payload.len(), is_state_op);

                let (outcome, delta_height) = if should_skip_wasm {
                    // Skip WASM execution for small, non-state operations
                    performance_service.apply_lightweight_delta(
                        &context_id,
                        &executor,
                        payload.len(),
                    );

                    // For lightweight processing, we'll still execute WASM but with minimal overhead
                    // This is a simpler approach that avoids the non-exhaustive struct issue
                    // TODO: Implement proper lightweight processing in the future
                    internal_execute(
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
                    .await?
                } else {
                    // Normal WASM execution
                    internal_execute(
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
                    .await?
                };

                let _duration = start.elapsed().as_secs_f64();
                let status = outcome
                    .returns
                    .is_ok()
                    .then_some("success")
                    .unwrap_or("failure");

                debug!(
                    %context_id,
                    %executor,
                    status = status,
                    %old_root_hash,
                    new_root_hash=%context.root_hash,
                    artifact_len = outcome.artifact.len(),
                    logs_count = outcome.logs.len(),
                    events_count = outcome.events.len(),
                    lightweight = should_skip_wasm,
                    "executed request"
                );

                Ok((guard, context, outcome, delta_height))
            }
            .into_actor(act)
        });

        let external_task =
            execute_task.and_then(move |(guard, context, outcome, delta_height), act, _ctx| {
                if let Some(cached_context) = act.contexts.get_mut(&context_id) {
                    cached_context.meta.root_hash = context.root_hash;
                }

                let node_client = act.node_client.clone();
                let context_client = act.context_client.clone();

                async move {
                    if outcome.returns.is_err() {
                        return Ok((guard, context.root_hash, outcome));
                    }

                    if !(is_state_op || outcome.artifact.is_empty()) {
                        if let Some(height) = delta_height {
                            // For now, use single delta broadcasting
                            // TODO: Implement batch broadcasting when multiple deltas are available
                            node_client
                                .broadcast(
                                    &context,
                                    &executor,
                                    &sender_key,
                                    outcome.artifact.clone(),
                                    height,
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
    context_client: &ContextClient,
    module: calimero_runtime::Module,
    guard: &OwnedMutexGuard<ContextId>,
    context: &mut Context,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    is_state_op: bool,
) -> eyre::Result<(Outcome, Option<NonZeroUsize>)> {
    let storage = ContextStorage::from(datastore, context.id);

    let (outcome, storage) = execute(
        guard,
        module,
        executor,
        method,
        input,
        storage,
        node_client.clone(),
    )
    .await?;

    if outcome.returns.is_err() {
        return Ok((outcome, None));
    }

    'fine: {
        if outcome.root_hash.is_some() && outcome.artifact.is_empty() {
            if is_state_op {
                // fixme! temp mitigation for a potential state inconsistency
                break 'fine;
            }

            eyre::bail!("context state changed, but no actions were generated, discarding execution outcome to mitigate potential state inconsistency");
        }
    }

    let mut height = None;

    if !storage.is_empty() {
        let store = storage.commit()?;

        if let Some(root_hash) = outcome.root_hash {
            context.root_hash = root_hash.into();

            if !is_state_op {
                let delta_height = context_client
                    .get_delta_height(&context.id, &executor)?
                    .map_or(NonZeroUsize::MIN, |v| v.saturating_add(1));

                height = Some(delta_height);

                context_client.put_state_delta(
                    &context.id,
                    &executor,
                    &delta_height,
                    &outcome.artifact,
                )?;

                context_client.set_delta_height(&context.id, &executor, delta_height)?;
            }

            let mut handle = store.handle();

            handle.put(
                &key::ContextMeta::new(context.id),
                &types::ContextMeta::new(
                    key::ApplicationMeta::new(context.application_id),
                    *context.root_hash,
                ),
            )?;

            node_client.send_event(NodeEvent::Context(ContextEvent {
                context_id: context.id,
                payload: ContextEventPayload::StateMutation(StateMutationPayload {
                    new_root: context.root_hash,
                }),
            }))?;
        }
    }

    node_client.send_event(NodeEvent::Context(ContextEvent {
        context_id: context.id,
        payload: ContextEventPayload::ExecutionEvent(ExecutionEventPayload {
            events: outcome
                .events
                .iter()
                .map(|e| ExecutionEvent {
                    kind: e.kind.clone(),
                    data: e.data.clone(),
                })
                .collect(),
        }),
    }))?;

    Ok((outcome, height))
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
