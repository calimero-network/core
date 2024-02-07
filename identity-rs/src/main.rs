use identity_provider::{create_identity, Authentication};

mod identity_provider;
mod types;

use libp2p::identity::Keypair;

use crate::types::AlgorithmType;

fn main() {
    //generate keypair in any way
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let authentication: Authentication = Authentication {
        algorithm: AlgorithmType::Ed25519,
        controller: None,
        public_key: public_key.clone(),
    };

    println!("Generating identity");
    let identity = create_identity(authentication);
    let formatted_identity = serde_json::to_string_pretty(&identity).unwrap();
    println!("DID: {}", formatted_identity);
}
