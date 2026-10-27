//! Test support: the v2 cluster fixtures (`apps/v2-fixtures/*`) built into components.

use std::path::{Path, PathBuf};

use calimero_app_build::build_component_cached;

/// The fixture `name` ("kv" or "org-gated") as a component, built on first use and cached
/// by source hash under `target/app-components` (the fixture, the SDK crates, the toolchain and
/// app-build's post-processing are hashed; see `calimero_app_build::cache_key`).
pub fn component(name: &str) -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("apps/v2-fixtures").join(name);
    let sdk = root.join("crates/app-sdk");
    let macros = root.join("crates/app-sdk-macros");
    let wit = root.join("crates/app-wit");
    let cache: PathBuf = root.join("target/app-components");
    build_component_cached(&dir, &[&sdk, &macros, &wit], &cache)
        .unwrap_or_else(|e| panic!("build fixture {name}: {e}"))
}
