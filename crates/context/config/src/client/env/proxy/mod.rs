use crate::client::{CallClient, Environment};

/// Context proxy environment type
#[derive(Copy, Clone, Debug)]
pub enum ContextProxy {}

impl<'a, T: 'a> Environment<'a, T> for ContextProxy {
    type Query = ();
    type Mutate = ();

    fn query(_client: CallClient<'a, T>) -> Self::Query {
        ()
    }

    fn mutate(_client: CallClient<'a, T>) -> Self::Mutate {
        ()
    }
}
