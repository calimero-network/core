//! The component this crate builds, and the org a scenario names in its `.org.json`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{Org, TestApp};
use serde_json::Value;

/// The harness's clock at `init`.
pub const T0: u64 = 1_700_000_000_000;

/// This crate as a `calimero:app` component, built once per test process.
pub fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        // One nested target dir for every standalone app's guest build.
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("component-guests");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build {}: {e}", dir.display()))
            .component
    })
}

/// The app in an org with these accounts besides the owner.
pub fn app(org: Org) -> TestApp {
    TestApp::new(component(), org, T0)
}

/// The scenario files under `scenarios/` (not the `.org.json` beside them), sorted.
pub fn scenarios() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && !p.to_string_lossy().ends_with(".org.json")
        })
        .collect();
    files.sort();
    files
}

/// The org of `<name>.json`: `<name>.org.json` beside it, else the owner alone.
///
/// `{ "admins": [..], "members": [..], "guests": [..], "teams": { "<team>": [..] } }`.
pub fn org_of(scenario: &Path) -> Org {
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
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
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
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
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
