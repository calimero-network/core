use std::collections::BTreeMap;
use std::io::Cursor;

use soroban_sdk::xdr::{FromXdr, Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{Address, Bytes, BytesN, Env, IntoVal, TryFromVal, TryIntoVal, Val};

use crate::client::env::config::requests::{
    ApplicationRequest, ApplicationRevisionRequest, FetchNonceRequest, HasMemberRequest,
    MembersRequest, MembersRevisionRequest, PrivilegesRequest, ProxyContractRequest,
};
use crate::client::env::Method;
use crate::client::protocol::stellar::Stellar;
use crate::repr::ReprTransmute;
use crate::stellar::stellar_types::{StellarApplication, StellarCapability};
use crate::types::{Application, Capability, ContextIdentity, Revision, SignerId};

impl Method<Stellar> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
        }

        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let stellar_application = StellarApplication::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        let application: Application<'_> = stellar_application.into();

        Ok(application)
    }
}

impl Method<Stellar> for ApplicationRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: Val = context_id.into_val(&env);

        let args = (context_id_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let revision: u64 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;
        Ok(revision)
    }
}

impl Method<Stellar> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let offset_val: u32 = self.offset as u32;
        let length_val: u32 = self.length as u32;

        let args = (context_id_val, offset_val, length_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let members: soroban_sdk::Vec<BytesN<32>> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to Vec<BytesN<32>>: {:?}", e))?;

        Ok(members
            .iter()
            .map(|id| id.to_array().rt().expect("infallible conversion"))
            .collect())
    }
}

impl Method<Stellar> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id_bytes: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id: BytesN<32> = context_id_bytes.into_val(&env);
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");
        let identity: BytesN<32> = identity_bytes.into_val(&env);

        let args = (context_id, identity);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let result: bool = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to bool: {:?}", e))?;

        Ok(result)
    }
}

impl Method<Stellar> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let args = (context_id_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let revision: u64 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;
        Ok(revision)
    }
}

impl<'a> Method<Stellar> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let mut identities: soroban_sdk::Vec<BytesN<32>> = soroban_sdk::Vec::new(&env);

        for identity in self.identities.iter() {
            let identity_raw: [u8; 32] = identity.rt().expect("infallible conversion");
            identities.push_back(identity_raw.into_val(&env));
        }

        let args = (context_id_val, identities);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let privileges_map: soroban_sdk::Map<BytesN<32>, soroban_sdk::Vec<StellarCapability>> =
            sc_val
                .try_into_val(&env)
                .map_err(|e| eyre::eyre!("Failed to convert to privileges map: {:?}", e))?;

        // Convert to standard collections
        privileges_map
            .iter()
            .map(|(id, caps)| {
                let signer = id.to_array().rt().expect("infallible conversion");

                let capabilities = caps.iter().map(|cap| cap.into()).collect();

                Ok((signer, capabilities))
            })
            .collect()
    }
}

impl Method<Stellar> for ProxyContractRequest {
    type Returns = String;

    const METHOD: &'static str = "proxy_contract";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_raw: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_val: BytesN<32> = context_raw.into_val(&env);

        let args = (context_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let env = Env::default();
        let address = Address::try_from_val(&env, &sc_val)
            .map_err(|e| eyre::eyre!("Failed to convert to address: {:?}", e))?;

        Ok(address.to_string().to_string())
    }
}

impl Method<Stellar> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let context_id_val: BytesN<32> = context_id.into_val(&env);

        let member_id: [u8; 32] = self.member_id.rt().expect("infallible conversion");
        let member_id_val: BytesN<32> = member_id.into_val(&env);

        let args = (context_id_val, member_id_val);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let nonce: u64 = sc_val
            .try_into()
            .map_err(|e| eyre::eyre!("Failed to convert to u64: {:?}", e))?;

        Ok(Some(nonce))
    }
}
