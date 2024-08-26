use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use core::convert::Infallible;
use core::fmt::{self, Display, Formatter};
use core::task::{Context, Poll};
use libp2p::futures::future::BoxFuture;
use multiaddr::{Multiaddr, Protocol};
use std::error::Error;
use tower::{Layer, Service};

#[derive(Clone)]
pub struct HostLayer {
    listen: Vec<Multiaddr>,
}

impl HostLayer {
    pub fn new(listen: Vec<Multiaddr>) -> Self {
        Self { listen }
    }
}

impl<S> Layer<S> for HostLayer {
    type Service = HostMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HostMiddleware {
            inner,
            listen: self.listen.clone(),
        }
    }
}

#[derive(Clone)]
pub struct HostMiddleware<S> {
    inner: S,
    listen: Vec<Multiaddr>,
}

impl<S> Service<Request<Body>> for HostMiddleware<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = Infallible> + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let result = host(request.headers(), &self.listen);

        if let Err(err) = result {
            let error_response = err.into_response();
            return Box::pin(async move { Ok(error_response) });
        }

        Box::pin(self.inner.call(request))
    }
}

pub fn host(headers: &HeaderMap, listen: &[Multiaddr]) -> Result<(), UnauthorizedError<'static>> {
    let caller_host = headers
        .get("referer")
        .ok_or_else(|| UnauthorizedError::new("Missing referer header"))?
        .to_str()
        .map_err(|_| UnauthorizedError::new("Invalid referer header"))?;

    let ip_caller_host = normalize_origin(caller_host);

    let hosts: Vec<String> = listen
        .iter()
        .filter_map(|addr| {
            let mut components = addr.iter();
            let host = match components.next() {
                Some(Protocol::Ip4(host)) => host.to_string(),
                Some(Protocol::Ip6(host)) => format!("[{host}]"),
                _ => return None,
            };

            let port = match components.next() {
                Some(Protocol::Tcp(port)) => port.to_string(),
                _ => return None,
            };

            Some(format!("{host}:{port}"))
        })
        .collect();

    if let Some(ip_caller_host) = ip_caller_host {
        let server_host = &hosts[0];
        if ip_caller_host == *server_host {
            return Ok(());
        }
        Err(UnauthorizedError::new(
            "Unauthorized: Origin does not match the expected address.",
        ))
    } else {
        Err(UnauthorizedError::new(
            "Unauthorized: Caller host is missing.",
        ))
    }
}

fn normalize_origin(origin: &str) -> Option<String> {
    let unschemed = origin.split("://").skip(1).next()?;
    let unpathed = unschemed.split('/').next()?;
    let mut parts = unpathed.split(':');
    let host = parts.next()?;
    let port = parts.next();

    let normalized_host = if host == "localhost" {
        "127.0.0.1".to_owned()
    } else {
        host.to_owned()
    };

    let normalized_origin = if let Some(port) = port {
        format!("{}:{}", normalized_host, port)
    } else {
        normalized_host
    };

    Some(normalized_origin)
}

#[derive(Debug)]
pub struct UnauthorizedError<'a> {
    reason: &'a str,
}

impl<'a> UnauthorizedError<'a> {
    pub const fn new(reason: &'a str) -> Self {
        Self { reason }
    }
}

impl Display for UnauthorizedError<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.reason)
    }
}

impl Error for UnauthorizedError<'_> {}

impl IntoResponse for UnauthorizedError<'_> {
    fn into_response(self) -> Response<Body> {
        (StatusCode::UNAUTHORIZED, self.reason.to_owned()).into_response()
    }
}
