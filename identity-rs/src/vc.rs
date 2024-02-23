use std::io;

use libp2p::identity::PublicKey;
use types::WalletType;

use crate::types::{
    self, AlgorithmType, VerifiableCredential, VerifiableCredentialType, WalletVerifiableCredential,
};

pub fn create_wallet_verifiable_credentials(
    peer_id: String,
    wallet_type: WalletType,
    address: String,
    public_key: Vec<u8>,
    proof: Vec<u8>,
) -> Result<VerifiableCredential, io::Error> {
    let wallet_verifiable_credential = WalletVerifiableCredential {
        wallet_type,
        address,
        public_key,
        peer_id: peer_id.clone(),
    };
    create_verifiable_credentials(
        peer_id,
        VerifiableCredentialType::Wallet(wallet_verifiable_credential),
        proof,
    )
}

pub fn create_verifiable_credentials(
    peer_id: String,
    credential_subject: VerifiableCredentialType,
    proof: Vec<u8>,
) -> Result<VerifiableCredential, io::Error> {
    //check for proof
    let verified_proof = match &credential_subject {
        VerifiableCredentialType::Wallet(wallet) => {
            let pubkey_result = PublicKey::try_decode_protobuf(&wallet.public_key);
            match pubkey_result {
                Ok(pub_k) => pub_k.verify(&wallet.peer_id.as_bytes(), &proof),
                Err(err) => {
                    print!("Error while decoding proof {err}");
                    false
                }
            }
        }
    };
    if !verified_proof {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid proof for public key",
        ));
    }

    let verifiable_credentials = VerifiableCredential {
        id: peer_id.clone(), //TBD
        issuer: peer_id,
        credential_subject,
        proof,
        algorithm_type: AlgorithmType::Ed25519,
    };

    Ok(verifiable_credentials)
}
