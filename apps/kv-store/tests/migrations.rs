//! Every shipped release's state, upgraded to this build: `migrations/<version>.json` runs
//! on `releases/<version>.mpk` (the signed bundle that release shipped) until its `upgrade`
//! step, then on this crate's component, in the org its `.org.json` names.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::Path;

use calimero_sdk::testing::{release_component, run_migration};

#[test]
fn every_release_upgrades_to_this_build() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = common::scenarios_in(&dir.join("migrations"));
    assert!(!files.is_empty(), "no migration scenarios");
    for path in files {
        let version = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let mpk = dir.join("releases").join(format!("{version}.mpk"));
        let mpk = std::fs::read(&mpk).unwrap_or_else(|e| panic!("{}: {e}", mpk.display()));
        let old = release_component(&mpk, None).unwrap_or_else(|e| panic!("{version}: {e:#}"));
        let org = common::org_of(&path);
        if let Err(e) = run_migration(&old, common::component(), org, common::T0, &path) {
            panic!("{}: {e:#}", path.display());
        }
    }
}
