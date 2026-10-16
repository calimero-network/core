//! Cached download and extraction of zip archives for build scripts.

use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read as _};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use eyre::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::Url;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

const CACHE_KEY_BYTES: usize = 16; // truncated sha256, wide enough that two sources never collide

/// Well above any bundle we ship, low enough that a bad source cannot fill memory.
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;

/// Extract the zip at `src` (`https`, loopback `http` or a local path) under `cache_dir`,
/// reusing an entry younger than `freshness`. A remote `src` needs `expected_sha256`.
pub fn fetch_and_extract(
    client: &Client,
    src: &str,
    cache_dir: &Path,
    freshness: Duration,
    force: bool,
    expected_sha256: Option<&str>,
) -> Result<PathBuf> {
    fetch_and_extract_with_limit(
        client,
        src,
        cache_dir,
        freshness,
        force,
        expected_sha256,
        MAX_ARCHIVE_BYTES,
    )
}

/// The sha256 an archive from `src` must match: a non-empty `override_sha256`, else
/// `pinned` (passed only for the default source). Only a local `src` may go unverified.
pub fn required_sha256<'a>(
    src: &str,
    pinned: Option<&'a str>,
    override_sha256: Option<&'a str>,
    sha256_var: &str,
) -> Result<Option<&'a str>> {
    let expected = override_sha256.filter(|sha| !sha.is_empty()).or(pinned);

    if expected.is_none() && is_remote(src) {
        bail!("{sha256_var} is required: {src} is not the pinned archive");
    }

    Ok(expected)
}

fn fetch_and_extract_with_limit(
    client: &Client,
    src: &str,
    cache_dir: &Path,
    freshness: Duration,
    force: bool,
    expected_sha256: Option<&str>,
    max_bytes: u64,
) -> Result<PathBuf> {
    if expected_sha256.is_none() && is_remote(src) {
        bail!("refusing {src}: a remote archive needs an expected sha256 from the caller");
    }

    let dest = cache_dir.join(cache_key(src, expected_sha256));

    if !force && is_fresh(&dest, freshness) {
        return Ok(dest);
    }

    let archive = read_archive(client, src, max_bytes)?;

    if let Some(expected) = expected_sha256 {
        verify_sha256(&archive, expected)
            .wrap_err_with(|| format!("refusing the archive from {src}"))?;
    }

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

/// Plain `http` would let anyone on the path swap the archive.
fn require_https_or_loopback(src: &str) -> Result<()> {
    let url = Url::parse(src).wrap_err_with(|| format!("refusing {src}: not a valid URL"))?;

    let allowed = match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }),
        _ => false,
    };

    if !allowed {
        bail!("refusing {src}: not https (plain http is allowed only for a loopback host)");
    }

    Ok(())
}

/// Anything that is not an `http(s)` URL is a path to a local archive.
fn is_remote(src: &str) -> bool {
    Url::parse(src).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn cache_key(src: &str, expected_sha256: Option<&str>) -> String {
    let mut hasher = Sha256::new();

    hasher.update(src.as_bytes());

    if let Some(expected) = expected_sha256 {
        hasher.update(b"\0sha256=");
        hasher.update(expected.to_ascii_lowercase().as_bytes());
    }

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

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);

    let mut hex = String::with_capacity(digest.len() * 2);

    for byte in &digest {
        let _ignored = write!(hex, "{byte:02x}");
    }

    hex
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    let expected = expected.trim().to_ascii_lowercase();

    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("expected sha256 is not 64 hex characters: {expected:?}");
    }

    let actual = hex_sha256(bytes);

    if actual != expected {
        bail!("sha256 mismatch: expected {expected}, got {actual}");
    }

    Ok(())
}

fn read_archive(client: &Client, src: &str, max_bytes: u64) -> Result<Vec<u8>> {
    if !is_remote(src) {
        return fs::read(src).wrap_err_with(|| format!("failed to read the archive at {src}"));
    }

    require_https_or_loopback(src)?;

    let response = client
        .get(src)
        .send()
        .wrap_err_with(|| format!("failed to request {src}"))?
        .error_for_status()
        .wrap_err_with(|| format!("failed to download {src}"))?;

    // A redirect can leave https; judge the URL the body actually came from.
    require_https_or_loopback(response.url().as_str())?;

    let too_large = || format!("refusing the archive from {src}: larger than {max_bytes} bytes");

    if response.content_length().is_some_and(|len| len > max_bytes) {
        bail!(too_large());
    }

    // One byte past the limit is enough to tell a body that overruns it.
    let mut archive = Vec::new();

    response
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut archive)
        .wrap_err_with(|| format!("failed to read the response body from {src}"))?;

    if archive.len() as u64 > max_bytes {
        bail!(too_large());
    }

    Ok(archive)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

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

        let extracted = fetch_and_extract(&client, src, &cache, FRESH, false, None)
            .expect("the archive should extract");

        assert_eq!(
            fs::read_to_string(extracted.join("index.html")).expect("the entry should exist"),
            "first"
        );

        // Survives only if the second call reused the entry instead of re-extracting.
        let sentinel = extracted.join("sentinel");
        fs::write(&sentinel, "kept").expect("the sentinel must be writable");

        let reused = fetch_and_extract(&client, src, &cache, FRESH, false, None)
            .expect("the cached extraction should be reused");

        assert_eq!(reused, extracted);
        assert!(sentinel.is_file());
    }

    #[test]
    fn a_fresh_entry_is_served_without_reaching_the_network() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let cache = tmp.path().to_path_buf();
        let src = "https://unresolvable.invalid/webui.zip";
        let pin = "ab".repeat(32);

        let entry = cache.join(cache_key(src, Some(&pin)));
        fs::create_dir_all(&entry).expect("the cache entry must be creatable");

        let served = fetch_and_extract(&Client::new(), src, &cache, FRESH, false, Some(&pin))
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

        let _extracted = fetch_and_extract(&client, src, &cache, FRESH, false, None)
            .expect("the archive should extract");

        write_archive(&archive, "second");

        let forced = fetch_and_extract(&client, src, &cache, FRESH, true, None)
            .expect("the archive should re-extract");

        assert_eq!(
            fs::read_to_string(forced.join("index.html")).expect("the entry should exist"),
            "second"
        );
    }

    #[test]
    fn distinct_sources_get_distinct_cache_entries() {
        assert_ne!(
            cache_key("https://example.invalid/a.zip", None),
            cache_key("https://example.invalid/b.zip", None)
        );
    }

    #[test]
    fn a_pin_gets_its_own_cache_entry() {
        let src = "https://example.invalid/a.zip";
        let pin = "ab".repeat(32);

        assert_ne!(cache_key(src, None), cache_key(src, Some(&pin)));
        assert_eq!(
            cache_key(src, Some(&pin)),
            cache_key(src, Some(&pin.to_ascii_uppercase()))
        );
    }

    #[test]
    fn extracts_an_archive_matching_its_pin() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let archive = tmp.path().join("webui.zip");
        let cache = tmp.path().join("cache");

        write_archive(&archive, "pinned");

        let pin = hex_sha256(&fs::read(&archive).expect("the archive should be readable"));
        let src = archive.to_str().expect("path should be valid utf-8");

        let extracted = fetch_and_extract(&Client::new(), src, &cache, FRESH, false, Some(&pin))
            .expect("a matching archive should extract");

        assert_eq!(
            fs::read_to_string(extracted.join("index.html")).expect("the entry should exist"),
            "pinned"
        );
    }

    #[test]
    fn refuses_an_archive_that_does_not_match_its_pin() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let archive = tmp.path().join("webui.zip");
        let cache = tmp.path().join("cache");

        write_archive(&archive, "swapped");

        let src = archive.to_str().expect("path should be valid utf-8");
        let pin = "00".repeat(32);

        let err = fetch_and_extract(&Client::new(), src, &cache, FRESH, false, Some(&pin))
            .expect_err("a mismatching archive must be refused");

        assert!(format!("{err:#}").contains("sha256 mismatch"));
        assert!(!cache.join(cache_key(src, Some(&pin))).exists());
    }

    #[test]
    fn a_malformed_pin_is_refused() {
        assert!(verify_sha256(b"x", "not-hex").is_err());
        assert!(verify_sha256(b"x", &"a".repeat(63)).is_err());
    }

    /// Answers one request on a loopback port with `head` and then `body`, and
    /// returns the URL to ask for.
    fn serve_once(head: String, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port must be free");
        let port = listener
            .local_addr()
            .expect("the listener has an address")
            .port();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the client must connect");
            let mut request = [0_u8; 1024];
            let _ignored = stream.read(&mut request);

            // The client hangs up early once it refuses the body.
            let _ignored = stream.write_all(head.as_bytes());
            let _ignored = stream.write_all(&body);
        });

        format!("http://127.0.0.1:{port}/webui.zip")
    }

    fn client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("the client must build")
    }

    fn zip_bytes(contents: &str) -> Vec<u8> {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let path = tmp.path().join("webui.zip");

        write_archive(&path, contents);

        fs::read(path).expect("the archive should be readable")
    }

    #[test]
    fn refuses_a_plain_http_source_before_any_request() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");

        // `.invalid` never resolves, so a request would fail with a different error.
        let err = fetch_and_extract(
            &client(),
            "http://example.invalid/webui.zip",
            tmp.path(),
            FRESH,
            false,
            Some(&"00".repeat(32)),
        )
        .expect_err("a plain http source must be refused");

        let report = format!("{err:#}");

        assert!(report.contains("refusing"), "{report}");
        assert!(report.contains("not https"), "{report}");
    }

    #[test]
    fn plain_http_is_allowed_only_to_a_loopback_host() {
        for loopback in [
            "http://127.0.0.1:8080/a.zip",
            "http://localhost/a.zip",
            "http://[::1]:8080/a.zip",
            "https://example.com/a.zip",
        ] {
            assert!(require_https_or_loopback(loopback).is_ok(), "{loopback}");
        }

        for remote in [
            "http://example.com/a.zip",
            "http://127.0.0.1.example.com/a.zip",
            "http://localhost.example.com/a.zip",
            "http://user@example.com/a.zip",
            "http://10.0.0.1/a.zip",
        ] {
            assert!(require_https_or_loopback(remote).is_err(), "{remote}");
        }
    }

    #[test]
    fn extracts_an_archive_served_within_the_limit() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let body = zip_bytes("served");
        let pin = hex_sha256(&body);

        let url = serve_once(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
            body,
        );

        let extracted = fetch_and_extract_with_limit(
            &client(),
            &url,
            tmp.path(),
            FRESH,
            false,
            Some(&pin),
            1024 * 1024,
        )
        .expect("an archive within the limit should extract");

        assert_eq!(
            fs::read_to_string(extracted.join("index.html")).expect("the entry should exist"),
            "served"
        );
    }

    #[test]
    fn refuses_a_download_that_declares_more_than_the_limit() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");

        // Only the header is sent: refusing on it must not wait for the body.
        let url = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n".into(),
            Vec::new(),
        );

        let pin = "00".repeat(32);
        let err = fetch_and_extract_with_limit(
            &client(),
            &url,
            tmp.path(),
            FRESH,
            false,
            Some(&pin),
            1024,
        )
        .expect_err("an oversized download must be refused");

        let report = format!("{err:#}");

        assert!(report.contains("refusing"), "{report}");
        assert!(report.contains("1024 bytes"), "{report}");
    }

    #[test]
    fn refuses_a_download_that_outgrows_the_limit_without_declaring_a_size() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");

        let url = serve_once(
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".into(),
            vec![0_u8; 256 * 1024],
        );

        let pin = "00".repeat(32);
        let err = fetch_and_extract_with_limit(
            &client(),
            &url,
            tmp.path(),
            FRESH,
            false,
            Some(&pin),
            1024,
        )
        .expect_err("an oversized download must be refused");

        let report = format!("{err:#}");

        assert!(report.contains("refusing"), "{report}");
        assert!(report.contains("1024 bytes"), "{report}");
    }

    #[test]
    fn a_missing_local_archive_reports_the_path() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");
        let missing = tmp.path().join("absent.zip");
        let src = missing.to_str().expect("path should be valid utf-8");

        let err = fetch_and_extract(&Client::new(), src, tmp.path(), FRESH, false, None)
            .expect_err("a missing archive should fail");

        assert!(err.to_string().contains(src));
    }

    // A fresh cache entry would be served if the check came after the cache lookup.
    #[test]
    fn refuses_a_remote_source_without_a_sha256() {
        let tmp = tempfile::tempdir().expect("temp dir must be creatable");

        for src in [
            "https://example.invalid/a.zip",
            "HTTPS://example.invalid/a.zip",
        ] {
            fs::create_dir_all(tmp.path().join(cache_key(src, None)))
                .expect("the cache entry must be creatable");

            let err = fetch_and_extract(&Client::new(), src, tmp.path(), FRESH, false, None)
                .expect_err("an unpinned remote source must be refused");

            assert!(format!("{err:#}").contains("expected sha256"), "{err:#}");
        }
    }

    #[test]
    fn a_remote_override_without_a_sha256_is_refused() {
        for src in [
            "https://example.com/a.zip",
            "HTTPS://example.com/a.zip",
            "http://127.0.0.1:8080/a.zip",
        ] {
            let err = required_sha256(src, None, None, "X_SHA256")
                .expect_err("an unpinned remote source must be refused");

            assert!(err.to_string().contains("X_SHA256"), "{err}");
        }
    }

    #[test]
    fn a_pinned_or_local_source_resolves_its_sha256() {
        let (pinned, given) = (Some("pinned"), Some("given"));
        let remote = "https://example.com/a.zip";
        let local = "/tmp/webui";

        for (src, pinned, given, expected) in [
            (remote, pinned, None, pinned),
            (remote, pinned, given, given),
            (remote, pinned, Some(""), pinned),
            (remote, None, given, given),
            (local, None, given, given),
            (local, None, None, None),
            (local, None, Some(""), None),
        ] {
            let resolved = required_sha256(src, pinned, given, "X_SHA256")
                .expect("a pinned or local source must resolve");

            assert_eq!(resolved, expected, "{src} {pinned:?} {given:?}");
        }
    }
}
