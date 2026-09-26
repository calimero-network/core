//! Sigstore verification of a keyless-signed release asset.
//!
//! mero-tee signs its release assets with cosign keyless: there is no release
//! key, only a short-lived Fulcio certificate issued to a GitHub Actions
//! workflow identity, logged in Rekor. Verifying an asset therefore means
//! checking the detached signature against that certificate, the Rekor signed
//! entry timestamp that proves the certificate was valid when it signed, and
//! the workflow identity baked into the certificate.

use base64::Engine;
use eyre::{bail, Result as EyreResult};
use sha2::{Digest, Sha256};
use sigstore::bundle::verify::policy::{
    AllOf, GitHubWorkflowName, GitHubWorkflowRef, GitHubWorkflowRepository, GitHubWorkflowTrigger,
    OIDCIssuer, SingleX509ExtPolicy, VerificationPolicy as SigstoreVerificationPolicy,
};
use sigstore::bundle::verify::Verifier as SigstoreBundleVerifier;
use sigstore::cosign::bundle::SignedArtifactBundle;
use sigstore::crypto::{CosignVerificationKey, Signature as SigstoreSignature, SigningScheme};
use sigstore::trust::sigstore::SigstoreTrustRoot;
use sigstore::trust::TrustRoot;
use tracing::warn;
use x509_cert::der::{DecodePem, Encode};
use x509_cert::Certificate;

/// The GitHub Actions OIDC issuer every mero-tee release certificate names.
pub const GITHUB_ACTIONS_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// Which workflow run a release asset must have been signed by.
#[derive(Clone, Copy, Debug)]
pub struct WorkflowIdentity {
    /// The workflow's `name:`, e.g. `Release mero-kms`.
    pub name: &'static str,
    /// `owner/repo`.
    pub repository: &'static str,
    /// The ref the run was on, e.g. `refs/heads/master`.
    pub git_ref: &'static str,
    /// The event that started the run, when it is pinned. `None` accepts any
    /// trigger: the node-image release runs on push and on a manual dispatch.
    pub trigger: Option<&'static str>,
}

/// The `Release mero-kms` workflow, which signs the KMS attestation policy.
pub const KMS_RELEASE_IDENTITY: WorkflowIdentity = WorkflowIdentity {
    name: "Release mero-kms",
    repository: "calimero-network/mero-tee",
    git_ref: "refs/heads/master",
    trigger: Some("push"),
};

/// The `Release mero-tee` workflow, which signs a node release's
/// `published-mrtds.json`. It runs on push and by manual dispatch (a release
/// is re-run by hand after a failure), so the trigger is not pinned.
pub const NODE_RELEASE_IDENTITY: WorkflowIdentity = WorkflowIdentity {
    name: "Release mero-tee",
    repository: "calimero-network/mero-tee",
    git_ref: "refs/heads/master",
    trigger: None,
};

/// Verify `body` against its detached `signature` and Sigstore `bundle`, and
/// that the signing certificate names `identity`.
pub async fn verify_signed_asset(
    body: &[u8],
    signature: &str,
    bundle: &str,
    identity: &WorkflowIdentity,
) -> EyreResult<()> {
    let trust_root = SigstoreTrustRoot::new(None)
        .await
        .map_err(|e| eyre::eyre!("Failed to initialize Sigstore trust root: {}", e))?;
    let rekor_pub_keys = rekor_public_keys(&trust_root)?;
    let signed_bundle = SignedArtifactBundle::new_verified(bundle, &rekor_pub_keys)
        .map_err(|e| eyre::eyre!("Invalid signed bundle: {}", e))?;

    let detached_signature = signature.trim();
    if detached_signature.is_empty() {
        bail!("signature asset is empty");
    }
    if detached_signature != signed_bundle.base64_signature.trim() {
        bail!("the detached signature and the bundle's signature differ");
    }

    let certificate_pem = decode_bundle_certificate_pem(&signed_bundle.cert)?;
    verify_blob_signature(body, &signed_bundle.base64_signature, &certificate_pem)?;

    let sigstore_bundle = build_sigstore_bundle(body, &signed_bundle, &certificate_pem)?;
    let oidc_issuer = OIDCIssuer::new(GITHUB_ACTIONS_OIDC_ISSUER);
    let workflow_name = GitHubWorkflowName::new(identity.name);
    let workflow_repository = GitHubWorkflowRepository::new(identity.repository);
    let workflow_ref = GitHubWorkflowRef::new(identity.git_ref);
    let workflow_trigger = identity.trigger.map(GitHubWorkflowTrigger::new);

    let mut constraints: Vec<&dyn SigstoreVerificationPolicy> = vec![
        &oidc_issuer,
        &workflow_name,
        &workflow_repository,
        &workflow_ref,
    ];
    if let Some(trigger) = &workflow_trigger {
        constraints.push(trigger);
    }
    let workflow_policy = AllOf::new(constraints)
        .ok_or_else(|| eyre::eyre!("Failed to construct Sigstore verification policy"))?;

    let verifier = SigstoreBundleVerifier::new(Default::default(), trust_root)
        .map_err(|e| eyre::eyre!("Failed to create Sigstore verifier: {}", e))?;
    verifier
        .verify(body, sigstore_bundle, &workflow_policy, true)
        .await
        .map_err(|e| eyre::eyre!("Sigstore bundle verification failed: {}", e))?;

    Ok(())
}

fn rekor_public_keys(
    trust_root: &SigstoreTrustRoot,
) -> EyreResult<std::collections::BTreeMap<String, CosignVerificationKey>> {
    let mut keys = std::collections::BTreeMap::new();
    for (key_id, key_der) in trust_root
        .rekor_keys()
        .map_err(|e| eyre::eyre!("Failed to read Rekor keys from trust root: {}", e))?
    {
        match CosignVerificationKey::from_der(key_der, &SigningScheme::default()) {
            Ok(key) => {
                keys.insert(key_id, key);
            }
            Err(err) => {
                warn!(
                    rekor_key_id = %key_id,
                    error = %err,
                    "Skipping unsupported Rekor key from Sigstore trust root"
                );
            }
        }
    }

    if keys.is_empty() {
        bail!("Sigstore trust root did not provide a usable Rekor public key");
    }

    Ok(keys)
}

pub(crate) fn decode_bundle_certificate_pem(encoded_cert: &str) -> EyreResult<String> {
    let cert_bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded_cert.trim())
        .map_err(|e| eyre::eyre!("Bundle certificate is not valid base64: {}", e))?;
    String::from_utf8(cert_bytes)
        .map_err(|e| eyre::eyre!("Bundle certificate is not valid UTF-8 PEM: {}", e))
}

fn verify_blob_signature(body: &[u8], signature_b64: &str, cert_pem: &str) -> EyreResult<()> {
    let certificate = Certificate::from_pem(cert_pem.as_bytes())
        .map_err(|e| eyre::eyre!("Failed to parse signing certificate PEM: {}", e))?;
    let spki_der = certificate
        .tbs_certificate()
        .subject_public_key_info()
        .to_der()?;
    let verification_key = CosignVerificationKey::try_from_der(&spki_der).map_err(|e| {
        eyre::eyre!(
            "Failed to extract verification key from signing certificate: {}",
            e
        )
    })?;
    verification_key
        .verify_signature(
            SigstoreSignature::Base64Encoded(signature_b64.trim().as_bytes()),
            body,
        )
        .map_err(|e| eyre::eyre!("Detached signature does not match the asset: {}", e))
}

pub(crate) fn build_sigstore_bundle(
    body: &[u8],
    signed_bundle: &SignedArtifactBundle,
    certificate_pem: &str,
) -> EyreResult<sigstore::bundle::Bundle> {
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(signed_bundle.base64_signature.trim())
        .map_err(|e| eyre::eyre!("Bundle signature is not valid base64: {}", e))?;
    let signed_entry_timestamp = base64::engine::general_purpose::STANDARD
        .decode(signed_bundle.rekor_bundle.signed_entry_timestamp.trim())
        .map_err(|e| eyre::eyre!("Bundle signed entry timestamp is not valid base64: {}", e))?;
    let canonicalized_body = base64::engine::general_purpose::STANDARD
        .decode(signed_bundle.rekor_bundle.payload.body.trim())
        .map_err(|e| eyre::eyre!("Bundle canonicalized body is not valid base64: {}", e))?;
    let canonicalized_body_json: serde_json::Value = serde_json::from_slice(&canonicalized_body)
        .map_err(|e| eyre::eyre!("Bundle canonicalized body is not valid JSON: {}", e))?;
    let kind = canonicalized_body_json
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| eyre::eyre!("Bundle canonicalized body is missing kind"))?;
    let api_version = canonicalized_body_json
        .get("apiVersion")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| eyre::eyre!("Bundle canonicalized body is missing apiVersion"))?;
    let log_id = hex::decode(signed_bundle.rekor_bundle.payload.log_id.trim())
        .map_err(|e| eyre::eyre!("Bundle log ID is not valid hex: {}", e))?;

    let certificate = Certificate::from_pem(certificate_pem.as_bytes())
        .map_err(|e| eyre::eyre!("Failed to parse bundle certificate PEM: {}", e))?;
    let cert_der = certificate
        .to_der()
        .map_err(|e| eyre::eyre!("Failed to encode bundle certificate DER: {}", e))?;
    let digest = Sha256::digest(body);

    let bundle_json = serde_json::json!({
        "mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.1",
        "verificationMaterial": {
            "x509CertificateChain": {
                "certificates": [{
                    "rawBytes": base64::engine::general_purpose::STANDARD.encode(cert_der),
                }]
            },
            "tlogEntries": [{
                "logIndex": signed_bundle.rekor_bundle.payload.log_index,
                "logId": {
                    "keyId": base64::engine::general_purpose::STANDARD.encode(log_id),
                },
                "kindVersion": {
                    "kind": kind,
                    "version": api_version,
                },
                "integratedTime": signed_bundle.rekor_bundle.payload.integrated_time,
                "inclusionPromise": {
                    "signedEntryTimestamp": base64::engine::general_purpose::STANDARD.encode(signed_entry_timestamp),
                },
                "canonicalizedBody": base64::engine::general_purpose::STANDARD.encode(canonicalized_body),
            }]
        },
        "messageSignature": {
            "messageDigest": {
                "algorithm": "SHA2_256",
                "digest": base64::engine::general_purpose::STANDARD.encode(digest),
            },
            "signature": base64::engine::general_purpose::STANDARD.encode(signature_bytes),
        }
    });

    serde_json::from_value(bundle_json).map_err(|e| {
        eyre::eyre!(
            "Failed to construct Sigstore bundle from policy artifacts: {}",
            e
        )
    })
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use sigstore::cosign::bundle::SignedArtifactBundle;
    use sigstore::cosign::bundle::{Bundle as RekorBundle, Payload as RekorPayload};

    use super::{build_sigstore_bundle, decode_bundle_certificate_pem};

    fn make_signed_artifact_bundle(log_id: &str) -> SignedArtifactBundle {
        let cert_pem = "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n";
        SignedArtifactBundle {
            base64_signature: base64::engine::general_purpose::STANDARD.encode([7u8; 64]),
            cert: base64::engine::general_purpose::STANDARD.encode(cert_pem),
            rekor_bundle: RekorBundle {
                signed_entry_timestamp: base64::engine::general_purpose::STANDARD.encode([8u8; 64]),
                payload: RekorPayload {
                    body: base64::engine::general_purpose::STANDARD
                        .encode(br#"{"apiVersion":"0.0.1","kind":"hashedrekord"}"#),
                    integrated_time: 1,
                    log_index: 1,
                    log_id: log_id.to_owned(),
                },
            },
        }
    }

    #[test]
    fn decode_bundle_certificate_pem_rejects_invalid_base64() {
        let err = decode_bundle_certificate_pem("!!not-base64!!")
            .expect_err("invalid certificate encoding must fail");
        assert!(err.to_string().contains("not valid base64"));
    }

    #[test]
    fn build_sigstore_bundle_rejects_invalid_log_id() {
        let signed_bundle = make_signed_artifact_bundle("not-a-hex-log-id");
        let err = build_sigstore_bundle(
            b"{}",
            &signed_bundle,
            "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n",
        )
        .expect_err("invalid log ID must fail");
        assert!(err.to_string().contains("log ID"));
    }
}
