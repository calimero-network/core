//! Generates `tests/fixtures/receipt-vectors.json`, the vectors mero-js tests its receipt
//! verification against, from this crate's own functions, and checks core accepts each one.
//! `UPDATE_RECEIPT_VECTORS=1 cargo test -p calimero-client --test receipt_vectors` rewrites the
//! file; without it the test fails while the committed file differs from what is built now.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_client::attest::{
    job_key_binding, receipt_key_binding, verify_job_key, verify_receipt_key, AttestationDoc,
    Governance, Policy, Release, MOCK_QUOTE_HEADER,
};
use calimero_client::receipt::{
    receipt_message, report_message, verify_job_reports, verify_pipeline_receipt, JobOutput,
    ReceiptAnswer, ReportFields,
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const UPDATE_VAR: &str = "UPDATE_RECEIPT_VECTORS"; // set to rewrite the committed file
const NONCE: [u8; 32] = [9; 32]; // the nonce of the receipt-key documents
const JOB_NONCE: [u8; 32] = [10; 32]; // the nonce of the job-key document
const NODE_1_SEED: u8 = 11; // signs as node 1 (its receipt key is in `nodes`)
const NODE_2_SEED: u8 = 22;
const CLUSTER: &str = "eu-shared-1";
const RELEASE: &str = "2.3.109";

fn hx(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(bytes)
}

fn sha(tag: &str) -> String {
    hx(Sha256::digest(tag.as_bytes()))
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public(seed: u8) -> [u8; 32] {
    key(seed).verifying_key().to_bytes()
}

fn governance(value: Value) -> Governance {
    serde_json::from_value(value).unwrap()
}

fn governances() -> [Governance; 3] {
    [
        governance(json!({"releases": "owner", "appUpgrades": "owner", "outbound": "owner"})),
        governance(json!({
            "releases": "operator", "appUpgrades": "owner", "outbound": "operator",
            "deployments": "granted", "deployers": "owner", "signUp": "invite"
        })),
        governance(json!({
            "releases": "owner", "appUpgrades": "operator", "outbound": "owner", "signUp": "closed"
        })),
    ]
}

fn mock_quote(nonce: &[u8; 32], binding: [u8; 32]) -> String {
    let mut report = [0; 64];
    report[..32].copy_from_slice(nonce);
    report[32..].copy_from_slice(&binding);
    let quote = calimero_tee_attestation::generate_mock_attestation(report).quote_bytes;
    assert!(quote.starts_with(MOCK_QUOTE_HEADER));
    // The header and report data are all a verifier reads; the padding is left out.
    STANDARD.encode(&quote[..MOCK_QUOTE_HEADER.len() + 64])
}

fn release() -> Release {
    Release {
        version: RELEASE.into(),
        profile: "locked-read-only".into(),
    }
}

fn receipt_doc(binding: [u8; 32]) -> AttestationDoc {
    AttestationDoc {
        quote: mock_quote(&NONCE, binding),
        transport_key: hx([5; 32]),
        app_hash: hx([0; 32]),
        nonce: hx(NONCE),
        release: release(),
        cluster: CLUSTER.into(),
        node: 1,
        mock: true,
        receipt_key: Some(hx(public(NODE_1_SEED))),
        governance: Some(governances()[1].clone()),
        kms: None,
        job_key: None,
        cluster_identity: None,
        interval: None,
    }
}

fn unbound_binding(receipt_key: &[u8; 32], governance: &Governance) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"calimero.tee-attest.receipt-key.v2");
    h.update(receipt_key);
    h.update(Sha256::digest(governance.canonical()));
    h.finalize().into()
}

fn bindings() -> Vec<Value> {
    let mut out = vec![];
    let mut seed = 1u8;
    for (cluster, node) in [
        (CLUSTER, 1),
        (CLUSTER, 2),
        ("caf\u{e9}-cluster", 300_000),
        ("", 0),
    ] {
        for g in governances() {
            let receipt_key = [seed; 32];
            seed += 1;
            out.push(json!({
                "receiptKey": hx(receipt_key),
                "governance": g,
                "governanceCanonical": String::from_utf8(g.canonical()).unwrap(),
                "cluster": cluster,
                "node": node,
                "binding": hx(receipt_key_binding(&receipt_key, &g, cluster, node)),
            }));
        }
    }
    out
}

fn sign(receipt: &Value, node: u64, seed: u8) -> ReceiptAnswer {
    let signature = key(seed).sign(&receipt_message(receipt));
    ReceiptAnswer {
        receipt: receipt.clone(),
        node,
        signature: hx(signature.to_bytes()),
    }
}

/// A version 1 receipt whose `params` is the given JSON text, signed by node 1 as the node
/// would send it; answers the text of the whole answer.
fn answer_text(params: &str) -> String {
    let base = serde_json::to_string(&v1_receipt(json!(0))).unwrap();
    let text = base.replace("\"params\":0", &format!("\"params\":{params}"));
    let receipt: Value = serde_json::from_str(&text).unwrap();
    let signature = key(NODE_1_SEED).sign(&receipt_message(&receipt));
    format!(
        "{{\"receipt\":{text},\"node\":1,\"signature\":\"{}\"}}",
        hx(signature.to_bytes())
    )
}

fn v1_receipt(params: Value) -> Value {
    json!({
        "version": 1,
        "cluster": hx([0xab; 32]),
        "app": "app-1",
        "run": "run-1",
        "pipeline": "etl",
        "release": "1.0.0",
        "pipelineHash": hx([0xcd; 32]),
        "modules": {"load": hx([0xef; 32]), "self": hx([0x12; 32])},
        "inputs": {"data": [{"id": hx([1; 32]), "size": 7, "sha256": hx([2; 32])}]},
        "params": params,
        "state": "succeeded",
        "startTsMs": 1_700_000_000_000u64,
        "endTsMs": 1_700_000_005_000u64,
        "steps": {"load": {"status": "succeeded", "runs": [
            {
                "run": "run-1.load.0", "step": "load", "index": 0, "outcome": "done", "attempt": 1,
                "node": 4, "jobKey": hx([3; 32]), "assignmentDigest": hx([4; 32]),
                "reportSha256": hx([5; 32]), "signature": hx([6; 64]), "outputs": [],
                "usage": {"cpuMicros": "1", "gibMicros": "2", "fuel": "3"}
            },
            {"index": 1, "cached": true, "cacheKey": hx([7; 32])}
        ]}},
    })
}

fn output(tag: &str, size: u64, segments: usize) -> JobOutput {
    JobOutput {
        id: sha(&format!("{tag}.id")),
        size,
        sha256: sha(&format!("{tag}.sha256")),
        segments: (0..segments)
            .map(|i| sha(&format!("{tag}.segment.{i}")))
            .collect(),
    }
}

struct Report {
    assignment: [u8; 32],
    app: &'static str,
    run: &'static str,
    attempt: u32,
    reason: Option<&'static str>,
    message: &'static str,
    result_sha256: [u8; 32],
    outputs: Vec<JobOutput>,
    usage: [u64; 3],
    log: Option<JobOutput>,
}

impl Report {
    fn message_bytes(&self) -> Vec<u8> {
        report_message(&ReportFields {
            assignment: &self.assignment,
            app: self.app,
            run: self.run,
            attempt: self.attempt,
            reason: self.reason,
            message: self.message,
            result_sha256: &self.result_sha256,
            outputs: &self.outputs,
            usage: self.usage,
            log: self.log.as_ref(),
        })
        .unwrap()
    }

    fn vector(&self) -> Value {
        let bytes = self.message_bytes();
        json!({
            "assignment": hx(self.assignment),
            "app": self.app,
            "run": self.run,
            "attempt": self.attempt,
            "reason": self.reason,
            "message": self.message,
            "resultSha256": hx(self.result_sha256),
            "outputs": self.outputs,
            "usage": self.usage.map(|n| n.to_string()),
            "log": self.log,
            "messageHex": hx(&bytes),
            "sha256": hx(Sha256::digest(&bytes)),
        })
    }
}

fn digest(tag: &str) -> [u8; 32] {
    Sha256::digest(tag.as_bytes()).into()
}

fn reports() -> Vec<Report> {
    let base = |tag: &'static str| Report {
        assignment: digest(&format!("{tag}.assignment")),
        app: "app-1",
        run: "run-1.load.0",
        attempt: 1,
        reason: None,
        message: "",
        result_sha256: digest(&format!("{tag}.result")),
        outputs: vec![],
        usage: [1_500, 2_500, 3_500],
        log: None,
    };
    vec![
        Report {
            outputs: vec![output("a", 4096, 3), output("b", 70_000, 2)],
            ..base("done-two-outputs")
        },
        Report {
            outputs: vec![output("c", 12, 1)],
            log: Some(output("log", 300, 2)),
            ..base("done-with-log")
        },
        Report {
            reason: Some("app-error"),
            message: "invalid input: row 7",
            attempt: 2,
            ..base("app-error")
        },
        Report {
            reason: Some("fuel"),
            usage: [u64::MAX, 0, 18_446_744_073_709_551_000],
            ..base("fuel")
        },
        Report {
            app: "caf\u{e9}-app",
            run: "run-\u{fc}.\u{65e5}\u{672c}.0",
            reason: Some("trap"),
            message: "trap: \u{1f600} \u{fc}berlauf \u{65e5}\u{672c}\u{8a9e}",
            outputs: vec![output("d", 1, 1)],
            ..base("non-ascii")
        },
    ]
}

struct JobBinding {
    transport: [u8; 32],
    job: [u8; 32],
    release: &'static str,
    cluster: &'static str,
    node: u64,
    identity: Option<[u8; 32]>,
}

fn job_binding_cases() -> Vec<JobBinding> {
    let base = || JobBinding {
        transport: [0x31; 32],
        job: public(0x41),
        release: RELEASE,
        cluster: CLUSTER,
        node: 4,
        identity: Some([0x21; 32]),
    };
    vec![
        base(),
        JobBinding {
            release: "2.4.0-rc.1",
            ..base()
        },
        JobBinding {
            cluster: "caf\u{e9}-cluster",
            ..base()
        },
        JobBinding {
            node: 300_000,
            ..base()
        },
        JobBinding {
            identity: Some([0x22; 32]),
            ..base()
        },
        JobBinding {
            identity: None,
            ..base()
        },
        JobBinding {
            transport: [0x32; 32],
            job: public(0x42),
            ..base()
        },
    ]
}

fn job_binding_vectors() -> Vec<Value> {
    job_binding_cases()
        .iter()
        .map(|c| {
            let binding = job_key_binding(
                &c.transport,
                &c.job,
                c.release,
                c.cluster,
                c.node,
                c.identity.as_ref(),
            );
            json!({
                "transportKey": hx(c.transport),
                "jobKey": hx(c.job),
                "release": c.release,
                "cluster": c.cluster,
                "node": c.node,
                "clusterIdentity": c.identity.map(hx),
                "binding": hx(binding),
            })
        })
        .collect()
}

fn job_doc(job: [u8; 32], identity: [u8; 32], node: u64) -> AttestationDoc {
    let transport = [0x31; 32];
    let binding = job_key_binding(&transport, &job, RELEASE, CLUSTER, node, Some(&identity));
    AttestationDoc {
        quote: mock_quote(&JOB_NONCE, binding),
        transport_key: hx(transport),
        app_hash: hx([0; 32]),
        nonce: hx(JOB_NONCE),
        release: release(),
        cluster: CLUSTER.into(),
        node,
        mock: true,
        receipt_key: None,
        governance: None,
        kms: None,
        job_key: Some(hx(job)),
        cluster_identity: Some(hx(identity)),
        interval: None,
    }
}

/// The version 2 receipt: a done run signed by a job node, a cached run, and a run the leader
/// ended as lost.
fn job_receipt() -> Value {
    let identity = [0x21; 32];
    let job_seed = 0x41;
    let job_key = public(job_seed);
    let node = 4;
    let outputs = vec![output("load-a", 2048, 2), output("load-b", 9, 1)];
    let log = output("load-log", 120, 1);
    let report = Report {
        assignment: digest("load.assignment"),
        app: "app-1",
        run: "run-1.load.0",
        attempt: 1,
        reason: None,
        message: "",
        result_sha256: digest("load.result"),
        outputs: outputs.clone(),
        usage: [1_500, 2_500, 3_500],
        log: Some(log.clone()),
    };
    let message = report.message_bytes();
    let signature = key(job_seed).sign(&message);
    let file = |o: &JobOutput| json!({"id": o.id, "size": o.size, "sha256": o.sha256});
    let doc = job_doc(job_key, identity, node);
    json!({
        "version": 2,
        "cluster": hx(identity),
        "app": "app-1",
        "run": "run-1",
        "pipeline": "etl",
        "release": "1.0.0",
        "pipelineHash": hx([0xcd; 32]),
        "modules": {"load": hx([0xef; 32]), "merge": hx([0xee; 32]), "self": hx([0x12; 32])},
        "inputs": {"data": [{"id": hx([1; 32]), "size": 7, "sha256": hx([2; 32])}]},
        "params": {},
        "state": "failed",
        "startTsMs": 1_700_000_000_000u64,
        "endTsMs": 1_700_000_005_000u64,
        "steps": {
            "load": {"status": "succeeded", "runs": [
                {
                    "run": "run-1.load.0", "step": "load", "index": 0, "outcome": "done",
                    "attempt": 1, "node": node, "jobKey": hx(job_key),
                    "assignmentDigest": hx(report.assignment),
                    "reportSha256": hx(Sha256::digest(&message)),
                    "signature": hx(signature.to_bytes()),
                    "outputs": outputs.iter().map(file).collect::<Vec<_>>(),
                    "usage": {"cpuMicros": "1500", "gibMicros": "2500", "fuel": "3500"},
                    "job": {
                        "resultSha256": hx(report.result_sha256),
                        "segments": outputs.iter().map(|o| o.segments.clone()).collect::<Vec<_>>(),
                        "log": log,
                    }
                },
                {"index": 1, "cached": true, "cacheKey": hx([7; 32])}
            ]},
            "merge": {"status": "failed", "reason": "lost", "runs": [
                {
                    "run": "run-1.merge.0", "step": "merge", "index": 0,
                    "outcome": "failed:lost", "attempt": 1, "node": 5, "jobKey": hx(public(0x42)),
                    "outputs": [],
                    "usage": {"cpuMicros": "0", "gibMicros": "0", "fuel": "0"}
                }
            ]}
        },
        "jobAttestations": {hx(job_key): doc},
    })
}

async fn build() -> Value {
    let mock = Policy {
        allow_mock: true,
        ..Policy::default()
    };
    let node1 = public(NODE_1_SEED);
    let g2 = governances()[1].clone();

    let doc = receipt_doc(receipt_key_binding(&node1, &g2, CLUSTER, 1));
    let unbound = receipt_doc(unbound_binding(&node1, &g2));
    verify_receipt_key(&doc, &NONCE, CLUSTER, 1, &mock)
        .await
        .unwrap();
    assert!(verify_receipt_key(&unbound, &NONCE, CLUSTER, 1, &mock)
        .await
        .is_err());

    let answer = sign(
        &v1_receipt(json!({
            "n": 3,
            "zeta": [true, null, "caf\u{e9}"],
            "alpha": {"b": "x\u{1f600}y", "a": 0},
        })),
        1,
        NODE_1_SEED,
    );
    verify_pipeline_receipt(&answer, &node1).unwrap();
    assert!(verify_pipeline_receipt(&answer, &public(NODE_2_SEED)).is_err());

    let float_text = answer_text(
        r#"{"a":0.1,"b":1e+21,"c":-0.0,"d":5e-324,"e":1.5,"f":1.7976931348623157e+308,"g":[1e-7,123456789.125,100.0,1e+22]}"#,
    );
    let big_text = answer_text(
        r#"{"a":9007199254740993,"b":5000000000000000000,"c":-9223372036854775808,"d":1e+19,"e":10000000000000000000,"f":18446744073709551615}"#,
    );
    for text in [&float_text, &big_text] {
        let a: ReceiptAnswer = serde_json::from_str(text).unwrap();
        verify_pipeline_receipt(&a, &node1).unwrap();
    }

    let reports: Vec<Value> = reports().iter().map(Report::vector).collect();

    let receipt = job_receipt();
    let job_answer = sign(&receipt, 1, NODE_1_SEED);
    let parsed = verify_pipeline_receipt(&job_answer, &node1).unwrap();
    let keys = verify_job_reports(&parsed).unwrap();
    let job_key = hx(public(0x41));
    assert_eq!(keys.get(&job_key), Some(&4));
    assert_eq!(keys.len(), 1);
    let job_doc: &AttestationDoc = &parsed.job_attestations[&job_key];
    let (_, got) = verify_job_key(job_doc, &JOB_NONCE, &mock).await.unwrap();
    assert_eq!(hx(got), job_key);

    json!({
        "bindings": bindings(),
        "nonce": hx(NONCE),
        "cluster": CLUSTER,
        "governance": g2,
        "nodes": {"1": hx(public(NODE_1_SEED)), "2": hx(public(NODE_2_SEED))},
        "mockDoc": doc,
        "unboundMockDoc": unbound,
        "receiptAnswer": answer,
        "floatAnswerText": float_text,
        "bigNumberAnswerText": big_text,
        "reportMessages": reports,
        "jobKeyBindings": job_binding_vectors(),
        "jobReceipt": {
            "clusterIdentity": hx([0x21; 32]),
            "jobNonce": hx(JOB_NONCE),
            "answer": job_answer,
        },
    })
}

#[tokio::test]
async fn the_committed_receipt_vectors_are_what_core_produces() {
    let mut text = serde_json::to_string_pretty(&build().await).unwrap();
    text.push('\n');
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/receipt-vectors.json");
    if std::env::var_os(UPDATE_VAR).is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == text,
        "{} differs from what core produces; run `{UPDATE_VAR}=1 cargo test -p calimero-client --test receipt_vectors` and refresh mero-js's copy",
        path.display()
    );
}

#[test]
fn every_report_vector_is_what_report_message_returns() {
    for r in reports() {
        let v = r.vector();
        let bytes = r.message_bytes();
        assert_eq!(v["messageHex"], hx(&bytes));
        assert_eq!(v["sha256"], hx(Sha256::digest(&bytes)));
        assert_eq!(v["reason"].is_null(), r.reason.is_none());
    }
}
