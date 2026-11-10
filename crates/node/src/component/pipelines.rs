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
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::runs::{
    FailedReason, FinalOutcome, PipelineLink, RunCaps, RunInputRef, RunRecord, RunState,
};
use super::{host_answer, parse_hex32, Applied, FileMark};
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
/// Steps per pipeline run: the compiler's bound.
pub const MAX_PIPELINE_STEPS: usize = 64;
/// Compute runs one step may have in all, at one item each.
pub const MAX_STEP_TOTAL_RUNS: usize = calimero_pipeline::MAX_ITEMS as usize;
/// Compute runs one step decision may start.
pub const MAX_STEP_RUNS: usize = host::MAX_ACTIVE_RUNS;
/// The longest pipeline run id, step name or reason an entry may carry.
const MAX_NAME_BYTES: usize = 128;

pub const TOO_MANY_PIPELINE_RUNS: &str = "TOO_MANY_PIPELINE_RUNS";
pub const TOO_MANY_FAILURES: &str = "TOO_MANY_FAILURES";
pub const NO_ROOM: &str = "NO_ROOM";
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
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

/// One step of a pipeline run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl StepState {
    /// A step that has not started.
    #[must_use]
    pub fn pending() -> Self {
        Self {
            status: StepStatus::Pending,
            runs: Vec::new(),
            run_outputs: BTreeMap::new(),
            run_results: BTreeMap::new(),
            results_too_large: false,
            reason: None,
            fan: FanOut::default(),
            finished: 0,
            failed_items: 0,
        }
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
    pub owner: String,
    /// Input name to file ids.
    pub inputs: BTreeMap<String, Vec<String>>,
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
    pub owner: String,
    pub steps: Vec<String>,
    pub inputs: BTreeMap<String, Vec<String>>,
    pub params: BTreeMap<String, Value>,
    pub caller: Caller,
    pub payer: String,
    #[serde(default)]
    pub fail_fast: bool,
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
}

/// What the leader decided for a pending step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StepVerdict {
    /// The step's first wave of runs; `fan` says how many it will have in all.
    Start {
        runs: Vec<StepRun>,
        fan: FanOut,
    },
    /// The next runs of a running fan-out step, from the first index it has not started.
    More {
        runs: Vec<StepRun>,
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

fn reason_code(reason: FailedReason) -> String {
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

    fn read_pipeline(tx: &StateTx<'_>, run: &str) -> Option<PipelineRun> {
        let raw = tx.get_hidden(rows::CPIPE, run.as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }

    fn write_pipeline(tx: &mut StateTx<'_>, record: &PipelineRun) {
        match serde_json::to_vec(record) {
            Ok(row) => tx.put_hidden(rows::CPIPE, record.id.as_bytes(), &row),
            Err(e) => tx.fail(e),
        }
    }

    /// The marks of `ids` as the app's sealed files hold them; none when one is not held.
    fn file_refs<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> Option<Vec<RunInputRef>> {
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
    fn pin_files<'a>(
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
            && entry.steps.len() <= MAX_PIPELINE_STEPS
            && entry.steps.iter().all(|s| name_fits(s));
        if !well_formed {
            return no_op(BAD_START);
        }
        if head.active_pipelines.contains(&entry.id) || Self::read_pipeline(tx, &entry.id).is_some()
        {
            return no_op(KNOWN);
        }
        if head.active_pipelines.len() >= MAX_ACTIVE_PIPELINE_RUNS {
            return no_op(TOO_MANY_PIPELINE_RUNS);
        }
        let files: BTreeSet<&String> = entry.inputs.values().flatten().collect();
        if self.file_refs(files.iter().copied()).is_none() {
            return no_op(INPUT_NOT_HELD);
        }
        let record = PipelineRun {
            id: entry.id.clone(),
            pipeline: entry.pipeline.clone(),
            version: entry.version.clone(),
            hash: entry.hash.clone(),
            compute_bundle: entry.compute_bundle.clone(),
            owner: entry.owner.clone(),
            inputs: entry.inputs.clone(),
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
                .map(|s| (s.clone(), StepState::pending()))
                .collect(),
            end_ts_ms: None,
        };
        Self::write_pipeline(tx, &record);
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
        let Some(mut pipeline) = Self::read_pipeline(tx, &entry.run) else {
            return no_op(NOT_RUNNING);
        };
        if pipeline.state != PipelineState::Running {
            return no_op(NOT_RUNNING);
        }
        let Some(step) = pipeline.steps.get(&entry.step) else {
            return no_op(NO_SUCH_STEP);
        };
        let between_waves = step.runs.len() < step.fan.total as usize;
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
        let (status, reason) = match &entry.decision {
            StepVerdict::Start { runs, fan } => {
                if pipeline.cancel_requested || pipeline.stopping {
                    return no_op(CANCELLING);
                }
                let started = if fan.well_formed() && runs.len() <= fan.total as usize {
                    self.start_step_runs(tx, &pipeline, &entry.step, runs, ts_ms)
                } else {
                    Err(Refusal::Failed("BAD_START"))
                };
                match started {
                    Ok(ids) => {
                        if let Some(step) = pipeline.steps.get_mut(&entry.step) {
                            step.runs = ids;
                            step.fan = *fan;
                        }
                        (StepStatus::Running, None)
                    }
                    Err(Refusal::NoRoom) => return no_op(NO_ROOM),
                    Err(Refusal::Failed(code)) => (StepStatus::Failed, Some(code.to_owned())),
                }
            }
            StepVerdict::More { runs } => {
                if pipeline.cancel_requested || pipeline.stopping {
                    return no_op(STEP_STOPPING);
                }
                match self.start_more_runs(tx, &pipeline, &entry.step, runs, ts_ms) {
                    Ok(ids) => {
                        if let Some(step) = pipeline.steps.get_mut(&entry.step) {
                            step.runs.extend(ids);
                        }
                        (StepStatus::Running, None)
                    }
                    Err(Refusal::NoRoom) => return no_op(NO_ROOM),
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
        self.settle_after_change(tx, &mut pipeline, ts_ms);
        Self::write_pipeline(tx, &pipeline);
        host_answer(index, true, "")
    }

    /// The first wave of a step: indices from 0, at most [`MAX_STEP_RUNS`].
    fn start_step_runs(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &PipelineRun,
        step: &str,
        runs: &[StepRun],
        ts_ms: u64,
    ) -> Result<Vec<String>, Refusal> {
        self.start_runs_from(tx, pipeline, step, 0, runs, ts_ms)
    }

    /// The next wave of a running fan-out: it continues where the step's runs end, never past
    /// the step's total and never once its failures have passed what it tolerates.
    fn start_more_runs(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &PipelineRun,
        step: &str,
        runs: &[StepRun],
        ts_ms: u64,
    ) -> Result<Vec<String>, Refusal> {
        let Some(state) = pipeline.steps.get(step) else {
            return Err(Refusal::Failed(NOT_NEXT));
        };
        if state.failed_items > state.fan.allowed {
            return Err(Refusal::Failed(STEP_STOPPING));
        }
        let first = state.runs.len();
        if runs.is_empty() || first + runs.len() > state.fan.total as usize {
            return Err(Refusal::Failed(NOT_NEXT));
        }
        let next = runs
            .iter()
            .enumerate()
            .all(|(i, r)| r.run == run_id(&pipeline.id, step, first + i));
        if !next {
            return Err(Refusal::Failed(NOT_NEXT));
        }
        self.start_runs_from(tx, pipeline, step, first, runs, ts_ms)
    }

    /// Records the runs of a step as queued compute runs, linked to it, and pins their inputs.
    fn start_runs_from(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &PipelineRun,
        step: &str,
        first: usize,
        runs: &[StepRun],
        ts_ms: u64,
    ) -> Result<Vec<String>, Refusal> {
        let ids_ok = runs.iter().enumerate().all(|(i, r)| {
            r.run == run_id(&pipeline.id, step, first + i)
                && !r.method.is_empty()
                && r.args.0.len() <= host::MAX_COMPUTE_ARGS_BYTES
                && r.inputs.len() <= host::MAX_PIPELINE_RUN_INPUTS
        });
        if runs.is_empty() || runs.len() > MAX_STEP_RUNS || !ids_ok {
            return Err(Refusal::Failed("BAD_START"));
        }
        let head = &self.state.component.head;
        if head.active_runs.len() + runs.len() > host::MAX_ACTIVE_RUNS {
            return Err(Refusal::NoRoom);
        }
        if runs
            .iter()
            .any(|r| head.active_runs.contains(&r.run) || Self::read_run(tx, &r.run).is_some())
        {
            return Err(Refusal::Failed("RUN_EXISTS"));
        }
        let mut records = Vec::new();
        for r in runs {
            let Some(inputs) = self.file_refs(&r.inputs) else {
                return Err(Refusal::Failed("INPUT_NOT_HELD"));
            };
            records.push((r, inputs));
        }
        let bundle = self
            .state
            .bundle
            .clone()
            .unwrap_or_else(|| pipeline.compute_bundle.clone());
        for (i, (r, inputs)) in records.into_iter().enumerate() {
            let record = RunRecord {
                run: r.run.clone(),
                method: r.method.clone(),
                args: r.args.clone(),
                args_hash: hex::encode(calimero_identity::sha256(&r.args.0)),
                inputs: inputs.clone(),
                caps: r.caps,
                result_method: String::new(),
                bundle: bundle.clone(),
                compute_bundle: Some(pipeline.compute_bundle.clone()),
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
                    index: u32::try_from(first + i).unwrap_or(u32::MAX),
                }),
            };
            Self::write_run(tx, &record);
            let _new = self.state.component.head.active_runs.insert(r.run.clone());
            self.pin_inputs(tx, &inputs, 1, ts_ms);
        }
        Ok(runs.iter().map(|r| r.run.clone()).collect())
    }

    /// Tells the active runs of `runs` to stop.
    fn flag_cancel<'a>(&self, tx: &mut StateTx<'_>, runs: impl IntoIterator<Item = &'a String>) {
        for run in runs {
            if !self.state.component.head.active_runs.contains(run) {
                continue;
            }
            match Self::read_run(tx, run) {
                Some(mut record) if !record.cancel_requested => {
                    record.cancel_requested = true;
                    Self::write_run(tx, &record);
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
        let Some(mut pipeline) = Self::read_pipeline(tx, &entry.run) else {
            return host_answer(index, false, NOT_RUNNING);
        };
        if pipeline.state != PipelineState::Running {
            return host_answer(index, false, NOT_RUNNING);
        }
        let c = &mut self.state.component;
        c.head.last_ts_ms = c.head.last_ts_ms.max(entry.ts_ms);
        pipeline.cancel_requested = true;
        self.flag_cancel(tx, pipeline.steps.values().flat_map(|s| &s.runs));
        self.settle_running_steps(tx, &mut pipeline, entry.ts_ms);
        self.settle_pipeline(tx, &mut pipeline, entry.ts_ms);
        Self::write_pipeline(tx, &pipeline);
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
        let Some(mut pipeline) = Self::read_pipeline(tx, &link.run) else {
            return;
        };
        let Some(step) = pipeline.steps.get_mut(&link.step) else {
            return;
        };
        if step.status != StepStatus::Running {
            return;
        }
        step.finished += 1;
        match failure_of(record) {
            Some(code) => {
                step.failed_items += step.items_of(link.index);
                step.reason.get_or_insert(code);
            }
            None => {
                let made: Vec<RunInputRef> = outputs
                    .iter()
                    .map(|o| RunInputRef {
                        id: o.id.clone(),
                        size: o.size,
                        sha256: hex::encode(&o.sha256.0),
                    })
                    .collect();
                self.pin_files(tx, made.iter().map(|o| &o.id), 1, ts_ms);
                if let Some(step) = pipeline.steps.get_mut(&link.step) {
                    step.run_outputs.insert(link.index, made);
                    keep_result(step, link.index, result);
                }
            }
        }
        self.settle_step(tx, &mut pipeline, &link.step);
        self.settle_after_change(tx, &mut pipeline, ts_ms);
        Self::write_pipeline(tx, &pipeline);
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
        let stopped = over || pipeline.cancel_requested || pipeline.stopping;
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
    fn settle_running_steps(&mut self, tx: &mut StateTx<'_>, pipeline: &mut PipelineRun, _ts: u64) {
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
            self.settle_running_steps(tx, pipeline, ts_ms);
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
            if let Some(dropped) = Self::read_pipeline(tx, &old) {
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
    /// The step can never start; the code says why.
    Failed(&'static str),
}
