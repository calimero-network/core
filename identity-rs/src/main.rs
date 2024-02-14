use std::borrow::BorrowMut;

use libp2p::{identity::Keypair, kad::store::MemoryStore};

mod dht;
mod identity_provider;
mod types;
mod vp;

use identity_provider::{create_identity, get_identifier, Authentication};
use types::AlgorithmType;

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

    println!("Generate identity");
    let identity = create_identity(store.borrow_mut(), authentication);
    let did_document = match identity {
        Ok(value) => {
            let formatted_identity = serde_json::to_string_pretty(&value).unwrap();
            println!("Stored did document: {}", formatted_identity);
            value
        }
        Err(err) => {
            println!("Error while reading record {}", err);
            return;
        }
    };

    println!("Fetch identity");
    let identity = get_identifier(store.borrow_mut(), did_document.id.clone());
    match identity {
        Ok(value) => {
            let formatted_identity = serde_json::to_string_pretty(&value).unwrap();
            println!("Fetched did document {}", formatted_identity)
        }
        Err(err) => {
            println!("Error while reading record {}", err)
        }
    }
}
