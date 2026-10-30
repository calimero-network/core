//! The host core's compute-run policy: what a run may touch and what it reports.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use calimero_app_host::{
    stored_key, Abi, Audience, Budget, Caller, ComputeCaps, ComputeCtx, Directory, Event,
    FileError, FileInfo, FileRead, FileWrite, Files, HostCore, HostDenied, InvalidInvocation,
    Invocation, InvocationKind, MemoryFiles, MethodKind, NewFile, OrgRole, PrepareError, ReadError,
    Status, TrapKind, Usage, FILE_CORRUPT, FILE_UNAVAILABLE, GENERATION, MAX_COMPUTE_FILE_BYTES,
    MAX_COMPUTE_FUEL, MAX_COMPUTE_RESULT_BYTES, MAX_FILE_BYTES, MAX_PROGRESS_STEP_BYTES,
    MAX_RUN_FILE_BYTES, RUN_WROTE_TOO_MUCH,
};
use serde_json::{json, Value};

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

fn inv(kind: InvocationKind, inputs: &[&str]) -> Invocation {
    Invocation {
        kind,
        method: "digest".to_owned(),
        args: Vec::new(),
        caller: Caller::System,
        entry_ts_ms: 1_234_000,
        id_term: 0,
        id_seq: 0,
        seed: [9; 32],
        timer: None,
        compute: (kind == InvocationKind::Action).then(|| ComputeCtx {
            run_id: "run-1".to_owned(),
            owner: "owner".to_owned(),
            inputs: inputs.iter().map(|i| (*i).to_owned()).collect(),
        }),
    }
}

fn host(inv: Invocation, files: &MemoryFiles) -> HostCore {
    let mut state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let _ = state.insert(stored_key(GENERATION, b"k"), b"v".to_vec());
    HostCore::new("app", Arc::new(state), 0, inv, Arc::new(NoDirectory))
        .unwrap()
        .with_files(Arc::new(files.clone()))
}

fn run(inputs: &[&str], files: &MemoryFiles) -> HostCore {
    host(inv(InvocationKind::Action, inputs), files)
}

fn denied<T: std::fmt::Debug>(result: Result<T, HostDenied>) {
    result.expect_err("expected HostDenied");
}

fn bytes(host: &mut HostCore) -> [u8; 8] {
    let mut out = [0u8; 8];
    host.random_fill(&mut out);
    out
}

#[test]
fn an_action_needs_its_run_and_only_an_action_carries_one() {
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let make = |inv| {
        HostCore::new(
            "app",
            Arc::new(state.clone()),
            0,
            inv,
            Arc::new(NoDirectory),
        )
    };
    let mut action = inv(InvocationKind::Action, &[]);
    action.compute = None;
    assert_eq!(
        make(action).err(),
        Some(InvalidInvocation::ActionWithoutRun)
    );
    let mut call = inv(InvocationKind::Call, &[]);
    call.compute = Some(ComputeCtx {
        run_id: "r".to_owned(),
        owner: "owner".to_owned(),
        inputs: Vec::new(),
    });
    assert_eq!(make(call).err(), Some(InvalidInvocation::RunOutsideAction));
}

#[test]
fn a_run_has_no_state_events_ids_or_actions() {
    let files = MemoryFiles::default();
    let mut h = run(&[], &files);
    denied(h.storage_get(b"k"));
    denied(h.storage_put(b"k", b"v"));
    denied(h.storage_delete(b"k"));
    denied(h.storage_add(b"k", 1));
    denied(h.range_open(b"", b"", false));
    denied(h.unique_id());
    denied(h.start_action("m", b""));
    denied(h.xcall_send("a", "m", b""));
    denied(h.emit(Event {
        name: "e".to_owned(),
        data: Vec::new(),
        audience: Audience::All,
    }));
}

#[test]
fn a_run_reads_only_its_inputs_and_never_deletes() {
    let files = MemoryFiles::default();
    let given = files.insert(b"given");
    let other = files.insert(b"other");
    let mut h = run(&[&given], &files);
    assert_eq!(h.files_info(&given).unwrap().unwrap().size, 5);
    let r = h.files_open(&given).unwrap().unwrap();
    assert_eq!(h.reader_read(r, 10).unwrap().unwrap(), b"given");
    denied(h.files_info(&other));
    denied(h.files_open(&other));
    denied(h.files_delete(&given));
    denied(h.files_delete(&other));
    denied(h.files_open("nothing"));
}

#[test]
fn a_run_creates_outputs_up_to_the_compute_cap_not_the_logged_one() {
    let files = MemoryFiles::default();
    let mut h = run(&[], &files);
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"out").unwrap().unwrap();
    let id = h.writer_finish(w).unwrap().unwrap();
    let out = h.finish(Status::Ok, Vec::new(), Usage::default());
    assert_eq!(out.new_files.len(), 1);
    assert_eq!(out.new_files[0].id, id);

    let mut h = run(&[], &files);
    let w = h.files_create().unwrap().unwrap();
    let piece = vec![0u8; 1024 * 1024];
    let mut written = 0u64;
    let mut files_made = 0;
    // 47 MiB per file, so the 64 MiB logged-run cap would end this at the second file.
    while written < MAX_RUN_FILE_BYTES * 2 {
        let w = if written == 0 {
            w
        } else {
            h.files_create().unwrap().unwrap()
        };
        for _ in 0..40 {
            h.writer_write(w, &piece).unwrap().unwrap();
            written += piece.len() as u64;
        }
        let _ = h.writer_finish(w).unwrap().unwrap();
        files_made += 1;
    }
    assert!(files_made >= 3);
}

#[test]
fn clock_and_randomness_come_from_the_start_entry_and_the_seed() {
    let files = MemoryFiles::default();
    let mut a = run(&[], &files);
    let mut b = run(&[], &files);
    assert_eq!(a.wall_ms(), 1_234_000);
    assert_eq!(a.monotonic_ns(), 1_234_000 * 1_000_000);
    assert_eq!(bytes(&mut a), bytes(&mut b));

    let mut other = inv(InvocationKind::Action, &[]);
    other.seed = [10; 32];
    let mut c = host(other, &files);
    assert_ne!(bytes(&mut a), bytes(&mut c));
}

#[test]
fn run_id_inputs_and_progress() {
    let files = MemoryFiles::default();
    let seen: Arc<Mutex<Vec<(u8, String)>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let mut h = run(&["f1", "f2"], &files).with_progress(Arc::new(move |p, s| {
        sink.lock().unwrap().push((p, s.to_owned()))
    }));
    assert_eq!(h.compute_run_id().as_deref(), Some("run-1"));
    assert_eq!(h.compute_inputs(), ["f1", "f2"]);
    h.compute_progress(10, "reading").unwrap();
    h.compute_progress(250, &"é".repeat(MAX_PROGRESS_STEP_BYTES))
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0], (10, "reading".to_owned()));
    assert_eq!(seen[1].0, 100);
    assert_eq!(seen[1].1.len(), MAX_PROGRESS_STEP_BYTES);
    assert!(seen[1].1.chars().all(|c| c == 'é'));

    let call = host(inv(InvocationKind::Call, &[]), &files);
    assert_eq!(call.compute_run_id(), None);
    assert!(call.compute_inputs().is_empty());
    let mut call = call;
    denied(call.compute_progress(1, "x"));
}

#[test]
fn a_result_over_the_cap_ends_the_run_denied() {
    let files = MemoryFiles::default();
    let ok = run(&[], &files).finish(
        Status::Ok,
        vec![1; MAX_COMPUTE_RESULT_BYTES],
        Usage::default(),
    );
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(ok.result.len(), MAX_COMPUTE_RESULT_BYTES);

    let mut h = run(&[], &files);
    let w = h.files_create().unwrap().unwrap();
    let _ = h.writer_finish(w).unwrap().unwrap();
    let big = h.finish(
        Status::Ok,
        vec![1; MAX_COMPUTE_RESULT_BYTES + 1],
        Usage::default(),
    );
    assert_eq!(
        big.status,
        Status::Trap {
            kind: TrapKind::HostDenied,
            detail: "result too large".to_owned()
        }
    );
    assert!(big.result.is_empty() && big.new_files.is_empty());

    let call = host(inv(InvocationKind::Call, &[]), &files).finish(
        Status::Ok,
        vec![1; MAX_COMPUTE_RESULT_BYTES + 1],
        Usage::default(),
    );
    assert_eq!(call.status, Status::Ok);
}

fn abi(methods: Value) -> Result<Abi, PrepareError> {
    Abi::from_json(&json!({ "abiVersion": 1, "methods": methods }))
}

fn compute(fields: Value) -> Value {
    json!({
        "digest": { "kind": "compute", "compute": fields },
        "onDone": { "kind": "call", "computeResult": true },
    })
}

#[test]
fn compute_methods_read_their_caps_and_default_memory_and_wall() {
    let abi = abi(compute(json!({ "fuel": 5, "result": "onDone" }))).unwrap();
    assert_eq!(
        abi.methods["digest"].kind,
        MethodKind::Compute(ComputeCaps {
            fuel: 5,
            memory_mib: 4096,
            wall_secs: 900,
            result: "onDone".to_owned()
        })
    );
    assert!(abi.methods["onDone"].compute_result);
    assert!(!abi.methods["digest"].compute_result);
    let budget = match &abi.methods["digest"].kind {
        MethodKind::Compute(caps) => Budget::compute(caps),
        _ => unreachable!(),
    };
    assert_eq!((budget.fuel, budget.memory_pages), (5, 4096 * 16));
    assert_eq!(budget.wall_clock.as_secs(), 900);
}

#[test]
fn bad_compute_caps_are_refused() {
    let ok = json!({ "fuel": 5, "result": "onDone" });
    let with = |key: &str, value: Value| {
        let mut caps = ok.clone();
        caps[key] = value;
        caps
    };
    let mut bad = vec![
        with("fuel", json!(MAX_COMPUTE_FUEL + 1)),
        with("fuel", json!(0)),
        with("memoryMib", json!(4097)),
        with("wallSecs", json!(901)),
        with("wallSecs", json!("5")),
        with("result", json!("digest")),
        with("result", json!("missing")),
        json!({ "result": "onDone" }),
        json!({ "fuel": 5 }),
    ];
    bad.push(json!(3));
    for caps in bad {
        assert!(
            matches!(abi(compute(caps.clone())), Err(PrepareError::BadAbi(_))),
            "{caps}"
        );
    }
    let at_max =
        json!({ "fuel": MAX_COMPUTE_FUEL, "memoryMib": 4096, "wallSecs": 900, "result": "onDone" });
    assert!(abi(compute(at_max)).is_ok());
    assert!(matches!(
        abi(json!({ "x": { "kind": "compute" } })),
        Err(PrepareError::BadAbi(_))
    ));
    assert!(matches!(
        abi(json!({ "x": { "kind": "call", "compute": { "fuel": 1, "result": "x" } } })),
        Err(PrepareError::BadAbi(_))
    ));
}

/// A provider whose every operation fails the way the test chose.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Break {
    Open,
    Read,
    Create,
    Write,
    Finish,
    OpenUnavailable,
}

struct BrokenFiles(Break);

struct BrokenReader;

impl FileRead for BrokenReader {
    fn read(&mut self, _max: usize) -> Result<Vec<u8>, ReadError> {
        Err(ReadError::Failed("disk on fire".to_owned()))
    }
}

struct BrokenWriter(Break);

impl FileWrite for BrokenWriter {
    fn write(&mut self, _bytes: &[u8]) -> Result<(), String> {
        if self.0 == Break::Write {
            return Err("disk on fire".to_owned());
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<NewFile, String> {
        Err("disk on fire".to_owned())
    }
}

impl Files for BrokenFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        match self.0 {
            Break::Open => Err(FileError::Failed("disk on fire".to_owned())),
            Break::OpenUnavailable => Err(FileError::Unavailable),
            _ => Ok(Box::new(BrokenReader)),
        }
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        match self.0 {
            Break::Create => Err(FileError::Failed("disk on fire".to_owned())),
            other => Ok(Box::new(BrokenWriter(other))),
        }
    }
}

fn unavailable(out: &calimero_app_host::Outcome) -> bool {
    matches!(
        &out.status,
        Status::Trap { kind: TrapKind::Unavailable, detail } if detail == FILE_UNAVAILABLE
    )
}

#[test]
fn a_provider_failure_in_a_compute_run_is_a_trap_never_a_soft_error() {
    for broken in [
        Break::Open,
        Break::OpenUnavailable,
        Break::Read,
        Break::Create,
        Break::Write,
        Break::Finish,
    ] {
        let mut h = HostCore::new(
            "app",
            Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
            0,
            inv(InvocationKind::Action, &["in"]),
            Arc::new(NoDirectory),
        )
        .unwrap()
        .with_files(Arc::new(BrokenFiles(broken)));
        match broken {
            Break::Open | Break::OpenUnavailable => denied(h.files_open("in")),
            Break::Read => {
                let r = h.files_open("in").unwrap().unwrap();
                denied(h.reader_read(r, 10));
            }
            Break::Create => denied(h.files_create()),
            Break::Write => {
                let w = h.files_create().unwrap().unwrap();
                denied(h.writer_write(w, b"x"));
            }
            Break::Finish => {
                let w = h.files_create().unwrap().unwrap();
                h.writer_write(w, b"x").unwrap().unwrap();
                denied(h.writer_finish(w));
            }
        }
        let out = h.finish(Status::Ok, Vec::new(), Usage::default());
        assert!(unavailable(&out), "{:?}", out.status);
    }
}

#[test]
fn a_corrupt_input_is_still_an_integrity_denial_not_unavailable() {
    struct Corrupt;
    struct CorruptReader;
    impl FileRead for CorruptReader {
        fn read(&mut self, _max: usize) -> Result<Vec<u8>, ReadError> {
            Err(ReadError::Corrupt)
        }
    }
    impl Files for Corrupt {
        fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
            Ok(Box::new(CorruptReader))
        }
        fn info(&self, _id: &str) -> Option<FileInfo> {
            None
        }
        fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
            Err(FileError::Unavailable)
        }
    }
    let mut h = HostCore::new(
        "app",
        Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
        0,
        inv(InvocationKind::Action, &["in"]),
        Arc::new(NoDirectory),
    )
    .unwrap()
    .with_files(Arc::new(Corrupt));
    let r = h.files_open("in").unwrap().unwrap();
    assert_eq!(
        h.reader_read(r, 1).unwrap_err(),
        HostDenied(FILE_CORRUPT.to_owned())
    );
}

#[test]
fn an_app_error_counts_against_the_result_cap() {
    let files = MemoryFiles::default();
    let small = Status::AppError {
        code: "C".to_owned(),
        message: "m".to_owned(),
    };
    let out = run(&[], &files).finish(small.clone(), Vec::new(), Usage::default());
    assert_eq!(out.status, small);
    let big = Status::AppError {
        code: "C".to_owned(),
        message: "m".repeat(MAX_COMPUTE_RESULT_BYTES),
    };
    let out = run(&[], &files).finish(big.clone(), Vec::new(), Usage::default());
    assert_eq!(
        out.status,
        Status::Trap {
            kind: TrapKind::HostDenied,
            detail: "result too large".to_owned()
        }
    );
    let call = host(inv(InvocationKind::Call, &[]), &files).finish(
        big.clone(),
        Vec::new(),
        Usage::default(),
    );
    assert_eq!(call.status, big);
}

#[test]
fn an_empty_file_id_names_no_input() {
    let files = MemoryFiles::default();
    let mut h = run(&["a"], &files);
    denied(h.files_open(""));
    denied(h.files_info(""));
}

/// A directory that fails the test when asked anything.
struct LiveDirectory;

impl Directory for LiveDirectory {
    fn org(&self) -> Option<String> {
        panic!("a compute run consulted the directory");
    }
    fn app_owner(&self) -> String {
        panic!("a compute run consulted the directory");
    }
    fn org_role(&self, _account: &str) -> Option<OrgRole> {
        panic!("a compute run consulted the directory");
    }
    fn in_team(&self, _account: &str, _team: &str) -> bool {
        panic!("a compute run consulted the directory");
    }
    fn team_exists(&self, _team: &str) -> bool {
        panic!("a compute run consulted the directory");
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        panic!("a compute run consulted the directory");
    }
}

#[test]
fn a_compute_run_sees_a_constant_context() {
    let h = HostCore::new(
        "app",
        Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
        77,
        inv(InvocationKind::Action, &[]),
        Arc::new(LiveDirectory),
    )
    .unwrap();
    assert_eq!(h.org(), None);
    assert_eq!(h.org_role("alice"), None);
    assert_eq!(h.org_role_strict("alice").unwrap(), None);
    assert!(h.orgs_of("alice").is_empty());
    assert!(!h.in_team("alice", "t"));
    assert_eq!(h.applied_index(), 0);
    assert_eq!(h.app_owner(), "owner");
}

/// A provider that keeps nothing, so a run can "write" gigabytes.
struct Sink;

struct SinkWriter(u64);

impl FileWrite for SinkWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0 += bytes.len() as u64;
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<NewFile, String> {
        Ok(NewFile {
            id: format!("sink-{}", self.0),
            size: self.0,
            sha256: Vec::new(),
            wrapped_key: Vec::new(),
            ciphertext: Vec::new(),
        })
    }
}

impl Files for Sink {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Err(FileError::Failed("none".to_owned()))
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Ok(Box::new(SinkWriter(0)))
    }
}

#[test]
fn a_compute_run_stops_at_two_gib_of_output() {
    let mut h = HostCore::new(
        "app",
        Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
        0,
        inv(InvocationKind::Action, &[]),
        Arc::new(NoDirectory),
    )
    .unwrap()
    .with_files(Arc::new(Sink));
    let piece = vec![0u8; 1024 * 1024];
    let per_file = MAX_FILE_BYTES / piece.len() as u64;
    let mut written = 0u64;
    while written < MAX_COMPUTE_FILE_BYTES {
        let w = h.files_create().unwrap().unwrap();
        for _ in 0..per_file {
            if written >= MAX_COMPUTE_FILE_BYTES {
                break;
            }
            h.writer_write(w, &piece).unwrap().unwrap();
            written += piece.len() as u64;
        }
        let _ = h.writer_finish(w).unwrap();
        h.writer_close(w);
    }
    let w = h.files_create().unwrap().unwrap();
    assert_eq!(
        h.writer_write(w, b"x").unwrap().unwrap_err(),
        RUN_WROTE_TOO_MUCH
    );
}

#[test]
fn compute_keys_other_than_on_a_compute_method_are_refused() {
    let caps = json!({ "fuel": 1, "result": "onDone" });
    for kind in ["call", "view", "init"] {
        let methods = json!({
            "x": { "kind": kind, "compute": caps },
            "onDone": { "kind": "call", "computeResult": true },
        });
        assert!(
            matches!(abi(methods), Err(PrepareError::BadAbi(_))),
            "{kind}"
        );
    }
    let timer = json!({
        "x": { "kind": "timer", "every": "1m", "compute": caps },
        "onDone": { "kind": "call", "computeResult": true },
    });
    assert!(matches!(abi(timer), Err(PrepareError::BadAbi(_))));
}

#[test]
fn compute_result_is_only_for_call_methods() {
    for kind in ["view", "init", "compute"] {
        let mut method = json!({ "kind": kind, "computeResult": true });
        if kind == "compute" {
            method["compute"] = json!({ "fuel": 1, "result": "x" });
        }
        assert!(
            matches!(abi(json!({ "x": method })), Err(PrepareError::BadAbi(_))),
            "{kind}"
        );
    }
    let timer = json!({ "x": { "kind": "timer", "every": "1m", "computeResult": true } });
    assert!(matches!(abi(timer), Err(PrepareError::BadAbi(_))));
}
