//! The runs a job node holds and the routes that assign, feed, poll, fetch and cancel them.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use calimero_app_host::{
    MethodKind, Prepared, MAX_COMPUTE_FILE_BYTES, MAX_COMPUTE_FUEL, MAX_COMPUTE_MEMORY_MIB,
    MAX_COMPUTE_WALL_SECS, MAX_FILE_BYTES,
};
use serde_json::{json, Value};
use tokio::sync::watch;
use zeroize::Zeroizing;

use super::admission::{admit_assignment, token_matches};
use super::{exec, locked, JobNode, RUNS_PER_JOB_NODE};
use crate::error::ApiError;
use crate::files::{sealed_len, unwrap_for_job, JobWrap, WrapKind};
use crate::job_wire::{assignment_message, is_hash, Assignment, Report};
use crate::node::MAX_ARGS_BYTES;

pub const JOB_RESULT_TTL: Duration = Duration::from_secs(60 * 60); // how long a finished or lost run is kept for its leader to collect
pub const AWAITING_INPUTS_TTL: Duration = Duration::from_secs(10 * 60); // how long a run may wait for its inputs before it is dropped
pub const CANCEL_WAIT: Duration = Duration::from_secs(5); // how long a cancel waits for the run to stop before answering that it still runs
pub use calimero_app_host::{MAX_RUN_INPUTS, MAX_RUN_INPUT_BYTES};
const MAX_ID_BYTES: usize = 256; // longest app, run, method or file id an assignment may carry
pub(super) const HASH_BYTES: usize = 32; // a run's token and every hash: 32 bytes
pub const MAX_FINISHED_BYTES: u64 = 8 * 1024 * 1024 * 1024; // sealed bytes held for finished runs plus those active and new runs may still hold, past which work is refused
pub const MAX_ASSIGNMENT_BYTES: usize = 1024 * 1024; // the most a request to assign a run may weigh
pub const TOKEN_HEADER: &str = "x-run-token"; // the header carrying a run's capability token

/// A run's secrets, unwrapped at admission and taken by the thread that runs it.
pub(super) struct Keys {
    pub seed: Zeroizing<[u8; 32]>,
    pub inputs: Vec<Zeroizing<[u8; 32]>>,
}

/// A sealed output left on disk, and the data key that opens it.
pub(super) struct OutputFile {
    pub path: PathBuf,
    pub key: Zeroizing<[u8; 32]>,
}

pub(super) struct Finished {
    pub report: Report,
    pub signature: String,
    pub outputs: Vec<OutputFile>,
    /// Bytes the outputs hold on disk.
    pub bytes: u64,
}

pub(super) enum Phase {
    Awaiting,
    Running,
    /// Ended with a signed report, a failure of the app's included.
    Done(Box<Finished>),
    /// The attempt ended for a reason of this node (instance pool, allocation, disk): it has
    /// no outcome, and its leader runs it elsewhere.
    Lost,
}

pub(super) struct RunState {
    pub phase: Phase,
    pub progress: Option<(u8, String)>,
    pub ended: Option<Instant>,
}

pub(super) struct Run {
    pub assignment: Assignment,
    /// What the leader signed; another assignment for the same attempt must match it.
    message: Vec<u8>,
    token: [u8; 32],
    pub prepared: Arc<Prepared>,
    pub keys: Mutex<Option<Keys>>,
    pub dir: PathBuf,
    pub cancel: Arc<AtomicBool>,
    created: Instant,
    received: tokio::sync::Mutex<BTreeSet<usize>>,
    pub state: Mutex<RunState>,
    /// Becomes true when the run ends.
    pub finished: watch::Sender<bool>,
}

impl Run {
    pub(super) fn new(
        assignment: Assignment,
        message: Vec<u8>,
        token: [u8; 32],
        prepared: Arc<Prepared>,
        keys: Keys,
        dir: PathBuf,
    ) -> Self {
        Self {
            assignment,
            message,
            token,
            prepared,
            keys: Mutex::new(Some(keys)),
            dir,
            cancel: Arc::default(),
            created: Instant::now(),
            received: tokio::sync::Mutex::default(),
            state: Mutex::new(RunState {
                phase: Phase::Awaiting,
                progress: None,
                ended: None,
            }),
            finished: watch::channel(false).0,
        }
    }

    /// What the cluster's report of this run is signed for.
    pub(super) fn digest(&self) -> [u8; 32] {
        calimero_identity::sha256(&self.message)
    }

    fn running(&self) -> bool {
        matches!(locked(&self.state).phase, Phase::Running)
    }

    /// Ends a run that never started with `phase`; one that runs ends itself. Whether it ended.
    pub(super) fn end_awaiting(&self, phase: Phase) -> bool {
        let mut state = locked(&self.state);
        if !matches!(state.phase, Phase::Awaiting) {
            return false;
        }
        state.phase = phase;
        state.ended = Some(Instant::now());
        drop(state);
        let _ended = self.finished.send_replace(true);
        true
    }

    fn active(&self) -> bool {
        matches!(locked(&self.state).phase, Phase::Awaiting | Phase::Running)
    }

    /// Ends the run with `phase` and wakes whoever waits for it.
    pub(super) fn end(&self, phase: Phase) {
        let mut state = locked(&self.state);
        state.phase = phase;
        state.ended = Some(Instant::now());
        drop(state);
        let _ended = self.finished.send_replace(true);
    }

    fn status(&self) -> Value {
        status_of(&locked(&self.state))
    }

    /// The status and whether the run runs, read under one lock.
    fn snapshot(&self) -> (Value, bool) {
        let state = locked(&self.state);
        (status_of(&state), matches!(state.phase, Phase::Running))
    }

    /// A wrong token answers as an unknown run does, so a probe learns nothing of what is held.
    fn authorize(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let given = headers
            .get(TOKEN_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if token_matches(&self.token, given) {
            return Ok(());
        }
        Err(unknown_run())
    }
}

fn status_of(state: &RunState) -> Value {
    match &state.phase {
        Phase::Awaiting => json!({ "state": "awaiting" }),
        Phase::Running => match &state.progress {
            Some((percent, step)) => {
                json!({ "state": "running", "progress": { "percent": percent, "step": step } })
            }
            None => json!({ "state": "running" }),
        },
        Phase::Done(done) => {
            json!({ "state": "done", "report": done.report, "signature": done.signature })
        }
        Phase::Lost => json!({ "state": "lost" }),
    }
}

fn unknown_run() -> ApiError {
    ApiError::new(404, "UNKNOWN_RUN", "this node holds no such run")
}

/// An attempt of a run of an app: run ids are the app's own, so two apps may share one.
type RunKey = (String, String, u32);

/// Every run this node holds, by app, run id and attempt. Lost when the process ends.
#[derive(Default)]
pub(super) struct Runs {
    map: Mutex<HashMap<RunKey, Arc<Run>>>,
}

impl Runs {
    /// Runs taking capacity: waiting for inputs or running.
    pub(super) fn active(&self) -> usize {
        locked(&self.map).values().filter(|r| r.active()).count()
    }

    fn get(&self, app: &str, run: &str, attempt: u32) -> Result<Arc<Run>, ApiError> {
        locked(&self.map)
            .get(&(app.to_owned(), run.to_owned(), attempt))
            .cloned()
            .ok_or_else(unknown_run)
    }

    fn remove(&self, run: &Run) {
        let _gone = locked(&self.map).remove(&key_of(&run.assignment));
    }

    /// Stops every run; the node is going away.
    pub(super) fn cancel_all(&self) {
        for run in locked(&self.map).values() {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Takes out finished or lost runs past [`JOB_RESULT_TTL`] and ones that waited past
    /// [`AWAITING_INPUTS_TTL`] for inputs, which end lost under their lock first.
    pub(super) fn sweep(&self, now: Instant) -> Vec<Arc<Run>> {
        let mut map = locked(&self.map);
        let due: Vec<RunKey> = map
            .iter()
            .filter(|(_, run)| {
                let mut state = locked(&run.state);
                match state.phase {
                    Phase::Awaiting if now.duration_since(run.created) >= AWAITING_INPUTS_TTL => {
                        state.phase = Phase::Lost;
                        state.ended = Some(now);
                        drop(state);
                        let _ended = run.finished.send_replace(true);
                        true
                    }
                    Phase::Awaiting | Phase::Running => false,
                    Phase::Done(_) | Phase::Lost => state
                        .ended
                        .is_some_and(|at| now.duration_since(at) >= JOB_RESULT_TTL),
                }
            })
            .map(|(key, _)| key.clone())
            .collect();
        due.iter().filter_map(|key| map.remove(key)).collect()
    }
}

fn key_of(a: &Assignment) -> RunKey {
    (a.app.clone(), a.run.clone(), a.attempt)
}

/// Bytes the outputs of the finished runs in `map` hold on disk.
fn finished_bytes(map: &HashMap<RunKey, Arc<Run>>) -> u64 {
    map.values()
        .map(|run| match &locked(&run.state).phase {
            Phase::Done(done) => done.bytes,
            _ => 0,
        })
        .sum()
}

/// The sealed bytes a run may hold on disk: its inputs, and the most its outputs may add.
fn reserve(a: &Assignment) -> u64 {
    let inputs: u64 = a.inputs.iter().map(|i| sealed_len(i.size)).sum();
    inputs.saturating_add(sealed_len(MAX_COMPUTE_FILE_BYTES))
}

/// Whether `held` sealed bytes of finished outputs, plus `reserved` for the runs that may
/// still write, fit within [`MAX_FINISHED_BYTES`].
fn has_room(held: u64, reserved: u64) -> bool {
    held.saturating_add(reserved) <= MAX_FINISHED_BYTES
}

pub(super) fn hash_hex(field: &str) -> Option<[u8; HASH_BYTES]> {
    hex::decode(field).ok()?.try_into().ok()
}

/// The refusals that need nothing but the assignment itself.
fn check_shape(a: &Assignment) -> Result<(), ApiError> {
    let ids = [&a.app, &a.run, &a.method, &a.owner];
    if ids
        .iter()
        .any(|id| id.is_empty() || id.len() > MAX_ID_BYTES)
    {
        return Err(ApiError::bad_request(
            "BAD_ASSIGNMENT",
            "an id is empty or too long",
        ));
    }
    if a.args.len() > MAX_ARGS_BYTES {
        return Err(ApiError::bad_request(
            "BAD_ASSIGNMENT",
            "the arguments are too large",
        ));
    }
    if hash_hex(&a.token).is_none() || !is_hash(&a.bundle) {
        return Err(ApiError::bad_request(
            "BAD_ASSIGNMENT",
            "the token and the bundle are 32-byte hex",
        ));
    }
    let b = &a.budget;
    let in_range = |v: u64, max: u64| (1..=max).contains(&v);
    if !in_range(b.fuel, MAX_COMPUTE_FUEL)
        || !in_range(b.memory_mib.into(), MAX_COMPUTE_MEMORY_MIB.into())
        || !in_range(b.wall_secs.into(), MAX_COMPUTE_WALL_SECS.into())
    {
        return Err(ApiError::bad_request(
            "BAD_BUDGET",
            "the budget is zero or over what a run may have",
        ));
    }
    if a.inputs.len() > MAX_RUN_INPUTS {
        return Err(ApiError::new(413, "TOO_MANY_INPUTS", "too many inputs"));
    }
    let mut total = 0u64;
    for input in &a.inputs {
        if !is_hash(&input.id) || hash_hex(&input.sha256).is_none() {
            return Err(ApiError::bad_request(
                "BAD_ASSIGNMENT",
                "an input is named by the hex SHA-256 of its ciphertext and has a plaintext hash",
            ));
        }
        if a.inputs.iter().filter(|other| other.id == input.id).count() > 1 {
            return Err(ApiError::bad_request(
                "BAD_ASSIGNMENT",
                "an input is named twice",
            ));
        }
        if input.size > MAX_FILE_BYTES {
            return Err(ApiError::new(
                413,
                "INPUT_TOO_LARGE",
                "an input is larger than a file may be",
            ));
        }
        total = total.saturating_add(input.size);
    }
    if total > MAX_RUN_INPUT_BYTES {
        return Err(ApiError::new(
            413,
            "INPUTS_TOO_LARGE",
            "the inputs add up to too much",
        ));
    }
    Ok(())
}

/// Opens every wrapped key of `a` with this node's transport key: a key made for another
/// node, attempt, run or place does not open.
fn unwrap_keys(job: &JobNode, a: &Assignment) -> Result<Keys, ApiError> {
    let bad = |what: &str| {
        ApiError::bad_request(
            "BAD_WRAP",
            &format!("the {what} does not open on this node"),
        )
    };
    let wrap = |kind, index| JobWrap {
        app_id: &a.app,
        run_id: &a.run,
        attempt: a.attempt,
        kind,
        index,
    };
    let seed = unwrap_for_job(&job.transport, &wrap(WrapKind::NoiseSeed, 0), &a.seed)
        .map_err(|_| bad("seed"))?;
    let mut inputs = Vec::new();
    for (i, input) in a.inputs.iter().enumerate() {
        let index = u32::try_from(i).map_err(|_| bad("input key"))?;
        let key = unwrap_for_job(
            &job.transport,
            &wrap(WrapKind::FileKey, index),
            &input.wrapped,
        )
        .map_err(|_| bad("input key"))?;
        inputs.push(key);
    }
    Ok(Keys { seed, inputs })
}

/// `POST /job/runs`: take an assignment, or answer what became of it.
pub(super) async fn assign(
    State(job): State<Arc<JobNode>>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let a: Assignment = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request("BAD_ASSIGNMENT", &format!("not an assignment: {e}")))?;
    check_shape(&a)?;
    admit_assignment(&job.config, &a)?;
    let message = assignment_message(&a).map_err(|e| ApiError::internal(&e.to_string()))?;
    let (run, is_new) = admit_run(&job, a, message)?;
    if is_new {
        tokio::fs::create_dir_all(&run.dir).await.map_err(|e| {
            job.runs.remove(&run);
            ApiError::internal(&format!("cannot make the run's directory: {e}"))
        })?;
        if run.assignment.inputs.is_empty() {
            exec::start(&job, &run);
        }
    }
    Ok(Json(run.status()))
}

/// Registers the run unless it is already held (then that one answers) or there is no room.
/// The flag is whether the run is new.
fn admit_run(
    job: &Arc<JobNode>,
    a: Assignment,
    message: Vec<u8>,
) -> Result<(Arc<Run>, bool), ApiError> {
    let mut map = locked(&job.runs.map);
    if let Some(held) = map.get(&key_of(&a)) {
        if held.message != message {
            return Err(ApiError::new(
                409,
                "RUN_CONFLICT",
                "another assignment already holds this run and attempt",
            ));
        }
        return Ok((Arc::clone(held), false));
    }
    let active = map.values().filter(|r| r.active()).count();
    if active >= RUNS_PER_JOB_NODE {
        return Err(ApiError::new(
            503,
            "BUSY",
            "this job node has no room for another run",
        ));
    }
    let reserved: u64 = map
        .values()
        .filter(|r| r.active())
        .map(|r| reserve(&r.assignment))
        .sum::<u64>()
        .saturating_add(reserve(&a));
    if !has_room(finished_bytes(&map), reserved) {
        return Err(ApiError::new(
            503,
            "BUSY",
            "finished runs wait to be collected: this job node holds too many outputs",
        ));
    }
    let prepared = job.bundles.get(&a.bundle).ok_or_else(|| {
        ApiError::new(
            409,
            "NEED_BUNDLE",
            "this node does not hold the bundle: PUT it first",
        )
    })?;
    let compute = prepared
        .abi
        .methods
        .get(&a.method)
        .is_some_and(|m| matches!(m.kind, MethodKind::Compute(_)));
    if !compute {
        return Err(ApiError::bad_request(
            "NOT_COMPUTE",
            "the bundle has no such compute method",
        ));
    }
    let keys = unwrap_keys(job, &a)?;
    let dir = job.scratch.join(job.next_dir().to_string());
    let token = hash_hex(&a.token).unwrap_or_default();
    let key = key_of(&a);
    let run = Arc::new(Run::new(a, message, token, prepared, keys, dir));
    let _none = map.insert(key, Arc::clone(&run));
    Ok((run, true))
}

fn find(
    job: &JobNode,
    app: &str,
    run: &str,
    attempt: u32,
    headers: &HeaderMap,
) -> Result<Arc<Run>, ApiError> {
    let run = job.runs.get(app, run, attempt)?;
    run.authorize(headers)?;
    Ok(run)
}

/// `PUT /job/runs/{app}/{run}/{attempt}/inputs/{i}`: the i-th input's ciphertext.
pub(super) async fn put_input(
    State(job): State<Arc<JobNode>>,
    Path((app, run, attempt, i)): Path<(String, String, u32, usize)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let run = find(&job, &app, &run, attempt, &headers)?;
    let input = run
        .assignment
        .inputs
        .get(i)
        .ok_or_else(|| ApiError::new(404, "NO_SUCH_INPUT", "the run has no such input"))?;
    if body.len() as u64 != sealed_len(input.size) {
        return Err(ApiError::bad_request(
            "BAD_INPUT_SIZE",
            "the ciphertext is not the size the input declares",
        ));
    }
    if hex::encode(calimero_identity::sha256(&body)) != input.id {
        return Err(ApiError::bad_request(
            "BAD_INPUT_ID",
            "the ciphertext is not the file the input names",
        ));
    }
    let mut received = run.received.lock().await;
    if received.contains(&i) {
        return Ok(Json(run.status()));
    }
    if !matches!(locked(&run.state).phase, Phase::Awaiting) {
        return Err(ApiError::new(
            409,
            "NOT_AWAITING",
            "the run takes no more inputs",
        ));
    }
    let key = locked(&run.keys)
        .as_ref()
        .map(|keys| Zeroizing::new(*keys.inputs[i]));
    let (size, sha256) = (input.size, hash_hex(&input.sha256));
    let (Some(key), Some(sha256)) = (key, sha256) else {
        return Err(ApiError::new(
            409,
            "NOT_AWAITING",
            "the run takes no more inputs",
        ));
    };
    let held = body.clone();
    let matches =
        tokio::task::spawn_blocking(move || exec::input_matches(&held, &key, size, &sha256))
            .await
            .map_err(|e| ApiError::internal(&e.to_string()))?;
    if !matches {
        // What the leader sent is not the file it declared: the report says so, signed.
        exec::fail_integrity(&job, &run);
        return Ok(Json(run.status()));
    }
    if let Err(e) = exec::store_input(&run.dir, i, &body).await {
        // A cancel removed the run's directory under this request.
        if e.kind() == std::io::ErrorKind::NotFound {
            return Err(ApiError::new(
                409,
                "NOT_AWAITING",
                "the run takes no more inputs",
            ));
        }
        return Err(ApiError::internal(&format!("cannot store the input: {e}")));
    }
    let _new = received.insert(i);
    let complete = received.len() == run.assignment.inputs.len();
    drop(received);
    if complete {
        exec::start(&job, &run);
    }
    Ok(Json(run.status()))
}

/// `GET /job/runs/{app}/{run}/{attempt}`.
pub(super) async fn status(
    State(job): State<Arc<JobNode>>,
    Path((app, run, attempt)): Path<(String, String, u32)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(find(&job, &app, &run, attempt, &headers)?.status()))
}

/// `read` of the `i`-th output of a finished run.
fn output_of<T>(run: &Run, i: usize, read: impl FnOnce(&OutputFile) -> T) -> Result<T, ApiError> {
    let state = locked(&run.state);
    let Phase::Done(done) = &state.phase else {
        return Err(ApiError::new(409, "NOT_DONE", "the run has not ended"));
    };
    done.outputs
        .get(i)
        .map(read)
        .ok_or_else(|| ApiError::new(404, "NO_SUCH_OUTPUT", "the run wrote no such output"))
}

/// `GET /job/runs/{app}/{run}/{attempt}/outputs/{i}`: the output's ciphertext.
pub(super) async fn output(
    State(job): State<Arc<JobNode>>,
    Path((app, run, attempt, i)): Path<(String, String, u32, usize)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let run = find(&job, &app, &run, attempt, &headers)?;
    let path = output_of(&run, i, |o| o.path.clone())?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| ApiError::internal(&format!("cannot read the output: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

/// `GET /job/runs/{app}/{run}/{attempt}/outputs/{i}/key`: the output's data key, hex. It travels
/// only inside the sealed channel.
pub(super) async fn output_key(
    State(job): State<Arc<JobNode>>,
    Path((app, run, attempt, i)): Path<(String, String, u32, usize)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let run = find(&job, &app, &run, attempt, &headers)?;
    let key = output_of(&run, i, |o| hex::encode(*o.key))?;
    Ok(Json(json!({ "key": key })))
}

/// `DELETE /job/runs/{app}/{run}/{attempt}`: stop the run if it runs, then drop it with its files.
/// Answers its last status; a run that did not stop within [`CANCEL_WAIT`] stays, and still runs.
pub(super) async fn cancel(
    State(job): State<Arc<JobNode>>,
    Path((app, run, attempt)): Path<(String, String, u32)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let run = find(&job, &app, &run, attempt, &headers)?;
    run.cancel.store(true, Ordering::Relaxed);
    let _ended = run.end_awaiting(Phase::Lost);
    if run.running() {
        let mut ended = run.finished.subscribe();
        let _ = tokio::time::timeout(CANCEL_WAIT, ended.wait_for(|ended| *ended)).await;
    }
    let (answer, running) = run.snapshot();
    if running {
        return Ok(Json(answer));
    }
    job.runs.remove(&run);
    let _ = tokio::fs::remove_dir_all(&run.dir).await;
    Ok(Json(answer))
}

#[cfg(test)]
mod tests {
    use calimero_app_host::Abi;

    use super::*;
    use crate::job_wire::sample_assignment;

    fn run(name: &str) -> Arc<Run> {
        let mut a = sample_assignment();
        a.run = name.to_owned();
        let prepared = Arc::new(Prepared {
            hash: String::new(),
            abi: Abi::default(),
            inner: Arc::new(()),
        });
        let keys = Keys {
            seed: Zeroizing::new([0; 32]),
            inputs: Vec::new(),
        };
        Arc::new(Run::new(
            a,
            Vec::new(),
            [0; 32],
            prepared,
            keys,
            PathBuf::new(),
        ))
    }

    fn held(runs: &Runs, run: &Arc<Run>) {
        let _none = locked(&runs.map).insert(key_of(&run.assignment), Arc::clone(run));
    }

    fn finished() -> Phase {
        Phase::Lost
    }

    #[test]
    fn a_run_waiting_for_inputs_is_dropped_after_its_time_and_a_running_one_never() {
        let runs = Runs::default();
        let waiting = run("waiting");
        let running = run("running");
        locked(&running.state).phase = Phase::Running;
        held(&runs, &waiting);
        held(&runs, &running);
        let now = Instant::now();
        assert!(runs
            .sweep(now + AWAITING_INPUTS_TTL - Duration::from_secs(1))
            .is_empty());
        let swept = runs.sweep(now + AWAITING_INPUTS_TTL + Duration::from_secs(1));
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].assignment.run, "waiting");
        // It ended lost before it left, so an input arriving now finds a run that takes none.
        assert!(matches!(locked(&swept[0].state).phase, Phase::Lost));
        assert!(*swept[0].finished.borrow());
        assert!(runs.sweep(now + JOB_RESULT_TTL * 24).is_empty());
        assert_eq!(runs.active(), 1);
    }

    #[test]
    fn an_ended_run_is_dropped_once_its_result_time_has_passed() {
        let runs = Runs::default();
        let ended = run("ended");
        ended.end(finished());
        held(&runs, &ended);
        assert_eq!(runs.active(), 0);
        let at = locked(&ended.state).ended.unwrap();
        assert!(runs
            .sweep(at + JOB_RESULT_TTL - Duration::from_secs(1))
            .is_empty());
        assert_eq!(runs.sweep(at + JOB_RESULT_TTL).len(), 1);
        assert!(runs.get("app", "ended", 1).is_err());
    }

    fn finished_with(bytes: u64) -> Phase {
        let report = crate::job_wire::Report {
            app: String::new(),
            run: String::new(),
            attempt: 1,
            status: crate::job_wire::ReportStatus::Done,
            reason: None,
            result: Vec::new(),
            outputs: Vec::new(),
            usage: crate::job_wire::ReportUsage::default(),
        };
        Phase::Done(Box::new(Finished {
            report,
            signature: String::new(),
            outputs: Vec::new(),
            bytes,
        }))
    }

    #[test]
    fn finished_outputs_count_against_the_nodes_room_until_collected() {
        let runs = Runs::default();
        assert_eq!(finished_bytes(&locked(&runs.map)), 0);
        assert!(has_room(0, reserve(&sample_assignment())));
        for (name, bytes) in [("a", 5), ("b", 7)] {
            let r = run(name);
            r.end(finished_with(bytes));
            held(&runs, &r);
        }
        let waiting = run("c");
        held(&runs, &waiting);
        assert_eq!(finished_bytes(&locked(&runs.map)), 12);
        // Every active run and the new one hold their inputs and may still write the most a run may.
        let one = reserve(&sample_assignment());
        assert_eq!(
            one,
            sealed_len(sample_assignment().inputs[0].size) + sealed_len(MAX_COMPUTE_FILE_BYTES)
        );
        for active in 0..3u64 {
            let reserved = (active + 1) * one;
            let limit = MAX_FINISHED_BYTES - reserved;
            assert!(has_room(limit, reserved), "{active}");
            assert!(!has_room(limit + 1, reserved), "{active}");
        }
        assert!(!has_room(0, 4 * one));
        assert!(!has_room(u64::MAX, 0));
    }

    #[test]
    fn a_cancelled_awaiting_run_ends_lost_and_wakes_its_waiters() {
        let waiting = run("waiting");
        let ended = waiting.finished.subscribe();
        assert!(waiting.end_awaiting(Phase::Lost));
        assert!(matches!(locked(&waiting.state).phase, Phase::Lost));
        assert!(*ended.borrow());
        // A run that runs is not ended from outside.
        let running = run("running");
        locked(&running.state).phase = Phase::Running;
        assert!(!running.end_awaiting(Phase::Lost));
        assert!(running.running());
    }

    #[test]
    fn the_status_names_the_phase_and_the_last_progress() {
        let r = run("status");
        assert_eq!(r.status(), json!({ "state": "awaiting" }));
        locked(&r.state).phase = Phase::Running;
        assert_eq!(r.status(), json!({ "state": "running" }));
        locked(&r.state).progress = Some((40, "hashing".to_owned()));
        assert_eq!(
            r.status(),
            json!({ "state": "running", "progress": { "percent": 40, "step": "hashing" } })
        );
        r.end(Phase::Lost);
        assert_eq!(r.status(), json!({ "state": "lost" }));
    }

    #[test]
    fn an_assignment_is_checked_for_ids_budget_inputs_and_hashes() {
        assert!(check_shape(&sample_assignment()).is_ok());
        let refused = |edit: &dyn Fn(&mut Assignment)| {
            let mut a = sample_assignment();
            edit(&mut a);
            check_shape(&a).unwrap_err().code
        };
        assert_eq!(refused(&|a| a.app.clear()), "BAD_ASSIGNMENT");
        assert_eq!(
            refused(&|a| a.method = "m".repeat(MAX_ID_BYTES + 1)),
            "BAD_ASSIGNMENT"
        );
        assert_eq!(
            refused(&|a| a.args = vec![0; MAX_ARGS_BYTES + 1]),
            "BAD_ASSIGNMENT"
        );
        assert_eq!(refused(&|a| a.bundle = "zz".into()), "BAD_ASSIGNMENT");
        assert_eq!(
            refused(&|a| a.inputs[0].sha256 = "zz".into()),
            "BAD_ASSIGNMENT"
        );
        assert_eq!(refused(&|a| a.inputs[0].id.clear()), "BAD_ASSIGNMENT");
        assert_eq!(
            refused(&|a| a.inputs[0].id = "CD".repeat(32)),
            "BAD_ASSIGNMENT"
        );
        assert_eq!(
            refused(&|a| a.inputs.push(a.inputs[0].clone())),
            "BAD_ASSIGNMENT"
        );
        assert_eq!(refused(&|a| a.budget.wall_secs = 0), "BAD_BUDGET");
        assert_eq!(
            refused(&|a| a.budget.memory_mib = MAX_COMPUTE_MEMORY_MIB + 1),
            "BAD_BUDGET"
        );
        assert_eq!(
            refused(&|a| a.inputs[0].size = MAX_FILE_BYTES + 1),
            "INPUT_TOO_LARGE"
        );
        assert_eq!(
            refused(&|a| a.inputs = vec![a.inputs[0].clone(); MAX_RUN_INPUTS + 1]),
            "TOO_MANY_INPUTS"
        );
    }
}
