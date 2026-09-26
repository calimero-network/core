// Bakes the SDK version `cargo mero new` scaffolds against from the workspace's
// own release version, so the scaffolded default cannot drift from what a
// released cargo-mero binary was actually built and tagged with.

fn main() {
    let version = calimero_build_utils::read_workspace_version().expect(
        "failed to read [workspace.metadata.workspaces].version from the workspace root Cargo.toml",
    );
    println!("cargo:rustc-env=CALIMERO_SDK_DEFAULT_VERSION={version}");
}
