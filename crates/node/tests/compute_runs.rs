//! Compute runs driven by an app's leader through real job nodes: assignment, polling, restart
//! on a lost job node, cancel, caps, the result method, usage and the queue. The app is the
//! `compute` fixture (`apps/v2-fixtures/compute`).

mod common;

use std::time::{Duration, Instant};

use calimero_client::{Client, Consistency};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::component::blobs::BlobRecord;
use calimero_node::component::{
    CallEntry, ComponentCmd, ComputeStart, FailedReason, FinalEntry, FinalOutcome, Outcome,
    RunCaps, RunRecord, RunState,
};
use calimero_node::files::noise_seed;
use calimero_node::job_wire::{Report, ReportStatus, ReportUsage};
use calimero_node::node::{ComputeLimits, PeerBlobs, JOB_CONTROL_TIMEOUT, JOB_POLL};
use calimero_node::types::Caller;
use calimero_node::Node;
use common::{mpk_with, signed_up, Cluster, JobNode, PUBLISHER};
use rand_chacha::rand_core::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde_json::{json, Value};

const JOB_A: u64 = 4;
const JOB_B: u64 = 5;
const INPUT_LEN: usize = 3 * 1024 * 1024 + 512 * 1024; // 3.5 MiB
const READ_PIECE: usize = 64 * 1024; // what the fixture reads and keys at a time
const WAIT: Duration = Duration::from_secs(180);
const PAST_IDLE_MS: i64 = 3_700_000; // past the idle time of an unreferenced blob

struct Rig {
    c: Cluster,
    alice: Client,
    pk: SoftAuthenticator,
    app: String,
    jobs: Vec<JobNode>,
    /// The hashes of the installed release's app component and compute component.
    hashes: (String, String),
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
            .upload_package(mpk_with(
                "com.test.compute",
                "1.0.0",
                &both.app,
                Some(&compute),
                &key,
            ))
            .await
            .unwrap();
        let app = alice
            .create_app_from_package("compute", "com.test.compute", Some("1.0.0"), None, options)
            .await
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
        }
    }

    fn leader(&self) -> &Node {
        &self.c.nodes[self.c.app_leader(&self.app).unwrap()]
            .as_ref()
            .unwrap()
            .node
    }

    async fn upload(&self, plain: &[u8]) -> String {
        let up = self
            .alice
            .upload_file(&self.app, plain.to_vec())
            .await
            .unwrap();
        up["id"].as_str().unwrap().to_owned()
    }

    async fn begin(&self, method: &str, args: Value) -> String {
        let r = self
            .alice
            .call(&self.app, method, args, None)
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
                .view(
                    &self.app,
                    "outcome",
                    json!({ "run": run }),
                    Consistency::Strong,
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
            assert!(started.elapsed() < WAIT, "{names:?} in\n{metrics}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn metrics(&self) -> String {
        let leader = self.c.app_leader(&self.app).unwrap();
        let ops = self.c.client(leader);
        ops.use_api_key("ops-secret").await;
        ops.get_text("ops/metrics").await.unwrap()
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

    fn holds_bytes(&self, id: &str) -> bool {
        (0..3).any(|i| {
            let node = &self.c.nodes[i].as_ref().unwrap().node;
            node.engine.get_blob(id).unwrap().is_some()
        })
    }

    /// Advances every clock past the idle time and logs an entry, which forgets what is due.
    async fn pass_the_idle_time(&self) {
        self.c.advance_all(PAST_IDLE_MS);
        let _ok = self
            .alice
            .call(&self.app, "ping", json!({}), None)
            .await
            .unwrap();
    }

    /// The file bytes the payer of alice's apps is charged for.
    async fn charged(&self) -> u64 {
        let account = self.alice.me().await.unwrap()["accountId"]
            .as_str()
            .unwrap()
            .to_owned();
        let usage = self.alice.usage().await.unwrap();
        usage["fileBytes"][format!("account:{account}")]
            .as_u64()
            .unwrap_or(0)
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
        .view(&rig.app, "done", json!({}), Consistency::Strong)
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
        .get(&format!("apps/{}/runs/{run}", rig.app))
        .await
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
        .view(&rig.app, "done", json!({}), Consistency::Strong)
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
        .call(&rig.app, "cancel", json!({ "run": run }), None)
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
    for (method, reason) in [("hog", "memory"), ("burn", "fuel")] {
        let run = rig.begin("startRun", json!({ "method": method })).await;
        let outcome = rig.outcome(&run).await;
        assert_eq!(outcome["status"], "failed", "{method}: {outcome}");
        assert_eq!(outcome["reason"], reason, "{method}");
        assert_eq!(outcome["outputs"], 0);
        assert_eq!(rig.record(&run).state, RunState::Failed);
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
        .view(&rig.app, "done", json!({}), Consistency::Strong)
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
    let path = format!("apps/{}/runs/{run}", rig.app);
    let owner = rig.alice.get(&path).await.unwrap();
    assert!(owner["inputs"][0]["sha256"].is_string(), "{owner}");
    assert!(
        owner["final"]["outputs"][0]["sha256"].is_string(),
        "{owner}"
    );

    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;
    let shown = bob.get(&path).await.unwrap();
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
async fn a_result_over_the_cap_fails_the_run_and_registers_nothing() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.begin("startRun", json!({ "method": "big" })).await;
    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "failed", "{outcome}");
    assert_eq!(outcome["reason"], "trap", "{outcome}");
    assert_eq!(rig.record(&run).attempts.len(), 1, "no attempt is lost");
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
async fn a_final_that_conflicts_is_run_again_and_leaks_no_output() {
    let rig = Rig::start(&[JOB_A]).await;
    let plain = input();
    let file = rig.upload(&plain).await;
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    rig.leader().conflict_next_call();
    let outcome = rig.outcome(&run).await;
    assert_eq!(outcome["status"], "done", "{outcome}");
    let output = rig.record(&run).final_.unwrap().outputs[0].id.clone();
    let stored = |i: usize| {
        rig.c.nodes[i]
            .as_ref()
            .unwrap()
            .node
            .engine
            .get_blob(&output)
            .unwrap()
            .is_some()
    };
    rig.until("the output is on a majority", || {
        (0..3).filter(|i| stored(*i)).count() >= 2
    })
    .await;
    // Counted once: the input and the output.
    let counted = rig.c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.app_file_bytes.get(&rig.app).copied().unwrap_or(0));
    assert_eq!(
        counted,
        2 * INPUT_LEN as u64,
        "the retry charged nothing twice"
    );
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
    // Rounds and sweeps go on after the end; nothing may count it a second time.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let fuel = rig.record(&run).final_.unwrap().usage.fuel as f64;
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
    let output = rig.record(&run).final_.unwrap().outputs[0].id.clone();
    let holds = |i: usize| {
        rig.c.nodes[i]
            .as_ref()
            .unwrap()
            .node
            .engine
            .get_blob(&output)
            .unwrap()
            .is_some()
    };
    rig.until("a majority holds the output", || {
        (0..3).filter(|i| holds(*i)).count() >= 2
    })
    .await;

    // A losing leader's cleanup of the same id: the barrier and the strong read find the blob record.
    rig.leader()
        .clean_up_outputs(&rig.app, vec![output.clone()])
        .await;
    // A peer's delete request: the replica itself refuses an id its app holds.
    let leader = rig.c.app_leader(&rig.app).unwrap();
    for i in (0..3).filter(|i| *i != leader && holds(*i)) {
        let _ = rig
            .leader()
            .peer_post(
                &rig.c.urls[i],
                "internal/blob/delete",
                &json!({ "hash": output }),
            )
            .await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        (0..3).filter(|i| holds(*i)).count() >= 2,
        "the registered output kept its bytes"
    );
    assert!(holds(leader));
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
        .call(&rig.app, "cancel", json!({ "run": run }), None)
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
        .view(&rig.app, "done", json!({}), Consistency::Strong)
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
    // The digest method's wall cap is 60 s: the ceiling is that plus the grace and the credit.
    let run = rig.begin("startDigest", json!({ "inputs": [file] })).await;
    let rec = until_ended(&rig, &run, 110).await;
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
        .call(&rig.app, "cancel", json!({ "run": run }), None)
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
async fn a_cancel_ends_in_time_while_every_transfer_to_job_nodes_is_stuck() {
    let rig = Rig::start(&[JOB_A]).await;
    rig.limits(ComputeLimits {
        cancel_deadline: Duration::from_secs(3),
        ..ComputeLimits::default()
    });
    let run = rig.begin("startRun", json!({ "method": "spin" })).await;
    rig.until("the run runs", || !rig.record(&run).attempts.is_empty())
        .await;
    rig.jobs[0].running.answer_running_always();
    // Transfers hold their places for minutes when a job node freezes mid-transfer.
    for n in rig.c.nodes.iter().flatten() {
        n.node.limit_job_transfers(0);
    }
    let _ok = rig
        .alice
        .call(&rig.app, "cancel", json!({ "run": run }), None)
        .await
        .unwrap();
    let started = Instant::now();
    let rec = until_ended(&rig, &run, 60).await;
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
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
        .call(&rig.app, "cancel", json!({ "run": run }), None)
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
        .call(&rig.app, "cancel", json!({ "run": first }), None)
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
        .call(&rig.app, "cancel", json!({ "run": run }), None)
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
        .call(&rig.app, "cancel", json!({ "run": run }), None)
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
            .view(
                &rig.app,
                "chained",
                json!({ "run": run }),
                Consistency::Strong,
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
