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
    let engine = WasmtimeEngine::new(cache.path().to_path_buf()).expect("engine");
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
    let sidecar = artifacts[0].with_extension("cwasm.sha256");
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
