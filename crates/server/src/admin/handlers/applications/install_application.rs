use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{InstallApplicationRequest, InstallApplicationResponse};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<InstallApplicationRequest>,
) -> impl IntoResponse {
    let coords = format!("{}@{}", req.package, req.version);
    info!(%coords, "Installing application");

    match state
        .node_client
        .install_by_coords(&req.package, &req.version)
        .await
    {
        Ok(Some(application_id)) => {
            info!(application_id=%application_id, "Application installed successfully");
            ApiResponse {
                payload: InstallApplicationResponse::new(application_id),
            }
            .into_response()
        }
        // Not this node's fault and not the caller's: the source it is
        // configured with has nothing published there yet.
        Ok(None) => {
            let mode = state.node_client.registry_config().mode;
            error!(%coords, ?mode, "Application not published at these coordinates");
            (
                StatusCode::BAD_GATEWAY,
                format!("the configured {mode:?} source has no application published at {coords}"),
            )
                .into_response()
        }
        Err(err) => {
            error!(%coords, error=?err, "Failed to install application");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use calimero_server_primitives::admin::InstallApplicationRequest;
    use calimero_server_primitives::validation::Validate;

    fn parse(json: &str) -> Result<InstallApplicationRequest, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The point of the break: a body naming a URL must fail loudly at
    /// deserialization (a 400), never install something the caller didn't name.
    #[test]
    fn a_url_shaped_request_no_longer_deserializes() {
        let err = parse(r#"{"url":"https://apps.example.com/a.mpk","metadata":[]}"#)
            .expect_err("a URL install must not deserialize");
        assert!(
            err.to_string().contains("unknown field `url`"),
            "got: {err}"
        );
    }

    #[test]
    fn a_request_missing_a_coordinate_is_refused() {
        let err = parse(r#"{"version":"1.0.0"}"#).expect_err("both halves are required");
        assert!(
            err.to_string().contains("missing field `package`"),
            "got: {err}"
        );
    }

    /// The dangerous shape: an old client sends all four legacy fields, so
    /// `package` is present and the body would otherwise deserialize - a 200
    /// that installed from the registry while ignoring the URL it named.
    #[test]
    fn a_full_legacy_request_is_refused_rather_than_reinterpreted() {
        let err = parse(
            r#"{"url":"https://apps.example.com/a.mpk","metadata":[],
                "package":"com.example.app","version":"1.0.0"}"#,
        )
        .expect_err("a legacy body must not be silently reinterpreted");
        assert!(
            err.to_string().contains("unknown field `url`"),
            "got: {err}"
        );
    }

    #[test]
    fn coordinates_deserialize_and_validate() {
        let req = parse(r#"{"package":"com.example.app","version":"1.0.0"}"#).expect("coords");
        assert_eq!(req.package, "com.example.app");
        assert_eq!(req.version, "1.0.0");
        assert!(req.validate().is_empty());
    }

    #[test]
    fn an_empty_or_over_long_coordinate_is_rejected() {
        for (package, version) in [
            (String::new(), "1.0.0".to_owned()),
            ("com.example.app".to_owned(), String::new()),
            ("a".repeat(129), "1.0.0".to_owned()),
            ("com.example.app".to_owned(), "1".repeat(65)),
        ] {
            let req = InstallApplicationRequest::new(package.clone(), version.clone());
            assert!(
                !req.validate().is_empty(),
                "package={package:?} version={version:?} must be rejected"
            );
        }
    }
}
