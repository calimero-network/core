use rand as _;
use ring::aead;
use serde::{Serialize, Deserialize};

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct SharedKey {
    key: ed25519_dalek::SecretKey,
}

#[derive(Debug)]
pub struct Record {
    pub token: Vec<u8>,
    pub nonce: [u8; aead::NONCE_LEN],
}

impl SharedKey {
    pub fn new(sk: &ed25519_dalek::SigningKey, pk: &ed25519_dalek::VerifyingKey) -> Self {
        SharedKey {
            key: (sk.to_scalar()
                * curve25519_dalek::edwards::CompressedEdwardsY(pk.to_bytes())
                    .decompress()
                    .expect("pk should be guaranteed to be the y coordinate"))
            .compress()
            .to_bytes(),
        }
    }

    pub fn encrypt(
        &self,
        token: Vec<u8>,
        nonce: [u8; aead::NONCE_LEN],
    ) -> eyre::Result<Vec<u8>, ()> {
        let encryption_key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &self.key).unwrap());

        let mut encrypted_token = token;
        encryption_key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut encrypted_token,
            )
            .expect("failed to encrypt token");

        Ok(encrypted_token)
    }

    pub fn decrypt(
        &self,
        token: Vec<u8>,
        nonce: [u8; aead::NONCE_LEN],
    ) -> eyre::Result<Vec<u8>, ()> {
        let mut decrypted_token = token;
        let decryption_key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &self.key).unwrap());

        let decrypted_len = decryption_key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut decrypted_token,
            )
            .expect("failed to decrypt token")
            .len();

        decrypted_token.truncate(decrypted_len);

        Ok(decrypted_token)
    }
}

#[cfg(test)]
mod tests {

    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use super::*;

    #[test]
    fn test_encrypt_decrypt() -> eyre::Result<(), eyre::ErrReport> {
        let mut csprng = OsRng {};
        let signer = SigningKey::generate(&mut csprng);
        let verifier = SigningKey::generate(&mut csprng);
        let signer_shared_key = SharedKey::new(&signer, &verifier.verifying_key());
        let verifier_shared_key = SharedKey::new(&verifier, &signer.verifying_key());

        let token = b"privacy is important".to_vec();
        let nonce = [0u8; aead::NONCE_LEN];

        let encrypted_token = signer_shared_key
            .encrypt(token.clone(), nonce)
            .expect("encryption failed");

        let decrypted_token = verifier_shared_key.decrypt(encrypted_token, nonce);

        let decrypted_token = decrypted_token.unwrap();
        assert_eq!(decrypted_token, token);
        assert_ne!(decrypted_token, b"privacy is not important".to_vec());

        Ok(())
    }

    #[test]
    fn test_decrypt_with_invalid_key() -> eyre::Result<(), eyre::ErrReport> {
        let mut csprng = OsRng {};
        let signer = SigningKey::generate(&mut csprng);
        let verifier = SigningKey::generate(&mut csprng);
        let invalid = SigningKey::generate(&mut csprng);

        let signer_shared_key = SharedKey::new(&signer, &verifier.verifying_key());
        let invalid_shared_key = SharedKey::new(&invalid, &invalid.verifying_key());

        let token = b"privacy is important".to_vec();
        let nonce = [0u8; aead::NONCE_LEN];

        let encrypted_token = signer_shared_key
            .encrypt(token.clone(), nonce)
            .expect("encryption failed");

        let decrypted_data = invalid_shared_key.decrypt(encrypted_token, nonce);

        assert!(decrypted_data.is_err());

        Ok(())
    }
}
