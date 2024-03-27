use base64::engine::general_purpose::STANDARD;
use base64::engine::Engine;
use borsh::BorshSerialize;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

fn create_payload(message: &str, nonce: [u8; 32], recipient: &str, callback_url: &str) -> Payload {
    Payload {
        tag: 2147484061,
        message: message.to_string(),
        nonce,
        recipient: recipient.to_string(),
        callback_url: Some(callback_url.to_string()),
    }
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();

    hasher.update(bytes);

    let result = hasher.finalize();

    let mut hash_array = [0u8; 32];
    hash_array.copy_from_slice(&result);

    hash_array
}

pub(crate) fn verify_signature(
    challenge: &str,
    message: &str,
    app: &str,
    curl: &str,
    signature_base64: &str,
    public_key_str: &str,
) -> bool {
    let nonce = match decode_to_fixed_array::<32>(Encoding::Base64, challenge) {
        Ok(nonce) => nonce,
        Err(_) => return false,
    };
    let payload: Payload = create_payload(message, nonce, app, curl);
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
    encoding: Encoding,
    encoded: &str,
) -> eyre::Result<[u8; N]> {
    let decoded_vec = match encoding {
        Encoding::Base58 => bs58::decode(encoded)
            .into_vec()
            .map_err(|e| eyre::Report::new(e))?,
        Encoding::Base64 => STANDARD.decode(encoded).map_err(|e| eyre::Report::new(e))?,
    };

    let fixed_array: [u8; N] = decoded_vec
        .try_into()
        .map_err(|_| eyre::Report::msg("Incorrect length"))?;
    Ok(fixed_array)
}

fn verify(public_key_str: &str, message: &[u8], signature: &str) -> eyre::Result<()> {
    let encoded_key = public_key_str.trim_start_matches("ed25519:");

    let decoded_key: [u8; 32] =
        decode_to_fixed_array(Encoding::Base58, encoded_key).map_err(|e| eyre::eyre!(e))?;
    let vk = VerifyingKey::from_bytes(&decoded_key).map_err(|e| eyre::eyre!(e))?;

    let decoded_signature: [u8; 64] =
        decode_to_fixed_array(Encoding::Base64, signature).map_err(|e| eyre::eyre!(e))?;
    let signature = Signature::from_bytes(&decoded_signature);

    vk.verify(message, &signature).map_err(|e| eyre::eyre!(e))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_signature_valid() {
        let challenge = "89qdrkz1egXlJ2wwF1tcZpuFT0LXT4AHhnAnFvG3N/E=";
        let message = "helloworld";
        let app = "me";
        let curl = "http://127.0.0.1:2428/admin/confirm-wallet";
        let signature_base64 = "rkBQLYN7xxe1oetSfktrqL5jgVsZWKNvKZJmoZLNh756KIUBseYIzK3Dt17O60aPMl6S17lDnIlLVLOLdi5OCw==";
        let public_key = "ed25519:DxdDEdfg4sARk2YteEvp6KsqUGAgKyCZkYTqrboGWwiV";

        assert!(verify_signature(
            challenge,
            message,
            app,
            curl,
            signature_base64,
            public_key
        ));
    }

    #[test]
    fn test_verify_signature_invalid() {
        let challenge = "89qdrkz1egXlJ2wwF1tcZpuFT0LXT4AHhnAnFvG3N/E=";
        let message = "helloworld";
        let app = "me";
        let curl = "http://127.0.0.1:2428/admin/confirm-wallet";
        let signature_base64 = "rkBQLYN7xxe1oetSfktrqL5jgVsZWKNvKZJmoZLNh756KIUBseYIzK3Dt17O60aPMl6S17lDnIlLVsOLdi5OCw==";
        let public_key = "ed25519:DxdDEdfg4sARk2YteEvp6KsqUGAgKyCZkYTqrboGWwiV";

        assert!(!verify_signature(
            challenge,
            message,
            app,
            curl,
            signature_base64,
            public_key
        ));
    }
}
