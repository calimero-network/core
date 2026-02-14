// Set version and build metadata env vars for the binary (NEAR-style).
// See https://github.com/near/nearcore/blob/master/neard/src/main.rs

fn main() {
    let pkg_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by Cargo");
    let version = calimero_build_utils::read_workspace_version()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let (build, commit) =
        calimero_build_utils::git_details(&pkg_dir).unwrap_or(("unknown".into(), "unknown".into()));
    let rustc_version = rustc_version::version()
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "unknown".into());

    println!("cargo:rustc-env=MEROD_VERSION={}", version);
    println!("cargo:rustc-env=MEROD_BUILD={}", build.trim());
    println!("cargo:rustc-env=MEROD_COMMIT={}", commit.trim());
    println!("cargo:rustc-env=MEROD_RUSTC_VERSION={}", rustc_version);
}
