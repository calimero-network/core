use core::convert::Infallible;
use core::task::{Context, Poll};
use std::error::Error;
use core::fmt::{Display, Formatter, self};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use libp2p::futures::future::BoxFuture;
use multiaddr::{Multiaddr, Protocol};
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

        let future = self.inner.call(request);

        Box::pin(async move {
            let response: Response = future.await?;
            Ok(response)
        })
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

    let server_host = &hosts[0];
    if ip_caller_host == *server_host {
        return Ok(());
    }
    Err(UnauthorizedError::new(
        "Unauthorized: Origin does not match the expected address.",
    ))
}

fn normalize_origin(origin: &str) -> String {
    let host: Vec<&str> = origin.split("://").collect();
    let parts: Vec<&str> = host[1].split(':').collect();

    let normalized_origin = if parts[0] == "localhost" {
        "127.0.0.1".to_owned()
    } else {
        parts[0].to_owned()
    };

    if parts.len() > 1 {
        let port = parts[1].split('/').next().unwrap();
        format!("{normalized_origin}:{port}")
    } else {
        normalized_origin
    }
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
