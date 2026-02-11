use super::protocol::Protocol;

pub mod config;
pub mod proxy;

pub trait Method<P: Protocol> {
    type Returns;

    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}

mod utils {
    #![expect(clippy::type_repetition_in_bounds, reason = "Useful for clarity")]

    use super::Method;
    use crate::client::protocol::near::Near;
    use crate::client::protocol::Protocol;
    use crate::client::transport::Transport;
    use crate::client::{CallClient, ClientError, Operation};

    // todo! when crates are broken up, appropriately locate this
    pub(super) async fn send<M, R, T: Transport>(
        client: &CallClient<'_, T>,
        params: Operation<M>,
    ) -> Result<R, ClientError<T>>
    where
        M: Method<Near, Returns = R>,
    {
        match &*client.protocol {
            Near::PROTOCOL => client.send::<Near, _>(params).await,
            unsupported_protocol => Err(ClientError::UnsupportedProtocol {
                found: unsupported_protocol.to_owned(),
                expected: vec![Near::PROTOCOL.into()].into(),
            }),
        }
    }
}
