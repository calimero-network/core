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
    "apps/v2-fixtures",
];

/// Single guests the tests build, relative to the repository root.
const GUESTS: &[&str] = &["apps/rooms"];

fn main() -> eyre::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut guests: Vec<PathBuf> = GUESTS.iter().map(|g| root.join(g)).collect();
    for dir in GUEST_DIRS {
        for entry in std::fs::read_dir(root.join(dir))? {
            let path = entry?.path();
            if path.join("Cargo.toml").is_file() {
                guests.push(path);
            }
        }
    }
    guests.sort();
    let all = Instant::now();
    for guest in &guests {
        let one = Instant::now();
        let built = calimero_app_build::build_in_repo_cache(guest)
            .map_err(|e| eyre::eyre!("{}: {e}", guest.display()))?;
        println!(
            "{:>6.1}s  {}{}",
            one.elapsed().as_secs_f64(),
            guest.strip_prefix(&root).unwrap_or(guest).display(),
            if built.compute.is_some() {
                " (+ compute)"
            } else {
                ""
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
