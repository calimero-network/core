//! Compute runs on job nodes: the test plays the leader of a cluster, signs assignments with
//! the real node's receipt key, wraps keys with the files crypto and checks the signed reports.
//! The input is a real sealed file of the `files` fixture; the runs are the `compute` fixture's.

mod common;

use std::sync::Mutex;
use std::time::Duration;

use calimero_client::Client;
use calimero_node::backup::cluster_identity_key;
use calimero_node::files::{
    noise_seed, run_token, unwrap_key, wrap_for_job, FileOpener, JobWrap, WrapKind,
};
use calimero_node::job_node::{MAX_RUN_INPUTS, TOKEN_HEADER};
use calimero_node::job_wire::{
    assignment_digest, sign_assignment, sign_bundle, verify_report, Assignment, FailReason, Report,
    ReportStatus, RunBudget, RunInput, BUNDLE_SIGNATURE_HEADER,
};
use calimero_node::types::Caller;
use calimero_node::{run_job, JobConfig, Node};
use calimero_sealed::client::{resolve_path, SealedTransport};
use common::{signed_up, Cluster, JobNode};
use rand_chacha::rand_core::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use reqwest::Method;
use serde_json::{json, Value};
use url::Url;

const MIB: usize = 1024 * 1024;
const INPUT_LEN: usize = 3 * MIB + MIB / 2 + 1000; // not a multiple of the 64 KiB chunk
const READ_PIECE: usize = 64 * 1024; // what the fixture reads and keys at a time
const START_TS_MS: u64 = 1_800_000_000_000;
const JOB_A: u64 = 4;
const JOB_B: u64 = 5;
const JOB_C: u64 = 6;
const RUN_WAIT: Duration = Duration::from_secs(120);

const DIGEST: RunBudget = RunBudget {
    fuel: 20_000_000_000,
    memory_mib: 256,
    wall_secs: 60,
};

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(calimero_identity::sha256(bytes))
}

/// The bytes `putFile(len, seed)` writes.
fn stream(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out.truncate(len);
    out
}

/// A job node as the leader talks to it: over a sealed channel pinned to its attested key.
struct JobClient {
    channel: SealedTransport,
    base: Url,
    transport_key: [u8; 32],
    job_key: [u8; 32],
}

impl JobClient {
    async fn request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        self.request_with(method, path, &[(TOKEN_HEADER, token)], body)
            .await
    }

    async fn request_with(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, Option<&str>)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        let mut request = reqwest::Client::new()
            .request(method, resolve_path(&self.base, path).unwrap())
            .body(body);
        for (name, value) in headers {
            if let Some(value) = value {
                request = request.header(*name, *value);
            }
        }
        let response = self
            .channel
            .execute(request.build().unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.bytes().await.unwrap().to_vec())
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> (u16, Value) {
        let (status, bytes) = self.request(method, path, token, body).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The component, signed by `signature` (the cluster identity's, hex) if there is one.
    async fn put_bundle_signed(&self, bytes: &[u8], signature: Option<&str>) -> (u16, Value) {
        let path = format!("job/bundles/{}", sha256_hex(bytes));
        let (status, body) = self
            .request_with(
                Method::PUT,
                &path,
                &[(BUNDLE_SIGNATURE_HEADER, signature)],
                bytes.to_vec(),
            )
            .await;
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn post(&self, a: &Assignment) -> (u16, Value) {
        let body = serde_json::to_vec(a).unwrap();
        self.json(Method::POST, "job/runs", None, body).await
    }

    async fn put_input(&self, a: &Assignment, i: usize, token: &str, bytes: &[u8]) -> (u16, Value) {
        let path = format!("job/runs/{}/inputs/{i}", run_path(a));
        self.json(Method::PUT, &path, Some(token), bytes.to_vec())
            .await
    }

    async fn status(&self, a: &Assignment, token: &str) -> (u16, Value) {
        let path = format!("job/runs/{}", run_path(a));
        self.json(Method::GET, &path, Some(token), Vec::new()).await
    }

    async fn cancel(&self, a: &Assignment, token: &str) -> (u16, Value) {
        let path = format!("job/runs/{}", run_path(a));
        self.json(Method::DELETE, &path, Some(token), Vec::new())
            .await
    }

    async fn load(&self) -> Value {
        self.json(Method::GET, "job/load", None, Vec::new()).await.1
    }

    /// Polls until the run is done or lost.
    async fn finish(&self, a: &Assignment, token: &str) -> Value {
        let started = std::time::Instant::now();
        loop {
            let (status, answer) = self.status(a, token).await;
            assert_eq!(status, 200, "{answer}");
            if matches!(answer["state"].as_str(), Some("done" | "lost")) {
                return answer;
            }
            assert!(
                started.elapsed() < RUN_WAIT,
                "the run did not end: {answer}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The output's ciphertext and data key.
    async fn output(&self, a: &Assignment, token: &str, i: usize) -> (Vec<u8>, [u8; 32]) {
        let base = format!("job/runs/{}/outputs/{i}", run_path(a));
        let (status, ciphertext) = self
            .request(Method::GET, &base, Some(token), Vec::new())
            .await;
        assert_eq!(status, 200);
        let (status, key) = self
            .json(Method::GET, &format!("{base}/key"), Some(token), Vec::new())
            .await;
        assert_eq!(status, 200, "{key}");
        let key = hex::decode(key["key"].as_str().unwrap()).unwrap();
        (ciphertext, key.try_into().unwrap())
    }
}

/// `{app}/{run}/{attempt}`, each part escaped as a path segment.
fn run_path(a: &Assignment) -> String {
    let seg = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!("{}/{}/{}", seg(&a.app), seg(&a.run), a.attempt)
}

struct Input {
    id: String,
    size: u64,
    sha256: [u8; 32],
    ciphertext: Vec<u8>,
    key: [u8; 32],
    plain: Vec<u8>,
}

struct Rig {
    c: Cluster,
    app: String,
    alice_bundle: String,
    file_key: [u8; 32],
    bundle: Vec<u8>,
    alice: Client,
}

/// Hashing megabytes in the guest costs more fuel than the default call budget.
fn more_fuel(cfg: &mut calimero_node::Config) {
    cfg.budget = Some(calimero_app_host::Budget {
        fuel: u64::MAX,
        ..calimero_app_host::Budget::call()
    });
}

impl Rig {
    async fn start() -> Self {
        let c = Cluster::start_with(3, more_fuel).await;
        let (alice, _pk) = signed_up(&c, 0, "alice").await;
        let bundle = alice
            .upload_bundle(common::component("files"))
            .await
            .unwrap();
        let app = alice.create_app("files", &bundle, json!({})).await.unwrap();
        let file_key = c.nodes[0].as_ref().unwrap().node.app_file_key(&app);
        Self {
            c,
            app,
            alice_bundle: bundle,
            file_key,
            bundle: common::component("compute"),
            alice,
        }
    }

    fn leader(&self) -> &Node {
        &self.c.nodes[0].as_ref().unwrap().node
    }

    /// Starts job node `id`, registers it and attests it as the leader would.
    async fn job(&self, id: u64) -> (JobNode, JobClient) {
        let job = self.c.start_job_node(id, |_| {}).await;
        self.c.register_job_node(id, &job.url).await;
        let client = self.attest(id, &job.url).await;
        (job, client)
    }

    /// A client for a job node the leader would not attest (it takes another cluster's work):
    /// its keys are read from its document without the checks.
    async fn attest_unchecked(&self, url: &str) -> JobClient {
        let nonce = hex::encode([3u8; 32]);
        let doc: Value = reqwest::get(format!("{url}attestation?nonce={nonce}&purpose=jobs"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let key = |field: &str| -> [u8; 32] {
            hex::decode(doc[field].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap()
        };
        let base: Url = url.parse().unwrap();
        JobClient {
            channel: SealedTransport::with_key(
                base.clone(),
                reqwest::Client::new(),
                key("transportKey"),
            ),
            base,
            transport_key: key("transportKey"),
            job_key: key("jobKey"),
        }
    }

    async fn attest(&self, id: u64, url: &str) -> JobClient {
        let record = self.leader().sys(|s| s.apps[&self.app].clone());
        let peer = self.leader().attest_job(id, &record).await.unwrap();
        let base: Url = url.parse().unwrap();
        JobClient {
            channel: SealedTransport::with_key(
                base.clone(),
                reqwest::Client::new(),
                peer.transport_key,
            ),
            base,
            transport_key: peer.transport_key,
            job_key: peer.job_key,
        }
    }

    /// A sealed file of the `files` app, as the leader knows it: its mark and its data key.
    async fn input(&self, seed: u64) -> Input {
        let r = self
            .alice
            .call(
                &self.app,
                "putFile",
                json!({ "len": INPUT_LEN.to_string(), "seed": seed.to_string() }),
                None,
            )
            .await
            .unwrap();
        let id = r.result["id"].as_str().unwrap().to_owned();
        let leader = self.c.app_leader(&self.app).unwrap();
        let node = &self.c.nodes[leader].as_ref().unwrap().node;
        let mark = node
            .hosted(&self.app)
            .unwrap()
            .read_local(|m, _| m.state.component.files.get(&id).cloned())
            .unwrap();
        let key = unwrap_key(&self.file_key, &self.app, &mark.wrapped_key.0).unwrap();
        let plain = stream(INPUT_LEN, seed);
        assert_eq!(hex::encode(&mark.sha256.0), sha256_hex(&plain));
        Input {
            ciphertext: node.engine.get_blob(&id).unwrap().unwrap(),
            sha256: mark.sha256.0.as_slice().try_into().unwrap(),
            size: mark.size,
            key: *key,
            plain,
            id,
        }
    }

    /// The signed assignment of `method` to `job`, its keys wrapped to that node.
    fn assignment(
        &self,
        job: &JobClient,
        run: &str,
        attempt: u32,
        method: &str,
        budget: RunBudget,
        inputs: &[&Input],
    ) -> Assignment {
        self.assignment_for(&self.app, job, run, attempt, method, budget, inputs)
    }

    /// The same for another app of the cluster: its own file key, wraps and token.
    #[allow(clippy::too_many_arguments, reason = "a test helper naming every part")]
    fn assignment_for(
        &self,
        app: &str,
        job: &JobClient,
        run: &str,
        attempt: u32,
        method: &str,
        budget: RunBudget,
        inputs: &[&Input],
    ) -> Assignment {
        let file_key = self.leader().app_file_key(app);
        let wrap = |kind, index| JobWrap {
            app_id: app,
            run_id: run,
            attempt,
            kind,
            index,
        };
        let mut a = Assignment {
            app: app.to_owned(),
            run: run.to_owned(),
            attempt,
            method: method.to_owned(),
            args: Vec::new(),
            bundle: sha256_hex(&self.bundle),
            caller: Caller::System,
            owner: "owner".to_owned(),
            start_ts_ms: START_TS_MS,
            budget,
            inputs: inputs
                .iter()
                .enumerate()
                .map(|(i, input)| RunInput {
                    id: input.id.clone(),
                    size: input.size,
                    sha256: hex::encode(input.sha256),
                    wrapped: wrap_for_job(
                        &job.transport_key,
                        &wrap(WrapKind::FileKey, i as u32),
                        &input.key,
                    )
                    .unwrap(),
                })
                .collect(),
            seed: wrap_for_job(
                &job.transport_key,
                &wrap(WrapKind::NoiseSeed, 0),
                &noise_seed(&file_key, run),
            )
            .unwrap(),
            token: token(&file_key, run, attempt),
            signature: String::new(),
        };
        a.signature = self.leader().sign_assignment(&a).unwrap();
        a
    }

    /// Pushes the component, assigns, pushes every input and waits for the end.
    /// Pushes the component to `job` with the cluster identity's signature of its name.
    async fn push(&self, job: &JobClient) -> (u16, Value) {
        let signature = self
            .leader()
            .sign_bundle(&sha256_hex(&self.bundle))
            .unwrap();
        job.put_bundle_signed(&self.bundle, Some(&signature)).await
    }

    async fn run(&self, job: &JobClient, a: &Assignment, inputs: &[&Input]) -> Value {
        let (status, answer) = self.push(job).await;
        assert_eq!(status, 200, "{answer}");
        let (status, answer) = job.post(a).await;
        assert_eq!(status, 200, "{answer}");
        let token = self.token(a);
        for (i, input) in inputs.iter().enumerate() {
            let (status, answer) = job.put_input(a, i, &token, &input.ciphertext).await;
            assert_eq!(status, 200, "{answer}");
        }
        job.finish(a, &token).await
    }

    fn token(&self, a: &Assignment) -> String {
        token(&self.leader().app_file_key(&a.app), &a.run, a.attempt)
    }
}

fn token(file_key: &[u8; 32], run: &str, attempt: u32) -> String {
    hex::encode(run_token(file_key, run, attempt))
}

/// The signed report in `answer`, checked against the assignment `a` it is for.
fn report(answer: &Value, job: &JobClient, a: &Assignment) -> Report {
    assert_eq!(answer["state"], "done", "{answer}");
    let report: Report = serde_json::from_value(answer["report"].clone()).unwrap();
    let signature = answer["signature"].as_str().unwrap();
    let digest = assignment_digest(a).unwrap();
    assert!(
        verify_report(&job.job_key, &digest, &report, signature),
        "{answer}"
    );
    report
}

fn failed(answer: &Value, job: &JobClient, a: &Assignment, reason: FailReason) {
    let report = report(answer, job, a);
    assert_eq!(report.status, ReportStatus::Failed, "{answer}");
    assert_eq!(report.reason, Some(reason), "{answer}");
    assert!(report.outputs.is_empty() && report.result.is_empty());
}

fn open(ciphertext: &[u8], key: &[u8; 32], size: u64, sha256: &[u8; 32]) -> Vec<u8> {
    let mut opener = FileOpener::new(key);
    let mut plain = opener.push(ciphertext).unwrap();
    plain.extend(opener.finish_expecting(size, sha256).unwrap());
    plain
}

/// The output the `digest` fixture writes: the input XOR the run's seeded random stream.
fn expected_output(plain: &[u8], seed: [u8; 32]) -> Vec<u8> {
    let mut rng = ChaCha20Rng::from_seed(seed);
    let mut out = Vec::with_capacity(plain.len());
    for piece in plain.chunks(READ_PIECE) {
        let mut keystream = vec![0u8; piece.len()];
        rng.fill_bytes(&mut keystream);
        out.extend(piece.iter().zip(&keystream).map(|(p, k)| p ^ k));
    }
    out
}

async fn wasm_series(c: &Cluster) -> Vec<String> {
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let text = ops.get_text("ops/metrics").await.unwrap();
    text.lines()
        .filter(|l| l.starts_with("calimero_app_"))
        .map(str::to_owned)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_streams_its_input_and_seals_its_output_and_a_second_node_writes_the_same_bytes() {
    let rig = Rig::start().await;
    let (_a, job_a) = rig.job(JOB_A).await;
    let (_b, job_b) = rig.job(JOB_B).await;
    let input = rig.input(7).await;
    let metered = wasm_series(&rig.c).await;

    let run = "run-1";
    let a = rig.assignment(&job_a, run, 1, "digest", DIGEST, &[&input]);
    // The component is pushed on demand: an assignment before it is refused.
    let (status, refused) = job_a.post(&a).await;
    assert_eq!(
        (status, refused["error"].as_str()),
        (409, Some("NEED_BUNDLE"))
    );
    let answer = rig.run(&job_a, &a, &[&input]).await;
    let report_a = report(&answer, &job_a, &a);
    assert_eq!(report_a.status, ReportStatus::Done);
    assert_eq!(report_a.reason, None);
    assert_eq!(
        serde_json::from_slice::<Vec<String>>(&report_a.result).unwrap(),
        vec![hex::encode(input.sha256)]
    );
    assert_eq!(
        (
            report_a.app.as_str(),
            report_a.run.as_str(),
            report_a.attempt
        ),
        (rig.app.as_str(), run, 1)
    );
    assert!(report_a.usage.fuel > 0 && report_a.usage.gib_micros > 0);

    // The output is sealed on the job node; the leader fetches it with its key and opens it.
    let token_a = rig.token(&a);
    let out = &report_a.outputs[0];
    let (ciphertext, key) = job_a.output(&a, &token_a, 0).await;
    assert_eq!(out.id, sha256_hex(&ciphertext));
    let plain_a = open(
        &ciphertext,
        &key,
        out.size,
        &hex::decode(&out.sha256).unwrap().try_into().unwrap(),
    );
    assert_eq!(plain_a.len(), INPUT_LEN);
    assert_eq!(
        plain_a,
        expected_output(&input.plain, noise_seed(&rig.file_key, run))
    );
    assert_ne!(plain_a, input.plain);
    assert_eq!(job_a.status(&a, &token_a).await.1["state"], "done");

    // Asking again answers what became of it; another assignment for the same attempt is refused.
    let (status, again) = job_a.post(&a).await;
    assert_eq!((status, again["state"].as_str()), (200, Some("done")));
    let mut other = a.clone();
    other.args = b"[]".to_vec();
    other.signature = rig.leader().sign_assignment(&other).unwrap();
    let (status, refused) = job_a.post(&other).await;
    assert_eq!(
        (status, refused["error"].as_str()),
        (409, Some("RUN_CONFLICT"))
    );
    // Outputs are the run's only: another index, and a key before the end.
    let path = format!("job/runs/{}/outputs/1", run_path(&a));
    assert_eq!(
        job_a
            .json(Method::GET, &path, Some(&token_a), Vec::new())
            .await
            .0,
        404
    );

    // A second node, the same assignment and seed, writes the same plaintext.
    let b = rig.assignment(&job_b, run, 1, "digest", DIGEST, &[&input]);
    let answer = rig.run(&job_b, &b, &[&input]).await;
    let report_b = report(&answer, &job_b, &b);
    let (ciphertext_b, key_b) = job_b.output(&b, &rig.token(&b), 0).await;
    let plain_b = open(
        &ciphertext_b,
        &key_b,
        report_b.outputs[0].size,
        &hex::decode(&report_b.outputs[0].sha256)
            .unwrap()
            .try_into()
            .unwrap(),
    );
    assert_eq!(plain_a, plain_b);
    assert_eq!(report_a.result, report_b.result);
    assert_eq!(report_a.outputs[0].sha256, report_b.outputs[0].sha256);
    // The ciphertexts differ: each node seals under a data key of its own.
    assert_ne!(ciphertext, ciphertext_b);

    // Nothing of the cluster's usage series moved: a compute run is counted nowhere yet.
    assert_eq!(wasm_series(&rig.c).await, metered);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_that_breaks_its_limits_fails_with_the_reason_and_a_signed_report() {
    let rig = Rig::start().await;
    let (_a, job) = rig.job(JOB_A).await;
    let small = |fuel, memory_mib, wall_secs| RunBudget {
        fuel,
        memory_mib,
        wall_secs,
    };
    let cases = [
        ("spin", small(1_000_000_000_000, 64, 2), FailReason::Time),
        ("hog", small(20_000_000_000, 64, 30), FailReason::Memory),
        ("burn", small(1_000_000, 64, 30), FailReason::Fuel),
        ("peek", small(20_000_000_000, 64, 30), FailReason::Trap),
    ];
    for (i, (method, budget, reason)) in cases.into_iter().enumerate() {
        let mut a = rig.assignment(&job, &format!("limit-{i}"), 1, method, budget, &[]);
        if method == "peek" {
            a.args = br#"{"id":"not-an-input"}"#.to_vec();
        }
        a.signature = rig.leader().sign_assignment(&a).unwrap();
        let answer = rig.run(&job, &a, &[]).await;
        failed(&answer, &job, &a, reason);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_altered_input_fails_the_run_as_integrity_and_a_malformed_one_is_refused() {
    let rig = Rig::start().await;
    let (_a, job) = rig.job(JOB_A).await;
    let input = rig.input(3).await;
    let mut tampered = rig.input(3).await;
    let middle = tampered.ciphertext.len() / 2;
    tampered.ciphertext[middle] ^= 1;
    // What the leader sent is named by its own hash, so it arrives; it is not what the mark says.
    tampered.id = sha256_hex(&tampered.ciphertext);

    let a = rig.assignment(&job, "tamper", 1, "digest", DIGEST, &[&tampered]);
    let answer = rig.run(&job, &a, &[&tampered]).await;
    failed(&answer, &job, &a, FailReason::Integrity);

    // A file that is not the one the mark describes: the hash at its end gives it away.
    let mut other = rig.input(4).await;
    other.sha256 = input.sha256;
    let a = rig.assignment(&job, "wrong-mark", 1, "digest", DIGEST, &[&other]);
    failed(
        &rig.run(&job, &a, &[&other]).await,
        &job,
        &a,
        FailReason::Integrity,
    );

    // Truncated input, wrong token, an input the run does not have.
    let a = rig.assignment(&job, "malformed", 1, "digest", DIGEST, &[&input]);
    let (status, _) = rig.push(&job).await;
    assert_eq!(status, 200);
    assert_eq!(job.post(&a).await.0, 200);
    let token = rig.token(&a);
    let short = &input.ciphertext[..input.ciphertext.len() - 1];
    let (status, e) = job.put_input(&a, 0, &token, short).await;
    assert_eq!((status, e["error"].as_str()), (400, Some("BAD_INPUT_SIZE")));
    let (status, e) = job
        .put_input(&a, 0, &hex::encode([1u8; 32]), &input.ciphertext)
        .await;
    assert_eq!((status, e["error"].as_str()), (404, Some("UNKNOWN_RUN")));
    assert_eq!(job.put_input(&a, 1, &token, &input.ciphertext).await.0, 404);
    assert_eq!(job.status(&a, &token).await.1["state"], "awaiting");
    assert_eq!(job.put_input(&a, 0, &token, &input.ciphertext).await.0, 200);
    assert_eq!(job.finish(&a, &token).await["state"], "done");
    // A late copy of an input a started run already has changes nothing.
    assert_eq!(job.put_input(&a, 0, &token, &input.ciphertext).await.0, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_takes_work_only_from_its_cluster_leader_with_its_runs_token_and_keys() {
    let rig = Rig::start().await;
    let (_a, job_a) = rig.job(JOB_A).await;
    let (_b, job_b) = rig.job(JOB_B).await;
    // A job node that takes the work of another cluster.
    let c = rig
        .c
        .start_job_node(JOB_C, |cfg| {
            cfg.cluster_identity = cluster_identity_key(&[9; 32]).verifying_key().to_bytes();
        })
        .await;
    rig.c.register_job_node(JOB_C, &c.url).await;
    let job_c = rig.attest_unchecked(&c.url).await;
    let (status, _) = rig.push(&job_a).await;
    assert_eq!(status, 200);
    assert_eq!(rig.push(&job_b).await.0, 200);
    // The cluster's signature is not good for a job node of another cluster.
    assert_eq!(rig.push(&job_c).await.0, 403);
    let code = |r: (u16, Value)| (r.0, r.1["error"].as_str().unwrap_or("").to_owned());
    let resign = |a: &mut Assignment| a.signature = rig.leader().sign_assignment(a).unwrap();

    let good = rig.assignment(
        &job_a,
        "admit",
        1,
        "burn",
        RunBudget {
            fuel: 1_000_000,
            memory_mib: 64,
            wall_secs: 30,
        },
        &[],
    );

    // Changed after signing, unsigned, or signed by the identity of another cluster.
    let mut changed = good.clone();
    changed.budget.fuel += 1;
    assert_eq!(
        code(job_a.post(&changed).await),
        (403, "BAD_SIGNATURE".into())
    );
    let mut unsigned = good.clone();
    unsigned.signature.clear();
    assert_eq!(
        code(job_a.post(&unsigned).await),
        (403, "BAD_SIGNATURE".into())
    );
    let mut foreign = good.clone();
    foreign.signature = sign_assignment(&cluster_identity_key(&[9; 32]), &foreign).unwrap();
    assert_eq!(
        code(job_a.post(&foreign).await),
        (403, "BAD_SIGNATURE".into())
    );
    // A job node of another cluster refuses what this cluster signed.
    assert_eq!(code(job_c.post(&good).await), (403, "BAD_SIGNATURE".into()));

    // Keys wrapped for another node, or for another attempt, do not open.
    let for_a = rig.assignment(
        &job_a,
        "wraps",
        1,
        "burn",
        RunBudget {
            fuel: 1_000_000,
            memory_mib: 64,
            wall_secs: 30,
        },
        &[],
    );
    assert_eq!(code(job_b.post(&for_a).await), (400, "BAD_WRAP".into()));
    let mut attempt_two = for_a.clone();
    attempt_two.attempt = 2;
    resign(&mut attempt_two);
    assert_eq!(
        code(job_a.post(&attempt_two).await),
        (400, "BAD_WRAP".into())
    );
    let (input, second) = (rig.input(5).await, rig.input(8).await);
    let mut one_key = rig.assignment(&job_a, "swap", 1, "digest", DIGEST, &[&input, &second]);
    one_key.inputs[1].wrapped = one_key.inputs[0].wrapped.clone();
    resign(&mut one_key);
    assert_eq!(code(job_a.post(&one_key).await), (400, "BAD_WRAP".into()));

    // An input named twice.
    let mut twice = rig.assignment(&job_a, "twice", 1, "digest", DIGEST, &[&input, &second]);
    twice.inputs[1] = twice.inputs[0].clone();
    resign(&mut twice);
    assert_eq!(
        code(job_a.post(&twice).await),
        (400, "BAD_ASSIGNMENT".into())
    );

    // The shape of an assignment: budgets, methods, sizes.
    let shaped = |edit: &dyn Fn(&mut Assignment)| {
        let mut a = good.clone();
        a.run = format!("shape-{}", rand_run());
        edit(&mut a);
        resign(&mut a);
        a
    };
    for (edit, want) in [
        (
            Box::new(|a: &mut Assignment| a.budget.fuel = 0) as Box<dyn Fn(&mut Assignment)>,
            (400, "BAD_BUDGET"),
        ),
        (
            Box::new(|a| a.budget.fuel = calimero_app_host::MAX_COMPUTE_FUEL + 1),
            (400, "BAD_BUDGET"),
        ),
        (
            Box::new(|a| a.budget.memory_mib = calimero_app_host::MAX_COMPUTE_MEMORY_MIB + 1),
            (400, "BAD_BUDGET"),
        ),
        (
            Box::new(|a| a.budget.wall_secs = calimero_app_host::MAX_COMPUTE_WALL_SECS + 1),
            (400, "BAD_BUDGET"),
        ),
        (Box::new(|a| a.method = "done".into()), (400, "NOT_COMPUTE")),
        (
            Box::new(|a| a.method = "nothing".into()),
            (400, "NOT_COMPUTE"),
        ),
        (Box::new(|a| a.token = "00".into()), (400, "BAD_ASSIGNMENT")),
        (Box::new(|a| a.run.clear()), (400, "BAD_ASSIGNMENT")),
        (
            Box::new(|a| a.bundle = "00".repeat(32)),
            (409, "NEED_BUNDLE"),
        ),
    ] {
        let a = shaped(&*edit);
        assert_eq!(code(job_a.post(&a).await), (want.0, want.1.to_owned()));
    }
    let many = |n: usize, size: u64| {
        let mut a = good.clone();
        a.run = format!("many-{}", rand_run());
        a.inputs = (0..n)
            .map(|i| RunInput {
                id: sha256_hex(&i.to_be_bytes()),
                size,
                sha256: "00".repeat(32),
                wrapped: Vec::new(),
            })
            .collect();
        resign(&mut a);
        a
    };
    let too_many = many(MAX_RUN_INPUTS + 1, 1);
    assert_eq!(
        code(job_a.post(&too_many).await),
        (413, "TOO_MANY_INPUTS".into())
    );
    let too_big = many(1, calimero_app_host::MAX_FILE_BYTES + 1);
    assert_eq!(
        code(job_a.post(&too_big).await),
        (413, "INPUT_TOO_LARGE".into())
    );
    let too_much = many(MAX_RUN_INPUTS, calimero_app_host::MAX_FILE_BYTES);
    assert_eq!(
        code(job_a.post(&too_much).await),
        (413, "INPUTS_TOO_LARGE".into())
    );
    assert_eq!(
        job_a
            .json(Method::POST, "job/runs", None, b"not json".to_vec())
            .await
            .0,
        400
    );

    // The token: every read of a run needs it.
    assert_eq!(job_a.post(&good).await.0, 200);
    let token = rig.token(&good);
    for wrong in [hex::encode([9u8; 32]), "zz".to_owned(), String::new()] {
        assert_eq!(
            code(job_a.status(&good, &wrong).await),
            (404, "UNKNOWN_RUN".into())
        );
        assert_eq!(
            code(job_a.cancel(&good, &wrong).await),
            (404, "UNKNOWN_RUN".into())
        );
    }
    let path = format!("job/runs/{}/outputs/0", run_path(&good));
    assert_eq!(
        job_a.json(Method::GET, &path, None, Vec::new()).await.0,
        404
    );
    // A wrong token is told apart from an unknown run by nothing.
    let mut nobody = good.clone();
    nobody.run = "nobody".into();
    assert_eq!(
        job_a.status(&good, &hex::encode([9u8; 32])).await,
        job_a.status(&nobody, &token).await
    );
    // The same token does not open another attempt or run.
    let mut elsewhere = good.clone();
    elsewhere.attempt = 2;
    assert_eq!(
        code(job_a.status(&elsewhere, &token).await),
        (404, "UNKNOWN_RUN".into())
    );
    assert_eq!(job_a.finish(&good, &token).await["state"], "done");
    // An output the run did not write.
    assert_eq!(
        code(
            job_a
                .json(Method::GET, &path, Some(&token), Vec::new())
                .await
        ),
        (404, "NO_SUCH_OUTPUT".into())
    );
}

fn rand_run() -> String {
    static NEXT: Mutex<u64> = Mutex::new(0);
    let mut n = NEXT.lock().unwrap();
    *n += 1;
    n.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_takes_two_runs_at_once_and_a_cancel_ends_one_as_cancelled() {
    let rig = Rig::start().await;
    let (_a, job) = rig.job(JOB_A).await;
    let long_spin = RunBudget {
        fuel: 1_000_000_000_000,
        memory_mib: 64,
        wall_secs: 60,
    };
    let one = rig.assignment(&job, "cap-1", 1, "spin", long_spin, &[]);
    let two = rig.assignment(&job, "cap-2", 1, "spin", long_spin, &[]);
    let three = rig.assignment(&job, "cap-3", 1, "spin", long_spin, &[]);
    assert_eq!(rig.push(&job).await.0, 200);
    assert_eq!(job.post(&one).await.0, 200);
    // A run waiting for an input takes room too.
    let input = rig.input(9).await;
    let waiting = rig.assignment(&job, "cap-wait", 1, "digest", DIGEST, &[&input]);
    assert_eq!(job.post(&waiting).await.1["state"], "awaiting");
    assert_eq!(job.load().await, json!({ "running": 2, "capacity": 2 }));
    let (status, busy) = job.post(&two).await;
    assert_eq!((status, busy["error"].as_str()), (503, Some("BUSY")));

    // Cancelling a run that never started frees its room; the leader sees it as lost.
    let (status, ended) = job.cancel(&waiting, &rig.token(&waiting)).await;
    assert_eq!((status, ended["state"].as_str()), (200, Some("lost")));
    assert_eq!(job.status(&waiting, &rig.token(&waiting)).await.0, 404);
    assert_eq!(job.post(&two).await.0, 200);
    assert_eq!(job.post(&three).await.0, 503);

    // Cancelling one that runs stops it at the next tick, and the signed report says so.
    let token = rig.token(&one);
    let started = std::time::Instant::now();
    while job.status(&one, &token).await.1["state"] != "running" {
        assert!(started.elapsed() < RUN_WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (status, ended) = job.cancel(&one, &token).await;
    assert_eq!(status, 200, "{ended}");
    failed(&ended, &job, &one, FailReason::Cancelled);
    assert_eq!(job.status(&one, &token).await.0, 404);
    assert_eq!(job.load().await["running"], 1);
    failed_after_cancel(&job, &two, &rig.token(&two)).await;
    assert_eq!(job.load().await["running"], 0);
}

async fn failed_after_cancel(job: &JobClient, a: &Assignment, token: &str) {
    let (status, ended) = job.cancel(a, token).await;
    assert_eq!(status, 200);
    failed(&ended, job, a, FailReason::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_restarted_job_node_knows_no_runs_and_holds_no_old_files() {
    let rig = Rig::start().await;
    let kept: Mutex<Option<JobConfig>> = Mutex::new(None);
    let job = rig
        .c
        .start_job_node(JOB_A, |cfg| *kept.lock().unwrap() = Some(cfg.clone()))
        .await;
    rig.c.register_job_node(JOB_A, &job.url).await;
    let client = rig.attest(JOB_A, &job.url).await;
    let input = rig.input(2).await;
    let a = rig.assignment(&client, "restart", 1, "digest", DIGEST, &[&input]);
    let answer = rig.run(&client, &a, &[&input]).await;
    assert_eq!(report(&answer, &client, &a).status, ReportStatus::Done);
    let config = kept.lock().unwrap().clone().unwrap();
    let scratch = config.data_dir.join("scratch");
    assert!(
        std::fs::read_dir(&scratch).unwrap().count() > 0,
        "the output is on disk"
    );

    job.running.stop().await;
    let _restarted = run_job(config).await.unwrap();
    // Its channel is new: the leader's cached attestation fails once, then attests again.
    let record = rig.leader().sys(|s| s.apps[&rig.app].clone());
    let stale = rig.leader().attest_job(JOB_A, &record).await.unwrap();
    assert!(rig.leader().job_load(&stale).await.is_err());
    let client = rig.attest(JOB_A, &job.url).await;
    let (status, e) = client.status(&a, &rig.token(&a)).await;
    assert_eq!((status, e["error"].as_str()), (404, Some("UNKNOWN_RUN")));
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
    assert_eq!(client.load().await["running"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_takes_components_only_when_the_cluster_signed_their_name() {
    let rig = Rig::start().await;
    let (_a, job) = rig.job(JOB_A).await;
    let hash = sha256_hex(&rig.bundle);
    let good = rig.leader().sign_bundle(&hash).unwrap();
    let code = |r: (u16, Value)| (r.0, r.1["error"].as_str().unwrap_or("").to_owned());
    let foreign = sign_bundle(&cluster_identity_key(&[9; 32]), &hash).unwrap();
    let other_name = sign_bundle(&cluster_identity_key(&[9; 32]), &"00".repeat(32)).unwrap();
    let a_name_only = rig.leader().sign_bundle(&"00".repeat(32)).unwrap();
    for bad in [
        None,
        Some(""),
        Some("zz"),
        Some(foreign.as_str()),
        Some(other_name.as_str()),
        Some(a_name_only.as_str()),
    ] {
        assert_eq!(
            code(job.put_bundle_signed(&rig.bundle, bad).await),
            (403, "BAD_SIGNATURE".into()),
            "{bad:?}"
        );
    }
    // The bytes must be the ones named, signed or not.
    let (status, e) = job
        .request_with(
            Method::PUT,
            &format!("job/bundles/{}", "00".repeat(32)),
            &[(BUNDLE_SIGNATURE_HEADER, Some(&a_name_only))],
            rig.bundle.clone(),
        )
        .await;
    assert_eq!(
        (
            status,
            serde_json::from_slice::<Value>(&e).unwrap()["error"]
                .as_str()
                .map(str::to_owned)
        ),
        (400, Some("HASH_MISMATCH".into()))
    );
    assert_eq!(job.put_bundle_signed(&rig.bundle, Some(&good)).await.0, 200);
    // A held component answers a repeat, and an assignment now finds it.
    assert_eq!(job.put_bundle_signed(&rig.bundle, Some(&good)).await.0, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn every_node_names_the_same_cluster_identity_and_a_report_stands_for_its_assignment_only() {
    let rig = Rig::start().await;
    let identity = rig.leader().cluster_identity();
    assert_eq!(
        identity,
        cluster_identity_key(rig.leader().cluster_key())
            .verifying_key()
            .to_bytes()
    );
    for i in 0..3 {
        let ops = rig.c.client(i);
        ops.use_api_key("ops-secret").await;
        let answer = ops.get("ops/cluster").await.unwrap();
        assert_eq!(answer["identity"], hex::encode(identity), "node {i}");
        assert_eq!(answer["cluster"], rig.c.configs[0].cluster);
    }
    // Operator only.
    assert!(rig.c.client(0).get("ops/cluster").await.is_err());

    let (_a, job) = rig.job(JOB_A).await;
    let burn = RunBudget {
        fuel: 1_000_000,
        memory_mib: 64,
        wall_secs: 30,
    };
    let a = rig.assignment(&job, "stands-for", 1, "burn", burn, &[]);
    let answer = rig.run(&job, &a, &[]).await;
    let report = report(&answer, &job, &a);
    // The same report does not verify for another assignment, even of the same run.
    let mut other = a.clone();
    other.args = b"[1]".to_vec();
    let digest = assignment_digest(&other).unwrap();
    let signature = answer["signature"].as_str().unwrap();
    assert!(!verify_report(&job.job_key, &digest, &report, signature));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_apps_with_the_same_run_id_run_side_by_side_on_one_job_node() {
    let rig = Rig::start().await;
    let (_a, job) = rig.job(JOB_A).await;
    let second = rig
        .alice
        .create_app("files-two", &rig.alice_bundle, json!({}))
        .await
        .unwrap();
    assert_ne!(second, rig.app);
    let long_spin = RunBudget {
        fuel: 1_000_000_000_000,
        memory_mib: 64,
        wall_secs: 60,
    };
    let one = rig.assignment(&job, "1.1.0", 1, "spin", long_spin, &[]);
    let two = rig.assignment_for(&second, &job, "1.1.0", 1, "spin", long_spin, &[]);
    assert_eq!(rig.push(&job).await.0, 200);
    assert_eq!(job.post(&one).await.0, 200);
    let (status, answer) = job.post(&two).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(job.load().await["running"], 2);
    // Each run answers to its own app's token only.
    let (token_one, token_two) = (rig.token(&one), rig.token(&two));
    assert_ne!(token_one, token_two);
    assert_eq!(job.status(&one, &token_one).await.0, 200);
    assert_eq!(job.status(&two, &token_two).await.0, 200);
    assert_eq!(job.status(&one, &token_two).await.0, 404);
    assert_eq!(job.status(&two, &token_one).await.0, 404);
    // Cancelling one leaves the other running.
    let (status, ended) = job.cancel(&one, &token_one).await;
    assert_eq!(status, 200);
    failed(&ended, &job, &one, FailReason::Cancelled);
    assert_eq!(job.status(&one, &token_one).await.0, 404);
    assert_eq!(job.status(&two, &token_two).await.1["state"], "running");
    let (_status, ended) = job.cancel(&two, &token_two).await;
    failed(&ended, &job, &two, FailReason::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_input_must_be_the_file_it_is_named_by_and_an_oversized_assignment_is_refused() {
    let rig = Rig::start().await;
    let (_a, job) = rig.job(JOB_A).await;
    let input = rig.input(6).await;
    let mut swapped = rig.input(7).await;
    let a = rig.assignment(&job, "named", 1, "digest", DIGEST, &[&input]);
    assert_eq!(rig.push(&job).await.0, 200);
    assert_eq!(job.post(&a).await.0, 200);
    let token = rig.token(&a);
    // Bytes of the right length that are not the file the assignment names.
    swapped.ciphertext.truncate(input.ciphertext.len());
    swapped.ciphertext.resize(input.ciphertext.len(), 0);
    let (status, e) = job.put_input(&a, 0, &token, &swapped.ciphertext).await;
    assert_eq!((status, e["error"].as_str()), (400, Some("BAD_INPUT_ID")));
    assert_eq!(job.status(&a, &token).await.1["state"], "awaiting");
    assert_eq!(job.put_input(&a, 0, &token, &input.ciphertext).await.0, 200);
    assert_eq!(job.finish(&a, &token).await["state"], "done");

    let big = vec![b' '; calimero_node::job_node::MAX_ASSIGNMENT_BYTES + 1];
    let (status, _) = job.json(Method::POST, "job/runs", None, big).await;
    assert_eq!(status, 413);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_input_that_changes_on_the_job_nodes_disk_loses_the_run_and_never_blames_the_leader() {
    let rig = Rig::start().await;
    let kept: Mutex<Option<JobConfig>> = Mutex::new(None);
    let job = rig
        .c
        .start_job_node(JOB_A, |cfg| *kept.lock().unwrap() = Some(cfg.clone()))
        .await;
    rig.c.register_job_node(JOB_A, &job.url).await;
    let client = rig.attest(JOB_A, &job.url).await;
    let scratch = kept
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .data_dir
        .join("scratch");
    let (one, two) = (rig.input(11).await, rig.input(12).await);
    let a = rig.assignment(&client, "disk", 1, "digest", DIGEST, &[&one, &two]);
    assert_eq!(rig.push(&client).await.0, 200);
    assert_eq!(client.post(&a).await.0, 200);
    let token = rig.token(&a);
    assert_eq!(
        client.put_input(&a, 0, &token, &one.ciphertext).await.0,
        200
    );
    // The first input is on the disk of the job node, which is not trusted: a byte changes.
    let stored = std::fs::read_dir(&scratch)
        .unwrap()
        .map(|e| e.unwrap().path().join("in-0"))
        .find(|p| p.exists())
        .unwrap();
    let mut bytes = std::fs::read(&stored).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    std::fs::write(&stored, bytes).unwrap();
    assert_eq!(
        client.put_input(&a, 1, &token, &two.ciphertext).await.0,
        200
    );
    let answer = client.finish(&a, &token).await;
    assert_eq!(answer, json!({ "state": "lost" }));
    // The run's files are deleted as it ends, and the failure happens in its own thread while
    // the file is altered: putting it back mid-run cannot be timed from here, so the reader's
    // unit test covers restoring.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_input_for_a_run_whose_directory_is_gone_is_a_conflict_not_a_server_error() {
    let rig = Rig::start().await;
    let kept: Mutex<Option<JobConfig>> = Mutex::new(None);
    let job = rig
        .c
        .start_job_node(JOB_A, |cfg| *kept.lock().unwrap() = Some(cfg.clone()))
        .await;
    rig.c.register_job_node(JOB_A, &job.url).await;
    let client = rig.attest(JOB_A, &job.url).await;
    let scratch = kept
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .data_dir
        .join("scratch");
    let input = rig.input(16).await;
    let a = rig.assignment(&client, "gone", 1, "digest", DIGEST, &[&input]);
    assert_eq!(rig.push(&client).await.0, 200);
    assert_eq!(client.post(&a).await.0, 200);
    // What a cancel does to the run's directory while an input is on its way.
    for entry in std::fs::read_dir(&scratch).unwrap() {
        std::fs::remove_dir_all(entry.unwrap().path()).unwrap();
    }
    let (status, e) = client
        .put_input(&a, 0, &rig.token(&a), &input.ciphertext)
        .await;
    assert_eq!((status, e["error"].as_str()), (409, Some("NOT_AWAITING")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_input_that_is_not_what_its_mark_declares_ends_the_run_at_once_as_integrity() {
    let rig = Rig::start().await;
    let (_a, job) = rig.job(JOB_A).await;
    let (good, other, third) = (
        rig.input(13).await,
        rig.input(14).await,
        rig.input(15).await,
    );
    // The ciphertext is the file named and opens under its key, but the declared plaintext
    // hash is another file's.
    let mut liar = Input {
        ciphertext: good.ciphertext.clone(),
        key: good.key,
        plain: good.plain.clone(),
        id: good.id.clone(),
        ..other
    };
    liar.size = good.size;
    let a = rig.assignment(&job, "mark", 1, "digest", DIGEST, &[&liar, &third]);
    assert_eq!(rig.push(&job).await.0, 200);
    assert_eq!(job.post(&a).await.0, 200);
    let token = rig.token(&a);
    let (status, answer) = job.put_input(&a, 0, &token, &liar.ciphertext).await;
    assert_eq!(status, 200, "{answer}");
    // The run ended on that input alone, without waiting for the rest, and says so, signed.
    assert_eq!(answer["state"], "done", "{answer}");
    failed(&answer, &job, &a, FailReason::Integrity);
    assert_eq!(job.load().await["running"], 0);
    let (status, e) = job.put_input(&a, 1, &token, &third.ciphertext).await;
    assert_eq!((status, e["error"].as_str()), (409, Some("NOT_AWAITING")));
}
