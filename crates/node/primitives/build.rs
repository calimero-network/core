// Expose workspace release version for runtime version checks (e.g. bundle min_runtime_version).
// See docs/RELEASE.md. Uses calimero-build-utils so the version matches the single source of truth.

fn main() {
    let version = calimero_build_utils::read_workspace_version()
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));
    println!("cargo:rustc-env=CALIMERO_RELEASE_VERSION={}", version);
}
