//! The leader of a component app drives its compute runs: it asks the system group for a place,
//! picks a job node, commits the attempt and sends it, polls the job node, restarts a lost attempt
//! elsewhere, and ends the run with the job's signed report (or its own outcome) and the result
//! method's effects in one entry. All of it is judged from the replicated record and the system
//! state, so a new leader picks every run up where the last one left it; what it keeps in memory
//! is only when each job node last answered and what it staged of a final.

mod finish;
mod job_io;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use calimero_app_host::{Invocation, InvocationKind, TrapKind};
use calimero_log::{Group, GroupId, NodeId, Proposal};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, warn};

use super::files::Stored;
use super::{entry_bytes, Node, Ran, Reads};
use crate::app::{AppMachine, Status};
use crate::component::{
    Applied, Attempt, AttemptEntry, ComponentCmd, FailedReason, FileMark, FinalEntry, FinalOutcome,
    Outcome, RunRecord, RunState, DEADLINE_GRACE, MAX_ATTEMPTS,
};
use crate::error::ApiError;
use crate::job_wire::{assignment_digest, FailReason, Report, ReportStatus, ReportUsage};
use crate::node::{ViewAnswer, ViewRequest};
use crate::system::{RunSlot, SystemCmd};
use crate::types::AppRecord;

/// How often the leader polls a job node for a run's status.
pub const JOB_POLL: Duration = Duration::from_secs(2);
/// How long a job node may go without answering a poll before its attempt is lost.
pub const JOB_LOST_AFTER: Duration = Duration::from_secs(20);
/// How long a cancel keeps trying to stop the job before the run ends cancelled anyway: well past `CANCEL_WAIT`.
pub const CANCEL_DEADLINE: Duration = Duration::from_secs(20);
/// How many times a final that conflicted is rebuilt and proposed again within one round.
const FINAL_CONFLICT_TRIES: usize = 5;
/// Unconfirmed finals remembered per run.
const MAX_PROPOSED_KEPT: usize = 8;
/// How long a granted run may wait for a job node that can take it before it ends refused.
const QUEUED_RUN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The most outage time a deadline forgives: a leader whose reads keep failing still ends a run.
const MAX_OUTAGE_CREDIT: Duration = QUEUED_RUN_TIMEOUT;
/// How many times its healthy limit the lost and cancel clocks may run in wall time.
const WALL_FALLBACK_FACTOR: u32 = 10;
/// The reserved view that answers a run's record: the node's own, never an app's method.
pub const RUN_VIEW: &str = "__run";
/// How many runs a list answers when the caller names no limit.
const DEFAULT_RUNS_PAGE: usize = 20;
/// The most runs a list page holds.
const MAX_RUNS_PAGE: usize = 100;

/// The waits that end a run the leader cannot get an answer for; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct ComputeLimits {
    pub lost_after: Duration,
    pub deadline_grace: Duration,
    pub queued_timeout: Duration,
    pub cancel_deadline: Duration,
    pub outage_credit: Duration,
}

impl Default for ComputeLimits {
    fn default() -> Self {
        Self {
            lost_after: JOB_LOST_AFTER,
            deadline_grace: DEADLINE_GRACE,
            queued_timeout: QUEUED_RUN_TIMEOUT,
            cancel_deadline: CANCEL_DEADLINE,
            outage_credit: MAX_OUTAGE_CREDIT,
        }
    }
}

/// What the leader remembers of one run it drives, in memory only.
#[derive(Clone)]
pub(super) struct Watch {
    /// The leadership it was made in: a new term starts it over.
    term: u64,
    attempt: u32,
    /// The healthy time and the wall time at which the job node last answered; a new leader
    /// starts both at takeover.
    seen: Duration,
    seen_at: Instant,
    /// Time in rounds whose system read succeeded: an outage counts toward no clock.
    healthy: Duration,
    /// When this watch began, and the last round's time and whether its system read succeeded.
    born: Instant,
    ticked: Option<(Instant, bool)>,
    polled: Option<Instant>,
    /// A valid report of an attempt (its number first), kept until the run ends.
    held: Option<(u32, Report, String)>,
    /// A final is built or its outputs are staged and only the leader's own writes are pending.
    leader_wait: bool,
    /// The healthy and wall time at which this leader first saw the cancel.
    cancel_seen: Option<(Duration, Instant)>,
    /// The job node holds this attempt's assignment, as far as this leader knows.
    sent: bool,
    progress: Option<(u8, String)>,
    /// The outputs of the attempt's report, stored on the replicas and charged, not yet committed.
    staged: Option<Vec<FileMark>>,
    /// The run ended: nothing is kept of it.
    finished: bool,
    /// The finals this leader proposed whose answer is not known, by the ids their entries were
    /// built with, and whether a job reported: counted when the run is later seen ended with one
    /// of those very entries.
    proposed: Vec<((u64, u64), bool)>,
    /// A final the state refused for good, with the cancel flag it was built under: not
    /// proposed again until the record changes.
    refused: Option<(u32, FinalOutcome, bool)>,
    /// The healthy time and the app time since which a granted run found no job node to take it.
    unplaced_since: Option<(Duration, u64)>,
}

impl Watch {
    fn new(term: u64, attempt: u32) -> Self {
        Self {
            term,
            attempt,
            seen: Duration::ZERO,
            seen_at: Instant::now(),
            healthy: Duration::ZERO,
            born: Instant::now(),
            ticked: None,
            polled: None,
            held: None,
            leader_wait: false,
            cancel_seen: None,
            sent: false,
            progress: None,
            staged: None,
            finished: false,
            proposed: Vec::new(),
            refused: None,
            unplaced_since: None,
        }
    }

    /// Whether this very final was already refused for good under this record.
    fn already_refused(&self, plan: &FinalPlan, cancel_requested: bool) -> bool {
        self.refused == Some((plan.attempt, plan.outcome, cancel_requested))
    }

    /// Counts the time since the last round as healthy when this round's system read and the
    /// last one's both succeeded.
    fn tick(&mut self, now: Instant, fresh: bool) {
        if let (Some((prev, true)), true) = (self.ticked, fresh) {
            self.healthy += now.saturating_duration_since(prev);
        }
        self.ticked = Some((now, fresh));
    }

    /// The time this watch has run without a healthy round, up to `cap`.
    fn outage(&self, now: Instant, cap: Duration) -> Duration {
        now.saturating_duration_since(self.born)
            .saturating_sub(self.healthy)
            .min(cap)
    }

    /// The job node answered now.
    fn saw(&mut self) {
        self.seen = self.healthy;
        self.seen_at = Instant::now();
    }

    /// Whether the job node has been silent for `limit` of healthy time, or for far longer in
    /// wall time.
    fn silent_for(&self, limit: Duration, now: Instant) -> bool {
        self.healthy.saturating_sub(self.seen) >= limit
            || now.saturating_duration_since(self.seen_at) >= limit * WALL_FALLBACK_FACTOR
    }

    /// Whether the cancel is `limit` old in healthy time, or far older in wall time.
    fn cancel_overdue(&mut self, limit: Duration, now: Instant) -> bool {
        let (since, at) = *self.cancel_seen.get_or_insert((self.healthy, now));
        self.healthy.saturating_sub(since) >= limit
            || now.saturating_duration_since(at) >= limit * WALL_FALLBACK_FACTOR
    }

    /// Whether the run has been without a job node for `limit` of healthy time, or for `ceiling`
    /// more than that of app time whatever the leader saw.
    fn unplaced_too_long(&mut self, limit: Duration, ceiling: Duration, now_ms: u64) -> bool {
        let (healthy, ms) = *self.unplaced_since.get_or_insert((self.healthy, now_ms));
        let app = Duration::from_millis(now_ms.saturating_sub(ms));
        self.healthy.saturating_sub(healthy) >= limit || app >= limit + ceiling
    }

    /// The report held for `attempt`, if it is that attempt's.
    fn held_for(&self, attempt: u32) -> Option<(Report, String)> {
        self.held
            .as_ref()
            .filter(|(a, ..)| *a == attempt)
            .map(|(_, r, s)| (r.clone(), s.clone()))
    }

    /// Whether the run ended, or only the leader's own writes keep a final from committing.
    fn rescued(&self) -> bool {
        self.finished || self.leader_wait
    }
}

/// The runs this node drives as a leader, and the groups it is driving now.
#[derive(Default)]
pub(in crate::node) struct ComputeState {
    /// The groups whose round of strays and new steps is under way.
    driving: Mutex<HashSet<GroupId>>,
    /// The runs, by app and run id, whose step is under way: one at a time each.
    stepping: Mutex<HashSet<(String, String)>>,
    watches: Mutex<HashMap<(String, String), Watch>>,
    /// One run at a time chooses a job node and commits its attempt, so two runs never take the
    /// same last place.
    assigning: tokio::sync::Mutex<()>,
    /// Runs being delivered to each job node and not yet counted in its load.
    reserved: Mutex<HashMap<NodeId, usize>>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ComputeState {
    fn take(&self, app: &str, run: &str, term: u64, attempt: u32) -> Watch {
        locked(&self.watches)
            .get(&(app.to_owned(), run.to_owned()))
            .filter(|w| w.term == term && w.attempt == attempt)
            .cloned()
            .unwrap_or_else(|| Watch::new(term, attempt))
    }

    fn put(&self, app: &str, run: &str, watch: Watch) {
        let mut watches = locked(&self.watches);
        let key = (app.to_owned(), run.to_owned());
        if watch.finished {
            let _gone = watches.remove(&key);
        } else {
            let _prev = watches.insert(key, watch);
        }
    }

    /// Whether a step of the run is under way: its watch is in use, not for sweeping.
    fn is_stepping(&self, app: &str, run: &str) -> bool {
        locked(&self.stepping).contains(&(app.to_owned(), run.to_owned()))
    }

    /// The runs that hold staged outputs or an unconfirmed final, for [`Node::sweep_watches`].
    fn candidates(&self) -> Vec<((String, String), Watch)> {
        locked(&self.watches)
            .iter()
            .map(|(k, w)| (k.clone(), w.clone()))
            .collect()
    }

    fn forget(&self, key: &(String, String)) {
        let _gone = locked(&self.watches).remove(key);
    }

    fn reserve(&self, node: NodeId) {
        *locked(&self.reserved).entry(node).or_default() += 1;
    }

    fn unreserve(&self, node: NodeId) {
        let mut reserved = locked(&self.reserved);
        if let Some(n) = reserved.get_mut(&node) {
            *n = n.saturating_sub(1);
        }
    }

    fn reserved(&self) -> HashMap<NodeId, usize> {
        locked(&self.reserved).clone()
    }

    fn progress(&self, app: &str, run: &str, term: u64) -> Option<(u8, String)> {
        locked(&self.watches)
            .get(&(app.to_owned(), run.to_owned()))
            .filter(|w| w.term == term)
            .and_then(|w| w.progress.clone())
    }
}

/// Marks a run's step as under way until dropped, even if the step panics.
struct Stepping<'a> {
    node: &'a Node,
    key: (String, String),
}

impl Drop for Stepping<'_> {
    fn drop(&mut self) {
        let _was = locked(&self.node.compute.stepping).remove(&self.key);
    }
}

/// Where a queued run stands with the system group's queue.
enum Acquired {
    Granted,
    Waiting,
    Refused,
}

/// What `AcquireRun` answers.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum AcquireState {
    Granted,
    Queued,
    Refused,
}

#[derive(Deserialize)]
struct AcquireAnswer {
    state: AcquireState,
    #[serde(default)]
    retryable: bool,
}

/// How an end of a run that was proposed turned out.
enum Landed {
    Committed,
    /// Nothing was decided: try again next round.
    Retry,
    /// The entry was built on state that moved: nothing applied, a rebuilt one may land.
    Conflict,
    /// The run moved on without this entry (another final, another attempt).
    Superseded,
    /// The entry was refused for good.
    Refused,
    /// The final does not fit the log even without its result method.
    TooLarge,
}

/// What ends a run: how, and what the job node reported.
struct FinalPlan {
    attempt: u32,
    outcome: FinalOutcome,
    report: Option<Report>,
    signature: Option<String>,
    outputs: Vec<FileMark>,
    usage: ReportUsage,
}

impl FinalPlan {
    /// The leader's own outcome: no report, no outputs, no usage.
    fn leader(rec: &RunRecord, outcome: FinalOutcome) -> Self {
        Self {
            attempt: u32::try_from(rec.attempts.len()).unwrap_or(u32::MAX),
            outcome,
            report: None,
            signature: None,
            outputs: Vec::new(),
            usage: ReportUsage::default(),
        }
    }

    fn failed(rec: &RunRecord, reason: FailedReason) -> Self {
        Self::leader(rec, FinalOutcome::Failed { reason })
    }

    /// A job's outcome, as its signed report says.
    fn reported(report: Report, signature: String, outputs: Vec<FileMark>) -> Self {
        let outcome = match report.reason {
            None => FinalOutcome::Done,
            Some(reason) => FinalOutcome::Failed {
                reason: reason.into(),
            },
        };
        Self {
            attempt: report.attempt,
            outcome,
            usage: report.usage,
            report: Some(report),
            signature: Some(signature),
            outputs,
        }
    }
}

/// The path of an attempt on a job node: `{app}/{run}/{attempt}`, each id percent-encoded as one
/// path segment.
fn run_path(app: &str, run: &str, attempt: u32) -> String {
    let mut url = url::Url::parse("http://job.invalid/").expect("a constant URL");
    if let Ok(mut segments) = url.path_segments_mut() {
        let _ = segments.extend([app, run, &attempt.to_string()]);
    }
    url.path().trim_start_matches('/').to_owned()
}

/// The record of a run as the API shows it: no arguments, no keys, and the plaintext sizes and
/// hashes of its files only when `files`.
fn run_json(rec: &RunRecord, progress: Option<(u8, String)>, files: bool) -> Value {
    let marks = |outputs: &[FileMark]| -> Vec<Value> {
        outputs
            .iter()
            .map(|o| {
                if files {
                    json!({ "id": o.id, "size": o.size, "sha256": hex::encode(&o.sha256.0) })
                } else {
                    json!({ "id": o.id })
                }
            })
            .collect()
    };
    let inputs: Vec<Value> = rec
        .inputs
        .iter()
        .map(|i| {
            if files {
                json!(i)
            } else {
                json!({ "id": i.id })
            }
        })
        .collect();
    json!({
        "run": rec.run,
        "method": rec.method,
        "state": rec.state,
        "cancelRequested": rec.cancel_requested,
        "startTsMs": rec.start_ts_ms,
        "caps": rec.caps,
        "inputs": inputs,
        "attempts": rec.attempts.iter().map(|a| json!({
            "attempt": a.attempt,
            "node": a.node,
            "tsMs": a.ts_ms,
        })).collect::<Vec<_>>(),
        "final": rec.final_.as_ref().map(|f| json!({
            "attempt": f.attempt,
            "outcome": f.outcome,
            "outputs": marks(&f.outputs),
            "usage": f.usage,
            "tsMs": f.ts_ms,
        })),
        "progress": progress.map(|(percent, step)| json!({ "percent": percent, "step": step })),
    })
}

impl Node {
    /// One round of run driving, for each component app group this node leads, unless the
    /// previous round for that app is still running. Each run is stepped in a task of its own.
    pub fn drive_runs(self: &Arc<Self>) {
        self.sweep_watches();
        for (group, g) in self.led_component_groups() {
            if !locked(&self.compute.driving).insert(group) {
                continue;
            }
            let node = Arc::clone(self);
            drop(tokio::spawn(async move {
                node.drive_app_runs(&g).await;
                let _was = locked(&node.compute.driving).remove(&group);
            }));
        }
    }

    /// Undoes the staged outputs `marks` unless the app's state marks one: then a final that did
    /// commit owns them.
    fn discard_unowned(&self, g: &Group<AppMachine>, app_id: &str, marks: &[FileMark]) {
        let owned = g.read_local(|m, _| {
            marks
                .iter()
                .any(|f| m.state.component.files.contains_key(&f.id))
        });
        if !owned {
            self.discard(app_id, stored_of(marks));
        }
    }

    /// Deals with what this node kept of runs that have ended: outputs it staged for a final that
    /// did not commit are undone unless the app's state marks them (a final that did commit, by
    /// this node or another, owns them), and a final it proposed that did commit is counted.
    /// A run still active is left alone, whoever leads it.
    fn sweep_watches(&self) {
        for (key, watch) in self.compute.candidates() {
            let (app_id, run) = &key;
            let Some(g) = self.hosted(app_id) else {
                self.compute.forget(&key);
                continue;
            };
            if self.compute.is_stepping(app_id, run) {
                continue;
            }
            let record = g.read_local(|m, _| m.run_record(run));
            if record.as_ref().is_some_and(|r| r.state.is_active()) {
                continue;
            }
            self.compute.forget(&key);
            if let Some(marks) = &watch.staged {
                self.discard_unowned(&g, app_id, marks);
            }
            if let Some((usage, reported)) = ended_with_a_proposed(record.as_ref(), &watch.proposed)
            {
                let bundle = record
                    .as_ref()
                    .map(|r| r.bundle.clone())
                    .unwrap_or_default();
                let meter = self.meter(app_id, &bundle);
                self.usage.computed(&meter, &usage, reported);
            }
        }
    }

    async fn drive_app_runs(self: &Arc<Self>, g: &Arc<Group<AppMachine>>) {
        let app_id = g.read_local(|m, _| m.app_id.clone());
        let Some(app) = self.sys(|s| s.apps.get(&app_id).cloned()) else {
            return;
        };
        let status = g.status();
        if !status.is_leader {
            return;
        }
        self.release_strays(g, &app_id).await;
        for run in g.read_local(|m, _| m.active_runs()) {
            let key = (app_id.clone(), run.clone());
            if !locked(&self.compute.stepping).insert(key.clone()) {
                continue;
            }
            let (node, g, app) = (Arc::clone(self), Arc::clone(g), app.clone());
            let term = status.term;
            drop(tokio::spawn(async move {
                let _stepping = Stepping { node: &node, key };
                node.step_run(&g, &app, &run, term).await;
            }));
        }
    }

    /// Frees the system slots of this app's runs that are no longer active: a leader that
    /// crashed between a final and its release, or a deposed one that never got to it.
    async fn release_strays(&self, g: &Group<AppMachine>, app_id: &str) {
        let slots: Vec<String> = self.sys(|s| {
            s.compute
                .get(app_id)
                .map(|runs| runs.keys().cloned().collect())
                .unwrap_or_default()
        });
        for run in slots {
            let status = g.status();
            let stray = g.read_local(|m, _| {
                let known = m.run_record(&run).is_some();
                let caught_up = status.last_log.is_none_or(|last| status.applied >= last);
                !m.active_runs().contains(&run) && (known || caught_up)
            });
            if stray {
                self.release_slot(app_id, &run).await;
            }
        }
    }

    async fn release_slot(&self, app_id: &str, run: &str) {
        let cmd = SystemCmd::ReleaseRun {
            app: app_id.to_owned(),
            run: run.to_owned(),
        };
        if let Err(e) = self.sys_write(&cmd).await {
            debug!(app = %app_id, run, error = %e.message, "run slot not released");
        }
    }

    async fn step_run(&self, g: &Group<AppMachine>, app: &AppRecord, run: &str, term: u64) {
        let status = g.status();
        if !status.is_leader || status.term != term {
            return;
        }
        let Some(rec) = g
            .read_local(|m, _| m.run_record(run))
            .filter(|r| r.state.is_active())
        else {
            return;
        };
        let attempts = u32::try_from(rec.attempts.len()).unwrap_or(u32::MAX);
        let mut watch = self.compute.take(&app.app_id, run, term, attempts);
        watch.tick(Instant::now(), self.sys_fresh().await);
        let drive = Drive {
            node: self,
            g,
            app,
            rec: &rec,
        };
        if rec.cancel_requested {
            drive.cancel(&mut watch).await;
        } else if rec.state == RunState::Queued {
            drive.place(&mut watch).await;
        } else {
            drive.watch(&mut watch).await;
        }
        self.compute.put(&app.app_id, run, watch);
    }

    /// The reserved view of a run's record, with its progress when this node leads the app.
    pub(super) fn run_view(
        &self,
        g: &Group<AppMachine>,
        req: &ViewRequest,
    ) -> Result<ViewAnswer, ApiError> {
        let files = req.args["files"].as_bool().unwrap_or(false);
        if req.args.get("run").is_none() {
            return self.runs_view(g, req, files);
        }
        let run = req.args["run"]
            .as_str()
            .ok_or_else(|| ApiError::bad_request("BAD_RUN", "name the run"))?;
        let (record, applied) = g.read_local(|m, applied| (m.run_record(run), applied));
        let status = g.status();
        let progress = record
            .as_ref()
            .filter(|r| status.is_leader && r.state.is_active())
            .and_then(|_| self.compute.progress(&req.app_id, run, status.term));
        Ok(ViewAnswer {
            index: applied,
            status: Status::Ok,
            result: record.map_or(Value::Null, |r| run_json(&r, progress, files)),
            error: Value::Null,
            timeout: false,
        })
    }

    /// A page of the run records, each as the single-run view shows it.
    fn runs_view(
        &self,
        g: &Group<AppMachine>,
        req: &ViewRequest,
        files: bool,
    ) -> Result<ViewAnswer, ApiError> {
        let limit = match req.args.get("limit").filter(|l| !l.is_null()) {
            None => DEFAULT_RUNS_PAGE,
            Some(l) => l
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| (1..=MAX_RUNS_PAGE).contains(n))
                .ok_or_else(|| {
                    ApiError::bad_request("BAD_LIMIT", &format!("limit is 1 to {MAX_RUNS_PAGE}"))
                })?,
        };
        let before = match req.args["before"].as_str() {
            None => None,
            Some(c) => Some(
                c.split_once(':')
                    .and_then(|(ts, run)| Some((ts.parse().ok()?, run.to_owned())))
                    .ok_or_else(|| {
                        ApiError::bad_request("BAD_CURSOR", "not a cursor of this list")
                    })?,
            ),
        };
        let (page, next, applied) = g.read_local(|m, applied| {
            let (page, next) = m.runs_page(limit, before.as_ref());
            (page, next, applied)
        });
        let status = g.status();
        let runs: Vec<Value> = page
            .iter()
            .map(|r| {
                let progress = (status.is_leader && r.state.is_active())
                    .then(|| self.compute.progress(&req.app_id, &r.run, status.term))
                    .flatten();
                run_json(r, progress, files)
            })
            .collect();
        Ok(ViewAnswer {
            index: applied,
            status: Status::Ok,
            result: json!({
                "runs": runs,
                "next": next.map(|(ts, run)| format!("{ts}:{run}")),
            }),
            error: Value::Null,
            timeout: false,
        })
    }
}

/// One run being driven: the node, the app's group and the run's record as the round read it.
struct Drive<'a> {
    node: &'a Node,
    g: &'a Group<AppMachine>,
    app: &'a AppRecord,
    rec: &'a RunRecord,
}

impl Drive<'_> {
    /// A queued run: get a place from the system queue, then a job node.
    async fn place(&self, watch: &mut Watch) {
        let refused = FinalPlan::failed(self.rec, FailedReason::Refused);
        match self.acquire().await {
            Acquired::Waiting => {}
            Acquired::Refused => {
                self.end(&refused, watch).await;
            }
            Acquired::Granted => {
                if self.assign(&[], watch).await {
                    watch.unplaced_since = None;
                } else {
                    let limits = self.node.limits();
                    let now_ms = self.node.clock.now_ms();
                    if watch.unplaced_too_long(limits.queued_timeout, limits.outage_credit, now_ms)
                    {
                        self.end(&refused, watch).await;
                    }
                }
            }
        }
    }

    async fn acquire(&self) -> Acquired {
        let (app_id, run) = (&self.app.app_id, &self.rec.run);
        let slot = self.node.sys(|s| s.run_slot(app_id, run).cloned());
        match slot {
            Some(RunSlot { refused: true, .. }) => return Acquired::Refused,
            Some(slot) if slot.granted => return Acquired::Granted,
            Some(_) => return Acquired::Waiting,
            None => {}
        }
        // Older nodes never see the queue's commands: only a registered job node lets them in.
        if self.node.sys(|s| s.job_nodes().next().is_none()) {
            return Acquired::Waiting;
        }
        let _fresh = self.node.sys_fresh().await;
        let Some(payer) = self
            .node
            .sys(|s| s.apps.get(app_id).map(crate::usage::payer))
        else {
            return Acquired::Waiting;
        };
        let cmd = SystemCmd::AcquireRun {
            app: app_id.clone(),
            run: run.clone(),
            payer,
            fuel: self.rec.caps.fuel,
        };
        let answer = match self.node.sys_write(&cmd).await {
            Ok(answer) => serde_json::from_value::<AcquireAnswer>(answer),
            Err(e) => {
                debug!(app = %app_id, run, error = %e.message, "run not acquired");
                return Acquired::Waiting;
            }
        };
        match answer {
            Ok(AcquireAnswer {
                state: AcquireState::Granted,
                ..
            }) => Acquired::Granted,
            Ok(AcquireAnswer {
                state: AcquireState::Refused,
                retryable: false,
            }) => Acquired::Refused,
            Ok(_) => Acquired::Waiting,
            Err(e) => {
                warn!(app = %app_id, run, error = %e, "unreadable answer to AcquireRun");
                Acquired::Waiting
            }
        }
    }

    /// Chooses a job node (not one of `avoid` while another is free), hands it the attempt and
    /// only then commits it, so a job node that is busy or unreachable spends no attempt; the
    /// next candidate is tried instead. The lock is held only to choose and reserve a place.
    /// Whether a job node was found.
    async fn assign(&self, avoid: &[NodeId], watch: &mut Watch) -> bool {
        let attempt = u32::try_from(self.rec.attempts.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        let marks = self.g.read_local(|m, _| m.file_marks());
        let mut failed: Vec<NodeId> = Vec::new();
        loop {
            let peer = {
                let _one = self.node.compute.assigning.lock().await;
                let reserved = self.node.compute.reserved();
                let Some(peer) = self
                    .node
                    .choose_job(self.app, avoid, &failed, &reserved)
                    .await
                else {
                    return false;
                };
                self.node.compute.reserve(peer.id);
                peer
            };
            let placed = self.place_on(&peer, attempt, &marks).await;
            self.node.compute.unreserve(peer.id);
            match placed {
                None => failed.push(peer.id),
                Some(committed) => {
                    if committed {
                        self.discard_staged(watch);
                        *watch = Watch::new(watch.term, attempt);
                        watch.sent = true;
                    }
                    return true;
                }
            }
        }
    }

    /// Builds and delivers the attempt to `peer`, then commits it. `None`: the job node could not
    /// take it. `Some(false)`: it took it but the attempt was not committed as ours, and its run
    /// is dropped again.
    async fn place_on(
        &self,
        peer: &crate::node::JobPeer,
        attempt: u32,
        marks: &crate::component::Marks,
    ) -> Option<bool> {
        let assignment = match self
            .node
            .build_assignment(self.app, self.rec, attempt, peer, marks)
        {
            Ok(a) => a,
            Err(e) => {
                warn!(app = %self.app.app_id, run = %self.rec.run, error = %e, "assignment not built");
                return None;
            }
        };
        let digest = hex::encode(assignment_digest(&assignment).ok()?);
        if let Err(e) = self.deliver(peer, &assignment).await {
            debug!(app = %self.app.app_id, run = %self.rec.run, node = peer.id, error = %e, "attempt not taken");
            return None;
        }
        if self.commit_attempt(attempt, peer, &digest).await {
            return Some(true);
        }
        self.delete_on(peer, attempt).await;
        Some(false)
    }

    /// Commits the attempt; whether it is the one recorded for `attempt` now, with this node and
    /// assignment. An attempt another leader committed first is not ours to send.
    async fn commit_attempt(
        &self,
        attempt: u32,
        peer: &crate::node::JobPeer,
        digest: &str,
    ) -> bool {
        let node = self.node;
        let run = &self.rec.run;
        let proposed = self
            .g
            .propose_led(|m, _, _| {
                let next = m.run_record(run).is_some_and(|r| {
                    r.state.is_active()
                        && !r.cancel_requested
                        && r.attempts.len() + 1 == attempt as usize
                });
                if !next {
                    return Proposal::Reply(Vec::new());
                }
                let mut cmd = ComponentCmd::ComputeAttempt(Box::new(AttemptEntry {
                    run: run.clone(),
                    attempt,
                    node: peer.id,
                    job_key: hex::encode(peer.job_key),
                    assignment_digest: digest.to_owned(),
                    ts_ms: node.stamp(m),
                }));
                Proposal::Append(entry_bytes(&mut cmd).0)
            })
            .await;
        if let Err(e) = proposed {
            debug!(app = %self.app.app_id, run, error = %e, "attempt not committed");
            return false;
        }
        self.g
            .read_local(|m, _| m.run_record(run))
            .and_then(|r| r.attempts.get(attempt as usize - 1).cloned())
            .is_some_and(|a| a.node == peer.id && a.assignment_digest == digest)
    }

    /// A running run: poll its job node, and act on what it says or on its silence.
    async fn watch(&self, watch: &mut Watch) {
        let Some(attempt) = self.rec.attempts.last() else {
            return;
        };
        if self.past_ceiling(attempt) {
            self.end_timeout(watch).await;
            return;
        }
        if self.timed_out(attempt, watch) {
            if !self.rescue(attempt, watch).await {
                self.end_timeout(watch).await;
            }
            return;
        }
        if watch.polled.is_some_and(|at| at.elapsed() < JOB_POLL) {
            return;
        }
        watch.polled = Some(Instant::now());
        match self.poll(attempt).await {
            job_io::Polled::Silent => self.silent(attempt, watch).await,
            job_io::Polled::Deferred => {}
            job_io::Polled::Lost => self.lost_unless_held(attempt, watch).await,
            job_io::Polled::Running(progress) => {
                watch.saw();
                watch.sent = true;
                watch.progress = progress;
            }
            job_io::Polled::Unknown if watch.sent => self.lost_unless_held(attempt, watch).await,
            job_io::Polled::Unknown | job_io::Polled::Awaiting => match self.send(attempt).await {
                job_io::Sent::Delivered => {
                    watch.sent = true;
                    watch.saw();
                }
                job_io::Sent::Failed => self.silent(attempt, watch).await,
                job_io::Sent::Impossible => self.lost_unless_held(attempt, watch).await,
            },
            job_io::Polled::Done(report, signature) => {
                if self.finish(attempt, *report, signature, watch).await {
                    self.silent(attempt, watch).await;
                }
            }
        }
    }

    /// The job node did not answer: the attempt is lost once it has been silent long enough,
    /// unless a final or a report for it turns up first.
    async fn silent(&self, attempt: &Attempt, watch: &mut Watch) {
        let limit = self.node.limits().lost_after;
        if watch.silent_for(limit, Instant::now()) && !self.rescue(attempt, watch).await {
            self.lost(attempt, watch).await;
        }
    }

    /// Before an attempt is given up: whether the record holds a final, or a held report or a
    /// fresh poll still ends the run. True when it must not be given up now.
    async fn rescue(&self, attempt: &Attempt, watch: &mut Watch) -> bool {
        let active = self
            .g
            .read_local(|m, _| m.run_record(&self.rec.run))
            .is_some_and(|r| r.state.is_active());
        if !active {
            return true;
        }
        if let Some((report, signature)) = watch.held_for(attempt.attempt) {
            let _job_side = self.finish(attempt, report, signature, watch).await;
            if watch.attempt != attempt.attempt || watch.rescued() {
                return true;
            }
        }
        match self.poll(attempt).await {
            job_io::Polled::Done(report, signature) => {
                let _job_side = self.finish(attempt, *report, signature, watch).await;
                watch.attempt != attempt.attempt || watch.rescued()
            }
            job_io::Polled::Deferred => {
                let credit = self.node.limits().outage_credit;
                watch.outage(Instant::now(), credit) < credit
            }
            _ => false,
        }
    }

    /// The job node says the attempt is gone: a held report for it still ends the run, and only
    /// if that neither ends the run nor waits on the leader's own writes is the attempt lost.
    async fn lost_unless_held(&self, attempt: &Attempt, watch: &mut Watch) {
        if let Some((report, signature)) = watch.held_for(attempt.attempt) {
            let _job_side = self.finish(attempt, report, signature, watch).await;
            if watch.attempt != attempt.attempt || watch.rescued() {
                return;
            }
        }
        self.lost(attempt, watch).await;
    }

    /// The attempt is lost (not charged): another job node takes the next one, or the run ends.
    async fn lost(&self, attempt: &Attempt, watch: &mut Watch) {
        self.node.forget_job(attempt.node);
        if self.rec.cancel_requested {
            self.end_cancelled(watch).await;
        } else if self.rec.attempts.len() < usize::try_from(MAX_ATTEMPTS).unwrap_or(usize::MAX) {
            let _placed = self.assign(&[attempt.node], watch).await;
        } else {
            let plan = FinalPlan::failed(self.rec, FailedReason::Lost);
            self.end(&plan, watch).await;
        }
    }

    /// Whether the app clock, less this leader's outages, is past the attempt's wall cap and
    /// the grace for its report.
    fn timed_out(&self, attempt: &Attempt, watch: &Watch) -> bool {
        let passed = Duration::from_millis(self.node.clock.now_ms().saturating_sub(attempt.ts_ms));
        let now = Instant::now();
        let allowed = Duration::from_secs(u64::from(self.rec.caps.wall_secs))
            + self.node.limits().deadline_grace;
        passed.saturating_sub(watch.outage(now, self.node.limits().outage_credit)) > allowed
    }

    /// The app time after which the run ends by timeout whatever the leader saw or holds: the
    /// wall cap, the grace and the most outage a deadline forgives.
    fn past_ceiling(&self, attempt: &Attempt) -> bool {
        let limits = self.node.limits();
        let passed = Duration::from_millis(self.node.clock.now_ms().saturating_sub(attempt.ts_ms));
        let wall = Duration::from_secs(u64::from(self.rec.caps.wall_secs));
        passed > wall + limits.deadline_grace + limits.outage_credit
    }

    async fn end_timeout(&self, watch: &mut Watch) {
        let plan = FinalPlan::failed(self.rec, FailedReason::Timeout);
        self.end(&plan, watch).await;
    }

    /// A cancel was requested. A job that finished first is taken as it reports; otherwise its
    /// run is stopped and the run ends cancelled with the usage the job node signed, or with none.
    /// A run that does not stop is asked again each round, until [`CANCEL_DEADLINE`].
    async fn cancel(&self, watch: &mut Watch) {
        let Some(attempt) = self.rec.attempts.last() else {
            self.end_cancelled(watch).await;
            return;
        };
        let limit = self.node.limits().cancel_deadline;
        if watch.cancel_overdue(limit, Instant::now()) {
            self.force_cancel(attempt, watch).await;
            return;
        }
        match self.poll(attempt).await {
            job_io::Polled::Done(report, signature) => {
                let _job_side = self.finish(attempt, *report, signature, watch).await;
                return;
            }
            job_io::Polled::Deferred => return,
            // A job node that does not answer a poll would not answer a delete either.
            job_io::Polled::Silent => {
                self.force_cancel(attempt, watch).await;
                return;
            }
            _ => {}
        }
        match self.delete_job_run(attempt).await {
            job_io::Deleted::Running => {}
            job_io::Deleted::Ended(report, signature)
                if report.status == ReportStatus::Failed
                    && report.reason == Some(FailReason::Cancelled) =>
            {
                let _job_side = self.finish(attempt, *report, signature, watch).await;
            }
            job_io::Deleted::Ended(..) | job_io::Deleted::Gone => self.end_cancelled(watch).await,
        }
    }

    /// The job node does not say it stopped: the run ends as the report held for the attempt
    /// says, else cancelled with no usage. The job run is deleted once the final stands.
    async fn force_cancel(&self, attempt: &Attempt, watch: &mut Watch) {
        if self.past_ceiling(attempt) {
            self.end_cancelled(watch).await;
            return;
        }
        if let Some((report, signature)) = watch.held_for(attempt.attempt) {
            let _job_side = self.finish(attempt, report, signature, watch).await;
            if watch.finished || watch.attempt != attempt.attempt {
                return;
            }
            // Only the leader's own writes keep the held report from ending the run.
            if watch.leader_wait {
                return;
            }
        }
        self.end_cancelled(watch).await;
    }

    /// Ends the run as cancelled by the leader, with no usage.
    async fn end_cancelled(&self, watch: &mut Watch) {
        let plan = FinalPlan::leader(self.rec, FinalOutcome::Cancelled);
        self.end(&plan, watch).await;
    }

    /// Commits `plan` and, once it stands, frees the run's slot and counts its usage. Whether
    /// the attempt of a reported final the state refused for good must be given up as lost.
    async fn end(&self, plan: &FinalPlan, watch: &mut Watch) -> bool {
        if watch.already_refused(plan, self.rec.cancel_requested) {
            return false;
        }
        let mut built = None;
        let landed = self.commit_final(plan, &mut built).await;
        if let Some(id) = built {
            watch.proposed.push((id, plan.report.is_some()));
            if watch.proposed.len() > MAX_PROPOSED_KEPT {
                let _oldest = watch.proposed.remove(0);
            }
        }
        match landed {
            Landed::Committed => self.after_final(plan, watch).await,
            Landed::Retry | Landed::Conflict if plan.report.is_some() => {
                watch.leader_wait = true;
                watch.saw();
            }
            Landed::Retry | Landed::Conflict => {}
            Landed::Superseded => {
                self.discard_staged(watch);
            }
            Landed::Refused => {
                warn!(app = %self.app.app_id, run = %self.rec.run, "a final was refused");
                watch.proposed.clear();
                watch.refused = Some((plan.attempt, plan.outcome, self.rec.cancel_requested));
                self.discard_staged(watch);
                return plan.report.is_some();
            }
            Landed::TooLarge => {
                watch.proposed.clear();
                self.discard_staged(watch);
                let timeout = FinalPlan::failed(self.rec, FailedReason::Timeout);
                if matches!(
                    self.commit_final(&timeout, &mut None).await,
                    Landed::Committed
                ) {
                    self.after_final(&timeout, watch).await;
                }
            }
        }
        false
    }

    /// Undoes what a final staged (the outputs' bytes and their charge) that did not commit,
    /// unless the app's state marks them: then a final that did commit owns them.
    fn discard_staged(&self, watch: &mut Watch) {
        let Some(marks) = watch.staged.take() else {
            return;
        };
        self.node.discard_unowned(self.g, &self.app.app_id, &marks);
    }

    async fn after_final(&self, plan: &FinalPlan, watch: &mut Watch) {
        watch.finished = true;
        watch.proposed.clear();
        if watch.staged.as_ref().is_some_and(|s| *s != plan.outputs) {
            self.discard_staged(watch);
        }
        let (app_id, run) = (&self.app.app_id, &self.rec.run);
        if self.node.sys(|s| s.run_slot(app_id, run).is_some()) {
            self.node.release_slot(app_id, run).await;
        }
        if let Some(attempt) = self.rec.attempts.last() {
            let _answer = self.delete_job_run(attempt).await;
        }
        let meter = self.node.meter(app_id, &self.rec.bundle);
        self.node
            .usage
            .computed(&meter, &plan.usage, plan.report.is_some());
    }

    /// Proposes the end of the run: the result method runs inside the entry, on the state it is
    /// built on, and its effects apply with the final or not at all.
    async fn commit_final(&self, plan: &FinalPlan, built: &mut Option<(u64, u64)>) -> Landed {
        for _ in 0..FINAL_CONFLICT_TRIES {
            match self.propose_final(plan, built).await {
                Landed::Conflict => {}
                landed => return landed,
            }
        }
        Landed::Retry
    }

    async fn propose_final(&self, plan: &FinalPlan, built: &mut Option<(u64, u64)>) -> Landed {
        let (node, rec) = (self.node, self.rec);
        let app_id = &self.app.app_id;
        let Some(bundle) = self.g.read_local(|m, _| m.component_bundle()) else {
            return Landed::Retry;
        };
        let Ok(prepared) = node.prepared_for(&bundle).await else {
            return Landed::Retry;
        };
        let meter = node.meter(app_id, &bundle);
        let args = finish::result_args(rec, plan);
        let (mut superseded, mut unfit) = (false, false);
        let mut persisted = Stored::default();
        let proposed = self
            .g
            .propose_led(|m, applied, leadership| {
                (superseded, unfit) = (false, false);
                persisted = Stored::default();
                *built = None;
                if m.state.bundle.as_deref() != Some(bundle.as_str()) {
                    return Proposal::Reply(Vec::new());
                }
                let current = m
                    .run_record(&rec.run)
                    .filter(|r| r.state.is_active() && r.attempts.len() == plan.attempt as usize);
                let Some(current) = current else {
                    superseded = true;
                    return Proposal::Reply(Vec::new());
                };
                let ts_ms = node.stamp(m);
                let (id_term, id_seq) =
                    node.next_id(self.g.id(), leadership.term, m.state.component.head.last_id);
                *built = Some((id_term, id_seq));
                let has_result = prepared
                    .abi
                    .methods
                    .get(&current.result_method)
                    .is_some_and(|x| x.compute_result);
                let ran = if has_result {
                    node.run_logged(
                        &meter,
                        &prepared,
                        app_id,
                        Reads {
                            keys: m.view(),
                            files: m.file_marks(),
                            runs: m.runs_provider(Some(&rec.run)),
                        },
                        applied,
                        Invocation {
                            kind: InvocationKind::Call,
                            method: current.result_method.clone(),
                            args: args.clone(),
                            caller: calimero_app_host::Caller::System,
                            entry_ts_ms: ts_ms,
                            id_term,
                            id_seq,
                            seed: calimero_identity::random::<32>(),
                            timer: None,
                            compute: None,
                        },
                    )
                } else {
                    Ran {
                        outcome: Outcome::trap(
                            TrapKind::Fault,
                            "the app has no such result method",
                        ),
                        blobs: Vec::new(),
                    }
                };
                let entry = FinalEntry {
                    run: rec.run.clone(),
                    attempt: plan.attempt,
                    outcome: plan.outcome,
                    report: plan.report.clone(),
                    signature: plan.signature.clone(),
                    outputs: plan.outputs.clone(),
                    usage: plan.usage,
                    result: ran.outcome,
                    base_index: m.state.last_index,
                    ts_ms,
                    id_term,
                    id_seq,
                };
                let built = node.logged_entry(
                    app_id,
                    ComponentCmd::ComputeFinal(Box::new(entry.clone())),
                    &ran.blobs,
                );
                // The result method's files could not be stored: the run still ends, without
                // the effects of its result method.
                let built = match built {
                    Err(e) if e.code != super::FINAL_TOO_LARGE => {
                        let denied = Outcome::trap(TrapKind::HostDenied, &e.message);
                        let bare = FinalEntry {
                            result: denied,
                            ..entry
                        };
                        node.logged_entry(app_id, ComponentCmd::ComputeFinal(Box::new(bare)), &[])
                    }
                    other => other,
                };
                match built {
                    Ok((bytes, stored)) => {
                        persisted = stored;
                        Proposal::Append(bytes)
                    }
                    Err(_) => {
                        unfit = true;
                        Proposal::Reply(Vec::new())
                    }
                }
            })
            .await;
        let p = match proposed {
            Ok(p) => p,
            Err(e) => {
                node.discard_failed(app_id, std::mem::take(&mut persisted), &e);
                return Landed::Retry;
            }
        };
        if superseded {
            return Landed::Superseded;
        }
        if unfit {
            return Landed::TooLarge;
        }
        let Ok(answer) = serde_json::from_slice::<Applied>(&p.output) else {
            return Landed::Retry;
        };
        if !p.appended {
            return Landed::Retry;
        }
        if answer.conflict {
            let _prev = node.conflicts.fetch_add(1, Ordering::Relaxed);
            node.discard(app_id, persisted);
            return Landed::Conflict;
        }
        let ended = self
            .g
            .read_local(|m, _| m.run_record(&rec.run))
            .is_some_and(|r| !r.state.is_active());
        if !ended {
            node.discard(app_id, persisted);
            return Landed::Refused;
        }
        node.refund_files(app_id, answer.freed).await;
        Landed::Committed
    }
}

/// The usage of the run's final, and whether a job reported it, if that final is one of the
/// entries this node proposed: matched by the ids the entry was built with, never by what it
/// says, which another leader's final may say as well.
fn ended_with_a_proposed(
    record: Option<&RunRecord>,
    proposed: &[((u64, u64), bool)],
) -> Option<(ReportUsage, bool)> {
    let end = record?.final_.as_ref()?;
    proposed
        .iter()
        .find(|(id, _)| *id == (end.id_term, end.id_seq))
        .map(|(_, reported)| (end.usage, *reported))
}

/// What a final's staged outputs hold on the replicas and in the quota.
fn stored_of(marks: &[FileMark]) -> Stored {
    Stored {
        ids: marks.iter().map(|m| m.id.clone()).collect(),
        charged: marks.iter().map(|m| m.size).sum(),
        outputs: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(outcome: FinalOutcome) -> FinalPlan {
        FinalPlan {
            attempt: 1,
            outcome,
            report: None,
            signature: None,
            outputs: Vec::new(),
            usage: ReportUsage::default(),
        }
    }

    #[test]
    fn a_refused_final_is_not_proposed_again_until_the_record_changes() {
        let mut watch = Watch::new(1, 1);
        let timeout = plan(FinalOutcome::Failed {
            reason: FailedReason::Timeout,
        });
        assert!(!watch.already_refused(&timeout, false));
        watch.refused = Some((1, timeout.outcome, false));
        assert!(watch.already_refused(&timeout, false));
        assert!(
            !watch.already_refused(&timeout, true),
            "a cancel changed the record"
        );
        assert!(!watch.already_refused(&plan(FinalOutcome::Cancelled), false));
        assert!(!watch.already_refused(
            &FinalPlan {
                attempt: 2,
                ..timeout
            },
            false
        ));
    }

    #[test]
    fn a_run_without_a_job_node_is_refused_only_after_its_limit() {
        let mut watch = Watch::new(1, 0);
        let start = Instant::now();
        let limit = Duration::from_secs(600);
        let ceiling = Duration::from_secs(60);
        watch.tick(start, true);
        assert!(!watch.unplaced_too_long(limit, ceiling, 0));
        watch.tick(start + limit - Duration::from_secs(1), true);
        assert!(!watch.unplaced_too_long(limit, ceiling, 0));
        watch.tick(start + limit, true);
        assert!(watch.unplaced_too_long(limit, ceiling, 0));
    }

    #[test]
    fn a_run_without_a_job_node_is_refused_at_the_app_time_ceiling_without_healthy_rounds() {
        let mut watch = Watch::new(1, 0);
        let (limit, ceiling) = (Duration::from_secs(600), Duration::from_secs(60));
        let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap();
        assert!(!watch.unplaced_too_long(limit, ceiling, 5));
        assert!(!watch.unplaced_too_long(limit, ceiling, 5 + ms(limit + ceiling) - 1));
        assert!(watch.unplaced_too_long(limit, ceiling, 5 + ms(limit + ceiling)));
    }

    fn ended(id: (u64, u64)) -> RunRecord {
        let mut rec: RunRecord = serde_json::from_value(json!({
            "run": "r", "method": "m", "args": "", "argsHash": "", "inputs": [],
            "caps": { "fuel": 1, "memoryMib": 1, "wallSecs": 1 }, "resultMethod": "x",
            "bundle": "b", "caller": { "kind": "system" }, "owner": "o", "startTsMs": 0,
            "state": "done", "cancelRequested": false, "attempts": [], "final": null,
        }))
        .unwrap();
        rec.final_ = Some(crate::component::Final {
            attempt: 1,
            outcome: FinalOutcome::Done,
            outputs: Vec::new(),
            usage: ReportUsage {
                cpu_micros: 1,
                gib_micros: 1,
                fuel: 5,
            },
            ts_ms: 0,
            id_term: id.0,
            id_seq: id.1,
        });
        rec
    }

    #[test]
    fn only_a_final_this_node_proposed_is_counted() {
        let mine = [((3, 7), true)];
        let counted = ended_with_a_proposed(Some(&ended((3, 7))), &mine).unwrap();
        assert_eq!((counted.0.fuel, counted.1), (5, true));
        // The same attempt and usage, ended by another leader's entry.
        assert!(ended_with_a_proposed(Some(&ended((4, 1))), &mine).is_none());
        assert!(ended_with_a_proposed(None, &mine).is_none());
    }

    #[test]
    fn only_rounds_with_a_system_read_count_as_healthy_time() {
        let mut watch = Watch::new(1, 1);
        let start = Instant::now();
        let secs = Duration::from_secs;
        watch.tick(start, true);
        watch.tick(start + secs(2), true);
        assert_eq!(watch.healthy, secs(2));
        // An outage and the round that ends it add nothing.
        watch.tick(start + secs(4), false);
        watch.tick(start + secs(500), false);
        watch.tick(start + secs(502), true);
        assert_eq!(watch.healthy, secs(2));
        watch.tick(start + secs(504), true);
        assert_eq!(watch.healthy, secs(4));
        assert!(!watch.silent_for(secs(5), start));
        assert!(watch.silent_for(secs(4), start));
        watch.saw();
        assert!(!watch.silent_for(secs(1), start));
    }

    #[test]
    fn an_outage_is_forgiven_only_up_to_the_cap() {
        let watch = Watch::new(1, 1);
        let at = watch.born;
        let cap = MAX_OUTAGE_CREDIT;
        assert_eq!(
            watch.outage(at + Duration::from_secs(30), cap),
            Duration::from_secs(30)
        );
        assert_eq!(watch.outage(at + cap * 3, cap), cap);
    }

    #[test]
    fn the_lost_and_cancel_clocks_end_in_wall_time_without_healthy_rounds() {
        let mut watch = Watch::new(1, 1);
        let at = watch.born;
        let limit = Duration::from_secs(20);
        let late = at + limit * WALL_FALLBACK_FACTOR;
        assert!(!watch.silent_for(limit, late - Duration::from_secs(1)));
        assert!(watch.silent_for(limit, late));
        assert!(!watch.cancel_overdue(limit, at));
        assert!(!watch.cancel_overdue(limit, late - Duration::from_secs(1)));
        assert!(watch.cancel_overdue(limit, late));
    }

    #[test]
    fn a_held_report_serves_only_its_own_attempt() {
        let mut watch = Watch::new(1, 2);
        let report = Report {
            app: "a".into(),
            run: "r".into(),
            attempt: 1,
            status: ReportStatus::Done,
            reason: None,
            result: Vec::new(),
            outputs: Vec::new(),
            usage: ReportUsage::default(),
        };
        watch.held = Some((1, report, "sig".into()));
        assert!(watch.held_for(1).is_some());
        assert!(watch.held_for(2).is_none());
    }

    #[test]
    fn only_a_finished_run_or_a_pending_commit_counts_as_rescued() {
        let mut watch = Watch::new(1, 1);
        assert!(!watch.rescued(), "a final refused for good rescues nothing");
        watch.leader_wait = true;
        assert!(watch.rescued());
        watch.leader_wait = false;
        watch.finished = true;
        assert!(watch.rescued());
    }

    #[test]
    fn a_group_being_driven_is_not_swept() {
        let state = ComputeState::default();
        assert!(!state.is_stepping("a", "1"));
        let _new = locked(&state.stepping).insert(("a".into(), "1".into()));
        assert!(state.is_stepping("a", "1"));
        assert!(!state.is_stepping("a", "2"));
        assert!(!state.is_stepping("b", "1"));
    }

    #[test]
    fn path_segments_are_percent_encoded() {
        assert_eq!(run_path("a b", "r/1", 2), "a%20b/r%2F1/2");
    }
}
