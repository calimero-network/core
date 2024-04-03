use libp2p::identity::Keypair;
use web3::signing::{keccak256, recover};

pub fn verify_peer_auth(keypair: &Keypair, msg: &[u8], signature: &[u8]) -> bool {
    keypair.public().verify(msg, signature)
}

pub fn verify_eth_signature(account: &str, message: &str, signature: &str) -> bool {
    let signature_bytes = match hex::decode(signature.trim_start_matches("0x")) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    // Ensure the signature is the correct length (65 bytes)
    if signature_bytes.len() != 65 {
        return false;
    }

    let message_hash = eth_message(message);
    let recovery_id = signature_bytes[64] as i32 - 27;

    // Attempt to recover the public key, returning false if recovery fails
    match recover(&message_hash, &signature_bytes[..64], recovery_id) {
        Ok(pubkey) => {
            // Format the recovered public key as a hex string
            let pubkey_hex = format!("{:02X}", pubkey);
            // Compare the recovered public key with the account address in a case-insensitive manner
            account.eq_ignore_ascii_case(&pubkey_hex)
        }
        Err(_) => false,
    }
}

pub fn eth_message(message: &str) -> [u8; 32] {
    keccak256(
        format!(
            "{}{}{}",
            "\x19Ethereum Signed Message:\n",
            message.len(),
            message
        )
        .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recover() {
        let account = "0x63f9a92d8d61b48a9fff8d58080425a3012d05c8".to_string();
        let message = "0x63f9a92d8d61b48a9fff8d58080425a3012d05c8igwyk4r1o7o";
        let message = eth_message(message);
        let signature ="0x382a3e04daf88f322730f6a2972475fc5646ea8c4a7f3b5e83a90b10ba08a7364cd2f55348f2b6d210fbed7fc485abf19ecb2f3967e410d6349dd7dd1d4487751b".trim_start_matches("0x");
        let signature = hex::decode(signature).unwrap();
        println!("{} {:?} {:?}", account, message, signature);
        let recovery_id = signature[64] as i32 - 27;
        let pubkey = recover(&message, &signature[..64], recovery_id);
        assert!(pubkey.is_ok());
        let pubkey = pubkey.unwrap();
        let pubkey = format!("{:02X?}", pubkey);
        assert_eq!(account, pubkey)
    }

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
