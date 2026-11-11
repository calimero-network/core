//! The signed receipt of a finished pipeline run: what the node states and the one message it
//! signs, shared by the node that signs it and the client that checks it.

use std::collections::BTreeMap;

use ed25519_dalek::Verifier;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a receipt's signature covers before the receipt.
pub const PIPELINE_RECEIPT_DOMAIN: &[u8] = b"calimero.pipeline.receipt.v1\0";

/// A file named by id, size and plaintext SHA-256 (hex).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileRef {
    pub id: String,
    pub size: u64,
    pub sha256: String,
}

/// What a step run used, as decimal strings (a JSON number loses precision past 2^53).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProofUsage {
    pub cpu_micros: String,
    pub gib_micros: String,
    pub fuel: String,
}

/// What the node kept of one step run when it ended: which job node ran which assignment and
/// what came out. The report fields are absent for a run the leader ended without a report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepRunProof {
    pub run: String,
    pub step: String,
    pub index: u32,
    /// How the run ended: `done`, `cancelled` or `failed:<reason>`.
    pub outcome: String,
    /// The attempt that ended it; zero when none was ever assigned.
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment_digest: Option<String>,
    /// SHA-256 of the job's signed report message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_sha256: Option<String>,
    /// The job key's signature over that message, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub outputs: Vec<FileRef>,
    pub usage: ProofUsage,
}

/// A run answered from the step cache: it ran nothing, so it has no proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CachedRun {
    pub index: u32,
    pub cached: bool,
    pub cache_key: String,
}

/// One run of a step in a receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunEntry {
    Cached(CachedRun),
    Ran(Box<StepRunProof>),
}

/// One step of the run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub runs: Vec<RunEntry>,
}

/// What the node states about a finished pipeline run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineReceipt {
    pub version: u32,
    /// The cluster identity key, hex.
    pub cluster: String,
    pub app: String,
    pub run: String,
    pub pipeline: String,
    /// The package release the run started under.
    pub release: String,
    pub pipeline_hash: String,
    /// Step to the compute component hash it ran, `self` included.
    pub modules: BTreeMap<String, String>,
    pub inputs: BTreeMap<String, Vec<FileRef>>,
    pub params: BTreeMap<String, Value>,
    pub state: String,
    pub start_ts_ms: u64,
    pub end_ts_ms: u64,
    pub steps: BTreeMap<String, StepReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_so_far: Option<String>,
}

/// `GET /apps/{app}/pipeline-runs/{run}/receipt`: the receipt as the node built it (kept as JSON
/// so the bytes signed are the ones sent), the answering node and its receipt key's signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptAnswer {
    pub receipt: Value,
    pub node: u64,
    /// Ed25519 over [`receipt_message`], hex.
    pub signature: String,
}

/// The bytes signed: [`PIPELINE_RECEIPT_DOMAIN`] then the receipt's canonical JSON (RFC 8785).
#[must_use]
pub fn receipt_message(receipt: &Value) -> Vec<u8> {
    let mut m = PIPELINE_RECEIPT_DOMAIN.to_vec();
    m.extend(serde_json_canonicalizer::to_vec(receipt).unwrap_or_default());
    m
}

/// Checks `answer` is signed by `key` (the answering node's attested receipt key) and returns the
/// receipt it carries.
pub fn verify_pipeline_receipt(
    answer: &ReceiptAnswer,
    key: &[u8; 32],
) -> eyre::Result<PipelineReceipt> {
    let vk = ed25519_dalek::VerifyingKey::from_bytes(key)
        .map_err(|e| eyre::eyre!("not a receipt key: {e}"))?;
    let sig = hex::decode(&answer.signature)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .ok_or_else(|| eyre::eyre!("the signature is not 64 bytes of hex"))?;
    vk.verify(
        &receipt_message(&answer.receipt),
        &ed25519_dalek::Signature::from_bytes(&sig),
    )
    .map_err(|_| eyre::eyre!("the receipt is not signed by that key"))?;
    Ok(serde_json::from_value(answer.receipt.clone())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
    use serde_json::json;

    fn receipt() -> PipelineReceipt {
        PipelineReceipt {
            version: 1,
            cluster: "ab".repeat(32),
            app: "app-1".into(),
            run: "run-1".into(),
            pipeline: "etl".into(),
            release: "1.0.0".into(),
            pipeline_hash: "cd".repeat(32),
            modules: BTreeMap::from([("load".to_owned(), "ef".repeat(32))]),
            inputs: BTreeMap::from([(
                "data".to_owned(),
                vec![FileRef {
                    id: "01".repeat(32),
                    size: 7,
                    sha256: "02".repeat(32),
                }],
            )]),
            params: BTreeMap::from([("n".to_owned(), json!(3))]),
            state: "succeeded".into(),
            start_ts_ms: 1_700_000_000_000,
            end_ts_ms: 1_700_000_005_000,
            steps: BTreeMap::from([(
                "load".to_owned(),
                StepReceipt {
                    status: "succeeded".into(),
                    reason: None,
                    runs: vec![
                        RunEntry::Ran(Box::new(StepRunProof {
                            run: "run-1.load.0".into(),
                            step: "load".into(),
                            index: 0,
                            outcome: "done".into(),
                            attempt: 1,
                            node: Some(4),
                            job_key: Some("03".repeat(32)),
                            assignment_digest: Some("04".repeat(32)),
                            report_sha256: Some("05".repeat(32)),
                            signature: Some("06".repeat(64)),
                            outputs: vec![],
                            usage: ProofUsage::default(),
                        })),
                        RunEntry::Cached(CachedRun {
                            index: 1,
                            cached: true,
                            cache_key: "07".repeat(32),
                        }),
                    ],
                },
            )]),
            cost_so_far: None,
        }
    }

    fn signed(sk: &ed25519_dalek::SigningKey, receipt: &PipelineReceipt) -> ReceiptAnswer {
        let receipt = serde_json::to_value(receipt).unwrap();
        ReceiptAnswer {
            signature: hex::encode(sk.sign(&receipt_message(&receipt)).to_bytes()),
            receipt,
            node: 4,
        }
    }

    #[test]
    fn a_receipt_verifies_under_its_key() {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let key = sk.verifying_key().to_bytes();
        let got = verify_pipeline_receipt(&signed(&sk, &receipt()), &key).unwrap();
        assert_eq!(got, receipt());
    }

    #[test]
    fn another_key_or_a_changed_field_fails() {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let key = sk.verifying_key().to_bytes();
        let answer = signed(&sk, &receipt());
        assert!(verify_pipeline_receipt(&answer, &[9; 32]).is_err());
        for (field, value) in [
            ("app", json!("app-2")),
            ("run", json!("run-2")),
            ("state", json!("failed")),
            ("endTsMs", json!(1)),
        ] {
            let mut forged = answer.clone();
            forged.receipt[field] = value;
            assert!(verify_pipeline_receipt(&forged, &key).is_err(), "{field}");
        }
        let mut forged = answer.clone();
        forged.receipt["steps"]["load"]["runs"][0]["jobKey"] = json!("08".repeat(32));
        assert!(verify_pipeline_receipt(&forged, &key).is_err());
        let mut forged = answer;
        forged.signature = "zz".into();
        assert!(verify_pipeline_receipt(&forged, &key).is_err());
    }

    /// The bytes a verifier in any language rebuilds from the receipt it was handed.
    #[test]
    fn the_signed_message_matches_the_published_vector() {
        let receipt = json!({
            "app": "a", "cluster": "c", "run": "r", "state": "succeeded", "version": 1,
        });
        assert_eq!(
            receipt_message(&receipt),
            b"calimero.pipeline.receipt.v1\0{\"app\":\"a\",\"cluster\":\"c\",\"run\":\"r\",\"state\":\"succeeded\",\"version\":1}"
        );
    }
}
