use std::borrow::Cow;

use actix::{ActorFutureExt, ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::messages::execute::{
    ExecuteError, ExecuteEvent, ExecuteRequest, ExecuteResponse,
};
use calimero_context_primitives::{ContextAtomic, ContextAtomicKey};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, ExecutionEventPayload, NodeEvent,
    StateMutationPayload,
};
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::Outcome;
use calimero_store::{key, types};
use calimero_utils_actix::global_runtime;
use either::Either;
use eyre::{bail, WrapErr};
use futures_util::future::TryFutureExt;
use memchr::memmem;
use tokio::sync::OwnedMutexGuard;
use tracing::{debug, error};

use crate::{get_module, ContextManager};

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
            context = %context_id,
            executor = %executor,
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

        let is_state_op = ["init", "__calimero_sync_next"].contains(&&*method);

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

        let prepare_task = guard_task.then(move |guard, act, _ctx| {
            let context_result = act
                .get_or_fetch_context(&context_id)
                .map(|c| c.map(|c| (c.meta, c.bytecode, c.compiled)));

            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let runtime_engine = act.runtime_engine.clone();
            let modules = act.modules.clone();

            async move {
                let Some((context, bytecode, _compiled_blob_id)) = context_result? else {
                    bail!("context '{context_id}' deleted before we could execute");
                };

                let module = get_module(&node_client, &runtime_engine, &modules, &bytecode).await?;

                Ok((guard, context, module, datastore))
            }
            .map_err(|err| {
                error!(?err, "failed to prepare execution");
                err
            })
            .into_actor(act)
        });

        let execute_task = prepare_task.then(move |prepared_result, act, _ctx| {
            let node_client = act.node_client.clone();
            async move {
                let (guard, mut context, module, datastore) = prepared_result?;

                let old_root_hash = context.root_hash;

                let method_clone = method.clone();
                let (outcome, storage) = execute(&guard, executor, module, method.into(), payload.into(), ContextStorage::from(datastore, context.id)).await?;

                if outcome.returns.is_ok() {
                    if outcome.root_hash.is_some() && outcome.artifact.is_empty() {
                        if is_state_op {
                            debug!(
                                %context_id,
                                %method_clone,
                                old_root_hash = ?old_root_hash,
                                new_root_hash = ?outcome.root_hash.unwrap(),
                                "State operation completed - state change without artifact is expected for internal operations"
                            );
                        } else {
                            eyre::bail!("context state changed, but no actions were generated, discarding execution outcome to mitigate potential state inconsistency");
                        }
                    }

                    if !storage.is_empty() {
                        let store = storage.commit()?;

                        if let Some(root_hash) = outcome.root_hash {
                            context.root_hash = root_hash.into();

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
                }

                debug!(
                    %context_id,
                    %executor,
                    status = outcome.returns.is_ok().then_some("success").unwrap_or("failure"),
                    %old_root_hash,
                    new_root_hash=%context.root_hash,
                    artifact_len = outcome.artifact.len(),
                    logs_count = outcome.logs.len(),
                    events_count = outcome.events.len(),
                    "executed request"
                );

                Ok((guard, context, outcome))
            }
            .map_err(|err| {
                error!(?err, "failed to execute request");
                err
            })
            .into_actor(act)
        });

        let external_task = execute_task.and_then(move |(guard, context, outcome), act, _ctx| {
            if let Some(cached_context) = act.contexts.get_mut(&context_id) {
                cached_context.meta.root_hash = context.root_hash;
            }

            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();

            async move {
                if outcome.returns.is_err() {
                    return Ok((guard, context.root_hash, outcome));
                }

                if !is_state_op {
                    node_client
                        .broadcast(&context, &executor, &sender_key, outcome.artifact.clone())
                        .await?;
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
                err.downcast::<ExecuteError>()
                    .unwrap_or_else(|_| ExecuteError::InternalError)
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

pub async fn execute(
    context: &OwnedMutexGuard<ContextId>,
    executor: PublicKey,
    module: calimero_runtime::Module,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    mut storage: ContextStorage,
) -> eyre::Result<(Outcome, ContextStorage)> {
    let context_id = **context;

    global_runtime()
        .spawn_blocking(move || {
            let outcome = module.run(context_id, executor, &method, &input, &mut storage)?;

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
