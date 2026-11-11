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

use super::runs::{FailedReason, FinalOutcome, RunCaps, RunInputRef, RunRecord, RunState};
use super::{host_answer, parse_hex32, Applied, FileMark};
use crate::app::{rows, AppMachine};
use crate::types::Caller;

/// Pipeline runs per app that are running at once.
pub const MAX_ACTIVE_PIPELINE_RUNS: usize = 8;
/// Finished pipeline runs whose records are kept, oldest dropped first.
pub const FINISHED_PIPELINE_RUNS_KEPT: usize = 64;
/// The most of a step's result a record keeps.
pub const MAX_STEP_RESULT_BYTES: usize = 64 * 1024;
/// Steps per pipeline run: the compiler's bound.
pub const MAX_PIPELINE_STEPS: usize = 64;
/// Compute runs one step decision may start.
pub const MAX_STEP_RUNS: usize = host::MAX_ACTIVE_RUNS;
/// The longest pipeline run id, step name or reason an entry may carry.
const MAX_NAME_BYTES: usize = 128;

pub const TOO_MANY_PIPELINE_RUNS: &str = "TOO_MANY_PIPELINE_RUNS";
pub const NO_ROOM: &str = "NO_ROOM";
pub(super) const KNOWN: &str = "the pipeline run exists";
pub(super) const BAD_START: &str = "the pipeline run is not well formed";
pub const INPUT_NOT_HELD: &str = "an input is not a file the app holds";
pub(super) const NOT_RUNNING: &str = "the pipeline run is not running";
pub(super) const NO_SUCH_STEP: &str = "the pipeline run has no such step";
pub(super) const DECIDED: &str = "the step was decided already";
pub(super) const CANCELLING: &str = "the pipeline run is being cancelled";

/// Where a pipeline run is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PipelineState {
    Running,
    Succeeded,
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
    /// The compute runs the step started, in index order.
    pub runs: Vec<String>,
    /// The files its runs made, pinned until the pipeline run's record is dropped.
    pub outputs: Vec<RunInputRef>,
    /// What its run returned, at most [`MAX_STEP_RESULT_BYTES`].
    pub result: Option<Bytes>,
    pub reason: Option<String>,
}

impl StepState {
    fn pending() -> Self {
        Self {
            status: StepStatus::Pending,
            runs: Vec::new(),
            outputs: Vec::new(),
            result: None,
            reason: None,
        }
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
    Start {
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

    /// `pipelineStep`: the first decision for a pending step. A start that does not fit changes
    /// nothing, so the step is decided again; one that can never work fails the step.
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
        if step.status != StepStatus::Pending {
            return no_op(DECIDED);
        }
        let ts_ms = entry.ts_ms;
        let c = &mut self.state.component;
        c.head.last_ts_ms = c.head.last_ts_ms.max(ts_ms);
        let (status, reason) = match &entry.decision {
            StepVerdict::Start { runs } => {
                if pipeline.cancel_requested {
                    return no_op(CANCELLING);
                }
                match self.start_step_runs(tx, &pipeline, &entry.step, runs, ts_ms) {
                    Ok(ids) => {
                        if let Some(step) = pipeline.steps.get_mut(&entry.step) {
                            step.runs = ids;
                        }
                        (StepStatus::Running, None)
                    }
                    Err(Refusal::NoRoom) => return no_op(NO_ROOM),
                    Err(Refusal::Failed(code)) => (StepStatus::Failed, Some(code.to_owned())),
                }
            }
            StepVerdict::Skipped => (StepStatus::Skipped, None),
            StepVerdict::UpstreamFailed => (StepStatus::UpstreamFailed, None),
            StepVerdict::Cancelled => (StepStatus::Cancelled, None),
            StepVerdict::Failed { reason } => (
                StepStatus::Failed,
                Some(reason.chars().take(MAX_NAME_BYTES).collect()),
            ),
        };
        if let Some(step) = pipeline.steps.get_mut(&entry.step) {
            step.status = status;
            step.reason = reason;
        }
        self.settle_pipeline(tx, &mut pipeline, ts_ms);
        Self::write_pipeline(tx, &pipeline);
        host_answer(index, true, "")
    }

    /// Records the runs of a step as queued compute runs, linked to it, and pins their inputs.
    fn start_step_runs(
        &mut self,
        tx: &mut StateTx<'_>,
        pipeline: &PipelineRun,
        step: &str,
        runs: &[StepRun],
        ts_ms: u64,
    ) -> Result<Vec<String>, Refusal> {
        let ids_ok = runs.iter().enumerate().all(|(i, r)| {
            r.run == format!("{}.{step}.{i}", pipeline.id)
                && !r.method.is_empty()
                && r.args.0.len() <= host::MAX_COMPUTE_ARGS_BYTES
                && r.inputs.len() <= host::MAX_RUN_INPUTS
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
        for (r, inputs) in records {
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
                pipeline: Some(super::runs::PipelineLink {
                    run: pipeline.id.clone(),
                    step: step.to_owned(),
                }),
            };
            Self::write_run(tx, &record);
            let _new = self.state.component.head.active_runs.insert(r.run.clone());
            self.pin_inputs(tx, &inputs, 1, ts_ms);
        }
        Ok(runs.iter().map(|r| r.run.clone()).collect())
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
        for run in pipeline.steps.values().flat_map(|s| &s.runs) {
            if !self.state.component.head.active_runs.contains(run) {
                continue;
            }
            if let Some(mut record) = Self::read_run(tx, run) {
                record.cancel_requested = true;
                Self::write_run(tx, &record);
            }
        }
        Self::write_pipeline(tx, &pipeline);
        host_answer(index, true, "")
    }

    /// A compute run that belongs to a step has ended: keep its outputs and result, and settle
    /// the step once every run of it is final. `record` is the run as it now stands.
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
        let Some(step) = pipeline.steps.get(&link.step) else {
            return;
        };
        if step.status != StepStatus::Running {
            return;
        }
        let runs = step.runs.clone();
        let mut failure = None;
        let mut all_final = true;
        for id in &runs {
            let run = if *id == record.run {
                Some(record.clone())
            } else {
                Self::read_run(tx, id)
            };
            match run {
                Some(r) if r.state.is_active() => all_final = false,
                Some(r) => failure = failure.or_else(|| failure_of(&r)),
                None => failure = failure.or_else(|| Some("lost".to_owned())),
            }
        }
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
            step.outputs.extend(made);
            if !result.is_empty() && result.len() <= MAX_STEP_RESULT_BYTES {
                step.result = Some(Bytes(result.to_vec()));
            }
            if all_final {
                step.status = if failure.is_none() {
                    StepStatus::Succeeded
                } else {
                    StepStatus::Failed
                };
                step.reason = failure;
            }
        }
        self.settle_pipeline(tx, &mut pipeline, ts_ms);
        Self::write_pipeline(tx, &pipeline);
    }

    /// Ends the run once every step is settled: releases its inputs and keeps its id among the
    /// finished ones, dropping the oldest records and their holds. The caller writes the record.
    fn settle_pipeline(&mut self, tx: &mut StateTx<'_>, pipeline: &mut PipelineRun, ts_ms: u64) {
        let settled = pipeline.steps.values().all(|s| s.status.is_settled());
        if pipeline.state != PipelineState::Running || !settled {
            return;
        }
        pipeline.state = if pipeline.steps.values().all(|s| s.status.is_ok()) {
            PipelineState::Succeeded
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
                    .flat_map(|s| &s.outputs)
                    .map(|o| &o.id);
                self.pin_files(tx, outputs, -1, ts_ms);
            }
            tx.delete_hidden(rows::CPIPE, old.as_bytes());
        }
    }
}

/// Why a start was not recorded.
enum Refusal {
    /// The app's active runs are full: try again later.
    NoRoom,
    /// The step can never start; the code says why.
    Failed(&'static str),
}
