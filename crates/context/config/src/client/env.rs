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
    use crate::client::protocol::ethereum::Ethereum;
    use crate::client::protocol::icp::Icp;
    use crate::client::protocol::near::Near;
    use crate::client::protocol::starknet::Starknet;
    use crate::client::protocol::stellar::Stellar;
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
        M: Method<Starknet, Returns = R>,
        M: Method<Icp, Returns = R>,
        M: Method<Stellar, Returns = R>,
        M: Method<Ethereum, Returns = R>,
    {
        match &*client.protocol {
            Near::PROTOCOL => client.send::<Near, _>(params).await,
            Starknet::PROTOCOL => client.send::<Starknet, _>(params).await,
            Icp::PROTOCOL => client.send::<Icp, _>(params).await,
            Stellar::PROTOCOL => client.send::<Stellar, _>(params).await,
            Ethereum::PROTOCOL => client.send::<Ethereum, _>(params).await,
            unsupported_protocol => Err(ClientError::UnsupportedProtocol {
                found: unsupported_protocol.to_owned(),
                expected: vec![
                    Near::PROTOCOL.into(),
                    Starknet::PROTOCOL.into(),
                    Icp::PROTOCOL.into(),
                    Stellar::PROTOCOL.into(),
                ]
                .into(),
            }),
        }
    }
}
