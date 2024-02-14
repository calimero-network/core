

pub fn create_wallet_verifiable_credentials(
    peer_id: &String,
    wallet_type: &WalletType,
    address: &String,
    public_key: &String,
    proof: &String
)-> Result<VerifiableCredentials, io::Error>{
    let wallet_verifiable_credential = WalletVerifiableCredential{
        wallet_type,
        address,
        public_key,
        peer_id
    };
    create_verifiable_credentials(wallet_verifiable_credential)
}

pub fn create_verifiable_credentials(
    peer_id: &String,
    credential_subject: &VerifiableCredentialType,
    proof: &String
)-> Result<VerifiableCredentials, io::Error>{

    let verifiable_credentials = VerifiableCredentials{
        id: peer_id, //TBD
        issuer: peer_id,
        credential_subject,
        proof
    };

    Ok(verifiable_credentials)
}