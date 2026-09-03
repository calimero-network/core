//! Cached download and extraction of zip archives for build scripts.

use std::fmt::Write as _;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use eyre::{bail, Context, Result};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

const CACHE_KEY_BYTES: usize = 16; // truncated sha256, wide enough that two sources never collide

/// Fetch a zip archive and return the directory it was extracted into.
///
/// `src` is an `http(s)` URL or an absolute path to a local zip. Extractions live
/// under `cache_dir` keyed by `src`, and are reused while younger than `freshness`.
pub fn fetch_and_extract(
    client: &Client,
    src: &str,
    cache_dir: &Path,
    freshness: Duration,
    force: bool,
) -> Result<PathBuf> {
    let dest = cache_dir.join(cache_key(src));

    if !force && is_fresh(&dest, freshness) {
        return Ok(dest);
    }

    let archive = read_archive(client, src)?;

    fs::create_dir_all(cache_dir).wrap_err_with(|| {
        format!(
            "failed to create the cache directory {}",
            cache_dir.display()
        )
    })?;

    // Extract aside and rename in, so an interrupted build never leaves a
    // half-written entry that the next one would treat as a cache hit.
    let staging = tempfile::tempdir_in(cache_dir)?;

    ZipArchive::new(Cursor::new(archive))
        .and_then(|mut archive| archive.extract(staging.path()))
        .wrap_err_with(|| format!("failed to extract the archive from {src}"))?;

    let staged = staging.keep();

    let _ignored = fs::remove_dir_all(&dest);

    if fs::rename(&staged, &dest).is_err() {
        let _ignored = fs::remove_dir_all(&staged);

        // A parallel build may have renamed its own extraction in first.
        if !dest.is_dir() {
            bail!(
                "failed to move the extracted archive into {}",
                dest.display()
            );
        }
    }

    Ok(dest)
}

/// Anything that is not an `http(s)` URL is a path to a local archive.
fn is_remote(src: &str) -> bool {
    src.starts_with("http://") || src.starts_with("https://")
}

fn cache_key(src: &str) -> String {
    let mut hasher = Sha256::new();

    hasher.update(src.as_bytes());

    // A local archive keeps its path across rebuilds, so only mtime tells two
    // builds of it apart.
    if let Some(mtime) = local_mtime(src) {
        hasher.update(mtime.to_le_bytes());
    }

    let digest = hasher.finalize();

    let mut key = String::with_capacity(CACHE_KEY_BYTES * 2);

    for byte in &digest[..CACHE_KEY_BYTES] {
        // Writing to a String is infallible.
        let _ignored = write!(key, "{byte:02x}");
    }

    key
}

fn local_mtime(src: &str) -> Option<u64> {
    if is_remote(src) {
        return None;
    }

    let modified = fs::metadata(src).and_then(|meta| meta.modified()).ok()?;

    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_secs())
}

fn is_fresh(dir: &Path, lifetime: Duration) -> bool {
    fs::metadata(dir)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < lifetime)
}

fn read_archive(client: &Client, src: &str) -> Result<Vec<u8>> {
    if !is_remote(src) {
        return fs::read(src).wrap_err_with(|| format!("failed to read the archive at {src}"));
    }

    let response = client
        .get(src)
        .send()
        .wrap_err_with(|| format!("failed to request {src}"))?
        .error_for_status()
        .wrap_err_with(|| format!("failed to download {src}"))?;

    Ok(response
        .bytes()
        .wrap_err_with(|| format!("failed to read the response body from {src}"))?
        .to_vec())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use zip::write::{SimpleFileOptions, ZipWriter};
    use zip::CompressionMethod;

    use super::*;

    const FRESH: Duration = Duration::from_secs(60);

    fn write_archive(path: &Path, contents: &str) {
        let file = fs::File::create(path).expect("the archive must be creatable");
        let mut zip = ZipWriter::new(file);

        zip.start_file(
            "index.html",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .expect("the entry must be writable");

        zip.write_all(contents.as_bytes())
            .expect("the entry body must be writable");

        zip.finish().expect("the archive must be finalizable");
    }

    #[test]
    fn extracts_a_local_archive_and_then_reuses_it() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let archive = tmp.path().join("webui.zip");
        let cache = tmp.path().join("cache");

        write_archive(&archive, "first");

        let src = archive.to_str().expect("path should be valid utf-8");
        let client = Client::new();

        let extracted = fetch_and_extract(&client, src, &cache, FRESH, false)
            .expect("the archive should extract");

        assert_eq!(
            fs::read_to_string(extracted.join("index.html")).expect("the entry should exist"),
            "first"
        );

        // Survives only if the second call reused the entry instead of re-extracting.
        let sentinel = extracted.join("sentinel");
        fs::write(&sentinel, "kept").expect("the sentinel must be writable");

        let reused = fetch_and_extract(&client, src, &cache, FRESH, false)
            .expect("the cached extraction should be reused");

        assert_eq!(reused, extracted);
        assert!(sentinel.is_file());
    }

    #[test]
    fn a_fresh_entry_is_served_without_reaching_the_network() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let cache = tmp.path().to_path_buf();
        let src = "https://unresolvable.invalid/webui.zip";

        let entry = cache.join(cache_key(src));
        fs::create_dir_all(&entry).expect("the cache entry must be creatable");

        let served = fetch_and_extract(&Client::new(), src, &cache, FRESH, false)
            .expect("a fresh entry must not be re-downloaded");

        assert_eq!(served, entry);
    }

    #[test]
    fn force_re_extracts_over_a_cache_hit() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let archive = tmp.path().join("webui.zip");
        let cache = tmp.path().join("cache");

        write_archive(&archive, "first");

        let src = archive.to_str().expect("path should be valid utf-8");
        let client = Client::new();

        let _extracted = fetch_and_extract(&client, src, &cache, FRESH, false)
            .expect("the archive should extract");

        write_archive(&archive, "second");

        let forced = fetch_and_extract(&client, src, &cache, FRESH, true)
            .expect("the archive should re-extract");

        assert_eq!(
            fs::read_to_string(forced.join("index.html")).expect("the entry should exist"),
            "second"
        );
    }

    #[test]
    fn distinct_sources_get_distinct_cache_entries() {
        assert_ne!(
            cache_key("https://example.invalid/a.zip"),
            cache_key("https://example.invalid/b.zip")
        );
    }

    #[test]
    fn a_missing_local_archive_reports_the_path() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let missing = tmp.path().join("absent.zip");
        let src = missing.to_str().expect("path should be valid utf-8");

        let err = fetch_and_extract(&Client::new(), src, tmp.path(), FRESH, false)
            .expect_err("a missing archive should fail");

        assert!(err.to_string().contains(src));
    }
}
