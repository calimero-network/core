//! How a replica applies the pipeline entries: every rule is judged on the record, the entry and
//! the app's blobs alone.

use ed25519_dalek::SigningKey;

use super::runs::{job_key, leader, out, report, why, World, DIGEST, INPUT};
use super::*;
use crate::component::pipelines::{
    BAD_START, CANCELLING, DECIDED, INPUT_NOT_HELD, KNOWN, NOT_RUNNING,
};
use crate::job_wire::{sign_report, FailReason, ReportStatus};

const PIPE: &str = "p1";

fn refs(w: &World, n: u8) -> u64 {
    w.r.machine.state.component.blobs.records[&FileMark::fixture_id(n)].refs
}

fn start_entry(id: &str, steps: &[&str]) -> PipelineStartEntry {
    PipelineStartEntry {
        id: id.into(),
        pipeline: "demo".into(),
        version: "1.0.0".into(),
        hash: "ab".repeat(32),
        compute_bundle: "compute-v1".into(),
        owner: "alice".into(),
        steps: steps.iter().map(|s| (*s).to_owned()).collect(),
        inputs: BTreeMap::from([("in".to_owned(), vec![fid(INPUT)])]),
        params: BTreeMap::new(),
        caller: Caller::Account {
            account: "a".into(),
        },
        payer: "account:alice".into(),
        ts_ms: 5,
    }
}

fn begin(w: &mut World, id: &str, steps: &[&str]) -> Applied {
    w.push(&ComponentCmd::PipelineStart(Box::new(start_entry(
        id, steps,
    ))))
}

fn step_run(pipeline: &str, step: &str, i: usize) -> StepRun {
    StepRun {
        run: format!("{pipeline}.{step}.{i}"),
        method: "digest".into(),
        args: Bytes(b"{}".to_vec()),
        inputs: vec![fid(INPUT)],
        caps: RunCaps {
            fuel: 5,
            memory_mib: 64,
            wall_secs: 7,
        },
    }
}

fn decide(w: &mut World, run: &str, step: &str, decision: StepVerdict) -> Applied {
    w.push(&ComponentCmd::PipelineStep(Box::new(StepDecisionEntry {
        run: run.into(),
        step: step.into(),
        decision,
        ts_ms: 6,
    })))
}

fn start_step(w: &mut World, pipeline: &str, step: &str) -> Applied {
    let runs = vec![step_run(pipeline, step, 0)];
    decide(w, pipeline, step, StepVerdict::Start { runs })
}

fn pipeline(w: &World, id: &str) -> PipelineRun {
    w.r.machine.pipeline_run(id).expect("no pipeline run")
}

fn status(w: &World, id: &str, step: &str) -> StepStatus {
    pipeline(w, id).steps[step].status
}

/// A done final of `run` whose single output is file `n`.
fn done_with(w: &World, run: &str, signer: &SigningKey, n: u8) -> FinalEntry {
    let mut entry = w.done(run, signer);
    let outputs = vec![out(n)];
    let rep = report(run, entry.attempt, ReportStatus::Done, None, &outputs);
    entry.signature = Some(sign_report(signer, &DIGEST, &rep).unwrap());
    entry.report = Some(rep);
    entry.outputs = outputs;
    entry.result = Outcome::default();
    entry
}

/// A final of `run` that failed with a trap.
fn trapped(w: &World, run: &str, signer: &SigningKey) -> FinalEntry {
    let mut entry = w.done(run, signer);
    let rep = report(
        run,
        entry.attempt,
        ReportStatus::Failed,
        Some(FailReason::Trap),
        &[],
    );
    entry.signature = Some(sign_report(signer, &DIGEST, &rep).unwrap());
    entry.report = Some(rep);
    entry.outputs = Vec::new();
    entry.outcome = FinalOutcome::Failed {
        reason: FailedReason::Trap,
    };
    entry.result = Outcome::default();
    entry
}

fn run_done(w: &mut World, run: &str, n: u8) -> Applied {
    let k = job_key(1);
    let _a = w.assign(run, &k);
    let entry = done_with(w, run, &k, n);
    w.finish(entry)
}

#[test]
fn a_start_pins_the_inputs_and_records_every_step_pending() {
    let mut w = World::new();
    let before = refs(&w, INPUT);
    let a = begin(&mut w, PIPE, &["a", "b"]);
    assert!(!a.conflict && a.status == Status::Ok, "{a:?}");
    let p = pipeline(&w, PIPE);
    assert_eq!(p.state, PipelineState::Running);
    assert!(p.steps.values().all(|s| s.status == StepStatus::Pending));
    assert_eq!(w.r.machine.active_pipelines(), vec![PIPE.to_owned()]);
    assert_eq!(refs(&w, INPUT), before + 1);
}

#[test]
fn a_duplicate_start_a_missing_input_and_a_full_app_change_nothing() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let held = refs(&w, INPUT);
    assert_eq!(why(&begin(&mut w, PIPE, &["a", "b"])), KNOWN);
    assert_eq!(pipeline(&w, PIPE).steps.len(), 1);
    assert_eq!(refs(&w, INPUT), held);

    let mut missing = start_entry("p2", &["a"]);
    missing.inputs = BTreeMap::from([("in".to_owned(), vec![fid(0x77)])]);
    let a = w.push(&ComponentCmd::PipelineStart(Box::new(missing)));
    assert_eq!(why(&a), INPUT_NOT_HELD);
    assert!(w.r.machine.pipeline_run("p2").is_none());

    for i in 1..MAX_ACTIVE_PIPELINE_RUNS {
        assert!(begin(&mut w, &format!("q{i}"), &["a"]).status == Status::Ok);
    }
    assert_eq!(why(&begin(&mut w, "late", &["a"])), TOO_MANY_PIPELINE_RUNS);
    assert!(w.r.machine.pipeline_run("late").is_none());
}

#[test]
fn a_start_that_is_not_well_formed_is_refused() {
    let mut w = World::new();
    assert_eq!(why(&begin(&mut w, "a.b", &["a"])), BAD_START);
    assert_eq!(why(&begin(&mut w, PIPE, &[])), BAD_START);
    assert_eq!(why(&begin(&mut w, PIPE, &["x.y"])), BAD_START);
    assert!(w.r.machine.active_pipelines().is_empty());
}

#[test]
fn a_started_step_makes_a_linked_queued_run_with_the_pinned_bundle() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let a = start_step(&mut w, PIPE, "a");
    assert!(a.status == Status::Ok, "{a:?}");
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Running);
    assert_eq!(step.runs, vec!["p1.a.0".to_owned()]);
    let run = w.record("p1.a.0");
    assert_eq!(run.state, RunState::Queued);
    assert_eq!(run.compute_bundle.as_deref(), Some("compute-v1"));
    assert_eq!(run.result_method, "");
    assert_eq!(
        run.pipeline,
        Some(PipelineLink {
            run: PIPE.into(),
            step: "a".into()
        })
    );
    assert!(w.active().contains(&"p1.a.0".to_owned()));
}

#[test]
fn the_first_decision_for_a_step_wins() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let _s = start_step(&mut w, PIPE, "a");
    let runs = w.active().len();
    let again = start_step(&mut w, PIPE, "a");
    assert_eq!(why(&again), DECIDED);
    let other = decide(&mut w, PIPE, "a", StepVerdict::Cancelled);
    assert_eq!(why(&other), DECIDED);
    assert_eq!(w.active().len(), runs);
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Running);
    assert_eq!(pipeline(&w, PIPE).steps["a"].runs.len(), 1);
}

#[test]
fn a_start_past_the_active_runs_is_refused_and_the_step_stays_pending() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    for i in 0..host::MAX_ACTIVE_RUNS {
        w.start(&format!("filler{i}"));
    }
    assert_eq!(w.active().len(), host::MAX_ACTIVE_RUNS);
    assert_eq!(why(&start_step(&mut w, PIPE, "a")), NO_ROOM);
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Pending);
    assert!(w.r.machine.run_record("p1.a.0").is_none());
}

#[test]
fn a_start_that_can_never_work_fails_the_step() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b", "c"]);
    let mut gone = step_run(PIPE, "a", 0);
    gone.inputs = vec![fid(0x77)];
    let _d = decide(&mut w, PIPE, "a", StepVerdict::Start { runs: vec![gone] });
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Failed);
    assert_eq!(
        pipeline(&w, PIPE).steps["a"].reason.as_deref(),
        Some("INPUT_NOT_HELD")
    );
    let mut wrong_id = step_run(PIPE, "b", 0);
    wrong_id.run = "elsewhere".into();
    let _d = decide(
        &mut w,
        PIPE,
        "b",
        StepVerdict::Start {
            runs: vec![wrong_id],
        },
    );
    assert_eq!(
        pipeline(&w, PIPE).steps["b"].reason.as_deref(),
        Some("BAD_START")
    );
    assert!(w.r.machine.run_record("elsewhere").is_none());
    let _d = decide(
        &mut w,
        PIPE,
        "c",
        StepVerdict::Failed {
            reason: "TOO_MANY_INPUTS".into(),
        },
    );
    assert_eq!(
        pipeline(&w, PIPE).steps["c"].reason.as_deref(),
        Some("TOO_MANY_INPUTS")
    );
}

#[test]
fn a_final_of_a_linked_run_settles_its_step_and_pins_its_outputs() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = start_step(&mut w, PIPE, "a");
    let a = run_done(&mut w, "p1.a.0", 1);
    assert!(a.status == Status::Ok && !a.conflict, "{a:?}");
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Succeeded);
    assert_eq!(step.outputs.len(), 1);
    assert_eq!(step.outputs[0].id, out(1).id);
    assert_eq!(step.result, Some(Bytes(b"\"ok\"".to_vec())));
    assert_eq!(refs(&w, 1), 1);
    assert_eq!(pipeline(&w, PIPE).state, PipelineState::Running);
}

#[test]
fn every_step_settled_ends_the_run_succeeded_and_releases_its_inputs() {
    let mut w = World::new();
    let base = refs(&w, INPUT);
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = start_step(&mut w, PIPE, "a");
    let _r = run_done(&mut w, "p1.a.0", 1);
    let _s = decide(&mut w, PIPE, "b", StepVerdict::Skipped);
    let p = pipeline(&w, PIPE);
    assert_eq!(p.state, PipelineState::Succeeded);
    assert_eq!(p.end_ts_ms, Some(6));
    assert!(w.r.machine.active_pipelines().is_empty());
    assert_eq!(refs(&w, INPUT), base);
    assert_eq!(
        refs(&w, 1),
        1,
        "outputs stay pinned while the record is kept"
    );
}

#[test]
fn a_failed_step_lets_its_dependents_be_upstream_failed_and_fails_the_run() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = start_step(&mut w, PIPE, "a");
    let k = job_key(1);
    let _t = w.assign("p1.a.0", &k);
    let end = trapped(&w, "p1.a.0", &k);
    let _f = w.finish(end);
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Failed);
    assert_eq!(step.reason.as_deref(), Some("trap"));
    assert_eq!(pipeline(&w, PIPE).state, PipelineState::Running);
    let a = decide(&mut w, PIPE, "b", StepVerdict::UpstreamFailed);
    assert!(a.status == Status::Ok, "{a:?}");
    let p = pipeline(&w, PIPE);
    assert_eq!(p.steps["b"].status, StepStatus::UpstreamFailed);
    assert_eq!(p.state, PipelineState::Failed);
}

#[test]
fn a_cancel_flags_the_running_runs_and_refuses_new_starts() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = start_step(&mut w, PIPE, "a");
    let a = w.push(&ComponentCmd::PipelineCancel(Box::new(
        PipelineCancelEntry {
            run: PIPE.into(),
            ts_ms: 7,
        },
    )));
    assert!(a.status == Status::Ok, "{a:?}");
    assert!(pipeline(&w, PIPE).cancel_requested);
    assert!(w.record("p1.a.0").cancel_requested);
    assert_eq!(why(&start_step(&mut w, PIPE, "b")), CANCELLING);
    let _c = decide(&mut w, PIPE, "b", StepVerdict::Cancelled);
    let end = leader(&w, "p1.a.0", FinalOutcome::Cancelled);
    let _f = w.finish(end);
    let p = pipeline(&w, PIPE);
    assert_eq!(p.steps["a"].status, StepStatus::Failed);
    assert_eq!(p.steps["a"].reason.as_deref(), Some("cancelled"));
    assert_eq!(p.steps["b"].status, StepStatus::Cancelled);
    assert_eq!(p.state, PipelineState::Cancelled);
}

#[test]
fn a_cancel_of_an_unknown_or_ended_run_changes_nothing() {
    let mut w = World::new();
    let cancel = |w: &mut World, run: &str| {
        w.push(&ComponentCmd::PipelineCancel(Box::new(
            PipelineCancelEntry {
                run: run.into(),
                ts_ms: 7,
            },
        )))
    };
    assert_eq!(why(&cancel(&mut w, "nope")), NOT_RUNNING);
    let _a = begin(&mut w, PIPE, &["a"]);
    let _s = decide(&mut w, PIPE, "a", StepVerdict::Skipped);
    assert_eq!(why(&cancel(&mut w, PIPE)), NOT_RUNNING);
    assert!(!pipeline(&w, PIPE).cancel_requested);
}

#[test]
fn a_dropped_pipeline_run_releases_the_holds_on_its_outputs() {
    let mut w = World::new();
    let _a = begin(&mut w, "first", &["a"]);
    let _s = start_step(&mut w, "first", "a");
    let _r = run_done(&mut w, "first.a.0", 1);
    assert_eq!(pipeline(&w, "first").state, PipelineState::Succeeded);
    assert_eq!(refs(&w, 1), 1);
    for i in 0..FINISHED_PIPELINE_RUNS_KEPT {
        let id = format!("n{i}");
        let _b = begin(&mut w, &id, &["a"]);
        let _d = decide(&mut w, &id, "a", StepVerdict::Skipped);
    }
    assert!(w.r.machine.pipeline_run("first").is_none());
    assert_eq!(refs(&w, 1), 0);
    assert!(w.r.machine.pipeline_run("n0").is_some());
}

#[test]
fn the_entries_have_a_fixed_wire_shape() {
    let start = ComponentCmd::PipelineStart(Box::new(start_entry(PIPE, &["a"])));
    let v: Value = serde_json::to_value(&start).unwrap();
    assert_eq!(v["op"], "pipelineStart");
    assert_eq!(v["computeBundle"], "compute-v1");
    let step = ComponentCmd::PipelineStep(Box::new(StepDecisionEntry {
        run: PIPE.into(),
        step: "a".into(),
        decision: StepVerdict::Failed { reason: "X".into() },
        ts_ms: 1,
    }));
    let v: Value = serde_json::to_value(&step).unwrap();
    assert_eq!(v["op"], "pipelineStep");
    assert_eq!(
        v["decision"],
        serde_json::json!({ "kind": "failed", "reason": "X" })
    );
    let cancel = ComponentCmd::PipelineCancel(Box::new(PipelineCancelEntry {
        run: PIPE.into(),
        ts_ms: 1,
    }));
    assert_eq!(
        serde_json::to_value(&cancel).unwrap()["op"],
        "pipelineCancel"
    );
}

#[test]
fn a_head_without_pipelines_reads_and_writes_as_before() {
    let head = ComponentHead::default();
    let v: Value = serde_json::to_value(&head).unwrap();
    assert!(v.get("activePipelines").is_none() && v.get("finishedPipelines").is_none());
    let back: ComponentHead = serde_json::from_value(v).unwrap();
    assert!(back.active_pipelines.is_empty());
}
