//! Manifest signature helpers: JCS canonicalization, domain-tagged signing payload,
//! Ed25519 verification and did:key signerId derivation.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use eyre::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

/// Multicodec indicator for ed25519-pub (0xed01, varint encoded).
const ED25519_PUB_MULTICODEC: [u8; 2] = [0xed, 0x01];

/// signerId of the public development key (`meroctl app sign --dev`, `app pack` without `--key`).
/// Anyone can sign as it, so a production node refuses packages it signed.
pub const DEV_SIGNER_ID: &str = "did:key:z6MknF3p5L5FDHJQ7FREUapuX4Wmp4MtF6WrHYaXS2B3eZQd";

#[derive(Debug, Clone)]
pub struct ManifestVerification {
    /// The derived signerId (did:key format).
    pub signer_id: String,
    /// The bundle hash (SHA-256 of canonical manifest bytes).
    pub bundle_hash: [u8; 32],
}

/// did:key of an Ed25519 key: base58btc ('z' multibase) of the 0xed01 multicodec prefix and the
/// 32 key bytes.
pub fn derive_signer_id_did_key(pubkey: &[u8; 32]) -> String {
    let mut multicodec_key = Vec::with_capacity(2 + 32);
    multicodec_key.extend_from_slice(&ED25519_PUB_MULTICODEC);
    multicodec_key.extend_from_slice(pubkey);

    let encoded = bs58::encode(&multicodec_key).into_string();

    format!("did:key:z{encoded}")
}

/// RFC 8785 canonical bytes of the manifest without `signature` and `_`-prefixed fields.
pub fn canonicalize_manifest(manifest_json: &serde_json::Value) -> Result<Vec<u8>> {
    let mut signing_view = manifest_json.clone();
    if let Some(obj) = signing_view.as_object_mut() {
        obj.remove("signature");
        // Unsigned `_` fields (e.g. _binary, _overwrite) are stripped so a field that is signed but
        // ignored at processing time cannot be used for signature confusion or replay.
        obj.retain(|k, _| !k.starts_with('_'));
    }

    let canonical_bytes = serde_json_canonicalizer::to_vec(&signing_view)
        .context("failed to canonicalize manifest JSON")?;

    Ok(canonical_bytes)
}

/// `bundleHash = sha256(canonical_manifest_bytes)`.
pub fn compute_bundle_hash(canonical_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(canonical_bytes);
    hasher.finalize().into()
}

/// Prefixed to the canonical bytes before signing so a manifest signature cannot be replayed
/// as a signature over another message; bump the version when the signed bytes change.
pub const MANIFEST_SIGNING_DOMAIN: &[u8] = b"calimero.bundle.manifest.v1\0";

/// `sha256(MANIFEST_SIGNING_DOMAIN || canonical_bytes)`, the digest Ed25519 signs.
/// The bundle hash stays plain SHA-256 so a bundle's identity does not depend on the tag.
pub fn compute_signing_payload(canonical_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(MANIFEST_SIGNING_DOMAIN);
    hasher.update(canonical_bytes);
    hasher.finalize().into()
}

/// Decodes a base64url (no padding) encoded public key.
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

pub fn verify_ed25519(
    signature_bytes: &[u8; 64],
    public_key_bytes: &[u8; 32],
    message: &[u8],
) -> Result<()> {
    let verifying_key =
        VerifyingKey::from_bytes(public_key_bytes).context("invalid Ed25519 public key")?;

    let signature = Signature::from_bytes(signature_bytes);

    // Strict verification also refuses small-order keys and non-canonical signatures.
    verifying_key
        .verify_strict(message, &signature)
        .context("Ed25519 signature verification failed")?;

    Ok(())
}

/// Verifies the signature and that the manifest's signerId is the did:key of the signing key.
pub fn verify_manifest_signature(
    manifest_json: &serde_json::Value,
) -> Result<ManifestVerification> {
    let signature_obj = manifest_json
        .get("signature")
        .ok_or_else(|| eyre::eyre!("manifest is missing required 'signature' field"))?;

    let algorithm = signature_obj
        .get("algorithm")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("signature missing 'algorithm' field"))?;

    // Case-sensitive on purpose: rejecting "ED25519" avoids bypasses if another path
    // normalizes the string.
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

    let public_key_bytes = decode_public_key(public_key_b64)?;
    let signature_bytes = decode_signature(signature_b64)?;

    let canonical_bytes = canonicalize_manifest(manifest_json)?;

    let signing_payload = compute_signing_payload(&canonical_bytes);

    verify_ed25519(&signature_bytes, &public_key_bytes, &signing_payload)?;

    let derived_signer_id = derive_signer_id_did_key(&public_key_bytes);

    // A signed bundle must declare the signerId its key derives to.
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

    let bundle_hash = compute_bundle_hash(&canonical_bytes);

    Ok(ManifestVerification {
        signer_id: derived_signer_id,
        bundle_hash,
    })
}

/// Formats a bundle hash as a lowercase hex string.
pub fn format_bundle_hash(hash: &[u8; 32]) -> String {
    hex::encode(hash)
}

/// Signs the manifest in place and returns the signerId.
pub fn sign_manifest_json(
    manifest_json: &mut serde_json::Value,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<String> {
    use ed25519_dalek::Signer;

    let verifying_key = signing_key.verifying_key();
    let signer_id = derive_signer_id_did_key(verifying_key.as_bytes());

    // Overwrite any existing signerId so it always matches the key, as `meroctl app sign` does.
    if let Some(obj) = manifest_json.as_object_mut() {
        obj.insert(
            "signerId".to_string(),
            serde_json::Value::String(signer_id.clone()),
        );
    }

    let canonical_bytes = canonicalize_manifest(manifest_json)?;

    let signing_payload = compute_signing_payload(&canonical_bytes);

    let signature = signing_key.sign(&signing_payload);

    let public_key_b64 = URL_SAFE_NO_PAD.encode(verifying_key.as_bytes());
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

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
    use ed25519_dalek::{Signer, SigningKey, Verifier};

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

    fn sign_manifest(manifest: &mut serde_json::Value, signing_key: &SigningKey) -> Result<()> {
        let canonical_bytes = canonicalize_manifest(manifest)?;

        let signing_payload = compute_signing_payload(&canonical_bytes);

        let signature = signing_key.sign(&signing_payload);

        let public_key_b64 = URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes());
        let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

        manifest["signature"] = serde_json::json!({
            "algorithm": "ed25519",
            "publicKey": public_key_b64,
            "signature": signature_b64
        });

        Ok(())
    }

    #[test]
    fn test_derive_signer_id_did_key() {
        let pubkey: [u8; 32] = [
            0x3b, 0x6a, 0x27, 0xbc, 0xce, 0xb6, 0xa4, 0x2d, 0x62, 0xa3, 0xa8, 0xd0, 0x2a, 0x6f,
            0x0d, 0x73, 0x65, 0x32, 0x15, 0x77, 0x1d, 0xe2, 0x43, 0xa6, 0x3a, 0xc0, 0x48, 0xa1,
            0x8b, 0x59, 0xda, 0x29,
        ];

        let signer_id = derive_signer_id_did_key(&pubkey);

        assert!(signer_id.starts_with("did:key:z"));
        assert!(signer_id.len() > 10);

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

        assert!(!canonical_str.contains("signature"));
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

        assert!(!canonical_str.contains("_binary"));
        assert!(!canonical_str.contains("_overwrite"));
    }

    // serde_json prints these numbers differently (and keeps key order with `preserve_order`),
    // so plain serialization cannot pass for canonicalization.
    #[test]
    fn the_canonical_form_is_rfc_8785() {
        let manifest: serde_json::Value = serde_json::from_str(
            r#"{"z_field": 1.0, "a_field": [2.50, 1e3], "m_field": "middle"}"#,
        )
        .unwrap();

        let canonical = canonicalize_manifest(&manifest).unwrap();

        assert_eq!(
            String::from_utf8(canonical).unwrap(),
            r#"{"a_field":[2.5,1000],"m_field":"middle","z_field":1}"#
        );
    }

    #[test]
    fn test_decode_public_key_valid() {
        let signing_key = SigningKey::generate(&mut rand::rng());
        let public_key = signing_key.verifying_key();

        let encoded = URL_SAFE_NO_PAD.encode(public_key.as_bytes());
        let decoded = decode_public_key(&encoded).unwrap();

        assert_eq!(decoded, *public_key.as_bytes());
    }

    #[test]
    fn test_decode_public_key_invalid_length() {
        let short_key = URL_SAFE_NO_PAD.encode([0u8; 16]);
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
        let encoded = URL_SAFE_NO_PAD.encode(sig_bytes);
        let decoded = decode_signature(&encoded).unwrap();
        assert_eq!(decoded, sig_bytes);
    }

    #[test]
    fn test_decode_signature_invalid_length() {
        let short_sig = URL_SAFE_NO_PAD.encode([0u8; 32]);
        let result = decode_signature(&short_sig);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid signature length"));
    }

    #[test]
    fn test_verify_ed25519_invalid_signature() {
        let signing_key = SigningKey::generate(&mut rand::rng());
        let message = b"test message";
        let wrong_message = b"wrong message";

        let signature = signing_key.sign(message);

        let result = verify_ed25519(
            &signature.to_bytes(),
            signing_key.verifying_key().as_bytes(),
            wrong_message,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_verify_ed25519_rejects_small_order_public_key() {
        // Identity point with R = identity, S = 0 satisfies the equation for any message;
        // only strict verification refuses it.
        let mut identity = [0u8; 32];
        identity[0] = 1;
        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(&identity);

        let key = VerifyingKey::from_bytes(&identity).unwrap();
        assert!(
            key.verify(b"any message", &Signature::from_bytes(&signature))
                .is_ok(),
            "precondition: non-strict verification accepts this forgery"
        );

        let result = verify_ed25519(&signature, &identity, b"any message");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Ed25519 signature verification failed"));
    }

    #[test]
    fn test_verify_manifest_signature_valid() {
        let signing_key = SigningKey::generate(&mut rand::rng());
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
        let signing_key = SigningKey::generate(&mut rand::rng());
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
        let signing_key = SigningKey::generate(&mut rand::rng());
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
        let signing_key = SigningKey::generate(&mut rand::rng());

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
        let signing_key = SigningKey::generate(&mut rand::rng());
        let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

        let mut manifest = create_test_manifest("com.example.app", "1.0.0", Some(&signer_id));

        sign_manifest(&mut manifest, &signing_key).unwrap();

        manifest["appVersion"] = serde_json::Value::String("2.0.0".to_string());

        let result = verify_manifest_signature(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("signature verification failed"));
    }

    /// The same vector is pinned in the registry's verifier tests, so the two
    /// implementations cannot drift apart on what is signed.
    #[test]
    fn the_signing_payload_matches_the_cross_implementation_vector() {
        let manifest = create_test_manifest("com.example.app", "1.0.0", None);
        let canonical = canonicalize_manifest(&manifest).unwrap();
        assert_eq!(
            String::from_utf8(canonical.clone()).unwrap(),
            r#"{"appVersion":"1.0.0","minRuntimeVersion":"1.0.0","package":"com.example.app","resources":[{"hash":"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2","path":"app.wasm","role":"executable","size":1024}],"version":"1.0"}"#
        );

        assert_eq!(
            hex::encode(compute_bundle_hash(&canonical)),
            "3dec7675feff863a1346c157e74ace6b77aaaf3483f2a35ecd419463c54f52a8"
        );
        let payload = compute_signing_payload(&canonical);
        assert_eq!(
            hex::encode(payload),
            "ce52b6437aa6e385d6ca6704775a2c7a89a5623e4f9520a7e411628d0ccf6c70"
        );

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        assert_eq!(
            URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes()),
            "6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw"
        );
        assert_eq!(
            URL_SAFE_NO_PAD.encode(signing_key.sign(&payload).to_bytes()),
            "E-QrACBF4wcY_y90BgszUPa8Gu2MD1fPycE_PkLnue3iFMXzCXTRYmNUJSBbq7IBoBSYDh81dBr-RyJzIOaXCw"
        );
    }

    /// A signature over the bare SHA-256 of the canonical manifest, the way
    /// signers worked before the payload carried a domain tag.
    #[test]
    fn an_untagged_signature_is_refused() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());
        let mut manifest = create_test_manifest("com.example.app", "1.0.0", Some(&signer_id));

        let canonical_bytes = canonicalize_manifest(&manifest).unwrap();
        let untagged_payload = compute_bundle_hash(&canonical_bytes);
        let signature = signing_key.sign(&untagged_payload);
        manifest["signature"] = serde_json::json!({
            "algorithm": "ed25519",
            "publicKey": URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes()),
            "signature": URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        });

        let err = verify_manifest_signature(&manifest).unwrap_err();
        assert!(
            format!("{err:#}").contains("signature verification failed"),
            "got: {err:#}"
        );
    }

    #[test]
    fn test_format_bundle_hash() {
        let hash: [u8; 32] = [
            0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0xa7, 0xb8, 0xc9, 0xd0, 0xe1, 0xf2, 0xa3, 0xb4,
            0xc5, 0xd6, 0xe7, 0xf8, 0xa9, 0xb0, 0xc1, 0xd2, 0xe3, 0xf4, 0xa5, 0xb6, 0xc7, 0xd8,
            0xe9, 0xf0, 0xa1, 0xb2,
        ];

        let formatted = format_bundle_hash(&hash);

        assert_eq!(formatted.len(), 64);
        assert!(formatted.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(formatted.chars().all(|c| !c.is_uppercase()));
    }
}
