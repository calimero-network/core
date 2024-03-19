use libp2p::identity::Keypair;

pub fn verify_peer_auth(keypair: &Keypair, msg: &[u8], signature: &[u8]) -> bool {
    keypair.public().verify(msg, signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_headers() {
        let keypair = get_peer_keypair().unwrap();
        let msg = "blabla";
        println!("challenge header= {:?}", bs58::encode(msg).into_string());

        let signature = keypair.sign(msg.as_bytes()).unwrap();
        let signature_header = bs58::encode(&signature).into_string();
        println!("signature header = {:?}", signature_header);

        assert_eq!(
            verify_peer_auth(&keypair, msg.as_bytes(), signature.as_slice()),
            true
        );
    }

    pub fn get_peer_keypair() -> Result<Keypair, String> {
        let private_key = "23jhTekjBHR2wvqeGe5kHwJAzoYbhRoqN1YHL9jSsSeqDFwdTJevSnYQ2hcWsBPjGeVMFaTPAX3bPkc2yzyGJQ6AMfCEo";

        let private_key = bs58::decode(private_key)
            .into_vec()
            .map_err(|_| "Invalid PrivKey base 58".to_string())?;

        let keypair = Keypair::from_protobuf_encoding(&private_key)
            .map_err(|_| "Decoding PrivKey failed.".to_string())?;
        Ok(keypair)
    }
}
