use ring::aead;
use serde::{Deserialize, Serialize};

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

    pub fn encrypt(&self, payload: Vec<u8>, nonce: [u8; aead::NONCE_LEN]) -> Option<Vec<u8>> {
        let encryption_key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &self.key).ok()?);

        let mut cipher_text = payload;
        encryption_key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut cipher_text,
            )
            .ok()?;

        Some(cipher_text)
    }

    pub fn decrypt(&self, cipher_text: Vec<u8>, nonce: [u8; aead::NONCE_LEN]) -> Option<Vec<u8>> {
        let decryption_key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &self.key).ok()?);

        let mut payload = cipher_text;
        let decrypted_len = decryption_key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut payload,
            )
            .ok()?
            .len();

        payload.truncate(decrypted_len);

        Some(payload)
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use eyre::OptionExt;

    use super::*;

    #[test]
    fn test_encrypt_decrypt() -> eyre::Result<()> {
        let mut csprng = rand::thread_rng();

        let signer = SigningKey::generate(&mut csprng);
        let verifier = SigningKey::generate(&mut csprng);

        let signer_shared_key = SharedKey::new(&signer, &verifier.verifying_key());
        let verifier_shared_key = SharedKey::new(&verifier, &signer.verifying_key());

        let payload = b"privacy is important";
        let nonce = [0u8; aead::NONCE_LEN];

        let encrypted_payload = signer_shared_key
            .encrypt(payload.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        let decrypted_payload = verifier_shared_key
            .decrypt(encrypted_payload, nonce)
            .ok_or_eyre("decryption failed")?;

        assert_eq!(decrypted_payload, payload);
        assert_ne!(decrypted_payload, b"privacy is not important");

        Ok(())
    }

    #[test]
    fn test_decrypt_with_invalid_key() -> eyre::Result<()> {
        let mut csprng = rand::thread_rng();

        let signer = SigningKey::generate(&mut csprng);
        let verifier = SigningKey::generate(&mut csprng);
        let invalid = SigningKey::generate(&mut csprng);

        let signer_shared_key = SharedKey::new(&signer, &verifier.verifying_key());
        let invalid_shared_key = SharedKey::new(&invalid, &invalid.verifying_key());

        let token = b"privacy is important";
        let nonce = [0u8; aead::NONCE_LEN];

        let encrypted_token = signer_shared_key
            .encrypt(token.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        let decrypted_data = invalid_shared_key.decrypt(encrypted_token, nonce);

        assert!(decrypted_data.is_none());

        Ok(())
    }
}
