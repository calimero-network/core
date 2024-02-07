use libp2p::{identity::PublicKey, kad::store::MemoryStore};
use multibase::{encode, Base};

use crate::{
    dht::Dht,
    types::{AlgorithmType, DidDocument, VerificationMethod},
};

#[derive(Debug)]
pub struct Authentication {
    pub algorithm: AlgorithmType,
    pub controller: Option<String>,
    pub public_key: PublicKey,
}

const DID_CALI_IDENTIFIER: &'static str = "did:cali:";

/// Create decentralized identity document based on provided public key
///  {
///   "id": "did:cali:12D3KooWLU42rBMLrzAyFg8CdpHiPcwJcVsKv9Wx9DtiT4QjPGGV",
///   "verificationMethod": [
///     {
///       "id": "did:cali:12D3KooWLU42rBMLrzAyFg8CdpHiPcwJcVsKv9Wx9DtiT4QjPGGV",
///       "type": "Ed25519",
///       "publicKeyMultibase": "zCovLVG4fQcqT8sDqj76uQuXtQU2LqABAf6X8vnDW36zAidisFK22Z5Ecm28apKb9Kg6ofRo",
///       "controller": "did:cali:12D3KooWLU42rBMLrzAyFg8CdpHiPcwJcVsKv9Wx9DtiT4QjPGGV"
///     }
///   ]
/// }
pub fn create_identity(store: &mut MemoryStore, authentication: Authentication) -> DidDocument {
    let public_key_id = authentication.public_key.to_peer_id().to_base58();
    let multibase_encoded = encode(Base::Base58Btc, &public_key_id);

    let did = DID_CALI_IDENTIFIER.to_string() + &public_key_id;

    let verification_method: VerificationMethod = VerificationMethod {
        id: did.clone() + "#key1",
        algorithm_type: authentication.algorithm.to_string(),
        public_key_multibase: multibase_encoded,
        controller: authentication.controller.unwrap_or(did.clone()),
    };

    let did_document: DidDocument = DidDocument {
        id: did.clone(),
        verification_method: vec![verification_method],
    };

    Dht::new(store).write_record(did_document.clone());

    return did_document;
}

pub fn get_identifier(store: &mut MemoryStore, did: String) -> Option<DidDocument> {
    if let Some(did_document) = Dht::new(store).read_record(did.clone()) {
        return Some(did_document);
    } else {
        return None;
    }
}

#[allow(dead_code)]
pub async fn update_identifier() {
    unimplemented!();
}

#[allow(dead_code)]
pub async fn delete_identifier() {
    unimplemented!();
}
