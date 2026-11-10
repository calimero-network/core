//! Pipeline runs driven by an app's leader through real job nodes: each step's work is an
//! ordinary compute run, placed, retried and ended by the run driver, and the pipeline loop only
//! decides which step starts when. The package is the `compute` fixture's compute component
//! alone (a compute-only app).

mod common;
use common::UploadBytes as _;

use std::time::{Duration, Instant};

use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::component::{RunRecord, MAX_ACTIVE_PIPELINE_RUNS};
use calimero_node::Node;
use common::{mpk_with_pipelines, signed_up, Cluster, JobNode, PUBLISHER};
use serde_json::{json, Value};

const JOB_A: u64 = 4;
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

const FAN: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: fan
inputs:
  docs: { kind: files }
steps:
  each: { method: digest, forEach: inputs.docs, maxItems: 4 }
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
    _jobs: Vec<JobNode>,
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
            ("pipelines/fan.yaml", FAN),
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
            _jobs: jobs,
        }
    }

    fn leader(&self) -> &Node {
        &self.c.nodes[self.c.app_leader(&self.app).unwrap()]
            .as_ref()
            .unwrap()
            .node
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_step_that_fans_out_fails_as_not_yet_supported() {
    let rig = Rig::start(&[JOB_A]).await;
    let file = rig.upload(b"some bytes").await;
    let run = rig
        .start_run("fan", json!({ "docs": [file] }), json!({}))
        .await;
    let done = rig.ended(&run).await;
    assert_eq!(done["state"], "failed", "{done}");
    assert_eq!(done["steps"]["each"]["reason"], "NOT_YET_SUPPORTED");
    assert!(rig.all_records().is_empty());
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
    let again = rig
        .alice
        .cancel_pipeline_run(&rig.app, &run)
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some("NOT_FOUND"), "{again}");
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
        Some("BAD_INPUTS".into()),
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
