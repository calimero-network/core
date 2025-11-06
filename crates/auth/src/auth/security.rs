use axum::http::header::HeaderValue;

use crate::config::SecurityHeadersConfig;

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
            "default-src {}; script-src {}; style-src {}; connect-src {};",
            config.csp.default_src.join(" "),
            config.csp.script_src.join(" "),
            config.csp.style_src.join(" "),
            config.csp.connect_src.join(" "),
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
