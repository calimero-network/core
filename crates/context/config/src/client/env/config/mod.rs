

use crate::client::transport::Transport;
use crate::client::{CallClient, Environment};

/// Context configuration environment that implements the Environment trait
#[derive(Debug, Clone, Copy)]
pub struct ContextConfig;

impl<'a, T: Transport + 'a> Environment<'a, T> for ContextConfig {
    type Query = ();
    type Mutate = ();

    fn query(_client: CallClient<'a, T>) -> Self::Query {
        ()
    }

    fn mutate(_client: CallClient<'a, T>) -> Self::Mutate {
        ()
    }
}
