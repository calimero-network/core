use std::boxed::Box;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::http::header::HeaderValue;
use axum::http::{Request, Response};
use axum::response::Response as AxumResponse;
use dashmap::DashMap;
use tower::{Layer, Service};

use crate::config::{RateLimitConfig, SecurityHeadersConfig};

/// Rate limiting state shared between middleware instances
#[derive(Clone)]
struct RateLimitState {
    config: RateLimitConfig,
    counters: Arc<DashMap<String, (u32, std::time::Instant)>>,
}

impl RateLimitState {
    fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            counters: Arc::new(DashMap::new()),
        }
    }

    fn is_rate_limited(&self, key: &str) -> bool {
        let now = std::time::Instant::now();
        let mut is_limited = false;

        self.counters
            .entry(key.to_string())
            .and_modify(|(count, last_reset)| {
                // Reset counter if minute has elapsed
                if now.duration_since(*last_reset) >= Duration::from_secs(60) {
                    *count = 1;
                    *last_reset = now;
                } else if *count >= self.config.rate_limit_rpm {
                    is_limited = true;
                } else {
                    *count += 1;
                }
            })
            .or_insert((1, now));

        is_limited
    }
}

/// Rate limiting layer that can be cloned
#[derive(Clone)]
pub struct RateLimitLayer {
    state: RateLimitState,
}

impl RateLimitLayer {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            state: RateLimitState::new(config),
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            state: self.state.clone(),
        }
    }
}

#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    state: RateLimitState,
}

impl<S> Service<Request<Body>> for RateLimitService<S>
where
    S: Service<Request<Body>, Response = AxumResponse<Body>> + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        // Get client identifier (IP address or API key)
        let client_id = request
            .headers()
            .get("X-Forwarded-For")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        let is_limited = self.state.is_rate_limited(&client_id);

        if is_limited {
            return Box::pin(async {
                Ok(AxumResponse::builder()
                    .status(429)
                    .body(Body::from("Rate limit exceeded"))
                    .unwrap())
            });
        }

        let future = self.inner.call(request);
        Box::pin(async move { future.await })
    }
}

/// Creates security headers middleware based on configuration
pub fn create_security_headers(
    config: &SecurityHeadersConfig,
) -> Vec<tower_http::set_header::SetResponseHeaderLayer<HeaderValue>> {
    use axum::http::header;

    let mut headers = Vec::new();

    if !config.enabled {
        return headers;
    }

    // Add HSTS header
    let hsts_value = if config.hsts_include_subdomains {
        format!("max-age={}; includeSubDomains", config.hsts_max_age)
    } else {
        format!("max-age={}", config.hsts_max_age)
    };
    if let Ok(value) = HeaderValue::from_str(&hsts_value) {
        headers.push(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            value,
        ));
    }

    // Add other security headers
    if let Ok(value) = HeaderValue::from_str(&config.frame_options) {
        headers.push(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            value,
        ));
    }

    if let Ok(value) = HeaderValue::from_str(&config.content_type_options) {
        headers.push(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            value,
        ));
    }

    if let Ok(value) = HeaderValue::from_str(&config.referrer_policy) {
        headers.push(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            value,
        ));
    }

    // Add CSP if enabled
    if config.csp.enabled {
        let csp_value = format!(
            "default-src {}; script-src {}; style-src {};",
            config.csp.default_src.join(" "),
            config.csp.script_src.join(" "),
            config.csp.style_src.join(" "),
        );
        if let Ok(value) = HeaderValue::from_str(&csp_value) {
            headers.push(tower_http::set_header::SetResponseHeaderLayer::overriding(
                header::CONTENT_SECURITY_POLICY,
                value,
            ));
        }
    }

    headers
}

/// Creates request body size limiting middleware
pub fn create_body_limit_layer(max_size: usize) -> tower_http::limit::RequestBodyLimitLayer {
    tower_http::limit::RequestBodyLimitLayer::new(max_size)
}
