use std::borrow::BorrowMut;

use identity_provider::{create_identity, Authentication};

mod dht;
mod identity_provider;
mod types;

use libp2p::{identity::Keypair, kad::store::MemoryStore};

use crate::{identity_provider::get_identifier, types::AlgorithmType};

fn main() {
    //generate keypair in any way
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let authentication: Authentication = Authentication {
        algorithm: AlgorithmType::Ed25519,
        controller: None,
        public_key: public_key.clone(),
    };

    let peer_id = public_key.to_peer_id();
    let mut store = MemoryStore::new(peer_id);

    println!("Generating identity");
    let identity = create_identity(store.borrow_mut(), authentication);
    let formatted_identity = serde_json::to_string_pretty(&identity).unwrap();
    println!("Stored did document: {}", formatted_identity);

    if let Some(identity) = get_identifier(store.borrow_mut(), identity.id.clone()) {
        let formatted_identity = serde_json::to_string_pretty(&identity).unwrap();
        println!("Fetched did document {}", formatted_identity)
    } else {
        println!("Error while reading record")
    }
}
