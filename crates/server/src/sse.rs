use axum::routing::get;
use std::collections::hash_map::Entry;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::MethodRouter;
use axum::Extension;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::ws::{Command, ConnectionId};
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::{time::Duration, convert::Infallible};
use tokio::sync::{mpsc, RwLock};
use tracing::info;
use rand::random;
use tokio_stream::StreamExt;
use tracing::debug;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SseConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
    pub path: &'static str,
}

impl SseConfig {
    #[must_use]
    pub const fn new(enabled: bool, path: &'static str) -> Self {
        Self { enabled, path }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionStateInner {
    subscriptions: HashSet<ContextId>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectionState {
    commands: mpsc::Sender<Command>,
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
    let sse_config = match &config.sse {
        Some(config) if config.enabled => config,
        _ => {
            info!("Sse server is disabled");
            return None;
        }
    };

    let path = sse_config.path;

    for listen in &config.listen {
        info!("Sse server listening on {}/{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState {
        node_client,
        connections: RwLock::default(),
    });

    Some((path, get(sse_handler).layer(Extension(state))))
}

async fn sse_handler(Extension(state): Extension<Arc<ServiceState>>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (commands_sender, commands_receiver) = mpsc::channel(32);
    
    let (connection_id, _) = loop {
        let connection_id = random();
        let mut connections = state.connections.write().await;

        match connections.entry(connection_id) {
            Entry::Occupied(_) => continue,
            Entry::Vacant(entry) => {
                let connection_state = ConnectionState {
                    commands: commands_sender.clone(),
                    inner: Arc::default(),
                };
                let _ = entry.insert(connection_state.clone());
                break (connection_id, connection_state);
            }
        }
    };

    debug!(%connection_id, "Client connection established");

    let stream = stream::repeat_with(|| Event::default().data("hi!"))
        .map(Ok)
        .throttle(Duration::from_secs(1));

    Sse::new(stream).keep_alive(KeepAlive::default())
}
use crate::config::ServerConfig;
