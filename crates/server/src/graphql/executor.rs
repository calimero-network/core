use std::future::Future;
use std::pin::Pin;

use tracing::info;

pub type BoxStream<'a, T> = Pin<Box<dyn futures_util::Stream<Item = T> + Send + 'a>>;

#[derive(Clone)]
pub struct GraphQLExecutor;

impl async_graphql::Executor for GraphQLExecutor {
    fn execute(
        &self,
        request: async_graphql::Request,
    ) -> impl Future<Output = async_graphql::Response> + Send {
        async {
            info!("Received query: {:?}", request);

            let mut request = request;
            let mut errors = vec![];

            let parsed_query = match request.parsed_query() {
                Ok(parsed_query) => parsed_query,
                Err(e) => {
                    errors.push(e);
                    return async_graphql::Response::from_errors(errors);
                }
            };

            info!("Executing query: {:#?}", parsed_query);

            todo!()
        }
    }

    fn execute_stream(
        &self,
        request: async_graphql::Request,
        session_data: Option<std::sync::Arc<async_graphql::Data>>,
    ) -> BoxStream<'static, async_graphql::Response> {
        info!("Executing stream: {:?}", request);
        todo!()
    }
}
