//! The signed receipt of a finished pipeline run: what the node states and the one message it
//! signs, shared by the node that signs it and the client that checks it.

use std::collections::BTreeMap;

use ed25519_dalek::Verifier;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;

use crate::attest::AttestationDoc;

/// What a receipt's signature covers before the receipt.
pub const PIPELINE_RECEIPT_DOMAIN: &[u8] = b"calimero.pipeline.receipt.v1\0";
/// A receipt without job evidence (what a caller gets unless it asks for more).
pub const RECEIPT_VERSION_SIGNED: u32 = 1;
/// A receipt whose proofs carry what each job signed and whose job keys carry their attestations.
pub const RECEIPT_VERSION_JOBS: u32 = 2;
/// What a job node's report signature covers before the report.
pub const JOB_REPORT_DOMAIN: &[u8] = b"calimero.compute.report.v1\0";
/// Tags the module log a report carries, after everything else it signs.
pub const JOB_REPORT_LOG_DOMAIN: &[u8] = b"calimero.compute.report.log.v1\0";
/// The reasons a job's own report gives for a failed run.
pub const JOB_FAIL_REASONS: [&str; 6] = [
    "fuel",
    "memory",
    "trap",
    "app-error",
    "integrity",
    "cancelled",
];
/// The reasons only the leader gives, with no report; any other reason is refused.
pub const LEADER_FAIL_REASONS: [&str; 6] = [
    "lost",
    "refused",
    "timeout",
    "quota",
    "not-granted",
    "too-large",
];

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
    /// What the job's report signs that the fields above do not (version 2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobEvidence>,
}

/// A file a job run wrote as its report signs it: the file id, plaintext size and SHA-256, and
/// the SHA-256 of each segment of its ciphertext, all lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobOutput {
    pub id: String,
    pub size: u64,
    pub sha256: String,
    pub segments: Vec<String>,
}

/// The rest of what a job's report signs, so a verifier rebuilds the exact message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobEvidence {
    /// SHA-256 of the method's return bytes, hex.
    pub result_sha256: String,
    /// The failure text of an app-error or trap report.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// The segment hashes of each output, in the order of the proof's `outputs`.
    pub segments: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<JobOutput>,
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
    /// Each job key a proof names, its job node's `purpose=jobs` attestation as the leader
    /// verified it before assigning the run (version 2 only).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub job_attestations: BTreeMap<String, AttestationDoc>,
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
    // The receipt as signed: JCS writes every number as an f64, so two spellings of one past 2^53
    // verify alike, and only the canonical one is what the node signed.
    let canonical = serde_json_canonicalizer::to_vec(&answer.receipt)?;
    Ok(serde_json::from_slice(&canonical)?)
}

/// One job report's signed fields, in the order [`report_message`] writes them.
pub struct ReportFields<'a> {
    /// SHA-256 of the assignment message the job ran.
    pub assignment: &'a [u8; 32],
    pub app: &'a str,
    pub run: &'a str,
    pub attempt: u32,
    /// `None` for a done report, else the reason it failed (one of [`JOB_FAIL_REASONS`]).
    pub reason: Option<&'a str>,
    pub message: &'a str,
    pub result_sha256: &'a [u8; 32],
    pub outputs: &'a [JobOutput],
    /// CPU micros, GiB micros, fuel.
    pub usage: [u64; 3],
    pub log: Option<&'a JobOutput>,
}

fn push_prefixed(m: &mut Vec<u8>, field: &[u8]) -> eyre::Result<()> {
    let len = u32::try_from(field.len()).map_err(|_| eyre::eyre!("a field is too long"))?;
    m.extend_from_slice(&len.to_be_bytes());
    m.extend_from_slice(field);
    Ok(())
}

/// The 32 bytes of a hash spelled as the wire spells it: 64 lowercase hex digits.
pub fn hash_bytes(text: &str) -> eyre::Result<[u8; 32]> {
    if text.len() != 64 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        eyre::bail!("a hash is 64 lowercase hex digits");
    }
    let mut out = [0; 32];
    hex::decode_to_slice(text, &mut out)?;
    Ok(out)
}

fn push_output(m: &mut Vec<u8>, output: &JobOutput) -> eyre::Result<()> {
    m.extend_from_slice(&hash_bytes(&output.id)?);
    m.extend_from_slice(&output.size.to_be_bytes());
    m.extend_from_slice(&hash_bytes(&output.sha256)?);
    let count =
        u32::try_from(output.segments.len()).map_err(|_| eyre::eyre!("too many segments"))?;
    m.extend_from_slice(&count.to_be_bytes());
    for segment in &output.segments {
        m.extend_from_slice(&hash_bytes(segment)?);
    }
    Ok(())
}

/// The bytes a job node signs for one report: [`JOB_REPORT_DOMAIN`], the assignment digest, the
/// length-prefixed app, run, reason and message, the result's hash, the outputs, the usage, then
/// the log after [`JOB_REPORT_LOG_DOMAIN`]; integers big-endian, hashes as their 32 bytes.
pub fn report_message(f: &ReportFields<'_>) -> eyre::Result<Vec<u8>> {
    let mut m = JOB_REPORT_DOMAIN.to_vec();
    m.extend_from_slice(f.assignment);
    push_prefixed(&mut m, f.app.as_bytes())?;
    push_prefixed(&mut m, f.run.as_bytes())?;
    m.extend_from_slice(&f.attempt.to_be_bytes());
    m.push(u8::from(f.reason.is_some()));
    push_prefixed(&mut m, f.reason.unwrap_or_default().as_bytes())?;
    push_prefixed(&mut m, f.message.as_bytes())?;
    m.extend_from_slice(f.result_sha256);
    let count = u32::try_from(f.outputs.len()).map_err(|_| eyre::eyre!("too many outputs"))?;
    m.extend_from_slice(&count.to_be_bytes());
    for output in f.outputs {
        push_output(&mut m, output)?;
    }
    for n in f.usage {
        m.extend_from_slice(&n.to_be_bytes());
    }
    if let Some(log) = f.log {
        m.extend_from_slice(JOB_REPORT_LOG_DOMAIN);
        push_output(&mut m, log)?;
    }
    Ok(m)
}

/// The reason a job signed for a proof's `outcome`: `Some(None)` for done, `Some(Some(r))` for a
/// job's failure, `None` for an end only the leader gives (a cancel is the job's when it reported).
fn signed_reason(outcome: &str, reported: bool) -> eyre::Result<Option<Option<&str>>> {
    if outcome == "done" {
        return Ok(Some(None));
    }
    if outcome == "cancelled" {
        return Ok(reported.then_some(Some("cancelled")));
    }
    let reason = outcome
        .strip_prefix("failed:")
        .ok_or_else(|| eyre::eyre!("the outcome {outcome:?} is not one a run ends with"))?;
    if JOB_FAIL_REASONS.contains(&reason) {
        return Ok(Some(Some(reason)));
    }
    if !LEADER_FAIL_REASONS.contains(&reason) {
        eyre::bail!("the outcome {outcome:?} is not one a run ends with");
    }
    Ok(None)
}

/// A decimal `u64` as the receipt spells usage: digits only.
fn decimal(text: &str) -> eyre::Result<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        eyre::bail!("{text:?} is not a decimal integer");
    }
    Ok(text.parse()?)
}

/// Checks one step run's proof: it is the run the receipt's pipeline run, step and index name,
/// and either the leader ended it (no report, no outputs, no usage) or its job node signed a
/// report that says exactly what the proof says. Answers the job key and node of a signed one.
fn check_proof(
    receipt: &PipelineReceipt,
    step: &str,
    p: &StepRunProof,
) -> eyre::Result<Option<(String, u64)>> {
    let expected = format!("{}.{step}.{}", receipt.run, p.index);
    if p.run != expected || p.step != step {
        eyre::bail!("the proof of {} is listed as {expected}", p.run);
    }
    let reported = p.signature.is_some();
    let Some(reason) = signed_reason(&p.outcome, reported)? else {
        let zero = [&p.usage.cpu_micros, &p.usage.gib_micros, &p.usage.fuel]
            .iter()
            .all(|n| n.as_str() == "0");
        if reported
            || p.report_sha256.is_some()
            || p.job.is_some()
            || !p.outputs.is_empty()
            || !zero
        {
            eyre::bail!(
                "{} ended {} by the leader, yet names a report, outputs or usage",
                p.run,
                p.outcome
            );
        }
        return Ok(None);
    };
    let missing =
        |what: &str| eyre::eyre!("{} ended {} but its proof has no {what}", p.run, p.outcome);
    let node = p.node.ok_or_else(|| missing("job node"))?;
    let key = p.job_key.as_deref().ok_or_else(|| missing("job key"))?;
    let digest = p
        .assignment_digest
        .as_deref()
        .ok_or_else(|| missing("assignment digest"))?;
    let report_sha256 = p
        .report_sha256
        .as_deref()
        .ok_or_else(|| missing("report hash"))?;
    let signature = p
        .signature
        .as_deref()
        .ok_or_else(|| missing("report signature"))?;
    let job = p
        .job
        .as_ref()
        .ok_or_else(|| missing("job evidence (it ended before receipts carried it)"))?;
    if job.segments.len() != p.outputs.len() {
        eyre::bail!(
            "{}: the job evidence lists segments for {} outputs, the proof {}",
            p.run,
            job.segments.len(),
            p.outputs.len()
        );
    }
    let outputs: Vec<JobOutput> = p
        .outputs
        .iter()
        .zip(&job.segments)
        .map(|(o, segments)| JobOutput {
            id: o.id.clone(),
            size: o.size,
            sha256: o.sha256.clone(),
            segments: segments.clone(),
        })
        .collect();
    let message = report_message(&ReportFields {
        assignment: &hash_bytes(digest)?,
        app: &receipt.app,
        run: &p.run,
        attempt: p.attempt,
        reason,
        message: &job.message,
        result_sha256: &hash_bytes(&job.result_sha256)?,
        outputs: &outputs,
        usage: [
            decimal(&p.usage.cpu_micros)?,
            decimal(&p.usage.gib_micros)?,
            decimal(&p.usage.fuel)?,
        ],
        log: job.log.as_ref(),
    })
    .map_err(|e| eyre::eyre!("{}: {e}", p.run))?;
    if hex::encode(sha2::Sha256::digest(&message)) != report_sha256 {
        eyre::bail!(
            "{}: the report hash is not that of what the proof says",
            p.run
        );
    }
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&hash_bytes(key)?)
        .map_err(|e| eyre::eyre!("{}: not a job key: {e}", p.run))?;
    let sig = hex::decode(signature)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .ok_or_else(|| eyre::eyre!("{}: the report signature is not 64 bytes of hex", p.run))?;
    vk.verify_strict(&message, &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| eyre::eyre!("{}: the job key did not sign what the proof says", p.run))?;
    Ok(Some((key.to_owned(), node)))
}

/// The attestation `receipt` carries for job key `key` on node `node`, if it names that key, that
/// node of `cluster` (the name every replica's quote bound) and the receipt's cluster identity.
/// Whether its quote binds them is [`crate::attest::verify_job_key`]'s to check.
pub fn job_attestation<'a>(
    receipt: &'a PipelineReceipt,
    cluster: &str,
    key: &str,
    node: u64,
) -> eyre::Result<&'a AttestationDoc> {
    let doc = receipt
        .job_attestations
        .get(key)
        .ok_or_else(|| eyre::eyre!("the receipt carries no attestation for it"))?;
    if doc.job_key.as_deref() != Some(key) {
        eyre::bail!("its attestation is another key's");
    }
    if doc.node != node || doc.cluster != cluster {
        eyre::bail!(
            "it is attested as node {} of cluster {}, not node {node} of {cluster}",
            doc.node,
            doc.cluster
        );
    }
    if doc.cluster_identity.as_deref() != Some(receipt.cluster.as_str()) {
        eyre::bail!("its job node takes work from another cluster identity");
    }
    Ok(doc)
}

/// Checks every step run's proof of a version 2 receipt (`check_proof`), each step's runs listed
/// once in index order, and answers each job key that signed a report with the node it ran on; a
/// key named on two nodes is refused. Whose key each is, its attestation in `job_attestations`, is
/// for the caller to verify. What a job signed is proven; the rest (step and run states, cached
/// runs, and the node and key a leader-ended run names) is the signing replica's word.
pub fn verify_job_reports(receipt: &PipelineReceipt) -> eyre::Result<BTreeMap<String, u64>> {
    if receipt.version != RECEIPT_VERSION_JOBS {
        eyre::bail!(
            "the receipt is version {}; only version {RECEIPT_VERSION_JOBS} carries what each job signed",
            receipt.version
        );
    }
    let mut keys = BTreeMap::new();
    for (step, s) in &receipt.steps {
        for (i, entry) in s.runs.iter().enumerate() {
            let index = match entry {
                RunEntry::Ran(p) => p.index,
                RunEntry::Cached(c) => c.index,
            };
            if usize::try_from(index).ok() != Some(i) {
                eyre::bail!("step {step} lists run {index} where run {i} belongs");
            }
            let p = match entry {
                RunEntry::Cached(c) if c.cached => continue,
                RunEntry::Cached(_) => eyre::bail!("step {step} run {i} is listed as not cached"),
                RunEntry::Ran(p) => p,
            };
            if let Some((key, node)) = check_proof(receipt, step, p)? {
                if *keys.entry(key.clone()).or_insert(node) != node {
                    eyre::bail!("job key {key} is named on two nodes");
                }
            }
        }
    }
    Ok(keys)
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
                            job: None,
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
            job_attestations: BTreeMap::new(),
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

    const JOB_SEED: [u8; 32] = [7; 32];

    fn job_output(n: u8) -> JobOutput {
        JobOutput {
            id: format!("{n:02x}").repeat(32),
            size: u64::from(n) * 10,
            sha256: format!("{:02x}", n + 1).repeat(32),
            segments: vec![format!("{:02x}", n + 2).repeat(32)],
        }
    }

    /// A version 2 receipt: step `load` ran once on job node 4, which signed its report.
    fn with_job(reason: Option<&str>, message: &str, log: Option<JobOutput>) -> PipelineReceipt {
        let sk = ed25519_dalek::SigningKey::from_bytes(&JOB_SEED);
        let outputs = if reason.is_some() {
            vec![]
        } else {
            vec![job_output(0x10), job_output(0x20)]
        };
        let result_sha256 = [0x33; 32];
        let usage = [11, 22, 33];
        let message_bytes = report_message(&ReportFields {
            assignment: &[0x44; 32],
            app: "app-1",
            run: "run-1.load.0",
            attempt: 2,
            reason,
            message,
            result_sha256: &result_sha256,
            outputs: &outputs,
            usage,
            log: log.as_ref(),
        })
        .unwrap();
        let mut r = receipt();
        r.version = RECEIPT_VERSION_JOBS;
        r.steps.get_mut("load").unwrap().runs = vec![RunEntry::Ran(Box::new(StepRunProof {
            run: "run-1.load.0".into(),
            step: "load".into(),
            index: 0,
            outcome: reason.map_or("done".to_owned(), |r| format!("failed:{r}")),
            attempt: 2,
            node: Some(4),
            job_key: Some(hex::encode(sk.verifying_key().to_bytes())),
            assignment_digest: Some("44".repeat(32)),
            report_sha256: Some(hex::encode(sha2::Sha256::digest(&message_bytes))),
            signature: Some(hex::encode(sk.sign(&message_bytes).to_bytes())),
            outputs: outputs
                .iter()
                .map(|o| FileRef {
                    id: o.id.clone(),
                    size: o.size,
                    sha256: o.sha256.clone(),
                })
                .collect(),
            usage: ProofUsage {
                cpu_micros: "11".into(),
                gib_micros: "22".into(),
                fuel: "33".into(),
            },
            job: Some(JobEvidence {
                result_sha256: hex::encode(result_sha256),
                message: message.to_owned(),
                segments: outputs.iter().map(|o| o.segments.clone()).collect(),
                log,
            }),
        }))];
        r
    }

    fn proof_mut(r: &mut PipelineReceipt) -> &mut StepRunProof {
        let RunEntry::Ran(p) = &mut r.steps.get_mut("load").unwrap().runs[0] else {
            panic!("a ran");
        };
        p
    }

    #[test]
    fn a_job_signed_report_verifies_and_names_its_key_and_node() {
        let key = hex::encode(
            ed25519_dalek::SigningKey::from_bytes(&JOB_SEED)
                .verifying_key()
                .to_bytes(),
        );
        for r in [
            with_job(None, "", None),
            with_job(None, "", Some(job_output(0x50))),
            with_job(Some("app-error"), "the input was empty", None),
            with_job(Some("fuel"), "", None),
        ] {
            assert_eq!(
                verify_job_reports(&r).unwrap(),
                BTreeMap::from([(key.clone(), 4)])
            );
        }
    }

    #[test]
    fn anything_the_job_did_not_sign_is_refused() {
        type Edit = (&'static str, fn(&mut StepRunProof));
        let edits: [Edit; 15] = [
            ("signature", |p| p.signature = Some("06".repeat(64))),
            ("job key", |p| {
                p.job_key = Some(hex::encode(
                    ed25519_dalek::SigningKey::from_bytes(&[8; 32])
                        .verifying_key()
                        .to_bytes(),
                ))
            }),
            ("assignment", |p| {
                p.assignment_digest = Some("45".repeat(32))
            }),
            ("attempt", |p| p.attempt = 3),
            ("outcome", |p| p.outcome = "failed:trap".into()),
            ("output id", |p| p.outputs[0].id = "11".repeat(32)),
            ("output size", |p| p.outputs[0].size += 1),
            ("output hash", |p| p.outputs[1].sha256 = "99".repeat(32)),
            ("output order", |p| p.outputs.swap(0, 1)),
            ("dropped output", |p| {
                p.outputs.pop();
                p.job.as_mut().unwrap().segments.pop();
            }),
            ("segment", |p| {
                p.job.as_mut().unwrap().segments[0][0] = "77".repeat(32)
            }),
            ("result", |p| {
                p.job.as_mut().unwrap().result_sha256 = "34".repeat(32)
            }),
            ("usage", |p| p.usage.fuel = "34".into()),
            ("report hash", |p| p.report_sha256 = Some("00".repeat(32))),
            ("evidence", |p| p.job = None),
        ];
        for (what, edit) in edits {
            let mut r = with_job(None, "", None);
            edit(proof_mut(&mut r));
            assert!(verify_job_reports(&r).is_err(), "{what}");
        }
        let mut r = with_job(Some("trap"), "boom", None);
        proof_mut(&mut r).job.as_mut().unwrap().message = "bang".into();
        assert!(verify_job_reports(&r).is_err(), "message");
        let mut r = with_job(None, "", Some(job_output(0x50)));
        proof_mut(&mut r).job.as_mut().unwrap().log = None;
        assert!(verify_job_reports(&r).is_err(), "log");
    }

    #[test]
    fn a_proof_is_bound_to_its_run_step_and_index() {
        let mut r = with_job(None, "", None);
        r.run = "run-2".into();
        assert!(
            verify_job_reports(&r).is_err(),
            "another pipeline run's proof"
        );
        let mut r = with_job(None, "", None);
        proof_mut(&mut r).index = 1;
        assert!(verify_job_reports(&r).is_err(), "another index");
        let mut r = with_job(None, "", None);
        let runs = r.steps.remove("load").unwrap();
        let _prev = r.steps.insert("other".into(), runs);
        assert!(verify_job_reports(&r).is_err(), "another step");
    }

    #[test]
    fn a_leader_ended_run_names_no_report_outputs_or_usage() {
        let leader = |outcome: &str| {
            let mut r = with_job(None, "", None);
            let p = proof_mut(&mut r);
            p.outcome = outcome.into();
            (p.signature, p.report_sha256, p.job) = (None, None, None);
            p.outputs.clear();
            p.usage = ProofUsage {
                cpu_micros: "0".into(),
                gib_micros: "0".into(),
                fuel: "0".into(),
            };
            r
        };
        for outcome in ["failed:lost", "failed:timeout", "failed:quota", "cancelled"] {
            assert_eq!(
                verify_job_reports(&leader(outcome)).unwrap(),
                BTreeMap::new(),
                "{outcome}"
            );
        }
        let mut r = leader("failed:lost");
        proof_mut(&mut r).outputs.push(FileRef {
            id: "10".repeat(32),
            size: 1,
            sha256: "11".repeat(32),
        });
        assert!(verify_job_reports(&r).is_err(), "outputs without a report");
        let mut r = leader("failed:lost");
        proof_mut(&mut r).usage.fuel = "5".into();
        assert!(verify_job_reports(&r).is_err(), "usage without a report");
        assert!(
            verify_job_reports(&leader("done")).is_err(),
            "done without a report"
        );
        assert!(
            verify_job_reports(&leader("failed:fuel")).is_err(),
            "a job's failure without its report"
        );
        assert!(
            verify_job_reports(&leader("failed:nonsense")).is_err(),
            "a reason no run ends with"
        );
        assert!(
            verify_job_reports(&leader("exploded")).is_err(),
            "not an outcome"
        );
    }

    #[test]
    fn only_a_version_2_receipt_carries_job_reports() {
        let mut r = with_job(None, "", None);
        r.version = RECEIPT_VERSION_SIGNED;
        assert!(verify_job_reports(&r).is_err());
    }

    /// The bytes a job node signs, which a verifier in any language rebuilds from a receipt.
    #[test]
    fn the_report_message_matches_the_published_vector() {
        let outputs = [job_output(0x10)];
        let log = job_output(0x50);
        let m = report_message(&ReportFields {
            assignment: &[0x44; 32],
            app: "a",
            run: "r.s.0",
            attempt: 1,
            reason: Some("trap"),
            message: "m",
            result_sha256: &[0x33; 32],
            outputs: &outputs,
            usage: [1, 2, 3],
            log: Some(&log),
        })
        .unwrap();
        let mut want = b"calimero.compute.report.v1\0".to_vec();
        want.extend([0x44; 32]);
        want.extend(b"\0\0\0\x01a\0\0\0\x05r.s.0\0\0\0\x01\x01\0\0\0\x04trap\0\0\0\x01m");
        want.extend([0x33; 32]);
        want.extend(1u32.to_be_bytes());
        let o = &outputs[0];
        want.extend(hex::decode(&o.id).unwrap());
        want.extend(o.size.to_be_bytes());
        want.extend(hex::decode(&o.sha256).unwrap());
        want.extend(1u32.to_be_bytes());
        want.extend(hex::decode(&o.segments[0]).unwrap());
        for n in [1u64, 2, 3] {
            want.extend(n.to_be_bytes());
        }
        want.extend(b"calimero.compute.report.log.v1\0");
        want.extend([0x50; 32]);
        want.extend(800u64.to_be_bytes());
        want.extend([0x51; 32]);
        want.extend(1u32.to_be_bytes());
        want.extend([0x52; 32]);
        assert_eq!(m, want);
    }

    #[test]
    fn each_steps_runs_are_listed_once_in_index_order() {
        let cached = |index| {
            RunEntry::Cached(CachedRun {
                index,
                cached: true,
                cache_key: "07".repeat(32),
            })
        };
        let mut r = with_job(None, "", None);
        r.steps.get_mut("load").unwrap().runs.push(cached(1));
        assert!(verify_job_reports(&r).is_ok());
        let mut gap = r.clone();
        gap.steps.get_mut("load").unwrap().runs[1] = cached(2);
        assert!(verify_job_reports(&gap).is_err(), "a run left out");
        let mut twice = r.clone();
        twice.steps.get_mut("load").unwrap().runs[1] = cached(0);
        assert!(verify_job_reports(&twice).is_err(), "a run listed twice");
        let mut not_cached = r;
        not_cached.steps.get_mut("load").unwrap().runs[1] = RunEntry::Cached(CachedRun {
            index: 1,
            cached: false,
            cache_key: "07".repeat(32),
        });
        assert!(
            verify_job_reports(&not_cached).is_err(),
            "a cached entry that says it was not"
        );
    }

    /// Past 2^53 two spellings of a number canonicalize alike; the receipt answered is the one signed.
    #[test]
    fn a_verified_receipt_is_the_one_signed() {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let key = sk.verifying_key().to_bytes();
        let mut answer = signed(&sk, &receipt());
        answer.receipt["params"]["n"] = json!(9_007_199_254_740_993_u64);
        answer.signature = hex::encode(sk.sign(&receipt_message(&answer.receipt)).to_bytes());
        let mut other = answer.clone();
        other.receipt["params"]["n"] = json!(9_007_199_254_740_992_u64);
        let got = verify_pipeline_receipt(&answer, &key).unwrap();
        assert_eq!(got, verify_pipeline_receipt(&other, &key).unwrap());
        assert_eq!(got.params["n"], json!(9_007_199_254_740_992_u64));
    }

    /// A job reports a cancel it stopped at as failed with reason `cancelled`, however the run ends.
    #[test]
    fn a_cancel_the_job_reported_verifies_under_either_outcome() {
        for outcome in ["failed:cancelled", "cancelled"] {
            let mut r = with_job(Some("cancelled"), "", None);
            proof_mut(&mut r).outcome = outcome.into();
            assert!(verify_job_reports(&r).is_ok(), "{outcome}");
        }
    }

    #[test]
    fn a_job_attestation_must_name_its_key_node_cluster_and_identity() {
        let key = "aa".repeat(32);
        let doc = AttestationDoc {
            quote: String::new(),
            transport_key: "00".repeat(32),
            app_hash: "00".repeat(32),
            nonce: "01".repeat(32),
            release: crate::attest::Release {
                version: "1.0.0".into(),
                profile: "mock".into(),
            },
            cluster: "eu-1".into(),
            node: 4,
            mock: true,
            receipt_key: None,
            governance: None,
            kms: None,
            job_key: Some(key.clone()),
            cluster_identity: Some("ab".repeat(32)),
        };
        let mut r = receipt();
        let _prev = r.job_attestations.insert(key.clone(), doc.clone());
        assert!(job_attestation(&r, "eu-1", &key, 4).is_ok());
        assert!(
            job_attestation(&r, "eu-1", &"bb".repeat(32), 4).is_err(),
            "no attestation for the key"
        );
        assert!(
            job_attestation(&r, "eu-1", &key, 5).is_err(),
            "another node"
        );
        assert!(
            job_attestation(&r, "eu-2", &key, 4).is_err(),
            "another cluster"
        );
        let mut other_key = r.clone();
        other_key.job_attestations.get_mut(&key).unwrap().job_key = Some("bb".repeat(32));
        assert!(
            job_attestation(&other_key, "eu-1", &key, 4).is_err(),
            "another key's attestation"
        );
        let mut other_identity = r.clone();
        other_identity.cluster = "cd".repeat(32);
        assert!(
            job_attestation(&other_identity, "eu-1", &key, 4).is_err(),
            "another cluster identity"
        );
        let mut no_identity = r;
        no_identity
            .job_attestations
            .get_mut(&key)
            .unwrap()
            .cluster_identity = None;
        assert!(
            job_attestation(&no_identity, "eu-1", &key, 4).is_err(),
            "no cluster identity"
        );
    }
}
