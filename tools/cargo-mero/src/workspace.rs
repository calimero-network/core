//! Cargo workspace plumbing: running `cargo metadata` and locating packages by
//! directory. The `[metadata.calimero]` schema itself lives in `meta`.

use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::{Metadata, MetadataCommand, Package};
use eyre::{Context, Result};

/// Run `cargo metadata`, scoped to `manifest_path` when given.
pub fn metadata_for(manifest_path: Option<&Utf8Path>) -> Result<Metadata> {
    let mut cmd = MetadataCommand::new();
    if let Some(path) = manifest_path {
        let _ = cmd.manifest_path(path);
    }
    cmd.exec().wrap_err("failed to run `cargo metadata`")
}

/// The directory holding `manifest_path`, canonicalized so a relative or
/// `..`-laden path still matches cargo's absolute package paths.
pub fn manifest_dir(manifest_path: &Utf8Path) -> Utf8PathBuf {
    let parent = match manifest_path.parent() {
        Some(dir) if !dir.as_str().is_empty() => dir,
        _ => Utf8Path::new("."),
    };
    canonical(parent)
}

/// Whether a package manifest lives directly in `dir`. Both sides are
/// canonicalized: cargo keeps symlinked components (macOS temp dirs sit under
/// `/var`, a symlink to `/private/var`), so a raw compare misses its own package.
pub fn same_dir(pkg_manifest_path: &Utf8Path, dir: &Utf8Path) -> bool {
    pkg_manifest_path
        .parent()
        .is_some_and(|pkg_dir| canonical(pkg_dir) == canonical(dir))
}

/// The workspace package whose manifest lives directly in `dir`, if any.
pub fn package_in_dir<'a>(metadata: &'a Metadata, dir: &Utf8Path) -> Option<&'a Package> {
    metadata
        .packages
        .iter()
        .find(|p| same_dir(&p.manifest_path, dir))
}

fn canonical(dir: &Utf8Path) -> Utf8PathBuf {
    dir.canonicalize_utf8().unwrap_or_else(|_| dir.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_dir_resolves_a_noncanonical_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        std::fs::create_dir(dir.join("app")).unwrap();
        std::fs::write(dir.join("app/Cargo.toml"), "").unwrap();

        let messy = dir.join("app/../app/./Cargo.toml");
        assert_eq!(
            manifest_dir(&messy),
            dir.join("app").canonicalize_utf8().unwrap()
        );
        assert!(manifest_dir(&messy).is_absolute());
    }

    #[cfg(unix)]
    #[test]
    fn same_dir_matches_through_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        std::fs::create_dir(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/Cargo.toml"), "").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link").as_std_path()).unwrap();

        assert!(same_dir(&dir.join("link/Cargo.toml"), &dir.join("real")));
        assert!(same_dir(&dir.join("real/Cargo.toml"), &dir.join("link")));
        assert!(!same_dir(&dir.join("real/Cargo.toml"), dir));
    }
}
