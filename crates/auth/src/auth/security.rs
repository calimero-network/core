use std::time::Duration;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::future::Future;
use std::pin::Pin;

use axum::{
    body::Body,
    http::Request,
    response::Response,
};
use tower::{Layer, Service};
use tokio::sync::Mutex;
use dashmap::DashMap;
use futures::future::BoxFuture;

/// Rate limit configuration
#[derive(Clone)]
pub struct RateLimitConfig {
    pub requests_per_minute: u32,
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 50,
            burst_size: 5,
        }
    }
}

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

        self.counters.entry(key.to_string()).and_modify(|(count, last_reset)| {
            // Reset counter if minute has elapsed
            if now.duration_since(*last_reset) >= Duration::from_secs(60) {
                *count = 1;
                *last_reset = now;
            } else if *count >= self.config.requests_per_minute {
                is_limited = true;
            } else {
                *count += 1;
            }
        }).or_insert((1, now));

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
    S: Service<Request<Body>, Response = Response> + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

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
                Ok(Response::builder()
                    .status(429)
                    .body(Body::from("Rate limit exceeded"))
                    .unwrap())
            });
        }

        let future = self.inner.call(request);
        Box::pin(async move {
            future.await
        })
    }
}

/// Creates security headers middleware
pub fn create_security_headers() -> Vec<tower_http::set_header::SetResponseHeaderLayer<&'static str>> {
    use axum::http::header;
    
    vec![
        tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000; includeSubDomains",
        ),
        tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            "DENY",
        ),
        tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            "nosniff",
        ),
        tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ),
        tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline';",
        ),
    ]
}

/// Creates request body size limiting middleware
pub fn create_body_limit_layer() -> tower_http::limit::RequestBodyLimitLayer {
    tower_http::limit::RequestBodyLimitLayer::new(1024 * 1024) // 1MB limit
} 