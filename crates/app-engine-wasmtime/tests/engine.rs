use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use calimero_app_build::build_component;
use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    Budget, Caller, Directory, Engine, HostCore, Invocation, InvocationKind, OrgRole, Outcome,
    PrepareError, Prepared, Status, TrapKind,
};

const ENTRY_TS_MS: u64 = 1_700_000_000_123;
/// The key the tests' engines authenticate their cache with.
const CACHE_KEY: [u8; 32] = [5; 32];

fn guest(name: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/guests")
        .join(name);
    // One target dir for every guest, so wit-bindgen compiles once per test binary run.
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guests");
    build_component(&dir, &target)
        .unwrap_or_else(|e| panic!("build {name}: {e}"))
        .component
}

struct TestDir {
    panic_on_org: bool,
}

impl Directory for TestDir {
    fn org(&self) -> Option<String> {
        assert!(!self.panic_on_org, "directory blew up");
        Some("acme".to_owned())
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

fn host_with(kind: InvocationKind, method: &str, args: &[u8], panic_on_org: bool) -> HostCore {
    let inv = Invocation {
        kind,
        method: method.to_owned(),
        args: args.to_vec(),
        caller: Caller::Account("alice".to_owned()),
        entry_ts_ms: ENTRY_TS_MS,
        id_term: 1,
        id_seq: 1,
        seed: [7; 32],
        timer: None,
    };
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    HostCore::new(
        "app-1",
        Arc::new(state),
        1,
        inv,
        Arc::new(TestDir { panic_on_org }),
    )
    .expect("valid invocation")
}

fn host(kind: InvocationKind, method: &str, args: &[u8]) -> HostCore {
    host_with(kind, method, args, false)
}

struct Fixture {
    engine: WasmtimeEngine,
    cache: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let cache = tempfile::tempdir().expect("tempdir");
    let engine = WasmtimeEngine::new(cache.path().to_path_buf(), &CACHE_KEY).expect("engine");
    Fixture { engine, cache }
}

fn prepare(fx: &Fixture, name: &str) -> Prepared {
    fx.engine
        .prepare(&format!("{name}-hash"), &guest(name))
        .unwrap_or_else(|e| panic!("prepare {name}: {e}"))
}

fn trap_kind(outcome: &Outcome) -> Option<TrapKind> {
    match &outcome.status {
        Status::Trap { kind, .. } => Some(*kind),
        _ => None,
    }
}

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let fx = fixture();
    let app = prepare(&fx, "spin");
    let start = Instant::now();
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "spin", b""),
        Budget::call(),
    );
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::OutOfFuel),
        "{:?}",
        out.status
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(out.usage.fuel, Budget::call().fuel);
}

#[test]
fn epoch_deadline_times_out() {
    let fx = fixture();
    let app = prepare(&fx, "spin");
    let budget = Budget {
        fuel: u64::MAX,
        wall_clock: Duration::from_millis(200),
        ..Budget::call()
    };
    let start = Instant::now();
    let out = fx
        .engine
        .invoke(&app, host(InvocationKind::Call, "spin", b""), budget);
    assert_eq!(trap_kind(&out), Some(TrapKind::Timeout), "{:?}", out.status);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn memory_bomb_is_out_of_memory() {
    let fx = fixture();
    let app = prepare(&fx, "membomb");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "bomb", b""),
        Budget::call(),
    );
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::OutOfMemory),
        "{:?}",
        out.status
    );
}

#[test]
fn deep_recursion_is_stack_overflow() {
    let fx = fixture();
    let app = prepare(&fx, "deep");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "deep", b""),
        Budget::call(),
    );
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::StackOverflow),
        "{:?}",
        out.status
    );
}

#[test]
fn globals_do_not_survive_calls() {
    let fx = fixture();
    let app = prepare(&fx, "global_counter");
    for _ in 0..2 {
        let out = fx.engine.invoke(
            &app,
            host(InvocationKind::Call, "bump", b""),
            Budget::call(),
        );
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.result, 1u32.to_le_bytes());
    }
}

#[test]
fn view_writes_trap_host_denied() {
    let fx = fixture();
    let app = prepare(&fx, "view_writer");
    let out = fx
        .engine
        .invoke(&app, host(InvocationKind::View, "put", b""), Budget::view());
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::HostDenied),
        "{:?}",
        out.status
    );
    assert!(out.diff.is_empty());

    let out = fx
        .engine
        .invoke(&app, host(InvocationKind::Call, "put", b""), Budget::call());
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.diff.len(), 1);
}

#[test]
fn unlisted_import_is_refused_at_prepare() {
    let fx = fixture();
    match fx.engine.prepare("fs-hash", &guest("fs_importer")) {
        Err(PrepareError::ForbiddenImport(name)) => {
            assert!(name.starts_with("wasi:sockets/"), "{name}");
        }
        Err(other) => panic!("expected ForbiddenImport, got {other}"),
        Ok(_) => panic!("expected ForbiddenImport, got a prepared app"),
    }
}

#[test]
fn an_older_wasi_patch_of_a_stubbed_interface_is_accepted() {
    let fx = fixture();
    let component = guest("global_counter");
    let imports = calimero_app_build::imports_of(&component).unwrap();
    assert!(
        imports.iter().any(|i| i == "wasi:cli/stdin@0.2.9"),
        "{imports:?}"
    );
    let app = fx.engine.prepare("gc", &component).expect("accepted");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "bump", b""),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok);
}

#[test]
fn a_component_without_an_abi_is_refused() {
    let fx = fixture();
    let raw = guest("spin");
    let stripped = strip_custom_section(&raw, calimero_app_build::ABI_SECTION);
    assert!(matches!(
        fx.engine.prepare("no-abi", &stripped),
        Err(PrepareError::NoAbi)
    ));
}

#[test]
fn stubbed_imports_refuse() {
    let fx = fixture();
    let app = prepare(&fx, "stubs");
    let run = |method: &str| {
        fx.engine.invoke(
            &app,
            host(InvocationKind::Call, method, b""),
            Budget::call(),
        )
    };

    let out = run("preopens");
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.result, 0u32.to_le_bytes(), "no preopened directories");

    let out = run("open");
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.result, [1], "opening a path fails");

    let out = run("stdin");
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.result, 0u32.to_le_bytes(), "stdin is empty");

    let out = run("terminals");
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.result, [1, 1, 1], "no terminal is present");

    let out = run("env");
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.result, [0, 0, 1], "no variables, arguments or cwd");

    let out = run("exit");
    assert_eq!(trap_kind(&out), Some(TrapKind::Fault), "{:?}", out.status);

    let out = run("preopens");
    assert_eq!(out.status, Status::Ok, "the engine keeps serving");
}

#[test]
fn wall_clock_is_the_entry_timestamp() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "wall", b""),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok);
    let seconds = u64::from_le_bytes(out.result[..8].try_into().unwrap());
    let nanos = u32::from_le_bytes(out.result[8..12].try_into().unwrap());
    assert_eq!(seconds * 1_000 + u64::from(nanos) / 1_000_000, ENTRY_TS_MS);
}

#[test]
fn monotonic_clock_is_fixed_and_sleep_does_not_wait() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    let start = Instant::now();
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "monotonic", b""),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    let before = u64::from_le_bytes(out.result[..8].try_into().unwrap());
    let after = u64::from_le_bytes(out.result[8..16].try_into().unwrap());
    assert_eq!(before, after);
}

#[test]
fn logged_randomness_follows_the_seed() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    let run = || {
        fx.engine.invoke(
            &app,
            host(InvocationKind::Call, "random", b""),
            Budget::call(),
        )
    };
    let (a, b) = (run(), run());
    assert_eq!(a.status, Status::Ok);
    assert_eq!(a.result.len(), 32);
    assert_eq!(a.result, b.result);
}

#[test]
fn stdout_and_stderr_lines_go_to_the_log() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "print", b""),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.logs, ["first line", "second line", "partial"]);
}

#[test]
fn outgoing_http_outside_actions_traps_host_denied() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "fetch", b""),
        Budget::call(),
    );
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::HostDenied),
        "{:?}",
        out.status
    );
}

#[test]
fn a_host_panic_is_a_fault() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    let out = fx.engine.invoke(
        &app,
        host_with(InvocationKind::View, "org", b"", true),
        Budget::view(),
    );
    assert_eq!(trap_kind(&out), Some(TrapKind::Fault), "{:?}", out.status);

    let out = fx
        .engine
        .invoke(&app, host(InvocationKind::View, "org", b""), Budget::view());
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.result, b"acme");
}

#[test]
fn tampered_cache_is_recompiled() {
    let fx = fixture();
    let component = guest("global_counter");
    let _ = fx.engine.prepare("gc", &component).expect("first prepare");

    let artifacts: Vec<PathBuf> = std::fs::read_dir(fx.cache.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "cwasm"))
        .collect();
    assert_eq!(artifacts.len(), 1, "{artifacts:?}");
    let sidecar = artifacts[0].with_extension("cwasm.mac");
    assert!(sidecar.exists(), "{sidecar:?}");

    let mut bytes = std::fs::read(&artifacts[0]).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    std::fs::write(&artifacts[0], &bytes).unwrap();

    let app = fx.engine.prepare("gc", &component).expect("second prepare");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "bump", b""),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok);
    assert_ne!(std::fs::read(&artifacts[0]).unwrap(), bytes, "rewritten");
}

/// The cache files of `component`: its artifact and the files beside it.
fn cache_files(dir: &Path, component: &[u8]) -> Vec<PathBuf> {
    use sha2::Digest;
    let key = hex::encode(sha2::Sha256::digest(component));
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&key) && !n.contains(".tmp-"))
        })
        .collect();
    files.sort();
    files
}

/// C3 (S8, Ruling 38): a party that can write the cache directory rewrites both the
/// artifact and the file beside it, putting another component's compiled code (with its
/// own, valid check file) under this component's name. The node must not load it.
#[test]
fn an_artifact_rewritten_with_its_check_file_is_not_loaded() {
    let fx = fixture();
    let (counter, writer) = (guest("global_counter"), guest("view_writer"));
    let _ = fx.engine.prepare("gc", &counter).expect("prepare gc");
    let _ = fx.engine.prepare("vw", &writer).expect("prepare vw");
    let (from, to) = (
        cache_files(fx.cache.path(), &counter),
        cache_files(fx.cache.path(), &writer),
    );
    assert_eq!(from.len(), 2, "{from:?}");
    assert_eq!(to.len(), 2, "{to:?}");
    for (f, t) in from.iter().zip(&to) {
        std::fs::copy(f, t).unwrap();
    }
    let planted = std::fs::read(&to[0]).unwrap();

    let app = fx.engine.prepare("vw", &writer).expect("prepare vw again");
    let out = fx
        .engine
        .invoke(&app, host(InvocationKind::Call, "put", b""), Budget::call());
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.diff.len(), 1, "ran the planted component's code");
    assert_ne!(
        std::fs::read(&to[0]).unwrap(),
        planted,
        "recompiled and rewritten"
    );
}

/// An artifact authenticated under one key is not loaded under another: a node never
/// runs code compiled (or planted) without its storage key.
#[test]
fn artifacts_are_only_loaded_under_the_key_they_were_written_with() {
    let fx = fixture();
    let component = guest("global_counter");
    let _ = fx.engine.prepare("gc", &component).expect("prepare");
    let files = cache_files(fx.cache.path(), &component);
    let mac = std::fs::read(&files[1]).unwrap();

    let other = WasmtimeEngine::new(fx.cache.path().to_path_buf(), &[6; 32]).expect("engine");
    let _ = other
        .prepare("gc", &component)
        .expect("prepare under another key");
    assert_ne!(
        std::fs::read(&files[1]).unwrap(),
        mac,
        "recompiled and re-signed"
    );
}

#[test]
fn ten_thousand_and_one_writes_trap() {
    let fx = fixture();
    let app = prepare(&fx, "view_writer");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "write_n", &10_001u32.to_le_bytes()),
        Budget::call(),
    );
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::HostDenied),
        "{:?}",
        out.status
    );
    assert!(out.diff.is_empty());

    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "write_n", &10_000u32.to_le_bytes()),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.diff.len(), 10_000);
}

/// C2 (Ruling 37): writes past the run's 1 MiB are refused at the host call, so a guest
/// cannot pile values into host memory until the entry check after the run.
#[test]
fn writes_past_the_run_cap_trap_host_denied_at_the_call() {
    let fx = fixture();
    let app = prepare(&fx, "view_writer");

    let started = Instant::now();
    let out = fx
        .engine
        .invoke(&app, host(InvocationKind::Call, "big", b""), Budget::call());
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::HostDenied),
        "{:?}",
        out.status
    );
    assert!(out.diff.is_empty());

    // 1,024 overwrites of 64 KiB (64 MiB in all) stop at the 16th.
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "overwrite", &1_024u32.to_le_bytes()),
        Budget::call(),
    );
    assert_eq!(
        trap_kind(&out),
        Some(TrapKind::HostDenied),
        "{:?}",
        out.status
    );
    assert!(out.diff.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );

    // Under the cap the same writes succeed: 15 versions of 64 KiB.
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "overwrite", &15u32.to_le_bytes()),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.diff.len(), 1);
}

/// Removes a top-level custom section by name (component header is 8 bytes).
fn strip_custom_section(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut out = bytes[..8].to_vec();
    let mut pos = 8;
    while pos < bytes.len() {
        let id = bytes[pos];
        let (len, used) = leb(&bytes[pos + 1..]);
        let end = pos + 1 + used + len;
        let content = &bytes[pos + 1 + used..end];
        let is_target = id == 0 && {
            let (name_len, name_used) = leb(content);
            &content[name_used..name_used + name_len] == name.as_bytes()
        };
        if !is_target {
            out.extend_from_slice(&bytes[pos..end]);
        }
        pos = end;
    }
    out
}

fn leb(bytes: &[u8]) -> (usize, usize) {
    let mut value = 0usize;
    for (i, byte) in bytes.iter().enumerate() {
        value |= usize::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return (value, i + 1);
        }
    }
    panic!("bad leb")
}

// --- fix round 1 regressions --------------------------------------------------------

#[test]
fn an_undrained_http_body_cannot_wedge_a_node_thread() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    for (kind, budget) in [
        (InvocationKind::Call, Budget::call()),
        (InvocationKind::View, Budget::view()),
    ] {
        let start = Instant::now();
        let out = fx
            .engine
            .invoke(&app, host(kind, "body_write", b""), budget);
        assert_eq!(
            trap_kind(&out),
            Some(TrapKind::HostDenied),
            "{:?}",
            out.status
        );
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
    }
}

#[test]
fn a_store_cannot_hold_unbounded_host_resources() {
    let fx = fixture();
    let app = prepare(&fx, "clock_reader");
    let out = fx.engine.invoke(
        &app,
        host(InvocationKind::Call, "pollables", b""),
        Budget::call(),
    );
    match &out.status {
        Status::Trap {
            kind: TrapKind::HostDenied,
            detail,
        } => assert!(detail.contains("resource table"), "{detail}"),
        other => panic!("expected HostDenied, got {other:?}"),
    }
}

#[test]
fn an_extra_memory_is_refused_at_prepare() {
    let fx = fixture();
    let one_module_two_memories =
        "(component (core module $m (memory 1) (memory 1)) (core instance (instantiate $m)))";
    let two_modules_one_memory_each = "(component
        (core module $a (memory 1)) (core module $b (memory 1))
        (core instance (instantiate $a)) (core instance (instantiate $b)))";
    for text in [one_module_two_memories, two_modules_one_memory_each] {
        let component = with_abi(wat::parse_str(text).expect("wat"));
        match fx.engine.prepare("mm", &component) {
            Err(PrepareError::Compile(reason)) => {
                assert!(reason.contains("2 memories"), "{reason}");
            }
            Err(other) => panic!("expected a compile refusal, got {other}"),
            Ok(_) => panic!("expected a compile refusal, got a prepared app"),
        }
    }
}

#[test]
fn an_exhausted_instance_pool_is_a_retryable_timeout() {
    let cache = tempfile::tempdir().expect("tempdir");
    let engine =
        WasmtimeEngine::with_pool_size(cache.path().to_path_buf(), &CACHE_KEY, 1).expect("engine");
    let spin = engine.prepare("spin", &guest("spin")).expect("spin");
    let counter = engine
        .prepare("gc", &guest("global_counter"))
        .expect("counter");
    let holding = Budget {
        fuel: u64::MAX,
        wall_clock: Duration::from_millis(1_500),
        ..Budget::call()
    };
    std::thread::scope(|scope| {
        let busy =
            scope.spawn(|| engine.invoke(&spin, host(InvocationKind::Call, "spin", b""), holding));
        std::thread::sleep(Duration::from_millis(300));
        let out = engine.invoke(
            &counter,
            host(InvocationKind::Call, "bump", b""),
            Budget::call(),
        );
        assert_eq!(
            out.status,
            Status::Trap {
                kind: TrapKind::Timeout,
                detail: "instance pool exhausted".to_owned(),
            }
        );
        let busy = busy.join().expect("busy thread");
        assert_eq!(trap_kind(&busy), Some(TrapKind::Timeout));
    });
    let out = engine.invoke(
        &counter,
        host(InvocationKind::Call, "bump", b""),
        Budget::call(),
    );
    assert_eq!(out.status, Status::Ok, "the slot is free again");
}

/// Appends a top-level `calimero:abi` custom section with a minimal valid ABI.
fn with_abi(mut component: Vec<u8>) -> Vec<u8> {
    let name = calimero_app_build::ABI_SECTION.as_bytes();
    let data = br#"{"abiVersion":1,"methods":{}}"#;
    let mut content = leb_bytes(name.len());
    content.extend_from_slice(name);
    content.extend_from_slice(data);
    component.push(0);
    component.extend(leb_bytes(content.len()));
    component.extend(content);
    component
}

fn leb_bytes(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}
