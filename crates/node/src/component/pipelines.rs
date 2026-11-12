//! Pipeline runs in a component app's state: the record of each run and the three entries that
//! move it (`pipelineStart`, `pipelineStep`, `pipelineCancel`), plus how a finished compute run
//! settles its step.
//!
//! Every rule is judged on the record and the entry alone, so all replicas agree. The leader
//! decides what a step does and logs it; apply only checks it still may. The first decision for a
//! step wins, so a leader that proposes again after a change of leader changes nothing. Step runs
//! are ordinary compute runs (`RunRecord::pipeline` links them back); their ids are fixed by the
//! pipeline run, step and index, so a repeated start can never make a second run.

use std::collections::{BTreeMap, BTreeSet};

use calimero_app_host as host;
use calimero_log::{Bytes, StateTx};
use calimero_pipeline::{priced_cost, run_cost, Caps, Limits, PriceTable};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::runs::{
    FailedReason, FinalOutcome, PipelineLink, RunCaps, RunInputRef, RunRecord, RunState,
};
use super::step_cache::{CacheEntry, CachedRun, MAX_CACHED_PER_DECISION};
use super::{get_json, host_answer, parse_hex32, put_json, Applied, FileMark};
use crate::app::{rows, AppMachine};
use crate::types::Caller;

/// Pipeline runs per app that are running at once.
pub const MAX_ACTIVE_PIPELINE_RUNS: usize = 8;
/// Finished pipeline runs whose records are kept, oldest dropped first.
pub const FINISHED_PIPELINE_RUNS_KEPT: usize = 64;
/// The most of one run's result a record keeps.
pub const MAX_STEP_RESULT_BYTES: usize = 64 * 1024;
/// The most results a step keeps in all; past it a reference to `<step>.results` fails.
pub const MAX_STEP_RESULTS_BYTES: usize = 1024 * 1024;
/// Compute runs one step may have in all, at one item each.
pub const MAX_STEP_TOTAL_RUNS: usize = calimero_pipeline::MAX_ITEMS as usize;
/// Compute runs one step decision may start.
pub const MAX_STEP_RUNS: usize = host::MAX_ACTIVE_RUNS;
/// The longest pipeline run id, step name or reason an entry may carry.
const MAX_NAME_BYTES: usize = 128;

pub const TOO_MANY_PIPELINE_RUNS: &str = "TOO_MANY_PIPELINE_RUNS";
pub const TOO_MANY_FAILURES: &str = "TOO_MANY_FAILURES";
pub const NO_ROOM: &str = "NO_ROOM";
pub const COST_CAP: &str = "COST_CAP";
pub(super) const KNOWN: &str = "the pipeline run exists";
pub(super) const BAD_START: &str = "the pipeline run is not well formed";
pub const INPUT_NOT_HELD: &str = "an input is not a file the app holds";
pub(super) const NOT_RUNNING: &str = "the pipeline run is not running";
pub(super) const NO_SUCH_STEP: &str = "the pipeline run has no such step";
pub(super) const DECIDED: &str = "the step was decided already";
pub(super) const CANCELLING: &str = "the pipeline run is being cancelled";
pub(super) const NOT_FAN_OUT: &str = "the step is not a running fan-out with runs left";
pub(super) const NOT_NEXT: &str = "the runs are not the step's next ones";
pub(super) const STEP_STOPPING: &str = "the step is ending and starts no more runs";
pub(super) const CACHE_GONE: &str = "a cached run's entry or outputs are gone";

/// Where a pipeline run is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PipelineState {
    Running,
    Succeeded,
    /// Every step succeeded or was skipped and some fan-out step tolerated failed items.
    SucceededWithFailures,
    Failed,
    Cancelled,
}

/// Where a step is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    UpstreamFailed,
    Cancelled,
}

impl StepStatus {
    /// Whether the step will not change again.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }

    /// Whether the steps that need this one may run.
    #[must_use]
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Succeeded | Self::Skipped)
    }
}

/// One step of a pipeline run; the default is a step that has not started.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepState {
    pub status: StepStatus,
    /// The compute runs the step started so far, in index order.
    pub runs: Vec<String>,
    /// The files each run made by run index, pinned until the pipeline run's record is dropped.
    pub run_outputs: BTreeMap<u32, Vec<RunInputRef>>,
    /// What each run returned by run index, each at most [`MAX_STEP_RESULT_BYTES`].
    pub run_results: BTreeMap<u32, Bytes>,
    /// Set once the results passed [`MAX_STEP_RESULTS_BYTES`]; none are kept then.
    pub results_too_large: bool,
    pub reason: Option<String>,
    /// How many runs the step will have, and how its items map onto them; zero before it starts.
    pub fan: FanOut,
    /// Runs of the step that have ended.
    pub finished: u32,
    /// Items of the runs that failed or were cancelled.
    pub failed_items: u32,
    /// Indices of the runs answered from the step cache, which started no compute run.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub cached_runs: BTreeSet<u32>,
    /// The step cache key each cached run was answered under, by run index.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cached_keys: BTreeMap<u32, String>,
    /// What the step's finished runs used, priced by the run's table; zero without a cost cap.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cost: u64,
    /// The worst case of one run of the step, which each of its active runs holds back.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub run_cost: u64,
    /// Set when the cost cap stopped the step from starting more runs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub capped: bool,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl StepState {
    /// Whether the step finished with every one of its runs answered from the cache.
    #[must_use]
    pub fn is_cached(&self) -> bool {
        self.status == StepStatus::Succeeded
            && !self.cached_runs.is_empty()
            && self.cached_runs.len() == self.fan.total as usize
    }

    /// Items carried by the runs answered from the cache.
    #[must_use]
    pub fn cached_items(&self) -> u32 {
        self.cached_runs.iter().map(|i| self.items_of(*i)).sum()
    }

    /// The files the step's runs made, in item order.
    pub fn outputs(&self) -> impl Iterator<Item = &RunInputRef> {
        self.run_outputs.values().flatten()
    }

    /// The result of a single-run step.
    #[must_use]
    pub fn result(&self) -> Option<&Bytes> {
        self.run_results.get(&0)
    }

    /// Items the run at `index` carries.
    #[must_use]
    pub fn items_of(&self, index: u32) -> u32 {
        let FanOut { items, batch, .. } = self.fan;
        items.saturating_sub(index.saturating_mul(batch)).min(batch)
    }
}

/// The shape of a started step: `total` runs carrying `items` items `batch` at a time, of which
/// `allowed` may fail. A step that is not a fan-out is one run of one item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanOut {
    pub total: u32,
    pub items: u32,
    pub batch: u32,
    pub allowed: u32,
}

impl FanOut {
    /// One run of one item.
    pub const SINGLE: Self = Self {
        total: 1,
        items: 1,
        batch: 1,
        allowed: 0,
    };

    fn well_formed(&self) -> bool {
        let batch = u64::from(self.batch);
        self.batch >= 1
            && self.total >= 1
            && self.total as usize <= MAX_STEP_TOTAL_RUNS
            && u64::from(self.items) <= batch * u64::from(self.total)
            && u64::from(self.items) > batch * (u64::from(self.total) - 1)
            && self.allowed <= self.items
    }
}

/// The cost cap of a run: what the caller allows, and the price table the run was admitted under,
/// copied so apply never reads system state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostCap {
    pub max_cost: u64,
    pub price_version: u64,
    pub table: PriceTable,
    /// The most the pipeline can cost under `table`, saturated; the cap does not refuse a run over it.
    pub worst_case: u64,
}

impl CostCap {
    /// The worst case of one run with `caps`, saturated.
    #[must_use]
    pub fn unit(&self, caps: RunCaps) -> u64 {
        let cost = run_cost(caps.fuel, caps.memory_mib, caps.wall_secs, &self.table);
        u64::try_from(cost).unwrap_or(u64::MAX)
    }
}

/// The record of one pipeline run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRun {
    pub id: String,
    pub pipeline: String,
    /// The package release the run started under.
    pub version: String,
    /// Hex SHA-256 of the pipeline's canonical JSON, which the leader loads by this hash.
    pub hash: String,
    /// The compute component of that release, fixed at start.
    pub compute_bundle: String,
    /// The compute component of each other package's module the pipeline names, by the name as
    /// written, fixed at start.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, String>,
    pub owner: String,
    /// Input name to file ids.
    pub inputs: BTreeMap<String, Vec<String>>,
    /// Each input file as the app held it at start, by file id, for the receipt.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_files: BTreeMap<String, RunInputRef>,
    pub params: BTreeMap<String, Value>,
    pub caller: Caller,
    pub payer: String,
    pub start_ts_ms: u64,
    pub state: PipelineState,
    pub cancel_requested: bool,
    /// The pipeline's `failFast`: the first failed step stops the run.
    #[serde(default)]
    pub fail_fast: bool,
    /// Set when a failed step stopped a fail-fast run; no step starts or runs more then.
    #[serde(default)]
    pub stopping: bool,
    pub steps: BTreeMap<String, StepState>,
    pub end_ts_ms: Option<u64>,
    /// The cost cap the run started with, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostCap>,
    /// What the finished runs of all steps used, priced.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cost_so_far: u64,
    /// The run this one retries, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    /// Per-step limits a retry raised; each replaces the same field of the pipeline's own.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub limits: BTreeMap<String, Limits>,
}

impl PipelineRun {
    /// What the run may already owe: its finished runs' usage plus the worst case of every active
    /// run of a running step.
    #[must_use]
    pub fn committed_cost(&self) -> u128 {
        self.steps
            .values()
            .map(|s| {
                let active = if s.status == StepStatus::Running {
                    u128::from(
                        u32::try_from(s.runs.len())
                            .unwrap_or(u32::MAX)
                            .saturating_sub(s.finished),
                    )
                } else {
                    0
                };
                u128::from(s.cost) + active * u128::from(s.run_cost)
            })
            .sum()
    }

    /// Whether starting `runs` would take the committed cost past the cap.
    #[must_use]
    pub fn over_cap(&self, runs: &[StepRun]) -> bool {
        let Some(cap) = &self.cost else { return false };
        let new: u128 = runs.iter().map(|r| u128::from(cap.unit(r.caps))).sum();
        self.committed_cost() + new > u128::from(cap.max_cost)
    }

    /// Whether a wave of `runs` may start: not while the run is cancelled or stopping (`stopped`
    /// is the refusal then) and not past the cost cap.
    fn admits(&self, runs: &[StepRun], stopped: &'static str) -> Result<(), &'static str> {
        if self.cancel_requested || self.stopping {
            return Err(stopped);
        }
        if self.over_cap(runs) {
            return Err(COST_CAP);
        }
        Ok(())
    }

    /// Holds back the worst case of `step`'s runs, once started.
    fn reserve(&mut self, step: &str, runs: &[StepRun]) {
        let Some(cap) = self.cost else { return };
        let unit = runs.iter().map(|r| cap.unit(r.caps)).max().unwrap_or(0);
        if let Some(state) = self.steps.get_mut(step) {
            state.run_cost = state.run_cost.max(unit);
        }
    }
}

/// `pipelineStart`: a pipeline run begins. `steps` names every step, which start pending.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStartEntry {
    pub id: String,
    pub pipeline: String,
    pub version: String,
    pub hash: String,
    pub compute_bundle: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, String>,
    pub owner: String,
    pub steps: Vec<String>,
    pub inputs: BTreeMap<String, Vec<String>>,
    pub params: BTreeMap<String, Value>,
    pub caller: Caller,
    pub payer: String,
    #[serde(default)]
    pub fail_fast: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostCap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub limits: BTreeMap<String, Limits>,
    pub ts_ms: u64,
}

/// One compute run a step starts, as the leader built it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRun {
    pub run: String,
    pub method: String,
    pub args: Bytes,
    /// File ids of the app.
    pub inputs: Vec<String>,
    pub caps: RunCaps,
    /// The module's compute component when the step runs another package's; none is the run's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute_bundle: Option<String>,
    /// The step cache key the leader made for this run; none when the step has `cache: false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    /// The pool the step names; none places the run by its memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
}

/// What the leader decided for a pending step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StepVerdict {
    /// The step's first wave of runs; `fan` says how many it will have in all.
    Start {
        runs: Vec<StepRun>,
        fan: FanOut,
        /// Runs answered from the step cache; with `runs` they cover the indices from 0.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cached: Vec<CachedRun>,
    },
    /// The next runs of a running fan-out step, from the first index it has not started.
    More {
        runs: Vec<StepRun>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cached: Vec<CachedRun>,
    },
    Skipped,
    UpstreamFailed,
    Cancelled,
    /// The step cannot be built; `reason` is a stable code.
    Failed {
        reason: String,
    },
}

/// `pipelineStep`: the decision for one step.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDecisionEntry {
    pub run: String,
    pub step: String,
    pub decision: StepVerdict,
    pub ts_ms: u64,
}

/// `pipelineCancel`: stop the pipeline run.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineCancelEntry {
    pub run: String,
    pub ts_ms: u64,
}

fn name_fits(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES && !name.contains('.')
}

/// The reason a run that did not finish done gives its step.
fn failure_of(record: &RunRecord) -> Option<String> {
    let outcome = record.final_.as_ref().map(|f| f.outcome);
    match (record.state, outcome) {
        (RunState::Done, _) => None,
        (_, Some(FinalOutcome::Failed { reason })) => Some(reason_code(reason)),
        _ => Some("cancelled".to_owned()),
    }
}

pub(super) fn reason_code(reason: FailedReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl AppMachine {
    /// The record of pipeline run `run`, active or among the last finished.
    #[must_use]
    pub fn pipeline_run(&self, run: &str) -> Option<PipelineRun> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CPIPE, run.as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }

    /// The ids of the pipeline runs that are running, in order.
    #[must_use]
    pub fn active_pipelines(&self) -> Vec<String> {
        self.state
            .component
            .head
            .active_pipelines
            .iter()
            .cloned()
            .collect()
    }

    /// Up to `limit` pipeline runs, newest first.
    #[must_use]
    pub fn pipeline_runs_page(&self, limit: usize) -> Vec<PipelineRun> {
        let head = &self.state.component.head;
        let mut all: Vec<PipelineRun> = head
            .active_pipelines
            .iter()
            .chain(&head.finished_pipelines)
            .filter_map(|run| self.pipeline_run(run))
            .collect();
        all.sort_by(|a, b| (b.start_ts_ms, &b.id).cmp(&(a.start_ts_ms, &a.id)));
        all.truncate(limit);
        all
    }

    /// The marks of `ids` as the app's sealed files hold them; none when one is not held.
    pub(super) fn file_refs<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a String>,
    ) -> Option<Vec<RunInputRef>> {
        let blobs = &self.state.component.blobs.records;
        ids.into_iter()
            .map(|id| {
                let record = blobs.get(&parse_hex32(id)?)?;
                let seal = record.file.as_ref()?;
                Some(RunInputRef {
                    id: id.clone(),
                    size: record.size,
                    sha256: hex::encode(&seal.sha256.0),
                })
            })
            .collect()
    }

    /// Adds `delta` references to each distinct file in `ids`.
    pub(super) fn pin_files<'a>(
        &mut self,
        tx: &mut StateTx<'_>,
        ids: impl IntoIterator<Item = &'a String>,
        delta: i64,
        ts_ms: u64,
    ) {
        let mut deltas = BTreeMap::new();
        for id in ids.into_iter().filter_map(|i| parse_hex32(i)) {
            *deltas.entry(id).or_insert(0) += delta;
        }
        self.state.component.blobs.count(tx, &deltas, ts_ms);
    }

    /// `pipelineStart`: a run with every step pending, its inputs pinned. Refused when the id is
    /// known, the app has [`MAX_ACTIVE_PIPELINE_RUNS`] running, or an input is not a held file.
    pub(super) fn apply_pipeline_start(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        entry: &PipelineStartEntry,
    ) -> Applied {
        let no_op = |why| host_answer(index, false, why);
        let head = &self.state.component.head;
        let well_formed = name_fits(&entry.id)
            && !entry.steps.is_empty()
            && entry.steps.len() <= calimero_pipeline::MAX_STEPS
            && entry.steps.iter().all(|s| name_fits(s))
            && entry.retry_of.as_deref().is_none_or(name_fits)
            && entry.limits.keys().all(|s| entry.steps.contains(s));
        if !well_formed {
            return no_op(BAD_START);
        }
        if let Some(known) = get_json::<PipelineRun>(tx, rows::CPIPE, &entry.id) {
            // A start the leader re-proposed after losing its answer is the same run: say so.
            return if same_start(&known, entry) {
                host_answer(index, true, "")
            } else {
                no_op(KNOWN)
            };
        }
        if head.active_pipelines.contains(&entry.id) {
            return no_op(KNOWN);
        }
        if head.active_pipelines.len() >= MAX_ACTIVE_PIPELINE_RUNS {
            return no_op(TOO_MANY_PIPELINE_RUNS);
        }
        let files: BTreeSet<&String> = entry.inputs.values().flatten().collect();
        let Some(held) = self.file_refs(files.iter().copied()) else {
            return no_op(INPUT_NOT_HELD);
        };
        let record = PipelineRun {
            id: entry.id.clone(),
            pipeline: entry.pipeline.clone(),
            version: entry.version.clone(),
            hash: entry.hash.clone(),
            compute_bundle: entry.compute_bundle.clone(),
            modules: entry.modules.clone(),
            owner: entry.owner.clone(),
            inputs: entry.inputs.clone(),
            input_files: held.into_iter().map(|f| (f.id.clone(), f)).collect(),
            params: entry.params.clone(),
            caller: entry.caller.clone(),
            payer: entry.payer.clone(),
            start_ts_ms: entry.ts_ms,
            state: PipelineState::Running,
            cancel_requested: false,
            fail_fast: entry.fail_fast,
            stopping: false,
            steps: entry
                .steps
                .iter()
                .map(|s| (s.clone(), StepState::default()))
                .collect(),
            end_ts_ms: None,
            cost: entry.cost,
            cost_so_far: 0,
            retry_of: entry.retry_of.clone(),
            limits: entry.limits.clone(),
        };
        put_json(tx, rows::CPIPE, &record.id, &record);
        let _new = self
            .state
            .component
            .head
            .active_pipelines
            .insert(entry.id.clone());
        let c = &mut self.state.component;
        c.head.last_ts_ms = c.head.last_ts_ms.max(entry.ts_ms);
        self.pin_files(tx, files, 1, entry.ts_ms);
        host_answer(index, true, "")
    }

    /// `pipelineStep`: the first decision for a pending step, or more runs for a running
    /// fan-out. A start that does not fit changes nothing, so it is decided again; one that can
    /// never work fails the step.
    pub(super) fn apply_pipeline_step(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        entry: &StepDecisionEntry,
    ) -> Applied {
        let no_op = |why| host_answer(index, false, why);
        let Some(mut pipeline) = get_json::<PipelineRun>(tx, rows::CPIPE, &entry.run) else {
            return no_op(NOT_RUNNING);
        };
        if pipeline.state != PipelineState::Running {
            return no_op(NOT_RUNNING);
        }
        let Some(step) = pipeline.steps.get(&entry.step) else {
            return no_op(NO_SUCH_STEP);
        };
        let between_waves = step.runs.len() < step.fan.total as usize;
        let running = step.status == StepStatus::Running;
        match (&entry.decision, step.status) {
            (StepVerdict::More { .. }, StepStatus::Running) => {}
            (StepVerdict::More { .. }, _) => return no_op(NOT_FAN_OUT),
            (_, StepStatus::Pending) => {}
            (StepVerdict::Failed { .. }, StepStatus::Running) if between_waves => {}
            _ => return no_op(DECIDED),
        }
        let ts_ms = entry.ts_ms;
        let c = &mut self.state.component;
        c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
        let mut hit_any = false;
        let mut capped = false;
        let (status, reason) = match &entry.decision {
            StepVerdict::Start { runs, fan, cached } => {
                if let Err(why) = pipeline.admits(runs, CANCELLING) {
                    return no_op(why);
                }
                let started =
                    if fan.well_formed() && runs.len() + cached.len() <= fan.total as usize {
                        self.start_wave(
                            tx,
                            &pipeline,
                            &entry.step,
                            0,
                            (runs, cached),
                            "BAD_START",
                            ts_ms,
                        )
                    } else {
                        Err(Refusal::Failed("BAD_START"))
                    };
                match started {
                    Ok(wave) => {
                        if let Some(step) = pipeline.steps.get_mut(&entry.step) {
                            step.runs = wave.ids;
                            step.fan = *fan;
                        }
                        hit_any = self.land(tx, &mut pipeline, &entry.step, runs, wave.hits, ts_ms);
                        (StepStatus::Running, None)
                    }
                    Err(Refusal::NoRoom) => return no_op(NO_ROOM),
                    Err(Refusal::CacheGone) => return no_op(CACHE_GONE),
                    Err(Refusal::Failed(code)) => (StepStatus::Failed, Some(code.to_owned())),
                }
            }
            StepVerdict::More { runs, cached } => {
                if let Err(why) = pipeline.admits(runs, STEP_STOPPING) {
                    return no_op(why);
                }
                match self.start_more_runs(tx, &pipeline, &entry.step, (runs, cached), ts_ms) {
                    Ok(wave) => {
                        if let Some(step) = pipeline.steps.get_mut(&entry.step) {
                            step.runs.extend(wave.ids);
                        }
                        hit_any = self.land(tx, &mut pipeline, &entry.step, runs, wave.hits, ts_ms);
                        (StepStatus::Running, None)
                    }
                    Err(Refusal::NoRoom) => return no_op(NO_ROOM),
                    Err(Refusal::CacheGone) => return no_op(CACHE_GONE),
                    Err(Refusal::Failed(code)) if code == NOT_NEXT => return no_op(NOT_NEXT),
                    Err(Refusal::Failed(code)) if code == STEP_STOPPING => {
                        return no_op(STEP_STOPPING)
                    }
                    Err(Refusal::Failed(code)) => {
                        self.stop_step_runs(tx, &pipeline, &entry.step);
                        (StepStatus::Failed, Some(code.to_owned()))
                    }
                }
            }
            StepVerdict::Skipped => (StepStatus::Skipped, None),
            StepVerdict::UpstreamFailed => (StepStatus::UpstreamFailed, None),
            StepVerdict::Cancelled => (StepStatus::Cancelled, None),
            StepVerdict::Failed { reason } if running && reason == COST_CAP => {
                // The runs already going finish and are paid for; only new ones are refused.
                capped = true;
                (StepStatus::Running, Some(COST_CAP.to_owned()))
            }
            StepVerdict::Failed { reason } => {
                self.stop_step_runs(tx, &pipeline, &entry.step);
                (
                    StepStatus::Failed,
                    Some(reason.chars().take(MAX_NAME_BYTES).collect()),
                )
            }
        };
        if let Some(step) = pipeline.steps.get_mut(&entry.step) {
            step.status = status;
            if reason.is_some() {
                step.reason = reason;
            }
        }
        if capped {
            if let Some(step) = pipeline.steps.get_mut(&entry.step) {
                step.capped = true;
            }
        }
        if hit_any || capped {
            self.settle_step(tx, &mut pipeline, &entry.step);
        }
        self.settle_after_change(tx, &mut pipeline, ts_ms);
        put_json(tx, rows::CPIPE, &pipeline.id, &pipeline);
        host_answer(index, true, "")
    }

    /// The next wave of a running fan-out: it continues where the step's runs end, never past
    /// the step's total and never once its failures have passed what it tolerates.
    fn start_more_runs(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &PipelineRun,
        step: &str,
        wave: (&[StepRun], &[CachedRun]),
        ts_ms: u64,
    ) -> Result<Wave, Refusal> {
        let Some(state) = pipeline.steps.get(step) else {
            return Err(Refusal::Failed(NOT_NEXT));
        };
        if state.failed_items > state.fan.allowed || state.capped {
            return Err(Refusal::Failed(STEP_STOPPING));
        }
        let first = state.runs.len();
        let n = wave.0.len() + wave.1.len();
        if n == 0 || first + n > state.fan.total as usize {
            return Err(Refusal::Failed(NOT_NEXT));
        }
        self.start_wave(tx, pipeline, step, first, wave, NOT_NEXT, ts_ms)
    }

    /// Starts the runs of a wave and finds its cache hits: the wave covers the indices from
    /// `first`, a hit whose entry or outputs are gone ends it there, and nothing before that is lost.
    #[allow(clippy::too_many_arguments)]
    fn start_wave(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &PipelineRun,
        step: &str,
        first: usize,
        (runs, cached): (&[StepRun], &[CachedRun]),
        bad: &'static str,
        ts_ms: u64,
    ) -> Result<Wave, Refusal> {
        let slots = wave_slots(&pipeline.id, step, first, runs, cached)
            .filter(|s| !s.is_empty() && cached.len() <= MAX_CACHED_PER_DECISION)
            .ok_or(Refusal::Failed(bad))?;
        let mut hits = Vec::new();
        let mut kept = slots.len();
        for (n, slot) in slots.iter().enumerate() {
            let Slot::Hit(index, c) = slot else { continue };
            let held = get_json::<CacheEntry>(tx, rows::CCACHE, &c.key)
                .filter(|e| self.file_refs(e.outputs.iter().map(|o| &o.id)).is_some());
            let Some(entry) = held else {
                kept = n;
                break;
            };
            hits.push((
                u32::try_from(*index).unwrap_or(u32::MAX),
                c.key.clone(),
                entry,
            ));
        }
        if kept == 0 {
            return Err(Refusal::CacheGone);
        }
        let fresh: Vec<(usize, &StepRun)> = slots[..kept]
            .iter()
            .filter_map(|s| match s {
                Slot::Run(i, r) => Some((*i, *r)),
                Slot::Hit(..) => None,
            })
            .collect();
        self.start_runs_from(tx, pipeline, step, &fresh, ts_ms)?;
        let ids = (first..first + kept)
            .map(|i| run_id(&pipeline.id, step, i))
            .collect();
        Ok(Wave { ids, hits })
    }

    /// Reserves the cost of a started wave and takes its cache hits; whether it had any.
    fn land(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &mut PipelineRun,
        step: &str,
        runs: &[StepRun],
        hits: Vec<(u32, String, CacheEntry)>,
        ts_ms: u64,
    ) -> bool {
        let any = !hits.is_empty();
        pipeline.reserve(step, runs);
        self.record_hits(tx, pipeline, step, hits, ts_ms);
        any
    }

    /// Takes the cache hits of a wave into the step as finished runs.
    fn record_hits(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &mut PipelineRun,
        step: &str,
        hits: Vec<(u32, String, CacheEntry)>,
        ts_ms: u64,
    ) {
        for (index, key, entry) in hits {
            self.pin_files(tx, entry.outputs.iter().map(|o| &o.id), 1, ts_ms);
            if let Some(state) = pipeline.steps.get_mut(step) {
                state.finished += 1;
                let _new = state.cached_runs.insert(index);
                let _prev = state.cached_keys.insert(index, key.clone());
                let _prev = state.run_outputs.insert(index, entry.outputs.clone());
                if let Some(result) = &entry.result {
                    keep_result(state, index, &result.0);
                }
            }
            self.touch_cache(tx, &key, entry, ts_ms);
        }
    }

    /// Records the runs as queued compute runs, linked to their step by index, and pins their
    /// inputs.
    fn start_runs_from(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &PipelineRun,
        step: &str,
        runs: &[(usize, &StepRun)],
        ts_ms: u64,
    ) -> Result<(), Refusal> {
        if runs.is_empty() {
            return Ok(());
        }
        let ids_ok = runs.iter().all(|(_, r)| {
            !r.method.is_empty()
                && r.args.0.len() <= host::MAX_COMPUTE_ARGS_BYTES
                && r.inputs.len() <= host::MAX_PIPELINE_RUN_INPUTS
        });
        if runs.len() > MAX_STEP_RUNS || !ids_ok {
            return Err(Refusal::Failed("BAD_START"));
        }
        let pinned = |b: &String| pipeline.modules.values().any(|m| m == b);
        if !runs
            .iter()
            .all(|(_, r)| r.compute_bundle.as_ref().is_none_or(pinned))
        {
            return Err(Refusal::Failed("UNKNOWN_MODULE"));
        }
        let head = &self.state.component.head;
        if head.active_runs.len() + runs.len() > host::MAX_ACTIVE_RUNS {
            return Err(Refusal::NoRoom);
        }
        if runs.iter().any(|(_, r)| {
            head.active_runs.contains(&r.run)
                || get_json::<RunRecord>(tx, rows::CRUN, &r.run).is_some()
        }) {
            return Err(Refusal::Failed("RUN_EXISTS"));
        }
        let mut records = Vec::new();
        for (index, r) in runs {
            let Some(inputs) = self.file_refs(&r.inputs) else {
                return Err(Refusal::Failed("INPUT_NOT_HELD"));
            };
            records.push((*index, *r, inputs));
        }
        let bundle = self
            .state
            .bundle
            .clone()
            .unwrap_or_else(|| pipeline.compute_bundle.clone());
        for (index, r, inputs) in records {
            let record = RunRecord {
                run: r.run.clone(),
                method: r.method.clone(),
                args: r.args.clone(),
                args_hash: hex::encode(calimero_identity::sha256(&r.args.0)),
                inputs: inputs.clone(),
                caps: r.caps,
                result_method: String::new(),
                bundle: bundle.clone(),
                compute_bundle: Some(
                    r.compute_bundle
                        .clone()
                        .unwrap_or_else(|| pipeline.compute_bundle.clone()),
                ),
                caller: pipeline.caller.clone(),
                owner: pipeline.owner.clone(),
                start_ts_ms: ts_ms,
                state: RunState::Queued,
                cancel_requested: false,
                attempts: Vec::new(),
                final_: None,
                pipeline: Some(PipelineLink {
                    run: pipeline.id.clone(),
                    step: step.to_owned(),
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                }),
                cache_key: r.cache_key.clone(),
                pool: r.pool.clone(),
            };
            put_json(tx, rows::CRUN, &record.run, &record);
            let _new = self.state.component.head.active_runs.insert(r.run.clone());
            self.pin_inputs(tx, &inputs, 1, ts_ms);
        }
        Ok(())
    }

    /// Tells the active runs of `runs` to stop.
    fn flag_cancel<'a>(&self, tx: &mut StateTx<'_>, runs: impl IntoIterator<Item = &'a String>) {
        for run in runs {
            if !self.state.component.head.active_runs.contains(run) {
                continue;
            }
            match get_json::<RunRecord>(tx, rows::CRUN, run) {
                Some(mut record) if !record.cancel_requested => {
                    record.cancel_requested = true;
                    put_json(tx, rows::CRUN, &record.run, &record);
                }
                _ => {}
            }
        }
    }

    fn stop_step_runs(&self, tx: &mut StateTx<'_>, pipeline: &PipelineRun, step: &str) {
        if let Some(state) = pipeline.steps.get(step) {
            self.flag_cancel(tx, &state.runs);
        }
    }

    /// `pipelineCancel`: the run stops starting steps, and its running steps' runs are told to
    /// stop. Pending steps are ended by the leader's `Cancelled` decisions.
    pub(super) fn apply_pipeline_cancel(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        entry: &PipelineCancelEntry,
    ) -> Applied {
        let Some(mut pipeline) = get_json::<PipelineRun>(tx, rows::CPIPE, &entry.run) else {
            return host_answer(index, false, NOT_RUNNING);
        };
        if pipeline.cancel_requested {
            return host_answer(index, true, "");
        }
        if pipeline.state != PipelineState::Running {
            return host_answer(index, false, NOT_RUNNING);
        }
        let c = &mut self.state.component;
        c.head.last_ts_ms = c.head.last_ts_ms.max(entry.ts_ms);
        pipeline.cancel_requested = true;
        self.flag_cancel(tx, pipeline.steps.values().flat_map(|s| &s.runs));
        self.settle_running_steps(tx, &mut pipeline);
        self.settle_pipeline(tx, &mut pipeline, entry.ts_ms);
        put_json(tx, rows::CPIPE, &pipeline.id, &pipeline);
        host_answer(index, true, "")
    }

    /// A compute run that belongs to a step has ended: keep its outputs and result by its index,
    /// count its items as failed if it did not finish done, and settle the step once every run it
    /// will have is final. `record` is the run as it now stands.
    pub(super) fn settle_step_run(
        &mut self,
        tx: &mut StateTx<'_>,
        record: &RunRecord,
        outputs: &[FileMark],
        result: &[u8],
        ts_ms: u64,
    ) {
        let Some(link) = &record.pipeline else {
            return;
        };
        let made: Vec<RunInputRef> = outputs
            .iter()
            .map(|o| RunInputRef {
                id: o.id.clone(),
                size: o.size,
                sha256: hex::encode(&o.sha256.0),
            })
            .collect();
        if let (Some(key), None) = (&record.cache_key, failure_of(record)) {
            if result.len() <= MAX_STEP_RESULT_BYTES {
                let entry = CacheEntry {
                    outputs: made.clone(),
                    result: (!result.is_empty()).then(|| Bytes(result.to_vec())),
                    ts_ms,
                    source_run: record.run.clone(),
                };
                self.cache_put(tx, key, entry, ts_ms);
            }
        }
        let Some(mut pipeline) = get_json::<PipelineRun>(tx, rows::CPIPE, &link.run) else {
            return;
        };
        let spent = pipeline.cost.map_or(0, |cap| {
            let usage = record.final_.as_ref().map(|f| f.usage).unwrap_or_default();
            let caps = Caps {
                fuel: record.caps.fuel,
                memory_mib: record.caps.memory_mib,
                wall_secs: record.caps.wall_secs,
            };
            let cost = priced_cost(
                usage.fuel,
                usage.gib_micros,
                usage.cpu_micros,
                caps,
                &cap.table,
            );
            u64::try_from(cost).unwrap_or(u64::MAX)
        });
        pipeline.cost_so_far = pipeline.cost_so_far.saturating_add(spent);
        let Some(step) = pipeline.steps.get_mut(&link.step) else {
            return;
        };
        step.cost = step.cost.saturating_add(spent);
        if step.status != StepStatus::Running {
            if pipeline.cost.is_some() {
                put_json(tx, rows::CPIPE, &pipeline.id, &pipeline);
            }
            return;
        }
        step.finished += 1;
        match failure_of(record) {
            Some(code) => {
                step.failed_items += step.items_of(link.index);
                step.reason.get_or_insert(code);
            }
            None => {
                self.pin_files(tx, made.iter().map(|o| &o.id), 1, ts_ms);
                if let Some(step) = pipeline.steps.get_mut(&link.step) {
                    step.run_outputs.insert(link.index, made);
                    keep_result(step, link.index, result);
                }
            }
        }
        self.settle_step(tx, &mut pipeline, &link.step);
        self.settle_after_change(tx, &mut pipeline, ts_ms);
        put_json(tx, rows::CPIPE, &pipeline.id, &pipeline);
    }

    /// How many items of `step` may fail: none once the run is being stopped.
    fn allowance(pipeline: &PipelineRun, step: &StepState) -> u32 {
        if pipeline.cancel_requested || pipeline.stopping {
            0
        } else {
            step.fan.allowed
        }
    }

    /// Ends a running step whose runs are all final and either all started or no longer going to
    /// be: it succeeds only if every run was started and the failures stay within its allowance.
    /// Past the allowance it cancels the runs still going.
    fn settle_step(&mut self, tx: &mut StateTx<'_>, pipeline: &mut PipelineRun, name: &str) {
        let Some(step) = pipeline.steps.get(name) else {
            return;
        };
        if step.status != StepStatus::Running {
            return;
        }
        let allowed = Self::allowance(pipeline, step);
        let over = step.failed_items > allowed;
        let stopped = over || step.capped || pipeline.cancel_requested || pipeline.stopping;
        if over {
            self.flag_cancel(tx, &step.runs);
        }
        let started = step.runs.len();
        let all_final = step.finished as usize == started;
        if !all_final || !(started == step.fan.total as usize || stopped) {
            return;
        }
        let complete = started == step.fan.total as usize;
        let ok = complete && !over;
        let own_reason = if over && step.fan.allowed > 0 && allowed > 0 {
            Some(TOO_MANY_FAILURES.to_owned())
        } else {
            step.reason
                .clone()
                .or_else(|| (!complete).then(|| "cancelled".to_owned()))
        };
        if let Some(step) = pipeline.steps.get_mut(name) {
            step.status = if ok {
                StepStatus::Succeeded
            } else {
                StepStatus::Failed
            };
            step.reason = if ok { None } else { own_reason };
        }
    }

    /// [`Self::settle_step`] for every running step: a step between waves has no run left to
    /// settle it when the run is cancelled.
    fn settle_running_steps(&mut self, tx: &mut StateTx<'_>, pipeline: &mut PipelineRun) {
        let running: Vec<String> = pipeline
            .steps
            .iter()
            .filter(|(_, s)| s.status == StepStatus::Running)
            .map(|(n, _)| n.clone())
            .collect();
        for name in running {
            self.settle_step(tx, pipeline, &name);
        }
    }

    /// After a step changed: a failed step stops a fail-fast run (pending steps end cancelled,
    /// running ones are told to stop), then the run ends if every step is settled.
    fn settle_after_change(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &mut PipelineRun,
        ts_ms: u64,
    ) {
        let failed = pipeline
            .steps
            .values()
            .any(|s| s.status == StepStatus::Failed);
        if pipeline.fail_fast && failed && !pipeline.stopping && !pipeline.cancel_requested {
            pipeline.stopping = true;
            for step in pipeline.steps.values_mut() {
                if step.status == StepStatus::Pending {
                    step.status = StepStatus::Cancelled;
                }
            }
            self.flag_cancel(tx, pipeline.steps.values().flat_map(|s| &s.runs));
            self.settle_running_steps(tx, pipeline);
        }
        self.settle_pipeline(tx, pipeline, ts_ms);
    }

    /// Ends the run once every step is settled: releases its inputs and keeps its id among the
    /// finished ones, dropping the oldest records and their holds. The caller writes the record.
    fn settle_pipeline(&mut self, tx: &mut StateTx<'_>, pipeline: &mut PipelineRun, ts_ms: u64) {
        let settled = pipeline.steps.values().all(|s| s.status.is_settled());
        if pipeline.state != PipelineState::Running || !settled {
            return;
        }
        pipeline.state = if pipeline.steps.values().all(|s| s.status.is_ok()) {
            if pipeline.steps.values().any(|s| s.failed_items > 0) {
                PipelineState::SucceededWithFailures
            } else {
                PipelineState::Succeeded
            }
        } else if pipeline.cancel_requested {
            PipelineState::Cancelled
        } else {
            PipelineState::Failed
        };
        pipeline.end_ts_ms = Some(ts_ms);
        let files: BTreeSet<&String> = pipeline.inputs.values().flatten().collect();
        self.pin_files(tx, files, -1, ts_ms);
        let head = &mut self.state.component.head;
        let _was = head.active_pipelines.remove(&pipeline.id);
        head.finished_pipelines.push_back(pipeline.id.clone());
        while self.state.component.head.finished_pipelines.len() > FINISHED_PIPELINE_RUNS_KEPT {
            let Some(old) = self.state.component.head.finished_pipelines.pop_front() else {
                break;
            };
            if let Some(dropped) = get_json::<PipelineRun>(tx, rows::CPIPE, &old) {
                Self::drop_step_proofs(tx, &dropped);
                let outputs = dropped
                    .steps
                    .values()
                    .flat_map(StepState::outputs)
                    .map(|o| &o.id);
                self.pin_files(tx, outputs, -1, ts_ms);
            }
            tx.delete_hidden(rows::CPIPE, old.as_bytes());
        }
    }
}

/// Keeps `result` for run `index` while the step's results stay within [`MAX_STEP_RESULTS_BYTES`];
/// past it none are kept, so a reference to them fails whatever the order the runs ended in.
fn keep_result(step: &mut StepState, index: u32, result: &[u8]) {
    if result.is_empty() || result.len() > MAX_STEP_RESULT_BYTES || step.results_too_large {
        return;
    }
    let held: usize = step.run_results.values().map(|b| b.0.len()).sum();
    if held + result.len() > MAX_STEP_RESULTS_BYTES {
        step.run_results.clear();
        step.results_too_large = true;
    } else {
        let _prev = step.run_results.insert(index, Bytes(result.to_vec()));
    }
}

/// The id of run `index` of `step`, fixed by the pipeline run so a repeated start is no new run.
#[must_use]
pub fn run_id(pipeline: &str, step: &str, index: usize) -> String {
    format!("{pipeline}.{step}.{index}")
}

/// Why a start was not recorded.
enum Refusal {
    /// The app's active runs are full: try again later.
    NoRoom,
    /// The wave's first cache hit is gone: decide again.
    CacheGone,
    /// The step can never start; the code says why.
    Failed(&'static str),
}

/// A wave as apply took it: the ids of the indices it covers, and the cache hits among them.
struct Wave {
    ids: Vec<String>,
    hits: Vec<(u32, String, CacheEntry)>,
}

/// One index of a wave: a run to start or a cache hit.
enum Slot<'a> {
    Run(usize, &'a StepRun),
    Hit(usize, &'a CachedRun),
}

/// The wave's indices from `first` in order, each a hit or the next run; none if the hits and
/// runs do not tile the indices or a run's id is not its index's.
fn wave_slots<'a>(
    pipeline: &str,
    step: &str,
    first: usize,
    runs: &'a [StepRun],
    cached: &'a [CachedRun],
) -> Option<Vec<Slot<'a>>> {
    let n = runs.len() + cached.len();
    let mut hits: BTreeMap<usize, &CachedRun> = BTreeMap::new();
    for c in cached {
        let i = c.index as usize;
        if i < first || i >= first + n || hits.insert(i, c).is_some() {
            return None;
        }
    }
    let mut next = runs.iter();
    (first..first + n)
        .map(|i| match hits.get(&i) {
            Some(c) => Some(Slot::Hit(i, c)),
            None => next
                .next()
                .filter(|r| r.run == run_id(pipeline, step, i))
                .map(|r| Slot::Run(i, r)),
        })
        .collect()
}

/// Whether `known` is the run `entry` starts: everything the caller chose and the leader pinned.
fn same_start(known: &PipelineRun, entry: &PipelineStartEntry) -> bool {
    known.pipeline == entry.pipeline
        && known.version == entry.version
        && known.hash == entry.hash
        && known.compute_bundle == entry.compute_bundle
        && known.modules == entry.modules
        && known.inputs == entry.inputs
        && known.params == entry.params
        && known.caller == entry.caller
        && known.fail_fast == entry.fail_fast
        && known.cost == entry.cost
        && known.retry_of == entry.retry_of
        && known.limits == entry.limits
        && known
            .steps
            .keys()
            .eq(entry.steps.iter().collect::<BTreeSet<_>>())
}
