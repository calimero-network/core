use calimero_primitives::identity::{PrivateKey, PublicKey};
use ed25519_dalek::{SecretKey, SigningKey};
use ring::aead;

pub const NONCE_LEN: usize = 12;

pub type Nonce = [u8; NONCE_LEN];

#[derive(Copy, Clone, Debug)]
pub struct SharedKey {
    key: SecretKey,
}

impl SharedKey {
    pub fn new(sk: &PrivateKey, pk: &PublicKey) -> Self {
        SharedKey {
            key: (SigningKey::from_bytes(sk).to_scalar()
                * curve25519_dalek::edwards::CompressedEdwardsY(**pk)
                    .decompress()
                    .expect("pk should be guaranteed to be the y coordinate"))
            .compress()
            .to_bytes(),
        }
    }

    pub fn from_sk(sk: &PrivateKey) -> Self {
        SharedKey { key: **sk }
    }

    pub fn encrypt(&self, payload: Vec<u8>, nonce: Nonce) -> Option<Vec<u8>> {
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

    pub fn decrypt(&self, cipher_text: Vec<u8>, nonce: Nonce) -> Option<Vec<u8>> {
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
    use eyre::OptionExt;
    use rand::thread_rng;

    use super::*;

    #[test]
    fn test_encrypt_decrypt() -> eyre::Result<()> {
        let mut csprng = thread_rng();

        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);

        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key());
        let verifier_shared_key = SharedKey::new(&verifier, &signer.public_key());

        let payload = b"privacy is important";
        let nonce = [0u8; NONCE_LEN];

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
        let mut csprng = thread_rng();

        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);
        let invalid = PrivateKey::random(&mut csprng);

        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key());
        let invalid_shared_key = SharedKey::new(&invalid, &invalid.public_key());

        let token = b"privacy is important";
        let nonce = [0u8; NONCE_LEN];

        let encrypted_token = signer_shared_key
            .encrypt(token.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        let decrypted_data = invalid_shared_key.decrypt(encrypted_token, nonce);

        assert!(decrypted_data.is_none());

        Ok(())
    }
}
