//! `meroctl proxy`: the node's API as plain HTTP on this machine.
//!
//! Every node route is reachable only through the sealed channel, opened
//! after the node's attestation passed the client's policy. The proxy is that
//! client: it attests the node exactly as every `meroctl` command does
//! (`calimero_client::Transport`, the same `--allow-mock`, `--tee-profiles`,
//! `--min-release`), keeps one sealed session, and serves each plain request
//! it gets on loopback by sending it through that session: method, path,
//! query, headers and body as they came, the node's status, headers and body
//! back as they come (a server-sent event stream stays a stream).
//!
//! Credentials: a request's own `Authorization` is passed through as given
//! (and never refreshed); without one, the proxy's credential goes on it:
//! `--api-key`, else the session `meroctl login` saved for the node,
//! refreshed on a `401` and saved again.
//!
//! What it refuses, so the session it holds serves only this machine's
//! tools: a non-loopback `--listen` without `--allow-remote`; a request whose
//! `Host` is not a loopback name (DNS rebinding) unless `--allow-remote`; a
//! request a browser marks as a page's (`Origin`, or `Sec-Fetch-Site` other
//! than `none`), since any page could otherwise use the session; and a request
//! the node cannot be attested for (`502 UPSTREAM`, nothing sent). It logs
//! one line per request (method, path without its query, status), never a
//! header, query or body.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body as AxumBody;
use axum::extract::{Request as AxumRequest, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use calimero_client::{Body, Credential, Error, Request, Transport};
use eyre::bail;
use futures_util::TryStreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::env::{now, Home};

/// The largest request body forwarded: the node's sealed envelope cap.
const MAX_BODY: usize = 64 * 1024 * 1024;

/// Headers that belong to one hop (RFC 9110 7.6.1), the length (the sealed
/// envelope carries the body whole), and those the proxy sets itself.
const NOT_FORWARDED: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
    "content-type",
    "authorization",
    "cookie",
    "accept-encoding",
];

/// Response headers not passed back: one hop's, and the length (the body is
/// streamed).
const NOT_RETURNED: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

/// How `meroctl proxy` serves.
#[derive(Debug, clap::Args)]
pub struct ProxyArgs {
    /// The address to serve plain HTTP on. Loopback only, unless `--allow-remote`.
    #[arg(long, default_value = "127.0.0.1:2580")]
    pub listen: SocketAddr,
    /// Serve on a non-loopback address, and take requests for any `Host`.
    /// Anyone who reaches the port then acts with this proxy's credential.
    #[arg(long)]
    pub allow_remote: bool,
    /// Serve this OpenAPI document at `GET /openapi.json` (core's
    /// `docs/public/openapi.json`), for a node that does not serve its own.
    #[arg(long, value_name = "FILE")]
    pub openapi: Option<PathBuf>,
}

struct Proxy {
    transport: Transport,
    home: Home,
    url: String,
    /// The session token last saved for the node, so a refresh is saved once.
    saved: Mutex<Option<String>>,
    allow_remote: bool,
    openapi: Option<Value>,
    listen: SocketAddr,
}

/// Run the proxy for the node at `url` until interrupted.
///
/// # Errors
/// When the address is not loopback and `--allow-remote` is not given, the
/// node cannot be attested, or the address cannot be bound.
pub async fn run(
    args: ProxyArgs,
    home: Home,
    url: String,
    policy: calimero_client::Policy,
    api_key: Option<String>,
) -> eyre::Result<()> {
    if !args.listen.ip().is_loopback() {
        if !args.allow_remote {
            bail!(
                "refusing to serve on {}: not a loopback address. Anyone who reaches it acts with \
                 this proxy's credential; pass --allow-remote to do it anyway",
                args.listen
            );
        }
        eprintln!(
            "WARNING: meroctl proxy is serving on {} (not loopback). ANYONE WHO CAN REACH THIS \
             PORT ACTS AS YOU on {url}. Stop it when you are done.",
            args.listen
        );
    }
    let openapi = match &args.openapi {
        Some(file) => Some(serde_json::from_slice(&crate::env::read_file(file)?)?),
        None => None,
    };
    // No overall deadline: an event stream stays open as long as the caller
    // reads it. Connecting still has one.
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()?;
    let transport = Transport::with_http(&url, policy, http)?;
    let mut saved = None;
    if let Some(key) = &api_key {
        transport.use_api_key(key).await;
    } else if let Some(session) = home.session(&url) {
        saved = Some(session.session_token.clone());
        transport.use_session(session.clone()).await;
        if session.expires_at <= now() + 30 {
            let fresh = transport.refresh().await?;
            home.save_session(&url, Some(&fresh))?;
            saved = Some(fresh.session_token);
        }
    }
    // Attest before taking any request: a node the policy refuses is found
    // here, not by the first caller.
    transport.attest().await?;

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let listen = listener.local_addr()?;
    let proxy = Arc::new(Proxy {
        transport,
        home,
        url: url.clone(),
        saved: Mutex::new(saved),
        allow_remote: args.allow_remote,
        openapi,
        listen,
    });
    let app = Router::new().fallback(forward).with_state(proxy);
    // The one line on stdout, for scripts: where to send requests.
    println!(
        "{}",
        json!({ "listening": format!("http://{listen}"), "node": url })
    );
    eprintln!("meroctl proxy: http://{listen} -> {url} (attested, sealed)");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// A refusal of the proxy's own, in the node's error shape.
fn refuse(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({ "error": code, "message": message })),
    )
        .into_response()
}

/// Whether `host` (a `Host` header) names this machine.
fn loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Why `headers` show a request a browser sent for a page, if they do.
fn from_a_page(headers: &HeaderMap) -> Option<&'static str> {
    if headers.contains_key(header::ORIGIN) {
        return Some("it carries an Origin");
    }
    match headers.get("sec-fetch-site").map(HeaderValue::as_bytes) {
        Some(b"none") | None => None,
        Some(_) => Some("a browser sent it for a page"),
    }
}

async fn forward(State(proxy): State<Arc<Proxy>>, req: AxumRequest) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let response = proxied(&proxy, req).await;
    eprintln!("{method} {path} -> {}", response.status().as_u16());
    response
}

async fn proxied(proxy: &Proxy, req: AxumRequest) -> Response {
    let (parts, body) = req.into_parts();
    if !proxy.allow_remote {
        let host = parts
            .headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        if !loopback_host(host) {
            return refuse(
                StatusCode::FORBIDDEN,
                "HOST_REFUSED",
                "the proxy takes requests for a loopback host only",
            );
        }
    }
    if let Some(why) = from_a_page(&parts.headers) {
        return refuse(
            StatusCode::FORBIDDEN,
            "BROWSER_REFUSED",
            &format!("the proxy acts with your credential, so it refuses a request from a web page: {why}"),
        );
    }
    if parts.method == Method::GET && parts.uri.path() == "/openapi.json" {
        return openapi(proxy).await;
    }
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                "TOO_LARGE",
                "a request body is at most 64 MiB",
            )
        }
    };
    let path_and_query = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_owned(), |p| p.as_str().to_owned());
    let mut request = Request::new(parts.method.clone(), path_and_query);
    for (name, value) in &parts.headers {
        if !NOT_FORWARDED.contains(&name.as_str()) {
            let _prev = request.headers.append(name.clone(), value.clone());
        }
    }
    if !bytes.is_empty() {
        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        request.body = Body::Bytes {
            content_type: content_type.into(),
            bytes,
        };
    }
    if let Some(auth) = parts.headers.get(header::AUTHORIZATION) {
        request.credential = Credential::Authorization(auth.clone());
    }
    match proxy.transport.send(request).await {
        Ok(resp) => {
            proxy.save_refreshed().await;
            passed_back(resp)
        }
        Err(Error::Api {
            status,
            code,
            message,
            ..
        }) => refuse(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            &code,
            &message,
        ),
        Err(e) => refuse(
            StatusCode::BAD_GATEWAY,
            "UPSTREAM",
            &format!("the node could not be reached, attested or understood: {e:#}"),
        ),
    }
}

/// The node's response as the proxy's: its status and headers, and its body
/// streamed as it arrives.
fn passed_back(resp: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = HeaderMap::new();
    for (name, value) in resp.headers() {
        if !NOT_RETURNED.contains(&name.as_str()) {
            let _prev = headers.append(name.clone(), value.clone());
        }
    }
    let body = AxumBody::from_stream(resp.bytes_stream().map_err(std::io::Error::other));
    let mut out = Response::new(body);
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    out
}

impl Proxy {
    /// Save the held session when a refresh replaced it.
    async fn save_refreshed(&self) {
        let Some(session) = self.transport.session().await else {
            return;
        };
        let mut saved = self.saved.lock().await;
        if saved.as_deref() == Some(session.session_token.as_str()) {
            return;
        }
        if let Err(e) = self.home.save_session(&self.url, Some(&session)) {
            eprintln!("meroctl proxy: the refreshed session could not be saved: {e}");
        }
        *saved = Some(session.session_token);
    }
}

/// `GET /openapi.json`: the document given with `--openapi`, else the node's
/// own, with `servers` set to this proxy; a clear `404` when there is neither.
async fn openapi(proxy: &Proxy) -> Response {
    let document = match &proxy.openapi {
        Some(doc) => Some(doc.clone()),
        None => match proxy
            .transport
            .json(Request::new(Method::GET, "openapi.json"))
            .await
        {
            Ok(doc) if doc.get("openapi").is_some() => Some(doc),
            _ => None,
        },
    };
    let Some(mut document) = document else {
        return refuse(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "this node serves no OpenAPI document. Pass --openapi FILE with core's \
             docs/public/openapi.json (published with core's docs as /core/openapi.json); \
             every route in it works through this proxy.",
        );
    };
    if let Some(doc) = document.as_object_mut() {
        let _prev = doc.insert(
            "servers".into(),
            json!([{ "url": format!("http://{}", proxy.listen), "description": "meroctl proxy" }]),
        );
    }
    axum::Json(document).into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{from_a_page, loopback_host};

    #[test]
    fn only_loopback_hosts_pass() {
        for host in [
            "localhost",
            "localhost:2580",
            "127.0.0.1:2580",
            "[::1]:2580",
            "127.1.2.3",
        ] {
            assert!(loopback_host(host), "{host}");
        }
        for host in [
            "",
            "evil.example:2580",
            "10.0.0.1:2580",
            "[2001:db8::1]:80",
            "localhost.evil.example",
        ] {
            assert!(!loopback_host(host), "{host}");
        }
    }

    #[test]
    fn a_page_is_refused_a_tool_is_not() {
        let mut tool = HeaderMap::new();
        assert!(from_a_page(&tool).is_none());
        let _prev = tool.insert("sec-fetch-site", HeaderValue::from_static("none"));
        assert!(from_a_page(&tool).is_none());
        let mut page = HeaderMap::new();
        let _prev = page.insert("origin", HeaderValue::from_static("https://evil.example"));
        assert!(from_a_page(&page).is_some());
        let mut navigated = HeaderMap::new();
        let _prev = navigated.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert!(from_a_page(&navigated).is_some());
    }
}
