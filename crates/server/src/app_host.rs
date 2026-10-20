//! Serves an installed app's frontend from its own origin, `{label}.{base_domain}`.

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::http::uri::Authority;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use calimero_bundle::FRONTEND_ROOT;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use futures_util::{stream, StreamExt as _};
use tracing::error;

const CSP: &str = "default-src 'self'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self'"; // see crates/bundle/AGENTS.md
const IMMUTABLE: &str = "public, max-age=31536000, immutable"; // a file's URL names its release
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567"; // RFC 4648 alphabet, lowercase
const LABEL_LEN: usize = 52; // 32 bytes in unpadded base32

#[derive(Clone)]
pub(crate) struct AppHosts {
    pub(crate) node_client: NodeClient,
    pub(crate) base_domain: String,
}

/// An app's DNS label: its id as unpadded lowercase base32.
pub(crate) fn encode_label(id: &ApplicationId) -> String {
    let mut label = String::with_capacity(LABEL_LEN);
    let (mut acc, mut bits) = (0u16, 0u8);
    for byte in **id {
        acc = (acc << 8) | u16::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            label.push(char::from(BASE32[usize::from((acc >> bits) & 31)]));
        }
    }
    if bits > 0 {
        label.push(char::from(BASE32[usize::from((acc << (5 - bits)) & 31)]));
    }
    label
}

/// The id a label encodes, only if the label is its canonical spelling.
fn decode_label(label: &str) -> Option<ApplicationId> {
    if label.len() != LABEL_LEN {
        return None;
    }
    let mut id = [0u8; 32];
    let (mut acc, mut bits, mut next) = (0u16, 0u8, 0usize);
    for ch in label.bytes() {
        let value = BASE32.iter().position(|&c| c == ch)? as u16;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            id[next] = (acc >> bits) as u8;
            next += 1;
        }
    }
    let id = ApplicationId::from(id);
    (encode_label(&id) == label).then_some(id)
}

impl AppHosts {
    /// The app a `Host` header names: exactly `{label}.{base_domain}`.
    fn app_for(&self, headers: &HeaderMap) -> Option<ApplicationId> {
        let host = headers.get(header::HOST)?.to_str().ok()?;
        let host = host.parse::<Authority>().ok()?.host().to_ascii_lowercase();
        let label = host.strip_suffix(&self.base_domain)?.strip_suffix('.')?;
        decode_label(label)
    }

    fn current_version(&self, app: &ApplicationId) -> Option<String> {
        let row = self.node_client.get_application(app).ok()??;
        Some(row.version?.to_string())
    }

    async fn respond(&self, app: &ApplicationId, request: &Parts) -> Response {
        if !matches!(request.method, Method::GET | Method::HEAD) {
            return status(StatusCode::NOT_FOUND);
        }
        let path = request.uri.path();
        if path == "/" {
            return self.redirect_to_current(app, request.uri.query());
        }
        let Some(rest_of_path) = path.strip_prefix('/') else {
            return status(StatusCode::NOT_FOUND);
        };
        let (version, rest) = match rest_of_path.split_once('/') {
            Some((version, rest)) => (version, Some(rest)),
            None => (rest_of_path, None),
        };
        let index = match self.node_client.find_bundle_frontend(app, version) {
            Ok(Some(index)) => index,
            Ok(None) => return status(StatusCode::NOT_FOUND),
            Err(err) => {
                error!(%err, "failed to read a frontend index");
                return status(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        let Some(rest) = rest else {
            return redirect(&format!("/{version}/"));
        };
        let wanted = if rest.is_empty() {
            index.entry.to_string()
        } else {
            format!("{FRONTEND_ROOT}{rest}")
        };
        let Some(file) = index.files.iter().find(|file| *file.path == *wanted) else {
            return status(StatusCode::NOT_FOUND);
        };
        let Some(mime) = calimero_bundle::mime_type(&wanted) else {
            return status(StatusCode::NOT_FOUND);
        };

        let blob_id = file.blob.blob_id();
        let etag = format!("\"{blob_id}\"");
        if matches_etag(&request.headers, &etag) {
            return with_file_headers(status(StatusCode::NOT_MODIFIED), mime, &etag);
        }
        let (body, size) = match self.open_blob(blob_id).await {
            Ok(opened) => opened,
            Err(code) => return status(code),
        };
        let body = if request.method == Method::HEAD {
            Body::empty()
        } else {
            body
        };
        let mut response = with_file_headers(hardened(Response::new(body)), mime, &etag);
        drop(
            response
                .headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from(size)),
        );
        response
    }

    fn redirect_to_current(&self, app: &ApplicationId, query: Option<&str>) -> Response {
        // A namespace's release is not recorded against the app, so it names nothing here.
        let names_namespace = query.is_some_and(|q| {
            q.split('&')
                .any(|p| p.split('=').next() == Some("namespace"))
        });
        match self.current_version(app) {
            Some(version) if !names_namespace => redirect(&format!("/{version}/")),
            _ => status(StatusCode::NOT_FOUND),
        }
    }

    /// Streams the blob, reading its first chunk first so a blob that is gone
    /// or fails its hash is a 500 before any header is sent.
    async fn open_blob(
        &self,
        blob_id: calimero_primitives::blobs::BlobId,
    ) -> Result<(Body, u64), StatusCode> {
        let failed = |why: &dyn core::fmt::Display| {
            error!(%blob_id, %why, "refusing to serve a frontend blob");
            StatusCode::INTERNAL_SERVER_ERROR
        };
        let size = match self.node_client.blob_size(&blob_id) {
            Ok(Some(size)) => size,
            Ok(None) => return Err(failed(&"blob is missing")),
            Err(err) => return Err(failed(&err)),
        };
        let mut blob = match self.node_client.get_blob(&blob_id, None).await {
            Ok(Some(blob)) => blob,
            Ok(None) => return Err(failed(&"blob is missing")),
            Err(err) => return Err(failed(&err)),
        };
        let first = match blob.next().await {
            Some(Ok(chunk)) => Some(chunk),
            Some(Err(err)) => return Err(failed(&err)),
            None => None,
        };
        let chunks = stream::iter(first.map(Ok)).chain(blob);
        let chunks = chunks.map(|chunk| chunk.map(Bytes::from).map_err(std::io::Error::other));
        Ok((Body::from_stream(chunks), size))
    }
}

pub(crate) async fn serve_app_hosts(
    State(hosts): State<AppHosts>,
    request: Request,
    next: Next,
) -> Response {
    let Some(app) = hosts.app_for(request.headers()) else {
        return next.run(request).await;
    };
    let (parts, _body) = request.into_parts();
    hosts.respond(&app, &parts).await
}

fn status(code: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = code;
    hardened(response)
}

fn redirect(to: &str) -> Response {
    let mut response = status(StatusCode::FOUND);
    if let Ok(location) = HeaderValue::from_str(to) {
        drop(response.headers_mut().insert(header::LOCATION, location));
    }
    response
}

fn hardened(mut response: Response) -> Response {
    let headers = response.headers_mut();
    drop(headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    ));
    drop(headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    ));
    drop(headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    ));
    response
}

fn with_file_headers(mut response: Response, mime: &'static str, etag: &str) -> Response {
    let headers = response.headers_mut();
    drop(headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime)));
    drop(headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE)));
    if let Ok(etag) = HeaderValue::from_str(etag) {
        drop(headers.insert(header::ETAG, etag));
    }
    response
}

fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|tag| tag.trim().trim_start_matches("W/"))
        .any(|tag| tag == "*" || tag == etag)
}

#[cfg(test)]
mod tests {
    use core::str;
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use axum::middleware::from_fn_with_state;
    use axum::response::Response;
    use axum::routing::any;
    use axum::Router;
    use calimero_primitives::blobs::BlobId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::types::{BundleFrontend, FrontendFile, PackageInfo};
    use calimero_store::{key, types, Store};
    use futures_util::io::Cursor;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::{decode_label, encode_label, serve_app_hosts, AppHosts, ApplicationId, BASE32};
    use crate::test_support::{stub_node_manager, test_node_client};

    const BASE: &str = "localhost";
    const CSP: &str = "default-src 'self'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self'";
    const APP: [u8; 32] = [7; 32];

    struct Fixture {
        router: Router,
        node_client: calimero_node_primitives::client::NodeClient,
        store: Store,
        blob_dir: TempDir,
        host: String,
        js_v1: BlobId,
    }

    async fn add(
        node_client: &calimero_node_primitives::client::NodeClient,
        bytes: &[u8],
    ) -> BlobId {
        node_client
            .add_blob(Cursor::new(bytes.to_vec()), Some(bytes.len() as u64), None)
            .await
            .expect("add blob")
            .0
    }

    fn publish(store: &Store, version: &str, bundle: u8, files: Vec<(&str, BlobId)>) {
        let files = files
            .into_iter()
            .map(|(path, blob)| FrontendFile::new(path.into(), key::BlobMeta::new(blob)))
            .collect();
        let index = BundleFrontend::new(
            key::ApplicationMeta::new(ApplicationId::from(APP)),
            version.into(),
            "frontend/index.html".into(),
            files,
        );
        let mut handle = store.handle();
        handle
            .put(
                &key::BundleFrontend::new(BlobId::from([bundle; 32])),
                &index,
            )
            .unwrap();
        handle
            .put(
                &key::ApplicationRelease::new(ApplicationId::from(APP), version),
                &types::ApplicationRelease {
                    bundle: [bundle; 32],
                },
            )
            .unwrap();
    }

    async fn release(
        store: &Store,
        node_client: &calimero_node_primitives::client::NodeClient,
        version: &str,
        bundle: u8,
        index_html: &[u8],
        app_js: &[u8],
    ) -> BlobId {
        let html = add(node_client, index_html).await;
        let js = add(node_client, app_js).await;
        let notes = add(node_client, b"notes").await;
        let files = vec![
            ("frontend/index.html", html),
            ("frontend/app.js", js),
            ("frontend/notes.xyz", notes),
        ];
        publish(store, version, bundle, files);
        js
    }

    async fn fixture() -> Fixture {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
        let (node_client, blob_dir) =
            test_node_client(&store, stub_node_manager(vec![]), event_sender).await;

        let js_v1 = release(&store, &node_client, "1.0.0", 1, b"<p>v1</p>", b"v1();").await;
        release(&store, &node_client, "2.0.0", 2, b"<p>v2</p>", b"v2();").await;
        let row = types::ApplicationMeta::new(
            key::BlobMeta::new(BlobId::from([0; 32])),
            0,
            "file:///test.mpk".into(),
            Box::default(),
            key::BlobMeta::new(BlobId::from([0; 32])),
            PackageInfo {
                package: "com.example.app".into(),
                version: "2.0.0".into(),
                signer_id: "did:key:test".into(),
                state_version: 0,
            },
        );
        store
            .handle()
            .put(&key::ApplicationMeta::new(ApplicationId::from(APP)), &row)
            .unwrap();

        let hosts = AppHosts {
            node_client: node_client.clone(),
            base_domain: BASE.to_owned(),
        };
        let router = Router::new()
            .route("/{*rest}", any(|| async { "normal router" }))
            .route("/", any(|| async { "normal router" }))
            .layer(from_fn_with_state(hosts, serve_app_hosts));
        Fixture {
            router,
            node_client,
            store,
            blob_dir,
            host: format!("{}.{BASE}:2428", encode_label(&ApplicationId::from(APP))),
            js_v1,
        }
    }

    async fn send(
        router: &Router,
        method: Method,
        host: &str,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> (Response, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, host);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX)
            .await
            .unwrap_or_default()
            .to_vec();
        (Response::from_parts(parts, Body::empty()), bytes)
    }

    async fn get(f: &Fixture, uri: &str) -> (Response, Vec<u8>) {
        send(&f.router, Method::GET, &f.host, uri, &[]).await
    }

    fn header_of<'a>(response: &'a Response, name: &str) -> &'a str {
        response.headers()[name].to_str().unwrap()
    }

    fn assert_hardened(response: &Response) {
        assert_eq!(header_of(response, "content-security-policy"), CSP);
        assert_eq!(header_of(response, "x-content-type-options"), "nosniff");
        assert_eq!(header_of(response, "referrer-policy"), "no-referrer");
    }

    #[actix::test]
    async fn each_release_serves_its_own_bytes() {
        let f = fixture().await;

        let (r1, b1) = get(&f, "/1.0.0/").await;
        let (r2, b2) = get(&f, "/2.0.0/").await;
        let (_, js1) = get(&f, "/1.0.0/app.js").await;
        let (_, js2) = get(&f, "/2.0.0/app.js").await;

        assert_eq!((r1.status(), r2.status()), (StatusCode::OK, StatusCode::OK));
        assert_eq!((b1, b2), (b"<p>v1</p>".to_vec(), b"<p>v2</p>".to_vec()));
        assert_eq!((js1, js2), (b"v1();".to_vec(), b"v2();".to_vec()));
    }

    #[actix::test]
    async fn a_file_carries_the_serving_headers() {
        let f = fixture().await;

        let (response, _) = get(&f, "/1.0.0/app.js").await;

        assert_eq!(
            header_of(&response, "content-type"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(header_of(&response, "etag"), format!("\"{}\"", f.js_v1));
        assert_eq!(
            header_of(&response, "cache-control"),
            "public, max-age=31536000, immutable"
        );
        assert_hardened(&response);
    }

    #[actix::test]
    async fn a_matching_etag_gets_304() {
        let f = fixture().await;
        let etag = format!("\"{}\"", f.js_v1);

        let (hit, body) = send(
            &f.router,
            Method::GET,
            &f.host,
            "/1.0.0/app.js",
            &[("if-none-match", &etag)],
        )
        .await;
        let (miss, _) = send(
            &f.router,
            Method::GET,
            &f.host,
            "/1.0.0/app.js",
            &[("if-none-match", "\"other\"")],
        )
        .await;

        assert_eq!(hit.status(), StatusCode::NOT_MODIFIED);
        assert!(body.is_empty());
        assert_hardened(&hit);
        assert_eq!(miss.status(), StatusCode::OK);
    }

    #[actix::test]
    async fn head_returns_the_headers_without_a_body() {
        let f = fixture().await;

        let (response, body) = send(&f.router, Method::HEAD, &f.host, "/1.0.0/app.js", &[]).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, "content-type"),
            "text/javascript; charset=utf-8"
        );
        assert!(body.is_empty());
    }

    #[actix::test]
    async fn an_app_host_serves_nothing_but_its_files() {
        let f = fixture().await;
        let paths = [
            "/admin-dashboard",
            "/admin-api/applications",
            "/auth/login",
            "/jsonrpc",
            "/ws",
            "/sse",
            "/health",
        ];

        for path in paths {
            for method in [Method::GET, Method::POST] {
                let (response, body) = send(&f.router, method.clone(), &f.host, path, &[]).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                assert_ne!(body, b"normal router");
                assert_hardened(&response);
            }
        }
        let (post, _) = send(&f.router, Method::POST, &f.host, "/1.0.0/app.js", &[]).await;
        assert_eq!(post.status(), StatusCode::NOT_FOUND);
    }

    #[actix::test]
    async fn other_hosts_reach_the_normal_router() {
        let f = fixture().await;
        let label = encode_label(&ApplicationId::from(APP));
        let hosts = [
            "localhost:2428".to_owned(),
            "127.0.0.1:2428".to_owned(),
            format!("{label}.other.example"),
            format!("{}.localhost", non_canonical(&label)),
            format!("x.{label}.localhost"),
            format!("{label}localhost"),
            "short.localhost".to_owned(),
        ];

        for host in hosts {
            let (response, body) =
                send(&f.router, Method::GET, &host, "/admin-dashboard", &[]).await;
            assert_eq!(response.status(), StatusCode::OK, "{host}");
            assert_eq!(body, b"normal router", "{host}");
        }
    }

    #[actix::test]
    async fn unknown_labels_versions_and_paths_are_404() {
        let f = fixture().await;
        let unknown_app = format!("{}.localhost", encode_label(&ApplicationId::from([9; 32])));
        let paths = [
            "/9.9.9/",
            "/9.9.9/app.js",
            "/1.0.0/missing.js",
            "/1.0.0/../2.0.0/app.js",
            "/1.0.0/%2e%2e/2.0.0/app.js",
            "/%2e%2e/app.js",
            "/1.0.0/..%2fapp.js",
            "/1.0.0/frontend/app.js",
            "/1.0.0/app.js/",
            "/9.9.9",
            "/../",
        ];

        for path in paths {
            let (response, _) = get(&f, path).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_hardened(&response);
        }
        let (response, _) = send(&f.router, Method::GET, &unknown_app, "/1.0.0/", &[]).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_hardened(&response);
    }

    #[actix::test]
    async fn the_root_redirects_to_the_current_version() {
        let f = fixture().await;

        let (response, _) = get(&f, "/").await;

        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(header_of(&response, "location"), "/2.0.0/");
        assert_hardened(&response);
    }

    #[actix::test]
    async fn the_root_with_a_namespace_is_404() {
        let f = fixture().await;

        let (response, _) = get(&f, "/?namespace=abc").await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[actix::test]
    async fn a_deleted_blob_is_an_error_not_wrong_bytes() {
        let f = fixture().await;
        let blob = f
            .node_client
            .get_bundle_frontend(&BlobId::from([1; 32]))
            .unwrap()
            .unwrap()
            .files[0]
            .blob
            .blob_id();
        f.node_client.delete_blob(blob).await.unwrap();

        let (response, body) = get(&f, "/1.0.0/").await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!str::from_utf8(&body).unwrap_or_default().contains("v1"));
        assert_hardened(&response);
    }

    #[actix::test]
    async fn a_corrupted_blob_is_an_error_not_wrong_bytes() {
        let f = fixture().await;
        let mut corrupted = 0;
        for entry in walk(f.blob_dir.path()) {
            let len = std::fs::metadata(&entry).unwrap().len() as usize;
            std::fs::write(&entry, vec![b'X'; len]).unwrap();
            corrupted += 1;
        }
        assert!(corrupted > 0);

        let (response, body) = get(&f, "/1.0.0/app.js").await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body.contains(&b'X'));
    }

    fn non_canonical(label: &str) -> String {
        let (head, last) = label.split_at(label.len() - 1);
        let next = BASE32[BASE32
            .iter()
            .position(|&c| c == last.as_bytes()[0])
            .unwrap()
            + 1];
        format!("{head}{}", char::from(next))
    }

    #[actix::test]
    async fn a_label_with_stray_padding_bits_names_no_app() {
        let id = ApplicationId::from(APP);
        let label = encode_label(&id);

        assert_eq!(decode_label(&label), Some(id));
        assert_eq!(decode_label(&non_canonical(&label)), None);
    }

    #[actix::test]
    async fn an_uppercase_host_names_the_app() {
        let f = fixture().await;
        let host = f.host.to_ascii_uppercase();

        let (response, body) = send(&f.router, Method::GET, &host, "/1.0.0/", &[]).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body, b"<p>v1</p>");
    }

    #[actix::test]
    async fn a_wildcard_or_weak_etag_gets_304() {
        let f = fixture().await;
        let weak = format!("W/\"{}\"", f.js_v1);

        for value in ["*", weak.as_str()] {
            let headers = [("if-none-match", value)];
            let (response, _) =
                send(&f.router, Method::GET, &f.host, "/1.0.0/app.js", &headers).await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{value}");
        }
    }

    #[actix::test]
    async fn a_stored_file_outside_the_mime_allowlist_is_404() {
        let f = fixture().await;

        let (response, _) = get(&f, "/1.0.0/notes.xyz").await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_hardened(&response);
    }

    #[actix::test]
    async fn a_version_without_its_slash_redirects_when_it_exists() {
        let f = fixture().await;

        let (found, _) = get(&f, "/1.0.0").await;

        assert_eq!(found.status(), StatusCode::FOUND);
        assert_eq!(header_of(&found, "location"), "/1.0.0/");
        assert_hardened(&found);
    }

    #[actix::test]
    async fn a_file_declares_its_length_and_head_matches() {
        let f = fixture().await;

        let (get_response, _) = get(&f, "/1.0.0/app.js").await;
        let (head_response, _) = send(&f.router, Method::HEAD, &f.host, "/1.0.0/app.js", &[]).await;

        assert_eq!(header_of(&get_response, "content-length"), "5");
        assert_eq!(header_of(&head_response, "content-length"), "5");
    }

    #[actix::test]
    async fn a_chunk_corrupted_after_the_first_breaks_the_body() {
        let f = fixture().await;
        let big: Vec<u8> = (0..3 * (1 << 20))
            .map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let blob = add(&f.node_client, &big).await;
        publish(
            &f.store,
            "3.0.0",
            3,
            vec![("frontend/index.html", blob), ("frontend/big.js", blob)],
        );
        let second = &big[1 << 20..2 << 20];
        let chunk_file = walk(f.blob_dir.path())
            .into_iter()
            .find(|path| std::fs::read(path).is_ok_and(|bytes| bytes == second))
            .expect("the second chunk's file");
        std::fs::write(chunk_file, vec![b'X'; 1 << 20]).unwrap();

        let response = f
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/3.0.0/big.js")
                    .header(header::HOST, &f.host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let declared = header_of(&response, "content-length").to_owned();
        let read = to_bytes(response.into_body(), usize::MAX).await;

        assert_eq!(declared, big.len().to_string());
        assert!(
            read.is_err(),
            "a complete body was delivered over a corrupt chunk"
        );
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
        files
    }
}
