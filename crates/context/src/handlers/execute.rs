use std::borrow::Cow;

use actix::{ActorFutureExt, ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::messages::execute::{
    ExecuteError, ExecuteEvent, ExecuteRequest, ExecuteResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
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
use memchr::memmem;
use tokio::sync::OwnedMutexGuard;
use tracing::error;

use crate::{ContextManager, ContextMeta};

pub mod storage;

use storage::ContextStorage;

impl Handler<ExecuteRequest> for ContextManager {
    type Result = ActorResponse<Self, <ExecuteRequest as Message>::Result>;

    fn handle(
        &mut self,
        ExecuteRequest {
            context: context_id,
            method,
            payload,
            executor,
            aliases,
        }: ExecuteRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let context = match self.get_or_fetch_context(&context_id) {
            Ok(Some(context)) => context,
            Ok(None) => return ActorResponse::reply(Err(ExecuteError::ContextNotFound)),
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        };

        if !["init", "__calimero_sync_next"].contains(&&*method)
            && *context.meta.root_hash == [0; 32]
        {
            return ActorResponse::reply(Err(ExecuteError::Uninitialized));
        }

        let guard = context.lock();

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

        let guard_fut = async {
            match guard {
                Either::Left(guard) => guard,
                Either::Right(task) => task.await,
            }
        };

        ActorResponse::r#async(guard_fut.into_actor(self).then(move |guard, act, _ctx| {
            let context = act.get_or_fetch_context(&context_id).map(|c| c.cloned());

            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();
            let engine = act.runtime_engine.clone();

            async move {
                let Some(mut context) = context? else {
                    bail!("context '{context_id}' deleted before we could execute");
                };

                let outcome = internal_execute(
                    datastore,
                    &node_client,
                    engine,
                    &mut context,
                    &guard,
                    method.into(),
                    payload.into(),
                    executor,
                )
                .await?;

                if outcome.returns.is_err() {
                    return Ok(outcome);
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

                node_client
                    .broadcast(
                        &context.meta,
                        &executor,
                        &sender_key,
                        outcome.artifact.clone(),
                    )
                    .await?;

                Ok(outcome)
            }
            .map_ok(|outcome| ExecuteResponse {
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
                root_hash: outcome.root_hash.map(Into::into),
                artifact: outcome.artifact,
            })
            .map_err(|err| {
                error!(?err, "failed to execute request");

                err.downcast::<ExecuteError>()
                    .unwrap_or_else(|_| ExecuteError::InternalError)
            })
            .into_actor(act)
        }))
    }
}

async fn internal_execute(
    datastore: Store,
    node_client: &NodeClient,
    engine: calimero_runtime::Engine,
    context: &mut ContextMeta,
    guard: &OwnedMutexGuard<ContextId>,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    executor: PublicKey,
) -> eyre::Result<Outcome> {
    let blob = node_client.get_blob_bytes(&context.blob).await?;

    let Some(blob) = blob else {
        bail!(ExecuteError::ApplicationNotInstalled {
            application_id: context.meta.application_id
        });
    };

    let storage = ContextStorage::from(datastore, context.meta.id);

    let module = engine.compile(&blob)?;

    let is_sync_operation = method == "__calimero_sync_next";

    let (outcome, storage) = execute(guard, module, method, input, executor, storage).await?;

    if outcome.returns.is_ok() {
        if outcome.root_hash.is_some() && outcome.artifact.is_empty() && !is_sync_operation {
            eyre::bail!("context state changed, but no actions were generated, discarding execution outcome to mitigate potential state inconsistency");
        }

        if !storage.is_empty() {
            let store = storage.commit()?;

            if let Some(root_hash) = outcome.root_hash {
                context.meta.root_hash = root_hash.into();

                let mut handle = store.handle();

                handle.put(
                    &key::ContextMeta::new(context.meta.id),
                    &types::ContextMeta::new(
                        key::ApplicationMeta::new(context.meta.application_id),
                        *context.meta.root_hash,
                    ),
                )?;

                node_client.send_event(NodeEvent::Context(ContextEvent {
                    context_id: context.meta.id,
                    payload: ContextEventPayload::StateMutation(StateMutationPayload {
                        new_root: context.meta.root_hash,
                    }),
                }))?;
            }
        }

        node_client.send_event(NodeEvent::Context(ContextEvent {
            context_id: context.meta.id,
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

    Ok(outcome)
}

pub async fn execute(
    context: &OwnedMutexGuard<ContextId>,
    module: calimero_runtime::Module,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    executor: PublicKey,
    mut storage: ContextStorage,
) -> eyre::Result<(Outcome, ContextStorage)> {
    let context_id = **context;

    global_runtime()
        .spawn_blocking(move || {
            let outcome = module.run(context_id, &method, &input, executor, &mut storage)?;

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

    // todo! evaluate a byte-version of calimero_server{-build}::replace
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
