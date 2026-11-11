//! How a replica applies the pipeline entries: every rule is judged on the record, the entry and
//! the app's blobs alone.

use ed25519_dalek::SigningKey;

use super::runs::{job_key, leader, out, report, why, World, DIGEST, INPUT};
use super::*;
use crate::app::rows;
use crate::component::pipelines::{
    BAD_START, CACHE_GONE, CANCELLING, COST_CAP, DECIDED, INPUT_NOT_HELD, KNOWN, NOT_FAN_OUT,
    NOT_NEXT, NOT_RUNNING, STEP_STOPPING, TOO_MANY_FAILURES,
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
        modules: BTreeMap::new(),
        owner: "alice".into(),
        steps: steps.iter().map(|s| (*s).to_owned()).collect(),
        inputs: BTreeMap::from([("in".to_owned(), vec![fid(INPUT)])]),
        params: BTreeMap::new(),
        caller: Caller::Account {
            account: "a".into(),
        },
        payer: "account:alice".into(),
        fail_fast: false,
        cost: None,
        retry_of: None,
        limits: BTreeMap::new(),
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
        compute_bundle: None,
        cache_key: None,
        pool: None,
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
    decide(
        w,
        pipeline,
        step,
        StepVerdict::Start {
            runs,
            fan: FanOut::SINGLE,
            cached: Vec::new(),
        },
    )
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
            step: "a".into(),
            index: 0,
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
    let _d = decide(
        &mut w,
        PIPE,
        "a",
        StepVerdict::Start {
            runs: vec![gone],
            fan: FanOut::SINGLE,
            cached: Vec::new(),
        },
    );
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
            fan: FanOut::SINGLE,
            cached: Vec::new(),
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
    assert_eq!(step.outputs().count(), 1);
    assert_eq!(step.outputs().next().unwrap().id, out(1).id);
    assert_eq!(step.result(), Some(&Bytes(b"\"ok\"".to_vec())));
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

/// The leader re-proposes a start or cancel whose answer was lost: the repeat answers as the
/// first did and changes nothing, so the caller never hears "not started" for a started run.
#[test]
fn a_repeated_start_or_cancel_of_the_same_run_answers_ok_and_changes_nothing() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let held = refs(&w, INPUT);
    let again = begin(&mut w, PIPE, &["a", "b"]);
    assert!(again.status == Status::Ok, "{again:?}");
    assert_eq!(refs(&w, INPUT), held);
    assert_eq!(w.r.machine.active_pipelines(), vec![PIPE.to_owned()]);

    let cancel = |w: &mut World| {
        w.push(&ComponentCmd::PipelineCancel(Box::new(
            PipelineCancelEntry {
                run: PIPE.into(),
                ts_ms: 7,
            },
        )))
    };
    assert!(cancel(&mut w).status == Status::Ok);
    let repeat = cancel(&mut w);
    assert!(repeat.status == Status::Ok, "{repeat:?}");
    assert!(pipeline(&w, PIPE).cancel_requested);
    let _a = decide(&mut w, PIPE, "a", StepVerdict::Cancelled);
    let _b = decide(&mut w, PIPE, "b", StepVerdict::Cancelled);
    let ended = pipeline(&w, PIPE);
    assert_eq!(ended.state, PipelineState::Cancelled);
    // The run ended: a repeat of its cancel or its start still answers as the first did.
    assert!(cancel(&mut w).status == Status::Ok);
    assert!(begin(&mut w, PIPE, &["a", "b"]).status == Status::Ok);
    assert_eq!(pipeline(&w, PIPE).end_ts_ms, ended.end_ts_ms);
    assert!(w.r.machine.active_pipelines().is_empty());
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

/// Runs `from..to` of `step`, one item each.
fn runs_of(pipeline: &str, step: &str, from: usize, to: usize) -> Vec<StepRun> {
    (from..to).map(|i| step_run(pipeline, step, i)).collect()
}

/// Starts `step` as a fan-out of `total` runs of one item, `allowed` of which may fail, with the
/// first `first` runs.
fn fan_start(w: &mut World, step: &str, total: u32, allowed: u32, first: usize) -> Applied {
    let fan = FanOut {
        total,
        items: total,
        batch: 1,
        allowed,
    };
    let runs = runs_of(PIPE, step, 0, first);
    decide(
        w,
        PIPE,
        step,
        StepVerdict::Start {
            runs,
            fan,
            cached: Vec::new(),
        },
    )
}

fn more(w: &mut World, step: &str, from: usize, to: usize) -> Applied {
    let runs = runs_of(PIPE, step, from, to);
    decide(
        w,
        PIPE,
        step,
        StepVerdict::More {
            runs,
            cached: Vec::new(),
        },
    )
}

/// A done final of `run` with output `n` and `result`.
fn run_done_with_result(w: &mut World, run: &str, n: u8, result: &[u8]) -> Applied {
    let k = job_key(1);
    let _a = w.assign(run, &k);
    let mut entry = done_with(w, run, &k, n);
    if let Some(rep) = entry.report.as_mut() {
        rep.result = result.to_vec();
        entry.signature = Some(sign_report(&k, &DIGEST, rep).unwrap());
    }
    w.finish(entry)
}

fn run_failed(w: &mut World, run: &str) -> Applied {
    let k = job_key(1);
    let _a = w.assign(run, &k);
    let end = trapped(w, run, &k);
    w.finish(end)
}

#[test]
fn more_runs_continue_a_running_fan_out_in_index_order() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    assert_eq!(why(&more(&mut w, "a", 0, 1)), NOT_FAN_OUT, "pending");
    let a = fan_start(&mut w, "a", 5, 0, 2);
    assert!(a.status == Status::Ok, "{a:?}");
    assert_eq!(pipeline(&w, PIPE).steps["a"].fan.total, 5);
    assert_eq!(why(&more(&mut w, "a", 3, 4)), NOT_NEXT, "a gap");
    assert_eq!(why(&more(&mut w, "a", 0, 2)), NOT_NEXT, "already created");
    assert_eq!(why(&more(&mut w, "a", 2, 6)), NOT_NEXT, "past the total");
    let a = more(&mut w, "a", 2, 4);
    assert!(a.status == Status::Ok, "{a:?}");
    let runs = &pipeline(&w, PIPE).steps["a"].runs;
    assert_eq!(runs.len(), 4);
    assert_eq!(runs[3], "p1.a.3");
    assert_eq!(why(&more(&mut w, "a", 2, 4)), NOT_NEXT, "a re-proposal");
    assert_eq!(w.record("p1.a.3").pipeline.unwrap().index, 3);
    let a = more(&mut w, "a", 4, 5);
    assert!(a.status == Status::Ok, "{a:?}");
    assert_eq!(why(&more(&mut w, "a", 5, 6)), NOT_NEXT, "no index left");
    assert_eq!(why(&more(&mut w, "b", 0, 1)), NOT_FAN_OUT, "b is pending");
}

#[test]
fn more_runs_are_refused_for_a_settled_step_and_a_full_app() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = fan_start(&mut w, "a", 3, 0, 1);
    for i in 0..host::MAX_ACTIVE_RUNS - 1 {
        w.start(&format!("filler{i}"));
    }
    assert_eq!(why(&more(&mut w, "a", 1, 2)), crate::component::NO_ROOM);
    assert_eq!(pipeline(&w, PIPE).steps["a"].runs.len(), 1);
    let _f = run_failed(&mut w, "p1.a.0");
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Failed);
    assert_eq!(why(&more(&mut w, "a", 1, 2)), NOT_FAN_OUT, "settled");
}

#[test]
fn outputs_and_results_are_in_item_order_whatever_order_runs_finish() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let _s = fan_start(&mut w, "a", 3, 0, 3);
    let _r = run_done_with_result(&mut w, "p1.a.2", 3, b"2");
    let _r = run_done_with_result(&mut w, "p1.a.0", 1, b"0");
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Running);
    let _r = run_done_with_result(&mut w, "p1.a.1", 2, b"1");
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Succeeded);
    let ids: Vec<&str> = step.outputs().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, [out(1).id, out(2).id, out(3).id]);
    let results: Vec<&[u8]> = step.run_results.values().map(|b| &b.0[..]).collect();
    assert_eq!(results, [&b"0"[..], b"1", b"2"]);
}

#[test]
fn failures_at_the_allowance_succeed_and_one_over_fails_and_stops_the_step() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = fan_start(&mut w, "a", 4, 1, 4);
    let _r = run_failed(&mut w, "p1.a.1");
    for (i, n) in [(0, 1), (2, 2), (3, 3)] {
        let _r = run_done_with_result(&mut w, &format!("p1.a.{i}"), n, b"1");
    }
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Succeeded, "exactly at the limit");
    assert_eq!(step.failed_items, 1);
    assert_eq!(step.outputs().count(), 3, "only the successful outputs");
    let _s = decide(&mut w, PIPE, "b", StepVerdict::Skipped);
    assert_eq!(
        pipeline(&w, PIPE).state,
        PipelineState::SucceededWithFailures
    );

    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let _s = fan_start(&mut w, "a", 4, 1, 3);
    let _r = run_failed(&mut w, "p1.a.0");
    assert!(!w.record("p1.a.2").cancel_requested, "within the allowance");
    let _r = run_failed(&mut w, "p1.a.1");
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Running, "a run is still going");
    assert!(w.record("p1.a.2").cancel_requested, "one over cancels it");
    assert_eq!(why(&more(&mut w, "a", 3, 4)), STEP_STOPPING);
    let end = leader(&w, "p1.a.2", FinalOutcome::Cancelled);
    let _f = w.finish(end);
    let p = pipeline(&w, PIPE);
    assert_eq!(p.steps["a"].status, StepStatus::Failed);
    assert_eq!(p.steps["a"].reason.as_deref(), Some(TOO_MANY_FAILURES));
    assert_eq!(p.steps["a"].failed_items, 3);
    assert_eq!(p.steps["a"].runs.len(), 3, "no fourth run was created");
    assert_eq!(p.state, PipelineState::Failed);
}

#[test]
fn a_failed_batch_run_fails_all_its_items() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let fan = FanOut {
        total: 3,
        items: 10,
        batch: 4,
        allowed: 2,
    };
    let runs = runs_of(PIPE, "a", 0, 3);
    let _s = decide(
        &mut w,
        PIPE,
        "a",
        StepVerdict::Start {
            runs,
            fan,
            cached: Vec::new(),
        },
    );
    let _r = run_failed(&mut w, "p1.a.2");
    assert_eq!(pipeline(&w, PIPE).steps["a"].failed_items, 2, "4,4,2 items");
    let _r = run_done_with_result(&mut w, "p1.a.0", 1, b"1");
    let _r = run_done_with_result(&mut w, "p1.a.1", 2, b"1");
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Succeeded);
    assert_eq!(
        pipeline(&w, PIPE).state,
        PipelineState::SucceededWithFailures
    );
}

#[test]
fn without_an_allowance_one_failed_item_fails_the_step_with_its_reason() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let _s = fan_start(&mut w, "a", 3, 0, 3);
    let _r = run_failed(&mut w, "p1.a.1");
    assert!(w.record("p1.a.0").cancel_requested);
    assert!(w.record("p1.a.2").cancel_requested);
    for i in [0, 2] {
        let run = format!("p1.a.{i}");
        let end = leader(&w, &run, FinalOutcome::Cancelled);
        let _f = w.finish(end);
    }
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Failed);
    assert_eq!(step.reason.as_deref(), Some("trap"), "the first failure");
}

#[test]
fn fail_fast_cancels_the_rest_of_the_run_and_ends_it_failed() {
    let mut w = World::new();
    let mut entry = start_entry(PIPE, &["a", "b", "c"]);
    entry.fail_fast = true;
    let _a = w.push(&ComponentCmd::PipelineStart(Box::new(entry)));
    let _s = start_step(&mut w, PIPE, "a");
    let _s = start_step(&mut w, PIPE, "b");
    let _r = run_failed(&mut w, "p1.a.0");
    let p = pipeline(&w, PIPE);
    assert!(p.stopping && !p.cancel_requested);
    assert_eq!(p.steps["c"].status, StepStatus::Cancelled);
    assert!(w.record("p1.b.0").cancel_requested);
    assert_eq!(p.state, PipelineState::Running, "b has a run going");
    let end = leader(&w, "p1.b.0", FinalOutcome::Cancelled);
    let _f = w.finish(end);
    let p = pipeline(&w, PIPE);
    assert_eq!(p.state, PipelineState::Failed);
    assert_eq!(p.steps["b"].status, StepStatus::Failed);
    assert!(w.active().is_empty());
}

#[test]
fn without_fail_fast_only_dependents_are_upstream_failed() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = start_step(&mut w, PIPE, "a");
    let _s = start_step(&mut w, PIPE, "b");
    let _r = run_failed(&mut w, "p1.a.0");
    assert!(!pipeline(&w, PIPE).stopping);
    assert!(!w.record("p1.b.0").cancel_requested);
}

#[test]
fn a_cancel_between_waves_settles_the_step_that_has_no_run_left() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let _s = fan_start(&mut w, "a", 3, 0, 1);
    let _r = run_done_with_result(&mut w, "p1.a.0", 1, b"1");
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Running, "two runs to go");
    let _c = w.push(&ComponentCmd::PipelineCancel(Box::new(
        PipelineCancelEntry {
            run: PIPE.into(),
            ts_ms: 7,
        },
    )));
    let p = pipeline(&w, PIPE);
    assert_eq!(p.steps["a"].status, StepStatus::Failed);
    assert_eq!(p.steps["a"].reason.as_deref(), Some("cancelled"));
    assert_eq!(p.state, PipelineState::Cancelled);
}

#[test]
fn results_past_the_step_bound_are_dropped_and_flagged() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let n = crate::component::MAX_STEP_RESULTS_BYTES / crate::component::MAX_STEP_RESULT_BYTES + 2;
    let _s = fan_start(&mut w, "a", u32::try_from(n).unwrap(), 0, n);
    let big = vec![b'x'; crate::component::MAX_STEP_RESULT_BYTES];
    for i in 0..n {
        let _r = run_done_with_result(
            &mut w,
            &format!("p1.a.{i}"),
            u8::try_from(i + 1).unwrap(),
            &big,
        );
    }
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(
        step.status,
        StepStatus::Succeeded,
        "the step still succeeds"
    );
    assert!(step.results_too_large);
    assert!(step.run_results.is_empty());
    assert_eq!(step.outputs().count(), n);
}

#[test]
fn a_running_fan_out_between_waves_can_be_failed_but_a_started_one_cannot() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = fan_start(&mut w, "a", 3, 0, 1);
    let fail = || StepVerdict::Failed {
        reason: "PIPELINE_UNAVAILABLE".into(),
    };
    let _s = start_step(&mut w, PIPE, "b");
    assert_eq!(
        why(&decide(&mut w, PIPE, "b", fail())),
        DECIDED,
        "b has all its runs"
    );
    let a = decide(&mut w, PIPE, "a", fail());
    assert!(a.status == Status::Ok, "{a:?}");
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Failed);
    assert!(w.record("p1.a.0").cancel_requested);
}

fn keyed(pipeline: &str, step: &str, i: usize, key: &str) -> StepRun {
    StepRun {
        cache_key: Some(key.into()),
        ..step_run(pipeline, step, i)
    }
}

fn hit(index: u32, key: &str) -> CachedRun {
    CachedRun {
        index,
        key: key.into(),
    }
}

fn decide_at(w: &mut World, run: &str, step: &str, decision: StepVerdict, ts_ms: u64) -> Applied {
    w.push(&ComponentCmd::PipelineStep(Box::new(StepDecisionEntry {
        run: run.into(),
        step: step.into(),
        decision,
        ts_ms,
    })))
}

fn entry(w: &World, key: &str) -> Option<CacheEntry> {
    w.r.machine.cache_entry(key)
}

fn order(w: &World) -> Vec<String> {
    w.r.machine
        .state
        .component
        .head
        .step_cache
        .iter()
        .cloned()
        .collect()
}

/// Pipeline `id` runs one keyed step to done at `ts_ms`, leaving a cache entry for output `n`.
fn make_entry(w: &mut World, id: &str, key: &str, n: u8, ts_ms: u64) {
    let _b = begin(w, id, &["a"]);
    let runs = vec![keyed(id, "a", 0, key)];
    let _s = decide(
        w,
        id,
        "a",
        StepVerdict::Start {
            runs,
            fan: FanOut::SINGLE,
            cached: Vec::new(),
        },
    );
    let run = format!("{id}.a.0");
    let k = job_key(1);
    let _a = w.assign(&run, &k);
    let mut done = done_with(w, &run, &k, n);
    done.ts_ms = ts_ms;
    let a = w.finish(done);
    assert!(a.status == Status::Ok && !a.conflict, "{a:?}");
}

fn start_cached(w: &mut World, id: &str, cached: Vec<CachedRun>, ts_ms: u64) -> Applied {
    let _b = begin(w, id, &["a"]);
    let start = StepVerdict::Start {
        runs: Vec::new(),
        fan: FanOut::SINGLE,
        cached,
    };
    decide_at(w, id, "a", start, ts_ms)
}

#[test]
fn a_done_run_with_a_key_writes_an_entry_and_pins_its_outputs() {
    let mut w = World::new();
    make_entry(&mut w, PIPE, "k1", 1, 9);
    let e = entry(&w, "k1").expect("no entry");
    assert_eq!(e.outputs.len(), 1);
    assert_eq!(e.outputs[0].id, out(1).id);
    assert_eq!(e.result, Some(Bytes(b"\"ok\"".to_vec())));
    assert_eq!((e.ts_ms, e.source_run.as_str()), (9, "p1.a.0"));
    assert_eq!(order(&w), vec!["k1".to_owned()]);
    assert_eq!(refs(&w, 1), 2, "the pipeline run and the entry");
}

#[test]
fn a_failed_run_or_a_run_without_a_key_writes_no_entry() {
    let mut w = World::new();
    let _b = begin(&mut w, PIPE, &["a", "b"]);
    let runs = vec![keyed(PIPE, "a", 0, "k1")];
    let _s = decide(
        &mut w,
        PIPE,
        "a",
        StepVerdict::Start {
            runs,
            fan: FanOut::SINGLE,
            cached: Vec::new(),
        },
    );
    let k = job_key(1);
    let _a = w.assign("p1.a.0", &k);
    let failed = trapped(&w, "p1.a.0", &k);
    let _f = w.finish(failed);
    assert!(entry(&w, "k1").is_none(), "a failed run");
    let _s = start_step(&mut w, PIPE, "b");
    let _r = run_done(&mut w, "p1.b.0", 2);
    assert!(order(&w).is_empty(), "a run with no key");
    assert_eq!(refs(&w, 2), 1);
}

#[test]
fn a_cached_decision_settles_the_step_with_no_run_record() {
    let mut w = World::new();
    make_entry(&mut w, PIPE, "k1", 1, 9);
    let a = start_cached(&mut w, "p2", vec![hit(0, "k1")], 20);
    assert!(a.status == Status::Ok, "{a:?}");
    let step = &pipeline(&w, "p2").steps["a"];
    assert_eq!(step.status, StepStatus::Succeeded);
    assert!(step.is_cached());
    assert_eq!(step.outputs().next().unwrap().id, out(1).id);
    assert_eq!(step.result(), Some(&Bytes(b"\"ok\"".to_vec())));
    assert_eq!(step.finished, 1);
    assert!(w.r.machine.run_record("p2.a.0").is_none());
    assert_eq!(refs(&w, 1), 3, "the first run, the entry and this run");
    assert_eq!(
        entry(&w, "k1").unwrap().ts_ms,
        20,
        "a hit refreshes the entry"
    );
    assert_eq!(pipeline(&w, "p2").state, PipelineState::Succeeded);
}

#[test]
fn a_fan_out_takes_its_cached_items_in_item_order() {
    let mut w = World::new();
    make_entry(&mut w, "p1", "k0", 1, 9);
    make_entry(&mut w, "q1", "k2", 2, 9);
    let _b = begin(&mut w, "p2", &["a"]);
    let fan = FanOut {
        total: 3,
        items: 3,
        batch: 1,
        allowed: 0,
    };
    let start = StepVerdict::Start {
        runs: vec![step_run("p2", "a", 1)],
        fan,
        cached: vec![hit(0, "k0"), hit(2, "k2")],
    };
    let a = decide(&mut w, "p2", "a", start);
    assert!(a.status == Status::Ok, "{a:?}");
    let step = &pipeline(&w, "p2").steps["a"];
    assert_eq!(step.status, StepStatus::Running);
    assert_eq!(step.runs.len(), 3);
    assert_eq!(step.finished, 2);
    assert_eq!(w.record("p2.a.1").pipeline.unwrap().index, 1);
    let _r = run_done(&mut w, "p2.a.1", 3);
    let step = &pipeline(&w, "p2").steps["a"];
    assert_eq!(step.status, StepStatus::Succeeded);
    assert!(!step.is_cached());
    assert_eq!(step.cached_items, 2);
    let ids: Vec<String> = step.outputs().map(|o| o.id.clone()).collect();
    assert_eq!(ids, vec![out(1).id, out(3).id, out(2).id]);
}

#[test]
fn a_cached_decision_whose_entry_is_gone_is_refused_from_that_index() {
    let mut w = World::new();
    make_entry(&mut w, PIPE, "k0", 1, 9);
    let _b = begin(&mut w, "p2", &["a"]);
    let fan = FanOut {
        total: 2,
        items: 2,
        batch: 1,
        allowed: 0,
    };
    let none = StepVerdict::Start {
        runs: Vec::new(),
        fan,
        cached: vec![hit(0, "missing"), hit(1, "k0")],
    };
    assert_eq!(why(&decide(&mut w, "p2", "a", none)), CACHE_GONE);
    assert_eq!(status(&w, "p2", "a"), StepStatus::Pending);
    let part = StepVerdict::Start {
        runs: Vec::new(),
        fan,
        cached: vec![hit(0, "k0"), hit(1, "missing")],
    };
    let a = decide(&mut w, "p2", "a", part);
    assert!(a.status == Status::Ok, "{a:?}");
    let step = &pipeline(&w, "p2").steps["a"];
    assert_eq!(
        (step.status, step.runs.len(), step.finished),
        (StepStatus::Running, 1, 1)
    );
}

#[test]
fn a_decision_that_does_not_tile_its_indices_fails_the_step() {
    let mut w = World::new();
    make_entry(&mut w, PIPE, "k0", 1, 9);
    let fan = FanOut {
        total: 2,
        items: 2,
        batch: 1,
        allowed: 0,
    };
    for (id, cached) in [
        ("p2", vec![hit(0, "k0"), hit(0, "k0")]),
        ("p3", vec![hit(5, "k0")]),
    ] {
        let _b = begin(&mut w, id, &["a"]);
        let start = StepVerdict::Start {
            runs: Vec::new(),
            fan,
            cached,
        };
        let a = decide(&mut w, id, "a", start);
        assert_eq!(status(&w, id, "a"), StepStatus::Failed, "{a:?}");
        assert_eq!(
            pipeline(&w, id).steps["a"].reason.as_deref(),
            Some("BAD_START")
        );
    }
}

#[test]
fn past_the_entry_limit_the_oldest_goes_and_releases_its_holds() {
    let mut w = World::new();
    for i in 0..=MAX_CACHE_ENTRIES {
        let n = u8::try_from(i + 1).unwrap();
        make_entry(&mut w, &format!("e{i}"), &format!("k{i}"), n, 9);
    }
    assert!(entry(&w, "k0").is_none());
    assert_eq!(order(&w).len(), MAX_CACHE_ENTRIES);
    assert_eq!(
        refs(&w, 1),
        1,
        "only the pipeline run holds the evicted one"
    );
    assert_eq!(refs(&w, 2), 2);
}

fn aged_world() -> World {
    let mut w = World::new();
    for (i, ts) in [(0u8, 100u64), (1, 100), (2, 100), (3, 200)] {
        make_entry(&mut w, &format!("e{i}"), &format!("k{i}"), i + 1, ts);
    }
    w
}

#[test]
fn entries_expire_by_entry_timestamps_a_few_per_entry() {
    let mut w = aged_world();
    let late = 100 + CACHE_TTL_MS;
    let _b = begin(&mut w, "z", &["a"]);
    let _d = decide_at(&mut w, "z", "a", StepVerdict::Skipped, late);
    assert_eq!(
        order(&w),
        vec!["k2".to_owned(), "k3".to_owned()],
        "two dropped at most"
    );
    assert!(entry(&w, "k0").is_none() && entry(&w, "k1").is_none());
    assert_eq!(refs(&w, 1), 1, "the dropped entry released its hold");
    let _b = begin(&mut w, "y", &["a"]);
    let _d = decide_at(&mut w, "y", "a", StepVerdict::Skipped, late);
    assert_eq!(
        order(&w),
        vec!["k3".to_owned()],
        "k3 is younger than the limit"
    );
}

#[test]
fn two_replicas_fed_the_same_entries_expire_the_same_entries() {
    let run = || {
        let mut w = aged_world();
        let _b = begin(&mut w, "z", &["a"]);
        let _d = decide_at(&mut w, "z", "a", StepVerdict::Skipped, 100 + CACHE_TTL_MS);
        w
    };
    let (a, b) = (run(), run());
    assert_eq!(order(&a), order(&b));
    for n in 1..=4 {
        assert_eq!(refs(&a, n), refs(&b, n));
    }
    assert_eq!(
        a.r.machine.state.component.head,
        b.r.machine.state.component.head
    );
}

/// A table whose run at `step_run`'s caps costs 55 at worst: 5 vCPU-seconds at 10 and 1 GiB-second at 5.
const TABLE: calimero_pipeline::PriceTable = calimero_pipeline::PriceTable {
    version: 1,
    fuel_per_vcpu_second: 1,
    vcpu_second: 10,
    gib_second: 5,
    connector_gib: 0,
    storage_gib_month: 0,
    egress_gib: 0,
};
const UNIT: u64 = 55;

fn begin_capped(w: &mut World, id: &str, max_cost: u64) -> Applied {
    let entry = PipelineStartEntry {
        cost: Some(CostCap {
            max_cost,
            price_version: 1,
            table: TABLE,
            worst_case: 0,
        }),
        ..start_entry(id, &["a"])
    };
    w.push(&ComponentCmd::PipelineStart(Box::new(entry)))
}

/// A done final of `run` that burned `fuel`.
fn run_done_burning(w: &mut World, run: &str, n: u8, fuel: u64) -> Applied {
    let k = job_key(1);
    let _a = w.assign(run, &k);
    let mut entry = done_with(w, run, &k, n);
    entry.usage.fuel = fuel;
    if let Some(rep) = entry.report.as_mut() {
        rep.usage.fuel = fuel;
        entry.signature = Some(sign_report(&k, &DIGEST, rep).unwrap());
    }
    w.finish(entry)
}

#[test]
fn a_start_that_would_cross_the_cost_cap_is_refused_and_changes_nothing() {
    let mut w = World::new();
    let _b = begin_capped(&mut w, PIPE, 2 * UNIT - 1);
    let a = fan_start(&mut w, "a", 3, 0, 2);
    assert_eq!(why(&a), COST_CAP);
    assert_eq!(status(&w, PIPE, "a"), StepStatus::Pending);
    assert!(w.r.machine.run_record("p1.a.0").is_none());
    let a = fan_start(&mut w, "a", 3, 0, 1);
    assert!(a.status == Status::Ok, "{a:?}");
    assert_eq!(pipeline(&w, PIPE).committed_cost(), u128::from(UNIT));
    assert_eq!(why(&more(&mut w, "a", 1, 2)), COST_CAP);
    assert_eq!(pipeline(&w, PIPE).steps["a"].runs.len(), 1);
    assert!(w.r.machine.run_record("p1.a.1").is_none());
}

#[test]
fn a_cost_cap_counts_what_finished_runs_used_not_their_worst_case() {
    let mut w = World::new();
    let _b = begin_capped(&mut w, PIPE, 2 * UNIT);
    let _s = fan_start(&mut w, "a", 3, 0, 1);
    let _r = run_done_burning(&mut w, "p1.a.0", 1, 1);
    let p = pipeline(&w, PIPE);
    // 1 vCPU-second at 10 and 1 GiB-second at 5
    assert_eq!((p.cost_so_far, p.steps["a"].cost), (15, 15));
    assert_eq!(p.committed_cost(), 15);
    let a = more(&mut w, "a", 1, 3);
    assert_eq!(why(&a), COST_CAP, "15 and two worst cases pass the cap");
    let a = more(&mut w, "a", 1, 2);
    assert!(a.status == Status::Ok, "{a:?}");
    assert_eq!(pipeline(&w, PIPE).committed_cost(), u128::from(15 + UNIT));
}

#[test]
fn a_run_answered_from_the_cache_costs_nothing() {
    let mut w = World::new();
    make_entry(&mut w, "e", "k1", 1, 9);
    let _b = begin_capped(&mut w, PIPE, UNIT);
    let fan = FanOut {
        total: 2,
        items: 2,
        batch: 1,
        allowed: 0,
    };
    let start = StepVerdict::Start {
        runs: vec![step_run(PIPE, "a", 0)],
        fan,
        cached: vec![hit(1, "k1")],
    };
    let a = decide(&mut w, PIPE, "a", start);
    assert!(a.status == Status::Ok, "{a:?}");
    let p = pipeline(&w, PIPE);
    assert_eq!((p.cost_so_far, p.committed_cost()), (0, u128::from(UNIT)));
}

#[test]
fn a_capped_step_lets_its_active_runs_finish_then_fails_with_the_cap() {
    let mut w = World::new();
    let _b = begin_capped(&mut w, PIPE, UNIT);
    let _s = fan_start(&mut w, "a", 3, 0, 1);
    let capped = StepVerdict::Failed {
        reason: COST_CAP.into(),
    };
    let a = decide(&mut w, PIPE, "a", capped.clone());
    assert!(a.status == Status::Ok, "{a:?}");
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Running, "the active run goes on");
    assert!(step.capped && !w.record("p1.a.0").cancel_requested);
    assert_eq!(why(&more(&mut w, "a", 1, 2)), COST_CAP);
    let again = decide(&mut w, PIPE, "a", capped);
    assert!(again.status == Status::Ok, "a repeated cap is the same cap");
    let _r = run_done_burning(&mut w, "p1.a.0", 1, 5);
    let p = pipeline(&w, PIPE);
    assert_eq!(p.steps["a"].status, StepStatus::Failed);
    assert_eq!(p.steps["a"].reason.as_deref(), Some(COST_CAP));
    assert_eq!(p.state, PipelineState::Failed);
    assert!(p.cost_so_far <= UNIT);
}

#[test]
fn a_retry_records_what_it_repeats_and_the_limits_it_raised() {
    let mut w = World::new();
    let limits = BTreeMap::from([(
        "a".to_owned(),
        calimero_pipeline::Limits {
            fuel: Some(9),
            ..Default::default()
        },
    )]);
    let entry = PipelineStartEntry {
        retry_of: Some("old".into()),
        limits: limits.clone(),
        ..start_entry(PIPE, &["a"])
    };
    let a = w.push(&ComponentCmd::PipelineStart(Box::new(entry.clone())));
    assert!(a.status == Status::Ok, "{a:?}");
    let p = pipeline(&w, PIPE);
    assert_eq!((p.retry_of.as_deref(), &p.limits), (Some("old"), &limits));
    let again = w.push(&ComponentCmd::PipelineStart(Box::new(entry)));
    assert!(
        again.status == Status::Ok,
        "the same start repeated is the same run"
    );
    let other = PipelineStartEntry {
        retry_of: Some("newer".into()),
        ..start_entry(PIPE, &["a"])
    };
    let a = w.push(&ComponentCmd::PipelineStart(Box::new(other)));
    assert_eq!(why(&a), KNOWN, "another retry of it is not the same run");
}

#[test]
fn limits_for_a_step_the_run_lacks_are_refused() {
    let mut w = World::new();
    let entry = PipelineStartEntry {
        limits: BTreeMap::from([("ghost".to_owned(), calimero_pipeline::Limits::default())]),
        ..start_entry(PIPE, &["a"])
    };
    let a = w.push(&ComponentCmd::PipelineStart(Box::new(entry)));
    assert_eq!(why(&a), BAD_START);
    assert!(w.r.machine.pipeline_run(PIPE).is_none());
}

#[test]
fn a_run_without_a_cost_cap_records_no_cost() {
    let mut w = World::new();
    let _b = begin(&mut w, PIPE, &["a"]);
    let _s = start_step(&mut w, PIPE, "a");
    let _r = run_done(&mut w, "p1.a.0", 1);
    let p = pipeline(&w, PIPE);
    assert!(p.cost.is_none() && p.cost_so_far == 0 && p.steps["a"].cost == 0);
}

#[test]
fn a_step_runs_in_a_module_the_run_pinned_and_in_no_other() {
    let mut w = World::new();
    let mut entry = start_entry(PIPE, &["a", "b"]);
    entry.modules = BTreeMap::from([("pdf@1".to_owned(), "module-v1".to_owned())]);
    let _a = w.push(&ComponentCmd::PipelineStart(Box::new(entry)));

    let mut pinned = step_run(PIPE, "a", 0);
    pinned.compute_bundle = Some("module-v1".into());
    let _d = decide(
        &mut w,
        PIPE,
        "a",
        StepVerdict::Start {
            runs: vec![pinned],
            fan: FanOut::SINGLE,
            cached: Vec::new(),
        },
    );
    assert_eq!(
        w.record("p1.a.0").compute_bundle.as_deref(),
        Some("module-v1")
    );

    let mut other = step_run(PIPE, "b", 0);
    other.compute_bundle = Some("module-v2".into());
    let _d = decide(
        &mut w,
        PIPE,
        "b",
        StepVerdict::Start {
            runs: vec![other],
            fan: FanOut::SINGLE,
            cached: Vec::new(),
        },
    );
    assert_eq!(status(&w, PIPE, "b"), StepStatus::Failed);
    assert_eq!(
        pipeline(&w, PIPE).steps["b"].reason.as_deref(),
        Some("UNKNOWN_MODULE")
    );
    assert!(w.r.machine.run_record("p1.b.0").is_none());
}

fn proof_of(w: &World, run: &str) -> calimero_client::receipt::StepRunProof {
    w.r.machine.step_proof(run).expect("no proof")
}

#[test]
fn a_linked_done_final_writes_a_proof_with_the_attempts_key_and_digest() {
    let mut w = World::new();
    let _b = begin(&mut w, PIPE, &["a"]);
    let _s = start_step(&mut w, PIPE, "a");
    let _r = run_done(&mut w, "p1.a.0", 1);
    let p = proof_of(&w, "p1.a.0");
    let k = job_key(1);
    assert_eq!((p.step.as_str(), p.index, p.attempt), ("a", 0, 1));
    assert_eq!(p.outcome, "done");
    assert_eq!(
        p.job_key.as_deref(),
        Some(hex::encode(k.verifying_key().to_bytes()).as_str())
    );
    assert_eq!(
        p.assignment_digest.as_deref(),
        Some(hex::encode(DIGEST).as_str())
    );
    let rep = report("p1.a.0", 1, ReportStatus::Done, None, &[out(1)]);
    let message = crate::job_wire::report_message(&rep, &DIGEST).unwrap();
    assert_eq!(
        p.report_sha256.as_deref(),
        Some(hex::encode(calimero_identity::sha256(&message)).as_str())
    );
    assert_eq!(
        p.signature,
        Some(sign_report(&k, &DIGEST, &rep).unwrap()),
        "the job's own signature"
    );
    assert_eq!(p.outputs.len(), 1);
    assert_eq!(p.outputs[0].id, out(1).id);
}

#[test]
fn a_leader_ended_run_has_a_proof_without_report_fields() {
    let mut w = World::new();
    let _b = begin(&mut w, PIPE, &["a"]);
    let _s = start_step(&mut w, PIPE, "a");
    let k = job_key(1);
    let _a = w.assign("p1.a.0", &k);
    let _c = w.push(&ComponentCmd::PipelineCancel(Box::new(
        PipelineCancelEntry {
            run: PIPE.into(),
            ts_ms: 7,
        },
    )));
    let entry = leader(&w, "p1.a.0", FinalOutcome::Cancelled);
    let _f = w.finish(entry);
    let p = proof_of(&w, "p1.a.0");
    assert_eq!(p.outcome, "cancelled");
    assert!(p.report_sha256.is_none() && p.signature.is_none());
    assert!(p.job_key.is_some(), "the attempt it was assigned to");
    assert!(p.outputs.is_empty());
}

#[test]
fn a_failed_job_run_keeps_its_proof_with_the_reason() {
    let mut w = World::new();
    let _b = begin(&mut w, PIPE, &["a"]);
    let _s = start_step(&mut w, PIPE, "a");
    let k = job_key(1);
    let _a = w.assign("p1.a.0", &k);
    let failed = trapped(&w, "p1.a.0", &k);
    let _f = w.finish(failed);
    let p = proof_of(&w, "p1.a.0");
    assert_eq!(p.outcome, "failed:trap");
    assert!(p.signature.is_some());
}

#[test]
fn a_cached_run_writes_no_proof_and_the_step_remembers_its_key() {
    let mut w = World::new();
    make_entry(&mut w, PIPE, "k1", 1, 9);
    let _a = start_cached(&mut w, "p2", vec![hit(0, "k1")], 20);
    assert!(w.r.machine.step_proof("p2.a.0").is_none());
    let step = &pipeline(&w, "p2").steps["a"];
    assert_eq!(step.cached_keys.get(&0).map(String::as_str), Some("k1"));
}

#[test]
fn a_dropped_pipeline_run_drops_its_proofs() {
    let mut w = World::new();
    let _a = begin(&mut w, "first", &["a"]);
    let _s = start_step(&mut w, "first", "a");
    let _r = run_done(&mut w, "first.a.0", 1);
    assert!(w.r.machine.step_proof("first.a.0").is_some());
    for i in 0..FINISHED_PIPELINE_RUNS_KEPT {
        let id = format!("n{i}");
        let _b = begin(&mut w, &id, &["a"]);
        let _d = decide(&mut w, &id, "a", StepVerdict::Skipped);
    }
    assert!(w.r.machine.pipeline_run("first").is_none());
    assert!(w.r.machine.step_proof("first.a.0").is_none());
}

#[test]
fn two_replicas_fed_the_same_entries_hold_the_same_proofs() {
    let run = || {
        let mut w = World::new();
        let _b = begin(&mut w, PIPE, &["a"]);
        let _s = start_step(&mut w, PIPE, "a");
        let _r = run_done(&mut w, "p1.a.0", 1);
        w
    };
    let (a, b) = (run(), run());
    assert_eq!(proof_of(&a, "p1.a.0"), proof_of(&b, "p1.a.0"));
}

#[test]
fn a_pipeline_run_records_its_input_files_as_they_were_at_start() {
    let mut w = World::new();
    let _b = begin(&mut w, PIPE, &["a"]);
    let run = pipeline(&w, PIPE);
    let held = &run.input_files[&fid(INPUT)];
    assert_eq!(held.id, fid(INPUT));
}

#[test]
fn a_receipt_still_lists_every_run_after_the_run_records_were_dropped() {
    let total = FINISHED_RUNS_KEPT + 20;
    let mut w = World::new();
    let _b = begin(&mut w, PIPE, &["a"]);
    let n = u32::try_from(total).unwrap();
    let _s = fan_start(&mut w, "a", n, n, 16);
    let mut from = 0;
    while from < total {
        let to = (from + 16).min(total);
        if from > 0 {
            let m = more(&mut w, "a", from, to);
            assert!(m.status == Status::Ok, "{from}: {m:?}");
        }
        for i in from..to {
            let _r = run_failed(&mut w, &format!("p1.a.{i}"));
        }
        from = to;
    }
    assert_eq!(
        pipeline(&w, PIPE).state,
        PipelineState::SucceededWithFailures
    );
    assert!(
        w.r.machine.run_record("p1.a.0").is_none(),
        "the first run's record is gone"
    );
    let source = crate::api::pipeline_receipt::ReceiptSource {
        proofs: (0..total)
            .filter_map(|i| {
                let id = format!("p1.a.{i}");
                Some((id.clone(), w.r.machine.step_proof(&id)?))
            })
            .collect(),
        run: pipeline(&w, PIPE),
    };
    let receipt = crate::api::pipeline_receipt::build_receipt(
        &"cc".repeat(32),
        "app",
        &source.run,
        &source.proofs,
    );
    assert_eq!(receipt.steps["a"].runs.len(), total);
    assert_eq!(receipt.modules["self"], "compute-v1");
    assert_eq!(receipt.inputs["in"].len(), 1);
}

#[test]
fn a_receipt_marks_cached_runs_with_their_key_and_keeps_index_order() {
    use calimero_client::receipt::RunEntry;
    let mut w = World::new();
    make_entry(&mut w, PIPE, "k1", 1, 9);
    let _b = begin(&mut w, "p3", &["a"]);
    let fan = FanOut {
        total: 2,
        items: 2,
        batch: 1,
        allowed: 0,
    };
    let _d = decide(
        &mut w,
        "p3",
        "a",
        StepVerdict::Start {
            runs: vec![step_run("p3", "a", 1)],
            fan,
            cached: vec![hit(0, "k1")],
        },
    );
    let _r = run_done(&mut w, "p3.a.1", 2);
    let run = pipeline(&w, "p3");
    let proofs = std::iter::once("p3.a.1")
        .filter_map(|id| Some((id.to_owned(), w.r.machine.step_proof(id)?)))
        .collect();
    let receipt =
        crate::api::pipeline_receipt::build_receipt(&"cc".repeat(32), "app", &run, &proofs);
    let runs = &receipt.steps["a"].runs;
    assert!(
        matches!(&runs[0], RunEntry::Cached(c) if c.index == 0 && c.cache_key == "k1"),
        "{runs:?}"
    );
    assert!(
        matches!(&runs[1], RunEntry::Ran(p) if p.index == 1),
        "{runs:?}"
    );
}

fn group(w: &World) -> &calimero_log::GroupState {
    w.r.machine.state.kv.group().expect("no group")
}

fn record_len(w: &World, id: &str) -> usize {
    group(w)
        .get_hidden(rows::CPIPE, id.as_bytes())
        .unwrap()
        .len()
}

fn item_row_exists(w: &World, run: &str, step: &str, index: u32) -> bool {
    let key = crate::component::pipelines::item_key(run, step, index);
    group(w).get_hidden(rows::CITEM, key.as_bytes()).is_some()
}

/// Runs `0..total` of step `a` to done, `wave` at a time, each finishing in reverse order within
/// its wave, run `i` making output `i + 1` and result `i`. `seen` gets the record's size after
/// each completion.
fn finish_fan_out(w: &mut World, total: usize, wave: usize, mut seen: impl FnMut(&World)) {
    let _a = begin(w, PIPE, &["a"]);
    let _s = fan_start(w, "a", total as u32, 0, wave.min(total));
    for first in (0..total).step_by(wave) {
        let end = (first + wave).min(total);
        if first > 0 {
            let a = more(w, "a", first, end);
            assert!(a.status == Status::Ok, "{a:?}");
        }
        for i in (first..end).rev() {
            let n = u8::try_from(i + 1).unwrap();
            let _r = run_done_with_result(w, &format!("p1.a.{i}"), n, i.to_string().as_bytes());
            seen(w);
        }
    }
}

#[test]
fn a_step_completion_rewrites_a_record_whose_size_does_not_grow_with_the_fan_out() {
    let mut w = World::new();
    let mut sizes = Vec::new();
    finish_fan_out(&mut w, 200, 32, |w| sizes.push(record_len(w, PIPE)));
    let (first, before_last) = (sizes[0], sizes[sizes.len() - 2]);
    assert!(
        before_last <= first + 32,
        "the record grew from {first} to {before_last} bytes"
    );
}

#[test]
fn a_step_s_runs_read_back_in_index_order() {
    let mut w = World::new();
    finish_fan_out(&mut w, 105, 60, |_| {});
    let step = &pipeline(&w, PIPE).steps["a"];
    assert_eq!(step.status, StepStatus::Succeeded);
    let ids: Vec<String> = step.outputs().map(|o| o.id.clone()).collect();
    let want: Vec<String> = (0..105).map(|i| out(i + 1).id).collect();
    assert_eq!(ids, want);
    let results: Vec<String> = step
        .run_results
        .values()
        .map(|b| String::from_utf8(b.0.clone()).unwrap())
        .collect();
    let want: Vec<String> = (0..105).map(|i| i.to_string()).collect();
    assert_eq!(results, want);
    assert_eq!(step.runs.len(), 105);
    assert_eq!(step.runs[104], "p1.a.104");
}

#[test]
fn an_item_row_key_sorts_by_index() {
    let key = |i| crate::component::pipelines::item_key("p1", "a", i);
    assert_eq!(key(7), "p1/a/00000007");
    let mut keys: Vec<_> = [100, 9, 10, 1].into_iter().map(key).collect();
    keys.sort();
    assert_eq!(keys, [key(1), key(9), key(10), key(100)]);
}

#[test]
fn dropping_an_ended_run_deletes_its_item_rows_and_unpins_its_outputs() {
    let mut w = World::new();
    let _a = begin(&mut w, "first", &["a"]);
    let _s = decide(
        &mut w,
        "first",
        "a",
        StepVerdict::Start {
            runs: runs_of("first", "a", 0, 2),
            fan: FanOut {
                total: 2,
                items: 2,
                batch: 1,
                allowed: 0,
            },
            cached: Vec::new(),
        },
    );
    let _r = run_done_with_result(&mut w, "first.a.0", 1, b"x");
    let _r = run_done(&mut w, "first.a.1", 2);
    assert_eq!(pipeline(&w, "first").state, PipelineState::Succeeded);
    assert!(item_row_exists(&w, "first", "a", 0) && item_row_exists(&w, "first", "a", 1));
    assert_eq!((refs(&w, 1), refs(&w, 2)), (1, 1));
    for i in 0..FINISHED_PIPELINE_RUNS_KEPT {
        let id = format!("n{i}");
        let _b = begin(&mut w, &id, &["a"]);
        let _d = decide(&mut w, &id, "a", StepVerdict::Skipped);
    }
    assert!(w.r.machine.pipeline_run("first").is_none());
    assert!(!item_row_exists(&w, "first", "a", 0) && !item_row_exists(&w, "first", "a", 1));
    assert_eq!((refs(&w, 1), refs(&w, 2)), (0, 0));
}

#[test]
fn a_later_step_fans_out_over_an_earlier_step_s_outputs_from_rows() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a", "b"]);
    let _s = fan_start(&mut w, "a", 3, 0, 3);
    for i in [2, 0, 1] {
        let _r = run_done_with_result(&mut w, &format!("p1.a.{i}"), i + 1, &[b'0' + i]);
    }
    let p = pipeline(&w, PIPE);
    let a = &p.steps["a"];
    let ids: Vec<&str> = a.outputs().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, [out(1).id, out(2).id, out(3).id]);
    let results: Vec<&[u8]> = a.run_results.values().map(|b| &b.0[..]).collect();
    assert_eq!(results, [&b"0"[..], b"1", b"2"]);
    assert!(a.status.is_ok() && p.steps["b"].status == StepStatus::Pending);
    assert!(item_row_exists(&w, PIPE, "a", 2));
}

/// Replaces the stored item row of `run`'s step `a` index `index` with `row`, or deletes it.
fn tamper_item(w: &World, run: &str, index: u32, row: Option<&[u8]>) {
    let state = w.r.engine.group_state(5);
    let mut tx = calimero_log::StateTx::new(&state);
    let key = crate::component::pipelines::item_key(run, "a", index);
    match row {
        Some(row) => tx.put_hidden(rows::CITEM, key.as_bytes(), row),
        None => tx.delete_hidden(rows::CITEM, key.as_bytes()),
    }
    tx.commit().unwrap();
}

fn three_runs_done(w: &mut World) {
    let _a = begin(w, PIPE, &["a"]);
    let _s = fan_start(w, "a", 3, 0, 3);
    for i in 0..3u8 {
        let _r = run_done_with_result(w, &format!("p1.a.{i}"), i + 1, &[b'0' + i]);
    }
}

#[test]
fn a_missing_or_corrupt_item_row_flags_the_step_instead_of_shortening_it() {
    let mut w = World::new();
    three_runs_done(&mut w);
    assert!(!pipeline(&w, PIPE).steps["a"].items_unreadable);
    tamper_item(&w, PIPE, 1, None);
    let a = &pipeline(&w, PIPE).steps["a"];
    assert!(a.items_unreadable, "a finished run's row is gone");
    tamper_item(&w, PIPE, 1, Some(b"not json"));
    assert!(pipeline(&w, PIPE).steps["a"].items_unreadable, "corrupt");
}

#[test]
fn a_failed_run_leaves_an_empty_row_so_a_tolerated_failure_is_not_a_lost_row() {
    let mut w = World::new();
    let _a = begin(&mut w, PIPE, &["a"]);
    let _s = fan_start(&mut w, "a", 2, 1, 2);
    let _f = run_failed(&mut w, "p1.a.0");
    let _r = run_done_with_result(&mut w, "p1.a.1", 2, b"1");
    let a = &pipeline(&w, PIPE).steps["a"];
    assert_eq!((a.status, a.failed_items), (StepStatus::Succeeded, 1));
    assert!(!a.items_unreadable);
    assert!(item_row_exists(&w, PIPE, "a", 0));
}

#[test]
fn the_head_read_carries_counters_and_no_item_data() {
    let mut w = World::new();
    three_runs_done(&mut w);
    let head = w.r.machine.pipeline_head(PIPE).unwrap();
    let a = &head.steps["a"];
    assert_eq!((a.started, a.finished), (3, 3));
    assert!(a.runs.is_empty() && a.run_outputs.is_empty() && a.run_results.is_empty());
    assert_eq!(pipeline(&w, PIPE).steps["a"].run_outputs.len(), 3);
}

#[test]
fn a_run_name_or_step_name_with_a_slash_is_refused() {
    let mut w = World::new();
    assert_eq!(why(&begin(&mut w, "a/b", &["a"])), BAD_START);
    assert_eq!(why(&begin(&mut w, PIPE, &["x/y"])), BAD_START);
    assert!(w.r.machine.active_pipelines().is_empty());
}
