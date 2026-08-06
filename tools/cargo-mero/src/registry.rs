//! Client for the Calimero App Registry: picks the next version and publishes
//! a signed `.mpk`, replacing the curl+python / npm CLI app repos otherwise carry.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use camino::Utf8Path;
use eyre::{eyre, Result};
use flate2::read::GzDecoder;
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(15);

/// Registry base URL used when `CALIMERO_REGISTRY_URL` is not set.
const DEFAULT_BASE_URL: &str = "https://apps.calimero.network";
const BASE_URL_ENV: &str = "CALIMERO_REGISTRY_URL";

/// Registry base URL: `CALIMERO_REGISTRY_URL`, or the public registry.
pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned())
}

pub enum Bump {
    Major,
    Minor,
    Patch,
}

/// Highest published appVersion for `package`, bumped per `bump`.
/// `Ok("0.1.0")` when the registry lists nothing for it.
pub fn next_version(base_url: &str, package: &str, bump: Bump) -> Result<String> {
    let url = format!("{base_url}/api/v2/bundles?package={package}");
    let body = ureq::get(&url)
        .timeout(TIMEOUT)
        .call()
        .map_err(|e| eyre!("failed to query the registry at {url}: {e}"))?
        .into_string()
        .map_err(|e| eyre!("failed to read the registry response from {url}: {e}"))?;
    let bundles: Value = serde_json::from_str(&body)
        .map_err(|e| eyre!("registry returned invalid JSON from {url}: {e}"))?;
    next_from_listing(&bundles, bump)
}

/// Split out from the HTTP call so version selection is unit-testable.
fn next_from_listing(bundles: &Value, bump: Bump) -> Result<String> {
    // A non-array top level (a wrapper object, an error envelope) must not be
    // read as "nothing published": that silently resets an existing package's
    // version instead of surfacing the unrecognized response shape.
    let array = bundles.as_array().ok_or_else(|| {
        eyre!("registry response is not a bundle list (expected a top-level JSON array)")
    })?;

    if array.is_empty() {
        return Ok("0.1.0".to_owned());
    }

    let declared: Vec<&str> = array
        .iter()
        .filter_map(|entry| entry.get("appVersion")?.as_str())
        .collect();

    // Only an empty listing means "nothing published". A listing that has
    // entries but none we can read is an unrecognized version scheme, and
    // defaulting there would sign below whatever is already published.
    let (major, minor, patch) = declared
        .iter()
        .copied()
        .filter_map(parse_version)
        .max()
        .ok_or_else(|| {
            eyre!(
                "registry listed {} bundle(s) but no readable appVersion among [{}]",
                array.len(),
                declared.join(", ")
            )
        })?;

    let (major, minor, patch) = match bump {
        Bump::Major => (major + 1, 0, 0),
        Bump::Minor => (major, minor + 1, 0),
        Bump::Patch => (major, minor, patch + 1),
    };
    Ok(format!("{major}.{minor}.{patch}"))
}

/// Parses `"X.Y.Z"` into numeric components, per-component so `X.Y.Z-rc.1`
/// still compares correctly on `X.Y.Z`; `None` if a component is missing.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let major = parse_component(parts.next()?)?;
    let minor = parse_component(parts.next()?)?;
    let patch = parse_component(parts.next()?)?;
    Some((major, minor, patch))
}

fn parse_component(s: &str) -> Option<u64> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Uploads a signed `.mpk`. The registry's CLI endpoint takes the manifest JSON
/// as the request body, with the whole archive attached under `_binary` (hex);
/// the registry strips `_`-prefixed keys before checking the signature, so this
/// doesn't touch what was signed.
pub fn publish(base_url: &str, api_key: &str, mpk: &Utf8Path) -> Result<()> {
    let bytes = std::fs::read(mpk).map_err(|e| eyre!("failed to read {mpk}: {e}"))?;
    let body = push_body(&bytes)
        .map_err(|e| eyre!("failed to build the publish request from {mpk}: {e}"))?;

    let url = format!("{base_url}/api/v2/bundles/push");
    match ureq::post(&url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Content-Type", "application/json")
        .timeout(TIMEOUT)
        .send_string(&body.to_string())
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            Err(eyre!("registry rejected the publish ({code}): {body}"))
        }
        Err(e) => Err(eyre!("failed to publish to {url}: {e}")),
    }
}

/// Split out from the HTTP call so the request body is unit-testable: parses
/// `manifest.json` out of the `.mpk` and attaches the raw archive as `_binary`.
fn push_body(mpk_bytes: &[u8]) -> Result<Value> {
    let mut manifest = manifest_json(mpk_bytes)?;
    let object = manifest
        .as_object_mut()
        .ok_or_else(|| eyre!("manifest.json is not a JSON object"))?;
    object.insert("_binary".to_owned(), Value::String(hex::encode(mpk_bytes)));
    Ok(manifest)
}

/// Extracts and parses `manifest.json` from a gzipped tar `.mpk`.
fn manifest_json(mpk_bytes: &[u8]) -> Result<Value> {
    let mut archive = tar::Archive::new(GzDecoder::new(mpk_bytes));
    let entries = archive
        .entries()
        .map_err(|e| eyre!("not a valid .mpk archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| eyre!("failed to read a .mpk archive entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| eyre!("failed to read a .mpk archive entry path: {e}"))?;
        if path.as_ref() == Path::new("manifest.json") {
            let mut json = String::new();
            entry
                .read_to_string(&mut json)
                .map_err(|e| eyre!("failed to read manifest.json: {e}"))?;
            return serde_json::from_str(&json)
                .map_err(|e| eyre!("manifest.json is not valid JSON: {e}"));
        }
    }
    Err(eyre!("archive has no manifest.json"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};

    use flate2::write::GzEncoder;
    use flate2::Compression;

    use super::*;

    /// One request as the server actually received it, so a test can assert on
    /// the method/path/headers/body a real client sent - not just what the
    /// request-builder code intended to send.
    struct RecordedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    /// Spins a one-shot HTTP server on a background thread bound to an
    /// ephemeral port (never a fixed one, so tests can't collide), accepts
    /// exactly one connection, records the request, and writes `response`
    /// verbatim. Returns the base URL to hit and a handle to join for the
    /// recorded request.
    fn one_shot_server(response: String) -> (String, JoinHandle<RecordedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        let base_url = format!("http://{}", listener.local_addr().expect("read local addr"));
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept a connection");
            let request = read_request(&mut stream);
            stream
                .write_all(response.as_bytes())
                .expect("write canned response");
            request
        });
        (base_url, handle)
    }

    /// An HTTP response with a correct `Content-Length`, computed from `body`
    /// rather than hardcoded, so a test editing the body can't desync it.
    fn http_response(status_line: &str, body: &str) -> String {
        format!(
            "{status_line}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    fn read_request(stream: &mut TcpStream) -> RecordedRequest {
        let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));

        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .expect("read the request line");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().expect("method").to_owned();
        let path = parts.next().expect("path").to_owned();

        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read a header line");
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').expect("header line has a colon");
            headers.insert(name.trim().to_lowercase(), value.trim().to_owned());
        }

        let body_len: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0; body_len];
        reader.read_exact(&mut body).expect("read the body");

        RecordedRequest {
            method,
            path,
            headers,
            body,
        }
    }

    #[test]
    fn next_version_sends_get_with_the_package_and_bumps_the_real_response() {
        let (base_url, handle) = one_shot_server(http_response(
            "HTTP/1.1 200 OK",
            r#"[{"appVersion":"0.1.9"},{"appVersion":"0.1.10"}]"#,
        ));

        // Numeric, not lexical, comparison must survive the real round trip:
        // "0.1.10" lexically sorts before "0.1.9".
        let version =
            next_version(&base_url, "com.example.demo", Bump::Patch).expect("next version");
        assert_eq!(version, "0.1.11");

        let request = handle.join().expect("server thread");
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/v2/bundles?package=com.example.demo");
    }

    #[test]
    fn next_version_errors_on_a_wrapped_listing_instead_of_returning_0_1_0() {
        let (base_url, handle) = one_shot_server(http_response(
            "HTTP/1.1 200 OK",
            r#"{"bundles":[{"appVersion":"3.4.5"}]}"#,
        ));

        let err = next_version(&base_url, "com.example.demo", Bump::Patch).unwrap_err();
        assert!(err.to_string().contains("not a bundle list"), "got: {err}");

        handle.join().expect("server thread");
    }

    #[test]
    fn publish_posts_to_push_with_bearer_auth_and_the_manifest_body() {
        let mpk = build_mpk(&[(
            "manifest.json",
            br#"{"package":"com.example.demo","appVersion":"1.2.3"}"#,
        )]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        let (base_url, handle) = one_shot_server(http_response("HTTP/1.1 200 OK", "{}"));

        publish(&base_url, "test-api-key", &mpk_path).expect("publish");

        let request = handle.join().expect("server thread");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/v2/bundles/push");
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer test-api-key")
        );

        let body: Value = serde_json::from_slice(&request.body).expect("body is valid JSON");
        assert_eq!(body["package"], "com.example.demo");
        assert_eq!(body["appVersion"], "1.2.3");
        let binary = body["_binary"].as_str().expect("_binary is a string");
        assert_eq!(hex::decode(binary).expect("valid hex"), mpk);
    }

    #[test]
    fn publish_surfaces_the_status_and_body_on_a_failed_push() {
        let mpk = build_mpk(&[("manifest.json", br#"{"package":"com.example.demo"}"#)]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        let (base_url, handle) = one_shot_server(http_response(
            "HTTP/1.1 422 Unprocessable Entity",
            "signature verification failed",
        ));

        let err = publish(&base_url, "test-api-key", &mpk_path).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("422"), "got: {message}");
        assert!(
            message.contains("signature verification failed"),
            "got: {message}"
        );

        handle.join().expect("server thread");
    }

    /// Builds a gzipped tar `.mpk` in memory from `(path, contents)` entries.
    fn build_mpk(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, *data)
                .expect("append tar entry");
        }
        tar.into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
    }

    #[test]
    fn push_body_carries_the_manifest_and_the_binary_round_trips() {
        let mpk = build_mpk(&[("manifest.json", br#"{"package":"com.example.demo"}"#)]);

        let body = push_body(&mpk).expect("push body");

        assert_eq!(body["package"], "com.example.demo");
        let binary = body["_binary"].as_str().expect("_binary is a string");
        assert_eq!(hex::decode(binary).expect("valid hex"), mpk);
    }

    #[test]
    fn push_body_errors_when_the_archive_has_no_manifest_json() {
        let mpk = build_mpk(&[("app.wasm", b"wasm-bytes")]);

        let err = push_body(&mpk).unwrap_err().to_string();

        assert!(err.contains("manifest.json"), "got: {err}");
    }

    #[test]
    fn picks_the_highest_version_then_bumps_the_patch() {
        let bundles = serde_json::json!([
            {"appVersion": "0.1.9"}, {"appVersion": "0.1.10"}, {"appVersion": "0.1.2"},
        ]);
        assert_eq!(
            next_from_listing(&bundles, Bump::Patch).expect("next"),
            "0.1.11"
        );
    }

    #[test]
    fn errors_when_entries_exist_but_none_carry_a_readable_version() {
        let bundles = serde_json::json!([
            {"appVersion": "not-a-version"},
            {"notAppVersion": "9.9.9"},
        ]);
        let err = next_from_listing(&bundles, Bump::Patch).expect_err("must not default");
        assert!(
            err.to_string().contains("no readable appVersion"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn falls_back_when_the_registry_lists_nothing() {
        assert_eq!(
            next_from_listing(&serde_json::json!([]), Bump::Patch).expect("next"),
            "0.1.0"
        );
    }

    #[test]
    fn errors_instead_of_treating_a_wrapped_listing_as_empty() {
        let wrapped = serde_json::json!({"bundles": [{"appVersion": "3.4.5"}]});
        let err = next_from_listing(&wrapped, Bump::Patch)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a bundle list"), "got: {err}");
    }

    #[test]
    fn bumps_minor_and_zeroes_patch() {
        let bundles = serde_json::json!([{"appVersion": "1.2.9"}]);
        assert_eq!(
            next_from_listing(&bundles, Bump::Minor).expect("next"),
            "1.3.0"
        );
    }

    #[test]
    fn bumps_major_and_zeroes_minor_and_patch() {
        let bundles = serde_json::json!([{"appVersion": "1.2.9"}]);
        assert_eq!(
            next_from_listing(&bundles, Bump::Major).expect("next"),
            "2.0.0"
        );
    }

    #[test]
    fn skips_malformed_versions_instead_of_crashing() {
        let bundles = serde_json::json!([
            {"appVersion": "not-a-version"},
            {"appVersion": "0.1"},
            {"notAppVersion": "9.9.9"},
            {"appVersion": "0.2.4"},
        ]);
        assert_eq!(
            next_from_listing(&bundles, Bump::Patch).expect("next"),
            "0.2.5"
        );
    }

    #[test]
    fn ignores_a_prerelease_suffix_on_the_patch_component() {
        let bundles = serde_json::json!([{"appVersion": "0.1.5-rc.1"}]);
        assert_eq!(
            next_from_listing(&bundles, Bump::Patch).expect("next"),
            "0.1.6"
        );
    }
}
