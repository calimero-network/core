//! mero-sign: CLI tool for signing Calimero bundle manifests
//!
//! This tool implements the signing flow for CIP-0001 compliant bundles:
//! - Generate Ed25519 keypairs
//! - Sign manifests using RFC 8785 canonicalization
//! - Derive did:key signerId from public keys

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use clap::{Parser, Subcommand};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use eyre::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

/// Multicodec indicator for ed25519-pub (0xed01, varint encoded).
const ED25519_PUB_MULTICODEC: [u8; 2] = [0xed, 0x01];

#[derive(Parser)]
#[command(name = "mero-sign")]
#[command(about = "Sign Calimero bundle manifests")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sign a manifest.json file in-place
    Sign {
        /// Path to the manifest.json file
        manifest: PathBuf,

        /// Path to the key file (JSON format)
        #[arg(long, short)]
        key: PathBuf,
    },

    /// Generate a new Ed25519 keypair
    GenerateKey {
        /// Output path for the key file
        #[arg(long, short)]
        output: PathBuf,
    },

    /// Derive the did:key signerId from a key file
    DeriveSignerId {
        /// Path to the key file
        #[arg(long, short)]
        key: PathBuf,
    },
}

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

/// Derives a did:key signerId from an Ed25519 public key.
///
/// The did:key format for Ed25519 is:
/// - `did:key:` prefix
/// - multibase base58btc encoding ('z' prefix)
/// - multicodec indicator for ed25519-pub (0xed01)
/// - the 32-byte Ed25519 public key
fn derive_signer_id_did_key(pubkey: &[u8; 32]) -> String {
    // Construct the multicodec-prefixed key
    let mut multicodec_key = Vec::with_capacity(2 + 32);
    multicodec_key.extend_from_slice(&ED25519_PUB_MULTICODEC);
    multicodec_key.extend_from_slice(pubkey);

    // Encode with base58btc (multibase 'z' prefix)
    let encoded = bs58::encode(&multicodec_key).into_string();

    format!("did:key:z{}", encoded)
}

/// Canonicalizes a manifest JSON value using RFC 8785 (JCS).
/// Removes the signature field and all underscore-prefixed transient fields before canonicalization.
fn canonicalize_manifest(manifest_json: &serde_json::Value) -> Result<Vec<u8>> {
    // Clone and remove the signature field for canonicalization
    let mut signing_view = manifest_json.clone();
    if let Some(obj) = signing_view.as_object_mut() {
        obj.remove("signature");
        // Remove all underscore-prefixed fields to prevent signature confusion.
        obj.retain(|k, _| !k.starts_with('_'));
    }

    // Canonicalize using RFC 8785 JCS
    let canonical_bytes = serde_json_canonicalizer::to_vec(&signing_view)
        .context("failed to canonicalize manifest JSON")?;

    Ok(canonical_bytes)
}

/// Computes the signing payload (SHA-256 hash of canonical manifest bytes).
fn compute_signing_payload(canonical_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(canonical_bytes);
    hasher.finalize().into()
}

/// Load a signing key from a key file
fn load_signing_key(key_path: &PathBuf) -> Result<SigningKey> {
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

/// Sign a manifest file
fn sign_manifest(manifest_path: &PathBuf, key_path: &PathBuf) -> Result<()> {
    // Load the signing key
    let signing_key = load_signing_key(key_path)?;
    let verifying_key = signing_key.verifying_key();
    let signer_id = derive_signer_id_did_key(verifying_key.as_bytes());

    // Read the manifest
    let manifest_content = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read manifest: {}", manifest_path.display()))?;

    let mut manifest: serde_json::Value = serde_json::from_str(&manifest_content)
        .with_context(|| format!("failed to parse manifest: {}", manifest_path.display()))?;

    // Add signerId to the manifest
    if let Some(obj) = manifest.as_object_mut() {
        obj.insert(
            "signerId".to_string(),
            serde_json::Value::String(signer_id.clone()),
        );
        // Also add minRuntimeVersion if not present
        if !obj.contains_key("minRuntimeVersion") {
            obj.insert(
                "minRuntimeVersion".to_string(),
                serde_json::Value::String("1.0.0".to_string()),
            );
        }
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

    eprintln!("Signed manifest: {}", manifest_path.display());
    eprintln!("  signerId: {}", signer_id);

    Ok(())
}

/// Generate a new Ed25519 keypair
fn generate_key(output_path: &PathBuf) -> Result<()> {
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
    fs::write(output_path, key_json)
        .with_context(|| format!("failed to write key file: {}", output_path.display()))?;

    eprintln!("Generated new keypair: {}", output_path.display());
    eprintln!("  signerId: {}", signer_id);

    Ok(())
}

/// Derive the signerId from a key file
fn derive_signer_id(key_path: &PathBuf) -> Result<()> {
    let key_content = fs::read_to_string(key_path)
        .with_context(|| format!("failed to read key file: {}", key_path.display()))?;

    let key_file: KeyFile = serde_json::from_str(&key_content)
        .with_context(|| format!("failed to parse key file: {}", key_path.display()))?;

    println!("{}", key_file.signer_id);

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Sign { manifest, key } => sign_manifest(&manifest, &key),
        Commands::GenerateKey { output } => generate_key(&output),
        Commands::DeriveSignerId { key } => derive_signer_id(&key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
    }

    #[test]
    fn test_generate_and_sign() {
        let dir = tempdir().unwrap();
        let key_path = dir.path().join("test-key.json");
        let manifest_path = dir.path().join("manifest.json");

        // Generate a key
        generate_key(&key_path).unwrap();

        // Create a test manifest
        let manifest = serde_json::json!({
            "version": "1.0",
            "package": "com.test.app",
            "appVersion": "1.0.0",
            "wasm": {
                "path": "app.wasm",
                "size": 1024,
                "hash": null
            }
        });
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        // Sign the manifest
        sign_manifest(&manifest_path, &key_path).unwrap();

        // Read the signed manifest
        let signed_content = fs::read_to_string(&manifest_path).unwrap();
        let signed_manifest: serde_json::Value = serde_json::from_str(&signed_content).unwrap();

        // Verify it has the required fields
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
    fn test_canonicalize_removes_all_underscore_prefixed_fields() {
        // Test that ALL underscore-prefixed fields are removed, not just known ones.
        // This ensures consistency with signature.rs verification logic.
        let manifest = serde_json::json!({
            "version": "1.0",
            "package": "com.test.app",
            "_binary": "some_value",
            "_overwrite": true,
            "_debug": "debug_info",
            "_custom_field": 123,
            "_temp": {"nested": "data"},
            "signature": {
                "algorithm": "ed25519",
                "publicKey": "test",
                "signature": "test"
            }
        });

        let canonical = canonicalize_manifest(&manifest).unwrap();
        let canonical_str = String::from_utf8(canonical).unwrap();

        // All underscore-prefixed fields should be removed
        assert!(!canonical_str.contains("_binary"));
        assert!(!canonical_str.contains("_overwrite"));
        assert!(!canonical_str.contains("_debug"));
        assert!(!canonical_str.contains("_custom_field"));
        assert!(!canonical_str.contains("_temp"));

        // signature should also be removed
        assert!(!canonical_str.contains("signature"));

        // Regular fields should remain
        assert!(canonical_str.contains("version"));
        assert!(canonical_str.contains("package"));
    }
}
