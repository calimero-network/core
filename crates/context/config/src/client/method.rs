#![cfg(feature = "client")]

use super::protocol::Protocol;

pub trait Method<P: Protocol> {
    type Returns;

    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}

pub mod utils {
    #![expect(clippy::type_repetition_in_bounds, reason = "Useful for clarity")]

    use super::Method;
    #[cfg(feature = "ethereum_client")]
    use crate::client::protocol::ethereum::Ethereum;
    #[cfg(feature = "icp_client")]
    use crate::client::protocol::icp::Icp;
    #[cfg(feature = "near_client")]
    use crate::client::protocol::near::Near;
    #[cfg(feature = "starknet_client")]
    use crate::client::protocol::starknet::Starknet;
    #[cfg(feature = "stellar_client")]
    use crate::client::protocol::stellar::Stellar;
    use crate::client::protocol::Protocol;
    use crate::client::transport::Transport;
    use crate::client::{CallClient, ClientError, Operation};

    pub async fn send<M, R, T: Transport>(
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
            #[cfg(feature = "near_client")]
            Near::PROTOCOL => client.send::<Near, _>(params).await,
            #[cfg(feature = "starknet_client")]
            Starknet::PROTOCOL => client.send::<Starknet, _>(params).await,
            #[cfg(feature = "icp_client")]
            Icp::PROTOCOL => client.send::<Icp, _>(params).await,
            #[cfg(feature = "stellar_client")]
            Stellar::PROTOCOL => client.send::<Stellar, _>(params).await,
            #[cfg(feature = "ethereum_client")]
            Ethereum::PROTOCOL => client.send::<Ethereum, _>(params).await,
            unsupported_protocol => Err(ClientError::UnsupportedProtocol {
                found: unsupported_protocol.to_owned(),
                expected: {
                    let mut v: Vec<std::borrow::Cow<'static, str>> = Vec::new();
                    #[cfg(feature = "near_client")]
                    v.push(std::borrow::Cow::from(Near::PROTOCOL));
                    #[cfg(feature = "starknet_client")]
                    v.push(std::borrow::Cow::from(Starknet::PROTOCOL));
                    #[cfg(feature = "icp_client")]
                    v.push(std::borrow::Cow::from(Icp::PROTOCOL));
                    #[cfg(feature = "stellar_client")]
                    v.push(std::borrow::Cow::from(Stellar::PROTOCOL));
                    #[cfg(feature = "ethereum_client")]
                    v.push(std::borrow::Cow::from(Ethereum::PROTOCOL));
                    std::borrow::Cow::Owned(v)
                },
            }),
        }
    }
}

use crate::repr::Repr;
use crate::types::ContextIdentity;

#[inline]
pub(crate) fn to_repr_identities(identities: &[ContextIdentity]) -> Vec<Repr<ContextIdentity>> {
    identities.iter().map(|id| Repr::new(*id)).collect()
}
