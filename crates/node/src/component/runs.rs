//! Compute runs in a component app's state: the record of each run, the two entries that move
//! it (`computeAttempt`, `computeFinal`) and how every replica applies them.
//!
//! A run is started by a call's outcome (`Outcome::compute_starts`), so it exists on every
//! replica or on none. Each entry below is checked against the record alone, so all replicas
//! reach the same state: the first final entry of a run wins, a job outcome must carry a report
//! signed by the job key the run's latest attempt recorded, and anything else changes nothing.
//! Records are hidden rows ([`rows::CRUN`]), never keys the app sees; the head keeps the active
//! ids and the last [`FINISHED_RUNS_KEPT`] finished ones, whose records are then deleted.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use calimero_app_host as host;
use calimero_log::{Bytes, GroupState, StateTx};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Applied, FileMark, Outcome};
use crate::app::{rows, AppMachine};
use crate::job_wire::{hash_bytes, verify_report, FailReason, Report, ReportStatus, ReportUsage};
use crate::types::Caller;

/// Attempts a run gets: the first and the restarts after a lost job node.
pub const MAX_ATTEMPTS: u32 = 3;
/// Finished runs whose records are kept, oldest expired first.
pub const FINISHED_RUNS_KEPT: usize = 256;
/// How long past its wall cap the leader waits for a run's report before ending it.
pub const DEADLINE_GRACE: Duration = Duration::from_secs(60);

const NOT_ACTIVE: &str = "run is not active";
const ALREADY_FINAL: &str = "already final";
const CANCEL_REQUESTED: &str = "cancel was requested";
const OUT_OF_ORDER: &str = "attempt is not the next one";
const TOO_MANY_ATTEMPTS: &str = "no attempts left";
const BAD_ATTEMPT_KEYS: &str = "job key or assignment digest is not a hash";
const STALE_ATTEMPT: &str = "stale attempt";
const NEEDS_REPORT: &str = "a job outcome needs a signed report";
const HAS_REPORT: &str = "a leader outcome carries no report";
const FOREIGN_REPORT: &str = "report is for another run";
const BAD_SIGNATURE: &str = "report not signed by the assigned job node";
const OUTCOME_MISMATCH: &str = "report does not match the outcome";
const OUTPUTS_MISMATCH: &str = "outputs differ from the report";
const USAGE_MISMATCH: &str = "usage differs from the report";
const OUTPUT_TAKEN: &str = "output is already a file";
const LEADER_OUTPUTS: &str = "a leader outcome has no outputs";
const NOT_CANCELLED: &str = "cancel was not requested";
const ATTEMPTS_LEFT: &str = "the run has attempts left";
const NOT_RUNNING: &str = "the run was never assigned";
const LEADER_USAGE: &str = "a leader outcome is not charged";
const OUTPUTS_TOO_LARGE: &str = "outputs are over the limits of a run";
const RESULT_TOO_LARGE: &str = "the result is over the limit of a run";
const USAGE_OVER_CAPS: &str = "usage is over what the run's caps allow";
const ASSIGNED: &str = "the run was assigned";
const STALE_BASE: &str = "conflict: retry";

/// Where a run is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl RunState {
    /// Whether the run can still change.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// What a run may use, copied from the method's ABI when it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCaps {
    pub fuel: u64,
    pub memory_mib: u32,
    pub wall_secs: u32,
}

/// An input file of a run, as its mark recorded it when the run started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInputRef {
    pub id: String,
    pub size: u64,
    /// Hex SHA-256 of the plaintext.
    pub sha256: String,
}

/// A run a call started, as its outcome carries it: the node fills `owner` from the system
/// state when the leader builds the entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputeStart {
    pub run: String,
    pub method: String,
    pub args: Bytes,
    pub inputs: Vec<RunInputRef>,
    pub caps: RunCaps,
    pub result_method: String,
    #[serde(default)]
    pub owner: String,
}

impl From<host::StartedRun> for ComputeStart {
    fn from(run: host::StartedRun) -> Self {
        Self {
            run: run.run,
            method: run.method,
            args: Bytes(run.args),
            inputs: run
                .inputs
                .into_iter()
                .map(|i| RunInputRef {
                    id: i.id,
                    size: i.size,
                    sha256: hex::encode(i.sha256),
                })
                .collect(),
            caps: RunCaps {
                fuel: run.caps.fuel,
                memory_mib: run.caps.memory_mib,
                wall_secs: run.caps.wall_secs,
            },
            result_method: run.caps.result,
            owner: String::new(),
        }
    }
}

/// One assignment of a run to a job node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub attempt: u32,
    pub node: u64,
    /// Hex of the job node's signing key, as attested when it was chosen.
    pub job_key: String,
    /// Hex SHA-256 of the exact assignment the leader built; the report binds it.
    pub assignment_digest: String,
    pub ts_ms: u64,
}

/// How a run ended and what it left.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Final {
    pub attempt: u32,
    pub outcome: FinalOutcome,
    pub outputs: Vec<FileMark>,
    pub usage: ReportUsage,
    pub ts_ms: u64,
    /// The `id_term` and `id_seq` of the entry that ended the run: which entry it was.
    #[serde(default)]
    pub id_term: u64,
    #[serde(default)]
    pub id_seq: u64,
}

/// The record of one run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run: String,
    pub method: String,
    pub args: Bytes,
    /// Hex SHA-256 of `args`.
    pub args_hash: String,
    pub inputs: Vec<RunInputRef>,
    pub caps: RunCaps,
    pub result_method: String,
    /// The bundle that ran when the run started: the one it executes whatever is installed later.
    pub bundle: String,
    pub caller: Caller,
    pub owner: String,
    pub start_ts_ms: u64,
    pub state: RunState,
    pub cancel_requested: bool,
    pub attempts: Vec<Attempt>,
    #[serde(rename = "final")]
    pub final_: Option<Final>,
}

/// Why a run failed. The first seven are a job's own outcome and need its signed report; the
/// rest are the leader's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailedReason {
    Fuel,
    Memory,
    Time,
    Trap,
    AppError,
    Integrity,
    /// The job node stopped at the leader's cancel.
    Cancelled,
    /// Every attempt was lost.
    Lost,
    /// Acquiring a place was refused for good before any attempt.
    Refused,
    /// No report by the deadline.
    Timeout,
    /// The outputs would pass the payer's stored-file quota.
    Quota,
}

impl From<FailReason> for FailedReason {
    fn from(reason: FailReason) -> Self {
        match reason {
            FailReason::Fuel => Self::Fuel,
            FailReason::Memory => Self::Memory,
            FailReason::Time => Self::Time,
            FailReason::Trap => Self::Trap,
            FailReason::AppError => Self::AppError,
            FailReason::Integrity => Self::Integrity,
            FailReason::Cancelled => Self::Cancelled,
        }
    }
}

impl FailedReason {
    /// The reason a job report must carry for this one; none for the leader's.
    const fn job(self) -> Option<FailReason> {
        match self {
            Self::Fuel => Some(FailReason::Fuel),
            Self::Memory => Some(FailReason::Memory),
            Self::Time => Some(FailReason::Time),
            Self::Trap => Some(FailReason::Trap),
            Self::AppError => Some(FailReason::AppError),
            Self::Integrity => Some(FailReason::Integrity),
            Self::Cancelled => Some(FailReason::Cancelled),
            Self::Lost | Self::Timeout | Self::Refused | Self::Quota => None,
        }
    }
}

/// How a final entry ends its run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FinalOutcome {
    Done,
    Failed { reason: FailedReason },
    Cancelled,
}

impl FinalOutcome {
    /// The state the run ends in.
    #[must_use]
    pub const fn state(self) -> RunState {
        match self {
            Self::Done => RunState::Done,
            Self::Failed {
                reason: FailedReason::Cancelled,
            }
            | Self::Cancelled => RunState::Cancelled,
            Self::Failed { .. } => RunState::Failed,
        }
    }

    /// Whether the run was cancelled, by the leader or as the job node's report says.
    const fn is_cancel(self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Failed {
                    reason: FailedReason::Cancelled
                }
        )
    }

    /// The reason a job report must carry, when the outcome is a job's: a cancel is the job
    /// node's when it brings its report, the leader's when it does not.
    fn job_reason(self, has_report: bool) -> Option<Option<FailReason>> {
        match self {
            Self::Done => Some(None),
            Self::Failed { reason } => reason.job().map(Some),
            Self::Cancelled => has_report.then_some(Some(FailReason::Cancelled)),
        }
    }
}

/// `computeAttempt`: the leader assigned the run to a job node.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptEntry {
    pub run: String,
    pub attempt: u32,
    pub node: u64,
    pub job_key: String,
    pub assignment_digest: String,
    pub ts_ms: u64,
}

/// `computeFinal`: the run ended; `result` is what its result method did.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalEntry {
    pub run: String,
    pub attempt: u32,
    pub outcome: FinalOutcome,
    #[serde(default)]
    pub report: Option<Report>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub outputs: Vec<FileMark>,
    pub usage: ReportUsage,
    #[serde(default)]
    pub result: Outcome,
    pub base_index: u64,
    pub ts_ms: u64,
    pub id_term: u64,
    pub id_seq: u64,
}

/// The app's active runs as the host core asks for them, read from the group's rows when asked.
pub struct NodeRuns {
    state: GroupState,
    ids: Vec<String>,
}

impl host::Runs for NodeRuns {
    fn active(&self) -> Result<Vec<host::ActiveRun>, String> {
        self.ids
            .iter()
            .map(|id| {
                let raw = self
                    .state
                    .get_hidden(rows::CRUN, id.as_bytes())
                    .ok_or_else(|| format!("no record of run {id}"))?;
                let record: RunRecord =
                    serde_json::from_slice(&raw).map_err(|e| format!("run {id}: {e}"))?;
                Ok(host::ActiveRun {
                    run: record.run,
                    method: record.method,
                    args_sha256: calimero_identity::sha256(&record.args.0).to_vec(),
                    inputs: record.inputs.into_iter().map(|i| i.id).collect(),
                    cancel_requested: record.cancel_requested,
                })
            })
            .collect()
    }
}

impl Outcome {
    /// Names the owner of the app for the runs this outcome starts.
    pub fn name_owner(&mut self, owner: &str) {
        for start in &mut self.compute_starts {
            owner.clone_into(&mut start.owner);
        }
    }
}

impl AppMachine {
    /// Whether `file` is an input of an active run; a record that cannot be read counts as one,
    /// so a damaged row never lets an input go.
    pub(super) fn is_run_input(&self, tx: &StateTx<'_>, file: &str) -> bool {
        self.state.component.head.active_runs.iter().any(|run| {
            Self::read_run(tx, run).is_none_or(|r| r.inputs.iter().any(|i| i.id == file))
        })
    }

    /// The record of `run`, active or among the last finished.
    #[must_use]
    pub fn run_record(&self, run: &str) -> Option<RunRecord> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CRUN, run.as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }

    /// The ids of the active runs, in order.
    #[must_use]
    pub fn active_runs(&self) -> Vec<String> {
        self.state
            .component
            .head
            .active_runs
            .iter()
            .cloned()
            .collect()
    }

    /// What a host core running on this state asks about the app's runs; `ending` is a run
    /// whose result method this is: it is left out, so its slot is free for what the method starts.
    #[must_use]
    pub fn runs_provider(&self, ending: Option<&str>) -> Arc<dyn host::Runs> {
        match self.state.kv.group() {
            Some(state) => Arc::new(NodeRuns {
                state: state.clone(),
                ids: self
                    .active_runs()
                    .into_iter()
                    .filter(|id| Some(id.as_str()) != ending)
                    .collect(),
            }),
            None => Arc::new(host::NoRuns),
        }
    }

    fn read_run(tx: &StateTx<'_>, run: &str) -> Option<RunRecord> {
        let raw = tx.get_hidden(rows::CRUN, run.as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }

    fn write_run(tx: &mut StateTx<'_>, record: &RunRecord) {
        match serde_json::to_vec(record) {
            Ok(row) => tx.put_hidden(rows::CRUN, record.run.as_bytes(), &row),
            Err(e) => tx.fail(e),
        }
    }

    /// Records the runs a call started and flags the ones it cancelled. A start past the active
    /// cap, or of an id that has a record, is dropped.
    pub(super) fn apply_run_changes(
        &mut self,
        tx: &mut StateTx<'_>,
        ts_ms: u64,
        caller: &Caller,
        outcome: &Outcome,
    ) {
        for start in &outcome.compute_starts {
            let head = &mut self.state.component.head;
            let known =
                head.active_runs.contains(&start.run) || Self::read_run(tx, &start.run).is_some();
            if known || head.active_runs.len() >= host::MAX_ACTIVE_RUNS {
                continue;
            }
            let Some(bundle) = self.state.bundle.clone() else {
                continue;
            };
            let record = RunRecord {
                run: start.run.clone(),
                method: start.method.clone(),
                args: start.args.clone(),
                args_hash: hex::encode(calimero_identity::sha256(&start.args.0)),
                inputs: start.inputs.clone(),
                caps: start.caps,
                result_method: start.result_method.clone(),
                bundle,
                caller: caller.clone(),
                owner: start.owner.clone(),
                start_ts_ms: ts_ms,
                state: RunState::Queued,
                cancel_requested: false,
                attempts: Vec::new(),
                final_: None,
            };
            Self::write_run(tx, &record);
            let _new = self
                .state
                .component
                .head
                .active_runs
                .insert(start.run.clone());
        }
        for run in &outcome.compute_cancels {
            if !self.state.component.head.active_runs.contains(run) {
                continue;
            }
            if let Some(mut record) = Self::read_run(tx, run) {
                record.cancel_requested = true;
                Self::write_run(tx, &record);
            }
        }
    }

    /// `computeAttempt`: the next attempt of an active run that was not cancelled.
    pub(super) fn apply_compute_attempt(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        entry: &AttemptEntry,
    ) -> Applied {
        let no_op = |reason| Applied::refused(index, reason);
        let Some(mut record) = Self::read_run(tx, &entry.run) else {
            return no_op(NOT_ACTIVE);
        };
        if !record.state.is_active() {
            return no_op(NOT_ACTIVE);
        }
        if record.cancel_requested {
            return no_op(CANCEL_REQUESTED);
        }
        if entry.attempt > MAX_ATTEMPTS {
            return no_op(TOO_MANY_ATTEMPTS);
        }
        if usize::try_from(entry.attempt).ok() != Some(record.attempts.len() + 1) {
            return no_op(OUT_OF_ORDER);
        }
        if hash_bytes(&entry.job_key).is_err() || hash_bytes(&entry.assignment_digest).is_err() {
            return no_op(BAD_ATTEMPT_KEYS);
        }
        record.state = RunState::Running;
        record.attempts.push(Attempt {
            attempt: entry.attempt,
            node: entry.node,
            job_key: entry.job_key.clone(),
            assignment_digest: entry.assignment_digest.clone(),
            ts_ms: entry.ts_ms,
        });
        Self::write_run(tx, &record);
        Applied::answered(index, Value::Null)
    }

    /// `computeFinal`: the first valid final ends the run. A refusal changes nothing and answers
    /// its reason; only a stale `baseIndex` is a conflict, which the leader retries.
    pub(super) fn apply_compute_final(
        &mut self,
        tx: &mut StateTx<'_>,
        index: u64,
        entry: &FinalEntry,
    ) -> Applied {
        let Some(mut record) = Self::read_run(tx, &entry.run) else {
            return Applied::refused(index, NOT_ACTIVE);
        };
        if let Err(reason) = self.check_final(&record, entry) {
            return Applied::refused(index, reason);
        }
        if entry.base_index != self.state.last_index {
            return Applied::conflict(index, STALE_BASE);
        }
        record.state = entry.outcome.state();
        record.final_ = Some(Final {
            attempt: entry.attempt,
            outcome: entry.outcome,
            outputs: entry.outputs.clone(),
            usage: entry.usage,
            ts_ms: entry.ts_ms,
            id_term: entry.id_term,
            id_seq: entry.id_seq,
        });
        Self::write_run(tx, &record);
        self.finish_run(tx, &entry.run);
        let marks = Outcome {
            files: entry.outputs.clone(),
            ..Outcome::default()
        };
        let _freed = self.apply_files(tx, index, &marks);
        self.apply_component_outcome(
            tx,
            index,
            entry.base_index,
            entry.ts_ms,
            &entry.result,
            &Caller::System,
        )
    }

    /// Takes `run` out of the active set and keeps its record among the last finished ones.
    fn finish_run(&mut self, tx: &mut StateTx<'_>, run: &str) {
        let head = &mut self.state.component.head;
        let _was = head.active_runs.remove(run);
        head.finished_runs.push_back(run.to_owned());
        while head.finished_runs.len() > FINISHED_RUNS_KEPT {
            if let Some(old) = head.finished_runs.pop_front() {
                tx.delete_hidden(rows::CRUN, old.as_bytes());
            }
        }
    }

    /// Whether `entry` may end `record`, judged on the record, the entry and the marks alone.
    fn check_final(&self, record: &RunRecord, entry: &FinalEntry) -> Result<(), &'static str> {
        if !record.state.is_active() {
            return Err(ALREADY_FINAL);
        }
        if usize::try_from(entry.attempt).ok() != Some(record.attempts.len()) {
            return Err(STALE_ATTEMPT);
        }
        Self::check_outputs(&entry.outputs)?;
        let marks = &self.state.component.files;
        let mut seen = BTreeSet::new();
        if entry
            .outputs
            .iter()
            .any(|o| marks.contains_key(&o.id) || !seen.insert(&o.id))
        {
            return Err(OUTPUT_TAKEN);
        }
        if entry.outcome.is_cancel() && !record.cancel_requested {
            return Err(NOT_CANCELLED);
        }
        match entry.outcome.job_reason(entry.report.is_some()) {
            Some(reason) => self.check_job_final(record, entry, reason),
            None => Self::check_leader_final(record, entry),
        }
    }

    /// The outputs fit what a run may write: how many, how large each, how large together.
    fn check_outputs(outputs: &[FileMark]) -> Result<(), &'static str> {
        let sizes: Vec<u64> = outputs.iter().map(|o| o.size).collect();
        if Self::outputs_fit(&sizes) {
            Ok(())
        } else {
            Err(OUTPUTS_TOO_LARGE)
        }
    }

    /// Whether outputs of these `sizes` are within what a run may write.
    #[must_use]
    pub fn outputs_fit(sizes: &[u64]) -> bool {
        let total = sizes.iter().try_fold(0u64, |sum, s| sum.checked_add(*s));
        sizes.len() <= host::MAX_NEW_FILES
            && sizes.iter().all(|s| *s <= host::MAX_FILE_BYTES)
            && total.is_some_and(|t| t <= host::MAX_COMPUTE_FILE_BYTES)
    }

    /// What a run measured cannot pass what its caps allow.
    pub fn check_usage(caps: &RunCaps, usage: &ReportUsage) -> Result<(), &'static str> {
        // CPU is one thread's clock and a report may come up to the grace after the wall cap.
        let micros =
            (u128::from(caps.wall_secs) + u128::from(DEADLINE_GRACE.as_secs())) * 1_000_000;
        let gib_cap = u128::from(caps.memory_mib) * micros / 1024;
        if usage.fuel > caps.fuel
            || u128::from(usage.cpu_micros) > micros
            || u128::from(usage.gib_micros) > gib_cap
        {
            return Err(USAGE_OVER_CAPS);
        }
        Ok(())
    }

    /// A job's outcome: the report is the attempt's, signed by its job key, and the entry
    /// restates the report exactly.
    fn check_job_final(
        &self,
        record: &RunRecord,
        entry: &FinalEntry,
        reason: Option<FailReason>,
    ) -> Result<(), &'static str> {
        let (Some(report), Some(signature), Some(attempt)) =
            (&entry.report, &entry.signature, record.attempts.last())
        else {
            return Err(NEEDS_REPORT);
        };
        if report.app != self.app_id || report.run != entry.run || report.attempt != entry.attempt {
            return Err(FOREIGN_REPORT);
        }
        let (Ok(key), Ok(digest)) = (
            hash_bytes(&attempt.job_key),
            hash_bytes(&attempt.assignment_digest),
        ) else {
            return Err(BAD_SIGNATURE);
        };
        if !verify_report(&key, &digest, report, signature) {
            return Err(BAD_SIGNATURE);
        }
        let status = if reason.is_some() {
            ReportStatus::Failed
        } else {
            ReportStatus::Done
        };
        if report.status != status || report.reason != reason {
            return Err(OUTCOME_MISMATCH);
        }
        let same_outputs = report.outputs.len() == entry.outputs.len()
            && report.outputs.iter().zip(&entry.outputs).all(|(r, o)| {
                r.id == o.id && r.size == o.size && r.sha256 == hex::encode(&o.sha256.0)
            });
        if !same_outputs {
            return Err(OUTPUTS_MISMATCH);
        }
        if report.usage != entry.usage {
            return Err(USAGE_MISMATCH);
        }
        if report.result.len() > host::MAX_COMPUTE_RESULT_BYTES {
            return Err(RESULT_TOO_LARGE);
        }
        Self::check_usage(&record.caps, &entry.usage)
    }

    /// The leader's outcomes: no report, no outputs, and the state that makes them true.
    fn check_leader_final(record: &RunRecord, entry: &FinalEntry) -> Result<(), &'static str> {
        if entry.report.is_some() || entry.signature.is_some() {
            return Err(HAS_REPORT);
        }
        if !entry.outputs.is_empty() {
            return Err(LEADER_OUTPUTS);
        }
        match entry.outcome {
            FinalOutcome::Failed {
                reason: FailedReason::Lost,
            } if record.attempts.len() != usize::try_from(MAX_ATTEMPTS).unwrap_or(usize::MAX) => {
                Err(ATTEMPTS_LEFT)
            }
            FinalOutcome::Failed {
                reason: FailedReason::Timeout,
            } if record.state != RunState::Running => Err(NOT_RUNNING),
            FinalOutcome::Failed {
                reason: FailedReason::Refused,
            } if !record.attempts.is_empty() => Err(ASSIGNED),
            _ if entry.usage != ReportUsage::default() => Err(LEADER_USAGE),
            _ => Ok(()),
        }
    }
}
