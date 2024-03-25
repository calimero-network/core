use base64;
use borsh::{BorshSerialize, BorshDeserialize};
use sha2::{Digest, Sha256};
use near_crypto::{KeyType, PublicKey, Signature};
use std::str::FromStr;

#[derive(Default, BorshSerialize, BorshDeserialize)]
struct Payload {
    message: String,
    nonce: Vec<u8>,
    recipient: String,
    callback_url: String,
}

fn create_payload(message: &str, nonce: Vec<u8>, recipient: &str, callback_url: &str) -> Payload {
    Payload {
        message: message.to_string(),
        nonce,
        recipient: recipient.to_string(),
        callback_url: callback_url.to_string(),
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

pub fn verify_signature(challenge: &str, message: &str, app: &str, curl: &str, signature_base64: &str, public_key_str: &str) -> bool {
    let decoded_bytes = match base64::decode(&challenge) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("Error decoding base64: {:?}", err);
            return false;
        }
    };
    let nonce = decoded_bytes.to_vec();

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
        let challenge = "oXY3ssl/va5Gv5FCb896AXa1z0WEQVVDl9tnl+udUuo=";
        let message = "helloworld";
        let app = "me";
        let curl = "http://127.0.0.1:2428/adminconfirm-wallet";
        let signature_base64 = "0Mhn7KKHL72BKsMzokCHatv/NvshfSYtcVrj3Jk952ykSEEWgRlmQIUSGNghaAOu5nvbtktrMyvt6olyCnbmDg==";
        let public_key = "ed25519:62WU79rjHuyBT7dcE1iYBHEcamSkmURGoRbcNDYB65rV";

        assert!(verify_signature(challenge, message, app, curl, signature_base64, public_key));
    }

    #[test]
    fn test_verify_signature_invalid() {
        let challenge = "oXY3ssl/va5Gv5FCb896AXa1z0WEQVVDl9tnl+udUuo=";
        let message = "helloworld";
        let app = "me";
        let curl = "http://127.0.0.1:2428/adminconfirm-wallet";
        let signature_base64 = "0Mhn7KKHL72BKsMzokCHatv/NvshfSYtcVrj3Jk952ykSEEWgRlmQIUSGNghaAOu5nvbtktrMyvt6xlyCnbmDg==";
        let public_key = "ed25519:62WU79rjHuyBT7dcE1iYBHEcamSkmURGoRbcNDYB65rV";

        assert!(!verify_signature(challenge, message, app, curl, signature_base64, public_key));
    }
}