//! Compute runs driven by an app's leader through real job nodes: assignment, polling, restart
//! on a lost job node, cancel, caps, the result method, usage and the queue. The app is the
//! `compute` fixture (`apps/v2-fixtures/compute`).

mod common;
use common::UploadBytes as _;

use std::time::{Duration, Instant};

use calimero_client::types::{AppCreateRequest, ViewsRequestConsistency};
use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::component::blobs::BlobRecord;
use calimero_node::component::{
    CallEntry, ComponentCmd, ComputeStart, FailedReason, FinalEntry, FinalOutcome, Outcome,
    RunCaps, RunRecord, RunState,
};
use calimero_node::files::noise_seed;
use calimero_node::job_wire::{Report, ReportStatus, ReportUsage};
use calimero_node::node::{ComputeLimits, PeerBlobs, JOB_CONTROL_TIMEOUT, JOB_POLL, SEGMENT_GRACE};
use calimero_node::types::Caller;
use calimero_node::Node;
use common::{mpk_with, signed_up, typed, wire, Cluster, JobNode, PUBLISHER};
use futures_util::FutureExt;
use rand_chacha::rand_core::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde_json::{json, Value};

const JOB_A: u64 = 4;
const JOB_B: u64 = 5;
const JOB_C: u64 = 6;
const INPUT_LEN: usize = 3 * 1024 * 1024 + 512 * 1024; // 3.5 MiB
const READ_PIECE: usize = 64 * 1024; // what the fixture reads and keys at a time
const WAIT: Duration = Duration::from_secs(180);
const PAST_IDLE_MS: i64 = 3_700_000; // past the idle time of an unreferenced blob
const SESSION_LEN: usize = 17 * 1024 * 1024 + 1000; // more than one upload part
const UPLOAD_TRIES: usize = 5; // FILES_UNAVAILABLE is retryable on an overloaded runner

struct Rig {
    c: Cluster,
    alice: Client,
    pk: SoftAuthenticator,
    app: String,
    jobs: Vec<JobNode>,
    /// The hashes of the installed release's app component and compute component.
    hashes: (String, String),
    /// The segments of each file a test saw, which outlive the file's record.
    seen: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
}

impl Rig {
    async fn start(job_ids: &[u64]) -> Self {
        Self::start_with(job_ids, json!({})).await
    }

    async fn start_with(job_ids: &[u64], options: Value) -> Self {
        let c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = false).await;
        let (alice, pk) = signed_up(&c, 0, "alice").await;
        let both = common::components("compute");
        let compute = both.compute.expect("a compute component");
        let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
        let _installed = alice
            .packages()
            .upload(mpk_with(
                "com.test.compute",
                "1.0.0",
                &both.app,
                Some(&compute),
                &key,
            ))
            .await
            .unwrap();
        let app = alice
            .apps()
            .create(AppCreateRequest {
                name: Some("compute".to_owned()),
                package: Some("com.test.compute".to_owned()),
                version: Some("1.0.0".to_owned()),
                ..typed(options)
            })
            .await
            .map(|a| a.app_id)
            .unwrap();
        let mut jobs = Vec::new();
        for id in job_ids {
            let job = c.start_job_node(*id, |_| {}).await;
            c.register_job_node(*id, &job.url).await;
            jobs.push(job);
        }
        Self {
            c,
            alice,
            pk,
            app,
            jobs,
            hashes: (sha256_hex(&both.app), sha256_hex(&compute)),
            seen: std::sync::Mutex::default(),
        }
    }

    fn leader(&self) -> &Node {
        &self.c.nodes[self.c.app_leader(&self.app).unwrap()]
            .as_ref()
            .unwrap()
            .node
    }

    async fn upload(&self, plain: &[u8]) -> String {
        let mut tries = 1;
        loop {
            match self.alice.upload_bytes(&self.app, plain.to_vec()).await {
                Ok(up) => return up["id"].as_str().unwrap().to_owned(),
                Err(e) if e.code() == Some("FILES_UNAVAILABLE") && tries < UPLOAD_TRIES => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => panic!("upload failed: {e}"),
            }
        }
    }

    async fn begin(&self, method: &str, args: Value) -> String {
        let r = self
            .alice
            .apps()
            .call(&self.app, method, Some(args), None, None)
            .await
            .unwrap();
        r.result.as_str().unwrap().to_owned()
    }

    fn record(&self, run: &str) -> RunRecord {
        self.leader()
            .hosted(&self.app)
            .unwrap()
            .read_local(|m, _| m.run_record(run))
            .unwrap()
    }

    /// Waits until the run's record is no longer active, and its result method recorded.
    async fn outcome(&self, run: &str) -> Value {
        let started = Instant::now();
        loop {
            let v = self
                .alice
                .apps()
                .view(
                    &self.app,
                    "outcome",
                    Some(json!({ "run": run })),
                    Some(ViewsRequestConsistency::Strong),
                    None,
                )
                .await
                .unwrap();
            if let Some(text) = v.result.as_str() {
                return serde_json::from_str(text).unwrap();
            }
            assert!(started.elapsed() < WAIT, "run {run} did not end");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn until(&self, what: &str, mut ready: impl FnMut() -> bool) {
        let started = Instant::now();
        while !ready() {
            assert!(started.elapsed() < WAIT, "{what}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The leader's metrics once every series in `names` shows: it counts a run's usage after
    /// the final commits and the job node drops the run, so the outcome is visible first.
    async fn metrics_with(&self, names: &[&str]) -> String {
        let started = Instant::now();
        loop {
            let metrics = self.metrics().await;
            if names.iter().all(|n| metrics.contains(n)) {
                return metrics;
            }
            let seen: Vec<&str> = metrics.lines().filter(|l| l.contains("compute")).collect();
            assert!(started.elapsed() < WAIT, "{names:?} not in {seen:#?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn metrics(&self) -> String {
        let leader = self.c.app_leader(&self.app).unwrap();
        let ops = self.c.client(leader);
        ops.use_api_key("ops-secret").await;
        ops.ops().metrics().await.unwrap()
    }

    fn slot_granted(&self, run: &str) -> Option<bool> {
        let node = &self.c.nodes[0].as_ref().unwrap().node;
        node.sys(|s| s.run_slot(&self.app, run).map(|slot| slot.granted))
    }

    /// How many runs the job node holds, asked as the leader asks.
    async fn job_running(&self, id: u64) -> usize {
        let leader = self.leader();
        let app = leader.sys(|s| s.apps[&self.app].clone());
        let peer = leader.attest_job(id, &app).await.unwrap();
        leader.job_load(&peer).await.unwrap().0
    }

    /// The leader's record of the blob `id`, if the app holds it.
    fn blob(&self, id: &str) -> Option<BlobRecord> {
        let raw = <[u8; 32]>::try_from(hex::decode(id).unwrap()).unwrap();
        self.leader()
            .hosted(&self.app)
            .unwrap()
            .read_local(|m, _| m.state.component.blobs.records.get(&raw).cloned())
    }

    /// The segments of file `id` as the leader records them, or as a test saw them before the
    /// record was forgotten; none for a plain blob.
    fn segments(&self, id: &str) -> Vec<String> {
        let found = self.blob(id).and_then(|r| r.file).map(|f| f.segments);
        let mut seen = self.seen.lock().unwrap();
        match found {
            Some(segments) => {
                let _prev = seen.insert(id.to_owned(), segments.clone());
                segments
            }
            None => seen.get(id).cloned().unwrap_or_default(),
        }
    }

    /// Whether node `i` stores every one of `segments`.
    fn holds_segments(&self, i: usize, segments: &[String]) -> bool {
        let node = &self.c.nodes[i].as_ref().unwrap().node;
        segments
            .iter()
            .all(|h| node.engine.get_segment(h).unwrap().is_some())
    }

    /// Whether any node stores the plain blob `id` or any segment of the file `id`.
    fn holds_bytes(&self, id: &str) -> bool {
        let segments = self.segments(id);
        (0..3).any(|i| {
            let node = &self.c.nodes[i].as_ref().unwrap().node;
            node.engine.get_blob(id).unwrap().is_some()
                || segments
                    .iter()
                    .any(|h| node.engine.get_segment(h).unwrap().is_some())
        })
    }

    /// Advances every clock past the idle time and logs an entry, which forgets what is due.
    async fn pass_the_idle_time(&self) {
        self.c.advance_all(PAST_IDLE_MS);
        let _ok = self
            .alice
            .apps()
            .call(&self.app, "ping", Some(json!({})), None, None)
            .await
            .unwrap();
    }

    /// Runs the segment sweep on every node once now and once after the grace.
    fn sweep_past_grace(&self) {
        let now = Instant::now();
        for n in self.c.nodes.iter().flatten() {
            n.node.sweep_segments_at(now);
            n.node
                .sweep_segments_at(now + SEGMENT_GRACE + Duration::from_secs(1));
        }
    }

    /// The file bytes the payer of alice's apps is charged for: her personal
    /// organization, where an app made without naming one goes.
    async fn charged(&self) -> u64 {
        let me = self.alice.auth().me().await.map(wire).unwrap();
        let payer = me["orgs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["personal"] == json!(true))
            .map(|o| o["orgId"].as_str().unwrap().to_owned())
            .expect("a personal organization from sign-up");
        let usage = self.alice.auth().usage().await.map(wire).unwrap();
        usage["fileBytes"][payer].as_u64().unwrap_or(0)
    }

    fn limits(&self, limits: ComputeLimits) {
        for n in self.c.nodes.iter().flatten() {
            n.node.set_compute_limits(limits);
        }
    }

    fn outage(&self, on: bool) {
        for n in self.c.nodes.iter().flatten() {
            n.node.set_system_outage(on);
        }
    }

    /// The run's result from `outcome`, within `secs`.
    async fn outcome_within(&self, run: &str, secs: u64) -> Value {
        tokio::time::timeout(Duration::from_secs(secs), self.outcome(run))
            .await
            .unwrap_or_else(|_| panic!("run {run} did not end within {secs} s"))
    }

    /// A final for attempt 1 of `run` that the job node `job` signed.
    fn signed_final(&self, run: &str, job: usize, status: ReportStatus) -> ComponentCmd {
        let report = Report {
            app: self.app.clone(),
            run: run.to_owned(),
            attempt: 1,
            status,
            reason: None,
            message: String::new(),
            result: Vec::new(),
            outputs: Vec::new(),
            log: None,
            usage: ReportUsage::default(),
        };
        let digest: [u8; 32] = hex::decode(&self.record(run).attempts[0].assignment_digest)
            .unwrap()
            .try_into()
            .unwrap();
        let signature = self.jobs[job]
            .running
            .sign_report(&digest, &report)
            .unwrap();
        let g = self.leader().hosted(&self.app).unwrap();
        ComponentCmd::ComputeFinal(Box::new(FinalEntry {
            run: run.to_owned(),
            attempt: 1,
            outcome: FinalOutcome::Done,
            report: Some(report),
            signature: Some(signature),
            outputs: Vec::new(),
            log: None,
            usage: ReportUsage::default(),
            result: Outcome::default(),
            base_index: g.read_local(|m, _| m.state.last_index),
            ts_ms: 1,
            id_term: 0,
            id_seq: 0,
        }))
    }

    /// The system queue holds no slot for the app: every run was released.
    fn slots(&self) -> usize {
        let node = &self.c.nodes[0].as_ref().unwrap().node;
        node.sys(|s| s.compute.get(&self.app).map_or(0, |r| r.len()))
    }
}

fn input() -> Vec<u8> {
    (0..INPUT_LEN).map(|i| (i % 251) as u8).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(calimero_identity::sha256(bytes))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_ends_with_its_output_usage_and_one_result() {
    let rig = Rig::start(&[JOB_A, JOB_B]).await;
    let plain = input();
    let file = rig.upload(&plain).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;

    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_eq!(outcome["attempt"], 1);
    let seed = noise_seed(&rig.leader().app_file_key(&rig.app), &run);
    let expected = sha256_hex(&expected_output(&plain, seed));
    assert_eq!(outcome["sha256"], json!(expected));
    assert_ne!(outcome["fuel"], "0");

    let record = rig.record(&run);
    assert_eq!(record.state, RunState::Done);
    assert_eq!(record.final_.as_ref().unwrap().outputs.len(), 1);
    let done = rig
        .alice
        .apps()
        .view(
            &rig.app,
            "done",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(done.result, json!("1"), "one result");

    let metrics = rig
        .metrics_with(&[
            "calimero_app_compute_cpu_seconds_total{",
            "calimero_app_compute_fuel_total{",
            "calimero_app_calls_total{",
        ])
        .await;
    assert!(metrics.contains("kind=\"compute\""), "{metrics}");
    rig.until("the slot is released", || rig.slots() == 0).await;

    // The API shows the record without its arguments.
    let shown = rig
        .alice
        .apps()
        .run(&rig.app, &run)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(shown["state"], "done", "{shown}");
    assert!(shown.get("args").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_run_restarts_on_the_other_job_node_when_its_node_is_lost() {
    let mut rig = Rig::start(&[JOB_A, JOB_B]).await;
    let plain = input();
    let file = rig.upload(&plain).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    rig.until("an attempt is assigned", || {
        !rig.record(&run).attempts.is_empty()
    })
    .await;
    let first = rig.record(&run).attempts[0].node;
    let at = rig.jobs.iter().position(|j| j.id == first).unwrap();
    rig.jobs.remove(at).running.stop().await;

    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    let record = rig.record(&run);
    assert_eq!(record.attempts.len(), 2);
    assert_ne!(record.attempts[1].node, first);
    let seed = noise_seed(&rig.leader().app_file_key(&rig.app), &run);
    assert_eq!(
        outcome["sha256"],
        json!(sha256_hex(&expected_output(&plain, seed))),
        "the same bytes as an uninterrupted run"
    );
    let done = rig
        .alice
        .apps()
        .view(
            &rig.app,
            "done",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(done.result, json!("1"), "exactly one result");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_ends_the_run_and_stops_the_job() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.begin("startRun", json!({ "method": "spin" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "cancelled", "{outcome}");
    assert_eq!(rig.record(&run).state, RunState::Cancelled);
    assert_ne!(
        outcome["fuel"], "0",
        "the job node's usage is billed: {outcome}"
    );
    let _metrics = rig
        .metrics_with(&["calimero_app_compute_fuel_total{"])
        .await;
    assert_eq!(
        rig.job_running(JOB_A).await,
        0,
        "the job node dropped the run"
    );
    rig.until("the slot is released", || rig.slots() == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_that_breaks_a_cap_fails_with_its_reason_and_registers_nothing() {
    let rig = Rig::start(&[JOB_A]).await;
    for (method, reason) in [("hog", "memory"), ("burn", "fuel"), ("big", "trap")] {
        let run = rig.begin("startRun", json!({ "method": method })).await;
        let outcome = rig.outcome(&run).await;
        assert_eq!(outcome["status"], "failed", "{method}: {outcome}");
        assert_eq!(outcome["reason"], reason, "{method}");
        assert_eq!(outcome["outputs"], 0);
        assert_eq!(rig.record(&run).state, RunState::Failed);
        assert_eq!(
            rig.record(&run).attempts.len(),
            1,
            "{method}: no attempt is lost"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_final_not_signed_by_the_assigned_job_node_changes_nothing() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    rig.until("an attempt is assigned", || {
        !rig.record(&run).attempts.is_empty()
    })
    .await;
    let g = rig.leader().hosted(&rig.app).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let report = Report {
        app: rig.app.clone(),
        run: run.clone(),
        attempt: 1,
        status: ReportStatus::Done,
        reason: None,
        message: String::new(),
        result: Vec::new(),
        outputs: Vec::new(),
        log: None,
        usage: ReportUsage::default(),
    };
    let digest: [u8; 32] = hex::decode(&rig.record(&run).attempts[0].assignment_digest)
        .unwrap()
        .try_into()
        .unwrap();
    let signature = calimero_node::job_wire::sign_report(&key, &digest, &report).unwrap();
    let forged = ComponentCmd::ComputeFinal(Box::new(FinalEntry {
        run: run.clone(),
        attempt: 1,
        outcome: FinalOutcome::Done,
        report: Some(report),
        signature: Some(signature),
        outputs: Vec::new(),
        log: None,
        usage: ReportUsage::default(),
        result: Outcome::default(),
        base_index: g.read_local(|m, _| m.state.last_index),
        ts_ms: 1,
        id_term: 0,
        id_seq: 0,
    }));
    let (_index, answer) = g.write(serde_json::to_vec(&forged).unwrap()).await.unwrap();
    let answer: Value = serde_json::from_slice(&answer).unwrap();
    assert_eq!(
        answer["error"],
        "report not signed by the assigned job node"
    );
    assert!(rig.record(&run).state.is_active());

    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_new_leader_keeps_polling_the_same_attempt() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let run = rig.begin("startRun", json!({ "method": "burn" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    let old = rig.c.app_leader(&rig.app).unwrap();
    let client = usize::from(old == 0);
    rig.c.stop(old).await;
    let g = rig.c.nodes[client]
        .as_ref()
        .unwrap()
        .node
        .hosted(&rig.app)
        .unwrap();
    rig.until("a new leader ends the run", || {
        g.read_local(|m, _| m.run_record(&run))
            .is_some_and(|r| !r.state.is_active())
    })
    .await;
    let record = g.read_local(|m, _| m.run_record(&run)).unwrap();
    assert_eq!(record.attempts.len(), 1, "no new attempt");
    let mut pk = rig.pk;
    let alice = rig.c.client(client);
    let _s = alice.sign_in(&mut pk).await.unwrap();
    let done = alice
        .apps()
        .view(
            &rig.app,
            "done",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(done.result, json!("1"), "the result method ran once");
    assert_eq!(
        record.final_.unwrap().outcome,
        FinalOutcome::Failed {
            reason: FailedReason::Fuel
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_second_run_of_an_org_waits_for_the_first_final() {
    let rig = Rig::start(&[JOB_A]).await;
    let ops = rig.c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ok = common::ops_signed(&ops, "ops/quotas", json!({ "maxConcurrentRunsPerOrg": 1 }))
        .await
        .unwrap();
    let first = rig.begin("startRun", json!({ "method": "spin" })).await;
    let second = rig.begin("startRun", json!({ "method": "burn" })).await;
    rig.until("the first runs", || !rig.record(&first).attempts.is_empty())
        .await;
    assert!(
        rig.record(&second).attempts.is_empty(),
        "the second is queued while the first runs"
    );
    rig.until("the second is queued", || {
        rig.slot_granted(&second).is_some()
    })
    .await;
    assert_eq!(
        rig.slot_granted(&second),
        Some(false),
        "not granted a place"
    );
    let _ = rig.outcome(&first).await;
    let outcome = rig.outcome(&second).await;
    assert_eq!(outcome["reason"], "fuel", "{outcome}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_stranger_sees_a_runs_record_without_file_hashes() {
    let rig = Rig::start_with(&[JOB_A], json!({ "access": { "kind": "public" } })).await;
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let _ = rig.outcome(&run).await;
    let owner = rig
        .alice
        .apps()
        .run(&rig.app, &run)
        .await
        .map(wire)
        .unwrap();
    assert!(owner["inputs"][0]["sha256"].is_string(), "{owner}");
    assert!(
        owner["final"]["outputs"][0]["sha256"].is_string(),
        "{owner}"
    );

    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;
    let shown = bob.apps().run(&rig.app, &run).await.map(wire).unwrap();
    assert_eq!(shown["state"], "done", "{shown}");
    assert!(shown["inputs"][0].get("sha256").is_none(), "{shown}");
    assert!(shown["inputs"][0].get("size").is_none(), "{shown}");
    assert!(
        shown["final"]["outputs"][0].get("sha256").is_none(),
        "{shown}"
    );
    assert!(
        shown["final"]["outputs"][0].get("size").is_none(),
        "{shown}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_run_list_pages_newest_first_with_caps_and_the_single_run_view() {
    let rig = Rig::start_with(&[JOB_A], json!({ "access": { "kind": "public" } })).await;
    let file = rig.upload(&input()).await;
    let first = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let _ = rig.outcome(&first).await;
    let second = rig.begin("startRun", json!({ "method": "spin" })).await;

    let all = rig
        .alice
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    let runs = all["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "{all}");
    assert_eq!(all["next"], Value::Null);
    assert_eq!(runs[0]["run"], json!(second), "newest first");
    assert_eq!(runs[1]["state"], "done");
    assert!(runs[1]["caps"]["wallSecs"].is_u64(), "{all}");
    assert!(runs[1]["caps"]["memoryMib"].is_u64() && runs[1]["caps"]["fuel"].is_u64());
    assert!(runs[1]["inputs"][0]["sha256"].is_string(), "{all}");
    let single = rig
        .alice
        .apps()
        .run(&rig.app, &first)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(runs[1], single, "the same object as the single run");

    let page = rig
        .alice
        .apps()
        .runs(&rig.app, Some("1"), None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(page["runs"].as_array().unwrap().len(), 1);
    let next = page["next"].as_str().unwrap().to_owned();
    let rest = rig
        .alice
        .apps()
        .runs(&rig.app, Some("1"), Some(&next))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(rest["runs"][0]["run"], json!(first), "{rest}");
    assert_eq!(rest["next"], Value::Null);

    for limit in ["0", "101", "x", "-1"] {
        let e = rig
            .alice
            .apps()
            .runs(&rig.app, Some(limit), None)
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("BAD_LIMIT"), "limit={limit}");
    }
    let e = rig
        .alice
        .apps()
        .runs(&rig.app, None, Some("zz"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_CURSOR"));
    let e = rig
        .alice
        .apps()
        .runs(&rig.app, None, Some(""))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_CURSOR"), "an empty cursor");
    let long = format!("1:{}", "a".repeat(255));
    let e = rig
        .alice
        .apps()
        .runs(&rig.app, None, Some(&long))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_CURSOR"), "a cursor over the byte bound");
    let colon = rig
        .alice
        .apps()
        .runs(&rig.app, None, Some("99999999999999:x:y"))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(colon["runs"].as_array().unwrap().len(), 2, "{colon}");

    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;
    let shown = bob
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(shown["runs"][1]["state"], "done", "{shown}");
    assert!(
        shown["runs"][1]["inputs"][0].get("sha256").is_none(),
        "{shown}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_stranger_lists_runs_without_file_sizes_hashes_or_keys() {
    let rig = Rig::start_with(&[JOB_A], json!({ "access": { "kind": "public" } })).await;
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let _ = rig.outcome(&run).await;
    let owner = rig
        .alice
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    assert!(
        owner["runs"][0]["inputs"][0]["sha256"].is_string(),
        "{owner}"
    );
    assert!(
        owner["runs"][0]["final"]["outputs"][0]["size"].is_u64(),
        "{owner}"
    );

    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;
    let shown = bob
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    let listed = &shown["runs"][0];
    assert_eq!(listed["state"], "done", "{shown}");
    for file in [&listed["inputs"][0], &listed["final"]["outputs"][0]] {
        assert!(file["id"].is_string(), "{shown}");
        assert_eq!(file.as_object().unwrap().len(), 1, "only the id: {shown}");
    }
    let text = shown.to_string();
    for word in ["wrapped", "key", "args"] {
        assert!(!text.contains(word), "{word} in {text}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn runs_that_meet_on_a_job_node_all_complete_without_a_lost_attempt() {
    let rig = Rig::start(&[JOB_A, JOB_B]).await;
    let mut runs = Vec::new();
    for method in ["burn", "burn", "hog", "hog"] {
        runs.push(rig.begin("startRun", json!({ "method": method })).await);
    }
    for run in &runs {
        let _ = rig.outcome(run).await;
        assert_eq!(rig.record(run).attempts.len(), 1, "{run}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_slot_whose_run_is_not_active_is_released() {
    let rig = Rig::start(&[JOB_A]).await;
    let leader = rig.leader();
    let payer = leader.sys(|s| calimero_node::usage::payer(&s.apps[&rig.app]));
    let _ok = leader
        .sys_write(&calimero_node::system::SystemCmd::AcquireRun {
            app: rig.app.clone(),
            run: "ghost".into(),
            payer,
            fuel: 1,
            memory_mib: 0,
            pool: None,
        })
        .await
        .unwrap();
    rig.until("the stray slot is released", || rig.slots() == 0)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_staged_output_is_committed_when_its_job_node_vanishes_after_reporting() {
    let mut rig = Rig::start(&[JOB_A, JOB_B]).await;
    rig.limits(ComputeLimits {
        lost_after: Duration::from_secs(4),
        ..ComputeLimits::default()
    });
    let plain = input();
    let file = rig.upload(&plain).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    rig.leader().conflict_next_call();
    let leader = rig.leader();
    rig.until("the final conflicted after its outputs were staged", || {
        leader.conflicts.load(std::sync::atomic::Ordering::Relaxed) > 0
    })
    .await;
    let first = rig.record(&run).attempts[0].node;
    let at = rig.jobs.iter().position(|j| j.id == first).unwrap();
    rig.jobs.remove(at).running.stop().await;

    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_eq!(
        rig.record(&run).attempts.len(),
        1,
        "the report and its staged outputs are all the leader needs"
    );
    let segments = rig.record(&run).final_.unwrap().outputs[0].segments.clone();
    rig.until("the output is on a majority", || {
        (0..3).filter(|i| rig.holds_segments(*i, &segments)).count() >= 2
    })
    .await;
    let counted = rig.c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.app_file_bytes.get(&rig.app).copied().unwrap_or(0));
    assert_eq!(counted, 2 * INPUT_LEN as u64, "the output is charged once");
}

fn series(metrics: &str, name: &str) -> f64 {
    metrics
        .lines()
        .find(|l| l.starts_with(&format!("{name}{{")))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_final_that_conflicted_is_billed_once() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    rig.leader().conflict_next_call();
    let _ = rig.outcome(&run).await;
    let fuel = rig.record(&run).final_.unwrap().usage.fuel as f64;
    let _counted = rig
        .metrics_with(&["calimero_app_compute_fuel_total", "kind=\"compute\"} 1\n"])
        .await;
    rig.until("the run ended on every node", || {
        rig.c.nodes.iter().flatten().all(|n| {
            n.node.hosted(&rig.app).is_some_and(|g| {
                g.read_local(|m, _| m.run_record(&run))
                    .is_some_and(|r| !r.state.is_active())
            })
        })
    })
    .await;
    // No signal says the leader's sweeps are done, so a bounded window shows no later count.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let metrics = rig.metrics().await;
    assert_eq!(series(&metrics, "calimero_app_compute_fuel_total"), fuel);
    assert!(
        metrics.contains("kind=\"compute\"} 1\n"),
        "one call: {metrics}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_bytes_of_a_registered_output_survive_every_cleanup() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let _ = rig.outcome(&run).await;
    let segments = rig.record(&run).final_.unwrap().outputs[0].segments.clone();
    let holds = |i: usize| rig.holds_segments(i, &segments);
    rig.until("a majority holds the output", || {
        (0..3).filter(|i| holds(*i)).count() >= 2
    })
    .await;

    // A losing leader's cleanup of the same id: the barrier and the strong read find the blob record.
    rig.leader()
        .clean_up_outputs(&rig.app, segments.clone())
        .await;
    let leader = rig.c.app_leader(&rig.app).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        (0..3).filter(|i| holds(*i)).count() >= 2,
        "the registered output kept its bytes"
    );
    assert!(holds(leader));
}

/// A little over one segment of plaintext, so the file is two segments.
const TWO_SEGMENT_LEN: usize = 17 * 1024 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_whose_input_lost_a_segment_on_the_leader_refetches_it() {
    let rig = Rig::start(&[JOB_A]).await;
    let plain: Vec<u8> = (0..TWO_SEGMENT_LEN).map(|i| (i % 251) as u8).collect();
    let file = rig.upload(&plain).await;
    let segments = rig.segments(&file);
    assert_eq!(segments.len(), 2, "{segments:?}");
    let leader = rig.c.app_leader(&rig.app).unwrap();
    let node = &rig.c.nodes[leader].as_ref().unwrap().node;
    node.engine.delete_segment(&segments[1]).unwrap();
    assert!(node.engine.get_segment(&segments[1]).unwrap().is_none());

    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert!(
        node.engine.get_segment(&segments[1]).unwrap().is_some(),
        "the leader fetched the segment from a replica and kept it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_ends_the_run_even_when_the_job_node_never_stops() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        cancel_deadline: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    let run = rig.begin("startRun", json!({ "method": "spin" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    rig.jobs[0].running.answer_running_always();
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    let outcome = rig.outcome_within(&run, 30).await;
    assert_eq!(outcome["status"], "cancelled", "{outcome}");
    assert_eq!(
        outcome["fuel"], "0",
        "no signed report, no usage: {outcome}"
    );
    let record = rig.record(&run);
    assert_eq!(record.state, RunState::Cancelled);
    rig.until("the slot is released", || rig.slots() == 0).await;

    // The job node's own report for the attempt comes too late: the first final stands.
    let late = rig.signed_final(&run, 0, ReportStatus::Done);
    let g = rig.leader().hosted(&rig.app).unwrap();
    let _answer = g.write(serde_json::to_vec(&late).unwrap()).await.unwrap();
    assert_eq!(rig.record(&run).final_, record.final_);
    let done = rig
        .alice
        .apps()
        .view(
            &rig.app,
            "done",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(done.result, json!("1"), "the result method ran once");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_outage_of_the_system_group_does_not_lose_a_running_attempt() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        lost_after: Duration::from_secs(4),
        ..ComputeLimits::default()
    });
    let run = rig.begin("startRun", json!({ "method": "burn" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    rig.outage(true);
    tokio::time::sleep(Duration::from_secs(10)).await;
    rig.outage(false);
    let outcome = rig.outcome_within(&run, 60).await;
    assert_eq!(outcome["reason"], "fuel", "{outcome}");
    assert_eq!(rig.record(&run).attempts.len(), 1, "no new attempt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_report_waiting_through_an_outage_past_the_deadline_still_ends_the_run() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        deadline_grace: Duration::from_secs(1),
        ..ComputeLimits::default()
    });
    let run = rig.begin("startRun", json!({ "method": "burn" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    rig.outage(true);
    // The job reports at once and the leader is out of reach meanwhile; the deadline is 3 s.
    tokio::time::sleep(Duration::from_secs(8)).await;
    rig.outage(false);
    let outcome = rig.outcome_within(&run, 60).await;
    assert_eq!(
        outcome["reason"], "fuel",
        "the job's own report, not a timeout: {outcome}"
    );
    assert_eq!(rig.record(&run).attempts.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_granted_run_is_not_refused_for_waiting_through_an_outage() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        queued_timeout: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    rig.outage(true);
    let run = rig.begin("startRun", json!({ "method": "burn" })).await;
    tokio::time::sleep(Duration::from_secs(9)).await;
    assert!(
        rig.record(&run).state.is_active(),
        "refused during an outage: {:?}",
        rig.record(&run).final_
    );
    rig.outage(false);
    let outcome = rig.outcome_within(&run, 60).await;
    assert_eq!(outcome["reason"], "fuel", "{outcome}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_held_report_survives_the_leaders_failing_writes() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        lost_after: Duration::from_secs(4),
        ..ComputeLimits::default()
    });
    let file = rig.upload(&input()).await;
    let leader = rig.c.app_leader(&rig.app).unwrap();
    let followers: Vec<usize> = (0..3).filter(|i| *i != leader).collect();
    for i in &followers {
        rig.c.nodes[*i]
            .as_ref()
            .unwrap()
            .node
            .set_peer_blobs(PeerBlobs::Refuse);
    }
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    // The job reports, and the leader cannot store its output for longer than the lost wait.
    tokio::time::sleep(Duration::from_secs(14)).await;
    assert_eq!(rig.record(&run).attempts.len(), 1, "no new attempt");
    for i in &followers {
        rig.c.nodes[*i]
            .as_ref()
            .unwrap()
            .node
            .set_peer_blobs(PeerBlobs::Accept);
    }
    let outcome = rig.outcome_within(&run, 60).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_eq!(outcome["attempt"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_forty_mib_output_is_fetched_a_segment_at_a_time_and_a_cut_fetch_resumes() {
    use calimero_node::files::SEGMENT_BYTES;

    let rig = Rig::start(&[JOB_A]).await;
    let plain: Vec<u8> = (0..40 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let file = rig.upload(&plain).await;
    // The first segment arrives, the second request fails once, and the fetch goes on from it.
    rig.jobs[0].running.cut_outputs_after(1);
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome_within(&run, 150).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    let seed = noise_seed(&rig.leader().app_file_key(&rig.app), &run);
    assert_eq!(
        outcome["sha256"],
        json!(sha256_hex(&expected_output(&plain, seed)))
    );
    let served = rig.jobs[0].running.served_output_segments();
    assert_eq!(served, [(0, 0), (0, 1), (0, 2)], "no segment fetched twice");
    assert!(rig.jobs[0].running.largest_output_answer() <= SEGMENT_BYTES);
    assert_eq!(rig.record(&run).attempts.len(), 1, "no attempt was lost");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_hundred_mib_file_is_uploaded_read_by_a_run_and_written_back() {
    let rig = Rig::start(&[JOB_A]).await;
    let plain: Vec<u8> = (0..100 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let file = rig.upload(&plain).await;
    assert_eq!(rig.segments(&file).len(), 7);
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome_within(&run, 300).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    let seed = noise_seed(&rig.leader().app_file_key(&rig.app), &run);
    assert_eq!(
        outcome["sha256"],
        json!(sha256_hex(&expected_output(&plain, seed)))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_that_reports_but_never_hands_over_its_outputs_does_not_hold_the_run_open() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        lost_after: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    rig.jobs[0].running.fail_outputs();
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome_within(&run, 90).await;
    assert_eq!(outcome["reason"], "lost", "{outcome}");
    assert_eq!(rig.record(&run).attempts.len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_output_that_takes_longer_than_the_shared_clients_timeout_is_still_fetched() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        lost_after: Duration::from_secs(120),
        ..ComputeLimits::default()
    });
    rig.jobs[0].running.delay_outputs(Duration::from_secs(40));
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome_within(&run, 150).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_eq!(rig.record(&run).attempts.len(), 1, "no new attempt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_slow_failing_output_fetch_does_not_lose_an_attempt_whose_job_node_answers() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        lost_after: Duration::from_secs(7),
        ..ComputeLimits::default()
    });
    rig.jobs[0]
        .running
        .fail_outputs_slowly(Duration::from_secs(8), 4);
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome_within(&run, 90).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_eq!(rig.record(&run).attempts.len(), 1);
}

async fn until_ended(rig: &Rig, run: &str, secs: u64) -> RunRecord {
    let started = Instant::now();
    loop {
        let rec = rig.record(run);
        if !rec.state.is_active() {
            return rec;
        }
        assert!(
            started.elapsed() < Duration::from_secs(secs),
            "run {run} still active after {secs} s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn failed(reason: FailedReason) -> FinalOutcome {
    FinalOutcome::Failed { reason }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_push_that_never_succeeds_ends_the_run_by_timeout_at_the_ceiling() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        deadline_grace: Duration::from_secs(1),
        outage_credit: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    let file = rig.upload(&input()).await;
    let leader = rig.c.app_leader(&rig.app).unwrap();
    for i in (0..3).filter(|i| *i != leader) {
        let node = &rig.c.nodes[i].as_ref().unwrap().node;
        node.set_peer_blobs(PeerBlobs::Refuse);
    }
    // digestBrief's wall cap is 10 s: the ceiling is that plus the grace and the credit.
    let run = rig
        .begin("startDigestBrief", json!({ "inputs": [file] }))
        .await;
    let rec = until_ended(&rig, &run, 60).await;
    assert_eq!(rec.final_.unwrap().outcome, failed(FailedReason::Timeout));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_leader_whose_system_reads_fail_forever_still_ends_the_run_at_the_ceiling() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        deadline_grace: Duration::from_secs(1),
        outage_credit: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    let run = rig.begin("startRun", json!({ "method": "spin" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    rig.outage(true);
    let rec = until_ended(&rig, &run, 40).await;
    assert_eq!(rec.final_.unwrap().outcome, failed(FailedReason::Timeout));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_granted_run_no_job_node_takes_is_refused_at_the_ceiling_whatever_the_outage() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        queued_timeout: Duration::from_secs(3),
        outage_credit: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    rig.outage(true);
    let run = rig.begin("startRun", json!({ "method": "burn" })).await;
    let rec = until_ended(&rig, &run, 40).await;
    assert_eq!(rec.final_.unwrap().outcome, failed(FailedReason::Refused));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_restarted_job_node_does_not_lose_an_attempt_whose_report_is_held() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&input()).await;
    for n in rig.c.nodes.iter().flatten() {
        n.node.conflict_finals(true);
    }
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let leader = rig.leader();
    rig.until("the final conflicted after its outputs were staged", || {
        leader.conflicts.load(std::sync::atomic::Ordering::Relaxed) > 0
    })
    .await;
    // The job node restarts at its address with a new key and no runs.
    let old = rig.jobs.remove(0);
    let listen = old
        .url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    old.running.stop().await;
    let new = rig.c.start_job_node(JOB_A, |c| c.listen = listen).await;
    rig.jobs.push(new);
    tokio::time::sleep(Duration::from_secs(8)).await;
    for n in rig.c.nodes.iter().flatten() {
        n.node.conflict_finals(false);
    }
    let outcome = rig.outcome_within(&run, 60).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_eq!(rig.record(&run).attempts.len(), 1, "the held report stands");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_ends_within_its_deadline_when_the_job_node_stops_answering() {
    let rig = Rig::start(&[JOB_A]).await;
    let deadline = Duration::from_secs(3);
    rig.limits(ComputeLimits {
        cancel_deadline: deadline,
        ..ComputeLimits::default()
    });
    let run = rig.begin("startRun", json!({ "method": "spin" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    rig.jobs[0].running.freeze();
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    let started = Instant::now();
    let rec = until_ended(&rig, &run, 90).await;
    let bound = deadline + JOB_CONTROL_TIMEOUT + JOB_POLL + Duration::from_secs(3);
    assert!(
        started.elapsed() < bound,
        "ended after {:?}",
        started.elapsed()
    );
    assert_eq!(rec.final_.unwrap().outcome, FinalOutcome::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_whose_job_node_goes_silent_ends_with_the_report_the_leader_holds() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&input()).await;
    for n in rig.c.nodes.iter().flatten() {
        n.node.conflict_finals(true);
    }
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let leader = rig.leader();
    rig.until("the final conflicted after its report was held", || {
        leader.conflicts.load(std::sync::atomic::Ordering::Relaxed) > 0
    })
    .await;
    rig.jobs.remove(0).running.stop().await;
    // Cancel while finals still conflict, so the held report cannot end the run first.
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    for n in rig.c.nodes.iter().flatten() {
        n.node.conflict_finals(false);
    }
    let outcome = rig.outcome_within(&run, 60).await;
    assert_eq!(
        outcome["status"], "done",
        "the held report stands: {outcome}"
    );
    assert_ne!(outcome["fuel"], "0", "its usage is billed: {outcome}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_is_not_delayed_by_another_run_of_the_app_stuck_in_a_transfer() {
    let rig = Rig::start(&[JOB_A]).await;
    let deadline = Duration::from_secs(3);
    rig.limits(ComputeLimits {
        cancel_deadline: deadline,
        ..ComputeLimits::default()
    });
    let first = rig.begin("startRun", json!({ "method": "spin" })).await;
    rig.until("the first runs", || !rig.record(&first).attempts.is_empty())
        .await;
    rig.jobs[0].running.answer_running_always();
    for n in rig.c.nodes.iter().flatten() {
        n.node.limit_job_transfers(0);
    }
    // This run waits forever for a place to deliver its assignment.
    let _second = rig.begin("startRun", json!({ "method": "burn" })).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let _ok = rig
        .alice
        .apps()
        .call(
            &rig.app,
            "cancel",
            Some(json!({ "run": first })),
            None,
            None,
        )
        .await
        .unwrap();
    let started = Instant::now();
    let rec = until_ended(&rig, &first, 60).await;
    let bound = deadline + JOB_CONTROL_TIMEOUT + JOB_POLL + Duration::from_secs(3);
    assert!(
        started.elapsed() < bound,
        "ended after {:?}",
        started.elapsed()
    );
    assert_eq!(rec.final_.unwrap().outcome, FinalOutcome::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn waiting_for_our_own_control_places_is_not_a_job_nodes_silence() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.begin("startRun", json!({ "method": "spin" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    for n in rig.c.nodes.iter().flatten() {
        n.node.limit_job_control(0);
    }
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    // Our own queue is full, not the job node silent: the cancel must not end at zero usage yet.
    tokio::time::sleep(Duration::from_secs(12)).await;
    assert!(
        rig.record(&run).state.is_active(),
        "ended early: {:?}",
        rig.record(&run).final_
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_held_report_waiting_on_the_leaders_push_is_not_replaced_by_a_zero_cancel() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&input()).await;
    let leader = rig.c.app_leader(&rig.app).unwrap();
    let followers: Vec<usize> = (0..3).filter(|i| *i != leader).collect();
    for i in &followers {
        let node = &rig.c.nodes[*i].as_ref().unwrap().node;
        node.set_peer_blobs(PeerBlobs::Refuse);
    }
    let pushes = rig.leader().file_pushes();
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    rig.until("the leader tried to push the output", || {
        rig.leader().file_pushes() > pushes
    })
    .await;
    // The job node stops answering status and cancel, but still hands out its outputs.
    rig.jobs[0].running.freeze();
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(20)).await;
    assert!(
        rig.record(&run).state.is_active(),
        "ended before the push could work: {:?}",
        rig.record(&run).final_
    );
    for i in &followers {
        let node = &rig.c.nodes[*i].as_ref().unwrap().node;
        node.set_peer_blobs(PeerBlobs::Accept);
    }
    let outcome = rig.outcome_within(&run, 60).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_ne!(outcome["fuel"], "0", "{outcome}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_output_the_result_method_stores_is_kept_past_the_idle_time_and_a_run_reads_it_back() {
    let rig = Rig::start(&[JOB_A]).await;
    let plain = input();
    let file = rig.upload(&plain).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    let output = rig.record(&run).final_.unwrap().outputs[0].id.clone();
    let record = rig.blob(&output).expect("the final registered the output");
    assert!(record.file.is_some(), "a sealed file");
    assert_eq!(record.refs, 1, "the result method stored its reference");

    rig.pass_the_idle_time().await;
    assert_eq!(rig.blob(&output).map(|r| r.refs), Some(1), "still held");
    assert!(
        rig.blob(&file).is_none(),
        "an input nobody stored is forgotten"
    );
    assert!(rig.holds_bytes(&output));
    // Only a compute run opens a sealed file: a second run takes the output as its input.
    let first = expected_output(
        &plain,
        noise_seed(&rig.leader().app_file_key(&rig.app), &run),
    );
    let again = rig
        .begin("startDigest", json!({ "inputs": [output] }))
        .await;
    let outcome = rig.outcome(&again).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    let seed = noise_seed(&rig.leader().app_file_key(&rig.app), &again);
    let expected = sha256_hex(&expected_output(&first, seed));
    assert_eq!(
        outcome["sha256"],
        json!(expected),
        "a run reads the output back"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_result_method_can_start_the_next_run() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.begin("startDigestThen", json!({ "inputs": [] })).await;
    until_ended(&rig, &run, 60).await;
    let started = Instant::now();
    let next = loop {
        let v = rig
            .alice
            .apps()
            .view(
                &rig.app,
                "chained",
                Some(json!({ "run": run })),
                Some(ViewsRequestConsistency::Strong),
                None,
            )
            .await
            .unwrap();
        if let Some(text) = v.result.as_str() {
            break text.to_owned();
        }
        assert!(started.elapsed() < WAIT, "the result method did not run");
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert!(!next.starts_with("error"), "{next}");
    let record = until_ended(&rig, &next, 60).await;
    assert_eq!(record.state, RunState::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_output_the_result_method_does_not_store_is_collected_and_its_quota_released() {
    let rig = Rig::start(&[JOB_A]).await;
    let plain = input();
    let file = rig.upload(&plain).await;
    let run = rig.begin("startUnkept", json!({ "inputs": [file] })).await;
    rig.until("the run ends", || !rig.record(&run).state.is_active())
        .await;
    assert_eq!(rig.record(&run).state, RunState::Done);
    let output = rig.record(&run).final_.unwrap().outputs[0].id.clone();
    assert!(
        rig.blob(&output).is_some_and(|r| r.refs == 0),
        "registered, idle"
    );
    assert_eq!(
        rig.charged().await,
        2 * plain.len() as u64,
        "input and output"
    );

    rig.pass_the_idle_time().await;
    assert!(rig.blob(&output).is_none(), "forgotten with its idle time");
    rig.until("the bytes are collected", || !rig.holds_bytes(&output))
        .await;
    rig.until("the quota is released", || {
        let now = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| now.block_on(rig.charged())) == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_is_only_ever_given_the_compute_component() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&input()).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");

    let (app, compute) = &rig.hashes;
    let record = rig.record(&run);
    assert_eq!(record.compute_bundle.as_ref(), Some(compute));
    assert_eq!(
        record.bundle, *app,
        "the run is still billed to the app's bundle"
    );
    assert_eq!(rig.jobs[0].running.held_bundles(), vec![compute.clone()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_whose_compute_component_is_missing_is_refused_promptly_without_a_job_node() {
    // The group runs no compute component, or one that is not installed any more.
    for (name, compute) in [("none", None), ("gone", Some("00".repeat(32)))] {
        let rig = Rig::start(&[JOB_A]).await;
        let g = rig.leader().hosted(&rig.app).unwrap();
        let (bundle, base) =
            g.read_local(|m, _| (m.state.bundle.clone().unwrap(), m.state.last_index));
        let upgrade = ComponentCmd::Upgrade {
            bundle,
            compute_bundle: compute,
            base_index: base,
        };
        let _answer = g
            .write(serde_json::to_vec(&upgrade).unwrap())
            .await
            .unwrap();
        let mut outcome = Outcome::default();
        outcome.compute_starts.push(ComputeStart {
            run: "old".into(),
            method: "digest".into(),
            args: Default::default(),
            inputs: Vec::new(),
            caps: RunCaps {
                fuel: 1_000_000,
                memory_mib: 64,
                wall_secs: 30,
            },
            result_method: "onDone".into(),
            owner: "o".into(),
        });
        let entry = ComponentCmd::Call(Box::new(CallEntry {
            method: "ping".into(),
            args: json!({}),
            caller: Caller::System,
            idempotency_key: None,
            consent: None,
            ts_ms: 1,
            base_index: g.read_local(|m, _| m.state.last_index),
            id_term: 0,
            id_seq: 0,
            outcome,
            xcall: None,
        }));
        let _answer = g.write(serde_json::to_vec(&entry).unwrap()).await.unwrap();
        let rec = until_ended(&rig, "old", 30).await;
        assert_eq!(
            rec.final_.unwrap().outcome,
            failed(FailedReason::Refused),
            "{name}"
        );
        assert!(rec.attempts.is_empty(), "no job node was given the run");
        assert!(rig.jobs[0].running.held_bundles().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_that_outlasts_its_wall_clock_is_retried_and_ends_lost_not_timed_out() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.begin("startRun", json!({ "method": "spin" })).await;
    let rec = until_ended(&rig, &run, 120).await;
    assert_eq!(rec.final_.unwrap().outcome, failed(FailedReason::Lost));
    assert_eq!(rec.attempts.len(), 3, "every attempt was spent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_the_queue_never_grants_ends_not_granted_instead_of_waiting_forever() {
    let rig = Rig::start(&[]).await;
    rig.limits(ComputeLimits {
        queued_timeout: Duration::from_secs(2),
        outage_credit: Duration::from_secs(1),
        ..ComputeLimits::default()
    });
    let run = rig.begin("startDigest", json!({ "inputs": [] })).await;
    let rec = until_ended(&rig, &run, 60).await;
    assert_eq!(
        rec.final_.unwrap().outcome,
        failed(FailedReason::NotGranted)
    );
    assert!(rec.attempts.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_a_call_that_starts_a_run_wakes_the_run_driver() {
    let rig = Rig::start(&[JOB_A]).await;
    let leader = rig.leader();
    let before = leader.run_kicks();
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "ping", Some(json!({})), None, None)
        .await
        .unwrap();
    assert_eq!(leader.run_kicks(), before, "an ordinary call woke it");
    let _run = rig.begin("startRun", json!({ "method": "burn" })).await;
    assert!(
        leader.run_kicks() > before,
        "a call that starts a run did not"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_that_never_answers_its_load_does_not_serialize_assignments() {
    let rig = Rig::start(&[JOB_A, JOB_B, JOB_C]).await;
    rig.jobs[0].running.freeze();
    let started = Instant::now();
    let mut runs = Vec::new();
    for method in ["spin", "burn", "hog"] {
        runs.push(rig.begin("startRun", json!({ "method": method })).await);
    }
    for run in &runs {
        rig.until("the run is placed", || !rig.record(run).attempts.is_empty())
            .await;
    }
    let took = started.elapsed();
    assert!(took < JOB_CONTROL_TIMEOUT * 2, "placed after {took:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_with_a_held_report_ends_at_the_ceiling_while_every_push_fails() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        deadline_grace: Duration::from_secs(1),
        outage_credit: Duration::from_secs(3),
        cancel_deadline: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    let file = rig.upload(&input()).await;
    let leader = rig.c.app_leader(&rig.app).unwrap();
    for i in (0..3).filter(|i| *i != leader) {
        let node = &rig.c.nodes[i].as_ref().unwrap().node;
        node.set_peer_blobs(PeerBlobs::Refuse);
    }
    let pushes = rig.leader().file_pushes();
    let run = rig
        .begin("startDigestBrief", json!({ "inputs": [file] }))
        .await;
    rig.until("the leader tried to push the output", || {
        rig.leader().file_pushes() > pushes
    })
    .await;
    rig.jobs[0].running.freeze();
    let _ok = rig
        .alice
        .apps()
        .call(&rig.app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    // digestBrief's wall cap is 10 s: the ceiling is that plus the grace and the credit.
    let rec = until_ended(&rig, &run, 60).await;
    assert_eq!(rec.final_.unwrap().outcome, FinalOutcome::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_failed_runs_message_is_shown_in_both_views_only_to_those_who_list_files() {
    let rig = Rig::start_with(&[JOB_A], json!({ "access": { "kind": "public" } })).await;
    let run = rig.begin("startRun", json!({ "method": "big" })).await;
    let _ = rig.outcome(&run).await;

    let owner = rig
        .alice
        .apps()
        .run(&rig.app, &run)
        .await
        .map(wire)
        .unwrap();
    let message = owner["final"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("too large"), "{owner}");
    let listed = rig
        .alice
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(
        listed["runs"][0]["final"]["message"],
        owner["final"]["message"]
    );

    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;
    let shown = bob.apps().run(&rig.app, &run).await.map(wire).unwrap();
    assert!(shown["final"]["message"].is_null(), "{shown}");
    assert!(shown["progress"].is_null(), "{shown}");
    let listed = bob
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    assert!(listed["runs"][0]["final"]["message"].is_null(), "{listed}");
    assert!(listed["runs"][0]["progress"].is_null(), "{listed}");
}

impl Rig {
    /// Starts job node `id` in `pool` and registers it.
    async fn join_pool(&self, id: u64, pool: &str) -> JobNode {
        let job = self.c.start_job_node(id, |_| {}).await;
        let ops = self.c.client(0);
        ops.use_api_key("ops-secret").await;
        let node = json!({ "id": id, "addr": job.url, "role": "job", "pool": pool });
        let _ = common::ops_signed(&ops, "ops/nodes", node).await.unwrap();
        let leader = self.leader();
        self.until("the leader applied the job node", || {
            leader.sys(|s| s.job_nodes().any(|n| n.id == id))
        })
        .await;
        job
    }

    /// The `queue` of the run in `view` (the single-run or the list view) once it shows one in
    /// `state`.
    async fn queue_in(&self, who: &Client, run: &str, state: &str) -> Value {
        let started = Instant::now();
        loop {
            let shown = who.apps().run(&self.app, run).await.map(wire).unwrap();
            if shown["queue"]["state"] == state {
                return shown["queue"].clone();
            }
            assert!(started.elapsed() < WAIT, "no queue in {state}: {shown}");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_queued_run_says_why_it_waits_and_proceeds_once_its_pool_has_a_job_node() {
    let rig = Rig::start_with(&[], json!({ "access": { "kind": "public" } })).await;
    let ops = rig.c.client(0);
    ops.use_api_key("ops-secret").await;
    let pools = json!({ "pools": {
        "small": { "runsPerNode": 2, "maxMemoryMib": 512 },
        "big": { "runsPerNode": 2, "maxMemoryMib": 4096 },
    } });
    let _ = common::ops_signed(&ops, "ops/pools", pools).await.unwrap();
    // The run fits "small", and only "big" has a job node.
    let _big = rig.join_pool(JOB_A, "big").await;
    let run = rig.begin("startDigest", json!({ "inputs": [] })).await;

    let queue = rig.queue_in(&rig.alice, &run, "waiting").await;
    assert_eq!(
        queue,
        json!({ "state": "waiting", "pool": "small", "capacity": 0, "reason": "NO_CAPACITY" })
    );
    let listed = rig
        .alice
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(listed["runs"][0]["queue"], queue);

    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;
    let seen = bob.apps().run(&rig.app, &run).await.map(wire).unwrap();
    assert!(seen["queue"].is_null(), "{seen}");
    let listed = bob
        .apps()
        .runs(&rig.app, None, None)
        .await
        .map(wire)
        .unwrap();
    assert!(listed["runs"][0]["queue"].is_null(), "{listed}");

    let _small = rig.join_pool(JOB_B, "small").await;
    let _ = rig.outcome(&run).await;
    let ended = rig
        .alice
        .apps()
        .run(&rig.app, &run)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(ended["state"], "done", "{ended}");
    assert!(ended["queue"].is_null(), "{ended}");
}

impl Rig {
    /// A finished `digest` run over a 100 kB input: its output id and the plaintext it wrote.
    async fn finished_output(&self) -> (String, Vec<u8>) {
        let plain = input()[..100_000].to_vec();
        let file = self.upload(&plain).await;
        let run = self.begin("startDigest", json!({ "inputs": [file] })).await;
        let outcome = self.outcome(&run).await;
        assert_eq!(outcome["status"], "done", "{outcome}");
        let output = self.record(&run).final_.unwrap().outputs[0].id.clone();
        let seed = noise_seed(&self.leader().app_file_key(&self.app), &run);
        (output, expected_output(&plain, seed))
    }

    async fn publish(&self, id: &str) -> String {
        let r = self
            .alice
            .apps()
            .call(
                &self.app,
                "publish",
                Some(json!({ "output": id })),
                None,
                None,
            )
            .await
            .unwrap();
        r.result.as_str().unwrap().to_owned()
    }

    async fn grant(&self, id: &str) -> String {
        let r = self
            .alice
            .apps()
            .call(
                &self.app,
                "share",
                Some(json!({ "id": id, "ttl": 600 })),
                None,
                None,
            )
            .await
            .unwrap();
        r.result.as_str().unwrap().to_owned()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_published_output_downloads_as_exactly_the_plaintext_the_run_wrote() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let charged = rig.charged().await;

    let refused = rig
        .alice
        .apps()
        .blob_download(&rig.app, &output, None)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("NOT_SUPPORTED"), "{refused}");

    let blob = rig.publish(&output).await;
    assert_eq!(blob, sha256_hex(&expected));
    let record = rig.blob(&blob).expect("the app holds the plain blob");
    assert!(record.file.is_none() && record.charged && record.refs == 1);
    assert_eq!(rig.charged().await, charged + expected.len() as u64);

    let grant = rig.grant(&blob).await;
    let bytes = rig
        .alice
        .apps()
        .blob_download(&rig.app, &blob, Some(&grant))
        .await
        .unwrap();
    assert_eq!(bytes, expected);
    let again = rig.publish(&output).await;
    assert_eq!(again, blob, "publishing twice answers the same blob");
    assert_eq!(rig.charged().await, charged + expected.len() as u64);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_published_blob_downloads_only_through_a_grant_a_logged_call_made() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let blob = rig.publish(&output).await;

    let e = rig
        .alice
        .apps()
        .blob_download(&rig.app, &blob, None)
        .await
        .unwrap_err();
    assert_eq!(
        e.code(),
        Some("NOT_FOUND"),
        "the owner needs a grant too: {e}"
    );
    let peeked = rig
        .alice
        .apps()
        .view(
            &rig.app,
            "shareView",
            Some(json!({ "id": blob, "ttl": 600 })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await;
    assert!(peeked.is_err(), "a view mints no grant for it");

    let grant = rig.grant(&blob).await;
    let bytes = rig
        .alice
        .apps()
        .blob_download(&rig.app, &blob, Some(&grant))
        .await;
    assert_eq!(bytes.unwrap(), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_revoked_grant_is_refused_on_every_replica_and_after_a_leader_change() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let blob = rig.publish(&output).await;
    let old = rig.grant(&blob).await;

    rig.alice.apps().blob_revoke(&rig.app, &blob).await.unwrap();
    for i in 0..3 {
        let e = rig
            .c
            .client(i)
            .apps()
            .blob_download(&rig.app, &blob, Some(&old))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("NOT_FOUND"), "node {i}: {e}");
    }
    let fresh = rig.grant(&blob).await;
    let bytes = rig
        .alice
        .apps()
        .blob_download(&rig.app, &blob, Some(&fresh))
        .await;
    assert_eq!(bytes.unwrap(), expected);

    let was = rig.c.app_leader(&rig.app).unwrap();
    rig.c.stop(was).await;
    rig.until("another node leads", || {
        rig.c.app_leader(&rig.app).is_some()
    })
    .await;
    let new = rig.c.client(usize::from(was == 0));
    let e = new
        .apps()
        .blob_download(&rig.app, &blob, Some(&old))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
    let bytes = new
        .apps()
        .blob_download(&rig.app, &blob, Some(&fresh))
        .await;
    assert_eq!(bytes.unwrap(), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_revoked_grant_stays_revoked_after_the_blob_is_deleted_and_published_again() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let blob = rig.publish(&output).await;
    let revoked = rig.grant(&blob).await;
    rig.alice.apps().blob_revoke(&rig.app, &blob).await.unwrap();
    let live = rig.grant(&blob).await;

    rig.alice.apps().blob_delete(&rig.app, &blob).await.unwrap();
    assert_eq!(rig.publish(&output).await, blob);
    for (name, old) in [("revoked", &revoked), ("deleted", &live)] {
        let e = rig
            .alice
            .apps()
            .blob_download(&rig.app, &blob, Some(old))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("NOT_FOUND"), "{name} grant: {e}");
    }
    let fresh = rig.grant(&blob).await;
    let bytes = rig
        .alice
        .apps()
        .blob_download(&rig.app, &blob, Some(&fresh))
        .await;
    assert_eq!(bytes.unwrap(), expected);
}

impl Rig {
    /// Deletes a published blob and waits for its queued collection to be logged, so a later
    /// publish of the same bytes meets no collection but the one a test starts.
    async fn delete_and_settle(&self, blob: &str) {
        self.alice
            .apps()
            .blob_delete(&self.app, blob)
            .await
            .unwrap();
        self.until("the collection of the deleted blob is logged", || {
            self.leader()
                .hosted(&self.app)
                .unwrap()
                .read_local(|m, _| m.state.component.head.collect.is_empty())
        })
        .await;
    }

    async fn assert_downloads_after_publishing_again(&self, blob: &str, expected: &[u8]) {
        let fresh = self.grant(blob).await;
        let bytes = self
            .alice
            .apps()
            .blob_download(&self.app, blob, Some(&fresh))
            .await;
        assert_eq!(bytes.unwrap(), expected);
    }
}

fn raw_id(id: &str) -> [u8; 32] {
    <[u8; 32]>::try_from(hex::decode(id).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_collection_between_a_publishes_push_and_its_commit_leaves_the_bytes() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let blob = rig.publish(&output).await;
    rig.delete_and_settle(&blob).await;

    let (reached, resume) = rig.leader().hold_next_file_push();
    let collect = async {
        reached.notified().await;
        rig.leader().collect_blob(&raw_id(&blob)).await;
        resume.notify_one();
    };
    let (again, ()) = tokio::join!(rig.publish(&output), collect);
    assert_eq!(again, blob);
    rig.assert_downloads_after_publishing_again(&blob, &expected)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_peer_asked_to_delete_between_a_publishes_push_and_its_commit_keeps_the_bytes() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, _expected) = rig.finished_output().await;
    let blob = rig.publish(&output).await;
    rig.delete_and_settle(&blob).await;

    let leader = rig.c.app_leader(&rig.app).unwrap();
    let (reached, resume) = rig.leader().hold_next_file_push();
    let ask = async {
        reached.notified().await;
        for i in (0..3).filter(|i| *i != leader) {
            let _refused = rig
                .leader()
                .peer_post(
                    &rig.c.urls[i],
                    "internal/blob/delete",
                    &json!({ "hash": blob }),
                )
                .await;
        }
        resume.notify_one();
    };
    let (again, ()) = tokio::join!(rig.publish(&output), ask);
    assert_eq!(again, blob);
    for i in 0..3 {
        let node = &rig.c.nodes[i].as_ref().unwrap().node;
        assert!(
            node.engine.get_blob(&blob).unwrap().is_some(),
            "node {i} lost the bytes"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_peers_delete_that_checked_before_a_publish_stored_its_bytes_keeps_them() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let blob = rig.publish(&output).await;
    rig.delete_and_settle(&blob).await;

    let leader = rig.c.app_leader(&rig.app).unwrap();
    let peer = (0..3).find(|i| *i != leader).unwrap();
    let (reached, resume) = rig.c.nodes[peer]
        .as_ref()
        .unwrap()
        .node
        .hold_next_deletion();
    let body = json!({ "hash": blob });
    let ask = rig
        .leader()
        .peer_post(&rig.c.urls[peer], "internal/blob/delete", &body);
    let publish_again = async {
        reached.notified().await;
        let again = rig.publish(&output).await;
        resume.notify_one();
        again
    };
    let (_answer, again) = tokio::join!(ask, publish_again);
    assert_eq!(again, blob);
    let node = &rig.c.nodes[peer].as_ref().unwrap().node;
    assert!(
        node.engine.get_blob(&blob).unwrap().is_some(),
        "the peer lost the bytes a publish stored after its check"
    );
    rig.assert_downloads_after_publishing_again(&blob, &expected)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_owner_takes_a_published_blob_down_and_its_charge_is_released() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, _expected) = rig.finished_output().await;
    let charged = rig.charged().await;
    let blob = rig.publish(&output).await;
    let grant = rig.grant(&blob).await;
    assert!(rig.charged().await > charged);

    rig.alice.apps().blob_delete(&rig.app, &blob).await.unwrap();
    assert!(rig.blob(&blob).is_none());
    let e = rig
        .alice
        .apps()
        .blob_download(&rig.app, &blob, Some(&grant))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
    rig.until("the bytes are collected and the quota released", || {
        let now = tokio::runtime::Handle::current();
        !rig.holds_bytes(&blob)
            && tokio::task::block_in_place(|| now.block_on(rig.charged())) == charged
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_publish_apply_refuses_keeps_no_charge_and_no_bytes() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let charged = rig.charged().await;
    let blob = sha256_hex(&expected);

    rig.leader().forge_next_publish();
    let e = rig
        .alice
        .apps()
        .call(
            &rig.app,
            "publish",
            Some(json!({ "output": output })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e:?}");
    assert!(rig.blob(&blob).is_none());
    rig.until("the bytes are discarded and the charge refunded", || {
        let now = tokio::runtime::Handle::current();
        !rig.holds_bytes(&blob)
            && tokio::task::block_in_place(|| now.block_on(rig.charged())) == charged
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_an_output_of_this_apps_done_runs_publishes() {
    let rig = Rig::start(&[JOB_A]).await;
    let (output, _expected) = rig.finished_output().await;
    let uploaded = rig.upload(&input()[..1000]).await;
    let other = rig
        .alice
        .apps()
        .create(typed(
            json!({ "name": "compute2", "package": "com.test.compute", "version": "1.0.0" }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    for (what, app, id) in [
        ("an uploaded file", rig.app.as_str(), uploaded.as_str()),
        ("an unknown id", rig.app.as_str(), &"ab".repeat(32)),
        ("another app's output", other.as_str(), output.as_str()),
    ] {
        let e = rig
            .alice
            .apps()
            .call(app, "publish", Some(json!({ "output": id })), None, None)
            .await
            .unwrap_err();
        assert!(
            format!("{e:?}").contains("NOT_A_RUN_OUTPUT"),
            "{what}: {e:?}"
        );
    }
    assert!(rig.blob(&uploaded).is_some_and(|r| r.file.is_some()));
    let e = rig
        .alice
        .apps()
        .blob_download(&rig.app, &uploaded, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_SUPPORTED"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_published_blob_survives_a_leader_change() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let (output, expected) = rig.finished_output().await;
    let blob = rig.publish(&output).await;
    let grant = rig.grant(&blob).await;
    let old = rig.c.app_leader(&rig.app).unwrap();
    rig.c.stop(old).await;
    let client = usize::from(old == 0);
    rig.until("another node leads", || {
        rig.c.app_leader(&rig.app).is_some()
    })
    .await;
    let new = rig.c.client(client);
    let bytes = new
        .apps()
        .blob_download(&rig.app, &blob, Some(&grant))
        .await
        .unwrap();
    assert_eq!(bytes, expected);
}

/// What is stored and then recorded: a plain upload, a run's output published, a sealed file
/// sent as one request, or one sent through an upload session.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Plain,
    Published,
    File,
    Session,
}

/// The node a request goes through.
#[derive(Clone, Copy, Debug)]
enum Via {
    Leader,
    Follower,
}

/// One case of the store-and-delete matrix: a rig, a client on each node, and the blob.
struct Case<'a> {
    rig: &'a Rig,
    kind: Kind,
    clients: Vec<Client>,
    leader: usize,
    follower: usize,
    /// A run's finished output to publish; unused for a plain upload.
    output: String,
    /// The bytes of a plain blob, a published output, or a sealed file's plaintext.
    expected: Vec<u8>,
}

impl Case<'_> {
    fn node(&self, via: Via) -> usize {
        match via {
            Via::Leader => self.leader,
            Via::Follower => self.follower,
        }
    }

    fn node_ref(&self, i: usize) -> &Node {
        &self.rig.c.nodes[i].as_ref().unwrap().node
    }

    /// Stores the blob through node `i` and records it; returns its id.
    async fn put(&self, i: usize) -> String {
        match self.kind {
            Kind::Plain => {
                let up = self.clients[i]
                    .apps()
                    .blob_upload(&self.rig.app, self.expected.clone(), None)
                    .await
                    .map(wire)
                    .unwrap();
                up["id"].as_str().unwrap().to_owned()
            }
            Kind::File | Kind::Session => {
                let up = self.clients[i]
                    .upload_bytes(&self.rig.app, self.expected.clone())
                    .await
                    .unwrap();
                up["id"].as_str().unwrap().to_owned()
            }
            Kind::Published => {
                let r = self.clients[i]
                    .apps()
                    .call(
                        &self.rig.app,
                        "publish",
                        Some(json!({ "output": self.output })),
                        None,
                        None,
                    )
                    .await
                    .unwrap();
                r.result.as_str().unwrap().to_owned()
            }
        }
    }

    /// The node whose next push of bytes a test holds: the receiving node for a plain upload,
    /// the leader for a publish (its entry stores the bytes).
    fn pushing_node(&self, up: usize) -> usize {
        match self.kind {
            Kind::Plain | Kind::File | Kind::Session => up,
            Kind::Published => self.leader,
        }
    }

    /// The owner's delete through node `i`, then until the leader's queue of collections is empty.
    async fn delete(&self, i: usize, id: &str) {
        self.clients[i]
            .apps()
            .blob_delete(&self.rig.app, id)
            .await
            .unwrap();
        self.rig
            .until("the collection of the deleted blob is logged", || {
                self.rig
                    .leader()
                    .hosted(&self.rig.app)
                    .unwrap()
                    .read_local(|m, _| m.state.component.head.collect.is_empty())
            })
            .await;
    }

    fn nodes_with_bytes(&self, id: &str) -> usize {
        (0..self.clients.len())
            .filter(|i| self.node_ref(*i).engine.get_blob(id).unwrap().is_some())
            .count()
    }

    async fn assert_downloads_from_a_majority(&self, id: &str) {
        let bytes = match self.kind {
            Kind::Plain => self.clients[self.leader]
                .apps()
                .blob_download(&self.rig.app, id, None)
                .await
                .unwrap(),
            Kind::File | Kind::Session => unreachable!("files are read by runs"),
            Kind::Published => {
                let grant = self.rig.grant(id).await;
                self.rig
                    .alice
                    .apps()
                    .blob_download(&self.rig.app, id, Some(&grant))
                    .await
                    .unwrap()
            }
        };
        assert_eq!(bytes, self.expected, "the download differs");
        let held = self.nodes_with_bytes(id);
        assert!(held * 2 > self.clients.len(), "bytes on {held} nodes only");
    }

    /// A collection on node `del` runs while a store through node `up` has pushed its bytes and
    /// not committed.
    async fn collect_during_store(&self, up: usize, del: usize, id: &str) {
        let (reached, resume) = self.node_ref(self.pushing_node(up)).hold_next_file_push();
        let collect = async {
            reached.notified().await;
            self.node_ref(del).collect_blob(&raw_id(id)).await;
            resume.notify_one();
        };
        let (again, ()) = tokio::join!(self.put(up), collect);
        assert_eq!(again, id, "the id changed");
    }

    /// A store through node `up` runs while a collection on node `del` is between its check
    /// and its delete.
    async fn store_during_collect(&self, up: usize, del: usize, id: &str) {
        let (reached, resume) = self.node_ref(del).hold_next_deletion();
        let raw = raw_id(id);
        let collect = self.node_ref(del).collect_blob(&raw);
        let store = async {
            reached.notified().await;
            let again = self.put(up).await;
            resume.notify_one();
            again
        };
        let ((), again) = tokio::join!(collect, store);
        assert_eq!(again, id, "the id changed");
    }

    /// A run reads file `id` and writes an output of the plaintext XOR its stream; the segments
    /// of the file are on a majority of the nodes.
    async fn assert_file_reads_from_a_majority(&self, id: &str) {
        let segments = self.rig.segments(id);
        let want = if matches!(self.kind, Kind::Session) {
            2
        } else {
            1
        };
        assert!(segments.len() >= want, "{} segments", segments.len());
        let held = (0..self.clients.len())
            .filter(|i| self.rig.holds_segments(*i, &segments))
            .count();
        assert!(
            held * 2 > self.clients.len(),
            "segments on {held} nodes only"
        );
        let run = self
            .rig
            .begin("startUnkept", json!({ "inputs": [id] }))
            .await;
        let ended = until_ended(self.rig, &run, 120).await;
        assert_eq!(ended.state, RunState::Done, "the run did not read the file");
        let seed = noise_seed(&self.rig.leader().app_file_key(&self.rig.app), &run);
        let output = ended.final_.unwrap().outputs[0].sha256.0.clone();
        assert_eq!(
            hex::encode(output),
            sha256_hex(&expected_output(&self.expected, seed)),
            "the run read other bytes"
        );
    }

    /// Passes the idle time with an entry logged through node `i`, which forgets the file.
    async fn forget_unreferenced(&self, i: usize, id: &str) {
        self.rig.c.advance_all(PAST_IDLE_MS);
        self.clients[i]
            .apps()
            .call(&self.rig.app, "ping", Some(json!({})), None, None)
            .await
            .unwrap();
        self.rig
            .until("the app forgets the file", || self.rig.blob(id).is_none())
            .await;
    }

    /// Files have no delete and no plain-blob id: one is forgotten after its idle time and its
    /// segments, which no new upload shares, go with the sweep after its grace.
    async fn run_file(&self, up: usize, del: usize, baseline: u64) {
        let id = self.put(up).await;
        self.assert_file_reads_from_a_majority(&id).await;
        let old = self.rig.segments(&id);

        self.rig.sweep_past_grace();
        self.assert_file_reads_from_a_majority(&id).await;

        self.forget_unreferenced(del, &id).await;
        self.rig
            .until("the segments are gone on every node", || {
                self.rig.sweep_past_grace();
                !self.rig.holds_bytes(&id)
            })
            .await;

        let again = self.put(up).await;
        assert_ne!(again, id, "a file got an id it had before");
        let fresh = self.rig.segments(&again);
        assert!(
            fresh.iter().all(|h| !old.contains(h)),
            "an upload shares segments with an earlier one"
        );
        self.assert_file_reads_from_a_majority(&again).await;
        self.forget_unreferenced(del, &again).await;
        self.rig
            .until("the segments are gone on every node", || {
                self.rig.sweep_past_grace();
                !self.rig.holds_bytes(&again)
            })
            .await;
        let started = Instant::now();
        while self.rig.charged().await != baseline {
            assert!(started.elapsed() < WAIT, "the quota charge is not released");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn run(&self, up: Via, del: Via, baseline: u64) {
        let (up, del) = (self.node(up), self.node(del));
        if matches!(self.kind, Kind::File | Kind::Session) {
            return self.run_file(up, del, baseline).await;
        }
        let id = self.put(up).await;
        self.assert_downloads_from_a_majority(&id).await;

        self.delete(del, &id).await;
        self.collect_during_store(up, del, &id).await;
        self.assert_downloads_from_a_majority(&id).await;

        self.delete(del, &id).await;
        self.store_during_collect(up, del, &id).await;
        self.assert_downloads_from_a_majority(&id).await;

        self.delete(del, &id).await;
        self.rig
            .until("the bytes are gone on every node", || {
                self.nodes_with_bytes(&id) == 0
            })
            .await;
        let started = Instant::now();
        while self.rig.charged().await != baseline {
            assert!(started.elapsed() < WAIT, "the quota charge is not released");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

// One test per kind, each over the four (upload, delete) node pairs: a case starts a cluster
// of its own, so the kinds run in parallel rather than sixteen clusters one after another.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_blob_stored_again_after_a_delete_survives_whichever_nodes_handle_it() {
    blob_stored_again_after_a_delete(Kind::Plain).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_published_output_stored_again_after_a_delete_survives_whichever_nodes_handle_it() {
    blob_stored_again_after_a_delete(Kind::Published).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_file_stored_again_after_a_delete_survives_whichever_nodes_handle_it() {
    blob_stored_again_after_a_delete(Kind::File).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_session_file_stored_again_after_a_delete_survives_whichever_nodes_handle_it() {
    blob_stored_again_after_a_delete(Kind::Session).await;
}

/// Stores a blob of `kind` through the leader or a follower, deletes it through either, and
/// stores it again: the bytes survive, whichever nodes each step went through.
async fn blob_stored_again_after_a_delete(kind: Kind) {
    let mut failures = Vec::new();
    for up in [Via::Leader, Via::Follower] {
        for del in [Via::Leader, Via::Follower] {
            let label = format!("kind={kind:?} up={up:?} del={del:?}");
            let jobs: &[u64] = match kind {
                Kind::Plain => &[],
                Kind::Published | Kind::File | Kind::Session => &[JOB_A],
            };
            let mut rig = Rig::start(jobs).await;
            let (output, expected) = match kind {
                Kind::Plain => (String::new(), format!("plain {label}").into_bytes()),
                Kind::Published => rig.finished_output().await,
                Kind::File => (String::new(), input()),
                Kind::Session => (String::new(), vec![7; SESSION_LEN]),
            };
            let baseline = rig.charged().await;
            let leader = rig.c.app_leader(&rig.app).unwrap();
            let follower = (0..3).find(|i| *i != leader).unwrap();
            let mut clients = Vec::new();
            for i in 0..3 {
                let client = rig.c.client(i);
                client.sign_in(&mut rig.pk).await.unwrap();
                clients.push(client);
            }
            let case = Case {
                rig: &rig,
                kind,
                clients,
                leader,
                follower,
                output,
                expected,
            };
            let ran = std::panic::AssertUnwindSafe(case.run(up, del, baseline))
                .catch_unwind()
                .await;
            if let Err(e) = ran {
                let why = e
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| e.downcast_ref::<&str>().copied())
                    .unwrap_or("panicked");
                failures.push(format!("{label}: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

impl Rig {
    /// A run of the argument-less compute method `method`, ended.
    async fn ended(&self, method: &str) -> (String, Value) {
        let run = self.begin("startRun", json!({ "method": method })).await;
        let outcome = self.outcome(&run).await;
        (run, outcome)
    }

    /// The logs of `run` as the owner lists them.
    async fn logs(&self, run: &str) -> Vec<Value> {
        let listed = self.alice.apps().run_logs(&self.app, run).await.unwrap();
        let listed = serde_json::to_value(listed).unwrap();
        listed["logs"].as_array().unwrap().clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_run_s_log_reads_back_exactly() {
    let rig = Rig::start(&[JOB_A]).await;
    let (run, outcome) = rig.ended("chatter").await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    let logs = rig.logs(&run).await;
    assert_eq!(logs.len(), 1, "{logs:?}");
    let text = "first line\nsecond line";
    assert_eq!(logs[0]["attempt"], 1);
    assert_eq!(logs[0]["size"], text.len());
    assert_eq!(logs[0]["sha256"], sha256_hex(text.as_bytes()));
    let id = logs[0]["id"].as_str().unwrap();
    assert_eq!(
        rig.alice.export_file(&rig.app, id).await.unwrap(),
        text.as_bytes()
    );
    assert_eq!(rig.record(&run).log.unwrap().id, id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_failed_run_keeps_its_log() {
    let rig = Rig::start(&[JOB_A]).await;
    let (run, outcome) = rig.ended("failLoudly").await;
    assert_eq!(outcome["status"], "failed", "{outcome}");
    assert_eq!(outcome["reason"], "app-error", "{outcome}");
    let logs = rig.logs(&run).await;
    let id = logs[0]["id"].as_str().unwrap();
    assert_eq!(
        rig.alice.export_file(&rig.app, id).await.unwrap(),
        b"about to fail"
    );
    assert_eq!(rig.record(&run).state, RunState::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_that_logs_nothing_has_no_log() {
    let rig = Rig::start(&[JOB_A]).await;
    let (run, outcome) = rig.ended("quiet").await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert!(rig.logs(&run).await.is_empty());
    assert!(rig.record(&run).log.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_log_cannot_be_published() {
    let rig = Rig::start(&[JOB_A]).await;
    let (run, _) = rig.ended("chatter").await;
    let id = rig.logs(&run).await[0]["id"].as_str().unwrap().to_owned();
    let e = rig
        .alice
        .apps()
        .call(
            &rig.app,
            "publish",
            Some(json!({ "output": id })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(format!("{e:?}").contains("NOT_A_RUN_OUTPUT"), "{e:?}");
    assert!(rig.blob(&id).is_some_and(|r| r.file.is_some_and(|f| f.log)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_stranger_cannot_list_a_run_s_logs() {
    let rig = Rig::start_with(&[JOB_A], json!({ "access": { "kind": "public" } })).await;
    let (run, _) = rig.ended("chatter").await;
    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;
    let e = bob.apps().run_logs(&rig.app, &run).await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let id = rig.logs(&run).await[0]["id"].as_str().unwrap().to_owned();
    let e = bob.export_file(&rig.app, &id).await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let shown = serde_json::to_value(bob.apps().run(&rig.app, &run).await.unwrap()).unwrap();
    assert!(shown.get("logs").is_none(), "{shown}");
    let listed = bob.apps().runs(&rig.app, None, None).await.unwrap();
    let listed = serde_json::to_string(&listed).unwrap();
    assert!(!listed.contains(&id), "{listed}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_log_the_job_node_misreports_is_skipped_and_the_run_still_commits() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.jobs[0].running.corrupt_logs();
    let (run, outcome) = rig.ended("chatter").await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert_eq!(rig.record(&run).state, RunState::Done);
    assert_eq!(rig.record(&run).attempts.len(), 1, "no attempt is lost");
    assert!(rig.logs(&run).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_log_the_job_node_will_not_hand_over_is_skipped_and_the_run_still_commits() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.jobs[0].running.fail_outputs();
    let (run, outcome) = rig.ended("failLoudly").await;
    assert_eq!(outcome["reason"], "app-error", "{outcome}");
    assert_eq!(rig.record(&run).attempts.len(), 1, "no attempt is lost");
    assert!(rig.logs(&run).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_started_while_a_replica_is_behind_keeps_no_log_and_still_commits() {
    let rig = Rig::start(&[JOB_A]).await;
    for n in rig.c.nodes.iter().flatten() {
        n.node.set_replica_behind(true);
    }
    let (run, outcome) = rig.ended("chatter").await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert!(rig.record(&run).log.is_none());
    assert!(rig.logs(&run).await.is_empty());
    for n in rig.c.nodes.iter().flatten() {
        n.node.set_replica_behind(false);
    }
    let (run, _) = rig.ended("chatter").await;
    assert_eq!(
        rig.logs(&run).await.len(),
        1,
        "logs return with the replicas"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_log_fetched_but_misreported_leaves_none_of_its_bytes_on_the_leader() {
    let rig = Rig::start(&[JOB_A]).await;
    let stored = |rig: &Rig| rig.leader().engine.list_segments().unwrap().len();
    let before = stored(&rig);
    rig.jobs[0].running.misreport_logs();
    let (run, outcome) = rig.ended("failLoudly").await;
    assert_eq!(outcome["reason"], "app-error", "{outcome}");
    assert!(rig.logs(&run).await.is_empty());
    rig.until("the log's segment is deleted", || stored(&rig) == before)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_held_report_with_a_log_is_not_finished_with_it_once_a_replica_is_behind() {
    let mut rig = Rig::start(&[JOB_A]).await;
    for n in rig.c.nodes.iter().flatten() {
        n.node.conflict_finals(true);
    }
    let run = rig.begin("startRun", json!({ "method": "chatter" })).await;
    let leader = rig.leader();
    rig.until("the final conflicted with its log staged", || {
        leader.conflicts.load(std::sync::atomic::Ordering::Relaxed) > 0
    })
    .await;
    // A replica falls behind, and the job node restarts: only the held report is left.
    for n in rig.c.nodes.iter().flatten() {
        n.node.set_replica_behind(true);
    }
    let old = rig.jobs.remove(0);
    let listen = old
        .url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    old.running.stop().await;
    let new = rig.c.start_job_node(JOB_A, |c| c.listen = listen).await;
    rig.jobs.push(new);
    tokio::time::sleep(Duration::from_secs(8)).await;
    for n in rig.c.nodes.iter().flatten() {
        n.node.conflict_finals(false);
    }
    let outcome = rig.outcome_within(&run, 90).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    assert!(rig.record(&run).log.is_none(), "a log no replica may read");
}
