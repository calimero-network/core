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
use calimero_pipeline::{parse_ref, AllowFailures, Caps, Pipeline, Ref, Step};
use serde_json::{json, Map, Value};
use tracing::debug;

use super::{locked, Driving};
use crate::app::{AppMachine, Status};
use crate::component::{
    cache_key, run_id, CachedRun, ComponentCmd, CostCap, FanOut, PipelineCancelEntry, PipelineRun,
    PipelineStartEntry, PipelineState, RunCaps, StepDecisionEntry, StepRun, StepStatus,
    StepVerdict, COST_CAP, MAX_CACHED_PER_DECISION, MAX_STEP_RUNS,
};
use crate::error::ApiError;
use crate::node::component::entry_bytes;
use crate::node::{off_workers, Node, ViewAnswer, ViewRequest};
use crate::packages::pipelines::SELF_MODULE;
use crate::types::{AppRecord, Caller};

const DEFAULT_PAGE: usize = 20; // pipeline runs a list answers when the caller names no limit
const MAX_PAGE: usize = 100; // the most pipeline runs a list answers
const MAX_CACHED: usize = 64; // pipelines and compute components kept in memory, by hash
const PIPELINE_UNAVAILABLE: &str = "PIPELINE_UNAVAILABLE"; // the reason of a step whose pipeline definition cannot be found

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

/// What the step cache knows, as the leader reads it from the group: a file's content hash and
/// whether a key has an entry whose outputs are all still held.
pub(super) struct CacheView<'a> {
    pub(super) sha: &'a dyn Fn(&str) -> Option<String>,
    pub(super) hit: &'a dyn Fn(&str) -> bool,
}

/// The compute methods of each compute component a run uses, by bundle hash.
pub(super) type BundleCaps = BTreeMap<String, Arc<BTreeMap<String, Caps>>>;

/// What each step does now: a pending one starts once its needs succeeded or were skipped, fails
/// upstream when one did not, and a cancelled run ends it; a running fan-out gets its next runs
/// while it has items left. `room` is the runs the app can take.
pub(super) fn plan(
    pipeline: &Pipeline,
    run: &PipelineRun,
    caps: &BundleCaps,
    mut room: usize,
    view: &CacheView<'_>,
) -> Vec<(String, StepVerdict)> {
    let mut status: BTreeMap<&str, StepStatus> = run
        .steps
        .iter()
        .map(|(name, step)| (name.as_str(), step.status))
        .collect();
    let mut decided = Vec::new();
    let mut waved: HashSet<&str> = HashSet::new();
    let mut budget = Budget::of(run);
    loop {
        let before = decided.len();
        for (name, step) in &pipeline.steps {
            if run.steps.get(name).map(|s| s.status) == Some(StepStatus::Running)
                && waved.insert(name.as_str())
            {
                let wave = next_wave(pipeline, run, name, step, (caps, room), view, &mut budget);
                if let Some((runs, cached, blocked)) = wave {
                    room -= runs.len();
                    if !runs.is_empty() || !cached.is_empty() {
                        decided.push((name.clone(), StepVerdict::More { runs, cached }));
                    }
                    if blocked {
                        decided.push((name.clone(), cost_capped()));
                        let _prev = status.insert(name.as_str(), StepStatus::Failed);
                    }
                }
                continue;
            }
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
            let mut capped = false;
            let verdict = if run.cancel_requested || run.stopping {
                Some(StepVerdict::Cancelled)
            } else if broken {
                Some(StepVerdict::UpstreamFailed)
            } else if ready {
                match build_step(pipeline, run, name, step, caps, view) {
                    Err(reason) => Some(StepVerdict::Failed {
                        reason: reason.to_owned(),
                    }),
                    Ok(Built::Skip) => Some(StepVerdict::Skipped),
                    Ok(Built::Runs { runs, fan }) => {
                        let wave = wave_size(step, 0, runs.len(), room);
                        let (runs, cached, blocked) = admit(&runs, 0, wave, view, &mut budget);
                        capped = blocked;
                        match (runs.is_empty() && cached.is_empty(), blocked) {
                            (true, true) => Some(cost_capped()),
                            (true, false) => None,
                            (false, _) => {
                                room -= runs.len();
                                Some(StepVerdict::Start { runs, fan, cached })
                            }
                        }
                    }
                }
            } else {
                None
            };
            let Some(verdict) = verdict else { continue };
            let after = match &verdict {
                StepVerdict::Start { .. } | StepVerdict::More { .. } => StepStatus::Running,
                StepVerdict::Skipped => StepStatus::Skipped,
                StepVerdict::UpstreamFailed => StepStatus::UpstreamFailed,
                StepVerdict::Cancelled => StepStatus::Cancelled,
                StepVerdict::Failed { .. } => StepStatus::Failed,
            };
            let _prev = status.insert(name.as_str(), after);
            decided.push((name.clone(), verdict));
            if capped && after == StepStatus::Running {
                decided.push((name.clone(), cost_capped()));
                let _prev = status.insert(name.as_str(), StepStatus::Failed);
            }
        }
        if decided.len() == before {
            return decided;
        }
    }
}

/// The step is stopped because starting its next run would pass the run's cost cap.
fn cost_capped() -> StepVerdict {
    StepVerdict::Failed {
        reason: COST_CAP.to_owned(),
    }
}

/// What a run may still start under its cost cap: the committed cost, counting what this round
/// already decided.
struct Budget {
    cap: Option<CostCap>,
    committed: u128,
}

impl Budget {
    fn of(run: &PipelineRun) -> Self {
        Self {
            cap: run.cost,
            committed: run.committed_cost(),
        }
    }

    /// How many runs with `caps` still fit.
    fn afford(&self, caps: RunCaps) -> usize {
        let Some(cap) = &self.cap else {
            return usize::MAX;
        };
        let unit = u128::from(cap.unit(caps));
        let left = u128::from(cap.max_cost).saturating_sub(self.committed);
        left.checked_div(unit)
            .map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX))
    }

    fn spend(&mut self, caps: RunCaps, runs: usize) {
        if let Some(cap) = &self.cap {
            self.committed += u128::from(cap.unit(caps)) * runs as u128;
        }
    }
}

/// [`take_wave`] held to what the budget affords; the last value says the cap kept runs out that
/// the wave would otherwise have carried, so the step stops here.
fn admit(
    runs: &[StepRun],
    from: usize,
    wave: usize,
    view: &CacheView<'_>,
    budget: &mut Budget,
) -> (Vec<StepRun>, Vec<CachedRun>, bool) {
    let Some(caps) = runs.first().map(|r| r.caps) else {
        return (Vec::new(), Vec::new(), false);
    };
    let fits = budget.afford(caps);
    let (fresh, hits) = take_wave(runs, from, wave.min(fits), view);
    let blocked = fits < wave && take_wave(runs, from, wave, view).0.len() > fresh.len();
    budget.spend(caps, fresh.len());
    (fresh, hits, blocked)
}

/// Runs of a step to start now: `maxParallel` less those active, the app's room and the most a
/// decision may carry, never more than are `left`.
fn wave_size(step: &Step, active: usize, left: usize, room: usize) -> usize {
    let parallel = step
        .max_parallel
        .map_or(usize::MAX, |m| m as usize)
        .saturating_sub(active);
    left.min(parallel).min(room).min(MAX_STEP_RUNS)
}

/// The runs of a step from index `from` on: each run the cache answers goes into the second list,
/// and at most `wave` others into the first, which ends the wave where it would pass that.
fn take_wave(
    runs: &[StepRun],
    from: usize,
    wave: usize,
    view: &CacheView<'_>,
) -> (Vec<StepRun>, Vec<CachedRun>) {
    let (mut fresh, mut hits) = (Vec::new(), Vec::new());
    for (index, run) in runs.iter().enumerate().skip(from) {
        match run.cache_key.as_ref().filter(|k| (view.hit)(k)) {
            Some(key) if hits.len() < MAX_CACHED_PER_DECISION => hits.push(CachedRun {
                index: u32::try_from(index).unwrap_or(u32::MAX),
                key: key.clone(),
            }),
            Some(_) => break,
            None if fresh.len() < wave => fresh.push(run.clone()),
            None => break,
        }
    }
    (fresh, hits)
}

/// The next runs of a running fan-out, if it has items left, tolerates its failures so far and
/// the run is not being stopped.
fn next_wave(
    pipeline: &Pipeline,
    run: &PipelineRun,
    name: &str,
    step: &Step,
    (caps, room): (&BundleCaps, usize),
    view: &CacheView<'_>,
    budget: &mut Budget,
) -> Option<(Vec<StepRun>, Vec<CachedRun>, bool)> {
    let state = run.steps.get(name)?;
    let started = state.runs.len();
    let total = state.fan.total as usize;
    let stopped = run.cancel_requested
        || run.stopping
        || state.capped
        || state.failed_items > state.fan.allowed;
    if stopped || started >= total || step.for_each.is_none() {
        return None;
    }
    let active = started.saturating_sub(state.finished as usize);
    let wave = wave_size(step, active, total - started, room);
    match build_step(pipeline, run, name, step, caps, view) {
        Ok(Built::Runs { runs, .. }) => {
            let (fresh, hits, blocked) = admit(&runs, started, wave, view, budget);
            (!fresh.is_empty() || !hits.is_empty() || blocked).then_some((fresh, hits, blocked))
        }
        _ => None,
    }
}

/// The compute component a step runs in: the run's own for `self`, else the one the run pinned
/// for the module's name.
fn bundle_of<'a>(
    pipeline: &Pipeline,
    run: &'a PipelineRun,
    step: &Step,
) -> Result<&'a str, &'static str> {
    match step.module.as_deref().or(pipeline.module.as_deref()) {
        None | Some(SELF_MODULE) => Ok(&run.compute_bundle),
        Some(name) => run
            .modules
            .get(name)
            .map(String::as_str)
            .ok_or("UNKNOWN_MODULE"),
    }
}

/// What building a step gave: no items to run, or every run it will have.
enum Built {
    Skip,
    Runs { runs: Vec<StepRun>, fan: FanOut },
}

/// What a fan-out runs over: the files of an input or of a step's outputs, or a step's results.
enum Items {
    Files(Vec<String>),
    Values(Vec<Value>),
}

impl Items {
    fn len(&self) -> usize {
        match self {
            Self::Files(f) => f.len(),
            Self::Values(v) => v.len(),
        }
    }
}

/// The files and results a step's `forEach` names, read from the record.
fn items_of(run: &PipelineRun, source: &str) -> Result<Items, &'static str> {
    match parse_ref(source) {
        Some(Ref::Input(n)) => Ok(Items::Files(run.inputs.get(n).cloned().unwrap_or_default())),
        Some(Ref::Step(s, "outputs")) => Ok(Items::Files(step_outputs(run, s))),
        Some(Ref::Step(s, "results")) => step_results(run, s).map(Items::Values),
        _ => Err("BAD_REF"),
    }
}

fn step_outputs(run: &PipelineRun, step: &str) -> Vec<String> {
    run.steps
        .get(step)
        .map(|s| s.outputs().map(|o| o.id.clone()).collect())
        .unwrap_or_default()
}

/// A step's results as JSON, in item order; none are kept once they passed their bound.
fn step_results(run: &PipelineRun, step: &str) -> Result<Vec<Value>, &'static str> {
    let state = run.steps.get(step).ok_or("BAD_REF")?;
    if state.results_too_large {
        return Err("RESULTS_TOO_LARGE");
    }
    state
        .run_results
        .values()
        .map(|b| serde_json::from_slice(&b.0).map_err(|_| "BAD_RESULT"))
        .collect()
}

/// The files of a run: the step's `inputs` references in the order written, with `lead` where
/// `item` stands in them, or first when none does. A file is named once.
fn ordered_inputs(
    step: &Step,
    run: &PipelineRun,
    lead: &[String],
) -> Result<Vec<String>, &'static str> {
    let item_placed = step
        .inputs
        .iter()
        .any(|r| matches!(parse_ref(r), Some(Ref::Item)));
    let mut seen: HashSet<String> = HashSet::new();
    let mut inputs: Vec<String> = Vec::new();
    let mut add = |ids: &[String]| {
        for id in ids {
            if seen.insert(id.clone()) {
                inputs.push(id.clone());
            }
        }
    };
    if !item_placed {
        add(lead);
    }
    for reference in &step.inputs {
        match parse_ref(reference) {
            Some(Ref::Item) => add(lead),
            Some(Ref::Input(n)) => add(run.inputs.get(n).map_or(&[][..], Vec::as_slice)),
            Some(Ref::Step(s, "outputs")) => add(&step_outputs(run, s)),
            _ => return Err("BAD_REF"),
        }
    }
    if inputs.len() > host::MAX_PIPELINE_RUN_INPUTS {
        return Err("TOO_MANY_INPUTS");
    }
    Ok(inputs)
}

/// Every compute run a step will have: one for a plain step, one per batch of items for a
/// `forEach`, one over all a fan-out's outputs for a `collect`. A refusal is a stable code that
/// fails the step.
fn build_step(
    pipeline: &Pipeline,
    run: &PipelineRun,
    name: &str,
    step: &Step,
    caps: &BundleCaps,
    view: &CacheView<'_>,
) -> Result<Built, &'static str> {
    let bundle = bundle_of(pipeline, run, step)?;
    let cap = caps
        .get(bundle)
        .and_then(|methods| methods.get(&step.method))
        .ok_or("UNKNOWN_METHOD")?;
    let limits = step
        .limits
        .unwrap_or_default()
        .overridden_by(run.limits.get(name).copied().unwrap_or_default());
    let caps = RunCaps {
        fuel: limits.fuel.map_or(cap.fuel, |v| v.min(cap.fuel)),
        memory_mib: limits
            .memory_mib
            .map_or(cap.memory_mib, |v| v.min(cap.memory_mib)),
        wall_secs: limits
            .wall_secs
            .map_or(cap.wall_secs, |v| v.min(cap.wall_secs)),
    };
    let make = |index: usize, inputs: Vec<String>, item: Option<&Value>| {
        let args = resolve_args(pipeline, run, step, item)?;
        let key = step
            .cache
            .then(|| step_key(bundle, step, &args, &inputs, view))
            .flatten();
        Ok::<_, &'static str>(StepRun {
            run: run_id(&run.id, name, index),
            method: step.method.clone(),
            args: Bytes(args),
            inputs,
            caps,
            compute_bundle: (bundle != run.compute_bundle).then(|| bundle.to_owned()),
            cache_key: key,
            pool: step.pool.clone(),
        })
    };
    if let Some(source) = &step.for_each {
        let items = items_of(run, source)?;
        let n = items.len();
        if n == 0 {
            return Ok(Built::Skip);
        }
        if n > step.max_items.unwrap_or(u32::MAX) as usize {
            return Err("TOO_MANY_ITEMS");
        }
        let batch = step.batch.unwrap_or(1).max(1) as usize;
        let mut runs = Vec::new();
        for index in 0..n.div_ceil(batch) {
            let range = index * batch..((index + 1) * batch).min(n);
            let (files, item) = match &items {
                Items::Files(f) => {
                    let group = &f[range];
                    (
                        group.to_vec(),
                        group_value(group.iter().map(|id| json!(id))),
                    )
                }
                Items::Values(v) => (Vec::new(), group_value(v[range].iter().cloned())),
            };
            let inputs = ordered_inputs(step, run, &files)?;
            runs.push(make(index, inputs, Some(&item))?);
        }
        let n = u32::try_from(n).map_err(|_| "TOO_MANY_ITEMS")?;
        let allowed = match step.allow_failures {
            None => 0,
            Some(AllowFailures::Count(c)) => c.min(n),
            Some(AllowFailures::Percent(p)) => {
                u32::try_from(u64::from(n) * u64::from(p) / 100).unwrap_or(n)
            }
        };
        let fan = FanOut {
            total: u32::try_from(runs.len()).map_err(|_| "TOO_MANY_ITEMS")?,
            items: n,
            batch: u32::try_from(batch).unwrap_or(u32::MAX),
            allowed,
        };
        return Ok(Built::Runs { runs, fan });
    }
    let lead = match step.collect.as_deref().and_then(parse_ref) {
        Some(Ref::Step(target, "outputs")) => step_outputs(run, target),
        _ => Vec::new(),
    };
    let inputs = ordered_inputs(step, run, &lead)?;
    let runs = vec![make(0, inputs, None)?];
    Ok(Built::Runs {
        runs,
        fan: FanOut::SINGLE,
    })
}

/// The step cache key of a run, none when an input's content is not known.
fn step_key(
    bundle: &str,
    step: &Step,
    args: &[u8],
    inputs: &[String],
    view: &CacheView<'_>,
) -> Option<String> {
    let hashes: Vec<String> = inputs
        .iter()
        .map(|i| (view.sha)(i))
        .collect::<Option<_>>()?;
    Some(cache_key(bundle, &step.method, args, &hashes))
}

/// A batch's `item`: the value itself for a batch of one, else the list of them.
fn group_value(values: impl Iterator<Item = Value>) -> Value {
    let mut all: Vec<Value> = values.collect();
    if all.len() == 1 {
        all.remove(0)
    } else {
        Value::Array(all)
    }
}

/// A step's `args` with references filled in, as bytes: none is no bytes, as `compute::start`
/// sends for `()`.
fn resolve_args(
    pipeline: &Pipeline,
    run: &PipelineRun,
    step: &Step,
    item: Option<&Value>,
) -> Result<Vec<u8>, &'static str> {
    let args = step
        .args
        .iter()
        .map(|(k, v)| Ok((k.clone(), resolve(pipeline, run, v, item)?)))
        .collect::<Result<Map<String, Value>, &'static str>>()?;
    if args.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = serde_json::to_vec(&args).map_err(|_| "BAD_REF")?;
    if bytes.len() > host::MAX_COMPUTE_ARGS_BYTES {
        return Err("ARGS_TOO_LARGE");
    }
    Ok(bytes)
}

/// `v` with each reference replaced: a param by the run's value or the param's default, the
/// batch's `item`, a step's `result` or `results` by those read as JSON.
fn resolve(
    pipeline: &Pipeline,
    run: &PipelineRun,
    v: &Value,
    item: Option<&Value>,
) -> Result<Value, &'static str> {
    match v {
        Value::String(s) => match parse_ref(s) {
            None => Ok(v.clone()),
            Some(Ref::Param(n)) => run
                .params
                .get(n)
                .cloned()
                .or_else(|| pipeline.params.get(n).and_then(|p| p.default.clone()))
                .ok_or("BAD_REF"),
            Some(Ref::Item) => item.cloned().ok_or("BAD_REF"),
            Some(Ref::Step(step, "result")) => {
                let state = run.steps.get(step).ok_or("NO_RESULT")?;
                if state.results_too_large {
                    return Err("RESULTS_TOO_LARGE");
                }
                let result = state.result().ok_or("NO_RESULT")?;
                serde_json::from_slice(&result.0).map_err(|_| "BAD_RESULT")
            }
            Some(Ref::Step(step, "results")) => step_results(run, step).map(Value::Array),
            Some(_) => Err("BAD_REF"),
        },
        Value::Array(items) => items
            .iter()
            .map(|x| resolve(pipeline, run, x, item))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => map
            .iter()
            .map(|(k, x)| Ok((k.clone(), resolve(pipeline, run, x, item)?)))
            .collect::<Result<Map<_, _>, _>>()
            .map(Value::Object),
        _ => Ok(v.clone()),
    }
}

/// A pipeline run as the API shows it: sizes, hashes and results only to those who may list
/// the app's files, a result as JSON when it is.
fn run_json(run: &PipelineRun, files: bool) -> Value {
    let mut v = serde_json::to_value(run).unwrap_or(Value::Null);
    if let Some(cap) = &run.cost {
        v["cost"]["maxCost"] = json!(cap.max_cost.to_string());
        v["cost"]["worstCase"] = json!(cap.worst_case.to_string());
        v["costSoFar"] = json!(run.cost_so_far.to_string());
    }
    for (name, step) in &run.steps {
        let Some(shown) = v["steps"][name].as_object_mut() else {
            continue;
        };
        let _gone = (shown.remove("runOutputs"), shown.remove("runResults"));
        let _gone = (shown.remove("cachedRuns"), shown.remove("runCost"));
        if run.cost.is_some() {
            let _prev = shown.insert("cost".to_owned(), json!(step.cost.to_string()));
        }
        let outputs: Vec<Value> = step
            .outputs()
            .map(|o| {
                let mut o = serde_json::to_value(o).unwrap_or(Value::Null);
                if let (false, Some(o)) = (files, o.as_object_mut()) {
                    let _gone = (o.remove("size"), o.remove("sha256"));
                }
                o
            })
            .collect();
        let _prev = shown.insert("outputs".to_owned(), Value::Array(outputs));
        let _prev = shown.insert("cached".to_owned(), json!(step.is_cached()));
        let _prev = shown.insert("cachedItems".to_owned(), json!(step.cached_items()));
        let json_of =
            |b: &Bytes| serde_json::from_slice(&b.0).unwrap_or_else(|_| json!(hex::encode(&b.0)));
        let result = step.result().filter(|_| files).map(json_of);
        let _prev = shown.insert("result".to_owned(), result.unwrap_or(Value::Null));
        let results: Vec<Value> = step.run_results.values().map(json_of).collect();
        let results = if files {
            Value::Array(results)
        } else {
            Value::Null
        };
        let _prev = shown.insert("results".to_owned(), results);
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
                Some(run) if req.args["receipt"].as_bool() == Some(true) => {
                    m.pipeline_run(run).map_or(Value::Null, |r| {
                        let proofs = r
                            .steps
                            .values()
                            .flat_map(|s| &s.runs)
                            .filter_map(|id| Some((id.clone(), m.step_proof(id)?)))
                            .collect();
                        let source = crate::api::pipeline_receipt::ReceiptSource { run: r, proofs };
                        serde_json::to_value(source).unwrap_or(Value::Null)
                    })
                }
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
        let waiting = |s: &crate::component::StepState| match s.status {
            StepStatus::Pending => true,
            StepStatus::Running => s.runs.len() < s.fan.total as usize,
            _ => false,
        };
        if !run.steps.values().any(waiting) {
            return;
        }
        let pipeline = match self.pipeline_by_hash(&run.hash).await {
            Ok(pipeline) => pipeline,
            Err(e) if e.status == 404 => {
                debug!(run = %run.id, error = %e.message, "pipeline is gone");
                self.fail_unavailable(g, run).await;
                return;
            }
            Err(e) => {
                debug!(run = %run.id, error = %e.message, "pipeline not loaded");
                return;
            }
        };
        let mut caps = BundleCaps::new();
        for bundle in std::iter::once(&run.compute_bundle).chain(run.modules.values()) {
            match self.caps_of_component(bundle).await {
                Ok(c) => {
                    let _prev = caps.insert(bundle.clone(), c);
                }
                Err(e) => {
                    debug!(run = %run.id, error = %e.message, "compute component not loaded");
                    return;
                }
            }
        }
        let decided = g.read_local(|m, _| {
            let room = host::MAX_ACTIVE_RUNS.saturating_sub(m.active_runs().len());
            let view = CacheView {
                sha: &|id| m.file_sha256(id),
                hit: &|key| m.cache_hit(key),
            };
            plan(&pipeline, run, &caps, room, &view)
        });
        for (step, verdict) in decided {
            self.propose_step(g, &run.id, &step, verdict).await;
        }
    }

    /// The pipeline's definition is gone: every step that has not started, and every fan-out that
    /// has runs left to start, fails, which ends the run once its running steps settle.
    async fn fail_unavailable(&self, g: &Group<AppMachine>, run: &PipelineRun) {
        for (name, step) in &run.steps {
            let open = match step.status {
                StepStatus::Pending => true,
                StepStatus::Running => step.runs.len() < step.fan.total as usize,
                _ => false,
            };
            if open {
                let reason = PIPELINE_UNAVAILABLE.to_owned();
                self.propose_step(g, &run.id, name, StepVerdict::Failed { reason })
                    .await;
            }
        }
    }

    /// Logs the decision unless it no longer applies (the step is not pending, or a fan-out's next
    /// runs are not these); apply judges it again.
    async fn propose_step(
        &self,
        g: &Group<AppMachine>,
        run: &str,
        step: &str,
        decision: StepVerdict,
    ) {
        let proposed = g
            .propose_led(|m, _, _| {
                let current = m
                    .pipeline_run(run)
                    .filter(|p| p.state == PipelineState::Running)
                    .and_then(|p| p.steps.get(step).cloned());
                let applies = current.is_some_and(|s| match &decision {
                    StepVerdict::More { runs, cached } => {
                        s.status == StepStatus::Running
                            && match (runs.first(), cached.first()) {
                                (Some(r), _) => r.run == run_id(run, step, s.runs.len()),
                                (None, Some(c)) => c.index as usize == s.runs.len(),
                                (None, None) => false,
                            }
                    }
                    StepVerdict::Failed { .. } => {
                        s.status == StepStatus::Pending
                            || (s.status == StepStatus::Running
                                && s.runs.len() < s.fan.total as usize)
                    }
                    _ => s.status == StepStatus::Pending,
                });
                if !applies {
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
    use calimero_pipeline::Limits;

    use super::*;
    use crate::component::{PipelineState, StepState};
    use crate::types::Caller;

    /// A cache that knows no file and has no entry.
    fn plan(
        p: &Pipeline,
        run: &PipelineRun,
        caps: &BTreeMap<String, Caps>,
        room: usize,
    ) -> Vec<(String, StepVerdict)> {
        let view = CacheView {
            sha: &|_| None,
            hit: &|_| false,
        };
        let by_bundle = BundleCaps::from([(run.compute_bundle.clone(), Arc::new(caps.clone()))]);
        super::plan(p, run, &by_bundle, room, &view)
    }

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
            modules: BTreeMap::new(),
            owner: "o".into(),
            inputs: BTreeMap::from([("docs".to_owned(), vec!["f1".to_owned(), "f2".to_owned()])]),
            input_files: BTreeMap::new(),
            params: BTreeMap::new(),
            caller: Caller::System,
            payer: "p".into(),
            start_ts_ms: 0,
            state: PipelineState::Running,
            cancel_requested: false,
            fail_fast: false,
            stopping: false,
            steps: pipeline
                .steps
                .keys()
                .map(|k| (k.clone(), StepState::pending()))
                .collect(),
            end_ts_ms: None,
            cost: None,
            cost_so_far: 0,
            retry_of: None,
            limits: BTreeMap::new(),
        }
    }

    fn set(run: &mut PipelineRun, step: &str, status: StepStatus) {
        run.steps.get_mut(step).unwrap().status = status;
    }

    fn start_of(plan: &[(String, StepVerdict)], step: &str) -> StepRun {
        match plan.iter().find(|(s, _)| s == step) {
            Some((_, StepVerdict::Start { runs, .. })) => runs[0].clone(),
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
        a.run_outputs = BTreeMap::from([(
            0,
            vec![crate::component::RunInputRef {
                id: "out1".into(),
                size: 1,
                sha256: String::new(),
            }],
        )]);
        a.run_results = BTreeMap::from([(0, Bytes(b"[\"h1\"]".to_vec()))]);
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
            (0..=host::MAX_PIPELINE_RUN_INPUTS)
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
        run.steps.get_mut("a").unwrap().run_results =
            BTreeMap::from([(0, Bytes(b"not json".to_vec()))]);
        assert_eq!(failure(&plan(&p, &run, &caps(), 10), "b"), "BAD_RESULT");
        let big = format!("[\"{}\"]", "x".repeat(host::MAX_COMPUTE_ARGS_BYTES));
        run.steps.get_mut("a").unwrap().run_results =
            BTreeMap::from([(0, Bytes(big.into_bytes()))]);
        assert_eq!(failure(&plan(&p, &run, &caps(), 10), "b"), "ARGS_TOO_LARGE");
    }

    const FAN_HEAD: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: fan
inputs:
  docs: { kind: files }
  model: { kind: files }
  extra: { kind: files }
steps:
";

    fn fan_pipeline(steps: &str) -> Pipeline {
        calimero_pipeline::compile(&format!("{FAN_HEAD}{steps}")).unwrap()
    }

    fn fan_run(p: &Pipeline, docs: usize) -> PipelineRun {
        let mut run = run_of(p);
        let docs = (0..docs).map(|i| format!("d{i}")).collect();
        run.inputs = BTreeMap::from([
            ("docs".to_owned(), docs),
            ("model".to_owned(), vec!["m".to_owned()]),
            ("extra".to_owned(), vec!["x".to_owned()]),
        ]);
        run
    }

    fn started(plan: &[(String, StepVerdict)], step: &str) -> (Vec<StepRun>, FanOut) {
        match plan.iter().find(|(s, _)| s == step) {
            Some((_, StepVerdict::Start { runs, fan, .. })) => (runs.clone(), *fan),
            other => panic!("{step} did not start: {other:?}"),
        }
    }

    fn inputs_of(runs: &[StepRun]) -> Vec<Vec<&str>> {
        runs.iter()
            .map(|r| r.inputs.iter().map(String::as_str).collect())
            .collect()
    }

    fn out_ref(id: &str) -> crate::component::RunInputRef {
        crate::component::RunInputRef {
            id: id.into(),
            size: 1,
            sha256: String::new(),
        }
    }

    #[test]
    fn a_fan_out_runs_once_per_item_with_the_item_before_the_steps_other_inputs() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 9, inputs: [inputs.model] }\n");
        let (runs, fan) = started(&plan(&p, &fan_run(&p, 3), &caps(), 10), "each");
        assert_eq!(inputs_of(&runs), [["d0", "m"], ["d1", "m"], ["d2", "m"]]);
        assert_eq!(runs[2].run, "r.each.2");
        assert_eq!(
            fan,
            FanOut {
                total: 3,
                items: 3,
                batch: 1,
                allowed: 0
            }
        );
    }

    #[test]
    fn a_step_with_no_inputs_of_its_own_gets_just_the_item_files() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 9 }\n");
        let (runs, _) = started(&plan(&p, &fan_run(&p, 2), &caps(), 10), "each");
        assert_eq!(inputs_of(&runs), [["d0"], ["d1"]]);
    }

    #[test]
    fn item_stands_where_the_step_writes_it() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 9, inputs: [inputs.model, item, inputs.extra] }\n");
        let (runs, _) = started(&plan(&p, &fan_run(&p, 2), &caps(), 10), "each");
        assert_eq!(inputs_of(&runs), [["m", "d0", "x"], ["m", "d1", "x"]]);
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 9, inputs: [inputs.model, item] }\n");
        let (runs, _) = started(&plan(&p, &fan_run(&p, 1), &caps(), 10), "each");
        assert_eq!(inputs_of(&runs), [["m", "d0"]], "last");
    }

    #[test]
    fn a_batch_groups_items_in_order_into_fewer_runs() {
        let p = fan_pipeline("  each: { method: echo, forEach: inputs.docs, maxItems: 9, batch: 2, args: { files: item } }\n");
        let (runs, fan) = started(&plan(&p, &fan_run(&p, 5), &caps(), 10), "each");
        assert_eq!(
            inputs_of(&runs),
            [vec!["d0", "d1"], vec!["d2", "d3"], vec!["d4"]]
        );
        assert_eq!((fan.total, fan.items, fan.batch), (3, 5, 2));
        let args: Value = serde_json::from_slice(&runs[0].args.0).unwrap();
        assert_eq!(args, json!({ "files": ["d0", "d1"] }));
        let args: Value = serde_json::from_slice(&runs[2].args.0).unwrap();
        assert_eq!(args, json!({ "files": "d4" }), "a batch of one is the item");
    }

    fn results_run(p: &Pipeline, results: &[&str]) -> PipelineRun {
        let mut run = fan_run(p, 1);
        set(&mut run, "a", StepStatus::Succeeded);
        run.steps.get_mut("a").unwrap().run_results = results
            .iter()
            .enumerate()
            .map(|(i, r)| (u32::try_from(i).unwrap(), Bytes(r.as_bytes().to_vec())))
            .collect();
        run
    }

    const OVER_RESULTS: &str = "  a: { method: digest }
  each: { method: echo, forEach: a.results, maxItems: 9, args: { v: item } }
";

    #[test]
    fn a_results_fan_out_passes_each_value_as_the_item() {
        let p = fan_pipeline(OVER_RESULTS);
        let run = results_run(&p, &["1", "{\"k\":2}"]);
        let (runs, fan) = started(&plan(&p, &run, &caps(), 10), "each");
        let args: Vec<Value> = runs
            .iter()
            .map(|r| serde_json::from_slice(&r.args.0).unwrap())
            .collect();
        assert_eq!(args, [json!({ "v": 1 }), json!({ "v": { "k": 2 } })]);
        assert!(runs.iter().all(|r| r.inputs.is_empty()));
        assert_eq!(fan.items, 2);
    }

    #[test]
    fn a_fan_out_over_results_that_passed_their_bound_fails_the_referencing_step() {
        let p = fan_pipeline(OVER_RESULTS);
        let mut run = results_run(&p, &[]);
        run.steps.get_mut("a").unwrap().results_too_large = true;
        assert_eq!(
            failure(&plan(&p, &run, &caps(), 10), "each"),
            "RESULTS_TOO_LARGE"
        );
        let p = fan_pipeline(
            "  a: { method: digest }\n  b: { method: echo, args: { all: a.results } }\n",
        );
        let mut run = results_run(&p, &["1"]);
        assert!(!started(&plan(&p, &run, &caps(), 10), "b").0[0]
            .args
            .0
            .is_empty());
        run.steps.get_mut("a").unwrap().results_too_large = true;
        assert_eq!(
            failure(&plan(&p, &run, &caps(), 10), "b"),
            "RESULTS_TOO_LARGE"
        );
    }

    #[test]
    fn an_empty_list_skips_the_step_and_too_many_items_fail_it_before_any_run() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 3 }\n");
        let decided = plan(&p, &fan_run(&p, 0), &caps(), 10);
        assert_eq!(decided, vec![("each".to_owned(), StepVerdict::Skipped)]);
        assert_eq!(
            failure(&plan(&p, &fan_run(&p, 4), &caps(), 10), "each"),
            "TOO_MANY_ITEMS"
        );
        started(&plan(&p, &fan_run(&p, 3), &caps(), 10), "each");
    }

    #[test]
    fn waves_respect_max_parallel_and_the_apps_room() {
        let p = fan_pipeline(
            "  each: { method: digest, forEach: inputs.docs, maxItems: 20, maxParallel: 3 }\n",
        );
        let mut run = fan_run(&p, 10);
        let (first, fan) = started(&plan(&p, &run, &caps(), 10), "each");
        assert_eq!(first.len(), 3, "maxParallel");
        assert_eq!(fan.total, 10);
        assert_eq!(
            started(&plan(&p, &run, &caps(), 2), "each").0.len(),
            2,
            "room"
        );
        assert!(plan(&p, &run, &caps(), 0).is_empty());

        let step = run.steps.get_mut("each").unwrap();
        step.status = StepStatus::Running;
        step.fan = fan;
        step.runs = (0..3).map(|i| format!("r.each.{i}")).collect();
        assert!(plan(&p, &run, &caps(), 10).is_empty(), "three are active");
        run.steps.get_mut("each").unwrap().finished = 1;
        let decided = plan(&p, &run, &caps(), 10);
        let StepVerdict::More { runs, .. } = &decided[0].1 else {
            panic!("{decided:?}")
        };
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run, "r.each.3", "the next index");
        assert_eq!(runs[0].inputs, ["d3"]);
        assert!(plan(&p, &run, &caps(), 0).is_empty());
        run.steps.get_mut("each").unwrap().finished = 3;
        let decided = plan(&p, &run, &caps(), 10);
        let StepVerdict::More { runs, .. } = &decided[0].1 else {
            panic!("{decided:?}")
        };
        assert_eq!(runs.len(), 3, "up to maxParallel again");
    }

    /// A table that prices a run of `caps()` at 20 at worst.
    fn capped(run: &mut PipelineRun, max_cost: u64) {
        let table = calimero_pipeline::PriceTable {
            version: 1,
            fuel_per_vcpu_second: 100,
            vcpu_second: 10,
            gib_second: 5,
            connector_gib: 0,
            storage_gib_month: 0,
            egress_gib: 0,
        };
        run.cost = Some(CostCap {
            max_cost,
            price_version: 1,
            table,
            worst_case: 0,
        });
    }

    fn is_cost_cap(v: &StepVerdict) -> bool {
        matches!(v, StepVerdict::Failed { reason } if reason == COST_CAP)
    }

    #[test]
    fn a_first_wave_is_cut_to_what_the_cost_cap_affords_and_the_step_stops() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 20 }\n");
        let mut run = fan_run(&p, 10);
        capped(&mut run, 65);
        let decided = plan(&p, &run, &caps(), 10);
        assert_eq!(decided.len(), 2, "{decided:?}");
        assert_eq!(
            started(&decided, "each").0.len(),
            3,
            "three worst cases fit"
        );
        assert!(is_cost_cap(&decided[1].1), "{decided:?}");
        capped(&mut run, 1_000);
        let decided = plan(&p, &run, &caps(), 10);
        assert_eq!(decided.len(), 1, "a cap that covers the step stops nothing");
        assert_eq!(started(&decided, "each").0.len(), 10);
    }

    #[test]
    fn a_retrys_limits_replace_the_pipelines_own_within_the_methods_caps() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let mut run = run_of(&p);
        // `a` has fuel 7 and wallSecs 9 in the pipeline; the cap is 100 fuel.
        run.limits = BTreeMap::from([(
            "a".to_owned(),
            Limits {
                fuel: Some(60),
                ..Limits::default()
            },
        )]);
        let a = start_of(&plan(&p, &run, &caps(), 10), "a");
        assert_eq!(
            (a.caps.fuel, a.caps.wall_secs),
            (60, 9),
            "fuel raised, wall kept"
        );
        run.limits.get_mut("a").unwrap().fuel = Some(5_000);
        let a = start_of(&plan(&p, &run, &caps(), 10), "a");
        assert_eq!(a.caps.fuel, 100, "never above the method's cap");
    }

    #[test]
    fn a_wave_the_parallelism_cuts_anyway_is_not_stopped_by_the_cap() {
        let p = fan_pipeline(
            "  each: { method: digest, forEach: inputs.docs, maxItems: 20, maxParallel: 2 }\n",
        );
        let mut run = fan_run(&p, 10);
        capped(&mut run, 65);
        let decided = plan(&p, &run, &caps(), 10);
        assert_eq!(decided.len(), 1, "{decided:?}");
        assert_eq!(started(&decided, "each").0.len(), 2);
    }

    #[test]
    fn a_next_run_that_does_not_fit_fails_the_step_instead_of_starting() {
        let p = fan_pipeline(
            "  each: { method: digest, forEach: inputs.docs, maxItems: 20, maxParallel: 5 }\n",
        );
        let mut run = fan_run(&p, 10);
        capped(&mut run, 65);
        let (_, fan) = started(&plan(&p, &run, &caps(), 10), "each");
        let step = run.steps.get_mut("each").unwrap();
        step.status = StepStatus::Running;
        step.fan = fan;
        step.runs = (0..3).map(|i| format!("r.each.{i}")).collect();
        step.run_cost = 20;
        let decided = plan(&p, &run, &caps(), 10);
        assert_eq!(decided.len(), 1, "no wave, only the stop: {decided:?}");
        assert!(is_cost_cap(&decided[0].1));
        let step = run.steps.get_mut("each").unwrap();
        step.capped = true;
        assert!(
            plan(&p, &run, &caps(), 10).is_empty(),
            "a capped step is left alone"
        );
    }

    #[test]
    fn finished_runs_free_the_cap_by_what_they_used() {
        let p = fan_pipeline(
            "  each: { method: digest, forEach: inputs.docs, maxItems: 20, maxParallel: 5 }\n",
        );
        let mut run = fan_run(&p, 10);
        capped(&mut run, 65);
        let (_, fan) = started(&plan(&p, &run, &caps(), 10), "each");
        let step = run.steps.get_mut("each").unwrap();
        step.status = StepStatus::Running;
        step.fan = fan;
        step.runs = (0..3).map(|i| format!("r.each.{i}")).collect();
        step.run_cost = 20;
        step.finished = 3;
        step.cost = 6;
        let decided = plan(&p, &run, &caps(), 10);
        let StepVerdict::More { runs, .. } = &decided[0].1 else {
            panic!("{decided:?}")
        };
        assert_eq!(runs.len(), 2, "6 used leaves room for two of 20 under 65");
        assert!(is_cost_cap(&decided[1].1), "{decided:?}");
    }

    #[test]
    fn a_running_fan_out_stops_when_failures_pass_the_allowance_or_the_run_ends() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 20, maxParallel: 3, allowFailures: 1 }\n");
        let mut run = fan_run(&p, 10);
        let (_, fan) = started(&plan(&p, &run, &caps(), 10), "each");
        assert_eq!(fan.allowed, 1);
        let step = run.steps.get_mut("each").unwrap();
        step.status = StepStatus::Running;
        step.fan = fan;
        step.runs = (0..3).map(|i| format!("r.each.{i}")).collect();
        step.finished = 3;
        step.failed_items = 1;
        assert_eq!(plan(&p, &run, &caps(), 10).len(), 1, "at the allowance");
        run.steps.get_mut("each").unwrap().failed_items = 2;
        assert!(plan(&p, &run, &caps(), 10).is_empty(), "one over");
        run.steps.get_mut("each").unwrap().failed_items = 0;
        run.stopping = true;
        assert!(plan(&p, &run, &caps(), 10).is_empty());
    }

    #[test]
    fn the_allowance_is_a_count_or_a_share_of_the_items_rounded_down() {
        let allowed = |spec: &str, docs: usize| {
            let p = fan_pipeline(&format!("  each: {{ method: digest, forEach: inputs.docs, maxItems: 200, allowFailures: {spec} }}\n"));
            started(&plan(&p, &fan_run(&p, docs), &caps(), 10), "each")
                .1
                .allowed
        };
        assert_eq!(allowed("5%", 100), 5);
        assert_eq!(allowed("5%", 39), 1);
        assert_eq!(allowed("3%", 10), 0);
        assert_eq!(allowed("100%", 7), 7);
        assert_eq!(allowed("3", 100), 3);
        assert_eq!(allowed("30", 4), 4, "never more than the items");
    }

    #[test]
    fn a_collect_gets_every_output_of_the_fan_out_in_item_order_then_its_own_inputs() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 200 }\n  all: { method: digest, collect: each.outputs, inputs: [inputs.model] }\n");
        let mut run = fan_run(&p, 100);
        set(&mut run, "each", StepStatus::Succeeded);
        let outputs = &mut run.steps.get_mut("each").unwrap().run_outputs;
        for i in (0..100u32).rev().filter(|i| i % 10 != 3) {
            outputs.insert(i, vec![out_ref(&format!("o{i}"))]);
        }
        let (runs, fan) = started(&plan(&p, &run, &caps(), 10), "all");
        assert_eq!(fan, FanOut::SINGLE);
        let inputs = &runs[0].inputs;
        assert_eq!(inputs.len(), 91, "the failed items' outputs are absent");
        assert!(inputs.len() > host::MAX_RUN_INPUTS);
        assert_eq!((inputs[0].as_str(), inputs[1].as_str()), ("o0", "o1"));
        assert_eq!(inputs[90], "m");
        assert_eq!(inputs[89], "o99");
    }

    #[test]
    fn a_start_of_a_full_collect_fits_one_log_entry() {
        let inputs = (0..host::MAX_PIPELINE_RUN_INPUTS)
            .map(|i| format!("{i:064x}"))
            .collect();
        let runs = vec![StepRun {
            run: "r.all.0".into(),
            method: "digest".into(),
            args: Bytes(Vec::new()),
            inputs,
            caps: RunCaps {
                fuel: 1,
                memory_mib: 1,
                wall_secs: 1,
            },
            compute_bundle: None,
            cache_key: None,
            pool: None,
        }];
        let mut cmd = ComponentCmd::PipelineStep(Box::new(StepDecisionEntry {
            run: "r".into(),
            step: "all".into(),
            decision: StepVerdict::Start {
                runs,
                fan: FanOut::SINGLE,
                cached: Vec::new(),
            },
            ts_ms: 1,
        }));
        let bytes = entry_bytes(&mut cmd).0;
        assert!(
            bytes.len() < calimero_log::MAX_ENTRY_BYTES,
            "{}",
            bytes.len()
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

    /// `plan` with a cache that hashes a file as `content` says (its id when not listed) and
    /// answers the keys in `hits`.
    fn cplan(
        p: &Pipeline,
        run: &PipelineRun,
        caps: &BTreeMap<String, Caps>,
        room: usize,
        content: &BTreeMap<&str, &str>,
        hits: &HashSet<String>,
    ) -> Vec<(String, StepVerdict)> {
        let view = CacheView {
            sha: &|id| Some((*content.get(id).unwrap_or(&id)).to_owned()),
            hit: &|key| hits.contains(key),
        };
        let by_bundle = BundleCaps::from([(run.compute_bundle.clone(), Arc::new(caps.clone()))]);
        super::plan(p, run, &by_bundle, room, &view)
    }

    fn key_of(
        p: &Pipeline,
        run: &PipelineRun,
        caps: &BTreeMap<String, Caps>,
        c: &[(&str, &str)],
    ) -> String {
        let content = c.iter().copied().collect();
        let decided = cplan(p, run, caps, 10, &content, &HashSet::new());
        start_of(&decided, "a").cache_key.expect("no key")
    }

    #[test]
    fn the_key_changes_with_what_a_run_computes_and_not_with_a_file_id_or_its_caps() {
        let p = calimero_pipeline::compile(SOURCE).unwrap();
        let run = run_of(&p);
        let base = key_of(&p, &run, &caps(), &[]);
        assert_eq!(base, key_of(&p, &run, &caps(), &[]), "stable");

        let mut other = run.clone();
        other.compute_bundle = "bundle-2".into();
        assert_ne!(base, key_of(&p, &other, &caps(), &[]), "bundle");

        let method =
            calimero_pipeline::compile(&SOURCE.replace("a: { method: digest", "a: { method: echo"))
                .unwrap();
        assert_ne!(
            base,
            key_of(&method, &run_of(&method), &caps(), &[]),
            "method"
        );

        let args = calimero_pipeline::compile(&SOURCE.replace(
            "a: { method: digest,",
            "a: { method: digest, args: { n: 1 },",
        ))
        .unwrap();
        assert_ne!(base, key_of(&args, &run_of(&args), &caps(), &[]), "args");

        assert_ne!(
            base,
            key_of(&p, &run, &caps(), &[("f1", "changed")]),
            "an input's content"
        );

        let mut lower = caps();
        lower.get_mut("digest").unwrap().fuel = 50;
        let tight = calimero_pipeline::compile(&SOURCE.replace("fuel: 7, ", "fuel: 90, ")).unwrap();
        assert_eq!(base, key_of(&tight, &run_of(&tight), &lower, &[]), "caps");

        let mut renamed = run.clone();
        renamed
            .inputs
            .insert("docs".into(), vec!["g1".into(), "g2".into()]);
        let same = key_of(&p, &renamed, &caps(), &[("g1", "f1"), ("g2", "f2")]);
        assert_eq!(base, same, "the same content under other file ids");
    }

    #[test]
    fn a_step_with_cache_false_has_no_key() {
        let p = calimero_pipeline::compile(
            &SOURCE.replace("a: { method: digest,", "a: { method: digest, cache: false,"),
        )
        .unwrap();
        let decided = cplan(
            &p,
            &run_of(&p),
            &caps(),
            10,
            &BTreeMap::new(),
            &HashSet::new(),
        );
        assert!(start_of(&decided, "a").cache_key.is_none());
    }

    #[test]
    fn hits_go_in_cached_and_misses_in_runs() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 9 }\n");
        let run = fan_run(&p, 4);
        let all = cplan(&p, &run, &caps(), 10, &BTreeMap::new(), &HashSet::new());
        let (runs, _) = started(&all, "each");
        let hits: HashSet<String> = [0, 2]
            .iter()
            .map(|i| runs[*i].cache_key.clone().unwrap())
            .collect();
        let decided = cplan(&p, &run, &caps(), 10, &BTreeMap::new(), &hits);
        let Some((_, StepVerdict::Start { runs, cached, .. })) = decided.first() else {
            panic!("{decided:?}");
        };
        let fresh: Vec<&str> = runs.iter().map(|r| r.run.as_str()).collect();
        assert_eq!(fresh, ["r.each.1", "r.each.3"]);
        let hit: Vec<u32> = cached.iter().map(|c| c.index).collect();
        assert_eq!(hit, [0, 2]);
    }

    #[test]
    fn hits_cost_no_room_and_a_wave_ends_at_the_first_miss_past_it() {
        let p = fan_pipeline("  each: { method: digest, forEach: inputs.docs, maxItems: 9 }\n");
        let run = fan_run(&p, 4);
        let all = cplan(&p, &run, &caps(), 10, &BTreeMap::new(), &HashSet::new());
        let (runs, _) = started(&all, "each");
        let hits: HashSet<String> = [0, 1, 3]
            .iter()
            .map(|i| runs[*i].cache_key.clone().unwrap())
            .collect();
        let decided = cplan(&p, &run, &caps(), 0, &BTreeMap::new(), &hits);
        let Some((_, StepVerdict::Start { runs, cached, .. })) = decided.first() else {
            panic!("{decided:?}");
        };
        assert!(runs.is_empty());
        assert_eq!(
            cached.len(),
            2,
            "indices 0 and 1, then the miss at 2 ends the wave"
        );
        let none = cplan(
            &p,
            &fan_run(&p, 4),
            &caps(),
            0,
            &BTreeMap::new(),
            &HashSet::new(),
        );
        assert!(none.is_empty(), "no hit and no room decides nothing");
    }

    const MODULE_SOURCE: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: m
steps:
  a: { module: pdf@1, method: digest }
  b: { method: digest, needs: [a] }
";

    #[test]
    fn a_module_step_runs_in_the_pinned_component_with_its_caps() {
        let p = calimero_pipeline::compile(MODULE_SOURCE).unwrap();
        let mut run = run_of(&p);
        run.modules = BTreeMap::from([("pdf@1".to_owned(), "module-v1".to_owned())]);
        let small = Caps {
            fuel: 7,
            memory_mib: 8,
            wall_secs: 9,
        };
        let mut by_bundle = BundleCaps::from([
            (run.compute_bundle.clone(), Arc::new(caps())),
            (
                "module-v1".to_owned(),
                Arc::new(BTreeMap::from([("digest".to_owned(), small)])),
            ),
        ]);
        let view = CacheView {
            sha: &|id| Some(id.to_owned()),
            hit: &|_| false,
        };
        let decided = super::plan(&p, &run, &by_bundle, 10, &view);
        let a = start_of(&decided, "a");
        assert_eq!(a.compute_bundle.as_deref(), Some("module-v1"));
        assert_eq!(a.caps.fuel, 7, "the module's caps, not the package's");

        set(&mut run, "a", StepStatus::Succeeded);
        let b = start_of(&super::plan(&p, &run, &by_bundle, 10, &view), "b");
        assert_eq!(b.compute_bundle, None, "`self` is the run's own component");

        // A run that pinned no such module fails the step; one whose component is unknown too.
        run.modules.clear();
        set(&mut run, "a", StepStatus::Pending);
        let decided = super::plan(&p, &run, &by_bundle, 10, &view);
        assert_eq!(failure(&decided, "a"), "UNKNOWN_MODULE");
        run.modules = BTreeMap::from([("pdf@1".to_owned(), "module-v1".to_owned())]);
        let _gone = by_bundle.remove("module-v1");
        let decided = super::plan(&p, &run, &by_bundle, 10, &view);
        assert_eq!(failure(&decided, "a"), "UNKNOWN_METHOD");
    }
}
