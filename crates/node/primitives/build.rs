// Expose workspace release version for runtime version checks (e.g. bundle min_runtime_version).
// See docs/RELEASE.md. Uses calimero-build-utils so the version matches the single source of truth.

fn main() {
    let version = calimero_build_utils::read_workspace_version()
        .expect("failed to read [workspace.metadata.workspaces].version from workspace Cargo.toml");
    println!("cargo:rustc-env=CALIMERO_RELEASE_VERSION={}", version);
}
