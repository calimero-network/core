use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::info;

pub mod executor;
mod model;

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphQLConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub fn service(
    config: &crate::config::ServerConfig,
    sender: crate::Sender,
) -> eyre::Result<
    Option<(
        &'static str,
        async_graphql_axum::GraphQL<
            /* executor::GraphQLExecutor */
            async_graphql::Schema<
                model::GQLAppQuery,
                model::GQLAppMutation,
                async_graphql::EmptySubscription,
            >,
        >,
    )>,
> {
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

    Ok(Some((
        path,
        async_graphql_axum::GraphQL::new(
            /* executor::GraphQLExecutor */
            async_graphql::Schema::new(
                model::GQLAppQuery {
                    sender: sender.clone(),
                },
                model::GQLAppMutation { sender },
                async_graphql::EmptySubscription,
            ),
        ),
    )))
}

pub async fn call<T>(
    sender: &crate::Sender,
    method: String,
    args: Vec<u8>,
) -> Result<T, async_graphql::Error>
where
    T: for<'de> Deserialize<'de>,
{
    let (tx, rx) = oneshot::channel();

    sender.send((method, args, tx)).await?;

    let outcome = rx.await?;

    for log in outcome.logs {
        info!("{}", log);
    }

    let result = serde_json::from_slice(&outcome.returns?.unwrap_or_default())?;

    Ok(result)
}
