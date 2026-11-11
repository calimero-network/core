//! The leader of a component app drives its pipeline runs: for each running run it decides every
//! pending step whose turn has come and logs the decision (`pipelineStep`). A step's work is
//! ordinary compute runs, which the loop in the parent module places, retries and ends.
//!
//! The loop keeps nothing but caches of immutable data (a pipeline by its hash, a compute
//! component's caps): every round reads the replicated records afresh and proposes only what is
//! still pending, and apply lets the first decision for a step win, so a change of leader mid-run
//! never runs a step twice.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use calimero_app_host as host;
use calimero_log::{Bytes, Group, GroupId, Proposal, WriteError};
use calimero_pipeline::{parse_ref, Caps, Pipeline, Ref, Step};
use serde_json::{json, Map, Value};
use tracing::debug;

use super::{locked, Driving};
use crate::app::{AppMachine, Status};
use crate::component::{
    ComponentCmd, PipelineCancelEntry, PipelineRun, PipelineStartEntry, PipelineState, RunCaps,
    StepDecisionEntry, StepRun, StepStatus, StepVerdict,
};
use crate::error::ApiError;
use crate::node::component::entry_bytes;
use crate::node::{off_workers, Node, ViewAnswer, ViewRequest};
use crate::types::{AppRecord, Caller};

const DEFAULT_PAGE: usize = 20; // pipeline runs a list answers when the caller names no limit
const MAX_PAGE: usize = 100; // the most pipeline runs a list answers
const MAX_CACHED: usize = 64; // pipelines and compute components kept in memory, by hash

/// The shared-memory state of the loop: what is being driven, and what it has looked up.
#[derive(Default)]
pub(super) struct PipelineCaches {
    pub(super) driving: std::sync::Mutex<HashSet<GroupId>>,
    pipelines: std::sync::Mutex<HashMap<String, Arc<Pipeline>>>,
    caps: std::sync::Mutex<HashMap<String, Arc<BTreeMap<String, Caps>>>>,
}

fn remember<T>(cache: &std::sync::Mutex<HashMap<String, Arc<T>>>, key: &str, value: Arc<T>) {
    let mut cache = locked(cache);
    if cache.len() >= MAX_CACHED {
        cache.clear();
    }
    let _prev = cache.insert(key.to_owned(), value);
}

/// What each pending step does now: it starts once its needs succeeded or were skipped, fails
/// upstream when one did not, and a cancelled run ends it. `room` is the runs the app can take.
pub(super) fn plan(
    pipeline: &Pipeline,
    run: &PipelineRun,
    caps: &BTreeMap<String, Caps>,
    mut room: usize,
) -> Vec<(String, StepVerdict)> {
    let mut status: BTreeMap<&str, StepStatus> = run
        .steps
        .iter()
        .map(|(name, step)| (name.as_str(), step.status))
        .collect();
    let mut decided = Vec::new();
    loop {
        let before = decided.len();
        for (name, step) in &pipeline.steps {
            if status.get(name.as_str()) != Some(&StepStatus::Pending) {
                continue;
            }
            let needs: Vec<StepStatus> = step
                .needs
                .iter()
                .filter_map(|n| status.get(n.as_str()).copied())
                .collect();
            let broken = needs.iter().any(|s| {
                matches!(
                    s,
                    StepStatus::Failed | StepStatus::UpstreamFailed | StepStatus::Cancelled
                )
            });
            let ready = needs.len() == step.needs.len() && needs.iter().all(|s| s.is_ok());
            let verdict = if run.cancel_requested {
                Some(StepVerdict::Cancelled)
            } else if broken {
                Some(StepVerdict::UpstreamFailed)
            } else if ready {
                match build_run(pipeline, run, name, step, caps) {
                    Err(reason) => Some(StepVerdict::Failed {
                        reason: reason.to_owned(),
                    }),
                    Ok(one) if room > 0 => {
                        room -= 1;
                        Some(StepVerdict::Start { runs: vec![one] })
                    }
                    Ok(_) => None,
                }
            } else {
                None
            };
            let Some(verdict) = verdict else { continue };
            let after = match &verdict {
                StepVerdict::Start { .. } => StepStatus::Running,
                StepVerdict::Skipped => StepStatus::Skipped,
                StepVerdict::UpstreamFailed => StepStatus::UpstreamFailed,
                StepVerdict::Cancelled => StepStatus::Cancelled,
                StepVerdict::Failed { .. } => StepStatus::Failed,
            };
            let _prev = status.insert(name.as_str(), after);
            decided.push((name.clone(), verdict));
        }
        if decided.len() == before {
            return decided;
        }
    }
}

/// The compute run of a single-run step: caps lowered by its limits, the files its `inputs` name
/// and its `args` with references filled in. A refusal is a stable code that fails the step.
fn build_run(
    pipeline: &Pipeline,
    run: &PipelineRun,
    name: &str,
    step: &Step,
    caps: &BTreeMap<String, Caps>,
) -> Result<StepRun, &'static str> {
    if step.for_each.is_some() || step.collect.is_some() {
        return Err("NOT_YET_SUPPORTED");
    }
    let cap = caps.get(&step.method).ok_or("UNKNOWN_METHOD")?;
    let limits = step.limits.unwrap_or_default();
    let caps = RunCaps {
        fuel: limits.fuel.map_or(cap.fuel, |v| v.min(cap.fuel)),
        memory_mib: limits
            .memory_mib
            .map_or(cap.memory_mib, |v| v.min(cap.memory_mib)),
        wall_secs: limits
            .wall_secs
            .map_or(cap.wall_secs, |v| v.min(cap.wall_secs)),
    };
    let mut inputs: Vec<String> = Vec::new();
    for reference in &step.inputs {
        let named: Vec<String> = match parse_ref(reference) {
            Some(Ref::Input(n)) => run.inputs.get(n).cloned().unwrap_or_default(),
            Some(Ref::Step(s, "outputs")) => run
                .steps
                .get(s)
                .map(|s| s.outputs.iter().map(|o| o.id.clone()).collect())
                .unwrap_or_default(),
            _ => return Err("BAD_REF"),
        };
        for id in named {
            if !inputs.contains(&id) {
                inputs.push(id);
            }
        }
    }
    if inputs.len() > host::MAX_RUN_INPUTS {
        return Err("TOO_MANY_INPUTS");
    }
    let args = step
        .args
        .iter()
        .map(|(k, v)| Ok((k.clone(), resolve(pipeline, run, v)?)))
        .collect::<Result<Map<String, Value>, &'static str>>()?;
    // No args is no bytes, as `compute::start` sends for `()`.
    let args = if args.is_empty() {
        Vec::new()
    } else {
        serde_json::to_vec(&args).map_err(|_| "BAD_REF")?
    };
    if args.len() > host::MAX_COMPUTE_ARGS_BYTES {
        return Err("ARGS_TOO_LARGE");
    }
    Ok(StepRun {
        run: format!("{}.{name}.0", run.id),
        method: step.method.clone(),
        args: Bytes(args),
        inputs,
        caps,
    })
}

/// `v` with each reference replaced: a param by the run's value or the param's default, a step's
/// `result` by that step's result read as JSON.
fn resolve(pipeline: &Pipeline, run: &PipelineRun, v: &Value) -> Result<Value, &'static str> {
    match v {
        Value::String(s) => match parse_ref(s) {
            None => Ok(v.clone()),
            Some(Ref::Param(n)) => run
                .params
                .get(n)
                .cloned()
                .or_else(|| pipeline.params.get(n).and_then(|p| p.default.clone()))
                .ok_or("BAD_REF"),
            Some(Ref::Step(step, "result")) => {
                let result = run
                    .steps
                    .get(step)
                    .and_then(|s| s.result.as_ref())
                    .ok_or("NO_RESULT")?;
                serde_json::from_slice(&result.0).map_err(|_| "BAD_RESULT")
            }
            Some(_) => Err("BAD_REF"),
        },
        Value::Array(items) => items
            .iter()
            .map(|x| resolve(pipeline, run, x))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => map
            .iter()
            .map(|(k, x)| Ok((k.clone(), resolve(pipeline, run, x)?)))
            .collect::<Result<Map<_, _>, _>>()
            .map(Value::Object),
        _ => Ok(v.clone()),
    }
}

/// A pipeline run as the API shows it: sizes, hashes and results only to those who may list
/// the app's files, a result as JSON when it is.
fn run_json(run: &PipelineRun, files: bool) -> Value {
    let mut v = serde_json::to_value(run).unwrap_or(Value::Null);
    for (name, step) in &run.steps {
        let Some(shown) = v["steps"][name].as_object_mut() else {
            continue;
        };
        if let Some(outputs) = shown.get_mut("outputs").and_then(Value::as_array_mut) {
            for output in outputs.iter_mut().filter_map(Value::as_object_mut) {
                if !files {
                    let _gone = (output.remove("size"), output.remove("sha256"));
                }
            }
        }
        let result = step
            .result
            .as_ref()
            .filter(|_| files)
            .map(|b| serde_json::from_slice(&b.0).unwrap_or_else(|_| json!(hex::encode(&b.0))));
        let _prev = shown.insert("result".to_owned(), result.unwrap_or(Value::Null));
    }
    v
}

impl Node {
    /// The reserved view of a pipeline run's record, or of a page of them.
    pub(in crate::node) fn pipeline_view(
        &self,
        g: &Group<AppMachine>,
        req: &ViewRequest,
    ) -> Result<ViewAnswer, ApiError> {
        let files = req.args["files"].as_bool().unwrap_or(false);
        let limit = match req.args.get("limit").filter(|l| !l.is_null()) {
            None => DEFAULT_PAGE,
            Some(l) => l
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| (1..=MAX_PAGE).contains(n))
                .ok_or_else(|| {
                    ApiError::bad_request("BAD_LIMIT", &format!("limit is 1 to {MAX_PAGE}"))
                })?,
        };
        let (result, applied) = g.read_local(|m, applied| {
            let result = match req.args["run"].as_str() {
                Some(run) => m
                    .pipeline_run(run)
                    .map_or(Value::Null, |r| run_json(&r, files)),
                None => json!({
                    "runs": m
                        .pipeline_runs_page(limit)
                        .iter()
                        .map(|r| run_json(r, files))
                        .collect::<Vec<_>>(),
                }),
            };
            (result, applied)
        });
        Ok(ViewAnswer {
            index: applied,
            status: Status::Ok,
            result,
            error: Value::Null,
            timeout: false,
        })
    }

    /// One round of pipeline driving, for each component app group this node leads, unless the
    /// previous round for that app is still running.
    pub fn drive_pipelines(self: &Arc<Self>) {
        for (group, g) in self.led_component_groups() {
            if !locked(&self.compute.pipelines.driving).insert(group) {
                continue;
            }
            let node = Arc::clone(self);
            drop(tokio::spawn(async move {
                let _driving = Driving {
                    groups: &node.compute.pipelines.driving,
                    group,
                };
                node.drive_app_pipelines(&g).await;
            }));
        }
    }

    async fn drive_app_pipelines(&self, g: &Group<AppMachine>) {
        if !g.status().is_leader {
            return;
        }
        for id in g.read_local(|m, _| m.active_pipelines()) {
            let running = g
                .read_local(|m, _| m.pipeline_run(&id))
                .filter(|r| r.state == PipelineState::Running);
            if let Some(run) = running {
                self.drive_pipeline(g, &run).await;
            }
        }
    }

    async fn drive_pipeline(&self, g: &Group<AppMachine>, run: &PipelineRun) {
        if !run.steps.values().any(|s| s.status == StepStatus::Pending) {
            return;
        }
        let looked = async {
            let pipeline = self.pipeline_by_hash(&run.hash).await?;
            let caps = self.caps_of_component(&run.compute_bundle).await?;
            Ok::<_, ApiError>((pipeline, caps))
        }
        .await;
        let (pipeline, caps) = match looked {
            Ok(found) => found,
            Err(e) => {
                debug!(run = %run.id, error = %e.message, "pipeline not loaded");
                return;
            }
        };
        let room = g.read_local(|m, _| host::MAX_ACTIVE_RUNS.saturating_sub(m.active_runs().len()));
        for (step, verdict) in plan(&pipeline, run, &caps, room) {
            self.propose_step(g, &run.id, &step, verdict).await;
        }
    }

    /// Logs the decision unless the step is no longer pending; apply judges it again.
    async fn propose_step(
        &self,
        g: &Group<AppMachine>,
        run: &str,
        step: &str,
        decision: StepVerdict,
    ) {
        let proposed = g
            .propose_led(|m, _, _| {
                let pending = m.pipeline_run(run).is_some_and(|p| {
                    p.state == PipelineState::Running
                        && p.steps
                            .get(step)
                            .is_some_and(|s| s.status == StepStatus::Pending)
                });
                if !pending {
                    return Proposal::Reply(Vec::new());
                }
                let mut cmd = ComponentCmd::PipelineStep(Box::new(StepDecisionEntry {
                    run: run.to_owned(),
                    step: step.to_owned(),
                    decision: decision.clone(),
                    ts_ms: self.stamp(m),
                }));
                Proposal::Append(entry_bytes(&mut cmd).0)
            })
            .await;
        if let Err(e) = proposed {
            debug!(run, step, error = %e, "step decision not logged");
        }
    }

    /// A registered pipeline by the hash of its canonical JSON.
    pub(crate) async fn pipeline_by_hash(&self, hash: &str) -> Result<Arc<Pipeline>, ApiError> {
        if let Some(p) = locked(&self.compute.pipelines.pipelines).get(hash) {
            return Ok(Arc::clone(p));
        }
        let bytes = self.pipeline_json(hash).await?;
        let pipeline: Pipeline =
            serde_json::from_slice(&bytes).map_err(|e| ApiError::internal(&e.to_string()))?;
        let pipeline = Arc::new(pipeline);
        remember(
            &self.compute.pipelines.pipelines,
            hash,
            Arc::clone(&pipeline),
        );
        Ok(pipeline)
    }

    /// The compute methods a compute component offers, with their caps, by its bundle hash.
    pub(crate) async fn caps_of_component(
        &self,
        bundle: &str,
    ) -> Result<Arc<BTreeMap<String, Caps>>, ApiError> {
        if let Some(c) = locked(&self.compute.pipelines.caps).get(bundle) {
            return Ok(Arc::clone(c));
        }
        let wasm = self.bundle_wasm(bundle).await?;
        let caps = off_workers(|| {
            use calimero_app_host::Engine as _;
            self.compute_engine()
                .prepare(bundle, &wasm)
                .map(|p| crate::packages::pipelines::compute_caps(&p.abi))
        })
        .map_err(|e| ApiError::internal(&e.to_string()))?;
        let caps = Arc::new(caps);
        remember(&self.compute.pipelines.caps, bundle, Arc::clone(&caps));
        Ok(caps)
    }

    /// Logs the start of a pipeline run on the app's leader, which stamps it with its app clock.
    pub(crate) async fn start_pipeline_run(
        &self,
        app: &AppRecord,
        entry: PipelineStartEntry,
        caller: Caller,
        sys_index: Option<u64>,
    ) -> Result<crate::app::Applied, ApiError> {
        let args = serde_json::to_value(&entry).map_err(|e| ApiError::internal(&e.to_string()))?;
        let req = crate::node::CallRequest {
            sys_index,
            ..crate::node::CallRequest::host(app.app_id.clone(), "__pipeline_start", args, caller)
        };
        let a = self.call(&req).await?;
        self.kick_runs();
        Ok(a)
    }

    /// Logs the cancel of a pipeline run on the app's leader.
    pub(crate) async fn cancel_pipeline_run(
        &self,
        app: &AppRecord,
        run: &str,
        caller: Caller,
        sys_index: Option<u64>,
    ) -> Result<crate::app::Applied, ApiError> {
        let req = crate::node::CallRequest {
            sys_index,
            ..crate::node::CallRequest::host(
                app.app_id.clone(),
                "__pipeline_cancel",
                serde_json::json!({ "run": run, "tsMs": 0 }),
                caller,
            )
        };
        let a = self.call(&req).await?;
        self.kick_runs();
        Ok(a)
    }

    /// The leader's side of `__pipeline_start` and `__pipeline_cancel`: the entry, stamped.
    pub(in crate::node) async fn component_pipeline_cmd(
        &self,
        g: &Group<AppMachine>,
        req: &crate::node::CallRequest,
    ) -> Result<crate::app::Applied, WriteError> {
        let p = g
            .propose_led(|m, _, _| {
                let ts_ms = self.stamp(m);
                let cmd = match req.method.as_str() {
                    "__pipeline_start" => {
                        serde_json::from_value::<PipelineStartEntry>(req.args.clone()).map(|e| {
                            ComponentCmd::PipelineStart(Box::new(PipelineStartEntry { ts_ms, ..e }))
                        })
                    }
                    _ => serde_json::from_value::<PipelineCancelEntry>(req.args.clone()).map(|e| {
                        ComponentCmd::PipelineCancel(Box::new(PipelineCancelEntry { ts_ms, ..e }))
                    }),
                };
                match cmd {
                    Ok(mut cmd) => Proposal::Append(entry_bytes(&mut cmd).0),
                    Err(_) => Proposal::Reply(Vec::new()),
                }
            })
            .await?;
        let a: crate::component::Applied = serde_json::from_slice(&p.output)
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        Ok(super::super::to_host_applied(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{PipelineState, StepState};
    use crate::types::Caller;

    const SOURCE: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: demo
params:
  limit: { type: number, default: 3 }
inputs:
  docs: { kind: files }
steps:
  a: { method: digest, inputs: [inputs.docs], limits: { fuel: 7, wallSecs: 9 } }
  b: { method: echo, inputs: [a.outputs], args: { value: a.result, limit: params.limit, fixed: [1, x] } }
  c: { method: echo, needs: [b] }
  d: { method: echo, needs: [c] }
";

    fn caps() -> BTreeMap<String, Caps> {
        let c = Caps {
            fuel: 100,
            memory_mib: 64,
            wall_secs: 30,
        };
        BTreeMap::from([("digest".to_owned(), c), ("echo".to_owned(), c)])
    }

    fn run_of(pipeline: &Pipeline) -> PipelineRun {
        PipelineRun {
            id: "r".into(),
            pipeline: pipeline.name.clone(),
            version: "1".into(),
            hash: String::new(),
            compute_bundle: "bundle".into(),
            owner: "o".into(),
            inputs: BTreeMap::from([("docs".to_owned(), vec!["f1".to_owned(), "f2".to_owned()])]),
            params: BTreeMap::new(),
            caller: Caller::System,
            payer: "p".into(),
            start_ts_ms: 0,
            state: PipelineState::Running,
            cancel_requested: false,
            steps: pipeline
                .steps
                .keys()
                .map(|k| {
                    (
                        k.clone(),
                        StepState {
                            status: StepStatus::Pending,
                            runs: Vec::new(),
                            outputs: Vec::new(),
                            result: None,
                            reason: None,
                        },
                    )
                })
                .collect(),
            end_ts_ms: None,
        }
    }

    fn set(run: &mut PipelineRun, step: &str, status: StepStatus) {
        run.steps.get_mut(step).unwrap().status = status;
    }

    fn start_of(plan: &[(String, StepVerdict)], step: &str) -> StepRun {
        match plan.iter().find(|(s, _)| s == step) {
            Some((_, StepVerdict::Start { runs })) => runs[0].clone(),
            other => panic!("{step} did not start: {other:?}"),
        }
    }

    fn failure(plan: &[(String, StepVerdict)], step: &str) -> String {
        match plan.iter().find(|(s, _)| s == step) {
            Some((_, StepVerdict::Failed { reason })) => reason.clone(),
            other => panic!("{step} did not fail: {other:?}"),
        }
    }

    #[test]
    fn only_the_steps_whose_needs_are_done_start_and_with_lowered_caps() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let run = run_of(&p);
        let decided = plan(&p, &run, &caps(), 10);
        assert_eq!(decided.len(), 1, "{decided:?}");
        let a = start_of(&decided, "a");
        assert_eq!(a.run, "r.a.0");
        assert_eq!(a.inputs, vec!["f1".to_owned(), "f2".to_owned()]);
        assert_eq!(
            (a.caps.fuel, a.caps.memory_mib, a.caps.wall_secs),
            (7, 64, 9),
            "limits lower the method's caps"
        );
        assert!(a.args.0.is_empty(), "no args is no bytes");
    }

    #[test]
    fn a_step_gets_its_needs_outputs_its_params_and_its_needs_result() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let mut run = run_of(&p);
        set(&mut run, "a", StepStatus::Succeeded);
        let a = run.steps.get_mut("a").unwrap();
        a.outputs = vec![crate::component::RunInputRef {
            id: "out1".into(),
            size: 1,
            sha256: String::new(),
        }];
        a.result = Some(Bytes(b"[\"h1\"]".to_vec()));
        let b = start_of(&plan(&p, &run, &caps(), 10), "b");
        assert_eq!(b.inputs, vec!["out1".to_owned()]);
        let args: Value = serde_json::from_slice(&b.args.0).unwrap();
        assert_eq!(
            args,
            json!({ "value": ["h1"], "limit": 3, "fixed": [1, "x"] })
        );
        run.params.insert("limit".into(), json!(8));
        let b = start_of(&plan(&p, &run, &caps(), 10), "b");
        let args: Value = serde_json::from_slice(&b.args.0).unwrap();
        assert_eq!(args["limit"], json!(8), "the run's value beats the default");
    }

    #[test]
    fn a_failed_step_fails_everything_downstream_in_one_pass() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let mut run = run_of(&p);
        set(&mut run, "a", StepStatus::Failed);
        let decided = plan(&p, &run, &caps(), 10);
        let verdicts: Vec<_> = decided.iter().map(|(s, v)| (s.as_str(), v)).collect();
        assert_eq!(
            verdicts,
            vec![
                ("b", &StepVerdict::UpstreamFailed),
                ("c", &StepVerdict::UpstreamFailed),
                ("d", &StepVerdict::UpstreamFailed),
            ]
        );
    }

    #[test]
    fn a_cancelled_run_ends_every_pending_step_and_starts_none() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let mut run = run_of(&p);
        run.cancel_requested = true;
        set(&mut run, "a", StepStatus::Running);
        let decided = plan(&p, &run, &caps(), 10);
        assert_eq!(decided.len(), 3);
        assert!(decided.iter().all(|(_, v)| *v == StepVerdict::Cancelled));
    }

    #[test]
    fn a_step_waits_while_the_app_has_no_room_and_a_running_one_is_left_alone() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let mut run = run_of(&p);
        assert!(plan(&p, &run, &caps(), 0).is_empty());
        set(&mut run, "a", StepStatus::Running);
        assert!(plan(&p, &run, &caps(), 10).is_empty());
    }

    #[test]
    fn a_step_that_cannot_be_built_fails_with_a_stable_code() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let mut run = run_of(&p);
        run.inputs.insert(
            "docs".into(),
            (0..=host::MAX_RUN_INPUTS)
                .map(|i| format!("f{i}"))
                .collect(),
        );
        assert_eq!(
            failure(&plan(&p, &run, &caps(), 10), "a"),
            "TOO_MANY_INPUTS"
        );

        let mut run = run_of(&p);
        assert_eq!(
            failure(&plan(&p, &run, &BTreeMap::new(), 10), "a"),
            "UNKNOWN_METHOD"
        );

        set(&mut run, "a", StepStatus::Succeeded);
        assert_eq!(failure(&plan(&p, &run, &caps(), 10), "b"), "NO_RESULT");
        run.steps.get_mut("a").unwrap().result = Some(Bytes(b"not json".to_vec()));
        assert_eq!(failure(&plan(&p, &run, &caps(), 10), "b"), "BAD_RESULT");
        let big = format!("[\"{}\"]", "x".repeat(host::MAX_COMPUTE_ARGS_BYTES));
        run.steps.get_mut("a").unwrap().result = Some(Bytes(big.into_bytes()));
        assert_eq!(failure(&plan(&p, &run, &caps(), 10), "b"), "ARGS_TOO_LARGE");
    }

    #[test]
    fn a_fan_out_step_is_not_supported_yet() {
        let source = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: fan
inputs:
  docs: { kind: files }
steps:
  each: { method: digest, forEach: inputs.docs, maxItems: 4 }
";
        let p = calimero_pipeline::compile(source).unwrap();
        let run = run_of(&p);
        assert_eq!(
            failure(&plan(&p, &run, &caps(), 10), "each"),
            "NOT_YET_SUPPORTED"
        );
    }

    #[test]
    fn a_plan_never_decides_a_step_twice() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let mut run = run_of(&p);
        set(&mut run, "a", StepStatus::Failed);
        let first = plan(&p, &run, &caps(), 10);
        for (step, _) in &first {
            set(&mut run, step, StepStatus::UpstreamFailed);
        }
        assert!(plan(&p, &run, &caps(), 10).is_empty());
    }
}
