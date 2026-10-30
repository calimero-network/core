//! The host core's policy for starting and cancelling compute runs.

use std::collections::BTreeMap;
use std::sync::Arc;

use calimero_app_host::{
    Abi, ActiveRun, Caller, ComputeCtx, Directory, FileInfo, HostCore, Invocation, InvocationKind,
    MemoryFiles, OrgRole, Runs, Status, TimerCtx, Usage, FILE_IS_RUN_INPUT, MAX_ACTIVE_RUNS,
    MAX_COMPUTE_ARGS_BYTES, MAX_RUN_INPUTS, MAX_STARTS_PER_RUN, NOT_A_COMPUTE_METHOD,
    NO_ACTIVE_RUN, RUN_ARGS_TOO_LARGE, RUN_DUPLICATE_INPUT, RUN_INPUT_MISSING, RUN_TOO_MANY_ACTIVE,
    RUN_TOO_MANY_INPUTS, RUN_TOO_MANY_STARTS,
};
use serde_json::json;
use sha2::{Digest, Sha256};

struct NoDirectory;

impl Directory for NoDirectory {
    fn org(&self) -> Option<String> {
        None
    }
    fn app_owner(&self) -> String {
        "owner".to_owned()
    }
    fn org_role(&self, _account: &str) -> Option<OrgRole> {
        None
    }
    fn in_team(&self, _account: &str, _team: &str) -> bool {
        false
    }
    fn team_exists(&self, _team: &str) -> bool {
        false
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        Vec::new()
    }
}

struct Active(Vec<ActiveRun>);

impl Runs for Active {
    fn active(&self) -> Result<Vec<ActiveRun>, String> {
        Ok(self.0.clone())
    }
}

fn abi() -> Abi {
    Abi::from_json(&json!({
        "abiVersion": 1,
        "methods": {
            "digest": { "kind": "compute",
                "compute": { "fuel": 5, "memoryMib": 64, "wallSecs": 7, "result": "onDone" } },
            "onDone": { "kind": "call", "computeResult": true },
            "plain": { "kind": "call" },
        }
    }))
    .unwrap()
}

fn inv(kind: InvocationKind) -> Invocation {
    Invocation {
        kind,
        method: "m".to_owned(),
        args: Vec::new(),
        caller: Caller::Account("ada".to_owned()),
        entry_ts_ms: 1_000,
        id_term: 3,
        id_seq: 42,
        seed: [7; 32],
        timer: (kind == InvocationKind::Timer).then_some(TimerCtx {
            scheduled_ms: 1,
            previous_ms: None,
        }),
        compute: (kind == InvocationKind::Action).then(|| ComputeCtx {
            run_id: "run".to_owned(),
            owner: "owner".to_owned(),
            inputs: Vec::new(),
        }),
    }
}

fn host_of(kind: InvocationKind, files: &MemoryFiles, active: Vec<ActiveRun>) -> HostCore {
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    HostCore::new("app", Arc::new(state), 0, inv(kind), Arc::new(NoDirectory))
        .unwrap()
        .with_files(Arc::new(files.clone()))
        .with_compute(abi(), Arc::new(Active(active)))
}

fn call(files: &MemoryFiles, active: Vec<ActiveRun>) -> HostCore {
    host_of(InvocationKind::Call, files, active)
}

fn active(run: &str, method: &str, args: &[u8], inputs: &[&str]) -> ActiveRun {
    ActiveRun {
        run: run.to_owned(),
        method: method.to_owned(),
        args_sha256: Sha256::digest(args).to_vec(),
        inputs: inputs.iter().map(|i| (*i).to_owned()).collect(),
        cancel_requested: false,
    }
}

fn ids(inputs: &[&str]) -> Vec<String> {
    inputs.iter().map(|i| (*i).to_owned()).collect()
}

fn finish(host: HostCore) -> calimero_app_host::Outcome {
    host.finish(Status::Ok, Vec::new(), Usage::default())
}

#[test]
fn start_and_cancel_are_for_logged_writes_only() {
    let files = MemoryFiles::default();
    let allowed = [
        InvocationKind::Call,
        InvocationKind::Timer,
        InvocationKind::Init,
        InvocationKind::XcallDeliver,
    ];
    for kind in allowed {
        let mut h = host_of(kind, &files, vec![active("r0", "digest", b"", &[])]);
        h.compute_start("digest", b"x", &[]).unwrap().unwrap();
        h.compute_cancel("r0").unwrap().unwrap();
    }
    for kind in [
        InvocationKind::View,
        InvocationKind::Action,
        InvocationKind::Migrate,
    ] {
        let mut h = host_of(kind, &files, vec![active("r0", "digest", b"", &[])]);
        h.compute_start("digest", b"x", &[]).unwrap_err();
        h.compute_cancel("r0").unwrap_err();
    }
}

#[test]
fn only_a_compute_method_of_the_abi_can_be_started() {
    let files = MemoryFiles::default();
    let mut h = call(&files, Vec::new());
    for method in ["plain", "onDone", "nothing"] {
        assert_eq!(
            h.compute_start(method, b"", &[]).unwrap().unwrap_err(),
            NOT_A_COMPUTE_METHOD
        );
    }
    let mut bare = HostCore::new(
        "app",
        Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
        0,
        inv(InvocationKind::Call),
        Arc::new(NoDirectory),
    )
    .unwrap();
    assert_eq!(
        bare.compute_start("digest", b"", &[]).unwrap().unwrap_err(),
        NOT_A_COMPUTE_METHOD
    );
}

#[test]
fn a_start_is_recorded_with_its_caps_inputs_and_a_unique_id_and_kept_only_when_ok() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let b = files.insert(b"second!");
    let mut h = call(&files, Vec::new());
    let run = h
        .compute_start("digest", b"args", &ids(&[&a, &b]))
        .unwrap()
        .unwrap();
    assert_eq!(run, "3.42.0");
    let other = h.compute_start("digest", b"other", &[]).unwrap().unwrap();
    assert_eq!(other, "3.42.1");
    let out = finish(h);
    assert_eq!(out.compute_starts.len(), 2);
    let started = &out.compute_starts[0];
    assert_eq!(
        (started.run.as_str(), started.method.as_str()),
        ("3.42.0", "digest")
    );
    assert_eq!(started.args, b"args");
    assert_eq!(
        (
            started.caps.fuel,
            started.caps.memory_mib,
            started.caps.wall_secs
        ),
        (5, 64, 7)
    );
    assert_eq!(started.caps.result, "onDone");
    let sizes: Vec<_> = started
        .inputs
        .iter()
        .map(|i| (i.id.clone(), i.size))
        .collect();
    assert_eq!(sizes, [(a.clone(), 5), (b.clone(), 7)]);
    assert_eq!(started.inputs[0].sha256, Sha256::digest(b"first").to_vec());

    let mut failed = call(&files, Vec::new());
    failed
        .compute_start("digest", b"args", &[])
        .unwrap()
        .unwrap();
    failed.compute_cancel("nothing").unwrap().unwrap_err();
    let out = failed.finish(
        Status::AppError {
            code: "x".to_owned(),
            message: String::new(),
        },
        Vec::new(),
        Usage::default(),
    );
    assert!(out.compute_starts.is_empty() && out.compute_cancels.is_empty());
}

#[test]
fn an_input_must_be_a_marked_file_not_deleted_in_this_run() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let mut h = call(&files, Vec::new());
    assert_eq!(
        h.compute_start("digest", b"", &ids(&["nothing"]))
            .unwrap()
            .unwrap_err(),
        RUN_INPUT_MISSING
    );
    h.files_delete(&a).unwrap().unwrap();
    assert_eq!(
        h.compute_start("digest", b"2", &ids(&[&a]))
            .unwrap()
            .unwrap_err(),
        RUN_INPUT_MISSING
    );
    // A file this run finished is not marked yet.
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"new").unwrap().unwrap();
    let made = h.writer_finish(w).unwrap().unwrap();
    assert_eq!(
        h.compute_start("digest", b"3", &ids(&[&made]))
            .unwrap()
            .unwrap_err(),
        RUN_INPUT_MISSING
    );
}

#[test]
fn the_inputs_args_and_declared_bytes_are_bounded() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let mut h = call(&files, Vec::new());
    let more: Vec<String> = (0..=MAX_RUN_INPUTS)
        .map(|i| files.insert(format!("file {i}").as_bytes()))
        .collect();
    assert_eq!(
        h.compute_start("digest", b"", &more).unwrap().unwrap_err(),
        RUN_TOO_MANY_INPUTS
    );
    let limit = more[..MAX_RUN_INPUTS].to_vec();
    let _ = &a;
    h.compute_start("digest", b"", &limit).unwrap().unwrap();
    let big = vec![0u8; MAX_COMPUTE_ARGS_BYTES + 1];
    assert_eq!(
        h.compute_start("digest", &big, &[]).unwrap().unwrap_err(),
        RUN_ARGS_TOO_LARGE
    );
    h.compute_start("digest", &big[1..], &[]).unwrap().unwrap();
}

#[test]
fn the_declared_input_bytes_are_capped() {
    let huge = Arc::new(HugeFiles);
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut h = HostCore::new(
        "app",
        Arc::new(state),
        0,
        inv(InvocationKind::Call),
        Arc::new(NoDirectory),
    )
    .unwrap()
    .with_files(huge)
    .with_compute(abi(), Arc::new(Active(Vec::new())));
    // Two files of 1 GiB + 1 are over 2 GiB together; one alone is not.
    h.compute_start("digest", b"", &ids(&["big"]))
        .unwrap()
        .unwrap();
    assert_eq!(
        h.compute_start("digest", b"", &ids(&["big", "bigger"]))
            .unwrap()
            .unwrap_err(),
        calimero_app_host::RUN_INPUTS_TOO_LARGE
    );
}

struct HugeFiles;

impl calimero_app_host::Files for HugeFiles {
    fn open(
        &self,
        _id: &str,
    ) -> Result<Box<dyn calimero_app_host::FileRead>, calimero_app_host::FileError> {
        Err(calimero_app_host::FileError::Unavailable)
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        Some(FileInfo {
            size: (1 << 30) + 1,
            sha256: vec![0; 32],
        })
    }
    fn create(
        &self,
    ) -> Result<Box<dyn calimero_app_host::FileWrite>, calimero_app_host::FileError> {
        Err(calimero_app_host::FileError::Unavailable)
    }
}

#[test]
fn the_same_method_args_and_inputs_is_the_same_run() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let b = files.insert(b"second");
    let mut h = call(&files, vec![active("old", "digest", b"args", &[&a, &b])]);
    // An active run with the same job answers its id and records nothing.
    assert_eq!(
        h.compute_start("digest", b"args", &ids(&[&a, &b]))
            .unwrap()
            .unwrap(),
        "old"
    );
    // Another order, other args, other inputs or another method are other jobs.
    let order = h
        .compute_start("digest", b"args", &ids(&[&b, &a]))
        .unwrap()
        .unwrap();
    let args = h
        .compute_start("digest", b"other", &ids(&[&a, &b]))
        .unwrap()
        .unwrap();
    let inputs = h
        .compute_start("digest", b"args", &ids(&[&a]))
        .unwrap()
        .unwrap();
    // Within one invocation too.
    let again = h
        .compute_start("digest", b"args", &ids(&[&b, &a]))
        .unwrap()
        .unwrap();
    assert_eq!(order, again);
    assert_eq!(finish(h).compute_starts.len(), 3);
    assert!(order != args && args != inputs && order != "old");
}

#[test]
fn a_repeated_start_is_answered_even_when_the_limits_are_reached() {
    let files = MemoryFiles::default();
    let full: Vec<_> = (0..MAX_ACTIVE_RUNS)
        .map(|i| active(&format!("r{i}"), "digest", format!("{i}").as_bytes(), &[]))
        .collect();
    let mut h = call(&files, full);
    assert_eq!(h.compute_start("digest", b"3", &[]).unwrap().unwrap(), "r3");
    assert_eq!(
        h.compute_start("digest", b"new", &[]).unwrap().unwrap_err(),
        RUN_TOO_MANY_ACTIVE
    );
}

#[test]
fn the_active_runs_and_the_starts_of_this_run_count_against_the_active_cap() {
    let files = MemoryFiles::default();
    let held: Vec<_> = (0..MAX_ACTIVE_RUNS - 2)
        .map(|i| active(&format!("r{i}"), "digest", format!("{i}").as_bytes(), &[]))
        .collect();
    let mut h = call(&files, held);
    h.compute_start("digest", b"a", &[]).unwrap().unwrap();
    h.compute_start("digest", b"b", &[]).unwrap().unwrap();
    assert_eq!(
        h.compute_start("digest", b"c", &[]).unwrap().unwrap_err(),
        RUN_TOO_MANY_ACTIVE
    );
}

#[test]
fn one_run_starts_at_most_sixteen() {
    let files = MemoryFiles::default();
    let mut h = call(&files, Vec::new());
    for i in 0..MAX_STARTS_PER_RUN {
        h.compute_start("digest", format!("{i}").as_bytes(), &[])
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        h.compute_start("digest", b"one more", &[])
            .unwrap()
            .unwrap_err(),
        RUN_TOO_MANY_STARTS
    );
    assert_eq!(finish(h).compute_starts.len(), MAX_STARTS_PER_RUN);
}

#[test]
fn only_an_active_run_can_be_cancelled_and_once() {
    let files = MemoryFiles::default();
    let mut h = call(&files, vec![active("r0", "digest", b"", &[])]);
    assert_eq!(h.compute_cancel("r1").unwrap().unwrap_err(), NO_ACTIVE_RUN);
    h.compute_cancel("r0").unwrap().unwrap();
    h.compute_cancel("r0").unwrap().unwrap();
    // A run started in this invocation is not active yet.
    let started = h.compute_start("digest", b"x", &[]).unwrap().unwrap();
    assert_eq!(
        h.compute_cancel(&started).unwrap().unwrap_err(),
        NO_ACTIVE_RUN
    );
    assert_eq!(finish(h).compute_cancels, ["r0"]);
}

#[test]
fn an_input_named_twice_is_refused() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let b = files.insert(b"second");
    let mut h = call(&files, Vec::new());
    assert_eq!(
        h.compute_start("digest", b"", &ids(&[&a, &b, &a]))
            .unwrap()
            .unwrap_err(),
        RUN_DUPLICATE_INPUT
    );
    h.compute_start("digest", b"", &ids(&[&a, &b]))
        .unwrap()
        .unwrap();
}

#[test]
fn a_file_that_a_run_starts_with_cannot_be_deleted_in_the_same_invocation() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let other = files.insert(b"other");
    let mut h = call(&files, Vec::new());
    h.compute_start("digest", b"", &ids(&[&a]))
        .unwrap()
        .unwrap();
    assert_eq!(h.files_delete(&a).unwrap().unwrap_err(), FILE_IS_RUN_INPUT);
    h.files_delete(&other).unwrap().unwrap();
    // The refusal changed nothing: the file is not deleted.
    let out = finish(h);
    assert_eq!(out.deleted_files, [other]);
}

#[test]
fn an_input_of_an_active_run_cannot_be_deleted_until_the_run_is_final() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let queued = active("r0", "digest", b"", &[&a]);
    let mut h = call(&files, vec![queued]);
    assert_eq!(h.files_delete(&a).unwrap().unwrap_err(), FILE_IS_RUN_INPUT);
    // Final: the run is no longer active, and the file can go.
    let mut later = call(&files, Vec::new());
    later.files_delete(&a).unwrap().unwrap();
}

#[test]
fn a_run_being_cancelled_is_not_the_answer_to_a_start_but_still_counts_and_protects_its_inputs() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let cancelling = ActiveRun {
        cancel_requested: true,
        ..active("old", "digest", b"args", &[&a])
    };
    let mut h = call(&files, vec![cancelling.clone()]);
    let fresh = h
        .compute_start("digest", b"args", &ids(&[&a]))
        .unwrap()
        .unwrap();
    assert_ne!(fresh, "old");
    assert_eq!(h.files_delete(&a).unwrap().unwrap_err(), FILE_IS_RUN_INPUT);
    // Counts toward the cap.
    let mut full: Vec<_> = (1..MAX_ACTIVE_RUNS)
        .map(|i| active(&format!("r{i}"), "digest", format!("{i}").as_bytes(), &[]))
        .collect();
    full.push(cancelling);
    let mut h = call(&files, full);
    assert_eq!(
        h.compute_start("digest", b"new", &[]).unwrap().unwrap_err(),
        RUN_TOO_MANY_ACTIVE
    );
}

#[test]
fn a_run_cancelled_in_this_call_is_not_the_answer_to_a_restart() {
    let files = MemoryFiles::default();
    let mut h = call(&files, vec![active("old", "digest", b"args", &[])]);
    h.compute_cancel("old").unwrap().unwrap();
    let fresh = h.compute_start("digest", b"args", &[]).unwrap().unwrap();
    assert_ne!(fresh, "old");
}

struct Unreadable;

impl Runs for Unreadable {
    fn active(&self) -> Result<Vec<ActiveRun>, String> {
        Err("bad row".to_owned())
    }
}

#[test]
fn unreadable_run_records_deny_the_calls_that_need_them() {
    let files = MemoryFiles::default();
    let a = files.insert(b"first");
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let make = || {
        HostCore::new(
            "app",
            Arc::new(state.clone()),
            0,
            inv(InvocationKind::Call),
            Arc::new(NoDirectory),
        )
        .unwrap()
        .with_files(Arc::new(files.clone()))
        .with_compute(abi(), Arc::new(Unreadable))
    };
    let denied = calimero_app_host::RUN_RECORDS_UNREADABLE;
    assert!(make()
        .compute_start("digest", b"", &[])
        .unwrap_err()
        .0
        .contains(denied));
    assert!(make().compute_cancel("r").unwrap_err().0.contains(denied));
    assert!(make().files_delete(&a).unwrap_err().0.contains(denied));
}
