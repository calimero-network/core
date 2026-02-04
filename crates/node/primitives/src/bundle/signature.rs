//! Signature Verification Helpers
//!
//! This module provides helpers for verifying bundle manifest signatures
//! - RFC 8785 JSON canonicalization (JCS)
//! - SHA-256 signing payload computation
//! - Ed25519 signature verification
//! - did:key signerId derivation

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use eyre::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

/// Multicodec indicator for ed25519-pub (0xed01, varint encoded).
const ED25519_PUB_MULTICODEC: [u8; 2] = [0xed, 0x01];

/// Result of manifest signature verification.
#[derive(Debug, Clone)]
pub struct ManifestVerification {
    /// The derived signerId (did:key format).
    pub signer_id: String,
    /// The bundle hash (SHA-256 of canonical manifest bytes).
    pub bundle_hash: [u8; 32],
}

/// Derives a did:key signerId from an Ed25519 public key.
///
/// The did:key format for Ed25519 is:
/// - `did:key:` prefix
/// - multibase base58btc encoding ('z' prefix)
/// - multicodec indicator for ed25519-pub (0xed01)
/// - the 32-byte Ed25519 public key
///
/// # Arguments
/// * `pubkey` - The 32-byte Ed25519 public key
///
/// # Returns
/// The did:key string (e.g., `did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK`)
pub fn derive_signer_id_did_key(pubkey: &[u8; 32]) -> String {
    // Construct the multicodec-prefixed key
    let mut multicodec_key = Vec::with_capacity(2 + 32);
    multicodec_key.extend_from_slice(&ED25519_PUB_MULTICODEC);
    multicodec_key.extend_from_slice(pubkey);

    // Encode with base58btc (multibase 'z' prefix)
    let encoded = bs58::encode(&multicodec_key).into_string();

    format!("did:key:z{}", encoded)
}

/// Canonicalizes a manifest JSON value using RFC 8785 (JCS).
///
/// The `signature` field MUST be excluded before canonicalization.
/// This function clones the input and removes the signature field.
///
/// # Arguments
/// * `manifest_json` - The manifest as a serde_json::Value
///
/// # Returns
/// The canonical JSON bytes
pub fn canonicalize_manifest(manifest_json: &serde_json::Value) -> Result<Vec<u8>> {
    // Clone and remove the signature field for canonicalization
    let mut signing_view = manifest_json.clone();
    if let Some(obj) = signing_view.as_object_mut() {
        obj.remove("signature");
        // Remove all underscore-prefixed fields to prevent signature confusion.
        // The underscore-prefix convention is reserved for transient/unsigned fields that
        // should not be included in signature verification. Known transient fields include:
        // - _binary: Binary data references (not part of canonical JSON)
        // - _overwrite: Overwrite flags for migration artifacts
        // Any future fields starting with underscore are also stripped to prevent signature
        // confusion attacks where fields could be canonicalized and signed but ignored during
        // processing, potentially leading to replay attacks.
        obj.retain(|k, _| !k.starts_with('_'));
    }

    // Canonicalize using RFC 8785 JCS
    let canonical_bytes = serde_json_canonicalizer::to_vec(&signing_view)
        .context("failed to canonicalize manifest JSON")?;

    Ok(canonical_bytes)
}

/// Computes the bundle hash from canonical manifest bytes.
///
/// `bundleHash = sha256(canonical_manifest_bytes)`
///
/// # Arguments
/// * `canonical_bytes` - The RFC 8785 canonical manifest bytes (without signature)
///
/// # Returns
/// The 32-byte SHA-256 hash
pub fn compute_bundle_hash(canonical_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(canonical_bytes);
    hasher.finalize().into()
}

/// Computes the signing payload from canonical manifest bytes.
///
/// `signingPayload = sha256(canonical_manifest_bytes_without_signature)`
/// Note: In v0, signingPayload equals bundleHash.
///
/// # Arguments
/// * `canonical_bytes` - The RFC 8785 canonical manifest bytes (without signature)
///
/// # Returns
/// The 32-byte signing payload
pub fn compute_signing_payload(canonical_bytes: &[u8]) -> [u8; 32] {
    // In v0, signing payload equals bundle hash
    compute_bundle_hash(canonical_bytes)
}

/// Decodes a base64url (no padding) encoded public key.
///
/// # Arguments
/// * `encoded` - The base64url encoded string
///
/// # Returns
/// The 32-byte Ed25519 public key
pub fn decode_public_key(encoded: &str) -> Result<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("invalid base64url encoding for public key")?;

    ensure!(
        bytes.len() == 32,
        "invalid public key length: expected 32 bytes, got {}",
        bytes.len()
    );

    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Decodes a base64url (no padding) encoded signature.
///
/// # Arguments
/// * `encoded` - The base64url encoded string
///
/// # Returns
/// The 64-byte Ed25519 signature
pub fn decode_signature(encoded: &str) -> Result<[u8; 64]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("invalid base64url encoding for signature")?;

    ensure!(
        bytes.len() == 64,
        "invalid signature length: expected 64 bytes, got {}",
        bytes.len()
    );

    let mut sig = [0u8; 64];
    sig.copy_from_slice(&bytes);
    Ok(sig)
}

/// Verifies an Ed25519 signature over a message.
///
/// # Arguments
/// * `signature_bytes` - The 64-byte Ed25519 signature
/// * `public_key_bytes` - The 32-byte Ed25519 public key
/// * `message` - The message that was signed
///
/// # Returns
/// Ok(()) if verification succeeds, Err otherwise
pub fn verify_ed25519(
    signature_bytes: &[u8; 64],
    public_key_bytes: &[u8; 32],
    message: &[u8],
) -> Result<()> {
    let verifying_key =
        VerifyingKey::from_bytes(public_key_bytes).context("invalid Ed25519 public key")?;

    let signature = Signature::from_bytes(signature_bytes);

    verifying_key
        .verify(message, &signature)
        .context("Ed25519 signature verification failed")?;

    Ok(())
}

/// Verifies a bundle manifest signature.
///
/// This function performs the complete verification flow:
/// 1. Extracts and validates the signature object
/// 2. Decodes the base64url public key and signature
/// 3. Canonicalizes the manifest (excluding signature field) using RFC 8785
/// 4. Computes the signing payload (SHA-256 of canonical bytes)
/// 5. Verifies the Ed25519 signature
/// 6. Derives the signerId (did:key) from the public key
/// 7. Validates that the derived signerId matches the manifest's signerId
///
/// # Arguments
/// * `manifest_json` - The complete manifest as a serde_json::Value
///
/// # Returns
/// A `ManifestVerification` containing the verified signerId and bundleHash
pub fn verify_manifest_signature(
    manifest_json: &serde_json::Value,
) -> Result<ManifestVerification> {
    // Extract the signature object
    let signature_obj = manifest_json
        .get("signature")
        .ok_or_else(|| eyre::eyre!("manifest is missing required 'signature' field"))?;

    // Parse the signature fields
    let algorithm = signature_obj
        .get("algorithm")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("signature missing 'algorithm' field"))?;

    // Algorithm comparison is intentionally case-sensitive per specification.
    // Only "ed25519" (lowercase) is supported. Case variations like "ED25519" or "Ed25519"
    // are rejected to prevent potential bypasses if other code paths normalize the string.
    ensure!(
        algorithm == "ed25519",
        "unsupported signature algorithm: '{}', expected 'ed25519'",
        algorithm
    );

    let public_key_b64 = signature_obj
        .get("publicKey")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("signature missing 'publicKey' field"))?;

    let signature_b64 = signature_obj
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("signature missing 'signature' field"))?;

    // Decode the public key and signature from base64url
    let public_key_bytes = decode_public_key(public_key_b64)?;
    let signature_bytes = decode_signature(signature_b64)?;

    // Canonicalize the manifest (excluding signature field)
    let canonical_bytes = canonicalize_manifest(manifest_json)?;

    // Compute the signing payload (SHA-256 of canonical bytes)
    let signing_payload = compute_signing_payload(&canonical_bytes);

    // Verify the Ed25519 signature
    verify_ed25519(&signature_bytes, &public_key_bytes, &signing_payload)?;

    // Derive the signerId from the public key
    let derived_signer_id = derive_signer_id_did_key(&public_key_bytes);

    // Validate that the manifest's signerId matches the derived signerId
    // signerId is required for signed bundles to ensure identity verification
    let manifest_signer_id = manifest_json
        .get("signerId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("bundle manifest is missing required signerId field"))?;

    if manifest_signer_id != derived_signer_id {
        bail!(
            "signerId mismatch: manifest declares '{}' but signature public key derives to '{}'",
            manifest_signer_id,
            derived_signer_id
        );
    }

    // Bundle hash equals signing payload in v0
    let bundle_hash = signing_payload;

    Ok(ManifestVerification {
        signer_id: derived_signer_id,
        bundle_hash,
    })
}

/// Formats a bundle hash as a lowercase hex string.
pub fn format_bundle_hash(hash: &[u8; 32]) -> String {
    hex::encode(hash)
}

/// Signs a manifest JSON value and adds the signature field.
///
/// This function performs the complete signing flow:
/// 1. Adds signerId to the manifest (if not present)
/// 2. Canonicalizes the manifest (excluding signature field)
/// 3. Computes the signing payload (SHA-256)
/// 4. Signs the payload with Ed25519
/// 5. Adds the signature object to the manifest
///
/// # Arguments
/// * `manifest_json` - The manifest as a mutable serde_json::Value
/// * `signing_key` - The Ed25519 signing key
///
/// # Returns
/// The derived signerId (did:key format)
pub fn sign_manifest_json(
    manifest_json: &mut serde_json::Value,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<String> {
    use ed25519_dalek::Signer;

    let verifying_key = signing_key.verifying_key();
    let signer_id = derive_signer_id_did_key(verifying_key.as_bytes());

    // Add signerId to the manifest if not present
    if let Some(obj) = manifest_json.as_object_mut() {
        obj.insert(
            "signerId".to_string(),
            serde_json::Value::String(signer_id.clone()),
        );
    }

    // Canonicalize the manifest (without signature field)
    let canonical_bytes = canonicalize_manifest(manifest_json)?;

    // Compute the signing payload
    let signing_payload = compute_signing_payload(&canonical_bytes);

    // Sign the payload
    let signature = signing_key.sign(&signing_payload);

    // Encode public key and signature as base64url
    let public_key_b64 = URL_SAFE_NO_PAD.encode(verifying_key.as_bytes());
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

    // Add the signature object to the manifest
    if let Some(obj) = manifest_json.as_object_mut() {
        obj.insert(
            "signature".to_string(),
            serde_json::json!({
                "algorithm": "ed25519",
                "publicKey": public_key_b64,
                "signature": signature_b64
            }),
        );
    }

    Ok(signer_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Creates a test manifest JSON with the given values.
    fn create_test_manifest(
        package: &str,
        version: &str,
        signer_id: Option<&str>,
    ) -> serde_json::Value {
        let mut manifest = serde_json::json!({
            "version": "1.0",
            "package": package,
            "appVersion": version,
            "minRuntimeVersion": "1.0.0",
            "resources": [
                {
                    "role": "executable",
                    "path": "app.wasm",
                    "hash": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
                    "size": 1024
                }
            ]
        });

        if let Some(sid) = signer_id {
            manifest["signerId"] = serde_json::Value::String(sid.to_string());
        }

        manifest
    }

    /// Signs a manifest and returns the complete manifest with signature.
    fn sign_manifest(manifest: &mut serde_json::Value, signing_key: &SigningKey) -> Result<()> {
        // Canonicalize the manifest (without signature)
        let canonical_bytes = canonicalize_manifest(manifest)?;

        // Compute the signing payload
        let signing_payload = compute_signing_payload(&canonical_bytes);

        // Sign the payload
        let signature = signing_key.sign(&signing_payload);

        // Encode public key and signature as base64url
        let public_key_b64 = URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes());
        let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

        // Add the signature object to the manifest
        manifest["signature"] = serde_json::json!({
            "algorithm": "ed25519",
            "publicKey": public_key_b64,
            "signature": signature_b64
        });

        Ok(())
    }

    #[test]
    fn test_derive_signer_id_did_key() {
        // Test with a known public key
        let pubkey: [u8; 32] = [
            0x3b, 0x6a, 0x27, 0xbc, 0xce, 0xb6, 0xa4, 0x2d, 0x62, 0xa3, 0xa8, 0xd0, 0x2a, 0x6f,
            0x0d, 0x73, 0x65, 0x32, 0x15, 0x77, 0x1d, 0xe2, 0x43, 0xa6, 0x3a, 0xc0, 0x48, 0xa1,
            0x8b, 0x59, 0xda, 0x29,
        ];

        let signer_id = derive_signer_id_did_key(&pubkey);

        // Verify the format
        assert!(signer_id.starts_with("did:key:z"));
        assert!(signer_id.len() > 10);

        // The same public key should always produce the same signerId
        let signer_id_2 = derive_signer_id_did_key(&pubkey);
        assert_eq!(signer_id, signer_id_2);
    }

    #[test]
    fn test_canonicalize_manifest_removes_signature() {
        let manifest = serde_json::json!({
            "version": "1.0",
            "package": "com.example.app",
            "signature": {
                "algorithm": "ed25519",
                "publicKey": "test",
                "signature": "test"
            }
        });

        let canonical = canonicalize_manifest(&manifest).unwrap();
        let canonical_str = String::from_utf8(canonical).unwrap();

        // The canonical form should not contain the signature field
        assert!(!canonical_str.contains("signature"));
        // But should contain other fields
        assert!(canonical_str.contains("package"));
        assert!(canonical_str.contains("version"));
    }

    #[test]
    fn test_canonicalize_manifest_removes_transient_fields() {
        let manifest = serde_json::json!({
            "version": "1.0",
            "package": "com.example.app",
            "_binary": "some_value",
            "_overwrite": true
        });

        let canonical = canonicalize_manifest(&manifest).unwrap();
        let canonical_str = String::from_utf8(canonical).unwrap();

        // Transient fields should be removed
        assert!(!canonical_str.contains("_binary"));
        assert!(!canonical_str.contains("_overwrite"));
    }

    #[test]
    fn test_canonicalize_manifest_key_ordering() {
        // RFC 8785 requires lexicographic key ordering
        let manifest = serde_json::json!({
            "z_field": "last",
            "a_field": "first",
            "m_field": "middle"
        });

        let canonical = canonicalize_manifest(&manifest).unwrap();
        let canonical_str = String::from_utf8(canonical).unwrap();

        // Keys should be in lexicographic order
        let a_pos = canonical_str.find("a_field").unwrap();
        let m_pos = canonical_str.find("m_field").unwrap();
        let z_pos = canonical_str.find("z_field").unwrap();

        assert!(a_pos < m_pos);
        assert!(m_pos < z_pos);
    }

    #[test]
    fn test_compute_bundle_hash() {
        let canonical_bytes = b"test manifest content";
        let hash = compute_bundle_hash(canonical_bytes);

        // Hash should be 32 bytes
        assert_eq!(hash.len(), 32);

        // Same input should produce same hash
        let hash_2 = compute_bundle_hash(canonical_bytes);
        assert_eq!(hash, hash_2);

        // Different input should produce different hash
        let different_bytes = b"different content";
        let different_hash = compute_bundle_hash(different_bytes);
        assert_ne!(hash, different_hash);
    }

    #[test]
    fn test_decode_public_key_valid() {
        // Generate a test key
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let public_key = signing_key.verifying_key();

        // Encode and decode
        let encoded = URL_SAFE_NO_PAD.encode(public_key.as_bytes());
        let decoded = decode_public_key(&encoded).unwrap();

        assert_eq!(decoded, *public_key.as_bytes());
    }

    #[test]
    fn test_decode_public_key_invalid_length() {
        let short_key = URL_SAFE_NO_PAD.encode(&[0u8; 16]);
        let result = decode_public_key(&short_key);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid public key length"));
    }

    #[test]
    fn test_decode_signature_valid() {
        let sig_bytes = [0u8; 64];
        let encoded = URL_SAFE_NO_PAD.encode(&sig_bytes);
        let decoded = decode_signature(&encoded).unwrap();
        assert_eq!(decoded, sig_bytes);
    }

    #[test]
    fn test_decode_signature_invalid_length() {
        let short_sig = URL_SAFE_NO_PAD.encode(&[0u8; 32]);
        let result = decode_signature(&short_sig);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid signature length"));
    }

    #[test]
    fn test_verify_ed25519_valid_signature() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let message = b"test message";

        let signature = signing_key.sign(message);

        let result = verify_ed25519(
            &signature.to_bytes(),
            signing_key.verifying_key().as_bytes(),
            message,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_ed25519_invalid_signature() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let message = b"test message";
        let wrong_message = b"wrong message";

        // Sign one message but verify with another
        let signature = signing_key.sign(message);

        let result = verify_ed25519(
            &signature.to_bytes(),
            signing_key.verifying_key().as_bytes(),
            wrong_message,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_verify_manifest_signature_valid() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

        let mut manifest = create_test_manifest("com.example.app", "1.0.0", Some(&signer_id));

        sign_manifest(&mut manifest, &signing_key).unwrap();

        let result = verify_manifest_signature(&manifest);
        assert!(result.is_ok());

        let verification = result.unwrap();
        assert_eq!(verification.signer_id, signer_id);
    }

    #[test]
    fn test_verify_manifest_signature_missing_signature() {
        let manifest = create_test_manifest("com.example.app", "1.0.0", None);

        let result = verify_manifest_signature(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required 'signature' field"));
    }

    #[test]
    fn test_verify_manifest_signature_wrong_algorithm() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());
        let mut manifest = create_test_manifest("com.example.app", "1.0.0", Some(&signer_id));
        manifest["signature"] = serde_json::json!({
            "algorithm": "rsa",
            "publicKey": "test",
            "signature": "test"
        });

        let result = verify_manifest_signature(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unsupported signature algorithm"));
    }

    #[test]
    fn test_verify_manifest_signature_missing_signer_id() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        // Create manifest without signerId (but with signature)
        let mut manifest = create_test_manifest("com.example.app", "1.0.0", None);
        sign_manifest(&mut manifest, &signing_key).unwrap();

        let result = verify_manifest_signature(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required signerId field"));
    }

    #[test]
    fn test_verify_manifest_signature_signer_id_mismatch() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());

        // Use a wrong signerId
        let mut manifest = create_test_manifest(
            "com.example.app",
            "1.0.0",
            Some("did:key:z6MkWRONGKEY123456789"),
        );

        sign_manifest(&mut manifest, &signing_key).unwrap();

        let result = verify_manifest_signature(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("signerId mismatch"));
    }

    #[test]
    fn test_verify_manifest_signature_invalid_signature() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

        let mut manifest = create_test_manifest("com.example.app", "1.0.0", Some(&signer_id));

        // Sign with the correct key
        sign_manifest(&mut manifest, &signing_key).unwrap();

        // Tamper with the manifest after signing
        manifest["appVersion"] = serde_json::Value::String("2.0.0".to_string());

        let result = verify_manifest_signature(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("signature verification failed"));
    }

    #[test]
    fn test_signer_id_derivation_is_stable() {
        // Generate a key and derive signerId multiple times
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let verifying_key = signing_key.verifying_key();
        let pubkey = verifying_key.as_bytes();

        let signer_id_1 = derive_signer_id_did_key(pubkey);
        let signer_id_2 = derive_signer_id_did_key(pubkey);
        let signer_id_3 = derive_signer_id_did_key(pubkey);

        assert_eq!(signer_id_1, signer_id_2);
        assert_eq!(signer_id_2, signer_id_3);
    }

    #[test]
    fn test_bundle_hash_is_deterministic() {
        let signing_key = SigningKey::generate(&mut rand::thread_rng());
        let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

        let manifest = create_test_manifest("com.example.app", "1.0.0", Some(&signer_id));

        // Compute hash multiple times
        let canonical_1 = canonicalize_manifest(&manifest).unwrap();
        let hash_1 = compute_bundle_hash(&canonical_1);

        let canonical_2 = canonicalize_manifest(&manifest).unwrap();
        let hash_2 = compute_bundle_hash(&canonical_2);

        assert_eq!(hash_1, hash_2);
        assert_eq!(canonical_1, canonical_2);
    }

    #[test]
    fn test_format_bundle_hash() {
        let hash: [u8; 32] = [
            0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0xa7, 0xb8, 0xc9, 0xd0, 0xe1, 0xf2, 0xa3, 0xb4,
            0xc5, 0xd6, 0xe7, 0xf8, 0xa9, 0xb0, 0xc1, 0xd2, 0xe3, 0xf4, 0xa5, 0xb6, 0xc7, 0xd8,
            0xe9, 0xf0, 0xa1, 0xb2,
        ];

        let formatted = format_bundle_hash(&hash);

        // Should be lowercase hex
        assert_eq!(formatted.len(), 64);
        assert!(formatted.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(formatted.chars().all(|c| !c.is_uppercase()));
    }
}
