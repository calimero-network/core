// `--version` names the release (`[workspace.metadata.workspaces].version`),
// not `CARGO_PKG_VERSION`: that is the workspace placeholder `0.0.0`.

fn main() {
    let version = calimero_build_utils::read_workspace_version()
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").expect("set by Cargo"));
    println!("cargo:rustc-env=CALIMERO_RELEASE_VERSION={version}");
}
