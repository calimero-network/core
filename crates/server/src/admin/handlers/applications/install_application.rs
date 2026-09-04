use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_app_downloader::RegistryMode;
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
        // Not this node's fault and not the caller's: the configured source
        // could not resolve these coordinates.
        Ok(None) => {
            let mode = state.node_client.registry_config().mode;
            error!(%coords, ?mode, "Application not published at these coordinates");
            (StatusCode::BAD_GATEWAY, not_found_message(mode, &coords)).into_response()
        }
        Err(err) => {
            error!(%coords, error=?err, "Failed to install application");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}

/// The 502 body for `Ok(None)`, per mode: `Http` failed to find a publish,
/// while `Dht` never resolves coordinates at all (no context to authorize
/// the lookup against).
fn not_found_message(mode: RegistryMode, coords: &str) -> String {
    match mode {
        RegistryMode::Http => {
            format!("the configured Http source has no application published at {coords}")
        }
        RegistryMode::Dht => "this node is in dht registry mode, which cannot install by \
                               package@version; install from a local .mpk bundle or via \
                               governance instead"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use calimero_app_downloader::RegistryMode;
    use calimero_server_primitives::admin::InstallApplicationRequest;
    use calimero_server_primitives::validation::Validate;

    use super::not_found_message;

    fn parse(json: &str) -> Result<InstallApplicationRequest, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn http_mode_message_names_the_coordinates() {
        let message = not_found_message(RegistryMode::Http, "com.example.app@1.0.0");
        assert!(message.contains("com.example.app@1.0.0"), "got: {message}");
        assert!(message.contains("Http"), "got: {message}");
    }

    #[test]
    fn dht_mode_message_explains_coordinates_are_unsupported() {
        let message = not_found_message(RegistryMode::Dht, "com.example.app@1.0.0");
        assert!(message.contains("dht"), "got: {message}");
        assert!(
            !message.contains("has no application published"),
            "Dht mode never resolves by coordinates, so must not imply it looked and \
             found nothing; got: {message}"
        );
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

    /// The dangerous shape: an old client sends every legacy field, so the body
    /// would otherwise deserialize and install while ignoring the URL it named.
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
