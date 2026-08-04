//! PNG icon encoding for `manifest.json`'s `metadata.icon`: a `data:` URI the
//! desktop decodes directly, with no separate asset fetch.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_bundle::MAX_MANIFEST_BYTES;
use camino::Utf8Path;
use eyre::{ensure, eyre, Result};

use crate::meta::BundleMeta;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Budget for the base64 data URI: base64 inflates the source PNG ~4/3, and the
/// manifest carries fields besides the icon, so this leaves headroom under the cap.
const MAX_ENCODED: usize = MAX_MANIFEST_BYTES as usize * 3 / 4;
/// Sentinel `icon = "default"` value selecting the bundled Calimero mark.
const DEFAULT_SENTINEL: &str = "default";
/// Only ever used when asked for by name: the desktop already falls back to the
/// app's own icon at its `frontend` URL, and a generic mark would outrank it.
const DEFAULT_MARK: &[u8] = include_bytes!("../assets/default-icon.png");

/// Encode a PNG file as a `data:image/png;base64,...` URI: standard alphabet
/// and unwrapped, since a wrapped URI breaks the surrounding JSON string.
pub fn encode(path: &Utf8Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| eyre!("failed to read icon {path}: {e}"))?;
    ensure!(bytes.starts_with(PNG_MAGIC), "icon must be a PNG: {path}");
    let uri = format!("data:image/png;base64,{}", STANDARD.encode(&bytes));
    ensure!(
        uri.len() <= MAX_ENCODED,
        "icon is too large: {} KiB encoded, limit is {} KiB",
        uri.len() / 1024,
        MAX_ENCODED / 1024
    );
    Ok(uri)
}

/// The three-way icon policy: explicit path, bundled default, or explicit
/// opt-out; errors when none of the three is chosen.
pub fn resolve(meta: &BundleMeta, no_icon: bool) -> Result<Option<String>> {
    if no_icon {
        return Ok(None);
    }
    match meta.icon.as_deref() {
        Some(DEFAULT_SENTINEL) => bundled_default(),
        Some(rel) => encode(&meta.manifest_dir.join(rel)).map(Some),
        None => Err(eyre!(
            "no icon configured for {}\n\
             \n  set one:      icon = \"path/to/icon-512x512.png\"   in [package.metadata.calimero]\
             \n  use ours:     icon = \"default\"\
             \n  ship without: --no-icon   (the desktop will look for one at your `frontend` URL)",
            meta.package
        )),
    }
}

/// The mark is compiled in, so this cannot fail and needs no filesystem access.
fn bundled_default() -> Result<Option<String>> {
    Ok(Some(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(DEFAULT_MARK)
    )))
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;

    #[test]
    fn encodes_a_png_as_a_standard_base64_data_uri() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = Utf8PathBuf::from_path_buf(dir.path().join("i.png")).expect("utf8");
        // Smallest valid PNG header plus a byte; only the magic is validated.
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n\x00").expect("write");

        let uri = encode(&path).expect("encode");
        assert!(uri.starts_with("data:image/png;base64,"));
        assert!(
            !uri.contains('\n'),
            "a wrapped data URI breaks the JSON string"
        );
        // Standard alphabet, not URL-safe: the desktop decoder expects standard.
        assert!(!uri.contains('-') && !uri.contains('_'));
    }

    #[test]
    fn rejects_a_file_that_is_not_a_png() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = Utf8PathBuf::from_path_buf(dir.path().join("i.png")).expect("utf8");
        std::fs::write(&path, b"GIF89a").expect("write");
        assert!(encode(&path).is_err());
    }

    #[test]
    fn rejects_an_icon_that_would_overflow_the_manifest_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = Utf8PathBuf::from_path_buf(dir.path().join("i.png")).expect("utf8");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(900_000, 0);
        std::fs::write(&path, &bytes).expect("write");
        assert!(encode(&path).is_err());
    }

    #[test]
    fn the_bundled_default_is_a_png_that_fits_the_manifest() {
        let uri = bundled_default().expect("encodes").expect("some");
        assert!(uri.starts_with("data:image/png;base64,"));
        assert!(
            !uri.contains('\n'),
            "a wrapped data URI breaks the JSON string"
        );
        assert!(
            uri.len() <= MAX_ENCODED,
            "bundled mark exceeds the manifest budget"
        );
        assert!(
            DEFAULT_MARK.starts_with(PNG_MAGIC),
            "bundled mark must be a PNG"
        );
    }

    fn test_meta(icon: Option<&str>, manifest_dir: &Utf8Path) -> BundleMeta {
        BundleMeta {
            package: "com.example.app".to_owned(),
            name: None,
            description: None,
            author: None,
            icon: icon.map(str::to_owned),
            slug: None,
            license: None,
            tags: Vec::new(),
            github: None,
            docs: None,
            min_runtime_version: "0.1.0".to_owned(),
            frontend: None,
            app_version: "0.1.0".to_owned(),
            services: Vec::new(),
            manifest_dir: manifest_dir.to_owned(),
        }
    }

    #[test]
    fn no_icon_flag_wins_over_a_configured_icon_and_never_touches_disk() {
        // A directory that does not exist: resolve() must not read it, since
        // --no-icon short-circuits before `icon` is even consulted.
        let meta = test_meta(Some("icon.png"), Utf8Path::new("/no/such/manifest/dir"));
        assert_eq!(resolve(&meta, true).expect("no-icon"), None);
    }

    #[test]
    fn a_relative_icon_path_resolves_against_manifest_dir_not_the_process_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest_dir = Utf8PathBuf::from_path_buf(dir.path().to_owned()).expect("utf8");
        std::fs::create_dir_all(manifest_dir.join("assets")).expect("mkdir");
        std::fs::write(
            manifest_dir.join("assets/icon-test.png"),
            b"\x89PNG\r\n\x1a\n\x00",
        )
        .expect("write");

        // "assets/icon-test.png" does not exist relative to the crate's cwd, so
        // this only passes if resolve() joins against `manifest_dir`.
        let meta = test_meta(Some("assets/icon-test.png"), &manifest_dir);
        let uri = resolve(&meta, false).expect("resolve").expect("some icon");
        assert!(uri.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn the_default_sentinel_uses_the_bundled_mark_not_the_manifest_dir() {
        // The sentinel is not a path, so it must resolve without touching disk.
        let meta = test_meta(Some("default"), Utf8Path::new("/no/such/manifest/dir"));
        let uri = resolve(&meta, false).expect("resolve").expect("some icon");
        assert_eq!(uri, bundled_default().expect("encodes").expect("some"));
    }

    #[test]
    fn neither_icon_nor_no_icon_names_all_three_routes() {
        let meta = test_meta(None, Utf8Path::new("/no/such/manifest/dir"));
        let err = resolve(&meta, false)
            .expect_err("no route chosen")
            .to_string();
        assert!(err.contains("icon = \"path/to/icon-512x512.png\""));
        assert!(err.contains("icon = \"default\""));
        assert!(err.contains("--no-icon"));
    }
}
