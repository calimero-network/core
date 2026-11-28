//! Builds every guest the workspace's tests build, into the repository's component cache
//! (`calimero_app_build::build_in_repo_cache`), so no test pays for a compile on its own clock.
//!
//! CI runs it before the tests (`cargo run -p calimero-app-build --example build-test-apps`);
//! a test that finds its guest cached only reads it. A guest the list misses is still built by
//! the first test that asks, just slower.

use std::path::{Path, PathBuf};
use std::time::Instant;

/// The directories whose every crate a test builds, relative to the repository root.
const GUEST_DIRS: &[&str] = &[
    "crates/app-engine-wasmtime/tests/guests",
    "crates/sdk/tests/guests",
];

/// Single guests the tests build, relative to the repository root.
const GUESTS: &[&str] = &[
    "apps/fixture-captcha",
    "apps/fixture-compute",
    "apps/fixture-files",
    "apps/fixture-kv",
    "apps/fixture-notes-v1",
    "apps/fixture-notes-v2",
    "apps/fixture-sandbox",
    "apps/rooms",
    "apps/team-space",
];

/// Guests that compile a crate outside the SDK by path, with that crate: its edits key the cache.
const GUESTS_WITH_PATH_DEPS: &[(&str, &[&str])] = &[(
    "sdks/plugins/connectors/nearai/tests/guest",
    &["sdks/plugins/connectors/nearai"],
)];

fn main() -> eyre::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut guests: Vec<(PathBuf, Vec<PathBuf>)> =
        GUESTS.iter().map(|g| (root.join(g), Vec::new())).collect();
    for dir in GUEST_DIRS {
        for entry in std::fs::read_dir(root.join(dir))? {
            let path = entry?.path();
            if path.join("Cargo.toml").is_file() {
                guests.push((path, Vec::new()));
            }
        }
    }
    for (guest, deps) in GUESTS_WITH_PATH_DEPS {
        guests.push((
            root.join(guest),
            deps.iter().map(|d| root.join(d)).collect(),
        ));
    }
    guests.sort();
    let all = Instant::now();
    for (guest, deps) in &guests {
        let one = Instant::now();
        let deps: Vec<&Path> = deps.iter().map(PathBuf::as_path).collect();
        let built = calimero_app_build::build_in_repo_cache_with(guest, &deps)
            .map_err(|e| eyre::eyre!("{}: {e}", guest.display()))?;
        println!(
            "{:>6.1}s  {}{}",
            one.elapsed().as_secs_f64(),
            guest.strip_prefix(&root).unwrap_or(guest).display(),
            match (&built.compute, &built.sandbox) {
                (Some(_), _) => " (+ compute)",
                (None, Some(_)) => " (+ sandbox)",
                (None, None) => "",
            },
        );
    }
    println!(
        "{} guests in {:.1}s",
        guests.len(),
        all.elapsed().as_secs_f64()
    );
    Ok(())
}
