//! CSV redaction on compute runs. The org admin sets one policy (`setPolicy`); a member
//! uploads a CSV as a sealed file and calls `submit`. A preview run (the first rows, redacted,
//! in its result) comes first and counts the rows; then one run redacts each part of the file.
//! At most `MAX_ACTIVE` runs are active at once, the rest wait their turn. Method names and
//! arguments are the ABI's camelCase ones.
//!
//! The rules engine is `rules` and runs in the compute component; `jobs` is a job's state.
//! `share` publishes a done job's outputs as plain blobs, for its submitter or an admin; a grant
//! (`blobs.grant`) then hands each out.

mod jobs;
mod rules;

use calimero_sdk::compute::{self, ComputeOutcome, RunId};
use calimero_sdk::env::{Caller, OrgRole};
use calimero_sdk::prelude::*;
use calimero_sdk::{files, BlobRef};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use jobs::{decode, Job, JobView, Publisher, Status};
use rules::{Policy, Preview};

/// Runs active at once in this app.
const MAX_ACTIVE: u64 = 4;
/// Rows of a file the preview redacts at most.
const PREVIEW_ROWS: usize = 500;
/// JSON bytes of the preview's rows at most, which leaves room under a result's 64 KiB.
const PREVIEW_BYTES: usize = 48 * 1024;
/// Rows per part when `submit` names none.
const DEFAULT_ROWS_PER_PART: u32 = 50_000;
/// Jobs one `jobs` call lists at most.
const PAGE: u32 = 1000;
/// Outputs one `share` call publishes at most, which is the host's cap on a logged call.
const SHARE_BATCH: usize = 16;

/// A run to make: a job's preview (`part` none) or one of its parts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub job: String,
    pub part: Option<u32>,
}

impl Task {
    /// The key a waiting task is kept under: first come, first served, a job's preview first.
    fn key(&self) -> String {
        format!("{}/{:05}", self.job, self.part.map_or(0, |p| p + 1))
    }
}

#[app::state(version = 1)]
pub struct Redact {
    /// The policy `submit` copies into each job.
    policy: Cell<Policy>,
    /// The last job number given out.
    last_job: Cell<u64>,
    /// Every job by id.
    jobs: UnorderedMap<String, Job>,
    /// The active runs, by run id.
    runs: UnorderedMap<String, Task>,
    /// The tasks waiting for a free slot, by `Task::key`.
    waiting: UnorderedMap<String, Task>,
}

#[app::error]
pub enum Error {
    MembersOnly,
    AdminsOnly,
    NoPolicy,
    BadPolicy { reason: String },
    NoSuchFile,
    NotFound { id: String },
    NotDone { id: String },
    NoInput,
    Csv { message: String },
}

/// What a part's run answers.
#[derive(Debug, Serialize, Deserialize)]
pub struct Redacted {
    pub rows: u64,
    pub cells: u64,
}

fn account() -> Option<String> {
    match env::caller() {
        Caller::Account(a) | Caller::ApiKey { account: a, .. } => Some(a),
        _ => None,
    }
}

fn role() -> Option<OrgRole> {
    env::org()?;
    env::org_role(&account()?)
}

fn csv_error(e: csv::Error) -> AppError {
    Error::Csv {
        message: e.to_string(),
    }
    .into()
}

fn input() -> app::Result<BlobRef> {
    compute::inputs()
        .into_iter()
        .next()
        .ok_or_else(|| Error::NoInput.into())
}

fn percent(done: u64, total: u64) -> u8 {
    u8::try_from(done * 100 / total.max(1)).unwrap_or(100)
}

/// Starts a run of a method over a file.
type Starter<'a> = dyn FnMut(&str, &Value, &BlobRef) -> app::Result<RunId> + 'a;

fn start_run(method: &str, args: &Value, file: &BlobRef) -> app::Result<RunId> {
    compute::start(method, args, std::slice::from_ref(file))
}

#[app::logic]
impl Redact {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Replaces the policy later submissions use. Admins and owners only; running jobs keep
    /// the policy they were submitted with.
    pub fn set_policy(&mut self, policy: Policy) -> app::Result<()> {
        if !role().is_some_and(|r| r >= OrgRole::Admin) {
            return Err(Error::AdminsOnly.into());
        }
        if policy.salt.is_empty() {
            return Err(Error::BadPolicy {
                reason: "the salt is empty".to_owned(),
            }
            .into());
        }
        self.policy.set(&policy)?;
        Ok(())
    }

    /// Redacts the CSV `file` (a sealed file of the app) under the current policy and answers
    /// the job's id. `rows_per_part` sizes the parts. Members and above.
    pub fn submit(&mut self, file: BlobRef, rows_per_part: Option<u32>) -> app::Result<String> {
        let Some(who) = account().filter(|_| role().is_some_and(|r| r >= OrgRole::Member)) else {
            return Err(Error::MembersOnly.into());
        };
        let Some(policy) = self.policy.get()? else {
            return Err(Error::NoPolicy.into());
        };
        if file.info().is_none() {
            return Err(Error::NoSuchFile.into());
        }
        let rows_per_part = rows_per_part.unwrap_or(DEFAULT_ROWS_PER_PART);
        let id = self.enqueue(Job::new(who, file, policy, rows_per_part))?;
        self.pump(&mut start_run)?;
        Ok(id)
    }

    /// Publishes the outputs of the done job `id` as plain blobs and answers the job, whose parts
    /// now name them. Its submitter and admins only. A job of more than 16 parts takes one call
    /// per 16; an output past 47 MiB cannot be published, so size the parts below it.
    pub fn share(&mut self, job: String) -> app::Result<JobView> {
        if !self.jobs.get(&job)?.is_some_and(|j| self.may_see(&j)) {
            return Err(Error::NotFound { id: job }.into());
        }
        self.share_done(job, &mut compute::publish)
    }

    /// Receives a run's outcome: the preview plans the parts, a part's output is kept, a failed
    /// run fails its job; then free slots go to waiting tasks.
    #[app::compute_result]
    pub fn on_run(&mut self, outcome: ComputeOutcome) -> app::Result<()> {
        self.finish(outcome, &mut start_run)
    }

    /// The job `id`: its status, parts, counts, preview rows and policy hash. Its submitter
    /// and admins only.
    #[app::view]
    pub fn job(&self, id: String) -> app::Result<JobView> {
        match self.jobs.get(&id)? {
            Some(job) if self.may_see(&job) => Ok(job.view(&id)),
            _ => Err(Error::NotFound { id }.into()),
        }
    }

    /// The caller's jobs, oldest first; an admin's call lists everyone's.
    #[app::view]
    pub fn jobs(&self) -> app::Result<Vec<JobView>> {
        let page = self.jobs.page(FindOpts::default().limit(PAGE))?;
        Ok(page
            .items
            .iter()
            .filter(|(_, job)| self.may_see(job))
            .map(|(id, job)| job.view(id))
            .collect())
    }

    /// Redacts the preview: the first rows, and the file's row count. `job` only tells the
    /// runs of two jobs on one file apart.
    #[app::compute(
        fuel = 100_000_000_000,
        memory_mib = 256,
        wall_secs = 300,
        result = "on_run"
    )]
    pub fn preview(policy: Policy, job: String) -> app::Result<Preview> {
        let _ = job;
        let reader = files::open(&input()?)?;
        rules::preview(reader, &policy, PREVIEW_ROWS, PREVIEW_BYTES).map_err(csv_error)
    }

    /// Redacts rows `start..start + rows` into one output file, header first.
    #[app::compute(
        fuel = 100_000_000_000,
        memory_mib = 256,
        wall_secs = 300,
        result = "on_run"
    )]
    pub fn redact(policy: Policy, job: String, start: u32, rows: u32) -> app::Result<Redacted> {
        let _ = job;
        let reader = files::open(&input()?)?;
        let mut writer = files::create()?;
        let stats = rules::redact_part(
            reader,
            &mut writer,
            &policy,
            start.into(),
            rows.into(),
            |done, cells| {
                let step = format!("{done} of {rows} rows, {cells} cells redacted");
                compute::progress(percent(done, rows.into()), &step);
            },
        )
        .map_err(csv_error)?;
        writer.finish()?;
        Ok(Redacted {
            rows: stats.rows,
            cells: stats.cells,
        })
    }
}

impl Redact {
    /// Publishes a done job's next outputs through `publish`; the job must be there and visible.
    fn share_done(&mut self, id: String, publish: &mut Publisher) -> app::Result<JobView> {
        let Some(mut job) = self.jobs.get(&id)? else {
            return Err(Error::NotFound { id }.into());
        };
        if job.status != Status::Done {
            return Err(Error::NotDone { id }.into());
        }
        job.share(publish, SHARE_BATCH)?;
        self.jobs.insert(&id, &job)?;
        Ok(job.view(&id))
    }

    fn may_see(&self, job: &Job) -> bool {
        role().is_some_and(|r| r >= OrgRole::Admin) || account().as_deref() == Some(&job.submitter)
    }

    /// Stores `job` under a new id and queues its preview.
    fn enqueue(&mut self, job: Job) -> app::Result<String> {
        let n = self.last_job.get()?.unwrap_or(0) + 1;
        self.last_job.set(&n)?;
        let id = format!("{n:08}");
        self.jobs.insert(&id, &job)?;
        self.wait(Task {
            job: id.clone(),
            part: None,
        })?;
        Ok(id)
    }

    fn wait(&mut self, task: Task) -> app::Result<()> {
        self.waiting.insert(&task.key(), &task)?;
        Ok(())
    }

    /// Starts waiting tasks while fewer than `MAX_ACTIVE` runs are active. A task whose job
    /// failed is dropped, and a refused start fails its job.
    fn pump(&mut self, start: &mut Starter) -> app::Result<()> {
        while self.runs.len()? < MAX_ACTIVE {
            let Some((key, task)) = self.waiting.first()? else {
                break;
            };
            self.waiting.remove(&key)?;
            let Some(mut job) = self.jobs.get(&task.job)? else {
                continue;
            };
            if job.status == Status::Failed {
                continue;
            }
            let (method, args) = match task.part {
                None => ("preview", json!({ "policy": job.policy, "job": task.job })),
                Some(i) => {
                    let p = &job.parts[i as usize];
                    let args = json!({
                        "policy": job.policy, "job": task.job, "start": p.start, "rows": p.rows,
                    });
                    ("redact", args)
                }
            };
            match start(method, &args, &job.file) {
                Ok(run) => {
                    if let Some(i) = task.part {
                        job.part_started(i as usize);
                    }
                    self.runs.insert(&run.to_string(), &task)?;
                }
                Err(e) => job.fail(format!("a run was refused: {e}")),
            }
            self.jobs.insert(&task.job, &job)?;
        }
        Ok(())
    }

    fn finish(&mut self, outcome: ComputeOutcome, start: &mut Starter) -> app::Result<()> {
        let Some(task) = self.runs.remove(&outcome.run.to_string())? else {
            return Ok(());
        };
        if let Some(mut job) = self.jobs.get(&task.job)? {
            let done = match task.part {
                None => self.preview_done(&mut job, &task.job, &outcome),
                Some(i) => part_done(&mut job, i as usize, &outcome),
            };
            if let Err(reason) = done {
                job.fail(reason);
            }
            self.jobs.insert(&task.job, &job)?;
        }
        self.pump(start)
    }

    /// Plans the parts from the preview and queues them.
    fn preview_done(
        &mut self,
        job: &mut Job,
        id: &str,
        outcome: &ComputeOutcome,
    ) -> Result<(), String> {
        job.previewed(decode::<Preview>(outcome)?);
        for part in 0..job.parts.len() {
            let task = Task {
                job: id.to_owned(),
                part: Some(part as u32),
            };
            self.wait(task).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Keeps the part's output, or says why the run's outcome is no good.
fn part_done(job: &mut Job, index: usize, outcome: &ComputeOutcome) -> Result<(), String> {
    let redacted: Redacted = decode(outcome)?;
    let [output] = outcome.outputs.as_slice() else {
        return Err("the run wrote no single output".to_owned());
    };
    job.part_done(index, output.id, redacted.cells);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobs::Status;

    fn policy() -> Policy {
        Policy {
            salt: "pepper".to_owned(),
            columns: [("name".to_owned(), rules::Rule::Pseudonym)].into(),
        }
    }

    fn blob(n: u8) -> BlobRef {
        BlobRef::parse(&format!("{n:02x}").repeat(32)).unwrap()
    }

    /// Hands out run ids and remembers what was started.
    #[derive(Default)]
    struct Runner {
        started: Vec<(String, String, Value)>,
        refuse: bool,
    }

    impl Runner {
        fn starter(&mut self) -> impl FnMut(&str, &Value, &BlobRef) -> app::Result<RunId> + '_ {
            |method, args, _file| {
                if self.refuse {
                    return Err(Error::NoInput.into());
                }
                let run = format!("r{}", self.started.len());
                let job = args["job"].as_str().unwrap().to_owned();
                self.started
                    .push((run.clone(), method.to_owned(), json!([job, args])));
                Ok(RunId::from(run))
            }
        }
    }

    fn outcome(run: &str, result: Option<Value>, output: Option<u8>) -> ComputeOutcome {
        use base64::Engine as _;
        let result =
            result.map(|v| base64::engine::general_purpose::STANDARD.encode(v.to_string()));
        let outputs: Vec<Value> = output
            .map(|n| json!({ "id": blob(n).id(), "size": "1", "sha256": "00" }))
            .into_iter()
            .collect();
        serde_json::from_value(json!({
            "run": run, "method": "x", "attempt": 1,
            "status": if result.is_some() { "done" } else { "failed" },
            "reason": if result.is_some() { Value::Null } else { json!("fuel") },
            "result": result, "outputs": outputs,
            "usage": { "cpuSeconds": 0.0, "gibSeconds": 0.0, "fuel": "0" },
        }))
        .unwrap()
    }

    fn preview_of(rows: u64) -> Value {
        json!({ "header": ["name"], "rows": [], "total_rows": rows, "cells": 0 })
    }

    fn job(n: u8, rows_per_part: u32) -> Job {
        Job::new("ann".to_owned(), blob(n), policy(), rows_per_part)
    }

    #[test]
    fn no_more_than_four_runs_are_active_and_the_rest_wait() {
        let mut app = Redact::load();
        let mut runner = Runner::default();
        for n in 0..6 {
            app.enqueue(job(n, 10)).unwrap();
        }
        app.pump(&mut runner.starter()).unwrap();
        assert_eq!(runner.started.len(), 4);
        assert_eq!(app.runs.len().unwrap(), 4);
        assert_eq!(app.waiting.len().unwrap(), 2);

        // A preview finishing frees a slot; the waiting tasks run in job order, so its first part is next.
        let done = outcome("r0", Some(preview_of(25)), None);
        app.finish(done, &mut runner.starter()).unwrap();
        assert_eq!(app.runs.len().unwrap(), 4);
        assert_eq!(runner.started.len(), 5);
        assert_eq!(runner.started[4].1, "redact");
        assert_eq!(runner.started[4].2[0], "00000001");
    }

    #[test]
    fn parts_are_started_as_slots_free_and_the_job_completes() {
        let mut app = Redact::load();
        let mut runner = Runner::default();
        let id = app.enqueue(job(1, 10)).unwrap();
        app.pump(&mut runner.starter()).unwrap();
        let first = outcome("r0", Some(preview_of(25)), None);
        app.finish(first, &mut runner.starter()).unwrap();
        // Three parts of 10, 10 and 5 rows, all within the cap.
        assert_eq!(runner.started.len(), 4);
        assert_eq!(runner.started[3].2[1]["start"], 20);
        assert_eq!(runner.started[3].2[1]["rows"], 5);
        for (i, n) in [(1, 11u8), (2, 12), (3, 13)] {
            let part = Some(json!({ "rows": 10, "cells": 3 }));
            let done = outcome(&format!("r{i}"), part, Some(n));
            app.finish(done, &mut runner.starter()).unwrap();
        }
        let job = app.jobs.get(&id).unwrap().unwrap();
        assert_eq!(job.status, Status::Done);
        assert_eq!(job.view(&id).parts[2].output, Some(blob(13)));
        assert_eq!(app.runs.len().unwrap(), 0);
    }

    #[test]
    fn a_failed_run_fails_its_job_and_its_waiting_parts_never_start() {
        let mut app = Redact::load();
        let mut runner = Runner::default();
        let id = app.enqueue(job(1, 1)).unwrap();
        for n in 2..6 {
            app.enqueue(job(n, 1)).unwrap();
        }
        app.pump(&mut runner.starter()).unwrap();
        let first = outcome("r0", Some(preview_of(8)), None);
        app.finish(first, &mut runner.starter()).unwrap();
        // Parts 0..4 of job 1 wait behind the other jobs' previews; job 1's first part fails.
        let before = runner.started.len();
        let failed = outcome("r4", None, None);
        app.finish(failed, &mut runner.starter()).unwrap();
        let job = app.jobs.get(&id).unwrap().unwrap();
        assert_eq!(job.status, Status::Failed);
        assert!(runner.started.len() >= before);
        let after_fail: Vec<_> = runner.started[before..]
            .iter()
            .filter(|s| s.2[0] == id)
            .collect();
        assert!(after_fail.is_empty(), "no run starts for a failed job");
    }

    #[test]
    fn a_refused_start_fails_the_job_instead_of_the_call() {
        let mut app = Redact::load();
        let mut runner = Runner {
            refuse: true,
            ..Runner::default()
        };
        let id = app.enqueue(job(1, 10)).unwrap();
        app.pump(&mut runner.starter()).unwrap();
        let job = app.jobs.get(&id).unwrap().unwrap();
        assert_eq!(job.status, Status::Failed);
        assert_eq!(app.runs.len().unwrap(), 0);
    }

    fn done_job(n: u8) -> Job {
        let mut job = job(n, 10);
        job.previewed(serde_json::from_value(preview_of(15)).unwrap());
        for i in 0..2 {
            assert!(job.part_done(i, blob(50 + i as u8), 1));
        }
        job
    }

    fn publish_as_new(output: &BlobRef) -> app::Result<BlobRef> {
        Ok(blob(output.id().len() as u8))
    }

    #[test]
    fn sharing_a_done_job_publishes_its_parts_and_a_job_still_running_is_refused() {
        let mut app = Redact::load();
        let id = app.enqueue(job(1, 10)).unwrap();
        let err = app.share_done(id.clone(), &mut publish_as_new).unwrap_err();
        assert_eq!(err.code, "NotDone");

        app.jobs.insert(&id, &done_job(1)).unwrap();
        let view = app.share_done(id.clone(), &mut publish_as_new).unwrap();
        assert!(view.parts.iter().all(|p| p.published.is_some()));
        let stored = app.jobs.get(&id).unwrap().unwrap();
        assert!(stored.parts.iter().all(|p| p.published.is_some()));
    }

    #[test]
    fn an_unknown_run_is_ignored() {
        let mut app = Redact::load();
        let mut runner = Runner::default();
        app.finish(outcome("nope", None, None), &mut runner.starter())
            .unwrap();
        assert!(runner.started.is_empty());
    }
}
