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

/// The publish request carries the API key, so it goes over https, or over
/// plain http only to this machine.
fn require_secure_registry(base_url: &str) -> Result<()> {
    let uri: ureq::http::Uri = base_url
        .parse()
        .map_err(|e| eyre!("invalid registry URL {base_url}: {e}"))?;
    match uri.scheme_str() {
        Some("https") => Ok(()),
        Some("http") if is_loopback(uri.host()) => Ok(()),
        _ => Err(eyre!(
            "refusing to send the registry API key to {base_url}: use an https URL \
             (plain http is accepted only for localhost)"
        )),
    }
}

fn is_loopback(host: Option<&str>) -> bool {
    let Some(host) = host else { return false };
    host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub enum Bump {
    Major,
    Minor,
    Patch,
}

/// Highest published appVersion for `package`, bumped per `bump`.
/// `Ok("0.1.0")` when the registry lists nothing for it.
pub fn next_version(base_url: &str, package: &str, bump: Bump) -> Result<String> {
    let url = format!("{base_url}/api/v2/bundles?package={package}&all_versions=true");
    let body = ureq::get(&url)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .call()
        .map_err(|e| eyre!("failed to query the registry at {url}: {e}"))?
        .body_mut()
        .read_to_string()
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

/// The archive's own PUT to storage: up to 50 MiB, so far longer than a JSON call.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Uploads a signed `.mpk`.
///
/// The registry runs on Vercel, which refuses a request body over 4.5 MB: the
/// archive as `_binary` hex in the push stops at about 2.2 MiB, and a bundle
/// that carries its frontend is usually past that. So the archive goes straight
/// to storage: `POST /upload-url` grants a signed URL, the `.mpk` is PUT there,
/// and the push carries the manifest with `_upload: {object}`.
///
/// A registry that does not grant one (403, 404 or 405: an older registry, or
/// an account it does not allow) gets the `_binary` push as before. The
/// registry strips `_`-prefixed keys before checking the signature, so neither
/// touches what was signed.
pub fn publish(base_url: &str, api_key: &str, mpk: &Utf8Path) -> Result<()> {
    require_secure_registry(base_url)?;
    let bytes = std::fs::read(mpk).map_err(|e| eyre!("failed to read {mpk}: {e}"))?;
    let mut manifest = manifest_object(&bytes)
        .map_err(|e| eyre!("failed to build the publish request from {mpk}: {e}"))?;

    let api = format!("{base_url}/api/v2/bundles");
    match upload_grant(&api, api_key)? {
        Some(grant) => {
            upload(&grant, &bytes)?;
            let _prev = manifest.insert(
                "_upload".to_owned(),
                serde_json::json!({ "object": grant.object }),
            );
        }
        None => {
            let _prev = manifest.insert("_binary".to_owned(), Value::String(hex::encode(&bytes)));
        }
    }
    push(&api, api_key, &Value::Object(manifest))
}

/// A signed URL to PUT the archive to, and the object name the push refers to.
#[derive(Debug)]
struct UploadGrant {
    url: String,
    object: String,
    headers: Vec<(String, String)>,
}

/// `Ok(None)` when the registry does not grant direct upload to this caller.
fn upload_grant(api: &str, api_key: &str) -> Result<Option<UploadGrant>> {
    let url = format!("{api}/upload-url");
    let mut response = ureq::post(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .config()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .send_empty()
        .map_err(|e| eyre!("failed to request an upload URL from {url}: {e}"))?;
    let code = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    if matches!(code, 403..=405) {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(eyre!("registry refused an upload URL ({code}): {body}"));
    }
    parse_grant(&body).map(Some)
}

/// Split out from the HTTP call so the grant's shape is unit-testable.
fn parse_grant(body: &str) -> Result<UploadGrant> {
    let grant: Value = serde_json::from_str(body)
        .map_err(|e| eyre!("registry returned an invalid upload grant: {e}"))?;
    let field = |name: &str| {
        grant
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| eyre!("registry's upload grant has no `{name}`"))
    };
    let headers = match grant.get("headers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(h)) => h
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|v| (k.clone(), v.to_owned()))
                    .ok_or_else(|| eyre!("registry's upload grant header `{k}` is not a string"))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(eyre!("registry's upload grant `headers` is not an object")),
    };
    Ok(UploadGrant {
        url: field("url")?,
        object: field("object")?,
        headers,
    })
}

/// PUTs the archive to the signed URL, replaying the grant's headers verbatim
/// (the size cap is one of them). The API key never goes to storage.
fn upload(grant: &UploadGrant, bytes: &[u8]) -> Result<()> {
    let mut request = ureq::put(&grant.url);
    for (name, value) in &grant.headers {
        request = request.header(name, value);
    }
    let mut response = request
        .config()
        .timeout_global(Some(UPLOAD_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .send(bytes)
        .map_err(|e| eyre!("failed to upload the bundle to storage: {e}"))?;
    if response.status().is_success() {
        return Ok(());
    }
    let code = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    Err(eyre!("storage refused the bundle upload ({code}): {body}"))
}

fn push(api: &str, api_key: &str, body: &Value) -> Result<()> {
    let url = format!("{api}/push");
    // ureq 3's `Error::StatusCode` carries only the code, so a non-2xx would
    // lose the body the registry explains itself in. Turn that mapping off and
    // read the status here, keeping the old message intact.
    let mut response = ureq::post(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .config()
        .timeout_global(Some(UPLOAD_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .send(body.to_string())
        .map_err(|e| eyre!("failed to publish to {url}: {e}"))?;

    if response.status().is_success() {
        return Ok(());
    }

    let code = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    Err(eyre!("registry rejected the publish ({code}): {body}"))
}

/// The `.mpk`'s `manifest.json`, as the object the push body is built on.
fn manifest_object(mpk_bytes: &[u8]) -> Result<serde_json::Map<String, Value>> {
    match manifest_json(mpk_bytes)? {
        Value::Object(map) => Ok(map),
        _ => Err(eyre!("manifest.json is not a JSON object")),
    }
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

    /// Like `one_shot_server`, for a client that makes several requests:
    /// accepts `n` connections in turn and answers the i-th with
    /// `respond(i, base_url)`, so a response can point back at this server.
    fn scripted_server(
        n: usize,
        respond: impl Fn(usize, &str) -> String + Send + 'static,
    ) -> (String, JoinHandle<Vec<RecordedRequest>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        let base_url = format!("http://{}", listener.local_addr().expect("read local addr"));
        let base = base_url.clone();
        let handle = thread::spawn(move || {
            (0..n)
                .map(|i| {
                    let (mut stream, _) = listener.accept().expect("accept a connection");
                    let request = read_request(&mut stream);
                    stream
                        .write_all(respond(i, &base).as_bytes())
                        .expect("write canned response");
                    request
                })
                .collect()
        });
        (base_url, handle)
    }

    /// An HTTP response with a correct `Content-Length`, computed from `body`
    /// rather than hardcoded, so a test editing the body can't desync it.
    fn http_response(status_line: &str, body: &str) -> String {
        format!(
            "{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
        assert_eq!(
            request.path,
            "/api/v2/bundles?package=com.example.demo&all_versions=true"
        );
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
    fn publish_uploads_straight_to_storage_then_pushes_the_object() {
        let mpk = build_mpk(&[(
            "manifest.json",
            br#"{"package":"com.example.demo","appVersion":"1.2.3"}"#,
        )]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        let (base_url, handle) = scripted_server(3, |i, base| match i {
            0 => http_response(
                "HTTP/1.1 200 OK",
                &serde_json::json!({
                    "url": format!("{base}/storage/signed?sig=x"),
                    "object": "uploads/abc/1.mpk",
                    "method": "PUT",
                    "headers": {
                        "Content-Type": "application/gzip",
                        "x-goog-content-length-range": "0,52428800",
                    },
                })
                .to_string(),
            ),
            _ => http_response("HTTP/1.1 200 OK", "{}"),
        });

        publish(&base_url, "test-api-key", &mpk_path).expect("publish");

        let requests = handle.join().expect("server thread");
        let [grant, put, push] = &requests[..] else {
            panic!("expected 3 requests, got {}", requests.len())
        };

        assert_eq!(grant.method, "POST");
        assert_eq!(grant.path, "/api/v2/bundles/upload-url");
        assert_eq!(
            grant.headers.get("authorization").map(String::as_str),
            Some("Bearer test-api-key")
        );

        assert_eq!(put.method, "PUT");
        assert_eq!(put.path, "/storage/signed?sig=x");
        assert_eq!(put.body, mpk);
        assert_eq!(
            put.headers
                .get("x-goog-content-length-range")
                .map(String::as_str),
            Some("0,52428800")
        );
        assert_eq!(
            put.headers.get("content-type").map(String::as_str),
            Some("application/gzip")
        );
        assert!(
            !put.headers.contains_key("authorization"),
            "the API key must not go to storage"
        );

        assert_eq!(push.method, "POST");
        assert_eq!(push.path, "/api/v2/bundles/push");
        assert_eq!(
            push.headers.get("authorization").map(String::as_str),
            Some("Bearer test-api-key")
        );
        let body: Value = serde_json::from_slice(&push.body).expect("body is valid JSON");
        assert_eq!(body["package"], "com.example.demo");
        assert_eq!(body["appVersion"], "1.2.3");
        assert_eq!(body["_upload"]["object"], "uploads/abc/1.mpk");
        assert!(body.get("_binary").is_none(), "got: {body}");
    }

    #[test]
    fn publish_falls_back_to_binary_when_the_registry_grants_no_upload() {
        let mpk = build_mpk(&[(
            "manifest.json",
            br#"{"package":"com.example.demo","appVersion":"1.2.3"}"#,
        )]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        for refused in ["403 Forbidden", "404 Not Found", "405 Method Not Allowed"] {
            let (base_url, handle) = scripted_server(2, move |i, _| match i {
                0 => http_response(&format!("HTTP/1.1 {refused}"), r#"{"error":"forbidden"}"#),
                _ => http_response("HTTP/1.1 200 OK", "{}"),
            });

            publish(&base_url, "test-api-key", &mpk_path).expect("publish");

            let requests = handle.join().expect("server thread");
            let push = &requests[1];
            assert_eq!(push.method, "POST");
            assert_eq!(push.path, "/api/v2/bundles/push");
            let body: Value = serde_json::from_slice(&push.body).expect("body is valid JSON");
            assert_eq!(body["package"], "com.example.demo");
            assert!(body.get("_upload").is_none(), "got: {body}");
            let binary = body["_binary"].as_str().expect("_binary is a string");
            assert_eq!(hex::decode(binary).expect("valid hex"), mpk);
        }
    }

    #[test]
    fn publish_stops_when_storage_refuses_the_upload() {
        let mpk = build_mpk(&[(
            "manifest.json",
            br#"{"package":"com.example.demo","appVersion":"1.2.3"}"#,
        )]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        let (base_url, handle) = scripted_server(2, |i, base| match i {
            0 => http_response(
                "HTTP/1.1 200 OK",
                &serde_json::json!({
                    "url": format!("{base}/storage/signed"),
                    "object": "uploads/abc/1.mpk",
                    "headers": {},
                })
                .to_string(),
            ),
            _ => http_response("HTTP/1.1 400 Bad Request", "EntityTooLarge"),
        });

        let err = publish(&base_url, "test-api-key", &mpk_path).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("storage refused"), "got: {message}");
        assert!(message.contains("EntityTooLarge"), "got: {message}");

        // No push follows a failed upload.
        assert_eq!(handle.join().expect("server thread").len(), 2);
    }

    #[test]
    fn publish_surfaces_a_refused_upload_grant() {
        let mpk = build_mpk(&[(
            "manifest.json",
            br#"{"package":"com.example.demo","appVersion":"1.2.3"}"#,
        )]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        let (base_url, handle) = one_shot_server(http_response(
            "HTTP/1.1 429 Too Many Requests",
            "At most 10 upload URLs per hour",
        ));

        let err = publish(&base_url, "test-api-key", &mpk_path).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("429"), "got: {message}");
        assert!(message.contains("At most 10"), "got: {message}");

        handle.join().expect("server thread");
    }

    #[test]
    fn publish_refuses_a_plain_http_registry_that_is_not_loopback() {
        let mpk = build_mpk(&[("manifest.json", br#"{"package":"com.example.demo"}"#)]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        let err = publish("http://registry.invalid", "test-api-key", &mpk_path)
            .expect_err("an api key must not travel over plain http");
        assert!(err.to_string().contains("https"), "got: {err}");
    }

    #[test]
    fn plain_http_is_accepted_only_for_loopback_hosts() {
        for ok in [
            "https://apps.calimero.network",
            "http://127.0.0.1:8080",
            "http://localhost:8080/",
            "http://[::1]:8080",
        ] {
            assert!(
                require_secure_registry(ok).is_ok(),
                "{ok} should be accepted"
            );
        }
        for refused in [
            "http://apps.calimero.network",
            "http://10.0.0.5:8080",
            "http://localhost.example.com",
            "http://127.0.0.1.example.com",
            "http://127.0.0.1@example.com",
            "ftp://127.0.0.1",
            "apps.calimero.network",
        ] {
            assert!(
                require_secure_registry(refused).is_err(),
                "{refused} should be refused"
            );
        }
    }

    #[test]
    fn publish_surfaces_the_status_and_body_on_a_failed_push() {
        let mpk = build_mpk(&[("manifest.json", br#"{"package":"com.example.demo"}"#)]);
        let tmp = tempfile::tempdir().expect("tempdir");
        let mpk_path = Utf8Path::from_path(tmp.path())
            .expect("utf8 tempdir")
            .join("demo.mpk");
        std::fs::write(&mpk_path, &mpk).expect("write fixture mpk");

        let (base_url, handle) = scripted_server(2, |i, _| match i {
            0 => http_response("HTTP/1.1 403 Forbidden", "{}"),
            _ => http_response(
                "HTTP/1.1 422 Unprocessable Entity",
                "signature verification failed",
            ),
        });

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
    fn manifest_object_reads_the_archive_manifest() {
        let mpk = build_mpk(&[("manifest.json", br#"{"package":"com.example.demo"}"#)]);

        let manifest = manifest_object(&mpk).expect("manifest");

        assert_eq!(manifest["package"], "com.example.demo");
    }

    #[test]
    fn parse_grant_needs_a_url_and_an_object() {
        let grant =
            parse_grant(r#"{"url":"https://s/x","object":"uploads/a/b.mpk","headers":{"A":"1"}}"#)
                .expect("grant");
        assert_eq!(grant.url, "https://s/x");
        assert_eq!(grant.object, "uploads/a/b.mpk");
        assert_eq!(grant.headers, vec![("A".to_owned(), "1".to_owned())]);

        let err = parse_grant(r#"{"url":"https://s/x"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("`object`"), "got: {err}");
        let err = parse_grant(r#"{"url":"u","object":"o","headers":{"A":1}}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a string"), "got: {err}");
    }

    #[test]
    fn manifest_object_errors_when_the_archive_has_no_manifest_json() {
        let mpk = build_mpk(&[("app.wasm", b"wasm-bytes")]);

        let err = manifest_object(&mpk).unwrap_err().to_string();

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
