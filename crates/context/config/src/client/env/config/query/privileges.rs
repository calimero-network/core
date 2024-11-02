use std::collections::BTreeMap;
use std::ptr;

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::repr::Repr;
use crate::types::{Capability, ContextId, ContextIdentity, SignerId};

#[derive(Debug, Clone, Copy)]
pub struct IdentitiyPrivileges<'a> {
    pub(crate) context_id: Repr<ContextId>,
    pub(crate) identities: &'a [ContextIdentity],
}

impl<'a> Method<IdentitiyPrivileges<'a>> for Near {
    const METHOD: &'static str = "privileges";

    type Returns = BTreeMap<Repr<SignerId>, Vec<Capability>>;

    fn encode(params: &IdentitiyPrivileges<'_>) -> eyre::Result<Vec<u8>> {
        let identities = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(&params.identities)
                as *const [Repr<ContextIdentity>])
        };
        let encoded_body = serde_json::to_vec(&identities)?;
        Ok(encoded_body)
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        let decoded_body = serde_json::from_slice(response)?;
        Ok(decoded_body)
    }
}

impl<'a> Method<IdentitiyPrivileges<'a>> for Starknet {
    type Returns = BTreeMap<Repr<SignerId>, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(params: &IdentitiyPrivileges<'_>) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
