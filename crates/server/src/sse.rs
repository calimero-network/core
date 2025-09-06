use core::convert::Infallible;
use core::pin::pin;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, MethodRouter};
use axum::Extension;
use axum_extra::extract::{Query, QueryRejection};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::NodeEvent;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Response, ResponseBody, ResponseBodyError, ServerResponseError,
    SseEvent, SubscribeRequest,
};
use futures_util::stream::Stream;
use rand::random;
use serde::{Deserialize, Serialize};
use serde_json::{to_string as to_json_string, to_value as to_json_value};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tracing::{debug, error, info};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SseConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

impl SseConfig {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionStateInner {
    subscriptions: HashSet<ContextId>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectionState {
    _commands: mpsc::Sender<Command>,
    inner: Arc<RwLock<ConnectionStateInner>>,
}

pub(crate) struct ServiceState {
    node_client: NodeClient,
    connections: RwLock<HashMap<ConnectionId, ConnectionState>>,
}

pub(crate) fn service(
    config: &ServerConfig,
    node_client: NodeClient,
) -> Option<(&'static str, MethodRouter)> {
    let _ = match &config.sse {
        Some(config) if config.enabled => config,
        _ => {
            info!("Sse server is disabled");
            return None;
        }
    };

    let path = "/sse";

    for listen in &config.listen {
        info!("Sse server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState {
        node_client,
        connections: RwLock::default(),
    });

    Some((path, get(sse_handler).layer(Extension(state))))
}

async fn sse_handler(
    query: Result<Query<SubscribeRequest>, QueryRejection>,
    Extension(state): Extension<Arc<ServiceState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (commands_sender, commands_receiver) = mpsc::channel::<Command>(32);
    
    match query {
        Ok(Query(q)) => {
            let (connection_id, _) = loop {
                let connection_id = random();
                let context_ids: HashSet<ContextId> = q.context_id.clone().into_iter().collect();
                let mut connections = state.connections.write().await;
                match connections.entry(connection_id) {
                    Entry::Occupied(_) => continue,
                    Entry::Vacant(entry) => {
                        let inner = Arc::new(RwLock::new(ConnectionStateInner {
                            subscriptions: context_ids,
                        }));
                        let connection_state = ConnectionState {
                            _commands: commands_sender.clone(),
                            inner,
                        };
                        let _ = entry.insert(connection_state.clone());
                        break (connection_id, connection_state);
                    }
                }
            };
            
            debug!(%connection_id, "Client connection established");
            drop(tokio::spawn(handle_node_events(
                connection_id,
                Arc::clone(&state),
                commands_sender.clone(),
            )));

            drop(tokio::spawn(handle_connection_cleanups(
                connection_id,
                Arc::clone(&state),
                commands_sender.clone(),
            )));
        }
        Err(e) => {
            drop(commands_sender.send(Command::Close(e.to_string())).await);
            drop(commands_sender)
        }
    }

    // converts commands from the nodes to tokio_stream
    let stream = ReceiverStream::new(commands_receiver).map(move |command| match command {
        Command::Close(reason) => Ok(Event::default()
            .event(SseEvent::Close.as_str())
            .data(reason)),
        Command::Send(response) => match to_json_string(&response) {
            Ok(message) => Ok(Event::default()
                .event(SseEvent::Message.as_str())
                .data(message)),
            Err(err) => {
                error!("Failed to serialize SseResponse: {}", err);
                Ok(Event::default()
                    .event(SseEvent::Error.as_str())
                    .data("Failed to serialize SseResponse"))
            }
        },
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn handle_node_events(
    connection_id: ConnectionId,
    state: Arc<ServiceState>,
    command_sender: mpsc::Sender<Command>,
) {
    let events = state.node_client.receive_events();

    let mut events = pin!(events);

    while let Some(event) = events.next().await {
        let Some(connection_state) = state.connections.read().await.get(&connection_id).cloned()
        else {
            error!(%connection_id, "Unexpected state, client_id not found in client state map");
            return;
        };

        debug!(
            %connection_id,
            "Received node event: {:?}, subscriptions state: {:?}",
            event,
            connection_state.inner.read().await.subscriptions
        );

        let event = match event {
            NodeEvent::Context(event) 
                if {
                    connection_state
                        .inner
                        .read()
                        .await
                        .subscriptions
                        .contains(&event.context_id)
                } =>
            {
                NodeEvent::Context(event)
            }
            NodeEvent::Context(_) => continue,
        };

        let body = match to_json_value(event) {
            Ok(v) => ResponseBody::Result(v),
            Err(err) => ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::InternalError {
                    err: Some(err.into()),
                },
            )),
        };

        let response = Response { body };

        if let Err(err) = command_sender.send(Command::Send(response)).await {
            error!(
                %connection_id,
                %err,
                "Failed to send SseCommand::Send",
            );
        };
    }
}

async fn handle_connection_cleanups(
    connection_id: ConnectionId,
    state: Arc<ServiceState>,
    command_sender: mpsc::Sender<Command>,
) {
    command_sender.closed().await;
    drop(state.connections.write().await.remove(&connection_id));
    debug!(%connection_id, "Cleaned up closed SSE connection");
}

use crate::config::ServerConfig;
