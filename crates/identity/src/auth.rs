use libp2p::identity::PublicKey;
use web3::signing::{keccak256, recover};

pub fn verify_near_public_key(
    public_key: &str,
    msg: &[u8],
    signature: &[u8],
) -> eyre::Result<bool> {
    let public_key = bs58::decode(public_key)
        .into_vec()
        .map_err(|_| eyre::eyre!("Invalid public key: Base58 encoding error"))?;

    let public_key = PublicKey::try_decode_protobuf(&public_key)
        .map_err(|_| eyre::eyre!("Invalid public key: Protobuf encoding error"))?;

    Ok(public_key.verify(msg, signature))
}

pub fn verify_eth_signature(account: &str, message: &str, signature: &str) -> eyre::Result<bool> {
    let signature_bytes = match hex::decode(signature.trim_start_matches("0x")) {
        Ok(bytes) => bytes,
        Err(_) => eyre::bail!("Cannot decode signature."),
    };

    // Ensure the signature is the correct length (65 bytes)
    if signature_bytes.len() != 65 {
        eyre::bail!("Signature must be 65 bytes long.")
    }

    let message_hash = eth_message(message);
    let recovery_id = signature_bytes[64] as i32 - 27;

    // Attempt to recover the public key, returning false if recovery fails
    match recover(&message_hash, &signature_bytes[..64], recovery_id) {
        Ok(pubkey) => {
            // Format the recovered public key as a hex string
            let pubkey_hex = format!("{:02X?}", pubkey);
            // Compare the recovered public key with the account address in a case-insensitive manner
            let result = account.eq_ignore_ascii_case(&pubkey_hex);
            if !result {
                eyre::bail!("Public key and account does not match.")
            }
            Ok(true)
        }
        Err(_) => eyre::bail!("Cannot recover public key."),
    }
}

fn eth_message(message: &str) -> [u8; 32] {
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
        let account = "0x04a4e95eeebe44a0f37b75f40957445d2d16a88c".to_string();
        let message = "blabla";
        let message_hash = eth_message(message);
        let signature ="0x15a88c2b8f3f3a0549c88dcbdba063de517ce55e68fdf2ad443f2c24f71904c740f212f0d74b94e271b9d549a8825894d0869b173709a5e798ec6997a1c72d5b1b".trim_start_matches("0x");
        let signature = hex::decode(signature).unwrap();
        println!("{} {:?} {:?}", account, message_hash, signature);
        let recovery_id = signature[64] as i32 - 27;
        let pubkey = recover(&message_hash, &signature[..64], recovery_id);
        assert!(pubkey.is_ok());
        let pubkey = pubkey.unwrap();
        let pubkey = format!("{:02X?}", pubkey);
        assert_eq!(account, pubkey)
    }
}
