//! mero-sign: library for signing Calimero bundle manifests
//!
//! Implements the signing flow:
//! - Generate Ed25519 keypairs
//! - Sign manifests using RFC 8785 canonicalization
//! - Derive did:key signerId from public keys

use std::fs;
use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
pub use calimero_bundle::{
    canonicalize_manifest, compute_signing_payload, derive_signer_id_did_key,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use eyre::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Well-known development signing key seed.
///
/// Derived deterministically: `SHA-256("calimero-dev-signing-key-v1")`.
/// This key is PUBLIC and provides no security. It exists solely as a
/// marker for development builds, analogous to Android's `debug.keystore`.
const DEV_SEED: [u8; 32] = [
    0x8c, 0xb7, 0xae, 0x25, 0x1b, 0x4d, 0xa5, 0x5a, 0xec, 0xc3, 0x87, 0x5f, 0x4c, 0x0a, 0x71, 0x2c,
    0xe2, 0xb3, 0x4c, 0xb0, 0x6d, 0x47, 0x71, 0xe7, 0x13, 0xf2, 0xbb, 0x32, 0x89, 0xd0, 0xf7, 0xb4,
];

/// Key file format
#[derive(Debug, Serialize, Deserialize)]
struct KeyFile {
    /// Base64url encoded 32-byte private key seed
    private_key: String,
    /// Base64url encoded 32-byte public key
    public_key: String,
    /// The derived did:key signerId
    signer_id: String,
}

/// Signature object in manifest
#[derive(Debug, Serialize, Deserialize)]
struct SignatureObject {
    algorithm: String,
    #[serde(rename = "publicKey")]
    public_key: String,
    signature: String,
}

/// Load a signing key from a key file
pub fn load_signing_key(key_path: &Path) -> Result<SigningKey> {
    let key_content = fs::read_to_string(key_path)
        .with_context(|| format!("failed to read key file: {}", key_path.display()))?;

    let key_file: KeyFile = serde_json::from_str(&key_content)
        .with_context(|| format!("failed to parse key file: {}", key_path.display()))?;

    let private_key_bytes = URL_SAFE_NO_PAD
        .decode(&key_file.private_key)
        .context("failed to decode private key from base64url")?;

    if private_key_bytes.len() != 32 {
        bail!(
            "invalid private key length: expected 32 bytes, got {}",
            private_key_bytes.len()
        );
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&private_key_bytes);

    Ok(SigningKey::from_bytes(&seed))
}

pub fn dev_signing_key() -> SigningKey {
    SigningKey::from_bytes(&DEV_SEED)
}

/// Returns the well-known dev signer_id (derived from DEV_SEED at runtime).
pub fn dev_signer_id() -> String {
    let key = dev_signing_key();
    derive_signer_id_did_key(key.verifying_key().as_bytes())
}

/// True when `key` is the well-known development key. Derived from the public
/// half so the dev warning cannot be suppressed by how a caller invokes signing
/// (e.g. a MERO_SIGN_KEY file that happens to hold the dev seed).
pub fn is_dev_key(key: &SigningKey) -> bool {
    key.verifying_key().as_bytes() == dev_signing_key().verifying_key().as_bytes()
}

/// Sign a manifest file in place.
pub fn sign_manifest(manifest_path: &Path, signing_key: &SigningKey) -> Result<()> {
    let is_dev = is_dev_key(signing_key);
    let verifying_key = signing_key.verifying_key();
    let signer_id = derive_signer_id_did_key(verifying_key.as_bytes());

    // Read the manifest
    let manifest_content = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read manifest: {}", manifest_path.display()))?;

    let mut manifest: serde_json::Value = serde_json::from_str(&manifest_content)
        .with_context(|| format!("failed to parse manifest: {}", manifest_path.display()))?;

    // A signer must not rewrite the document it signs: `minRuntimeVersion` is
    // required on the node's BundleManifest, so a manifest missing it is refused.
    if manifest
        .get("minRuntimeVersion")
        .and_then(|v| v.as_str())
        .is_none()
    {
        bail!("manifest is missing required field `minRuntimeVersion`");
    }

    // Add signerId to the manifest
    if let Some(obj) = manifest.as_object_mut() {
        obj.insert(
            "signerId".to_string(),
            serde_json::Value::String(signer_id.clone()),
        );
    }

    // Canonicalize the manifest (without signature)
    let canonical_bytes = canonicalize_manifest(&manifest)?;

    // Compute the signing payload
    let signing_payload = compute_signing_payload(&canonical_bytes);

    // Sign the payload
    let signature = signing_key.sign(&signing_payload);

    // Encode public key and signature as base64url
    let public_key_b64 = URL_SAFE_NO_PAD.encode(verifying_key.as_bytes());
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

    // Add the signature object to the manifest
    let signature_obj = SignatureObject {
        algorithm: "ed25519".to_string(),
        public_key: public_key_b64,
        signature: signature_b64,
    };

    if let Some(obj) = manifest.as_object_mut() {
        obj.insert(
            "signature".to_string(),
            serde_json::to_value(&signature_obj)?,
        );
    }

    // Write the signed manifest back
    let signed_manifest = serde_json::to_string_pretty(&manifest)?;
    fs::write(manifest_path, signed_manifest).with_context(|| {
        format!(
            "failed to write signed manifest: {}",
            manifest_path.display()
        )
    })?;

    if is_dev {
        eprintln!(
            "\u{26a0}  Signed with DEVELOPMENT key. This bundle cannot be published to the registry."
        );
    } else {
        eprintln!("Signed manifest: {}", manifest_path.display());
    }
    eprintln!("  signerId: {signer_id}");

    Ok(())
}

/// Verify a signed manifest end to end: re-canonicalize (RFC 8785, signature and
/// underscore fields excluded), hash, and check the ed25519 signature against the
/// embedded public key, then confirm `signerId` derives from that same key.
///
/// Returns `Ok(true)` when the signature verifies, `Ok(false)` when it is present
/// but invalid, and `Err` when the manifest lacks a well-formed `signature`
/// object. Mirrors the node's `verify_manifest_signature` gate.
pub fn verify_manifest(manifest: &serde_json::Value) -> Result<bool> {
    let signature_obj = manifest
        .get("signature")
        .ok_or_else(|| eyre::eyre!("manifest is missing the 'signature' field"))?;

    let algorithm = signature_obj
        .get("algorithm")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("signature missing 'algorithm' field"))?;
    if algorithm != "ed25519" {
        bail!("unsupported signature algorithm: '{algorithm}', expected 'ed25519'");
    }

    let public_key_b64 = signature_obj
        .get("publicKey")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("signature missing 'publicKey' field"))?;
    let signature_b64 = signature_obj
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("signature missing 'signature' field"))?;

    let public_key_bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(public_key_b64)
        .context("failed to decode publicKey from base64url")?
        .try_into()
        .map_err(|_| eyre::eyre!("publicKey is not 32 bytes"))?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .context("failed to decode signature from base64url")?;

    let verifying_key = VerifyingKey::from_bytes(&public_key_bytes)
        .context("publicKey is not a valid ed25519 point")?;
    let signature = Signature::from_slice(&signature_bytes)
        .context("signature is not a valid ed25519 signature")?;

    let canonical_bytes = canonicalize_manifest(manifest)?;
    let signing_payload = compute_signing_payload(&canonical_bytes);

    if verifying_key.verify(&signing_payload, &signature).is_err() {
        return Ok(false);
    }

    // A valid signature over a mismatched signerId is still a forgery vector.
    if let Some(declared) = manifest.get("signerId").and_then(|v| v.as_str()) {
        if declared != derive_signer_id_did_key(&public_key_bytes) {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Generate a new Ed25519 keypair. Refuses to replace an existing file unless
/// `force`: a signing key is unrecoverable, and losing one means no further
/// updates can be published for any app already installed under its signerId.
pub fn generate_key(output_path: &Path, force: bool) -> Result<()> {
    let mut rng = rand::thread_rng();
    let signing_key = SigningKey::generate(&mut rng);
    let verifying_key: VerifyingKey = signing_key.verifying_key();

    let private_key_b64 = URL_SAFE_NO_PAD.encode(signing_key.to_bytes());
    let public_key_b64 = URL_SAFE_NO_PAD.encode(verifying_key.as_bytes());
    let signer_id = derive_signer_id_did_key(verifying_key.as_bytes());

    let key_file = KeyFile {
        private_key: private_key_b64,
        public_key: public_key_b64,
        signer_id: signer_id.clone(),
    };

    let key_json = serde_json::to_string_pretty(&key_file)?;
    write_private_key(output_path, &key_json, force).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            eyre::eyre!(
                "{} already exists, and replacing a signing key is irreversible: apps \
                 installed under its signerId could never be updated again. Pick another \
                 path, or pass --force to overwrite it.",
                output_path.display()
            )
        } else {
            eyre::eyre!("failed to write key file {}: {e}", output_path.display())
        }
    })?;

    eprintln!("Generated new keypair: {}", output_path.display());
    eprintln!("  signerId: {signer_id}");

    Ok(())
}

/// Write the private-key JSON to a file this process creates, never to one that
/// already exists: an existing file would keep its old permissions, and
/// `create_new` refuses to write through a pre-placed symlink. On unix the seed
/// is owner-only (0600) from its first byte, since `.mode()` applies at creation
/// only. Other platforms inherit the default ACL, and say so. Without `force`,
/// `create_new` leaves an existing key untouched and reports `AlreadyExists`.
fn write_private_key(path: &Path, contents: &str, force: bool) -> std::io::Result<()> {
    use std::io::Write as _;

    if force {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }

    let mut options = fs::OpenOptions::new();
    let _ = options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let _ = options.mode(0o600);
    }
    // std cannot set a Windows ACL, so the file inherits the directory's. Say so
    // rather than leaving the operator to assume the key is protected.
    #[cfg(not(unix))]
    eprintln!(
        "warning: {} inherits the directory's default permissions on this platform; \
         keep it out of shared or world-readable locations",
        path.display()
    );

    options.open(path)?.write_all(contents.as_bytes())
}

/// Derive the signerId from a key file
pub fn derive_signer_id(key_path: &Path) -> Result<()> {
    let key_content = fs::read_to_string(key_path)
        .with_context(|| format!("failed to read key file: {}", key_path.display()))?;

    let key_file: KeyFile = serde_json::from_str(&key_content)
        .with_context(|| format!("failed to parse key file: {}", key_path.display()))?;

    println!("{}", key_file.signer_id);

    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::Verifier;
    use serde_json::json;

    use super::*;

    fn base64_decode_url(s: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(s).unwrap()
    }

    #[test]
    fn canonicalize_drops_signature_and_private_fields() {
        let m = json!({"b": 1, "a": 2, "signature": {"x": 1}, "_local": true});
        let canon = canonicalize_manifest(&m).unwrap();
        let s = String::from_utf8(canon).unwrap();
        assert_eq!(s, r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn dev_sign_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        std::fs::write(
            &path,
            r#"{"package":"com.example.app","appVersion":"1.0.0","minRuntimeVersion":"0.1.0"}"#,
        )
        .unwrap();

        let key = dev_signing_key();
        sign_manifest(&path, &key).unwrap();

        let signed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(signed["signature"]["algorithm"], "ed25519");
        assert_eq!(signed["signerId"], dev_signer_id());

        // signature must verify over the SHA-256 digest of the re-canonicalized manifest
        let canon = canonicalize_manifest(&signed).unwrap();
        let payload = compute_signing_payload(&canon);
        let sig_bytes = base64_decode_url(signed["signature"]["signature"].as_str().unwrap());
        let sig = ed25519_dalek::Signature::from_slice(&sig_bytes).unwrap();
        key.verifying_key().verify(&payload, &sig).unwrap();
    }

    #[test]
    fn signing_refuses_a_manifest_missing_min_runtime_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let original = r#"{"package":"com.example.app","appVersion":"1.0.0"}"#;
        std::fs::write(&path, original).unwrap();

        let err = sign_manifest(&path, &dev_signing_key()).unwrap_err();
        assert!(err.to_string().contains("minRuntimeVersion"));

        // A signer that refuses must leave the document exactly as it found it.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn verify_manifest_accepts_valid_and_rejects_tampered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        std::fs::write(
            &path,
            r#"{"package":"com.example.app","appVersion":"1.0.0","minRuntimeVersion":"0.1.0","wasm":{"path":"app.wasm","size":10}}"#,
        )
        .unwrap();

        sign_manifest(&path, &dev_signing_key()).unwrap();
        let signed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(verify_manifest(&signed).unwrap());

        // Mutating a signed field must invalidate the signature.
        let mut tampered = signed.clone();
        tampered["appVersion"] = json!("9.9.9");
        assert!(!verify_manifest(&tampered).unwrap());

        // A signerId that no longer derives from the public key is a forgery.
        let mut spoofed = signed;
        spoofed["signerId"] = json!("did:key:zSpoofed");
        assert!(!verify_manifest(&spoofed).unwrap());

        // No signature object at all is a hard error, not a silent `false`.
        assert!(verify_manifest(&json!({"package": "x"})).is_err());
    }

    #[test]
    fn verify_manifest_rejects_signer_id_mismatched_with_embedded_key() {
        let mut rng = rand::thread_rng();
        let key_a = SigningKey::generate(&mut rng);
        let key_b = SigningKey::generate(&mut rng);

        // Builds and signs a manifest with `signing_key`, claiming `signer_id`.
        // signerId is part of the canonicalized (signed) payload, so it must be
        // baked in before signing rather than patched in afterward.
        let sign_with = |signing_key: &SigningKey, signer_id: &str| -> serde_json::Value {
            let mut manifest = json!({
                "package": "com.example.app",
                "appVersion": "1.0.0",
                "signerId": signer_id,
            });
            let canonical_bytes = canonicalize_manifest(&manifest).unwrap();
            let signing_payload = compute_signing_payload(&canonical_bytes);
            let signature = signing_key.sign(&signing_payload);
            manifest["signature"] = serde_json::to_value(SignatureObject {
                algorithm: "ed25519".to_string(),
                public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes()),
                signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            })
            .unwrap();
            manifest
        };

        // Key A truthfully claims its own identity: signature verifies and
        // signerId matches the embedded key.
        let honest_id = derive_signer_id_did_key(key_a.verifying_key().as_bytes());
        let honest = sign_with(&key_a, &honest_id);
        assert!(verify_manifest(&honest).unwrap());

        // Key A signs the identical structure but falsely claims key B's
        // identity. The ed25519 signature still verifies (it signs over
        // whatever bytes it is given, lie included), so the rejection below
        // can only come from the signerId-vs-embedded-key check, not a
        // signature failure - the "honest" case above proves the signature
        // path alone succeeds for this exact signing key.
        let claimed_id = derive_signer_id_did_key(key_b.verifying_key().as_bytes());
        let forged = sign_with(&key_a, &claimed_id);
        assert!(!verify_manifest(&forged).unwrap());
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
    }

    #[test]
    fn test_generate_and_sign() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test-key.json");
        let manifest_path = dir.path().join("manifest.json");

        generate_key(&key_path, false).unwrap();

        let manifest = json!({
            "version": "1.0",
            "package": "com.test.app",
            "appVersion": "1.0.0",
            "minRuntimeVersion": "0.1.0",
            "wasm": {
                "path": "app.wasm",
                "size": 1024,
                "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            }
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let signing_key = load_signing_key(&key_path).unwrap();
        sign_manifest(&manifest_path, &signing_key).unwrap();

        let signed_content = std::fs::read_to_string(&manifest_path).unwrap();
        let signed_manifest: serde_json::Value = serde_json::from_str(&signed_content).unwrap();

        assert!(signed_manifest.get("signerId").is_some());
        assert!(signed_manifest.get("signature").is_some());
        assert!(signed_manifest.get("minRuntimeVersion").is_some());

        let signature = signed_manifest.get("signature").unwrap();
        assert_eq!(
            signature.get("algorithm").unwrap().as_str().unwrap(),
            "ed25519"
        );
        assert!(signature.get("publicKey").is_some());
        assert!(signature.get("signature").is_some());
    }

    #[test]
    fn dev_key_is_detected_however_it_was_loaded() {
        // A dev-seeded key file passed via --key / MERO_SIGN_KEY must still be
        // reported as the dev key, so the warning cannot be dodged.
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("dev-key.json");
        let dev = dev_signing_key();
        std::fs::write(
            &key_path,
            serde_json::to_string(&KeyFile {
                private_key: URL_SAFE_NO_PAD.encode(dev.to_bytes()),
                public_key: URL_SAFE_NO_PAD.encode(dev.verifying_key().as_bytes()),
                signer_id: dev_signer_id(),
            })
            .unwrap(),
        )
        .unwrap();

        assert!(is_dev_key(&load_signing_key(&key_path).unwrap()));

        let mut rng = rand::thread_rng();
        assert!(!is_dev_key(&SigningKey::generate(&mut rng)));
    }

    #[test]
    fn generate_key_refuses_to_replace_an_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.json");
        generate_key(&key_path, false).unwrap();
        let original = std::fs::read_to_string(&key_path).unwrap();

        // Losing a signing key is unrecoverable, so a second generate must fail
        // and leave the first key exactly as it was.
        let err = generate_key(&key_path, false).unwrap_err().to_string();
        assert!(err.contains("already exists"), "got: {err}");
        assert!(
            err.contains("--force"),
            "error must name the escape hatch: {err}"
        );
        assert_eq!(std::fs::read_to_string(&key_path).unwrap(), original);

        generate_key(&key_path, true).unwrap();
        assert_ne!(std::fs::read_to_string(&key_path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn generate_key_writes_owner_only_perms() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.json");

        // Pre-place a world-readable file with junk: a forced replacement must
        // both replace the content AND end at 0600, never leave the new seed
        // under the old 0644.
        std::fs::write(&key_path, "junk, not a key").unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        generate_key(&key_path, true).unwrap();

        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "private key must be readable only by owner"
        );

        let content = std::fs::read_to_string(&key_path).unwrap();
        assert!(
            content.contains("private_key"),
            "junk content must be replaced by the key JSON"
        );
        assert!(!content.contains("junk"), "stale content must not survive");
    }

    #[test]
    fn test_dev_key_deterministic() {
        let key = dev_signing_key();
        let signer_id = derive_signer_id_did_key(key.verifying_key().as_bytes());

        assert!(signer_id.starts_with("did:key:z"));

        // Must be deterministic across runs
        let key2 = dev_signing_key();
        let signer_id2 = derive_signer_id_did_key(key2.verifying_key().as_bytes());
        assert_eq!(signer_id, signer_id2);
    }

    #[test]
    fn test_dev_signer_id_is_stable() {
        // This is the canonical dev signer_id. If this changes, all dev
        // ApplicationIds change and existing dev contexts break.
        let id = dev_signer_id();
        assert_eq!(
            id,
            "did:key:z6MknF3p5L5FDHJQ7FREUapuX4Wmp4MtF6WrHYaXS2B3eZQd"
        );
    }
}
