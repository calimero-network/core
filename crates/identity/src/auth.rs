#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;

use eyre::{bail, eyre, Result as EyreResult};
use libp2p::identity::PublicKey;
use web3::signing::{keccak256, recover};

pub fn verify_near_public_key(public_key: &str, msg: &[u8], signature: &[u8]) -> EyreResult<bool> {
    let public_key = bs58::decode(public_key)
        .into_vec()
        .map_err(|_| eyre!("Invalid public key: Base58 encoding error"))?;

    let public_key = PublicKey::try_decode_protobuf(&public_key)
        .map_err(|_| eyre!("Invalid public key: Protobuf encoding error"))?;

    Ok(public_key.verify(msg, signature))
}

pub fn verify_eth_signature(account: &str, message: &str, signature: &str) -> EyreResult<bool> {
    let Ok(signature_bytes) = hex::decode(signature.trim_start_matches("0x")) else {
        bail!("Cannot decode signature.")
    };

    // Ensure the signature is the correct length (65 bytes)
    if signature_bytes.len() != 65 {
        bail!("Signature must be 65 bytes long.")
    }

    let message_hash = eth_message(message);
    let recovery_id = i32::from(signature_bytes[64]).saturating_sub(27_i32);

    // Attempt to recover the public key, returning false if recovery fails
    match recover(&message_hash, &signature_bytes[..64], recovery_id) {
        Ok(pubkey) => {
            // Format the recovered public key as a hex string
            let pubkey_hex = format!("{pubkey:02X?}");
            // Compare the recovered public key with the account address in a case-insensitive manner
            let result = account.eq_ignore_ascii_case(&pubkey_hex);
            if !result {
                bail!("Public key and account does not match.")
            }
            Ok(true)
        }
        Err(_) => bail!("Cannot recover public key."),
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
