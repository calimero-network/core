use axum::http::{HeaderMap, StatusCode};

use calimero_identity::auth::verify_peer_auth;
use error::ErrorUnauthorized;
use libp2p::identity::Keypair;
use tracing::info;

use crate::error;

struct AuthHeaders {
    signature: Vec<u8>,
    content: Vec<u8>,
}

pub fn auth(
    // run the `HeaderMap` extractor
    headers: &HeaderMap,
    keypair: &Keypair,
) -> Result<(), ErrorUnauthorized> {
    match get_auth_headers(&headers) {
        Ok(auth_headers) if token_is_valid(&keypair, &auth_headers) => Ok(()),
        // Ok(_) => Err(ErrorUnauthorized::new("Unauthorized")),
        Ok(_) => Err(ErrorUnauthorized::new("Keypair not matching signature.")),
        Err(error) => Err(error), // _ => Err(StatusCode::UNAUTHORIZED),
    }
}

fn get_auth_headers(headers: &HeaderMap) -> Result<AuthHeaders, ErrorUnauthorized> {
    let signature = headers.get("signature");
    if signature.is_none() {
        return Err(ErrorUnauthorized::new("Missing signature header"));
    }
    let signature = signature.unwrap().to_str();
    if signature.is_err() {
        return Err(ErrorUnauthorized::new("Cannot unwrap signature"));
    }
    let signature = bs58::decode(signature.unwrap()).into_vec();
    if signature.is_err() {
        return Err(ErrorUnauthorized::new("Invalid base58"));
    }
    let signature = signature.unwrap();

    let content = headers.get("content");
    if content.is_none() {
        return Err(ErrorUnauthorized::new("Missing content header"));
    }
    let content = content.unwrap().to_str();
    if content.is_err() {
        return Err(ErrorUnauthorized::new("Cannot unwrap content"));
    }
    let content = bs58::decode(content.unwrap()).into_vec();
    if content.is_err() {
        return Err(ErrorUnauthorized::new("Invalid base58"));
    }
    let content = content.unwrap();

    let auth = AuthHeaders { signature, content };
    Ok(auth)
}

fn token_is_valid(keypair: &Keypair, auth_headers: &AuthHeaders) -> bool {
    let verify_result = verify_peer_auth(
        keypair,
        auth_headers.content.as_slice(),
        auth_headers.signature.as_slice(),
    );
    if verify_result.is_err() {
        info!("{:?}", verify_result.err().unwrap().as_str());
        return false;
    }
    let res = verify_result.unwrap();
    println!("keypair {:?}", keypair);
    res
}
