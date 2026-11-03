//! The cluster tests' compute fixture on the real engine: its result method takes the outcome
//! shape the node hands it and keeps the outputs by `BlobRef`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use calimero_app_build::build_component;
use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    blob_refs, Budget, Caller, Directory, Engine, HostCore, Invocation, InvocationKind,
    MemoryFiles, OrgRole, Status,
};
use serde_json::json;

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

fn on_done(
    engine: &WasmtimeEngine,
    files: &MemoryFiles,
    outcome: &serde_json::Value,
) -> calimero_app_host::Outcome {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/v2-fixtures/compute");
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("compute-fixture");
    let component = build_component(&dir, &target)
        .unwrap_or_else(|e| panic!("build compute fixture: {e}"))
        .component;
    let app = engine.prepare("compute-hash", &component).expect("prepare");
    let inv = Invocation {
        kind: InvocationKind::Call,
        method: "onDone".to_owned(),
        args: serde_json::to_vec(&json!({ "outcome": outcome })).unwrap(),
        caller: Caller::System,
        entry_ts_ms: 1_700_000_000_000,
        id_term: 1,
        id_seq: 1,
        seed: [7; 32],
        timer: None,
        compute: None,
    };
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let host = HostCore::new("app", Arc::new(state), 1, inv, Arc::new(NoDirectory))
        .expect("valid invocation")
        .with_files(Arc::new(files.clone()));
    engine.invoke(&app, host, Budget::call())
}

#[test]
fn the_result_method_keeps_the_outputs_of_the_outcome_it_is_given() {
    let cache = tempfile::tempdir().expect("tempdir");
    let engine = WasmtimeEngine::new(cache.path().to_path_buf(), &[5; 32]).expect("engine");
    let files = MemoryFiles::default();
    let output = files.insert(b"sealed output");
    let done = json!({
        "run": "r1", "method": "digest", "attempt": 1, "status": "done", "reason": null,
        "result": null,
        "outputs": [{ "id": output, "size": "13", "sha256": "ab".repeat(32) }],
        "usage": { "cpuSeconds": 1.5, "gibSeconds": 0.25, "fuel": "99" },
    });
    let out = on_done(&engine, &files, &done);
    assert_eq!(out.status, Status::Ok, "{:?}", out.status);
    let stored: Vec<[u8; 32]> = out
        .diff
        .iter()
        .filter_map(|w| w.value.as_deref())
        .flat_map(blob_refs)
        .collect();
    assert_eq!(stored.len(), 1, "the output is stored as a BlobRef");
    assert_eq!(hex::encode(stored[0]), output);

    let lost = json!({
        "run": "r2", "method": "digest", "attempt": 3, "status": "failed", "reason": "lost",
        "result": null, "outputs": [],
        "usage": { "cpuSeconds": 0.0, "gibSeconds": 0.0, "fuel": "0" },
    });
    let out = on_done(&engine, &files, &lost);
    assert_eq!(out.status, Status::Ok, "{:?}", out.status);
    assert!(out
        .diff
        .iter()
        .filter_map(|w| w.value.as_deref())
        .all(|v| blob_refs(v).is_empty()));
}
