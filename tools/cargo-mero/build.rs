// Bakes in the workspace release version; a crates.io build has no workspace root, but
// there `cargo ws publish` has already rewritten CARGO_PKG_VERSION to that same version.

fn main() {
    let version = calimero_build_utils::read_workspace_version()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    println!("cargo:rustc-env=CALIMERO_SDK_DEFAULT_VERSION={version}");
}
