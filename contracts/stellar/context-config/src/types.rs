extern crate alloc;
use alloc::vec::Vec as StdVec;

use soroban_sdk::{
  contracterror, contracttype, xdr::ToXdr, Bytes, BytesN, Env, String, Vec
};
// Core types for Application
#[contracttype]
#[derive(Clone, Debug)]
pub struct Application {
  pub id: BytesN<32>,
  pub blob: BytesN<32>,
  pub size: u64,
  pub source: String,
  pub metadata: Bytes,
}

// Request structures
#[contracttype]
#[derive(Clone, Debug)]
pub struct ContextRequest {
  pub context_id: BytesN<32>,
  pub kind: ContextRequestKind,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum Capability {
  ManageApplication,
  ManageMembers,
  Proxy,
}

// Request types without named fields in enum variants
#[contracttype]
#[derive(Clone, Debug)]
pub enum ContextRequestKind {
  Add(BytesN<32>, Application),
  UpdateApplication(Application),
  AddMembers(Vec<BytesN<32>>),
  RemoveMembers(Vec<BytesN<32>>),
  Grant(Vec<(BytesN<32>, Capability)>),
  Revoke(Vec<(BytesN<32>, Capability)>),
  UpdateProxyContract,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum RequestKind {
  Context(ContextRequest),
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Request {
  pub kind: RequestKind,
  pub signer_id: BytesN<32>,
  pub nonce: u64,
}

// Signed request wrapper
#[contracttype]
#[derive(Clone, Debug)]
pub struct SignedRequest {
  pub payload: Request,
  pub signature: BytesN<64>,
}

impl SignedRequest {
    pub fn new<F>(
        env: &Env, 
        payload: Request, 
        sign: F
    ) -> Result<Self, Error>
    where 
        F: FnOnce(&[u8]) -> Result<ed25519_dalek::Signature, ed25519_dalek::SignatureError>
    {
        // Use the same XDR serialization as the original working code
        let request_xdr = payload.clone().to_xdr(env);
        let std_vec: StdVec<u8> = request_xdr.into_iter().collect();
        
        // Sign using the provided signing function
        let signature = sign(&std_vec)
            .map_err(|_| Error::InvalidSignature)?;
        
        Ok(Self {
            payload,
            signature: BytesN::from_array(env, &signature.to_bytes()),
        })
    }

    pub fn verify(&self, env: &Env) -> Result<Request, Error> {
        // Use the exact same verification as before
        let bytes = self.payload.clone().to_xdr(env);
        
        env.crypto().ed25519_verify(
            &self.payload.signer_id,
            &bytes,
            &self.signature,
        );
    
        Ok(self.payload.clone())
    }
}

#[contracterror]
#[derive(Copy, Clone, Debug)]
pub enum Error {
    InvalidSignature = 1,
    Unauthorized = 2,
    ContextExists = 3,
    ContextNotFound = 4,
    InvalidNonce = 5,
    ProxyCodeNotSet = 6,
    NotAMember = 7,
    InvalidState = 8,
    ProxyUpgradeFailed = 9,
}
