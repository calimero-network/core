use base64;
use borsh::{BorshSerialize, BorshDeserialize};
use sha2::{Digest, Sha256};
use near_crypto::{KeyType, PublicKey, Signature};
use std::str::FromStr;
use std::convert::TryInto;

#[derive(Default, BorshSerialize, BorshDeserialize)]
struct Payload {
    tag: u32,
    message: String,
    nonce: [u8; 32],
    recipient: String,
    callback_url: Option<String>,
}


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

fn verify_signature(challenge: &str, message: &str, app: &str, curl: &str, signature_base64: &str, public_key_str: &str) -> bool {
    let decoded_bytes = match base64::decode(&challenge) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("Error decoding base64: {:?}", err);
            return false;
        }
    };

    let mut nonce_vec = decoded_bytes.to_vec();
    nonce_vec.resize(32, 0);
    let nonce: [u8; 32] = nonce_vec.try_into().unwrap();

    let payload = create_payload(message, nonce, app, curl);
    let mut borsh_payload: Vec<u8> = Vec::new();
    payload.serialize(&mut borsh_payload).unwrap();

    let message_signed = hash_bytes(&borsh_payload);

    let real_signature = match base64::decode(signature_base64) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("Error decoding base64 signature: {:?}", err);
            return false;
        }
    };

    let signature_type = KeyType::ED25519;

    let public_key = match PublicKey::from_str(public_key_str) {
        Ok(pk) => pk,
        Err(err) => {
            eprintln!("Error creating public key: {:?}", err);
            return false;
        }
    };

    let signature = match Signature::from_parts(signature_type, &real_signature) {
        Ok(sig) => sig,
        Err(err) => {
            eprintln!("Error creating signature: {:?}", err);
            return false; 
        }
    };

    signature.verify(&message_signed, &public_key)
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

        assert!(verify_signature(challenge, message, app, curl, signature_base64, public_key));
    }

    #[test]
    fn test_verify_signature_invalid() {
        let challenge = "89qdrkz1egXlJ2wwF1tcZpuFT0LXT4AHhnAnFvG3N/E=";
        let message = "helloworld";
        let app = "me";
        let curl = "http://127.0.0.1:2428/admin/confirm-wallet";
        let signature_base64 = "rkBQLYN7xxe1oetSfktrqL5jgVsZWKNvKZJmoZLNh756KIUBseYIzK3Dt17O60aPMl6S17lDnIlLVsOLdi5OCw==";
        let public_key = "ed25519:DxdDEdfg4sARk2YteEvp6KsqUGAgKyCZkYTqrboGWwiV";

        assert!(!verify_signature(challenge, message, app, curl, signature_base64, public_key));
    }
}