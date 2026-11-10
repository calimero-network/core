//! App frontends, served from each package's own origin:
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
//! - `GET /app/{app}/{path}` redirects to `/{version}/{path}` of the release
//!   of this package that instance runs (`AppRecord::release`; for a record
//!   from before, the newest release shipping its WASM; else 404), so a link
//!   handed to a customer opens the frontend matching their instance.
//! - `GET /d/{deployment}/{path}` redirects to
//!   `/{version}/{path}?deployment={id}`, the release that deployment offers
//!   for this package (the newest, if it offers several), so the page knows
//!   which deployment to create instances from.
//! - `GET /.well-known/calimero.json`: the package, its releases, the
//!   deployments offering it (`id`, `name`, `app`, `version`) and the
//!   cluster's API URLs, for the page to connect to.
//! - Nothing else: only `GET` and `HEAD`, never the node's own routes.
//!
//! Every redirect keeps the request's query, after the `deployment` it adds
//! (an invite link, `/?invite=...`, reaches the page with its token); a query
//! past [`MAX_REDIRECT_QUERY_BYTES`] is refused `414`. The fragment never
//! reaches the node: the browser keeps it across the redirect itself.
//!
//! On a cluster, a package's frontend is also at
//! `{cluster_label}.{apps_domain}` ([`cluster_label`] of the node's
//! `--cluster` and the package): the same site and the same package origin as
//! `{label}`, under a name a hostname-routing front (Calimero Cloud's
//! resolver) can send to this cluster when the package is on several. A
//! cluster label made for another cluster is a 404 here.
//!
//! Every deployment has a host of its own too, `{dlabel}.{apps_domain}`
//! ([`deployment_label`] of its id), serving the frontend of the package it
//! offers (none while it offers nothing or apps of several packages). A
//! package may be deployed on several clusters while each customer lives on
//! one, and the resolver in front routes by hostname alone, so the host names
//! the deployment and with it the cluster. It serves what `/d/{deployment}/`
//! does on the package's host: `GET /` redirects to
//! `/{version}/?deployment={id}`, the files and `/app/{app}/` as above,
//! `/d/{deployment}/` for its own deployment only, and the well-known file
//! with `deployment` set and that deployment listed first. It is an origin of
//! its own, of the same package: CORS admits it and a passkey ceremony from it
//! opens a session bound to that package, as from the package's host
//! (`Node::app_origin_package` answers for both).
//!
//! A package's manager may also have the cluster serve its frontend on a
//! domain of their own (a custom domain, `kv.example.com`): an origin they
//! registered with `served: true` (`crate::origins`). The node serves the
//! same site there as on the package's host ([`Node::served_site`]); unlike
//! the package's host it is a registered origin, under its own rp-id, so
//! ceremonies, CORS and session binding go as they do for any registered
//! origin. TLS for it ends where it does for the apps domain, inside the TD
//! in front of merod, with a certificate mdma orders over the node's quote;
//! design and rollout in `docs/specs/custom-domains.md`.

use core::future::Future;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use calimero_bundle::{mime_type, FRONTEND_ROOT};
use tokio::sync::Semaphore;

use crate::node::Node;
use crate::system::{compare_versions, Deployment, SystemState};

/// v1's content security policy; [`page_csp`] completes its `connect-src`.
const CSP_HEAD: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self'; connect-src 'self'";
/// Where a page's client gets what it verifies a node's quote against: the
/// release's signed measurements (`/api/tee/node-releases/{version}`) and
/// Intel's collateral (`/api/tee/pcs`), mero-js's `DEFAULT_RELEASE_MIRROR`
/// and `DEFAULT_COLLATERAL_MIRROR`. Both are signed (Sigstore over the
/// measurements, Intel over the collateral) and checked by the client, so the
/// mirror is trusted with nothing; without it no page can verify a TDX node.
const ATTESTATION_MIRROR: &str = "https://cloud.calimero.network";

/// A served page's content security policy: it talks to its own origin, the
/// attestation mirror and the cluster's nodes (`api_origins`), and nothing else.
fn page_csp(api_origins: &[String]) -> String {
    format!("{CSP_HEAD} {ATTESTATION_MIRROR} {}", api_origins.join(" "))
}
const IMMUTABLE: &str = "public, max-age=31536000, immutable"; // a file's URL names its release
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const MAX_STRIPPED_SEGMENTS: usize = 16;
/// The longest query a redirect carries on to the release it names.
const MAX_REDIRECT_QUERY_BYTES: usize = 4096;
/// How many releases' frontends a node keeps open in memory.
const CACHED_FRONTENDS: usize = 32;
/// How many releases a node remembers shipping no frontend, so a request for
/// one does not fetch and verify its `.mpk` again.
const CACHED_ABSENT_FRONTENDS: usize = 4096;
/// Releases' `.mpk`s fetched and verified for their frontends at once,
/// node-wide: a frontend's URL needs no credential.
const FRONTEND_LOADS: usize = 4;

/// The DNS label of `package`'s origin: SHA-256 of a domain-separated name,
/// unpadded lowercase base32 (52 characters, as v1's).
#[must_use]
pub fn label(package: &str) -> String {
    base32(&calimero_identity::sha256(
        format!("calimero.app-host.v1\0{package}").as_bytes(),
    ))
}

/// The DNS label of `package`'s origin on cluster `cluster` (merod's
/// `--cluster`): SHA-256 of `calimero.app-host.cluster.v1`, a zero byte,
/// the cluster, a zero byte and the package, unpadded lowercase base32 (52
/// characters, shaped like [`label`]). A package deployed on several clusters
/// has one such host on each, so a front that routes by hostname alone (mdma's
/// resolver, which computes the same) sends it to the right cluster; on its
/// cluster it is the package's origin, as `{label}` is.
#[must_use]
pub fn cluster_label(cluster: &str, package: &str) -> String {
    base32(&calimero_identity::sha256(
        format!("calimero.app-host.cluster.v1\0{cluster}\0{package}").as_bytes(),
    ))
}

/// The DNS label of deployment `id`'s host (`system::deployment_id`, 32
/// lowercase hex characters): SHA-256 of `calimero.app-host.deployment.v1`,
/// a zero byte and the id, unpadded lowercase base32 (52 characters, shaped
/// like a package's [`label`]; its own prefix keeps the two apart). mdma
/// computes the same to route the host to the deployment's cluster.
#[must_use]
pub fn deployment_label(id: &str) -> String {
    base32(&calimero_identity::sha256(
        format!("calimero.app-host.deployment.v1\0{id}").as_bytes(),
    ))
}

/// Unpadded lowercase RFC 4648 base32.
fn base32(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len().div_ceil(5) * 8);
    let (mut acc, mut bits) = (0u16, 0u8);
    for &byte in digest {
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

/// The package a deployment's host serves: the one its offers name; `None`
/// while it offers nothing, or apps of several packages (no one origin).
#[must_use]
pub fn deployment_package(d: &Deployment) -> Option<&str> {
    let mut packages = d.apps.values().map(|o| o.package.as_str());
    let first = packages.next()?;
    packages.all(|p| p == first).then_some(first)
}

/// What an app host serves: a package's frontend, on the package's own host
/// or on one of its deployments' hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub package: String,
    /// The deployment whose host this is; `None` on the package's host.
    pub deployment: Option<String>,
}

/// One release's frontend, verified.
pub struct Frontend {
    /// The entry's path, relative to the frontend root.
    entry: String,
    files: BTreeMap<String, Arc<[u8]>>,
}

/// `(package, version)` -> (last use, the frontend, or `None` when the release
/// ships none).
type OpenFrontends = HashMap<(String, String), (u64, Option<Arc<Frontend>>)>;

/// Frontends opened from their `.mpk`, newest use last, and the releases known
/// to ship none. One load runs per release at a time, and at most
/// [`FRONTEND_LOADS`] across releases.
pub struct FrontendCache {
    open: Mutex<OpenFrontends>,
    clock: std::sync::atomic::AtomicU64,
    /// The load of each release in progress, which a second request waits on.
    loading: Mutex<HashMap<(String, String), Weak<tokio::sync::Mutex<()>>>>,
    loads: Semaphore,
}

impl Default for FrontendCache {
    fn default() -> Self {
        Self {
            open: Mutex::default(),
            clock: std::sync::atomic::AtomicU64::default(),
            loading: Mutex::default(),
            loads: Semaphore::new(FRONTEND_LOADS),
        }
    }
}

impl FrontendCache {
    /// The cached answer for `key`: `Some(None)` for a release known to ship
    /// no frontend.
    fn get(&self, key: &(String, String)) -> Option<Option<Arc<Frontend>>> {
        let tick = self
            .clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = open.get_mut(key)?;
        entry.0 = tick;
        Some(entry.1.clone())
    }

    /// Remember `f` for `key`; each kind evicts its own least recently used.
    fn put(&self, key: (String, String), f: Option<Arc<Frontend>>) {
        let tick = self
            .clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cap = if f.is_some() {
            CACHED_FRONTENDS
        } else {
            CACHED_ABSENT_FRONTENDS
        };
        let same_kind = |v: &Option<Arc<Frontend>>| v.is_some() == f.is_some();
        if open.values().filter(|v| same_kind(&v.1)).count() >= cap {
            let oldest = open
                .iter()
                .filter(|(_, v)| same_kind(&v.1))
                .min_by_key(|(_, v)| v.0)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                let _gone = open.remove(&k);
            }
        }
        let _prev = open.insert(key, (tick, f));
    }

    /// The lock a load of `key` holds, shared by every request for it.
    fn load_lock(&self, key: &(String, String)) -> Arc<tokio::sync::Mutex<()>> {
        let mut loading = self
            .loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(lock) = loading.get(key).and_then(Weak::upgrade) {
            return lock;
        }
        loading.retain(|_, lock| lock.strong_count() > 0);
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let _prev = loading.insert(key.clone(), Arc::downgrade(&lock));
        lock
    }

    /// `key`'s frontend from the cache, else from `load`, run once however
    /// many requests ask at the same time; a failed load is not remembered.
    async fn get_or_load<F, Fut>(
        &self,
        key: (String, String),
        load: F,
    ) -> Result<Option<Arc<Frontend>>, crate::error::ApiError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<Arc<Frontend>>, crate::error::ApiError>>,
    {
        if let Some(f) = self.get(&key) {
            return Ok(f);
        }
        let lock = self.load_lock(&key);
        let _loading = lock.lock().await;
        if let Some(f) = self.get(&key) {
            return Ok(f);
        }
        let _permit = self
            .loads
            .acquire()
            .await
            .map_err(|_| crate::error::ApiError::internal("frontend loads closed"))?;
        let f = load().await?;
        self.put(key, f.clone());
        Ok(f)
    }
}

impl Node {
    /// The package whose origin a `Host` header (`name[:port]`) names: a
    /// package's host or a deployment's.
    #[must_use]
    pub fn app_host_package(&self, host: &str) -> Option<String> {
        self.app_host_site(host).map(|s| s.package)
    }

    /// What a `Host` header (`name[:port]`) under the apps domain serves.
    #[must_use]
    pub fn app_host_site(&self, host: &str) -> Option<Site> {
        let domain = self.config.apps_domain.as_deref()?;
        let (name, _port) = host_parts(host);
        let label = name.strip_suffix(domain)?.strip_suffix('.')?;
        if label.len() != 52 || label.contains('.') {
            return None;
        }
        self.sys(|s| {
            let cluster = self.config.cluster.as_str();
            if let Some(p) = s
                .packages
                .keys()
                .find(|p| self::label(p) == label || cluster_label(cluster, p) == label)
            {
                return Some(Site {
                    package: p.clone(),
                    deployment: None,
                });
            }
            let d = s
                .deployments
                .values()
                .find(|d| deployment_label(&d.id) == label)?;
            Some(Site {
                package: deployment_package(d)?.to_owned(),
                deployment: Some(d.id.clone()),
            })
        })
    }

    /// What a `Host` header names when it is a custom domain: the package
    /// whose manager registered its origin served ([`served_origin_of`]).
    /// Never a name under the apps domain (those are [`Self::app_host_site`]'s)
    /// nor a node's own API host, and only on a node that serves frontends.
    #[must_use]
    pub fn served_site(&self, host: &str) -> Option<Site> {
        let domain = self.config.apps_domain.as_deref()?;
        let (name, _port) = host_parts(host);
        if crate::origins::within(&name, domain) {
            return None;
        }
        let origin = served_origin_of(host)?;
        // The state first: the node's own API requests pass here too, and
        // almost never name a served origin.
        let package = self.sys(|s| s.served_origin(&origin).map(|o| o.package.clone()))?;
        if self.is_api_host(&name) {
            return None;
        }
        Some(Site {
            package,
            deployment: None,
        })
    }

    /// Whether `name` is the host of a node's API URL (this one's or a
    /// replica's): never a frontend's.
    pub(crate) fn is_api_host(&self, name: &str) -> bool {
        self.api_origins().iter().any(|o| {
            url::Url::parse(o)
                .ok()
                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                .is_some_and(|h| h == name)
        })
    }

    /// Deployment `id`'s host, `{deployment_label}.{apps_domain}`, where the
    /// node serves frontends.
    #[must_use]
    pub fn deployment_host(&self, id: &str) -> Option<String> {
        let domain = self.config.apps_domain.as_deref()?;
        Some(format!("{}.{domain}", deployment_label(id)))
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
        // A release the fleet does not have is answered from state, never
        // cached, so made-up versions do not crowd out real ones.
        let Some(v) = self.sys(|s| s.packages.get(package)?.versions.get(version).cloned()) else {
            return Ok(None);
        };
        let key = (package.to_owned(), version.to_owned());
        self.frontends
            .get_or_load(key, || async {
                let mpk = self.release_mpk(package, &v).await?;
                let (bundle, _wasm) = crate::packages::verify(Arc::clone(&mpk))?;
                let Some(entry) = bundle.manifest().frontend.as_ref().map(|f| f.entry.clone())
                else {
                    return Ok(None);
                };
                let contents = bundle
                    .contents()
                    .map_err(|e| crate::error::ApiError::internal(&format!("{e:#}")))?;
                let strip = |p: &str| p.strip_prefix(FRONTEND_ROOT).unwrap_or(p).to_owned();
                Ok(Some(Arc::new(Frontend {
                    entry: strip(&entry),
                    files: contents
                        .frontend
                        .into_iter()
                        .map(|(p, b)| (strip(&p), b))
                        .collect(),
                })))
            })
            .await
    }

    /// Every node's API origin, for the page's `connect-src`.
    pub(crate) fn api_origins(&self) -> Vec<String> {
        let mut out: Vec<String> = self.sys(|s| {
            s.replica_nodes()
                .filter_map(|n| origin_of(&n.addr))
                .collect()
        });
        out.extend(origin_of(&self.config.public_url));
        out.sort();
        out.dedup();
        out
    }
}

/// A `Host` header's name, lowercase without a trailing dot, and its port
/// when it has a numeric one.
fn host_parts(host: &str) -> (String, Option<&str>) {
    let (name, port) = match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|c| c.is_ascii_digit()) => {
            (h, Some(port).filter(|p| !p.is_empty()))
        }
        _ => (host, None),
    };
    let mut name = name.to_ascii_lowercase();
    if name.ends_with('.') {
        let _dot = name.pop();
    }
    (name, port)
}

/// The origin a browser names on a page whose `Host` header is `host`, when
/// the cluster may serve a custom domain there: `https://{name}` (TLS ended in
/// front of merod, on 443), or `http://{name}:{port}` for a `localhost` name
/// (`merod local`). Anything else (another port, a bracketed address) names
/// none.
#[must_use]
pub fn served_origin_of(host: &str) -> Option<String> {
    let (name, port) = host_parts(host);
    if name.is_empty() || name.contains(['[', ']', '/', '@']) {
        return None;
    }
    let local = name == "localhost" || name.ends_with(".localhost");
    match (local, port) {
        (true, Some(port)) => Some(format!("http://{name}:{port}")),
        (false, None | Some("443")) => Some(format!("https://{name}")),
        _ => None,
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
    let site = node
        .app_host_site(&host)
        .or_else(|| node.served_site(&host));
    match site {
        Some(site) => serve(&node, &site, req).await,
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

async fn serve(node: &Node, site: &Site, req: Request) -> Response {
    let package = site.package.as_str();
    let own = site.deployment.as_deref();
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
        let mut deployments: Vec<serde_json::Value> = node.sys(|s| {
            s.deployments
                .values()
                .flat_map(|d| {
                    d.apps
                        .iter()
                        .filter(|(_, o)| o.package == package)
                        .map(move |(app, o)| {
                            serde_json::json!({
                                "id": d.id, "name": d.name, "app": app, "version": o.version,
                            })
                        })
                })
                .collect()
        });
        // A deployment's host names its own deployment first.
        deployments.sort_by_key(|d| own.is_none_or(|own| d["id"] != own));
        let body = serde_json::json!({
            "package": package,
            "deployment": own,
            "versions": versions,
            "deployments": deployments,
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
    let redirect = if let (Some(id), "") = (own, rest) {
        // A deployment's host opens where `/d/{deployment}/` does.
        Some(pinned(node, |s| offered_release(s, package, &format!("{id}/"))).await)
    } else if rest.is_empty() {
        Some(Some(format!("/{latest}/")))
    } else if let Some(tail) = rest.strip_prefix("app/") {
        Some(pinned(node, |s| instance_release(s, node, package, tail)).await)
    } else if let Some(tail) = rest.strip_prefix("d/") {
        // A deployment's host knows its own deployment only.
        let mine = own.is_none_or(|id| tail.split('/').next() == Some(id));
        if mine {
            Some(pinned(node, |s| offered_release(s, package, tail)).await)
        } else {
            Some(None)
        }
    } else {
        None
    };
    match redirect {
        Some(Some(location)) => {
            let query = req.uri().query().unwrap_or_default();
            if query.len() > MAX_REDIRECT_QUERY_BYTES {
                return StatusCode::URI_TOO_LONG.into_response();
            }
            let location = with_query(location, query);
            return (StatusCode::FOUND, [(header::LOCATION, location)]).into_response();
        }
        Some(None) => return StatusCode::NOT_FOUND.into_response(),
        None => {}
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
    let csp = page_csp(&node.api_origins());
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

/// Where a pinned path (`/app/...`, `/d/...`) redirects, from this node's
/// state, or, when that names nothing, from state caught up with the log.
async fn pinned(node: &Node, find: impl Fn(&SystemState) -> Option<String>) -> Option<String> {
    if let Some(found) = node.sys(&find) {
        return Some(found);
    }
    let _caught_up = node.sys_fresh().await;
    node.sys(&find)
}

/// `location` with the request's `query` appended (after a query of its
/// own); an empty one adds nothing.
fn with_query(location: String, query: &str) -> String {
    if query.is_empty() {
        return location;
    }
    let sep = if location.contains('?') { '&' } else { '?' };
    format!("{location}{sep}{query}")
}

/// `{app}/{tail}`: `/{version}/{tail}` of the release of `package` that
/// instance runs (`AppRecord::release`): its frontend works with that
/// release's backend only, so never simply the newest. A record from before
/// names none: then the newest release shipping the WASM it runs (releases
/// may share one).
fn instance_release(s: &SystemState, node: &Node, package: &str, rest: &str) -> Option<String> {
    let (app, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let a = s.apps.get(&node.canonical_app_id(app))?;
    if let Some(r) = &a.release {
        let installed = s
            .packages
            .get(package)
            .is_some_and(|p| p.versions.contains_key(&r.version));
        return (r.package == package && installed).then(|| format!("/{}/{tail}", r.version));
    }
    let version = s
        .packages
        .get(package)?
        .versions
        .values()
        .filter(|v| v.services.values().any(|h| *h == a.bundle))
        .map(|v| v.version.as_str())
        .max_by(|a, b| compare_versions(a, b))?;
    Some(format!("/{version}/{tail}"))
}

/// `{deployment}/{tail}`: `/{version}/{tail}?deployment={id}`, the installed
/// release of `package` the deployment offers (the newest of several).
fn offered_release(s: &SystemState, package: &str, rest: &str) -> Option<String> {
    let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let d = s.deployments.get(id)?;
    let version = d
        .apps
        .values()
        .filter(|o| o.package == package && s.offered_release(o).is_some())
        .map(|o| o.version.as_str())
        .max_by(|a, b| compare_versions(a, b))?;
    Some(format!("/{version}/{tail}?deployment={}", d.id))
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

    /// A release that ships no frontend is loaded once, however many requests
    /// ask for it at once or after: the answer is remembered.
    #[tokio::test]
    async fn a_release_without_a_frontend_is_loaded_once() {
        let cache = FrontendCache::default();
        let loads = std::sync::atomic::AtomicUsize::new(0);
        let key = ("kv".to_owned(), "1.0.0".to_owned());
        let ask = || {
            cache.get_or_load(key.clone(), || async {
                let _n = loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                Ok(None)
            })
        };
        let answers = futures_util::future::join_all((0..16).map(|_| ask())).await;
        assert!(answers.iter().all(|a| matches!(a, Ok(None))));
        assert!(matches!(ask().await, Ok(None)));
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Loads of different releases run at most `FRONTEND_LOADS` at a time.
    #[tokio::test]
    async fn frontend_loads_are_bounded_node_wide() {
        let cache = FrontendCache::default();
        let running = std::sync::atomic::AtomicUsize::new(0);
        let most = std::sync::atomic::AtomicUsize::new(0);
        let answers = futures_util::future::join_all((0..16).map(|i| {
            cache.get_or_load(("kv".to_owned(), format!("1.0.{i}")), || async {
                let now = running.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let _prev = most.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let _prev = running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Some(Arc::new(site())))
            })
        }))
        .await;
        assert!(answers.iter().all(|a| matches!(a, Ok(Some(_)))));
        assert_eq!(
            most.load(std::sync::atomic::Ordering::SeqCst),
            FRONTEND_LOADS
        );
    }

    /// A served page verifies the node before it signs anyone in, and on a TDX
    /// node that means fetching the release's signed measurements and Intel's
    /// collateral from the attestation mirror. Without the mirror in
    /// `connect-src` the browser blocks both and no one can sign in (Safari:
    /// "Load failed"); `merod local`'s mock quotes never fetch, so no e2e test
    /// saw it.
    #[test]
    fn a_served_page_may_reach_the_cluster_and_the_attestation_mirror() {
        let origins = vec![
            "https://n1.example.com".to_owned(),
            "https://n2.example.com".to_owned(),
        ];
        let csp = page_csp(&origins);
        let connect = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("connect-src "))
            .expect("a connect-src directive");
        let sources: Vec<&str> = connect.split_whitespace().skip(1).collect();
        assert_eq!(
            sources,
            [
                "'self'",
                ATTESTATION_MIRROR,
                "https://n1.example.com",
                "https://n2.example.com"
            ],
            "{csp}"
        );
        assert!(csp.starts_with("default-src 'self';"), "{csp}");
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
        assert_eq!(a, "7pysax5oylhat255idekdoknnceibzxygvjbceej2umvlmpwx2aa");
    }

    /// A custom domain's `Host` maps to the one origin a browser on it names:
    /// `https` on 443 (the port is never sent there), `http` with its port on
    /// a `localhost` name; anything else to none, so a request on another
    /// port never reaches a custom domain's site.
    #[test]
    fn a_custom_domains_host_names_one_origin() {
        for (host, origin) in [
            ("kv.example.com", Some("https://kv.example.com")),
            ("KV.Example.com.", Some("https://kv.example.com")),
            ("kv.example.com:443", Some("https://kv.example.com")),
            ("kv.example.com:8443", None),
            ("kv.localhost:2501", Some("http://kv.localhost:2501")),
            ("kv.localhost", None),
            ("[::1]:443", None),
            ("", None),
        ] {
            assert_eq!(served_origin_of(host).as_deref(), origin, "{host}");
        }
        assert_eq!(
            host_parts("Kv.Example.com.:2501"),
            ("kv.example.com".to_owned(), Some("2501"))
        );
        assert_eq!(
            host_parts("kv.example.com:"),
            ("kv.example.com".to_owned(), None)
        );
    }

    /// The value mdma's resolver pins too, computed once with Python's
    /// `hashlib` and `base64`.
    #[test]
    fn cluster_labels_match_the_resolvers_vector() {
        let c = cluster_label("eu-shared-1", "com.calimero.kv");
        assert_eq!(c, "zqcnu7vlgoxqnu6ar6awxsbcclqru6ry4ygt3bou7tqpimvx25va");
        assert_ne!(c, cluster_label("eu-shared-2", "com.calimero.kv"));
        assert_ne!(c, label("com.calimero.kv"));
    }

    /// The value mdma computes too (its resolver routes the host).
    #[test]
    fn deployment_labels_are_shaped_like_package_labels_and_apart_from_them() {
        let id = "0123456789abcdef0123456789abcdef";
        let d = deployment_label(id);
        assert_eq!(d, "lprfchrsznkwfwiu7gfovvcywyujxdza3rentyjkivnw6cxescca");
        assert_eq!(d.len(), 52);
        assert_ne!(d, label(id), "domain-separated from a package of that name");
        assert_ne!(d, deployment_label("fedcba9876543210fedcba9876543210"));
    }
}
