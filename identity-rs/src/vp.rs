use libp2p::identity::{Keypair, PublicKey, SigningError};

use crate::types::{VerifiableCredential, VerifiablePresentation};

pub fn create_verifiable_presentation(
    challenge: String,
    verifiable_credential: VerifiableCredential,
    key_pair: &Keypair,
) -> Result<VerifiablePresentation, SigningError> {
    //sign challenge with node private key
    let signature = key_pair.sign(challenge.as_bytes())?;
    let vp = VerifiablePresentation {
        challenge,
        verifiable_credential,
        signature,
    };

    Ok(vp)
}

pub fn validate_verifiable_presentation(
    public_key: &PublicKey,
    verifiable_presentation: &VerifiablePresentation,
) -> bool {
    public_key.verify(
        verifiable_presentation.challenge.as_bytes(),
        &verifiable_presentation.signature,
    )
}
