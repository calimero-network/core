use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Extension;
use axum::Json;
use axum::Router;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::NodeEvent;
use calimero_server_primitives::sse::{
    Command, ConnectionId, Request, RequestPayload, Response, ResponseBody, ResponseBodyError,
    ServerResponseError, SseEvent,
};
use core::convert::Infallible;
use core::pin::pin;
use futures_util::stream::{self as stream, Stream};
use rand::random;
use serde::{Deserialize, Serialize};
use serde_json::{to_string as to_json_string, to_value as to_json_value};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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
) -> Option<(&'static str, Router)> {
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
    let router = Router::new()
        .route("/", get(sse_handler))
        .route("/subscription", post(handle_subscription))
        .layer(Extension(state));

    Some((path, router))
}

async fn handle_subscription(
    Extension(state): Extension<Arc<ServiceState>>,
    Json(request): Json<Request<serde_json::Value>>,
) -> impl IntoResponse {
    let request_id = match request.id.parse::<u64>() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Response {
                    body: ResponseBody::Error(ResponseBodyError::HandlerError(
                        "Invalid Connection Id".into(),
                    )),
                }),
            );
        }
    };

    match serde_json::from_value(request.payload) {
        Ok(RequestPayload::Subscribe(ctxs)) => {
            info!(
                "Subscribe: connection_id = {:?}, context_ids = {:?}",
                request_id, ctxs
            );

            let mut connections = state.connections.write().await;

            if let Some(conn) = connections.get_mut(&request_id) {
                let mut inner = conn.inner.write().await;
                for ctx in &ctxs.context_ids {
                    let _ = inner.subscriptions.insert(*ctx);
                }

                (
                    StatusCode::OK,
                    Json(Response {
                        body: ResponseBody::Result(serde_json::json!({
                            "status": "subscribed",
                            "contexts": ctxs.context_ids,
                        })),
                    }),
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(Response {
                        body: ResponseBody::Error(ResponseBodyError::HandlerError(
                            "Invalid Connection Id".into(),
                        )),
                    }),
                )
            }
        }
        Ok(RequestPayload::Unsubscribe(ctxs)) => {
            info!(
                "Unsubscribe: connection_id = {:?}, context_ids = {:?}",
                request_id, ctxs
            );

            let mut connections = state.connections.write().await;
            if let Some(conn) = connections.get_mut(&request_id) {
                let mut inner = conn.inner.write().await;
                let mut invalid = Vec::new();

                for ctx in &ctxs.context_ids {
                    if !inner.subscriptions.remove(ctx) {
                        invalid.push(*ctx);
                    }
                }

                if !invalid.is_empty() {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(Response {
                            body: ResponseBody::Error(ResponseBodyError::HandlerError(
                                "Invalid Context Id".into(),
                            )),
                        }),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(Response {
                            body: ResponseBody::Result(serde_json::json!({
                                "status": "unsubscribed",
                                "contexts": ctxs.context_ids,
                            })),
                        }),
                    )
                }
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(Response {
                        body: ResponseBody::Error(ResponseBodyError::HandlerError(
                            "Invalid Connection Id".into(),
                        )),
                    }),
                )
            }
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(Response {
                body: ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::ParseError(err.to_string()),
                )),
            }),
        ),
    }
}

async fn sse_handler(
    Extension(state): Extension<Arc<ServiceState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (commands_sender, commands_receiver) = mpsc::channel::<Command>(32);

    let (connection_id, _) = loop {
        let connection_id = random();
        let mut connections = state.connections.write().await;
        match connections.entry(connection_id) {
            Entry::Occupied(_) => continue,
            Entry::Vacant(entry) => {
                let inner = Arc::new(RwLock::new(ConnectionStateInner {
                    subscriptions: HashSet::new(),
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

    // converts commands from the nodes to tokio_stream
    let command_stream = ReceiverStream::new(commands_receiver).map(move |command| match command {
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

    // inital connection command
    let initial_event = Event::default()
        .event(SseEvent::Connect.as_str())
        .data(&connection_id.to_string());
    let initial_stream = stream::once(async { Ok(initial_event) });

    let stream = initial_stream.chain(command_stream);
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
