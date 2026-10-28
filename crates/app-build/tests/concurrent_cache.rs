//! Concurrent first builds across processes (Ruling 46): nextest runs each test in its own
//! process, so on a cold cache many processes build the same guest at once. They must build
//! once and all succeed.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use calimero_app_build::build_component_cached;

const CHILD_CACHE: &str = "APP_BUILD_TEST_CHILD_CACHE";
const PROCESSES: usize = 6;

/// In a child process: one cached build of the echo guest. In the test process: `PROCESSES`
/// children at once on a cold cache, each running `cargo` through a wrapper that logs every
/// invocation.
#[test]
fn concurrent_processes_on_a_cold_cache_build_once() {
    let guest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/echo");
    if let Ok(cache) = std::env::var(CHILD_CACHE) {
        let component = build_component_cached(&guest, &[], Path::new(&cache))
            .unwrap_or_else(|e| panic!("child build: {e}"));
        assert!(!component.is_empty());
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");
    let bin = tmp.path().join("bin");
    let log = tmp.path().join("cargo-builds.log");
    std::fs::create_dir_all(&bin).unwrap();
    // The wrapper logs, then runs the `cargo` the test was given (the rustup proxy, which
    // sets up the toolchain the linker needs) from the original PATH.
    let original = std::env::var("PATH").unwrap_or_default();
    let wrapper = bin.join("cargo");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nPATH='{original}' exec cargo \"$@\"\n",
            log.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{original}", bin.display());

    let children: Vec<_> = (0..PROCESSES)
        .map(|_| {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "concurrent_processes_on_a_cold_cache_build_once",
                    "--nocapture",
                ])
                .env(CHILD_CACHE, &cache)
                .env("PATH", &path)
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success(), "a child build failed");
    }

    let builds = std::fs::read_to_string(&log).unwrap_or_default();
    let builds: Vec<&str> = builds.lines().filter(|l| l.starts_with("build")).collect();
    assert_eq!(
        builds.len(),
        1,
        "cargo build ran {} times: {builds:?}",
        builds.len()
    );
    let cached: Vec<_> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".wasm"))
        .collect();
    assert_eq!(cached.len(), 1, "{cached:?}");
}
