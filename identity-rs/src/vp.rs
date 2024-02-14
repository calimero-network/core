use std::io;

use libp2p::identity::Keypair;

use crate::types::{VerifiableCredential, VerifiablePresentation};

pub fn create_verifiable_presentation(
    challenge: &String,
    verifiable_credentials: VerifiableCredential,
    key_pair: &Keypair,
) -> Result<VerifiablePresentation, io::Error> {
    //sign challenge with node private key

    let signature = key_pair
        .sign(challenge.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let vp = VerifiablePresentation {
        challenge: challenge.clone(),
        verifiable_credential: verifiable_credentials,
        signature,
    };

    Ok(vp)
}
