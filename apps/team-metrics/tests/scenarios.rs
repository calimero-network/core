//! Every `scenarios/*.json` on the real engine, each in the org its `.org.json` names.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::run_scenario;

#[test]
fn every_scenario_passes() {
    let files = common::scenarios();
    assert!(!files.is_empty(), "no scenarios");
    for path in files {
        let mut t = common::app(common::org_of(&path));
        if let Err(e) = run_scenario(&mut t, &path) {
            panic!("{}: {e:#}", path.display());
        }
    }
}
