//! App frontends, served from each package's own origin (v1's `app_host`):
//! `{label}.{apps_domain}`, where the label is the package's (52 lowercase
//! base32 characters). The files are the release's own `.mpk` frontend,
//! fetched like its WASM and verified against the publisher's signature
//! before a byte is served.
//!
//! An app's page is third-party code, so it gets an origin of its own, and a
//! passkey ceremony from that origin opens a session bound to its package
//! (`SessionClaims::app`): it reaches that package's apps and nothing of the
//! account beyond them (`api::Principal::within`, `unbound`).
//!
//! - `GET /` redirects to the newest release's entry, `/{version}/`.
//! - `GET /{version}/{path}`: the file; for a path with no file, no `.` in its
//!   last segment and an `Accept` naming HTML, the entry (a single-page app's
//!   routes survive a reload); otherwise the path with leading segments
//!   dropped (an entry built with a relative base on a deep link); else 404.
//! - `GET /.well-known/calimero.json`: the package, its releases and the
//!   cluster's API URLs, for the page to connect to.
//! - Nothing else: only `GET` and `HEAD`, never the node's own routes.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use calimero_bundle::{mime_type, FRONTEND_ROOT};

use crate::node::Node;
use crate::system::compare_versions;

/// v1's content security policy, with the cluster's API origins added to
/// `connect-src` (the page talks to the cluster, not to its own origin).
const CSP_HEAD: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self'; connect-src 'self'";
const IMMUTABLE: &str = "public, max-age=31536000, immutable"; // a file's URL names its release
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const MAX_STRIPPED_SEGMENTS: usize = 16;
/// How many releases' frontends a node keeps open in memory.
const CACHED_FRONTENDS: usize = 32;

/// The DNS label of `package`'s origin: SHA-256 of a domain-separated name,
/// unpadded lowercase base32 (52 characters, as v1's).
#[must_use]
pub fn label(package: &str) -> String {
    let digest = calimero_identity::sha256(format!("calimero.app-host.v1\0{package}").as_bytes());
    let mut out = String::with_capacity(52);
    let (mut acc, mut bits) = (0u16, 0u8);
    for byte in digest {
        acc = (acc << 8) | u16::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[usize::from((acc >> bits) & 31)]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[usize::from((acc << (5 - bits)) & 31)]));
    }
    out
}

/// One release's frontend, verified.
pub struct Frontend {
    /// The entry's path, relative to the frontend root.
    entry: String,
    files: BTreeMap<String, Arc<[u8]>>,
}

/// `(package, version)` -> (last use, the frontend).
type OpenFrontends = HashMap<(String, String), (u64, Arc<Frontend>)>;

/// Frontends opened from their `.mpk`, newest use last.
#[derive(Default)]
pub struct FrontendCache {
    open: Mutex<OpenFrontends>,
    clock: std::sync::atomic::AtomicU64,
}

impl FrontendCache {
    fn get(&self, key: &(String, String)) -> Option<Arc<Frontend>> {
        let tick = self
            .clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = open.get_mut(key)?;
        entry.0 = tick;
        Some(Arc::clone(&entry.1))
    }

    fn put(&self, key: (String, String), f: Arc<Frontend>) {
        let tick = self
            .clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if open.len() >= CACHED_FRONTENDS {
            let oldest = open.iter().min_by_key(|(_, v)| v.0).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                let _gone = open.remove(&k);
            }
        }
        let _prev = open.insert(key, (tick, f));
    }
}

impl Node {
    /// The package whose origin a `Host` header (`name[:port]`) names.
    #[must_use]
    pub fn app_host_package(&self, host: &str) -> Option<String> {
        let domain = self.config.apps_domain.as_deref()?;
        let name = host.rsplit_once(':').map_or(host, |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                host
            }
        });
        let name = name.to_ascii_lowercase();
        let name = name.strip_suffix('.').unwrap_or(&name);
        let label = name.strip_suffix(domain)?.strip_suffix('.')?;
        if label.len() != 52 || label.contains('.') {
            return None;
        }
        self.sys(|s| s.packages.keys().find(|p| self::label(p) == label).cloned())
    }

    /// The package whose frontend origin `origin` (`scheme://host[:port]`) is:
    /// `https`, or `http` on a `localhost` apps domain.
    #[must_use]
    pub fn app_origin_package(&self, origin: &str) -> Option<String> {
        let domain = self.config.apps_domain.as_deref()?;
        let local = domain == "localhost" || domain.ends_with(".localhost");
        let host = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://").filter(|_| local))?;
        if host.contains('/') {
            return None;
        }
        self.app_host_package(host)
    }

    /// `package@version`'s frontend, verified; `None` when it ships none.
    async fn frontend(
        &self,
        package: &str,
        version: &str,
    ) -> Result<Option<Arc<Frontend>>, crate::error::ApiError> {
        let key = (package.to_owned(), version.to_owned());
        if let Some(f) = self.frontends.get(&key) {
            return Ok(Some(f));
        }
        let Some(v) = self.sys(|s| s.packages.get(package)?.versions.get(version).cloned()) else {
            return Ok(None);
        };
        let mpk = self.release_mpk(package, &v).await?;
        let (bundle, _wasm) = crate::packages::verify(Arc::clone(&mpk))?;
        let Some(entry) = bundle.manifest().frontend.as_ref().map(|f| f.entry.clone()) else {
            return Ok(None);
        };
        let contents = bundle
            .contents()
            .map_err(|e| crate::error::ApiError::internal(&format!("{e:#}")))?;
        let strip = |p: &str| p.strip_prefix(FRONTEND_ROOT).unwrap_or(p).to_owned();
        let f = Arc::new(Frontend {
            entry: strip(&entry),
            files: contents
                .frontend
                .into_iter()
                .map(|(p, b)| (strip(&p), b))
                .collect(),
        });
        self.frontends.put(key, Arc::clone(&f));
        Ok(Some(f))
    }

    /// Every node's API origin, for the page's `connect-src`.
    fn api_origins(&self) -> Vec<String> {
        let mut out: Vec<String> = self.sys(|s| {
            s.nodes
                .values()
                .filter_map(|n| origin_of(&n.addr))
                .collect()
        });
        out.extend(origin_of(&self.config.public_url));
        out.sort();
        out.dedup();
        out
    }
}

fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let origin = u.origin().ascii_serialization();
    (origin != "null").then_some(origin)
}

/// The outermost layer: a request for an app's origin is answered here and
/// never reaches the node's routes, CORS or the sealed channel.
pub async fn layer(State(node): State<Arc<Node>>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned);
    let Some(host) = host else {
        return next.run(req).await;
    };
    if node.config.apps_domain.is_none() {
        return next.run(req).await;
    }
    match node.app_host_package(&host) {
        Some(package) => serve(&node, &package, req).await,
        None if looks_like_app_host(&node, &host) => StatusCode::NOT_FOUND.into_response(),
        None => next.run(req).await,
    }
}

/// A host under the apps domain that is no package's: answered 404 here, so
/// it never reaches the node's routes either.
fn looks_like_app_host(node: &Node, host: &str) -> bool {
    let Some(domain) = node.config.apps_domain.as_deref() else {
        return false;
    };
    let name = host
        .rsplit_once(':')
        .map_or(host, |(h, _)| h)
        .to_ascii_lowercase();
    name.strip_suffix('.')
        .unwrap_or(&name)
        .strip_suffix(domain)
        .is_some_and(|rest| rest.ends_with('.'))
}

async fn serve(node: &Node, package: &str, req: Request) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = req.uri().path().to_owned();
    let wants_html = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    let versions: Vec<String> = node.sys(|s| {
        let mut v: Vec<String> = s
            .packages
            .get(package)
            .map(|p| p.versions.keys().cloned().collect())
            .unwrap_or_default();
        v.sort_by(|a, b| compare_versions(a, b));
        v
    });
    if path == "/.well-known/calimero.json" {
        let body = serde_json::json!({
            "package": package,
            "versions": versions,
            "api": node.api_origins(),
        });
        return (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            body.to_string(),
        )
            .into_response();
    }
    let Some(latest) = versions.last() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let rest = path.trim_start_matches('/');
    if rest.is_empty() {
        return (
            StatusCode::FOUND,
            [(header::LOCATION, format!("/{latest}/"))],
        )
            .into_response();
    }
    let (version, file) = rest.split_once('/').unwrap_or((rest, ""));
    if !versions.iter().any(|v| v == version) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let frontend = match node.frontend(package, version).await {
        Ok(Some(f)) => f,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return e.into_response(),
    };
    let Some((name, entry_served)) = resolve(&frontend, file, wants_html) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(bytes) = frontend.files.get(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(mime) = mime_type(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let csp = format!("{CSP_HEAD} {}", node.api_origins().join(" "));
    let mut resp = if req.method() == Method::HEAD {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from(bytes.to_vec()))
    };
    let h = resp.headers_mut();
    let set = |h: &mut axum::http::HeaderMap, k: header::HeaderName, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            let _prev = h.insert(k, v);
        }
    };
    set(h, header::CONTENT_TYPE, mime);
    set(h, header::CONTENT_SECURITY_POLICY, &csp);
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::REFERRER_POLICY, "no-referrer");
    if entry_served {
        set(h, header::CACHE_CONTROL, "no-cache");
        set(h, header::VARY, "Accept");
    } else {
        set(h, header::CACHE_CONTROL, IMMUTABLE);
    }
    resp
}

/// Which file `file` (the path after `/{version}/`) answers with, and
/// whether it is the entry standing in for a client route.
fn resolve(f: &Frontend, file: &str, wants_html: bool) -> Option<(String, bool)> {
    if file.is_empty() {
        return Some((f.entry.clone(), false));
    }
    if f.files.contains_key(file) {
        return Some((file.to_owned(), false));
    }
    let last = file.rsplit('/').next().unwrap_or(file);
    if wants_html && !last.contains('.') {
        return Some((f.entry.clone(), true));
    }
    let segments: Vec<&str> = file.split('/').collect();
    for drop in 1..segments.len().min(MAX_STRIPPED_SEGMENTS + 1) {
        let tail = segments[drop..].join("/");
        if f.files.contains_key(&tail) {
            return Some((tail, false));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Frontend {
        Frontend {
            entry: "index.html".into(),
            files: [
                ("index.html".to_owned(), Arc::from(&b"<html>"[..])),
                ("assets/app.js".to_owned(), Arc::from(&b"js"[..])),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn files_client_routes_and_relative_tails_resolve_like_v1() {
        let f = site();
        assert_eq!(resolve(&f, "", false), Some(("index.html".into(), false)));
        assert_eq!(
            resolve(&f, "assets/app.js", false),
            Some(("assets/app.js".into(), false))
        );
        // A single-page app's route, reloaded.
        assert_eq!(
            resolve(&f, "docs/42", true),
            Some(("index.html".into(), true))
        );
        assert_eq!(
            resolve(&f, "docs/42", false),
            None,
            "not for a non-HTML request"
        );
        // A deep link's relative asset.
        assert_eq!(
            resolve(&f, "docs/42/assets/app.js", false),
            Some(("assets/app.js".into(), false))
        );
        assert_eq!(resolve(&f, "missing.js", true), None);
    }

    #[test]
    fn labels_are_v1_shaped_and_per_package() {
        let a = label("com.calimero.kv");
        assert_eq!(a.len(), 52);
        assert!(a.bytes().all(|c| BASE32.contains(&c)));
        assert_ne!(a, label("com.calimero.chat"));
    }
}
