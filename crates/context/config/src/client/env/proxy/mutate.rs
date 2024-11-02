use crate::client::CallClient;

mod propose;

#[derive(Debug)]
pub struct ContextProxyMutate<'a, T> {
    client: CallClient<'a, T>,
}
