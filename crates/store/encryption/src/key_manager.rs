//! Key management for store encryption.
//!
//! Provides key derivation using HKDF and encryption/decryption using AES-256-GCM.

use std::collections::BTreeMap;

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use eyre::{bail, eyre, Result};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Size of AES-256 key in bytes.
const AES_KEY_SIZE: usize = 32;

/// Size of AES-GCM nonce in bytes.
const NONCE_SIZE: usize = 12;

/// Size of AES-GCM authentication tag in bytes.
const TAG_SIZE: usize = 16;

/// Minimum ciphertext size (version + nonce + tag).
const MIN_CIPHERTEXT_SIZE: usize = 1 + NONCE_SIZE + TAG_SIZE;

/// Build the additional authenticated data (AAD) bound to a ciphertext.
///
/// The AAD is `version || nonce`, matching the unencrypted header prepended to
/// every ciphertext. Authenticating it ensures the header cannot be altered
/// without failing decryption.
///
/// The nonce is taken as a fixed-size array so the length is enforced at
/// compile time rather than risking a runtime panic from `copy_from_slice`.
fn aad_for(version: u8, nonce: &[u8; NONCE_SIZE]) -> [u8; 1 + NONCE_SIZE] {
    let mut aad = [0u8; 1 + NONCE_SIZE];
    aad[0] = version;
    aad[1..].copy_from_slice(nonce);
    aad
}

/// A Data Encryption Key (DEK) with secure memory handling.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct Dek {
    key: [u8; AES_KEY_SIZE],
}

impl Dek {
    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.key).expect("valid key size")
    }
}

/// Manages encryption keys with support for key versioning and rotation.
///
/// The `KeyManager` derives Data Encryption Keys (DEKs) from a master key
/// using HKDF. Each DEK has a version, allowing key rotation without
/// re-encrypting all existing data.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct KeyManager {
    /// The master Key Encryption Key (KEK) from KMS.
    master_key: Vec<u8>,
    /// Current DEK version used for new encryptions.
    current_version: u8,
    /// Cached DEKs keyed by version.
    #[zeroize(skip)]
    deks: BTreeMap<u8, Dek>,
}

impl KeyManager {
    /// Create a new `KeyManager` with the given master key.
    ///
    /// The master key is typically obtained from a KMS service and used
    /// to derive all Data Encryption Keys.
    ///
    /// # Arguments
    ///
    /// * `master_key` - The Key Encryption Key (KEK) from KMS
    ///
    /// # Errors
    ///
    /// Returns an error if the master key is empty.
    pub fn new(master_key: Vec<u8>) -> Result<Self> {
        if master_key.is_empty() {
            bail!("Master key cannot be empty");
        }

        let mut manager = Self {
            master_key,
            current_version: 1,
            deks: BTreeMap::new(),
        };

        // Pre-derive the initial DEK
        manager.derive_dek(1)?;

        Ok(manager)
    }

    /// Derive a DEK for the given version and cache it.
    ///
    /// Uses HKDF-SHA256 with a version-specific salt.
    fn derive_dek(&mut self, version: u8) -> Result<()> {
        if self.deks.contains_key(&version) {
            return Ok(());
        }

        let salt = format!("calimero-dek-v{version}");
        let info = b"encryption";

        let hkdf = Hkdf::<Sha256>::new(Some(salt.as_bytes()), &self.master_key);
        let mut key = [0u8; AES_KEY_SIZE];
        hkdf.expand(info, &mut key)
            .map_err(|_| eyre!("HKDF expansion failed"))?;

        drop(self.deks.insert(version, Dek { key }));

        Ok(())
    }

    /// Get the DEK for a specific version, deriving it if necessary.
    fn get_dek(&mut self, version: u8) -> Result<&Dek> {
        self.derive_dek(version)?;
        self.deks
            .get(&version)
            .ok_or_else(|| eyre!("DEK not found for version {version}"))
    }

    /// Encrypt plaintext using the current DEK version.
    ///
    /// The output format is:
    /// ```text
    /// [version: 1 byte][nonce: 12 bytes][ciphertext + tag: variable]
    /// ```
    ///
    /// # Arguments
    ///
    /// * `plaintext` - The data to encrypt
    ///
    /// # Returns
    ///
    /// The encrypted data with version and nonce prepended.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let version = self.current_version;
        let dek = self.get_dek(version)?;
        let cipher = dek.cipher();

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Bind the version and nonce as additional authenticated data so the
        // unencrypted header cannot be tampered with (e.g. flipping the version
        // byte to force a different DEK) without failing the integrity check.
        let aad = aad_for(version, &nonce_bytes);

        // Encrypt
        let ciphertext = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|e| eyre!("Encryption failed: {e}"))?;

        // Build output: version || nonce || ciphertext
        let mut output = Vec::with_capacity(1 + NONCE_SIZE + ciphertext.len());
        output.push(version);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);

        Ok(output)
    }

    /// Decrypt ciphertext, automatically using the correct DEK version.
    ///
    /// The input format must be:
    /// ```text
    /// [version: 1 byte][nonce: 12 bytes][ciphertext + tag: variable]
    /// ```
    ///
    /// # Arguments
    ///
    /// * `ciphertext` - The encrypted data with version and nonce
    ///
    /// # Returns
    ///
    /// The decrypted plaintext.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() < MIN_CIPHERTEXT_SIZE {
            bail!(
                "Ciphertext too short: {} bytes (minimum {})",
                ciphertext.len(),
                MIN_CIPHERTEXT_SIZE
            );
        }

        let version = ciphertext[0];
        // The length check above guarantees these slices are exactly sized.
        let nonce_bytes: [u8; NONCE_SIZE] = ciphertext[1..1 + NONCE_SIZE]
            .try_into()
            .expect("slice is NONCE_SIZE bytes");
        let nonce = Nonce::from_slice(&nonce_bytes);
        let encrypted_data = &ciphertext[1 + NONCE_SIZE..];

        // The version and nonce header is authenticated via AAD, so any
        // tampering with it (or with the DEK version selection) causes the
        // integrity check below to fail.
        //
        // AAD must use the same version byte that drives DEK selection below;
        // both must agree for decryption to succeed, so do not separate these
        // two uses in any future refactor.
        let aad = aad_for(version, &nonce_bytes);

        let dek = self.get_dek(version)?;
        let cipher = dek.cipher();

        cipher
            .decrypt(
                nonce,
                Payload {
                    msg: encrypted_data,
                    aad: &aad,
                },
            )
            .map_err(|e| eyre!("Decryption failed (version {version}): {e}"))
    }

    /// Rotate to a new DEK version.
    ///
    /// After rotation, all new encryptions will use the new version.
    /// Old data can still be decrypted using cached DEKs.
    ///
    /// # Returns
    ///
    /// The new DEK version.
    pub fn rotate_key(&mut self) -> Result<u8> {
        let new_version = self
            .current_version
            .checked_add(1)
            .ok_or_else(|| eyre!("Maximum key version reached"))?;

        self.derive_dek(new_version)?;
        self.current_version = new_version;

        Ok(new_version)
    }

    /// Get the current DEK version used for encryption.
    #[must_use]
    pub const fn current_version(&self) -> u8 {
        self.current_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_master_key() -> Vec<u8> {
        // 48-byte key similar to what dstack returns
        vec![0x42; 48]
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();
        let plaintext = b"Hello, encrypted world!";

        let ciphertext = manager.encrypt(plaintext).unwrap();
        let decrypted = manager.decrypt(&ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_ciphertext_format() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();
        let plaintext = b"test";

        let ciphertext = manager.encrypt(plaintext).unwrap();

        // Check format: version(1) + nonce(12) + data(4) + tag(16) = 33
        assert_eq!(
            ciphertext.len(),
            1 + NONCE_SIZE + plaintext.len() + TAG_SIZE
        );
        assert_eq!(ciphertext[0], 1); // version 1
    }

    #[test]
    fn test_key_rotation() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();

        // Encrypt with version 1
        let plaintext1 = b"data with key v1";
        let ciphertext1 = manager.encrypt(plaintext1).unwrap();
        assert_eq!(ciphertext1[0], 1);

        // Rotate to version 2
        let new_version = manager.rotate_key().unwrap();
        assert_eq!(new_version, 2);

        // Encrypt with version 2
        let plaintext2 = b"data with key v2";
        let ciphertext2 = manager.encrypt(plaintext2).unwrap();
        assert_eq!(ciphertext2[0], 2);

        // Both can still be decrypted
        assert_eq!(manager.decrypt(&ciphertext1).unwrap(), plaintext1);
        assert_eq!(manager.decrypt(&ciphertext2).unwrap(), plaintext2);
    }

    #[test]
    fn test_different_plaintexts_produce_different_ciphertexts() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();
        let plaintext = b"same data";

        let ct1 = manager.encrypt(plaintext).unwrap();
        let ct2 = manager.encrypt(plaintext).unwrap();

        // Same plaintext produces different ciphertext due to random nonce
        assert_ne!(ct1, ct2);

        // But both decrypt to the same value
        assert_eq!(manager.decrypt(&ct1).unwrap(), plaintext);
        assert_eq!(manager.decrypt(&ct2).unwrap(), plaintext);
    }

    #[test]
    fn test_empty_master_key_rejected() {
        let result = KeyManager::new(vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_ciphertext_rejected() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();

        // Too short
        let short = vec![1, 2, 3];
        assert!(manager.decrypt(&short).is_err());

        // Valid length but tampered
        let mut tampered = manager.encrypt(b"test").unwrap();
        tampered[15] ^= 0xFF; // Flip a bit
        assert!(manager.decrypt(&tampered).is_err());
    }

    #[test]
    fn test_tampered_version_rejected() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();

        // Rotate so a second valid DEK version exists; flipping the header to
        // that version must still be rejected because it is bound as AAD.
        let _ = manager.rotate_key().unwrap();

        let plaintext = b"data with key v1";
        let mut ciphertext = manager.encrypt(plaintext).unwrap();
        // The encryption used the current version (2); rewrite the header to a
        // different valid version (1) to simulate header tampering.
        assert_eq!(ciphertext[0], 2);
        ciphertext[0] = 1;

        assert!(manager.decrypt(&ciphertext).is_err());
    }

    #[test]
    fn test_tampered_nonce_rejected() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();

        let mut ciphertext = manager.encrypt(b"sensitive").unwrap();
        // Flip a bit in the nonce portion of the header.
        ciphertext[1] ^= 0xFF;

        assert!(manager.decrypt(&ciphertext).is_err());
    }

    #[test]
    fn test_empty_plaintext() {
        let mut manager = KeyManager::new(test_master_key()).unwrap();
        let plaintext = b"";

        let ciphertext = manager.encrypt(plaintext).unwrap();
        let decrypted = manager.decrypt(&ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }
}
