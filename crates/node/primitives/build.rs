// Expose release version for runtime version checks (e.g. bundle min_runtime_version).
// Prefer workspace metadata when building in-repo, but fall back to package version
// when building from a published crate tarball.

use std::path::{Path, PathBuf};

fn main() {
    let version = read_workspace_version().unwrap_or_else(|| {
        let pkg_version = std::env::var("CARGO_PKG_VERSION")
            .expect("CARGO_PKG_VERSION is not set by Cargo for build script");
        println!(
            "cargo:warning=unable to resolve workspace metadata version; \
             using CARGO_PKG_VERSION={pkg_version}"
        );
        pkg_version
    });

    println!("cargo:rustc-env=CALIMERO_RELEASE_VERSION={version}");
}

fn read_workspace_version() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let (version, version_file) = read_workspace_version_for_dir(Path::new(&manifest_dir))?;

    // Re-run build script when workspace version changes.
    println!("cargo:rerun-if-changed={}", version_file.display());
    Some(version)
}

fn read_workspace_version_for_dir(manifest_dir: &Path) -> Option<(String, PathBuf)> {
    let mut dir = manifest_dir.to_path_buf();

    loop {
        let cargo_toml = dir.join("Cargo.toml");

        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                if let Some(version) = parse_workspace_metadata_version(&content) {
                    return Some((version, cargo_toml));
                }
            }
        }

        if !dir.pop() {
            break;
        }
    }

    None
}

fn parse_workspace_metadata_version(content: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(content).ok()?;
    let version = value
        .get("workspace")?
        .get("metadata")?
        .get("workspaces")?
        .get("version")?
        .as_str()?
        .trim();

    (!version.is_empty()).then(|| version.to_string())
}
