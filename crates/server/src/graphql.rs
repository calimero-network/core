use axum::http;
use axum::response::Html;
use axum::routing::{get, MethodRouter};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tower_http::cors;
use tracing::info;

mod model;

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphQLConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub fn service(
    config: &crate::config::ServerConfig,
    sender: crate::Sender,
) -> eyre::Result<Option<(&'static str, MethodRouter)>> {
    let _config = match &config.graphql {
        Some(config) if config.enabled => config,
        _ => {
            info!("GraphQL is disabled");
            return Ok(None);
        }
    };

    let path = "/graphql"; // todo! source from config

    for listen in config.listen.iter() {
        info!("GraphQL server listening on {}/http{{{}}}", listen, path);
    }

    let graphql = async_graphql_axum::GraphQL::new(async_graphql::Schema::new(
        model::AppQuery {
            sender: sender.clone(),
        },
        model::AppMutation { sender },
        async_graphql::EmptySubscription,
    ));

    Ok(Some((
        path,
        get(|| graphiql(path)).post_service(graphql).layer(
            cors::CorsLayer::new()
                .allow_origin(cors::Any)
                .allow_headers(cors::Any)
                .allow_methods([http::Method::POST]),
        ),
    )))
}

async fn graphiql(path: &str) -> Html<String> {
    Html(
        async_graphql::http::GraphiQLSource::build()
            .endpoint(path)
            .finish(),
    )
}

async fn _call<T>(
    sender: &crate::Sender,
    method: String,
    args: Vec<u8>,
    writes: bool,
) -> Result<T, async_graphql::Error>
where
    T: for<'de> Deserialize<'de>,
{
    let (tx, rx) = oneshot::channel();

    sender.send((method, args, writes, tx)).await?;

    let outcome = rx.await?;

    for log in outcome.logs {
        info!("RPC log: {}", log);
    }

    let result = serde_json::from_slice(&outcome.returns?.unwrap_or_default())?;

    Ok(result)
}

async fn call<T>(
    sender: &crate::Sender,
    method: String,
    args: Vec<u8>,
) -> Result<T, async_graphql::Error>
where
    T: for<'de> Deserialize<'de>,
{
    _call(sender, method, args, false).await
}

async fn call_mut<T>(
    sender: &crate::Sender,
    method: String,
    args: Vec<u8>,
) -> Result<T, async_graphql::Error>
where
    T: for<'de> Deserialize<'de>,
{
    _call(sender, method, args, true).await
}
