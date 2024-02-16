use std::{borrow::BorrowMut, error::Error};

use libp2p::{identity::Keypair, kad::store::MemoryStore};

mod dht;
mod identity_provider;
mod types;
mod vc;
mod vp;

use identity_provider::{create_identity, get_identifier, Authentication};
use types::AlgorithmType;

use crate::{
    types::WalletType,
    vc::create_wallet_verifiable_credentials,
    vp::{create_verifiable_presentation, validate_verifiable_presentation},
};

fn main() -> Result<(), Box<dyn Error>> {
    //generate keypair in any way
    let peer_keypair = Keypair::generate_ed25519();
    let peer_public_key = peer_keypair.public();

    let authentication: Authentication = Authentication {
        algorithm: AlgorithmType::Ed25519,
        controller: None,
        public_key: peer_public_key.clone(),
    };

    let peer_id = peer_public_key.to_peer_id();
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
            return Err(Box::new(err));
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

    println!("Create verifiable credential");
    let wallet_keypair = Keypair::generate_ed25519();
    let message = peer_id.to_string();
    let proof = wallet_keypair.sign(&message.into_bytes())?;
    let verifiable_credential = create_wallet_verifiable_credentials(
        peer_id.to_string().as_str(),
        &WalletType::NEAR,
        "vuki.near",
        &wallet_keypair.public().encode_protobuf(),
        &proof,
    )?;
    println!(
        "Verifiable credential: {}",
        format!("{:?}", verifiable_credential)
    );

    println!("Create verifiable presentation");
    let application_challenge = "alpha bravo sigma";
    let verifiable_presentation = create_verifiable_presentation(
        &application_challenge.to_string(),
        &verifiable_credential,
        &peer_keypair,
    )?;
    println!(
        "Verifiable presentation: {}",
        format!("{:?}", verifiable_presentation)
    );
    let verifiable_presentation_result =
        validate_verifiable_presentation(&peer_keypair.public(), &verifiable_presentation)?;
    println!("Validate verifiable presentation: {verifiable_presentation_result}");

    Ok(())
}
