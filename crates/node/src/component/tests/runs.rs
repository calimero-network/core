//! How a replica applies the compute run entries: every rule is a no-op or a state change
//! judged on the record, the entry and the marks alone.

use ed25519_dalek::SigningKey;
use std::sync::Arc;

use super::*;
use crate::job_wire::{sign_report, FailReason, OutputRef, Report, ReportStatus, ReportUsage};

const DIGEST: [u8; 32] = [9; 32];

type Edit = (&'static str, fn(&mut Report));
type Refusal = (&'static str, &'static str, fn(&mut FinalEntry));

/// A machine and the index its next entry gets.
struct World {
    r: Rig,
    last: u64,
}

impl World {
    fn new() -> Self {
        let mut r = Rig::new();
        let _i = r.apply(1, &init(0));
        Self { r, last: 1 }
    }

    fn push(&mut self, cmd: &ComponentCmd) -> Applied {
        self.last += 1;
        self.r.apply(self.last, &serde_json::to_vec(cmd).unwrap())
    }

    fn record(&self, run: &str) -> RunRecord {
        self.r.machine.run_record(run).expect("no record")
    }

    fn state(&self, run: &str) -> RunState {
        self.record(run).state
    }

    fn active(&self) -> Vec<String> {
        self.r.machine.active_runs()
    }

    /// A call that starts `starts` and cancels `cancels`.
    fn call(&mut self, starts: Vec<ComputeStart>, cancels: &[&str]) -> Applied {
        let mut cmd: ComponentCmd =
            serde_json::from_slice(&super::call(self.last, None, Vec::new())).unwrap();
        if let ComponentCmd::Call(c) = &mut cmd {
            c.outcome.compute_starts = starts;
            c.outcome.compute_cancels = cancels.iter().map(|c| (*c).to_owned()).collect();
        }
        self.push(&cmd)
    }

    fn start(&mut self, run: &str) {
        let a = self.call(vec![started(run)], &[]);
        assert!(!a.conflict, "{a:?}");
    }

    /// The next attempt of `run`, assigned to the job key of `signer`.
    fn assign(&mut self, run: &str, signer: &SigningKey) -> Applied {
        let n = u32::try_from(self.record(run).attempts.len()).unwrap() + 1;
        self.push(&attempt(run, n, signer, DIGEST))
    }

    /// A valid job final of `run`'s latest attempt, to edit.
    fn done(&self, run: &str, signer: &SigningKey) -> FinalEntry {
        let n = u32::try_from(self.record(run).attempts.len()).unwrap();
        let outputs = vec![out(1)];
        let report = report(run, n, ReportStatus::Done, None, &outputs);
        FinalEntry {
            run: run.into(),
            attempt: n,
            outcome: FinalOutcome::Done,
            signature: Some(sign_report(signer, &DIGEST, &report).unwrap()),
            report: Some(report),
            outputs,
            usage: usage(),
            result: Outcome {
                diff: vec![w("result", run)],
                ..Outcome::default()
            },
            base_index: self.last,
            ts_ms: 9,
            id_term: 1,
            id_seq: 7,
        }
    }

    fn finish(&mut self, entry: FinalEntry) -> Applied {
        self.push(&ComponentCmd::ComputeFinal(Box::new(entry)))
    }

    fn files(&self) -> Vec<String> {
        marked(&self.r)
    }

    fn wrote(&self, key: &str) -> Option<Vec<u8>> {
        self.r.machine.view().get(&stored_key(key))
    }
}

fn stored_key(key: &str) -> Vec<u8> {
    key.as_bytes().to_vec()
}

fn job_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn started(run: &str) -> ComputeStart {
    ComputeStart {
        run: run.into(),
        method: "digest".into(),
        args: Bytes(b"{\"n\":1}".to_vec()),
        inputs: vec![RunInputRef {
            id: "ab".repeat(32),
            size: 7,
            sha256: "cd".repeat(32),
        }],
        caps: RunCaps {
            fuel: 5,
            memory_mib: 64,
            wall_secs: 7,
        },
        result_method: "onDone".into(),
        owner: "alice".into(),
    }
}

fn attempt(run: &str, n: u32, signer: &SigningKey, digest: [u8; 32]) -> ComponentCmd {
    ComponentCmd::ComputeAttempt(Box::new(AttemptEntry {
        run: run.into(),
        attempt: n,
        node: u64::from(n) + 40,
        job_key: hex::encode(signer.verifying_key().to_bytes()),
        assignment_digest: hex::encode(digest),
        ts_ms: 100 + u64::from(n),
    }))
}

/// A file the run wrote: ids and hashes are hashes, as a report spells them.
fn out(n: u8) -> FileMark {
    FileMark {
        id: hex::encode([n; 32]),
        size: 10 + u64::from(n),
        sha256: Bytes(vec![n + 1; 32]),
        wrapped_key: Bytes(vec![2; 60]),
    }
}

fn usage() -> ReportUsage {
    ReportUsage {
        cpu_micros: 5,
        gib_micros: 6,
        fuel: 5,
    }
}

fn report(
    run: &str,
    attempt: u32,
    status: ReportStatus,
    reason: Option<FailReason>,
    outputs: &[FileMark],
) -> Report {
    Report {
        app: "app".into(),
        run: run.into(),
        attempt,
        status,
        reason,
        result: if status == ReportStatus::Done {
            b"\"ok\"".to_vec()
        } else {
            Vec::new()
        },
        outputs: outputs
            .iter()
            .map(|o| OutputRef {
                id: o.id.clone(),
                size: o.size,
                sha256: hex::encode(&o.sha256.0),
            })
            .collect(),
        usage: usage(),
    }
}

/// A leader outcome of the latest attempt, with no usage.
fn leader(w: &World, run: &str, outcome: FinalOutcome) -> FinalEntry {
    FinalEntry {
        outcome,
        report: None,
        signature: None,
        outputs: Vec::new(),
        usage: ReportUsage::default(),
        ..w.done(run, &job_key(1))
    }
}

/// The reason a permanent refusal answered: nothing changed and it is not a conflict.
fn why(a: &Applied) -> String {
    assert!(!a.conflict, "a conflict, not a refusal: {a:?}");
    assert_eq!(a.status, Status::AppError, "{a:?}");
    a.error.as_str().expect("no reason").to_owned()
}

#[test]
fn a_started_run_is_a_queued_record_and_an_active_id() {
    let mut w = World::new();
    w.start("r1");
    let rec = w.record("r1");
    assert_eq!(rec.state, RunState::Queued);
    assert_eq!(
        (
            rec.method.as_str(),
            rec.result_method.as_str(),
            rec.bundle.as_str()
        ),
        ("digest", "onDone", "b")
    );
    assert_eq!(
        rec.args_hash,
        hex::encode(calimero_identity::sha256(b"{\"n\":1}"))
    );
    assert_eq!(rec.args.0, b"{\"n\":1}");
    assert_eq!(rec.owner, "alice");
    assert_eq!(
        rec.caller,
        Caller::Account {
            account: "a".into()
        }
    );
    assert_eq!(rec.start_ts_ms, 1);
    assert_eq!(rec.inputs[0].size, 7);
    assert_eq!(rec.caps.fuel, 5);
    assert!(!rec.cancel_requested && rec.attempts.is_empty() && rec.final_.is_none());
    assert_eq!(w.active(), ["r1"]);
}

#[test]
fn a_run_starts_on_the_bundle_that_ran_it_whatever_is_installed_later() {
    let mut w = World::new();
    w.start("r1");
    let upgrade = ComponentCmd::Upgrade {
        bundle: "b2".into(),
        base_index: w.last,
    };
    let _a = w.push(&upgrade);
    w.start("r2");
    assert_eq!(
        (
            w.record("r1").bundle.as_str(),
            w.record("r2").bundle.as_str()
        ),
        ("b", "b2")
    );
}

#[test]
fn a_run_started_by_a_call_that_did_not_end_ok_is_not_recorded() {
    let mut w = World::new();
    let mut cmd: ComponentCmd =
        serde_json::from_slice(&super::call(w.last, None, Vec::new())).unwrap();
    if let ComponentCmd::Call(c) = &mut cmd {
        c.outcome.status = Status::AppError;
        c.outcome.compute_starts = vec![started("r1")];
    }
    let _a = w.push(&cmd);
    assert!(w.active().is_empty() && w.r.machine.run_record("r1").is_none());
}

#[test]
fn a_start_past_the_active_cap_or_of_a_known_id_is_dropped() {
    let mut w = World::new();
    let all: Vec<_> = (0..=host::MAX_ACTIVE_RUNS)
        .map(|i| started(&format!("r{i:02}")))
        .collect();
    let _a = w.call(all, &[]);
    assert_eq!(w.active().len(), host::MAX_ACTIVE_RUNS);
    assert!(w
        .r
        .machine
        .run_record(&format!("r{:02}", host::MAX_ACTIVE_RUNS))
        .is_none());
    // A known id keeps its record.
    let _a = w.push(&attempt("r00", 1, &job_key(1), DIGEST));
    let mut again = started("r00");
    again.method = "other".into();
    let _a = w.call(vec![again], &[]);
    assert_eq!(w.record("r00").method, "digest");
    assert_eq!(w.state("r00"), RunState::Running);
}

#[test]
fn a_cancel_flags_an_active_run_and_nothing_else() {
    let mut w = World::new();
    w.start("r1");
    w.start("r2");
    let _a = w.call(Vec::new(), &["r1", "nobody"]);
    assert!(w.record("r1").cancel_requested);
    assert!(!w.record("r2").cancel_requested);
    assert!(w.r.machine.run_record("nobody").is_none());
    // A finished run is not cancelled after the fact.
    let _a = w.assign("r2", &job_key(1));
    let done = w.done("r2", &job_key(1));
    assert!(!w.finish(done).conflict);
    let _a = w.call(Vec::new(), &["r2"]);
    assert!(!w.record("r2").cancel_requested);
}

#[test]
fn attempts_are_numbered_from_one_in_order_up_to_three() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    assert_eq!(
        why(&w.push(&attempt("r1", 2, &k, DIGEST))),
        "attempt is not the next one"
    );
    assert_eq!(
        why(&w.push(&attempt("r1", 0, &k, DIGEST))),
        "attempt is not the next one"
    );
    assert!(!w.assign("r1", &k).conflict);
    assert_eq!(w.state("r1"), RunState::Running);
    assert_eq!(
        why(&w.push(&attempt("r1", 1, &k, DIGEST))),
        "attempt is not the next one"
    );
    assert!(!w.assign("r1", &job_key(2)).conflict);
    assert!(!w.assign("r1", &job_key(3)).conflict);
    assert_eq!(
        why(&w.push(&attempt("r1", 4, &k, DIGEST))),
        "no attempts left"
    );
    let rec = w.record("r1");
    let nodes: Vec<_> = rec
        .attempts
        .iter()
        .map(|a| (a.attempt, a.node, a.ts_ms))
        .collect();
    assert_eq!(nodes, [(1, 41, 101), (2, 42, 102), (3, 43, 103)]);
    assert_eq!(
        rec.attempts[1].job_key,
        hex::encode(job_key(2).verifying_key().to_bytes())
    );
    assert_eq!(rec.attempts[0].assignment_digest, hex::encode(DIGEST));
}

#[test]
fn an_attempt_needs_an_active_run_without_a_cancel_and_hash_spelled_keys() {
    let mut w = World::new();
    let k = job_key(1);
    assert_eq!(
        why(&w.push(&attempt("nobody", 1, &k, DIGEST))),
        "run is not active"
    );
    w.start("r1");
    for (job_key, digest) in [
        ("zz".repeat(32), hex::encode(DIGEST)),
        (
            hex::encode(k.verifying_key().to_bytes()).to_uppercase(),
            hex::encode(DIGEST),
        ),
        (hex::encode(k.verifying_key().to_bytes()), "ab".repeat(31)),
    ] {
        let bad = ComponentCmd::ComputeAttempt(Box::new(AttemptEntry {
            run: "r1".into(),
            attempt: 1,
            node: 1,
            job_key,
            assignment_digest: digest,
            ts_ms: 1,
        }));
        assert_eq!(
            why(&w.push(&bad)),
            "job key or assignment digest is not a hash"
        );
    }
    let _a = w.call(Vec::new(), &["r1"]);
    assert_eq!(why(&w.assign("r1", &k)), "cancel was requested");
    assert!(w.record("r1").attempts.is_empty());
    w.start("r2");
    let _a = w.assign("r2", &k);
    let done = w.done("r2", &k);
    assert!(!w.finish(done).conflict);
    assert_eq!(
        why(&w.push(&attempt("r2", 2, &k, DIGEST))),
        "run is not active"
    );
}

#[test]
fn a_valid_job_final_marks_the_outputs_records_the_usage_and_applies_the_result() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let a = w.finish(w.done("r1", &k));
    assert!(!a.conflict && a.status == Status::Ok, "{a:?}");
    assert_eq!(w.files(), [out(1).id]);
    assert_eq!(w.r.machine.state.component.files[&out(1).id], out(1));
    assert_eq!(w.wrote("result"), Some(b"r1".to_vec()));
    let rec = w.record("r1");
    assert_eq!(rec.state, RunState::Done);
    let end = rec.final_.unwrap();
    assert_eq!(
        (end.attempt, end.outcome, end.usage, end.ts_ms),
        (1, FinalOutcome::Done, usage(), 9)
    );
    assert_eq!(end.outputs, [out(1)]);
    assert!(w.active().is_empty());
    let head = &w.r.machine.state.component.head;
    assert_eq!(head.finished_runs, ["r1"]);
    assert_eq!(head.last_id, (1, 7));
    assert_eq!(head.last_ts_ms, 9);
}

#[test]
fn only_the_first_final_of_a_run_applies() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let first = w.done("r1", &k);
    assert!(!w.finish(first).conflict);
    let mut second = w.done("r1", &k);
    second.result.diff = vec![w2("second")];
    assert_eq!(why(&w.finish(second)), "already final");
    assert_eq!(w.wrote("second"), None);
    let cancel = FinalEntry {
        base_index: w.last,
        ..leader(&w, "r1", FinalOutcome::Cancelled)
    };
    assert_eq!(why(&w.finish(cancel)), "already final");
    assert_eq!(w.state("r1"), RunState::Done);
    assert_eq!(w.r.machine.state.component.head.finished_runs.len(), 1);
}

fn w2(key: &str) -> Write {
    w(key, "x")
}

#[test]
fn a_final_for_an_unknown_run_changes_nothing() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let mut entry = w.done("r1", &k);
    entry.run = "nobody".into();
    assert_eq!(why(&w.finish(entry)), "run is not active");
    assert_eq!(w.active(), ["r1"]);
}

#[test]
fn a_forged_signature_or_another_attempts_key_is_a_no_op_and_the_real_final_still_applies() {
    let mut w = World::new();
    w.start("r1");
    let (first, second, forger) = (job_key(1), job_key(2), job_key(3));
    let _a = w.assign("r1", &first);
    let _a = w.assign("r1", &second);
    for signer in [&forger, &first] {
        let entry = w.done("r1", signer);
        assert_eq!(
            why(&w.finish(entry)),
            "report not signed by the assigned job node"
        );
        assert_eq!(w.state("r1"), RunState::Running);
        assert!(w.files().is_empty() && w.wrote("result").is_none());
    }
    let mut garbled = w.done("r1", &second);
    garbled.signature = Some("00".repeat(64));
    assert_eq!(
        why(&w.finish(garbled)),
        "report not signed by the assigned job node"
    );
    let mut short = w.done("r1", &second);
    short.signature = Some("zz".into());
    assert_eq!(
        why(&w.finish(short)),
        "report not signed by the assigned job node"
    );
    let real = w.done("r1", &second);
    assert!(!w.finish(real).conflict);
    assert_eq!(w.state("r1"), RunState::Done);
}

#[test]
fn a_report_bound_to_another_assignment_is_refused() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let mut entry = w.done("r1", &k);
    let report = entry.report.clone().unwrap();
    entry.signature = Some(sign_report(&k, &[8; 32], &report).unwrap());
    assert_eq!(
        why(&w.finish(entry)),
        "report not signed by the assigned job node"
    );
}

#[test]
fn a_final_of_an_attempt_that_is_not_the_latest_is_stale_even_when_signed() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let old = w.done("r1", &k);
    let _a = w.assign("r1", &job_key(2));
    let mut old = FinalEntry {
        base_index: w.last,
        ..old
    };
    assert_eq!(why(&w.finish(old.clone())), "stale attempt");
    old.attempt = 2;
    assert_eq!(why(&w.finish(old)), "report is for another run");
    assert_eq!(w.state("r1"), RunState::Running);
}

#[test]
fn a_report_for_another_app_run_or_attempt_is_refused() {
    let k = job_key(1);
    let edits: [Edit; 3] = [
        ("app", |r| r.app = "other".into()),
        ("run", |r| r.run = "r2".into()),
        ("attempt", |r| r.attempt = 2),
    ];
    for (name, edit) in edits {
        let mut w = World::new();
        w.start("r1");
        let _a = w.assign("r1", &k);
        let mut entry = w.done("r1", &k);
        let mut report = entry.report.take().unwrap();
        edit(&mut report);
        entry.signature = Some(sign_report(&k, &DIGEST, &report).unwrap());
        entry.report = Some(report);
        assert_eq!(why(&w.finish(entry)), "report is for another run", "{name}");
    }
}

#[test]
fn outputs_and_usage_must_be_the_reports() {
    let k = job_key(1);
    let edits: [Refusal; 7] = [
        ("id", "outputs differ from the report", |e| {
            e.outputs[0].id = hex::encode([5; 32])
        }),
        ("size", "outputs differ from the report", |e| {
            e.outputs[0].size += 1
        }),
        ("sha", "outputs differ from the report", |e| {
            e.outputs[0].sha256 = Bytes(vec![9; 32])
        }),
        ("missing", "outputs differ from the report", |e| {
            e.outputs.clear()
        }),
        ("extra", "outputs differ from the report", |e| {
            e.outputs.push(out(2))
        }),
        ("cpu", "usage differs from the report", |e| {
            e.usage.cpu_micros += 1
        }),
        ("fuel", "usage differs from the report", |e| {
            e.usage.fuel = 0
        }),
    ];
    for (name, reason, edit) in edits {
        let mut w = World::new();
        w.start("r1");
        let _a = w.assign("r1", &k);
        let mut entry = w.done("r1", &k);
        edit(&mut entry);
        assert_eq!(why(&w.finish(entry)), reason, "{name}");
        assert_eq!(w.state("r1"), RunState::Running, "{name}");
        assert!(w.files().is_empty(), "{name}");
    }
}

#[test]
fn the_outcome_must_be_what_the_report_says() {
    let k = job_key(1);
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    // A failed report for a done outcome, and the other way round.
    let mut entry = w.done("r1", &k);
    let failed = report("r1", 1, ReportStatus::Failed, Some(FailReason::Fuel), &[]);
    entry.signature = Some(sign_report(&k, &DIGEST, &failed).unwrap());
    entry.report = Some(failed.clone());
    entry.outputs.clear();
    assert_eq!(
        why(&w.finish(entry.clone())),
        "report does not match the outcome"
    );
    entry.outcome = FinalOutcome::Failed {
        reason: FailedReason::Time,
    };
    assert_eq!(
        why(&w.finish(entry.clone())),
        "report does not match the outcome"
    );
    entry.outcome = FinalOutcome::Failed {
        reason: FailedReason::Fuel,
    };
    entry.base_index = w.last;
    assert!(!w.finish(entry).conflict);
    let rec = w.record("r1");
    assert_eq!(rec.state, RunState::Failed);
    assert_eq!(
        rec.final_.unwrap().outcome,
        FinalOutcome::Failed {
            reason: FailedReason::Fuel
        }
    );
    assert!(w.files().is_empty());
}

#[test]
fn every_job_reason_ends_the_run_failed_with_its_report() {
    let k = job_key(1);
    let cases = [
        (FailedReason::Fuel, FailReason::Fuel),
        (FailedReason::Memory, FailReason::Memory),
        (FailedReason::Time, FailReason::Time),
        (FailedReason::Trap, FailReason::Trap),
        (FailedReason::AppError, FailReason::AppError),
        (FailedReason::Integrity, FailReason::Integrity),
    ];
    for (reason, job) in cases {
        let mut w = World::new();
        w.start("r1");
        let _a = w.assign("r1", &k);
        let mut entry = w.done("r1", &k);
        let failed = report("r1", 1, ReportStatus::Failed, Some(job), &[]);
        entry.signature = Some(sign_report(&k, &DIGEST, &failed).unwrap());
        entry.report = Some(failed);
        entry.outputs.clear();
        entry.outcome = FinalOutcome::Failed { reason };
        assert!(!w.finish(entry).conflict, "{reason:?}");
        assert_eq!(w.state("r1"), RunState::Failed);
    }
}

#[test]
fn a_job_outcome_without_a_report_and_a_leader_outcome_with_one_are_refused() {
    let k = job_key(1);
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let mut bare = w.done("r1", &k);
    bare.report = None;
    assert_eq!(why(&w.finish(bare)), "a job outcome needs a signed report");
    let mut unsigned = w.done("r1", &k);
    unsigned.signature = None;
    assert_eq!(
        why(&w.finish(unsigned)),
        "a job outcome needs a signed report"
    );
    for reason in [FailedReason::Fuel, FailedReason::Integrity] {
        let entry = FinalEntry {
            outcome: FinalOutcome::Failed { reason },
            ..leader(&w, "r1", FinalOutcome::Done)
        };
        assert_eq!(why(&w.finish(entry)), "a job outcome needs a signed report");
    }
    let _a = w.call(Vec::new(), &["r1"]);
    let mut signed = leader(&w, "r1", FinalOutcome::Cancelled);
    signed.signature = Some("00".repeat(64));
    assert_eq!(why(&w.finish(signed)), "a leader outcome carries no report");
    assert_eq!(w.state("r1"), RunState::Running);
}

#[test]
fn a_cancelled_final_needs_a_requested_cancel_whoever_signs_it() {
    let k = job_key(1);
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let unsigned = leader(&w, "r1", FinalOutcome::Cancelled);
    assert_eq!(why(&w.finish(unsigned)), "cancel was not requested");
    let signed = signed_cancel(&w, "r1", &k);
    assert_eq!(why(&w.finish(signed)), "cancel was not requested");
    assert_eq!(w.state("r1"), RunState::Running);
}

/// The job node's report of a run it stopped at the leader's cancel, in a final entry.
fn signed_cancel(w: &World, run: &str, signer: &SigningKey) -> FinalEntry {
    let rep = report(
        run,
        1,
        ReportStatus::Failed,
        Some(FailReason::Cancelled),
        &[],
    );
    FinalEntry {
        signature: Some(sign_report(signer, &DIGEST, &rep).unwrap()),
        report: Some(rep),
        outputs: Vec::new(),
        usage: usage(),
        ..leader(w, run, FinalOutcome::Cancelled)
    }
}

#[test]
fn an_unsigned_cancel_is_not_charged_and_a_signed_one_carries_the_reports_usage() {
    let k = job_key(1);
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let _a = w.call(Vec::new(), &["r1"]);
    let mut unsigned = leader(&w, "r1", FinalOutcome::Cancelled);
    unsigned.usage = usage();
    assert_eq!(why(&w.finish(unsigned)), "a leader outcome is not charged");
    // The signed form is verified like any job outcome.
    let mut forged = signed_cancel(&w, "r1", &job_key(2));
    forged.base_index = w.last;
    assert_eq!(
        why(&w.finish(forged)),
        "report not signed by the assigned job node"
    );
    let mut other_usage = signed_cancel(&w, "r1", &k);
    other_usage.usage.fuel += 1;
    assert_eq!(why(&w.finish(other_usage)), "usage differs from the report");
    let mut done_report = signed_cancel(&w, "r1", &k);
    let rep = report("r1", 1, ReportStatus::Done, None, &[]);
    done_report.signature = Some(sign_report(&k, &DIGEST, &rep).unwrap());
    done_report.report = Some(rep);
    assert_eq!(
        why(&w.finish(done_report)),
        "report does not match the outcome"
    );
    assert_eq!(w.state("r1"), RunState::Running);
    let mut ok = signed_cancel(&w, "r1", &k);
    ok.base_index = w.last;
    assert!(!w.finish(ok).conflict);
    let rec = w.record("r1");
    assert_eq!(rec.state, RunState::Cancelled);
    assert_eq!(rec.final_.unwrap().usage, usage());
    assert!(w.active().is_empty());
}

#[test]
fn an_unsigned_cancel_ends_the_run_cancelled_with_no_charge() {
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &job_key(1));
    let _a = w.call(Vec::new(), &["r1"]);
    let entry = leader(&w, "r1", FinalOutcome::Cancelled);
    assert!(!w.finish(entry).conflict);
    let rec = w.record("r1");
    assert_eq!(rec.state, RunState::Cancelled);
    assert_eq!(rec.final_.unwrap().usage, ReportUsage::default());
}

#[test]
fn a_job_reported_cancel_as_a_failed_reason_also_needs_the_cancel() {
    let k = job_key(1);
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let mut entry = signed_cancel(&w, "r1", &k);
    entry.outcome = FinalOutcome::Failed {
        reason: FailedReason::Cancelled,
    };
    assert_eq!(why(&w.finish(entry.clone())), "cancel was not requested");
    let _a = w.call(Vec::new(), &["r1"]);
    entry.base_index = w.last;
    assert!(!w.finish(entry).conflict);
    assert_eq!(w.state("r1"), RunState::Cancelled);
}

#[test]
fn a_queued_run_can_be_cancelled_with_no_attempt() {
    let mut w = World::new();
    w.start("r1");
    let _a = w.call(Vec::new(), &["r1"]);
    let mut entry = leader(&w, "r1", FinalOutcome::Cancelled);
    entry.attempt = 0;
    assert!(!w.finish(entry).conflict);
    assert_eq!(w.state("r1"), RunState::Cancelled);
}

#[test]
fn a_done_report_that_beats_the_cancel_still_ends_the_run_done() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let _a = w.call(Vec::new(), &["r1"]);
    assert!(!w.finish(w.done("r1", &k)).conflict);
    assert_eq!(w.state("r1"), RunState::Done);
}

#[test]
fn lost_needs_every_attempt_used_and_timeout_a_run_that_was_assigned() {
    let mut w = World::new();
    w.start("r1");
    let lost = FailedReason::Lost;
    let timeout = FailedReason::Timeout;
    let entry = leader(&w, "r1", FinalOutcome::Failed { reason: timeout });
    assert_eq!(
        why(&w.finish(FinalEntry {
            attempt: 0,
            ..entry
        })),
        "the run was never assigned"
    );
    for n in 1..=MAX_ATTEMPTS {
        let _a = w.assign("r1", &job_key(n as u8));
        let early = leader(&w, "r1", FinalOutcome::Failed { reason: lost });
        let expected = if n < MAX_ATTEMPTS {
            "the run has attempts left"
        } else {
            ""
        };
        if n < MAX_ATTEMPTS {
            assert_eq!(why(&w.finish(early)), expected);
        }
    }
    let entry = leader(&w, "r1", FinalOutcome::Failed { reason: lost });
    assert!(!w.finish(entry).conflict);
    assert_eq!(w.state("r1"), RunState::Failed);
    assert_eq!(
        w.record("r1").final_.unwrap().outcome,
        FinalOutcome::Failed { reason: lost }
    );

    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &job_key(1));
    let entry = leader(&w, "r1", FinalOutcome::Failed { reason: timeout });
    assert!(!w.finish(entry).conflict);
    assert_eq!(w.state("r1"), RunState::Failed);
}

#[test]
fn a_leader_outcome_has_no_outputs_and_a_lost_or_timed_out_run_no_usage() {
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &job_key(1));
    let _a = w.call(Vec::new(), &["r1"]);
    let mut entry = leader(&w, "r1", FinalOutcome::Cancelled);
    entry.outputs = vec![out(1)];
    assert_eq!(why(&w.finish(entry)), "a leader outcome has no outputs");
    let mut charged = leader(
        &w,
        "r1",
        FinalOutcome::Failed {
            reason: FailedReason::Timeout,
        },
    );
    charged.usage = usage();
    assert_eq!(why(&w.finish(charged)), "a leader outcome is not charged");
    assert!(w.files().is_empty());
    assert_eq!(w.state("r1"), RunState::Running);
}

#[test]
fn an_output_that_is_already_a_file_or_listed_twice_is_refused() {
    let k = job_key(1);
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let held = Box::new(CallEntry {
        outcome: Outcome {
            files: vec![out(1)],
            ..Outcome::default()
        },
        ..call_entry(w.last)
    });
    let _a = w.push(&ComponentCmd::Call(held));
    let entry = w.done("r1", &k);
    assert_eq!(why(&w.finish(entry)), "output is already a file");

    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let mut entry = w.done("r1", &k);
    let twice = vec![out(1), out(1)];
    let rep = report("r1", 1, ReportStatus::Done, None, &twice);
    entry.signature = Some(sign_report(&k, &DIGEST, &rep).unwrap());
    entry.report = Some(rep);
    entry.outputs = twice;
    assert_eq!(why(&w.finish(entry)), "output is already a file");
}

fn call_entry(base: u64) -> CallEntry {
    let ComponentCmd::Call(c) =
        serde_json::from_slice(&super::call(base, None, Vec::new())).unwrap()
    else {
        unreachable!()
    };
    *c
}

#[test]
fn a_final_on_a_state_that_moved_on_conflicts_and_leaves_the_run_active() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let stale = w.done("r1", &k);
    // Another entry applies first.
    let _a = w.call(Vec::new(), &[]);
    let a = w.finish(stale.clone());
    assert!(a.conflict, "a stale base is a conflict: {a:?}");
    assert_eq!(a.error["detail"], "conflict: retry");
    assert_eq!(w.state("r1"), RunState::Running);
    assert_eq!(w.active(), ["r1"]);
    assert!(w.files().is_empty() && w.wrote("result").is_none());
    assert!(w.record("r1").final_.is_none());
    assert!(w.r.machine.state.component.head.finished_runs.is_empty());
    // The next leader re-drives it on the current state.
    let again = FinalEntry {
        base_index: w.last,
        ..stale
    };
    assert!(!w.finish(again).conflict);
    assert_eq!(w.state("r1"), RunState::Done);
}

#[test]
fn the_outputs_stay_marked_when_the_result_method_failed() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let mut entry = w.done("r1", &k);
    entry.result = Outcome {
        status: Status::AppError,
        diff: vec![w2("nope")],
        ..Outcome::default()
    };
    let a = w.finish(entry);
    assert!(!a.conflict);
    assert_eq!(a.status, Status::AppError);
    assert_eq!(w.state("r1"), RunState::Done);
    assert_eq!(w.files(), [out(1).id]);
    assert_eq!(w.wrote("nope"), None);
}

#[test]
fn the_result_method_s_own_starts_and_cancels_apply_after_the_run_is_over() {
    let mut w = World::new();
    w.start("r1");
    w.start("r2");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    let mut entry = w.done("r1", &k);
    entry.result.compute_starts = vec![started("r3")];
    entry.result.compute_cancels = vec!["r2".into(), "r1".into()];
    assert!(!w.finish(entry).conflict);
    assert_eq!(w.active(), ["r2", "r3"]);
    assert!(w.record("r2").cancel_requested);
    assert!(
        !w.record("r1").cancel_requested,
        "a finished run is not cancelled"
    );
}

#[test]
fn the_result_method_frees_the_slot_of_the_run_that_ended_for_its_own_start() {
    let mut w = World::new();
    let all: Vec<_> = (0..host::MAX_ACTIVE_RUNS)
        .map(|i| started(&format!("r{i:02}")))
        .collect();
    let _a = w.call(all, &[]);
    let k = job_key(1);
    let _a = w.assign("r00", &k);
    let mut entry = w.done("r00", &k);
    entry.result.compute_starts = vec![started("next")];
    assert!(!w.finish(entry).conflict);
    assert!(w.active().contains(&"next".to_owned()));
    assert_eq!(w.active().len(), host::MAX_ACTIVE_RUNS);
}

fn finish_quickly(w: &mut World, run: &str) {
    let _a = w.assign(run, &job_key(1));
    let mut entry = w.done(run, &job_key(1));
    entry.outputs.clear();
    let rep = report(run, 1, ReportStatus::Done, None, &[]);
    entry.signature = Some(sign_report(&job_key(1), &DIGEST, &rep).unwrap());
    entry.report = Some(rep);
    entry.result = Outcome::default();
    assert!(!w.finish(entry).conflict);
}

fn feed(total: usize) -> World {
    let mut w = World::new();
    for batch in (0..total).collect::<Vec<_>>().chunks(40) {
        let starts: Vec<_> = batch.iter().map(|i| started(&format!("r{i:03}"))).collect();
        let _a = w.call(starts, &[]);
        for i in batch {
            finish_quickly(&mut w, &format!("r{i:03}"));
        }
    }
    w
}

#[test]
fn the_oldest_finished_records_expire_identically_on_every_replica() {
    let total = FINISHED_RUNS_KEPT + 30;
    let (one, two) = (feed(total), feed(total));
    let kept: Vec<String> = (30..total).map(|i| format!("r{i:03}")).collect();
    for w in [&one, &two] {
        let head = &w.r.machine.state.component.head;
        assert_eq!(head.finished_runs.iter().cloned().collect::<Vec<_>>(), kept);
        assert!(head.active_runs.is_empty());
        for i in 0..total {
            let id = format!("r{i:03}");
            assert_eq!(w.r.machine.run_record(&id).is_some(), i >= 30, "{id}");
        }
    }
    assert_eq!(
        one.r.machine.state.component.head,
        two.r.machine.state.component.head
    );
}

#[test]
fn runs_come_back_from_their_rows_after_a_restore() {
    let mut w = World::new();
    w.start("r1");
    w.start("r2");
    w.start("r3");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    assert!(!w.finish(w.done("r1", &k)).conflict);
    let _a = w.assign("r2", &k);
    let mut fresh = AppMachine::new("app".into());
    calimero_log::Machine::attach(&mut fresh, w.r.engine.group_state(5));
    calimero_log::Machine::restore(&mut fresh, &[]).unwrap();
    assert_eq!(fresh.active_runs(), ["r2", "r3"]);
    assert_eq!(fresh.state.component.head.finished_runs, ["r1"]);
    for id in ["r1", "r2", "r3"] {
        assert_eq!(fresh.run_record(id), Some(w.record(id)));
    }
    assert_eq!(fresh.run_record("r2").unwrap().state, RunState::Running);
}

#[test]
fn the_host_sees_the_active_runs_only() {
    let mut w = World::new();
    w.start("r1");
    w.start("r2");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    assert!(!w.finish(w.done("r1", &k)).conflict);
    let seen = w.r.machine.runs_provider(None).active().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].run, "r2");
    assert_eq!(seen[0].method, "digest");
    assert_eq!(
        seen[0].args_sha256,
        calimero_identity::sha256(b"{\"n\":1}").to_vec()
    );
    assert_eq!(seen[0].inputs, ["ab".repeat(32)]);
}

#[test]
fn the_entries_have_a_fixed_wire_shape() {
    let k = job_key(1);
    let attempt = attempt("r1", 1, &k, DIGEST);
    let wire = serde_json::to_value(&attempt).unwrap();
    assert_eq!(
        wire,
        json!({
            "op": "computeAttempt", "run": "r1", "attempt": 1, "node": 41,
            "jobKey": hex::encode(k.verifying_key().to_bytes()),
            "assignmentDigest": hex::encode(DIGEST), "tsMs": 101,
        })
    );
    let wire = |o| serde_json::to_value(o).unwrap();
    assert_eq!(wire(FinalOutcome::Done), json!({ "kind": "done" }));
    assert_eq!(
        wire(FinalOutcome::Cancelled),
        json!({ "kind": "cancelled" })
    );
    assert_eq!(
        wire(FinalOutcome::Failed {
            reason: FailedReason::AppError
        }),
        json!({ "kind": "failed", "reason": "app-error" })
    );
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let entry = ComponentCmd::ComputeFinal(Box::new(leader(&w, "r1", FinalOutcome::Cancelled)));
    let back: ComponentCmd = serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
    assert!(matches!(back, ComponentCmd::ComputeFinal(_)));
    assert_eq!(serde_json::to_value(&entry).unwrap()["op"], "computeFinal");
}

#[test]
fn a_run_record_has_the_documented_shape() {
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &job_key(1));
    let shown = serde_json::to_value(w.record("r1")).unwrap();
    for key in [
        "run",
        "method",
        "args",
        "argsHash",
        "inputs",
        "caps",
        "resultMethod",
        "bundle",
        "caller",
        "owner",
        "startTsMs",
        "state",
        "cancelRequested",
        "attempts",
        "final",
    ] {
        assert!(shown.get(key).is_some(), "missing {key}: {shown}");
    }
    assert_eq!(shown["state"], "running");
    assert_eq!(shown["final"], Value::Null);
    assert_eq!(
        shown["caps"],
        json!({ "fuel": 5, "memoryMib": 64, "wallSecs": 7 })
    );
}

#[test]
fn state_logged_before_runs_existed_still_decodes() {
    let head: ComponentHead = serde_json::from_str(r#"{"lastTsMs":5}"#).unwrap();
    assert!(head.active_runs.is_empty() && head.finished_runs.is_empty());
    let outcome: Outcome = serde_json::from_str(r#"{"status":"ok"}"#).unwrap();
    assert!(outcome.compute_starts.is_empty() && outcome.compute_cancels.is_empty());
}

#[test]
fn naming_the_owner_fills_every_start() {
    let mut outcome = Outcome {
        compute_starts: vec![
            ComputeStart {
                owner: String::new(),
                ..started("r1")
            },
            ComputeStart {
                owner: String::new(),
                ..started("r2")
            },
        ],
        ..Outcome::default()
    };
    outcome.name_owner("alice");
    assert!(outcome.compute_starts.iter().all(|s| s.owner == "alice"));
}

#[test]
fn a_started_run_of_the_host_becomes_the_nodes_start() {
    let run = host::StartedRun {
        run: "r1".into(),
        method: "digest".into(),
        args: b"args".to_vec(),
        inputs: vec![host::RunInput {
            id: "ab".repeat(32),
            size: 7,
            sha256: vec![0xcd; 32],
        }],
        caps: host::ComputeCaps {
            fuel: 5,
            memory_mib: 64,
            wall_secs: 7,
            result: "onDone".into(),
        },
    };
    let start = ComputeStart::from(run);
    assert_eq!(start.inputs[0].sha256, "cd".repeat(32));
    assert_eq!(start.result_method, "onDone");
    assert_eq!(
        start.caps,
        RunCaps {
            fuel: 5,
            memory_mib: 64,
            wall_secs: 7
        }
    );
    assert_eq!(start.args.0, b"args");
}

#[test]
fn a_run_started_by_init_is_recorded_with_inits_bundle_and_can_be_cancelled() {
    let mut r = Rig::new();
    let mut init: ComponentCmd = serde_json::from_slice(&init(0)).unwrap();
    if let ComponentCmd::Init { outcome, .. } = &mut init {
        outcome.compute_starts = vec![started("boot")];
    }
    let a = r.apply(1, &serde_json::to_vec(&init).unwrap());
    assert!(!a.conflict, "{a:?}");
    let rec = r
        .machine
        .run_record("boot")
        .expect("init's run was dropped");
    assert_eq!((rec.bundle.as_str(), rec.state), ("b", RunState::Queued));
    assert_eq!(r.machine.active_runs(), ["boot"]);
    let mut w = World { r, last: 1 };
    let _a = w.call(Vec::new(), &["boot"]);
    assert!(w.record("boot").cancel_requested);
}

#[test]
fn an_init_that_conflicts_starts_nothing_and_leaves_no_bundle() {
    let mut r = Rig::new();
    let mut init: ComponentCmd = serde_json::from_slice(&init(7)).unwrap();
    if let ComponentCmd::Init { outcome, .. } = &mut init {
        outcome.compute_starts = vec![started("boot")];
    }
    assert!(r.apply(1, &serde_json::to_vec(&init).unwrap()).conflict);
    assert!(r.machine.active_runs().is_empty());
    assert_eq!(r.machine.state.bundle, None);
}

#[test]
fn permanent_refusals_answer_a_reason_and_only_a_stale_base_is_a_conflict() {
    let mut w = World::new();
    w.start("r1");
    let k = job_key(1);
    let a = w.push(&attempt("r1", 2, &k, DIGEST));
    assert!(!a.conflict && a.status == Status::AppError);
    assert_eq!(a.error, "attempt is not the next one");
    let _a = w.assign("r1", &k);
    let stale = w.done("r1", &k);
    let _a = w.call(Vec::new(), &[]);
    let a = w.finish(stale);
    assert!(a.conflict && a.error["detail"] == "conflict: retry");
    let mut bad = w.done("r1", &job_key(9));
    bad.base_index = w.last;
    let a = w.finish(bad);
    assert!(!a.conflict && a.error == "report not signed by the assigned job node");
}

/// A host core on `w`'s state, running a call the way the node runs one.
fn host_on(w: &World, ending: Option<&str>) -> host::HostCore {
    struct NoDir;
    impl host::Directory for NoDir {
        fn org(&self) -> Option<String> {
            None
        }
        fn app_owner(&self) -> String {
            "alice".into()
        }
        fn org_role(&self, _: &str) -> Option<host::OrgRole> {
            None
        }
        fn in_team(&self, _: &str, _: &str) -> bool {
            false
        }
        fn team_exists(&self, _: &str) -> bool {
            false
        }
        fn orgs_of(&self, _: &str) -> Vec<(String, host::OrgRole)> {
            Vec::new()
        }
    }
    let abi = host::Abi::from_json(&json!({
        "abiVersion": 1,
        "methods": {
            "digest": { "kind": "compute",
                "compute": { "fuel": 5, "memoryMib": 64, "wallSecs": 7, "result": "onDone" } },
            "onDone": { "kind": "call", "computeResult": true },
        }
    }))
    .unwrap();
    let inv = host::Invocation {
        kind: host::InvocationKind::Call,
        method: "onDone".into(),
        args: Vec::new(),
        caller: host::Caller::System,
        entry_ts_ms: 5,
        id_term: 2,
        id_seq: 9,
        seed: [1; 32],
        timer: None,
        compute: None,
    };
    host::HostCore::new("app", w.r.machine.view(), w.last, inv, Arc::new(NoDir))
        .unwrap()
        .with_compute(abi, w.r.machine.runs_provider(ending))
}

#[test]
fn a_result_method_starts_a_new_run_of_the_job_that_just_ended_in_a_full_app() {
    let mut w = World::new();
    let all: Vec<_> = (0..host::MAX_ACTIVE_RUNS)
        .map(|i| ComputeStart {
            inputs: Vec::new(),
            args: Bytes(i.to_string().into_bytes()),
            ..started(&format!("r{i:02}"))
        })
        .collect();
    let _a = w.call(all, &[]);
    let args = b"0";
    // Not excluded, the app is full and the same job answers the old run.
    let mut full = host_on(&w, None);
    assert_eq!(
        full.compute_start("digest", args, &[]).unwrap().unwrap(),
        "r00"
    );
    // As the result method of r00 it starts a new run of the same job: its slot is free.
    let mut ending = host_on(&w, Some("r00"));
    let fresh = ending.compute_start("digest", args, &[]).unwrap().unwrap();
    assert_eq!(fresh, "2.9.0");
    let out = ending.finish(host::Status::Ok, Vec::new(), host::Usage::default());
    let k = job_key(1);
    let _a = w.assign("r00", &k);
    let mut entry = w.done("r00", &k);
    entry.result = Outcome::from_host(out);
    assert!(!w.finish(entry).conflict);
    assert!(w.active().contains(&"2.9.0".to_owned()));
    assert_eq!(w.state("r00"), RunState::Done);
}

/// Copies every row of `from`'s group state into a fresh one, as a snapshot install does.
fn install_rows(from: &Rig) -> Rig {
    let mut to = Rig::new();
    let mut rows = Vec::new();
    from.engine
        .group_state(5)
        .scan(&[], |k, v| {
            rows.push((k.to_vec(), v.to_vec()));
            Ok(true)
        })
        .unwrap();
    let state = to.engine.group_state(5);
    let mut tx = StateTx::new(&state);
    for (k, v) in rows {
        tx.put(k, v);
    }
    tx.commit().unwrap();
    calimero_log::Machine::restore(&mut to.machine, &[]).unwrap();
    to
}

#[test]
fn a_snapshot_installed_on_another_machine_carries_the_runs() {
    let mut w = World::new();
    w.start("r1");
    w.start("r2");
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    assert!(!w.finish(w.done("r1", &k)).conflict);
    let _a = w.assign("r2", &k);
    let b = install_rows(&w.r);
    assert_eq!(b.machine.active_runs(), ["r2"]);
    assert_eq!(b.machine.state.component.head.finished_runs, ["r1"]);
    for id in ["r1", "r2"] {
        assert_eq!(b.machine.run_record(id), Some(w.record(id)));
    }
    assert_eq!(
        b.machine.state.component.head,
        w.r.machine.state.component.head
    );
}

#[test]
fn a_replica_installed_partway_through_the_ring_expires_the_same_runs() {
    let total = FINISHED_RUNS_KEPT + 30;
    let mut a = feed_part(0, 150);
    let b_rig = install_rows(&a.r);
    let mut b = World {
        r: b_rig,
        last: a.last,
    };
    for w in [&mut a, &mut b] {
        feed_more(w, 150, total);
    }
    let heads = [&a, &b].map(|w| w.r.machine.state.component.head.clone());
    assert_eq!(heads[0], heads[1]);
    for i in 0..total {
        let id = format!("r{i:03}");
        assert_eq!(
            a.r.machine.run_record(&id),
            b.r.machine.run_record(&id),
            "{id}"
        );
    }
    assert_eq!(a.r.machine.run_record("r029"), None);
    assert!(a.r.machine.run_record("r030").is_some());
}

fn feed_part(from: usize, to: usize) -> World {
    let mut w = World::new();
    feed_more(&mut w, from, to);
    w
}

fn feed_more(w: &mut World, from: usize, to: usize) {
    for batch in (from..to).collect::<Vec<_>>().chunks(40) {
        let starts: Vec<_> = batch.iter().map(|i| started(&format!("r{i:03}"))).collect();
        let _a = w.call(starts, &[]);
        for i in batch {
            finish_quickly(w, &format!("r{i:03}"));
        }
    }
}

/// A job final of `run` whose report (and entry) carry `usage`, signed.
fn with_usage(w: &World, run: &str, k: &SigningKey, usage: ReportUsage) -> FinalEntry {
    let mut entry = w.done(run, k);
    let mut rep = entry.report.take().unwrap();
    rep.usage = usage;
    entry.signature = Some(sign_report(k, &DIGEST, &rep).unwrap());
    entry.report = Some(rep);
    entry.usage = usage;
    entry
}

#[test]
fn the_usage_of_a_run_cannot_pass_what_its_caps_allow() {
    let k = job_key(1);
    // The caps are fuel 5, 64 MiB and 7 s, and a report may come 60 s after the wall cap.
    let wall = 67_000_000u64;
    let over = [
        ("fuel", ReportUsage { fuel: 6, ..usage() }),
        (
            "cpu",
            ReportUsage {
                cpu_micros: wall + 1,
                ..usage()
            },
        ),
        (
            "gib",
            ReportUsage {
                gib_micros: 64 * wall / 1024 + 1,
                ..usage()
            },
        ),
    ];
    for (name, bad) in over {
        let mut w = World::new();
        w.start("r1");
        let _a = w.assign("r1", &k);
        let entry = with_usage(&w, "r1", &k, bad);
        assert_eq!(
            why(&w.finish(entry)),
            "usage is over what the run's caps allow",
            "{name}"
        );
        assert_eq!(w.state("r1"), RunState::Running);
    }
    let edge = ReportUsage {
        fuel: 5,
        cpu_micros: wall,
        gib_micros: 64 * wall / 1024,
    };
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let entry = with_usage(&w, "r1", &k, edge);
    assert!(!w.finish(entry).conflict);
}

#[test]
fn the_outputs_of_a_run_are_bounded_in_count_each_size_and_total() {
    let k = job_key(1);
    let sized = |n: usize, size: u64| -> Vec<FileMark> {
        (0..n)
            .map(|i| FileMark {
                id: hex::encode(calimero_identity::sha256(&i.to_be_bytes())),
                size,
                sha256: Bytes(vec![1; 32]),
                wrapped_key: Bytes(vec![2; 60]),
            })
            .collect()
    };
    let cases = [
        ("count", sized(host::MAX_NEW_FILES + 1, 1)),
        ("size", sized(1, host::MAX_FILE_BYTES + 1)),
        ("total", sized(45, host::MAX_FILE_BYTES)),
    ];
    for (name, outputs) in cases {
        let mut w = World::new();
        w.start("r1");
        let _a = w.assign("r1", &k);
        let mut entry = w.done("r1", &k);
        entry.outputs = outputs;
        assert_eq!(
            why(&w.finish(entry)),
            "outputs are over the limits of a run",
            "{name}"
        );
        assert!(w.files().is_empty());
    }
    // At the limits it applies.
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &k);
    let outputs = sized(1, host::MAX_FILE_BYTES);
    let mut entry = w.done("r1", &k);
    let rep = report("r1", 1, ReportStatus::Done, None, &outputs);
    entry.signature = Some(sign_report(&k, &DIGEST, &rep).unwrap());
    entry.report = Some(rep);
    entry.outputs = outputs;
    assert!(!w.finish(entry).conflict);
}

/// `w` with run `r1` (input `ab..`) started, and that file marked.
fn world_with_input() -> World {
    let mut w = World::new();
    let mut cmd: ComponentCmd =
        serde_json::from_slice(&super::call(w.last, None, Vec::new())).unwrap();
    if let ComponentCmd::Call(c) = &mut cmd {
        c.outcome.files = vec![FileMark {
            id: "ab".repeat(32),
            ..mark("x")
        }];
        c.outcome.compute_starts = vec![started("r1")];
    }
    assert!(!w.push(&cmd).conflict);
    w
}

fn forget(w: &mut World) -> Applied {
    w.push(&ComponentCmd::FileForget {
        id: "ab".repeat(32),
        key: None,
    })
}

#[test]
fn an_admin_delete_of_an_input_of_an_active_run_is_refused_until_the_run_is_final() {
    let mut w = world_with_input();
    assert_eq!(why(&forget(&mut w)), host::FILE_IS_RUN_INPUT);
    assert_eq!(w.files(), ["ab".repeat(32)]);
    let k = job_key(1);
    let _a = w.assign("r1", &k);
    assert!(!w.finish(w.done("r1", &k)).conflict);
    let gone = forget(&mut w);
    assert_eq!(gone.status, Status::Ok, "{gone:?}");
    assert!(!w.files().contains(&"ab".repeat(32)));
}

#[test]
fn an_unreadable_run_record_keeps_its_inputs_and_makes_the_host_fail_closed() {
    let mut w = world_with_input();
    let state = w.r.engine.group_state(5);
    let mut tx = StateTx::new(&state);
    tx.put_hidden(rows::CRUN, b"r1", b"not json");
    tx.commit().unwrap();
    assert_eq!(why(&forget(&mut w)), host::FILE_IS_RUN_INPUT);
    assert!(w.r.machine.runs_provider(None).active().is_err());
}

#[test]
fn the_host_is_told_which_runs_are_being_cancelled() {
    let mut w = World::new();
    w.start("r1");
    let _a = w.call(Vec::new(), &["r1"]);
    let seen = w.r.machine.runs_provider(None).active().unwrap();
    assert!(seen[0].cancel_requested);
}

#[test]
fn a_run_whose_acquire_was_refused_for_good_can_end_refused_only_before_any_attempt() {
    let refused = FinalOutcome::Failed {
        reason: FailedReason::Refused,
    };
    let mut w = World::new();
    w.start("r1");
    let mut entry = leader(&w, "r1", refused);
    entry.attempt = 0;
    let mut charged = entry.clone();
    charged.usage = usage();
    assert_eq!(why(&w.finish(charged)), "a leader outcome is not charged");
    let mut with_outputs = FinalEntry {
        base_index: w.last,
        ..entry.clone()
    };
    with_outputs.outputs = vec![out(1)];
    assert_eq!(
        why(&w.finish(with_outputs)),
        "a leader outcome has no outputs"
    );
    entry.base_index = w.last;
    assert!(!w.finish(entry).conflict);
    let rec = w.record("r1");
    assert_eq!(rec.state, RunState::Failed);
    assert_eq!(rec.final_.unwrap().outcome, refused);
    assert!(w.active().is_empty());

    let mut w = World::new();
    w.start("r2");
    let _a = w.assign("r2", &job_key(1));
    let late = leader(&w, "r2", refused);
    assert_eq!(why(&w.finish(late)), "the run was assigned");
}

#[test]
fn a_run_whose_outputs_pass_the_quota_ends_failed_quota_as_a_leader_outcome() {
    let quota = FinalOutcome::Failed {
        reason: FailedReason::Quota,
    };
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &job_key(1));
    let mut with_report = w.done("r1", &job_key(1));
    with_report.outcome = quota;
    assert_eq!(
        why(&w.finish(with_report)),
        "a leader outcome carries no report"
    );
    let mut entry = leader(&w, "r1", quota);
    entry.base_index = w.last;
    assert!(!w.finish(entry).conflict);
    let rec = w.record("r1");
    assert_eq!(rec.state, RunState::Failed);
    assert_eq!(rec.final_.unwrap().outcome, quota);
    assert!(w.active().is_empty());
}

#[test]
fn a_signed_result_over_the_limit_is_refused() {
    let mut w = World::new();
    w.start("r1");
    let _a = w.assign("r1", &job_key(1));
    let mut entry = w.done("r1", &job_key(1));
    let report = entry.report.as_mut().unwrap();
    report.result = vec![0; calimero_app_host::MAX_COMPUTE_RESULT_BYTES + 1];
    entry.signature = Some(sign_report(&job_key(1), &DIGEST, report).unwrap());
    assert_eq!(
        why(&w.finish(entry)),
        "the result is over the limit of a run"
    );
    assert!(w.state("r1").is_active());
}
