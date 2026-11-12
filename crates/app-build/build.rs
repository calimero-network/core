// `--version` names the release (`[workspace.metadata.workspaces].version`),
// not `CARGO_PKG_VERSION`: that is the workspace placeholder `0.0.0`. Beside it,
// the source the binary was built from: `git describe` and the commit, so a
// build from a branch or an untagged commit says so (`unknown` outside git).

fn main() {
    let version = calimero_build_utils::read_workspace_version()
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").expect("set by Cargo"));
    println!("cargo:rustc-env=CALIMERO_RELEASE_VERSION={version}");
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by Cargo");
    let git = calimero_build_utils::git_details_or_unknown(&dir);
    println!(
        "cargo:rustc-env=CALIMERO_BUILD_DESCRIBE={}",
        git.describe.trim()
    );
    println!(
        "cargo:rustc-env=CALIMERO_BUILD_COMMIT={}",
        git.commit.trim()
    );
}
