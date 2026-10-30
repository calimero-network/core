//! Every `scenarios/*.json` on the real engine, each with the org its `.org.json` names
//! (no file: the owner only). The cluster runs the same files (`docs/.../contribute/testing.mdx`).

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{run_scenario, Org, TestApp};
use serde_json::Value;

const T0: u64 = 1_700_000_000_000;

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        // The test's own target dir (`<target>/tmp`'s parent), so the guest shares its deps.
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("..");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build authored-sorted-kv-store: {e}"))
            .component
    })
}

/// The org a scenario's `<name>.org.json` describes:
/// `{ "admins": [..], "members": [..], "guests": [..], "teams": { "<team>": [..] } }`.
fn org_of(scenario: &Path) -> Org {
    let path = scenario.with_extension("org.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Org::new();
    };
    let spec: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let names = |key: &str| -> Vec<String> {
        spec.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let teams: Vec<(String, Vec<String>)> = spec
        .get("teams")
        .and_then(Value::as_object)
        .map(|t| {
            t.iter()
                .map(|(team, accounts)| {
                    let accounts = accounts
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    (team.clone(), accounts)
                })
                .collect()
        })
        .unwrap_or_default();
    Org::new()
        .admins(names("admins"))
        .members(names("members"))
        .guests(names("guests"))
        .teams(teams)
}

#[test]
fn every_scenario_passes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|x| x == "json")
                && !p.to_string_lossy().ends_with(".org.json")
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no scenarios in {}", dir.display());
    for file in files {
        let mut t = TestApp::new(component(), org_of(&file), T0);
        if let Err(e) = run_scenario(&mut t, &file) {
            panic!("{}: {e:#}", file.display());
        }
    }
}
