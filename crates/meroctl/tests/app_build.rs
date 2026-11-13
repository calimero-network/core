//! `meroctl app build` through the binary (what `mero-app-build`'s CLI test held):
//! the compute component lands beside the app component, and asking for one from a
//! crate without compute methods fails.

use std::path::{Path, PathBuf};
use std::process::Command;

fn target() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("app-build-tests")
}

fn build(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .args(["app", "build"])
        .args(args)
        .output()
        .expect("run meroctl app build")
}

#[test]
fn build_writes_the_compute_component_beside_the_app_component() {
    let out = tempfile::tempdir().expect("tempdir");
    let app = out.path().join("fixture.wasm");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = root.join("apps/v2-fixtures/compute");
    let target = target();
    let run = build(&[
        fixture.as_os_str(),
        "--target-dir".as_ref(),
        target.as_os_str(),
        "--out".as_ref(),
        app.as_os_str(),
    ]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(app.exists());
    let compute = std::fs::read(out.path().join("compute.component.wasm")).expect("compute out");
    let abi = calimero_app_build::abi_of(&compute)
        .unwrap()
        .expect("a compute ABI");
    assert_eq!(abi["world"], calimero_app_wit::COMPUTE_WORLD);

    let echo = root.join("crates/app-build/tests/guests/echo");
    let echo_out = out.path().join("echo.wasm");
    let echo_compute = out.path().join("echo-compute.wasm");
    let refused = build(&[
        echo.as_os_str(),
        "--target-dir".as_ref(),
        target.as_os_str(),
        "--out".as_ref(),
        echo_out.as_os_str(),
        "--compute-out".as_ref(),
        echo_compute.as_os_str(),
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("has no compute methods"));
}
