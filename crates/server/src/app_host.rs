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
use calimero_primitives::blobs::BlobId;
use futures_util::{stream, StreamExt as _};
use tracing::error;

use crate::config::AppsConfig;
use crate::origin_guard::names_origin;

const CSP: &str = "default-src 'self'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self'"; // see crates/bundle/AGENTS.md
const IMMUTABLE: &str = "public, max-age=31536000, immutable"; // a file's URL names its release
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567"; // RFC 4648 alphabet, lowercase
const LABEL_LEN: usize = 52; // 32 bytes in unpadded base32
const NATIVE_SCHEME: &str = "calimero-app"; // the desktop webview's app scheme on macOS and Linux
const WINDOWS_HOST_PREFIX: &str = "calimero-app."; // its host form on Windows: `http(s)://calimero-app.{label}`
pub(crate) const AUTH_REFRESH: &str = "/auth/refresh"; // a login-side route an app origin reaches: a JSON token exchange
const AUTH_VALIDATE: &str = "/auth/validate"; // the other: a token check that reads nothing but the token
const ADMIN_API: &str = "/admin-api"; // where the node mounts its admin routes, under `NODE_PATH_PREFIX`
const NO_APP_API: &str = "the app API on an app origin needs the node's embedded auth"; // body of the 403 where this node does not verify tokens
const FILES_ONLY: &str =
    "the fallback apps port serves files only; the app API needs the app's own subdomain origin"; // body of the 403 to an API path on the apps port
const ADMIN_REFUSED: &str = "an app's origin may use only the admin routes an app needs"; // body of the 403 to any other admin route
const FOREIGN_ORIGIN: &str = "an app's API answers a page of its own origin only"; // body of the 403 to another origin

#[derive(Clone)]
pub(crate) struct AppHosts {
    pub(crate) node_client: NodeClient,
    pub(crate) origins: AppOrigins,
    pub(crate) app_api: bool,
}

/// Marks a request answered for an app's own origin, which the origin checks
/// let through, and names the app whose origin it is.
#[derive(Clone, Debug)]
pub(crate) struct AppApi(pub(crate) ApplicationId);

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

/// The browser origins an app's page can run from.
#[derive(Clone, Debug)]
pub(crate) struct AppOrigins {
    base_domain: String,
    port: Option<u16>,
}

impl AppOrigins {
    pub(crate) fn new(apps: &AppsConfig) -> Self {
        Self {
            base_domain: apps.base_domain.to_ascii_lowercase(),
            port: apps.port,
        }
    }

    /// Whether `origin` is shaped like any app's: `{label}.{base_domain}`, any
    /// page on the apps port, or one of the desktop app's own origins.
    pub(crate) fn is_app_origin(&self, origin: &str) -> bool {
        let Some((scheme, authority)) = split_origin(origin) else {
            return false;
        };
        let host = authority.host().to_ascii_lowercase();
        match scheme.as_str() {
            NATIVE_SCHEME => decode_label(&host).is_some(),
            "http" | "https" => {
                self.app_at(&host).is_some()
                    || is_windows_host(&host)
                    || self.is_apps_port(&scheme, &authority)
            }
            _ => false,
        }
    }

    /// A browser omits a scheme's default port from `Origin`.
    fn is_apps_port(&self, scheme: &str, authority: &Authority) -> bool {
        let default = if scheme == "https" { 443 } else { 80 };
        self.port
            .is_some_and(|port| authority.port_u16().unwrap_or(default) == port)
    }

    /// The app whose host is exactly `{label}.{base_domain}`.
    fn app_at(&self, host: &str) -> Option<ApplicationId> {
        let host = host.to_ascii_lowercase();
        decode_label(host.strip_suffix(&self.base_domain)?.strip_suffix('.')?)
    }
}

fn split_origin(origin: &str) -> Option<(String, Authority)> {
    let (scheme, rest) = origin.split_once("://")?;
    Some((scheme.to_ascii_lowercase(), rest.parse().ok()?))
}

fn is_windows_host(host: &str) -> bool {
    host.strip_prefix(WINDOWS_HOST_PREFIX)
        .is_some_and(|label| decode_label(label).is_some())
}

impl AppHosts {
    /// Passes an app API request to the node's own handlers, only where this
    /// node verifies the token itself and the page is of the origin it calls.
    async fn hand_over(&self, app: ApplicationId, mut request: Request, next: Next) -> Response {
        if !self.app_api {
            return refusal(StatusCode::FORBIDDEN, NO_APP_API);
        }
        if !is_same_origin(request.headers()) {
            return refusal(StatusCode::FORBIDDEN, FOREIGN_ORIGIN);
        }
        request.extensions_mut().insert(AppApi(app));
        hardened(next.run(request).await)
    }

    /// The app a `Host` header names: exactly `{label}.{base_domain}`.
    fn app_for(&self, headers: &HeaderMap) -> Option<ApplicationId> {
        let host = headers.get(header::HOST)?.to_str().ok()?;
        self.origins.app_at(host.parse::<Authority>().ok()?.host())
    }

    /// Answers `path`, relative to the app's root; `prefix` is where that root
    /// sits on the host, for redirects.
    async fn respond(
        &self,
        app: &ApplicationId,
        request: &Parts,
        path: &str,
        prefix: &str,
    ) -> Response {
        if !matches!(request.method, Method::GET | Method::HEAD) {
            return status(StatusCode::NOT_FOUND);
        }
        if path == "/" {
            return self.redirect_to_current(app, request.uri.query(), prefix);
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
            return redirect(&format!("{prefix}/{version}/"));
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

    fn redirect_to_current(
        &self,
        app: &ApplicationId,
        query: Option<&str>,
        prefix: &str,
    ) -> Response {
        // A namespace's release is not recorded against the app, so it names nothing here.
        let names_namespace = query.is_some_and(|q| {
            q.split('&')
                .any(|p| p.split('=').next() == Some("namespace"))
        });
        let current = self.node_client.get_application(app).ok().flatten();
        match current.and_then(|row| row.version) {
            Some(version) if !names_namespace => redirect(&format!("{prefix}/{version}/")),
            _ => status(StatusCode::NOT_FOUND),
        }
    }

    /// Streams the blob, reading its first chunk first so a blob that is gone
    /// or fails its hash is a 500 before any header is sent.
    async fn open_blob(&self, blob_id: BlobId) -> Result<(Body, u64), StatusCode> {
        let failed = |why: &dyn core::fmt::Display| {
            error!(%blob_id, %why, "refusing to serve a frontend blob");
            StatusCode::INTERNAL_SERVER_ERROR
        };
        let size = self
            .node_client
            .blob_size(&blob_id)
            .map_err(|e| failed(&e))?;
        let size = size.ok_or_else(|| failed(&"blob is missing"))?;
        let blob = self.node_client.get_blob(&blob_id, None).await;
        let mut blob = blob
            .map_err(|e| failed(&e))?
            .ok_or_else(|| failed(&"blob is missing"))?;
        let first = blob.next().await.transpose().map_err(|e| failed(&e))?;
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
    let (method, path) = (request.method(), request.uri().path());
    if is_app_api(method, path, node_prefix()) {
        return hosts.hand_over(app, request, next).await;
    }
    if is_admin_api(path, node_prefix()) {
        return refusal(StatusCode::FORBIDDEN, ADMIN_REFUSED);
    }
    let (parts, _body) = request.into_parts();
    hosts.respond(&app, &parts, parts.uri.path(), "").await
}

/// The apps port: `/{label}/{version}/{path}` and nothing else. Its apps share
/// one origin, so none of them gets the node's API from it.
pub(crate) async fn serve_apps_port(State(hosts): State<AppHosts>, request: Request) -> Response {
    if is_node_api(request.uri().path(), node_prefix()) {
        return refusal(StatusCode::FORBIDDEN, FILES_ONLY);
    }
    let (parts, _body) = request.into_parts();
    let rest = parts.uri.path().strip_prefix('/').unwrap_or_default();
    let (label, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let Some(app) = decode_label(label) else {
        return status(StatusCode::NOT_FOUND);
    };
    let prefix = format!("/{label}");
    hosts
        .respond(&app, &parts, &format!("/{tail}"), &prefix)
        .await
}

fn node_prefix() -> &'static str {
    crate::admin::service::node_path_prefix().unwrap_or_default()
}

/// The routes an app's own origin passes on to the node: RPC, sockets, events,
/// the token exchange and the admin routes an app needs. Never login.
fn is_app_api(method: &Method, path: &str, prefix: &str) -> bool {
    let path = path.trim_end_matches('/');
    path.strip_prefix(prefix).is_some_and(|route| {
        route == "/jsonrpc" || route == "/ws" || is_app_admin_route(method, route)
    }) || (path == AUTH_REFRESH && method == Method::POST)
        || (path == AUTH_VALIDATE && matches!(*method, Method::GET | Method::HEAD))
        || path == "/sse"
        || path.starts_with("/sse/")
}

/// The admin routes an app uses to find and make its contexts and namespaces,
/// matched by method and exact segment shape.
fn is_app_admin_route(method: &Method, route: &str) -> bool {
    let Some(route) = route
        .strip_prefix(ADMIN_API)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    let segments: Vec<&str> = route.split('/').collect();
    matches!(
        (method, segments.as_slice()),
        (
            &Method::GET,
            ["contexts" | "namespaces" | "applications" | "health" | "is-authed"]
        ) | (&Method::GET, ["contexts", _])
            | (&Method::GET, ["applications", _])
            | (&Method::GET, ["namespaces", "for-application", _])
            | (&Method::GET, ["contexts", "for-application", _])
            | (&Method::GET, ["contexts", _, "identities-owned" | "group"])
            | (&Method::GET, ["groups", _, "members"])
            | (&Method::POST, ["contexts" | "namespaces"])
            | (&Method::POST, ["contexts", _, "join"])
            | (
                &Method::POST,
                ["namespaces", _, "invite" | "join" | "groups"]
            )
    )
}

fn is_admin_api(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .and_then(|route| route.strip_prefix(ADMIN_API))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Whether `path` is under a route the node answers with an API: RPC, sockets,
/// events, auth or the admin surface.
fn is_node_api(path: &str, prefix: &str) -> bool {
    fn first(path: &str) -> Option<&str> {
        path.strip_prefix('/')?.split('/').next()
    }
    let prefixed = path.strip_prefix(prefix).and_then(first);
    matches!(prefixed, Some("jsonrpc" | "ws" | "admin-api"))
        || matches!(first(path), Some("sse" | "auth"))
}

/// Whether a page's `Origin` is the host it called.
fn is_same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let called = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok()?.parse::<Authority>().ok());
    let page = origin.to_str().ok().and_then(split_origin);
    matches!((called, page), (Some(called), Some((scheme, page)))
        if names_origin(&called, &scheme, &page))
}

fn refusal(code: StatusCode, why: &'static str) -> Response {
    let mut response = Response::new(Body::from(why));
    *response.status_mut() = code;
    drop(
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain")),
    );
    hardened(response)
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

    use super::{
        decode_label, encode_label, is_app_api, is_node_api, serve_app_hosts, serve_apps_port,
        AppApi, AppHosts, AppOrigins, ApplicationId, AUTH_REFRESH, BASE32,
    };
    use crate::config::AppsConfig;
    use crate::test_support::{stub_node_manager, test_node_client};

    const BASE: &str = "localhost";
    const CSP: &str = "default-src 'self'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self'";
    const APP: [u8; 32] = [7; 32];
    const API_PATHS: [&str; 5] = ["/jsonrpc", "/ws", "/sse", "/sse/subscribe", "/auth/refresh"];
    const ID: &str = "0123abcd";
    const ALLOWED_ADMIN: [(Method, &str); 16] = [
        (Method::GET, "/admin-api/applications"),
        (Method::GET, "/admin-api/applications/0123abcd"),
        (
            Method::GET,
            "/admin-api/namespaces/for-application/0123abcd",
        ),
        (Method::GET, "/admin-api/contexts"),
        (Method::GET, "/admin-api/contexts/0123abcd"),
        (Method::GET, "/admin-api/contexts/for-application/0123abcd"),
        (Method::GET, "/admin-api/contexts/0123abcd/identities-owned"),
        (Method::GET, "/admin-api/contexts/0123abcd/group"),
        (Method::POST, "/admin-api/contexts"),
        (Method::POST, "/admin-api/contexts/0123abcd/join"),
        (Method::GET, "/admin-api/namespaces"),
        (Method::POST, "/admin-api/namespaces"),
        (Method::POST, "/admin-api/namespaces/0123abcd/invite"),
        (Method::POST, "/admin-api/namespaces/0123abcd/join"),
        (Method::POST, "/admin-api/namespaces/0123abcd/groups"),
        (Method::GET, "/admin-api/groups/0123abcd/members"),
    ];

    struct Fixture {
        router: Router,
        port_router: Router,
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
        fixture_with(false).await
    }

    /// What the node's router does with a request an app host hands it.
    async fn reached(request: Request<Body>) -> &'static str {
        match request.extensions().get::<AppApi>() {
            Some(_) => "app api",
            None => "normal router",
        }
    }

    async fn fixture_with(app_api: bool) -> Fixture {
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
            origins: AppOrigins::new(&AppsConfig {
                base_domain: BASE.to_owned(),
                ..AppsConfig::default()
            }),
            app_api,
        };
        let routes = Router::new()
            .route("/{*rest}", any(reached))
            .route("/", any(reached));
        let port_router = Router::new()
            .fallback(serve_apps_port)
            .with_state(hosts.clone());
        let router = routes.layer(from_fn_with_state(hosts, serve_app_hosts));
        Fixture {
            router,
            port_router,
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
        let paths = ["/admin-dashboard", "/auth/login", "/health"];

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
    async fn an_app_host_hands_the_app_api_to_the_node_when_tokens_are_enforced() {
        let f = fixture_with(true).await;

        for path in API_PATHS {
            for method in [Method::GET, Method::POST] {
                if path == "/auth/refresh" && method == Method::GET {
                    continue;
                }
                let (response, body) = send(&f.router, method.clone(), &f.host, path, &[]).await;
                assert_eq!(response.status(), StatusCode::OK, "{method} {path}");
                assert_eq!(body, b"app api", "{method} {path}");
                assert_hardened(&response);
            }
        }
    }

    #[actix::test]
    async fn an_app_host_hands_over_the_admin_routes_an_app_needs() {
        let f = fixture_with(true).await;
        let mut routes = ALLOWED_ADMIN.to_vec();
        routes.extend([
            (Method::GET, "/admin-api/health"),
            (Method::GET, "/admin-api/is-authed"),
            (Method::GET, "/auth/validate"),
            (Method::HEAD, "/auth/validate"),
        ]);

        for (method, path) in routes {
            let (response, body) = send(&f.router, method.clone(), &f.host, path, &[]).await;
            assert_eq!(response.status(), StatusCode::OK, "{method} {path}");
            if method != Method::HEAD {
                assert_eq!(body, b"app api", "{method} {path}");
            }
            assert_hardened(&response);
        }
    }

    #[actix::test]
    async fn an_app_host_refuses_every_other_admin_route() {
        let f = fixture_with(true).await;
        let refused = [
            (Method::DELETE, format!("/admin-api/contexts/{ID}")),
            (Method::PUT, format!("/admin-api/contexts/{ID}")),
            (Method::DELETE, format!("/admin-api/blobs/{ID}")),
            (Method::GET, "/admin-api/blobs".to_owned()),
            (Method::POST, "/admin-api/install-application".to_owned()),
            (Method::DELETE, format!("/admin-api/applications/{ID}")),
            (Method::GET, format!("/admin-api/applications/{ID}/abi")),
            (
                Method::GET,
                format!("/admin-api/applications/{ID}/versions"),
            ),
            (Method::POST, "/admin-api/applications".to_owned()),
            (Method::POST, format!("/admin-api/groups/{ID}/members")),
            (
                Method::DELETE,
                format!("/admin-api/groups/{ID}/members/{ID}"),
            ),
            (Method::GET, format!("/admin-api/groups/{ID}/members/{ID}")),
            (Method::GET, format!("/admin-api/groups/{ID}")),
            (Method::POST, "/admin-api/alias/create/context".to_owned()),
            (Method::GET, "/admin-api/alias/list/context".to_owned()),
            (Method::DELETE, format!("/admin-api/namespaces/{ID}")),
            (Method::GET, format!("/admin-api/namespaces/{ID}")),
            (Method::POST, format!("/admin-api/namespaces/{ID}/leave")),
            (Method::GET, format!("/admin-api/contexts/{ID}/identities")),
            (Method::GET, format!("/admin-api/contexts/{ID}/group/extra")),
            (Method::POST, format!("/admin-api/contexts/{ID}/group")),
            (Method::POST, format!("/admin-api/contexts/{ID}/join/extra")),
            (Method::GET, format!("/admin-api/contexts/{ID}/join")),
            (Method::POST, "/admin-api/contexts/sync".to_owned()),
            (Method::HEAD, "/admin-api/contexts".to_owned()),
            (Method::POST, "/admin-api/health".to_owned()),
            (Method::GET, "/admin-api".to_owned()),
            (Method::GET, "/admin-api/".to_owned()),
            (Method::GET, "/admin-api/tee/attest".to_owned()),
            (Method::POST, "/auth/validate".to_owned()),
            (Method::DELETE, "/auth/validate".to_owned()),
        ];

        for app_api in [true, false] {
            let f = if app_api { &f } else { &fixture().await };
            for (method, path) in &refused {
                let (response, body) = send(&f.router, method.clone(), &f.host, path, &[]).await;
                if path.starts_with("/auth") {
                    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                    continue;
                }
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
                assert_ne!(body, b"app api", "{method} {path}");
                assert!(
                    *method == Method::HEAD || !body.is_empty(),
                    "{method} {path}"
                );
                assert_hardened(&response);
            }
        }
    }

    #[actix::test]
    async fn the_token_refresh_is_handed_over_for_a_post_only() {
        let f = fixture_with(true).await;

        for method in [Method::GET, Method::PUT, Method::DELETE] {
            let (response, body) =
                send(&f.router, method.clone(), &f.host, "/auth/refresh", &[]).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method}");
            assert_ne!(body, b"app api", "{method}");
        }
    }

    #[test]
    fn the_app_api_is_where_the_router_mounts_it() {
        for (method, path, prefix, api) in [
            (Method::POST, "/jsonrpc", "", true),
            (Method::POST, "/node1/jsonrpc", "/node1", true),
            (Method::GET, "/node1/ws", "/node1", true),
            (Method::GET, "/sse/subscribe", "/node1", true),
            (Method::POST, "/auth/refresh", "/node1", true),
            (Method::POST, "/jsonrpc", "/node1", false),
            (Method::GET, "/ws", "/node1", false),
            (Method::GET, "/node1/sse", "/node1", false),
            (Method::POST, "/node1/auth/refresh", "/node1", false),
            (Method::GET, "/node1/node1/ws", "/node1", false),
            (Method::GET, "/auth/refresh", "", false),
            (Method::GET, "/node1/admin-api/contexts", "/node1", true),
            (
                Method::POST,
                "/node1/admin-api/contexts/c/join",
                "/node1",
                true,
            ),
            (Method::GET, "/admin-api/contexts", "/node1", false),
            (
                Method::GET,
                "/node1/node1/admin-api/contexts",
                "/node1",
                false,
            ),
            (
                Method::GET,
                "/node1/admin-api/contexts/c/extra",
                "/node1",
                false,
            ),
            (
                Method::DELETE,
                "/node1/admin-api/contexts/c",
                "/node1",
                false,
            ),
            (Method::GET, "/admin-apix/contexts", "", false),
            (Method::GET, "/auth/validate", "/node1", true),
            (Method::HEAD, "/auth/validate", "", true),
            (Method::POST, "/auth/validate", "", false),
        ] {
            assert_eq!(is_app_api(&method, path, prefix), api, "{method} {path}");
        }
    }

    #[test]
    fn the_node_api_is_where_the_router_mounts_it() {
        for (path, prefix, api) in [
            ("/jsonrpc", "", true),
            ("/admin-api", "", true),
            ("/admin-api/contexts", "", true),
            ("/node1/admin-api/contexts", "/node1", true),
            ("/node1/ws", "/node1", true),
            ("/sse/subscribe", "/node1", true),
            ("/auth/login", "/node1", true),
            ("/admin-api/contexts", "/node1", false),
            ("/jsonrpcx", "", false),
            ("/admin-dashboard", "", false),
            ("/health", "", false),
            ("/abc/1.0.0/", "", false),
        ] {
            assert_eq!(is_node_api(path, prefix), api, "{path} under {prefix:?}");
        }
    }

    #[actix::test]
    async fn an_app_host_has_no_app_api_where_the_node_does_not_verify_tokens() {
        let f = fixture().await;
        let forged = [("x-auth-account", "victim"), ("x-auth-device", "d")];

        for path in API_PATHS {
            let mut refused = vec![
                send(&f.router, Method::POST, &f.host, path, &[]).await,
                send(&f.router, Method::POST, &f.host, path, &forged).await,
            ];
            if path != AUTH_REFRESH {
                refused.push(get(&f, path).await);
            }
            for (response, body) in refused {
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
                assert!(
                    str::from_utf8(&body).unwrap().contains("embedded auth"),
                    "{path}"
                );
                assert_hardened(&response);
            }
        }
    }

    #[actix::test]
    async fn an_app_host_keeps_the_nodes_own_routes_off_its_origin_even_with_tokens() {
        let f = fixture_with(true).await;
        let paths = [
            "/admin-dashboard",
            "/auth/login",
            "/auth/token",
            "/auth/providers",
            "/auth/refresh/x",
            "/jsonrpc/x",
            "/ws/x",
            "/health",
        ];

        for path in paths {
            for method in [Method::GET, Method::POST] {
                let (response, body) = send(&f.router, method.clone(), &f.host, path, &[]).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                assert!(body.is_empty(), "{method} {path}");
            }
        }
    }

    #[actix::test]
    async fn the_app_api_answers_a_page_of_the_same_origin_only() {
        let f = fixture_with(true).await;
        let own = format!("http://{}", f.host);
        let other = format!(
            "http://{}.localhost:2428",
            encode_label(&ApplicationId::from([9; 32]))
        );

        let (same, _) = send(
            &f.router,
            Method::POST,
            &f.host,
            "/jsonrpc",
            &[("origin", &own)],
        )
        .await;
        assert_eq!(same.status(), StatusCode::OK);
        let label = encode_label(&ApplicationId::from(APP));
        for (host, origin) in [
            (
                format!("{label}.localhost:80"),
                format!("http://{label}.localhost"),
            ),
            (
                format!("{label}.localhost"),
                format!("http://{label}.localhost:80"),
            ),
            (
                format!("{label}.localhost:443"),
                format!("https://{label}.localhost"),
            ),
        ] {
            let (response, _) = send(
                &f.router,
                Method::POST,
                &host,
                "/jsonrpc",
                &[("origin", &origin)],
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{host} {origin}");
        }
        let (response, _) = send(
            &f.router,
            Method::POST,
            &format!("{label}.localhost:443"),
            "/jsonrpc",
            &[("origin", &format!("http://{label}.localhost"))],
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        for origin in [
            other.as_str(),
            "https://evil.example",
            "http://localhost:2428",
            "null",
        ] {
            let (response, body) = send(
                &f.router,
                Method::POST,
                &f.host,
                "/jsonrpc",
                &[("origin", origin)],
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin}");
            assert_ne!(body, b"app api", "{origin}");
        }
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

    async fn on_port(f: &Fixture, method: Method, uri: &str) -> (Response, Vec<u8>) {
        send(&f.port_router, method, "127.0.0.1:2429", uri, &[]).await
    }

    #[actix::test]
    async fn the_apps_port_serves_each_apps_files_under_its_label() {
        let f = fixture().await;
        let label = encode_label(&ApplicationId::from(APP));

        let (page, html) = on_port(&f, Method::GET, &format!("/{label}/1.0.0/")).await;
        let (_, js) = on_port(&f, Method::GET, &format!("/{label}/2.0.0/app.js")).await;
        let (head, _) = on_port(&f, Method::HEAD, &format!("/{label}/1.0.0/app.js")).await;

        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!((html, js), (b"<p>v1</p>".to_vec(), b"v2();".to_vec()));
        assert_hardened(&page);
        assert_eq!(header_of(&head, "content-length"), "5");
    }

    #[actix::test]
    async fn the_apps_port_redirects_within_the_label() {
        let f = fixture().await;
        let label = encode_label(&ApplicationId::from(APP));

        for (uri, to) in [
            (format!("/{label}"), format!("/{label}/2.0.0/")),
            (format!("/{label}/"), format!("/{label}/2.0.0/")),
            (format!("/{label}/1.0.0"), format!("/{label}/1.0.0/")),
        ] {
            let (response, _) = on_port(&f, Method::GET, &uri).await;
            assert_eq!(response.status(), StatusCode::FOUND, "{uri}");
            assert_eq!(header_of(&response, "location"), to, "{uri}");
            assert_hardened(&response);
        }
    }

    #[actix::test]
    async fn the_apps_port_serves_files_only() {
        let paths = [
            "/jsonrpc",
            "/ws",
            "/sse",
            "/sse/subscribe",
            "/auth/refresh",
            "/auth/login",
            "/auth/token",
            "/admin-api",
            "/admin-api/contexts",
            "/admin-api/applications",
        ];

        for app_api in [false, true] {
            let f = fixture_with(app_api).await;
            for path in paths {
                for method in [Method::GET, Method::POST, Method::DELETE] {
                    let (response, body) = on_port(&f, method.clone(), path).await;
                    assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
                    assert!(
                        str::from_utf8(&body).unwrap().contains("files only"),
                        "{method} {path}"
                    );
                    assert_hardened(&response);
                }
            }
        }
    }

    #[actix::test]
    async fn the_apps_port_serves_nothing_else() {
        let f = fixture().await;
        let label = encode_label(&ApplicationId::from(APP));
        let unknown = encode_label(&ApplicationId::from([9; 32]));
        let paths = [
            "/".to_owned(),
            "/admin-dashboard".to_owned(),
            "/health".to_owned(),
            "/1.0.0/app.js".to_owned(),
            format!("/{}/1.0.0/", non_canonical(&label)),
            format!("/{unknown}/1.0.0/"),
            format!("/{label}/9.9.9/"),
            format!("/{label}/1.0.0/../2.0.0/app.js"),
            format!("/{label}/1.0.0/%2e%2e/2.0.0/app.js"),
        ];

        for path in paths {
            for method in [Method::GET, Method::POST] {
                let (response, body) = on_port(&f, method.clone(), &path).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                assert!(body.is_empty(), "{path}");
                assert_hardened(&response);
            }
        }
        let (post, _) = on_port(&f, Method::POST, &format!("/{label}/1.0.0/app.js")).await;
        assert_eq!(post.status(), StatusCode::NOT_FOUND);
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

    #[test]
    fn only_a_canonical_label_under_the_base_domain_is_an_app_origin() {
        let label = encode_label(&ApplicationId::from(APP));
        let apps = AppsConfig {
            port: Some(2429),
            ..AppsConfig::default()
        };
        let origins = AppOrigins::new(&apps);
        let base = &apps.base_domain;

        for origin in [
            format!("http://{label}.{base}:2428"),
            format!("HTTPS://{}.{base}", label.to_ascii_uppercase()),
            format!("calimero-app://{label}"),
            format!("http://calimero-app.{label}"),
            format!("https://calimero-app.{label}"),
            "http://203.0.113.9:2429".to_owned(),
        ] {
            assert!(origins.is_app_origin(&origin), "{origin}");
        }
        for origin in [
            format!("http://{}.{base}", non_canonical(&label)),
            format!("ftp://{label}.{base}"),
            format!("http://{label}.evil.example"),
            format!("http://{label}{base}"),
            format!("calimero-app://{}", non_canonical(&label)),
            "http://203.0.113.9:2430".to_owned(),
            "http://localhost:5173".to_owned(),
            "null".to_owned(),
        ] {
            assert!(!origins.is_app_origin(&origin), "{origin}");
        }
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
