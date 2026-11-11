//! Pipeline runs driven by an app's leader through real job nodes: each step's work is an
//! ordinary compute run, placed, retried and ended by the run driver, and the pipeline loop only
//! decides which step starts when. The package is the `compute` fixture's compute component
//! alone (a compute-only app).

mod common;
use common::UploadBytes as _;

use std::time::{Duration, Instant};

use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::component::blobs::BLOB_IDLE_MS;
use calimero_node::component::{FailedReason, FinalOutcome, RunRecord, MAX_ACTIVE_PIPELINE_RUNS};
use calimero_node::Node;
use common::{mpk_with_pipelines, signed_up, Cluster, JobNode, PUBLISHER};
use serde_json::{json, Value};

const JOB_A: u64 = 4;
const JOB_B: u64 = 5;
const WAIT: Duration = Duration::from_secs(180);
const INPUT_LEN: usize = 3 * 1024 * 1024 + 512 * 1024; // 3.5 MiB

const CHAIN: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: chain
inputs:
  docs: { kind: files }
steps:
  a: { method: digest, inputs: [inputs.docs] }
  b: { method: digest, inputs: [a.outputs] }
  c: { method: echo, args: { value: b.result } }
";

const BROKEN: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: broken
inputs:
  docs: { kind: files, maxItems: 2 }
params:
  rounds: { type: number, default: 1, min: 1, max: 3 }
steps:
  a: { method: burn }
  b: { method: digest, inputs: [inputs.docs], needs: [a] }
  c: { method: echo, needs: [b] }
";

const SLOW: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: slow
steps:
  a: { method: spin }
  b: { method: digest, needs: [a] }
";

const LOTS: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: lots
inputs:
  docs: { kind: files }
steps:
  each: { method: check, forEach: inputs.docs, maxItems: 200, maxParallel: 6, allowFailures: 5% }
  all: { method: digest, collect: each.outputs }
";

const BATCHED: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: batched
inputs:
  docs: { kind: files }
steps:
  each: { method: check, forEach: inputs.docs, maxItems: 20, batch: 4 }
";

const EMPTY: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: empty
steps:
  none: { method: echo, args: { value: [] } }
  each: { method: check, forEach: none.outputs, maxItems: 5 }
";

const SMALL: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: small
inputs:
  docs: { kind: files }
steps:
  each: { method: check, forEach: inputs.docs, maxItems: 2 }
";

const EAGER: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: eager
failFast: true
steps:
  bad: { method: burn }
  slow: { method: spin }
  later: { method: digest, needs: [slow] }
";

const RESUME: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: resume
inputs:
  docs: { kind: files }
steps:
  each: { method: check, forEach: inputs.docs, maxItems: 20, maxParallel: 3 }
  all: { method: digest, collect: each.outputs }
";

const PARTIAL: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: partial
inputs:
  docs: { kind: files }
params:
  p: { type: string, default: one }
steps:
  a: { method: digest, inputs: [inputs.docs] }
  b: { method: echo, needs: [a], args: { value: [params.p] } }
  c: { method: echo, inputs: [a.outputs], args: { value: b.result } }
";

const FRESH: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: fresh
inputs:
  docs: { kind: files }
steps:
  a: { method: digest, inputs: [inputs.docs], cache: false }
";

const COSTED: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: costed
inputs:
  docs: { kind: files }
steps:
  each: { method: check, forEach: inputs.docs, maxItems: 20 }
";

const RAISE: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: raise
inputs:
  docs: { kind: files }
steps:
  each: { method: digest, forEach: inputs.docs, maxItems: 20, allowFailures: 2, limits: { fuel: 50000000 } }
";

const STRICT: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: strict
inputs:
  docs: { kind: files }
steps:
  a: { method: digest, inputs: [inputs.docs], limits: { fuel: 50000000 } }
";

const METERED: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: metered
inputs:
  docs: { kind: files }
steps:
  a: { method: digest, inputs: [inputs.docs] }
";

const PLACED: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: placed
inputs:
  docs: { kind: files }
steps:
  named: { method: digest, inputs: [inputs.docs], pool: big }
  plain: { method: digest, inputs: [inputs.docs] }
";

const CHAIN_V2: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: chain
inputs:
  docs: { kind: files }
steps:
  a: { method: digest, inputs: [inputs.docs] }
  b: { method: digest, inputs: [a.outputs] }
  c: { method: echo, args: { value: b.result } }
  d: { method: echo, needs: [c] }
";

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

struct Rig {
    c: Cluster,
    alice: Client,
    pk: SoftAuthenticator,
    app: String,
    compute: Vec<u8>,
    jobs: Vec<JobNode>,
}

impl Rig {
    async fn start(job_ids: &[u64]) -> Self {
        Self::start_with(job_ids, |_| {}).await
    }

    async fn start_with(job_ids: &[u64], configure: impl Fn(&mut calimero_node::Config)) -> Self {
        let c = Cluster::start_with(3, |cfg| {
            cfg.allow_unsigned_bundles = false;
            configure(cfg);
        })
        .await;
        let (alice, pk) = signed_up(&c, 0, "alice").await;
        let compute = common::components("compute")
            .compute
            .expect("a compute component");
        let pipelines = [
            ("pipelines/chain.yaml", CHAIN),
            ("pipelines/broken.yaml", BROKEN),
            ("pipelines/slow.yaml", SLOW),
            ("pipelines/lots.yaml", LOTS),
            ("pipelines/batched.yaml", BATCHED),
            ("pipelines/empty.yaml", EMPTY),
            ("pipelines/small.yaml", SMALL),
            ("pipelines/eager.yaml", EAGER),
            ("pipelines/resume.yaml", RESUME),
            ("pipelines/partial.yaml", PARTIAL),
            ("pipelines/fresh.yaml", FRESH),
            ("pipelines/costed.yaml", COSTED),
            ("pipelines/raise.yaml", RAISE),
            ("pipelines/strict.yaml", STRICT),
            ("pipelines/metered.yaml", METERED),
            ("pipelines/placed.yaml", PLACED),
        ];
        let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
        let _installed = alice
            .upload_package(mpk_with_pipelines(
                "com.test.job",
                "1.0.0",
                None,
                Some(&compute),
                &pipelines,
                &key,
            ))
            .await
            .unwrap();
        let app = alice
            .create_app_from_package("job", "com.test.job", Some("1.0.0"), None, json!({}))
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
            compute,
            jobs,
        }
    }

    /// The app's leader, or during an election any live node hosting it: callers poll or route.
    fn leader(&self) -> &Node {
        let hosting = |i: &usize| {
            self.c.nodes[*i]
                .as_ref()
                .is_some_and(|r| r.node.hosted(&self.app).is_some())
        };
        let i = self
            .c
            .app_leader(&self.app)
            .or_else(|| (0..self.c.nodes.len()).find(hosting))
            .expect("a live node hosts the app");
        &self.c.nodes[i].as_ref().unwrap().node
    }

    async fn upload(&self, plain: &[u8]) -> String {
        for _ in 0..5 {
            match self.alice.upload_bytes(&self.app, plain.to_vec()).await {
                Ok(up) => return up["id"].as_str().unwrap().to_owned(),
                Err(e) if e.code() == Some("FILES_UNAVAILABLE") => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => panic!("upload failed: {e}"),
            }
        }
        panic!("upload kept failing");
    }

    async fn start_run(&self, pipeline: &str, inputs: Value, params: Value) -> String {
        let started = self
            .alice
            .start_pipeline_run(&self.app, pipeline, inputs, params)
            .await
            .unwrap();
        started["run"].as_str().unwrap().to_owned()
    }

    async fn run(&self, run: &str) -> Value {
        self.alice.pipeline_run(&self.app, run).await.unwrap()
    }

    /// The pipeline run once it is no longer running.
    async fn ended(&self, run: &str) -> Value {
        let started = Instant::now();
        loop {
            let v = self.run(run).await;
            if v["state"] != "running" {
                return v;
            }
            assert!(
                started.elapsed() < WAIT,
                "pipeline run {run} did not end: {v}"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn until(&self, what: &str, mut ready: impl AsyncFnMut() -> bool) {
        let started = Instant::now();
        while !ready().await {
            assert!(started.elapsed() < WAIT, "{what}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn record(&self, run: &str) -> RunRecord {
        self.leader()
            .hosted(&self.app)
            .unwrap()
            .read_local(|m, _| m.run_record(run))
            .unwrap()
    }

    /// Every compute run record the app's leader keeps.
    fn all_records(&self) -> Vec<RunRecord> {
        self.leader()
            .hosted(&self.app)
            .unwrap()
            .read_local(|m, _| m.runs_page(100, None).0)
    }
}

fn step_run(v: &Value, step: &str) -> String {
    let runs = v["steps"][step]["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "step {step} has exactly one run: {v}");
    runs[0].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_three_step_pipeline_completes() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&vec![5u8; INPUT_LEN]).await;
    let run = rig
        .start_run("chain", json!({ "docs": [file] }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    for step in ["a", "b", "c"] {
        assert_eq!(done["steps"][step]["status"], "succeeded", "{done}");
        let record = rig.record(&step_run(&done, step));
        assert_eq!(record.attempts.len(), 1, "one attempt for step {step}");
        assert_eq!(record.result_method, "");
        assert_eq!(
            record.compute_bundle.as_deref(),
            Some(sha256_hex(&rig.compute).as_str())
        );
    }
    let a_out = done["steps"]["a"]["outputs"][0]["id"].as_str().unwrap();
    let b = rig.record(&step_run(&done, "b"));
    assert_eq!(b.inputs.len(), 1);
    assert_eq!(b.inputs[0].id, a_out, "b ran on a's output");
    let b_result = done["steps"]["b"]["result"].clone();
    assert!(b_result.is_array(), "{done}");
    let c = rig.record(&step_run(&done, "c"));
    let args: Value = serde_json::from_slice(&c.args.0).unwrap();
    assert_eq!(
        args,
        json!({ "value": b_result }),
        "c's args saw b's result"
    );
    assert_eq!(done["steps"]["c"]["result"], b_result, "echo gave it back");
    assert_eq!(rig.all_records().len(), 3);
    let listed = rig.alice.pipeline_runs(&rig.app).await.unwrap();
    assert_eq!(listed["runs"][0]["id"], json!(run));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn killing_the_leader_mid_run_resumes_with_no_item_run_twice() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&vec![5u8; INPUT_LEN]).await;
    let run = rig
        .start_run("chain", json!({ "docs": [file] }), json!({}))
        .await;
    rig.until("step b runs", async || {
        rig.run(&run).await["steps"]["b"]["status"] == "running"
    })
    .await;
    let old = rig.c.app_leader(&rig.app).unwrap();
    rig.c.stop(old).await;
    let alive = usize::from(old == 0);
    let alice = rig.c.client(alive);
    let mut pk = rig.pk;
    let _s = alice.sign_in(&mut pk).await.unwrap();
    let started = Instant::now();
    let done = loop {
        let v = alice.pipeline_run(&rig.app, &run).await;
        match v {
            Ok(v) if v["state"] != "running" => break v,
            _ => {}
        }
        assert!(
            started.elapsed() < WAIT,
            "the run did not end after the leader died"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert_eq!(done["state"], "succeeded", "{done}");
    let g = rig.c.nodes[alive]
        .as_ref()
        .unwrap()
        .node
        .hosted(&rig.app)
        .unwrap();
    let records = g.read_local(|m, _| m.runs_page(100, None).0);
    let mut ids: Vec<&str> = records.iter().map(|r| r.run.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![
            format!("{run}.a.0"),
            format!("{run}.b.0"),
            format!("{run}.c.0")
        ],
        "every step has exactly one run"
    );
    for r in &records {
        assert!(r.final_.is_some(), "{} has one final", r.run);
        assert!(r.attempts.len() <= 3);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_failed_step_marks_its_dependents_upstream_failed() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(b"some bytes").await;
    let run = rig
        .start_run("broken", json!({ "docs": [file] }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "failed", "{done}");
    assert_eq!(done["steps"]["a"]["status"], "failed", "{done}");
    assert_eq!(done["steps"]["a"]["reason"], "fuel", "{done}");
    assert_eq!(done["steps"]["b"]["status"], "upstreamFailed");
    assert_eq!(done["steps"]["c"]["status"], "upstreamFailed");
    assert_eq!(rig.all_records().len(), 1, "only the failed step ran");
}

fn step_runs(v: &Value, step: &str) -> Vec<String> {
    v["steps"][step]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_owned())
        .collect()
}

fn output_ids(v: &Value, step: &str) -> Vec<String> {
    v["steps"][step]["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap().to_owned())
        .collect()
}

impl Rig {
    async fn uploads(&self, texts: &[String]) -> Vec<String> {
        let mut ids = Vec::new();
        for text in texts {
            ids.push(self.upload(text.as_bytes()).await);
        }
        ids
    }

    /// The result values of a step's runs, in item order.
    fn rest_of(&self, v: &Value, step: &str) -> (Vec<String>, Value) {
        (output_ids(v, step), v["steps"][step]["results"].clone())
    }

    fn active_runs(&self) -> usize {
        self.leader()
            .hosted(&self.app)
            .unwrap()
            .read_local(|m, _| m.active_runs().len())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_fan_out_over_many_items_with_a_few_failures_succeeds_with_failures() {
    let rig = Rig::start(&[JOB_A, JOB_A + 1, JOB_A + 2]).await;
    let bad = [7, 41, 88];
    let texts: Vec<String> = (0..100)
        .map(|i| {
            if bad.contains(&i) {
                format!("fail {i}")
            } else {
                format!("item {i}")
            }
        })
        .collect();
    let files = rig.uploads(&texts).await;
    let run = rig
        .start_run("lots", json!({ "docs": files }), json!({}))
        .await;
    let started = Instant::now();
    let mut most = 0;
    let done = loop {
        most = most.max(rig.active_runs());
        let v = rig.run(&run).await;
        if v["state"] != "running" {
            break v;
        }
        assert!(started.elapsed() < WAIT * 3, "the run did not end: {v}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(done["state"], "succeededWithFailures", "{done}");
    assert_eq!(done["steps"]["each"]["status"], "succeeded", "{done}");
    assert_eq!(done["steps"]["each"]["failedItems"], 3, "{done}");
    assert_eq!(done["steps"]["each"]["fan"]["allowed"], 5);
    assert!(most <= 6, "at most maxParallel runs at once, saw {most}");
    let each = step_runs(&done, "each");
    assert_eq!(each.len(), 100);
    for (i, r) in each.iter().enumerate() {
        assert_eq!(r, &format!("{run}.each.{i}"));
        assert_eq!(
            rig.record(r).inputs[0].id,
            files[i],
            "item {i} is input {i}"
        );
    }
    let outputs = output_ids(&done, "each");
    assert_eq!(outputs.len(), 97);
    let all = rig.record(&step_run(&done, "all"));
    let collected: Vec<&str> = all.inputs.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        collected, outputs,
        "collect got 97 outputs (over 64) in item order"
    );
    assert_eq!(done["steps"]["all"]["status"], "succeeded", "{done}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn batch_groups_items_into_fewer_runs() {
    let rig = Rig::start(&[JOB_A, JOB_A + 1]).await;
    let texts: Vec<String> = (0..10).map(|i| format!("item {i}")).collect();
    let files = rig.uploads(&texts).await;
    let run = rig
        .start_run("batched", json!({ "docs": files }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    let runs = step_runs(&done, "each");
    let sizes: Vec<usize> = runs.iter().map(|r| rig.record(r).inputs.len()).collect();
    assert_eq!(sizes, [4, 4, 2]);
    let first: Vec<String> = rig
        .record(&runs[0])
        .inputs
        .iter()
        .map(|i| i.id.clone())
        .collect();
    assert_eq!(first, files[..4]);
    assert_eq!(done["steps"]["each"]["fan"]["batch"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_empty_list_skips_the_step() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.start_run("empty", json!({}), json!({})).await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["steps"]["none"]["status"], "succeeded");
    assert_eq!(done["steps"]["each"]["status"], "skipped", "{done}");
    assert_eq!(rig.all_records().len(), 1, "only `none` ran");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn more_items_than_max_items_fails_before_any_item_runs() {
    let rig = Rig::start(&[JOB_A]).await;
    let files = rig
        .uploads(&["a".to_owned(), "b".to_owned(), "c".to_owned()])
        .await;
    let run = rig
        .start_run("small", json!({ "docs": files }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "failed", "{done}");
    assert_eq!(done["steps"]["each"]["reason"], "TOO_MANY_ITEMS", "{done}");
    assert!(rig.all_records().is_empty(), "no item ran");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fail_fast_cancels_the_rest_of_the_run() {
    let rig = Rig::start(&[JOB_A, JOB_A + 1]).await;
    let run = rig.start_run("eager", json!({}), json!({})).await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "failed", "{done}");
    assert_eq!(done["steps"]["bad"]["status"], "failed", "{done}");
    assert_eq!(done["steps"]["bad"]["reason"], "fuel", "{done}");
    assert_eq!(done["steps"]["slow"]["status"], "failed", "{done}");
    assert_eq!(done["steps"]["slow"]["reason"], "cancelled", "{done}");
    assert_eq!(done["steps"]["later"]["status"], "cancelled", "{done}");
    let slow = rig.record(&step_run(&done, "slow"));
    assert_eq!(slow.state, calimero_node::component::RunState::Cancelled);
    assert!(rig.all_records().iter().all(|r| r.final_.is_some()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_pipeline_whose_definition_is_gone_fails() {
    use calimero_node::component::PipelineStartEntry;
    use calimero_node::node::CallRequest;
    use calimero_node::types::Caller;
    let rig = Rig::start(&[JOB_A]).await;
    let entry = PipelineStartEntry {
        id: "gone1".into(),
        pipeline: "chain".into(),
        version: "1.0.0".into(),
        hash: "ab".repeat(32),
        compute_bundle: sha256_hex(&rig.compute),
        modules: Default::default(),
        owner: "alice".into(),
        steps: vec!["a".into(), "b".into()],
        inputs: Default::default(),
        params: Default::default(),
        caller: Caller::System,
        payer: "account:alice".into(),
        fail_fast: false,
        cost: None,
        retry_of: None,
        limits: Default::default(),
        ts_ms: 0,
    };
    let req = CallRequest::host(
        rig.app.clone(),
        "__pipeline_start",
        serde_json::to_value(&entry).unwrap(),
        Caller::System,
    );
    let a = rig.leader().call(&req).await.unwrap();
    assert!(a.error.is_null(), "{a:?}");
    let done = rig.ended("gone1").await;
    assert_eq!(done["state"], "failed", "{done}");
    for step in ["a", "b"] {
        assert_eq!(done["steps"][step]["status"], "failed", "{done}");
        assert_eq!(
            done["steps"][step]["reason"], "PIPELINE_UNAVAILABLE",
            "{done}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn killing_the_leader_mid_fan_out_creates_no_run_index_twice() {
    let mut rig = Rig::start(&[JOB_A, JOB_A + 1]).await;
    let texts: Vec<String> = (0..12).map(|i| format!("item {i}")).collect();
    let files = rig.uploads(&texts).await;
    let run = rig
        .start_run("resume", json!({ "docs": files }), json!({}))
        .await;
    rig.until("some items ran", async || {
        rig.run(&run).await["steps"]["each"]["runs"]
            .as_array()
            .is_some_and(|r| r.len() >= 4)
    })
    .await;
    let old = rig.c.app_leader(&rig.app).unwrap();
    rig.c.stop(old).await;
    let alive = usize::from(old == 0);
    let alice = rig.c.client(alive);
    let mut pk = rig.pk;
    let _s = alice.sign_in(&mut pk).await.unwrap();
    let started = Instant::now();
    let done = loop {
        if let Ok(v) = alice.pipeline_run(&rig.app, &run).await {
            if v["state"] != "running" {
                break v;
            }
        }
        assert!(
            started.elapsed() < WAIT * 3,
            "the run did not end after the leader died"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert_eq!(done["state"], "succeeded", "{done}");
    let g = rig.c.nodes[alive]
        .as_ref()
        .unwrap()
        .node
        .hosted(&rig.app)
        .unwrap();
    let records = g.read_local(|m, _| m.runs_page(100, None).0);
    let mut ids: Vec<String> = records.iter().map(|r| r.run.clone()).collect();
    ids.sort();
    let mut want: Vec<String> = (0..12).map(|i| format!("{run}.each.{i}")).collect();
    want.push(format!("{run}.all.0"));
    want.sort();
    assert_eq!(ids, want, "every index exactly once");
    assert_eq!(output_ids(&done, "each").len(), 12);
    for r in &records {
        assert!(r.final_.is_some(), "{} has one final", r.run);
    }
}

/// A pipeline reads the app's sealed files and spends its payer's compute, so only those who may
/// handle the app's files start, cancel or retry one, even on a public app; anyone may read.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_those_who_handle_the_apps_files_start_cancel_or_retry_a_pipeline() {
    let rig = Rig::start(&[JOB_A]).await;
    let public = rig
        .alice
        .create_app_from_package(
            "open",
            "com.test.job",
            Some("1.0.0"),
            None,
            json!({ "access": { "kind": "public" } }),
        )
        .await
        .unwrap();
    let doc = rig
        .alice
        .upload_bytes(&public, b"private".to_vec())
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let inputs = json!({ "docs": [doc] });
    let (bob, _pk) = signed_up(&rig.c, 1, "bob").await;

    let e = bob
        .start_pipeline_run(&public, "chain", inputs.clone(), json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    let run = rig
        .alice
        .start_pipeline_run(&public, "chain", inputs, json!({}))
        .await
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = bob.cancel_pipeline_run(&public, &run).await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let seen = bob.pipeline_run(&public, &run).await.unwrap();
    assert!(seen["steps"]["c"]["result"].is_null(), "{seen}");
    let _c = rig.alice.cancel_pipeline_run(&public, &run).await.unwrap();
    let mut waited = 0;
    while bob.pipeline_run(&public, &run).await.unwrap()["state"] == "running" {
        waited += 1;
        assert!(waited < 300, "the cancelled run never ended");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let e = bob
        .retry_pipeline_run(&public, &run, json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cancel_stops_a_running_pipeline() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.start_run("slow", json!({}), json!({})).await;
    rig.until("step a is assigned", async || {
        let v = rig.run(&run).await;
        v["steps"]["a"]["runs"]
            .as_array()
            .and_then(|r| r.first())
            .and_then(Value::as_str)
            .is_some_and(|id| !rig.record(id).attempts.is_empty())
    })
    .await;
    let _c = rig.alice.cancel_pipeline_run(&rig.app, &run).await.unwrap();
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "cancelled", "{done}");
    assert_eq!(done["steps"]["a"]["status"], "failed", "{done}");
    assert_eq!(done["steps"]["a"]["reason"], "cancelled", "{done}");
    assert_eq!(done["steps"]["b"]["status"], "cancelled", "{done}");
    assert_eq!(rig.all_records().len(), 1, "step b never started");
    // A cancel repeated after the run ended (a retry whose first answer was lost) still succeeds.
    let _again = rig.alice.cancel_pipeline_run(&rig.app, &run).await.unwrap();
    assert_eq!(rig.run(&run).await["state"], "cancelled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_started_on_v1_keeps_running_v1_after_an_upgrade_to_v2() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let v2 = calimero_app_build::with_abi(&rig.compute, &{
        let mut abi = calimero_app_build::abi_of(&rig.compute).unwrap().unwrap();
        abi["build"] = json!(2);
        abi
    })
    .unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let _installed = rig
        .alice
        .upload_package(mpk_with_pipelines(
            "com.test.job",
            "2.0.0",
            None,
            Some(&v2),
            &[("pipelines/chain.yaml", CHAIN)],
            &key,
        ))
        .await
        .unwrap();
    let file = rig.upload(&vec![5u8; INPUT_LEN]).await;
    let run = rig
        .start_run("chain", json!({ "docs": [file] }), json!({}))
        .await;
    let up = rig
        .alice
        .upgrade_app(&rig.app, json!({ "version": "2.0.0" }), &mut rig.pk)
        .await
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    let now = rig.leader().app(&rig.app).await.unwrap();
    assert_eq!(
        now.compute_bundle,
        Some(sha256_hex(&v2)),
        "the app runs v2 now"
    );
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["version"], "1.0.0");
    let v1 = sha256_hex(&rig.compute);
    assert_eq!(done["computeBundle"], json!(v1));
    for step in ["a", "b", "c"] {
        let record = rig.record(&step_run(&done, step));
        assert_eq!(
            record.compute_bundle.as_deref(),
            Some(v1.as_str()),
            "step {step}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn starting_needs_every_node_on_this_release() {
    let mut c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = false).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let compute = common::components("compute")
        .compute
        .expect("a compute component");
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let _installed = alice
        .upload_package(mpk_with_pipelines(
            "com.test.job",
            "1.0.0",
            None,
            Some(&compute),
            &[("pipelines/chain.yaml", CHAIN)],
            &key,
        ))
        .await
        .unwrap();
    let app = alice
        .create_app_from_package("job", "com.test.job", Some("1.0.0"), None, json!({}))
        .await
        .unwrap();
    let file = loop {
        match alice.upload_bytes(&app, b"bytes".to_vec()).await {
            Ok(up) => break up["id"].as_str().unwrap().to_owned(),
            Err(e) if e.code() == Some("FILES_UNAVAILABLE") => {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Err(e) => panic!("upload failed: {e}"),
        }
    };
    // Node 3 falls behind the release the other nodes run.
    c.restart(2, |cfg| cfg.release = "0.9.0".into()).await;
    let started = Instant::now();
    let mut began = 0;
    let refused = loop {
        let tried = alice
            .start_pipeline_run(&app, "chain", json!({ "docs": [file] }), json!({}))
            .await;
        match tried {
            Err(e) => break e,
            Ok(_) => began += 1,
        }
        assert!(started.elapsed() < Duration::from_secs(60), "never refused");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert_eq!(refused.code(), Some("CLUSTER_NOT_UPGRADED"), "{refused}");
    let listed = alice.pipeline_runs(&app).await.unwrap();
    assert_eq!(
        listed["runs"].as_array().unwrap().len(),
        began,
        "the refusal logged nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_start_is_refused_with_a_stable_code() {
    let rig = Rig::start(&[]).await;
    let file = rig.upload(b"some bytes").await;
    let rig = &rig;
    let code =
        |r: Result<Value, Box<calimero_client::Error>>| r.unwrap_err().code().map(str::to_owned);
    let start = |name: &'static str, inputs: Value, params: Value| async move {
        rig.alice
            .start_pipeline_run(&rig.app, name, inputs, params)
            .await
            .map_err(Box::new)
    };
    let docs = json!({ "docs": [file] });
    assert_eq!(
        code(start("ghost", docs.clone(), json!({})).await),
        Some("NO_SUCH_PIPELINE".into())
    );
    assert_eq!(
        code(start("broken", json!({}), json!({})).await),
        Some("BAD_INPUTS".into()),
        "an input left unbound"
    );
    assert_eq!(
        code(start("broken", json!({ "docs": [file, file, file] }), json!({})).await),
        Some("BAD_INPUTS".into()),
        "past maxItems"
    );
    assert_eq!(
        code(start("broken", json!({ "docs": ["ab".repeat(32)] }), json!({})).await),
        Some("OUTPUT_EXPIRED".into()),
        "a file the app does not hold"
    );
    assert_eq!(
        code(start("broken", docs.clone(), json!({ "rounds": 9 })).await),
        Some("BAD_PARAMS".into())
    );
    assert_eq!(
        code(start("broken", docs.clone(), json!({ "other": 1 })).await),
        Some("BAD_PARAMS".into())
    );
    // With no job node, runs stay running: the app holds only so many at once.
    for _ in 0..MAX_ACTIVE_PIPELINE_RUNS {
        let _run = rig.start_run("broken", docs.clone(), json!({})).await;
    }
    assert_eq!(
        code(start("broken", docs, json!({})).await),
        Some("TOO_MANY_PIPELINE_RUNS".into())
    );
}

fn cached(v: &Value, step: &str) -> bool {
    v["steps"][step]["cached"] == json!(true)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn rerunning_with_one_changed_param_runs_only_the_steps_it_reaches() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(b"some document").await;
    let docs = json!({ "docs": [file] });
    let first = rig
        .start_run("partial", docs.clone(), json!({ "p": "one" }))
        .await;
    let first = rig.ended(&first).await;
    assert_eq!(first["state"], "succeeded", "{first}");
    assert!(!cached(&first, "a") && !cached(&first, "b"), "{first}");
    assert_eq!(rig.all_records().len(), 3);

    let second = rig
        .start_run("partial", docs.clone(), json!({ "p": "two" }))
        .await;
    let second = rig.ended(&second).await;
    assert_eq!(second["state"], "succeeded", "{second}");
    assert!(cached(&second, "a"), "a does not read p: {second}");
    assert!(!cached(&second, "b") && !cached(&second, "c"), "{second}");
    assert_eq!(rig.all_records().len(), 5, "only b and c ran");
    let a_run = step_runs(&second, "a");
    assert!(
        rig.leader()
            .hosted(&rig.app)
            .unwrap()
            .read_local(|m, _| m.run_record(&a_run[0]))
            .is_none(),
        "a cached step has no run record, so no usage"
    );
    assert_eq!(output_ids(&second, "a"), output_ids(&first, "a"));

    let third = rig.start_run("partial", docs, json!({ "p": "one" })).await;
    let third = rig.ended(&third).await;
    assert_eq!(third["state"], "succeeded", "{third}");
    for step in ["a", "b", "c"] {
        assert!(cached(&third, step), "{step}: {third}");
    }
    assert_eq!(rig.all_records().len(), 5, "no run record at all");
    assert_eq!(third["steps"]["c"]["result"], first["steps"]["c"]["result"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_step_with_cache_false_always_runs() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(b"some document").await;
    for n in 1..=2 {
        let run = rig
            .start_run("fresh", json!({ "docs": [file] }), json!({}))
            .await;
        let done = rig.ended(&run).await;
        assert_eq!(done["state"], "succeeded", "{done}");
        assert!(!cached(&done, "a"), "{done}");
        assert_eq!(rig.all_records().len(), n, "run {n} ran the step");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_fan_out_reuses_its_cached_items() {
    let rig = Rig::start(&[JOB_A]).await;
    let files = rig
        .uploads(&["item one".to_owned(), "item two".to_owned()])
        .await;
    let first = rig
        .start_run("small", json!({ "docs": files }), json!({}))
        .await;
    let first = rig.ended(&first).await;
    assert_eq!(first["state"], "succeeded", "{first}");
    assert_eq!(first["steps"]["each"]["cachedItems"], 0);
    assert_eq!(rig.all_records().len(), 2);
    let again = rig
        .start_run("small", json!({ "docs": files }), json!({}))
        .await;
    let again = rig.ended(&again).await;
    assert_eq!(again["state"], "succeeded", "{again}");
    assert!(cached(&again, "each"), "{again}");
    assert_eq!(again["steps"]["each"]["cachedItems"], 2);
    assert_eq!(rig.rest_of(&again, "each"), rig.rest_of(&first, "each"));
    assert_eq!(rig.all_records().len(), 2, "no new run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn starting_with_an_expired_output_answers_output_expired() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(b"will be forgotten").await;
    rig.c
        .advance_all(i64::try_from(BLOB_IDLE_MS).unwrap() + 1000);
    // Any later entry expires what nothing refers to.
    let _other = rig.upload(b"a later file").await;
    let refused = rig
        .alice
        .start_pipeline_run(&rig.app, "fresh", json!({ "docs": [file] }), json!({}))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("OUTPUT_EXPIRED"), "{refused}");
}

/// The operator's prices, signed as mdma does: `check` then costs 2020 per run at worst.
async fn set_prices(rig: &Rig) {
    let ops = rig.c.client(0);
    ops.use_api_key("ops-secret").await;
    let table = json!({ "table": {
        "version": 1, "fuelPerVcpuSecond": 1_000_000_000, "vcpuSecond": 100, "gibSecond": 10,
        "connectorGib": 0, "storageGibMonth": 0, "egressGib": 0,
    } });
    common::ops_signed(&ops, "ops/prices", table).await.unwrap();
}

const CHECK_RUN_COST: u64 = 2020; // 20 vCPU-seconds at 100 and 2 GiB-seconds at 10

async fn start_capped(rig: &Rig, files: &[String], max_cost: u64) -> Value {
    let body = json!({ "inputs": { "docs": files }, "maxCost": max_cost.to_string() });
    rig.alice
        .start_pipeline_run_with(&rig.app, "costed", body)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn max_cost_without_a_price_table_is_refused() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(b"one").await;
    let body = json!({ "inputs": { "docs": [file] }, "maxCost": "1000" });
    let err = rig
        .alice
        .start_pipeline_run_with(&rig.app, "costed", body)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("NO_PRICE_TABLE"), "{err}");
    assert!(matches!(
        err,
        calimero_client::Error::Api { status: 409, .. }
    ));
    assert!(rig.alice.pipeline_runs(&rig.app).await.unwrap()["runs"]
        .as_array()
        .unwrap()
        .is_empty());
    let body = json!({ "inputs": { "docs": [file] }, "maxCost": "many" });
    set_prices(&rig).await;
    let err = rig
        .alice
        .start_pipeline_run_with(&rig.app, "costed", body)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("BAD_MAX_COST"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_run_stops_admitting_before_it_crosses_its_cost_cap() {
    let rig = Rig::start(&[JOB_A]).await;
    set_prices(&rig).await;
    let mut files = Vec::new();
    for i in 0..6 {
        files.push(rig.upload(format!("document {i}").as_bytes()).await);
    }
    // Below one run's worst case the step starts nothing and fails with the cap.
    let probe = start_capped(&rig, &files, 1).await;
    assert_eq!(probe["worstCase"], json!((20 * CHECK_RUN_COST).to_string()));
    let none = rig.ended(probe["run"].as_str().unwrap()).await;
    assert_eq!(none["state"], "failed", "{none}");
    assert_eq!(none["steps"]["each"]["reason"], "COST_CAP", "{none}");
    assert_eq!(none["steps"]["each"]["runs"], json!([]), "{none}");
    assert_eq!(none["costSoFar"], "0", "{none}");

    // Three worst cases fit, six items need more: exactly three runs are created.
    let max_cost = 3 * CHECK_RUN_COST;
    let started = start_capped(&rig, &files, max_cost).await;
    let run = started["run"].as_str().unwrap();
    let done = rig.ended(run).await;
    assert_eq!(done["state"], "failed", "{done}");
    assert_eq!(done["steps"]["each"]["status"], "failed", "{done}");
    assert_eq!(done["steps"]["each"]["reason"], "COST_CAP", "{done}");
    assert_eq!(done["steps"]["each"]["runs"].as_array().unwrap().len(), 3);
    let linked = |r: &RunRecord| r.pipeline.as_ref().is_some_and(|l| l.run == run);
    assert_eq!(rig.all_records().iter().filter(|r| linked(r)).count(), 3);
    let so_far: u64 = done["costSoFar"].as_str().unwrap().parse().unwrap();
    assert!(so_far > 0 && so_far <= max_cost, "{done}");
    assert_eq!(done["cost"]["maxCost"], json!(max_cost.to_string()));
    assert_eq!(done["cost"]["priceVersion"], 1);
    assert_eq!(done["steps"]["each"]["cost"], done["costSoFar"], "{done}");
}

/// The compute runs a pipeline run made, by item index.
fn runs_of(rig: &Rig, run: &str) -> Vec<(u32, RunRecord)> {
    let mut runs: Vec<(u32, RunRecord)> = rig
        .all_records()
        .into_iter()
        .filter_map(|r| {
            let link = r.pipeline.clone()?;
            (link.run == run).then_some((link.index, r))
        })
        .collect();
    runs.sort_by_key(|(i, _)| *i);
    runs
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_retry_with_raised_limits_reruns_only_the_failed_items() {
    let rig = Rig::start(&[JOB_A]).await;
    let mut files = Vec::new();
    for i in 0..4 {
        files.push(rig.upload(format!("small {i}").as_bytes()).await);
    }
    for i in 1..=2u8 {
        files.push(rig.upload(&vec![i; 1024 * 1024]).await);
    }
    // 50M of fuel is plenty for a few bytes and short of a mebibyte (about 100M).
    let first = rig
        .start_run("raise", json!({ "docs": files }), json!({}))
        .await;
    let done = rig.ended(&first).await;
    assert_eq!(done["state"], "succeededWithFailures", "{done}");
    assert_eq!(done["steps"]["each"]["failedItems"], 2, "{done}");
    let made = runs_of(&rig, &first);
    assert_eq!(made.len(), 6);
    let failed: Vec<u32> = made
        .iter()
        .filter(|(_, r)| {
            matches!(
                r.final_.as_ref().map(|f| f.outcome),
                Some(FinalOutcome::Failed { .. })
            )
        })
        .map(|(i, _)| *i)
        .collect();
    assert_eq!(failed, [4, 5]);

    let retried = rig
        .alice
        .retry_pipeline_run(
            &rig.app,
            &first,
            json!({ "limits": { "each": { "fuel": 400_000_000u64 } } }),
        )
        .await
        .unwrap();
    assert_eq!(retried["retryOf"], json!(first));
    let second = retried["run"].as_str().unwrap().to_owned();
    let done = rig.ended(&second).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["retryOf"], json!(first), "{done}");
    assert_eq!(done["steps"]["each"]["cachedItems"], 4, "{done}");
    let again: Vec<u32> = runs_of(&rig, &second).iter().map(|(i, _)| *i).collect();
    assert_eq!(again, [4, 5], "only the failed items ran again");
    assert_eq!(
        done["steps"]["each"]["outputs"].as_array().unwrap().len(),
        6
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_fuel_failure_is_not_retried_automatically() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&vec![3u8; 1024 * 1024]).await;
    let run = rig
        .start_run("strict", json!({ "docs": [file] }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "failed", "{done}");
    let record = rig.record(&step_run(&done, "a"));
    assert_eq!(record.attempts.len(), 1, "one attempt");
    assert_eq!(
        record.final_.as_ref().map(|f| f.outcome),
        Some(FinalOutcome::Failed {
            reason: FailedReason::Fuel
        })
    );
    assert_eq!(runs_of(&rig, &run).len(), 1, "and no second run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_lost_item_is_retried_and_billed_once() {
    let mut rig = Rig::start(&[JOB_A, JOB_B]).await;
    set_prices(&rig).await;
    let file = rig.upload(&vec![5u8; INPUT_LEN]).await;
    let body = json!({ "inputs": { "docs": [file] }, "maxCost": "1000000000" });
    let started = rig
        .alice
        .start_pipeline_run_with(&rig.app, "metered", body)
        .await
        .unwrap();
    let run = started["run"].as_str().unwrap().to_owned();
    let item = format!("{run}.a.0");
    rig.until("an attempt is assigned", async || {
        let records = rig.all_records();
        records
            .iter()
            .any(|r| r.run == item && !r.attempts.is_empty())
    })
    .await;
    let first = rig.record(&item).attempts[0].node;
    let at = rig.jobs.iter().position(|j| j.id == first).unwrap();
    rig.jobs.remove(at).running.stop().await;

    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    let record = rig.record(&item);
    assert_eq!(
        record.attempts.len(),
        2,
        "a second attempt on the other node"
    );
    assert_ne!(record.attempts[1].node, first);
    assert_eq!(runs_of(&rig, &run).len(), 1, "one run for the item");
    let usage = record.final_.as_ref().unwrap().usage;
    let table = calimero_pipeline::PriceTable {
        version: 1,
        fuel_per_vcpu_second: 1_000_000_000,
        vcpu_second: 100,
        gib_second: 10,
        connector_gib: 0,
        storage_gib_month: 0,
        egress_gib: 0,
    };
    let caps = calimero_pipeline::Caps {
        fuel: record.caps.fuel,
        memory_mib: record.caps.memory_mib,
        wall_secs: record.caps.wall_secs,
    };
    let once = calimero_pipeline::priced_cost(
        usage.fuel,
        usage.gib_micros,
        usage.cpu_micros,
        caps,
        &table,
    );
    assert!(once > 0);
    assert_eq!(
        done["costSoFar"],
        json!(once.to_string()),
        "billed once: {done}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn retrying_a_running_run_is_refused() {
    let rig = Rig::start(&[JOB_A]).await;
    let run = rig.start_run("slow", json!({}), json!({})).await;
    let err = rig
        .alice
        .retry_pipeline_run(&rig.app, &run, json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("STILL_RUNNING"), "{err}");
    assert!(matches!(
        err,
        calimero_client::Error::Api { status: 409, .. }
    ));
    let _ok = rig.alice.cancel_pipeline_run(&rig.app, &run).await.unwrap();
    let _ended = rig.ended(&run).await;
    let err = rig
        .alice
        .retry_pipeline_run(&rig.app, "nope", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, calimero_client::Error::Api { status: 404, .. }),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn limits_a_retry_may_not_take_are_refused() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&vec![3u8; 1024 * 1024]).await;
    let run = rig
        .start_run("strict", json!({ "docs": [file] }), json!({}))
        .await;
    let _done = rig.ended(&run).await;
    for (limits, code) in [
        (
            json!({ "a": { "fuel": 20_000_000_001u64 } }),
            "LIMIT_EXCEEDS_CAP",
        ),
        (json!({ "a": { "wallSecs": 61 } }), "LIMIT_EXCEEDS_CAP"),
        (json!({ "zz": { "fuel": 1 } }), "BAD_LIMITS"),
        (json!({ "a": { "fuel": 0 } }), "BAD_LIMITS"),
    ] {
        let err = rig
            .alice
            .retry_pipeline_run(&rig.app, &run, json!({ "limits": limits }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(code), "{limits}: {err}");
        assert!(matches!(
            err,
            calimero_client::Error::Api { status: 400, .. }
        ));
    }
    assert_eq!(
        rig.alice.pipeline_runs(&rig.app).await.unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_retry_keeps_the_original_release_after_an_upgrade() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(&vec![5u8; 4096]).await;
    let first = rig
        .start_run("chain", json!({ "docs": [file] }), json!({}))
        .await;
    let original = rig.ended(&first).await;
    assert_eq!(original["state"], "succeeded", "{original}");

    let pipelines = [("pipelines/chain.yaml", CHAIN_V2)];
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let mpk = mpk_with_pipelines(
        "com.test.job",
        "1.0.1",
        None,
        Some(&rig.compute),
        &pipelines,
        &key,
    );
    rig.alice.upload_package(mpk).await.unwrap();
    rig.alice
        .upgrade_app(&rig.app, json!({ "version": "1.0.1" }), &mut rig.pk)
        .await
        .unwrap();
    let fresh = rig
        .start_run("chain", json!({ "docs": [file] }), json!({}))
        .await;
    let now = rig.ended(&fresh).await;
    assert_eq!(
        now["version"], "1.0.1",
        "a new start uses the upgraded release: {now}"
    );
    assert!(now["steps"].get("d").is_some(), "{now}");

    let retried = rig
        .alice
        .retry_pipeline_run(&rig.app, &first, json!({}))
        .await
        .unwrap();
    let second = retried["run"].as_str().unwrap();
    let done = rig.ended(second).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["version"], "1.0.0", "{done}");
    assert_eq!(done["hash"], original["hash"], "{done}");
    assert!(
        done["steps"].get("d").is_none(),
        "the old definition: {done}"
    );
    assert_eq!(
        done["steps"]["a"]["cached"], true,
        "nothing ran again: {done}"
    );
}

const VIA_MODULE: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: via-module
inputs:
  docs: { kind: files }
steps:
  a: { module: com.test.module@1.0.0, method: digest, inputs: [inputs.docs] }
  b: { method: digest, inputs: [a.outputs] }
";

/// A step runs the code of another installed package, while the files, the payer and the usage
/// stay the calling app's: the module only supplies the component.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_step_runs_another_packages_module_under_the_callers_keys_and_payer() {
    let mut rig = Rig::start(&[JOB_A]).await;
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let module = calimero_app_build::with_abi(&rig.compute, &{
        let mut abi = calimero_app_build::abi_of(&rig.compute).unwrap().unwrap();
        abi["build"] = json!(7);
        abi
    })
    .unwrap();
    let _module = rig
        .alice
        .upload_package(common::mpk_parts(
            "com.test.module",
            "1.0.0",
            None,
            Some(&module),
            &key,
        ))
        .await
        .unwrap();
    let _user = rig
        .alice
        .upload_package(mpk_with_pipelines(
            "com.test.job",
            "2.0.0",
            None,
            Some(&rig.compute),
            &[("pipelines/via-module.yaml", VIA_MODULE)],
            &key,
        ))
        .await
        .unwrap();
    let _up = rig
        .alice
        .upgrade_app(&rig.app, json!({ "version": "2.0.0" }), &mut rig.pk)
        .await
        .unwrap();

    let file = rig.upload(&vec![5u8; 4096]).await;
    let run = rig
        .start_run("via-module", json!({ "docs": [file] }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(
        done["modules"],
        json!({ "com.test.module@1.0.0": sha256_hex(&module) }),
        "{done}"
    );

    let (own, theirs) = (sha256_hex(&rig.compute), sha256_hex(&module));
    let a = rig.record(&step_run(&done, "a"));
    let b = rig.record(&step_run(&done, "b"));
    assert_eq!(a.compute_bundle.as_deref(), Some(theirs.as_str()));
    assert_eq!(b.compute_bundle.as_deref(), Some(own.as_str()));
    // Billed, owned and sealed as the app's: the module's package never appears in the record.
    assert_eq!(
        (a.bundle.as_str(), a.owner.as_str()),
        (own.as_str(), b.owner.as_str())
    );
    assert_eq!(
        done["payer"],
        json!(format!("account:{}", a.owner)),
        "the app's payer, never the module package's: {done}"
    );

    let outputs = done["steps"]["a"]["outputs"].as_array().unwrap();
    assert!(!outputs.is_empty(), "{done}");
    for output in outputs {
        let id = output["id"].as_str().unwrap();
        let held = rig
            .leader()
            .hosted(&rig.app)
            .unwrap()
            .read_local(|m, _| m.file_sha256(id));
        assert!(
            held.is_some(),
            "the module's output {id} is a file of the app"
        );
    }

    // A retry keeps the pinned module.
    let retried = rig
        .alice
        .retry_pipeline_run(&rig.app, &run, json!({}))
        .await
        .unwrap();
    let again = rig.ended(retried["run"].as_str().unwrap()).await;
    assert_eq!(again["state"], "succeeded", "{again}");
    assert_eq!(again["modules"], done["modules"], "{again}");
}

async fn two_pools(rig: &mut Rig, extra: &[(u64, &str, Option<&str>)]) {
    let pools = json!({ "pools": {
        "standard": { "runsPerNode": 2, "maxMemoryMib": 2048 },
        "big": { "runsPerNode": 2, "maxMemoryMib": 4096 },
    } });
    let ops = rig.c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = common::ops_signed(&ops, "ops/pools", pools).await.unwrap();
    let mut nodes = vec![(JOB_A, "standard", None), (JOB_B, "big", None)];
    nodes.extend_from_slice(extra);
    for (id, pool, release) in nodes {
        let job = rig
            .c
            .start_job_node(id, |cfg| {
                if let Some(release) = release {
                    cfg.release = release.into();
                }
            })
            .await;
        rig.c.register_job_node_in(id, &job.url, Some(pool)).await;
        rig.jobs.push(job);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_steps_pool_places_its_runs_on_that_pools_job_node() {
    let mut rig = Rig::start_with(&[], |cfg| cfg.release = "1.0.0".into()).await;
    two_pools(&mut rig, &[]).await;

    let file = rig.upload(&vec![7u8; 4096]).await;
    let run = rig
        .start_run("placed", json!({ "docs": [file] }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    for (step, node, pool) in [("named", JOB_B, Some("big")), ("plain", JOB_A, None)] {
        let record = rig.record(&step_run(&done, step));
        let placed = record.attempts.last().map(|a| a.node);
        assert_eq!(placed, Some(node), "{step}: ran on {placed:?}");
        assert_eq!(record.pool.as_deref(), pool, "{step}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_named_pool_waits_until_every_node_runs_this_release() {
    let mut rig = Rig::start_with(&[], |cfg| cfg.release = "1.0.0".into()).await;
    two_pools(&mut rig, &[(6, "standard", Some("0.9.0"))]).await;
    let file = rig.upload(&vec![7u8; 4096]).await;
    let run = rig
        .start_run("placed", json!({ "docs": [file] }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "succeeded", "{done}");
    let record = rig.record(&step_run(&done, "named"));
    assert_eq!(record.pool.as_deref(), Some("big"));
    let placed = record.attempts.last().map(|a| a.node);
    assert_ne!(
        placed,
        Some(JOB_B),
        "placed by memory, not by the named pool"
    );
}
