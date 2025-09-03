use crate::client::{CallClient, Environment};

pub mod mutate;
pub mod query;
pub mod requests;
pub mod types;
use mutate::ContextConfigMutate;
use query::ContextConfigQuery;

#[derive(Copy, Clone, Debug)]
pub enum ContextConfig {}

impl<'a, T: 'a> Environment<'a, T> for ContextConfig {
    type Query = ContextConfigQuery<'a, T>;
    type Mutate = ContextConfigMutate<'a, T>;

    fn query(client: CallClient<'a, T>) -> Self::Query {
        ContextConfigQuery { client }
    }

    fn mutate(client: CallClient<'a, T>) -> Self::Mutate {
        ContextConfigMutate { client }
    }
}
