//! A host that cannot reserve 4 GiB per pool slot still runs apps. Its own test binary:
//! the address-space limit it sets holds for the whole process.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use calimero_app_build::build_in_repo_cache;
use calimero_app_engine_wasmtime::{WasmtimeEngine, SLOT_RESERVATION_BYTES};

use calimero_app_host::{
    Budget, Caller, Directory, Engine, HostCore, Invocation, InvocationKind, OrgRole, Status,
};

/// Well under the 256 slots x (4 GiB + guard) the pool asks for, well over what the
/// bounds-checked pool (256 x (64 MiB + guard)), the build and the test runner need.
const ADDRESS_SPACE_LIMIT: u64 = 256 << 30;

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

fn host(method: &str) -> HostCore {
    let inv = Invocation {
        kind: InvocationKind::Call,
        method: method.to_owned(),
        args: Vec::new(),
        caller: Caller::Account("alice".to_owned()),
        entry_ts_ms: 1_700_000_000_123,
        id_term: 1,
        id_seq: 1,
        seed: [7; 32],
        timer: None,
        compute: None,
    };
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    HostCore::new("app-1", Arc::new(state), 1, inv, Arc::new(NoDirectory)).unwrap()
}

#[test]
fn an_engine_without_the_address_space_falls_back_to_bounds_checked_slots() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/crunch");
    let component = build_in_repo_cache(&dir).unwrap().app.component;

    // No engine with the full pool may exist first: its epoch thread keeps the pool's
    // address space mapped after the engine is dropped.
    let limit = libc::rlimit {
        rlim_cur: ADDRESS_SPACE_LIMIT,
        rlim_max: ADDRESS_SPACE_LIMIT,
    };
    // SAFETY: setrlimit reads the struct it is given; lowering the limit is always allowed.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) }, 0);

    let cache = tempfile::tempdir().unwrap();
    let engine = WasmtimeEngine::new(cache.path().to_path_buf(), &[5; 32]).unwrap();
    assert_ne!(engine.memory_reservation(), SLOT_RESERVATION_BYTES);
    assert_eq!(engine.memory_reservation(), 64 << 20);
    let app = engine.prepare("crunch", &component).unwrap();
    let out = engine.invoke(
        &app,
        host("sieve"),
        Budget {
            fuel: 50_000_000_000,
            ..Budget::call()
        },
    );
    assert!(matches!(out.status, Status::Ok), "{:?}", out.status);
    assert_eq!(out.result, 564_163_u64.to_le_bytes());
}
