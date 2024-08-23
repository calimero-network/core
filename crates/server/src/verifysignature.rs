#[cfg(test)]
#[path = "tests/verifysignature.rs"]
mod tests;

use base64::engine::general_purpose::STANDARD;
use base64::engine::Engine;
use borsh::BorshSerialize;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use eyre::{eyre, Report, Result as EyreResult};
use sha2::{Digest, Sha256};

fn create_payload(message: &str, nonce: [u8; 32], recipient: &str, callback_url: &str) -> Payload {
    Payload {
        tag: 2_147_484_061,
        message: message.to_owned(),
        nonce,
        recipient: recipient.to_owned(),
        callback_url: Some(callback_url.to_owned()),
    }
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();

    hasher.update(bytes);

    let result = hasher.finalize();

    let mut hash_array = [0_u8; 32];
    hash_array.copy_from_slice(&result);

    hash_array
}

pub fn verify_near_signature(
    challenge: &str,
    message: &str,
    app: &str,
    callback_url: &str,
    signature_base64: &str,
    public_key_str: &str,
) -> bool {
    let Ok(nonce) = decode_to_fixed_array::<32>(&Encoding::Base64, challenge) else {
        return false;
    };
    let payload = create_payload(message, nonce, app, callback_url);
    let mut borsh_payload: Vec<u8> = Vec::new();
    payload.serialize(&mut borsh_payload).unwrap();

    let payload_hash = hash_bytes(&borsh_payload);

    verify(public_key_str, &payload_hash, signature_base64).is_ok()
}

enum Encoding {
    Base64,
    Base58,
}

fn decode_to_fixed_array<const N: usize>(
    encoding: &Encoding,
    encoded: &str,
) -> EyreResult<[u8; N]> {
    let decoded_vec = match encoding {
        Encoding::Base58 => bs58::decode(encoded)
            .into_vec()
            .map_err(|e| Report::new(e))?,
        Encoding::Base64 => STANDARD.decode(encoded).map_err(|e| Report::new(e))?,
    };

    let fixed_array: [u8; N] = decoded_vec
        .try_into()
        .map_err(|_| Report::msg("Incorrect length"))?;
    Ok(fixed_array)
}

fn verify(public_key_str: &str, message: &[u8], signature: &str) -> EyreResult<()> {
    let encoded_key = public_key_str.trim_start_matches("ed25519:");

    let decoded_key: [u8; 32] =
        decode_to_fixed_array(&Encoding::Base58, encoded_key).map_err(|e| eyre!(e))?;
    let vk = VerifyingKey::from_bytes(&decoded_key).map_err(|e| eyre!(e))?;

    let decoded_signature: [u8; 64] =
        decode_to_fixed_array(&Encoding::Base64, signature).map_err(|e| eyre!(e))?;
    let signature = Signature::from_bytes(&decoded_signature);

    vk.verify(message, &signature).map_err(|e| eyre!(e))?;

    Ok(())
}

#[derive(BorshSerialize)]
struct Payload {
    tag: u32,
    message: String,
    nonce: [u8; 32],
    recipient: String,
    callback_url: Option<String>,
}
