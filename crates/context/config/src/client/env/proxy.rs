use crate::client::{CallClient, Environment};

mod mutate;
mod query;
mod types;
use mutate::ContextProxyMutate;
use query::ContextProxyQuery;
pub use types::*;

#[derive(Copy, Clone, Debug)]
pub enum ContextProxy {}

impl<'a, T: 'a> Environment<'a, T> for ContextProxy {
    type Query = ContextProxyQuery<'a, T>;
    type Mutate = ContextProxyMutate<'a, T>;

    fn query(client: CallClient<'a, T>) -> Self::Query {
        ContextProxyQuery { client }
    }

    fn mutate(client: CallClient<'a, T>) -> Self::Mutate {
        ContextProxyMutate { client }
    }
}
