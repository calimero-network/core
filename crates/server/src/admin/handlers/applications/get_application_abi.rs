use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{GetApplicationAbiQuery, GetApplicationAbiResponse};
use calimero_wasm_abi::embed::{read_embedded_state_schema_versioned, EmbeddedSchema};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(application_id): Path<ApplicationId>,
    Query(query): Query<GetApplicationAbiQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    info!(application_id=%application_id, "Getting application ABI");

    let application = match state.node_client.get_application(&application_id) {
        Ok(Some(application)) => application,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Application not found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(application_id=%application_id, error=?err, "Failed to get application");
            return parse_api_error(err).into_response();
        }
    };

    let service_names: Vec<String> = application.services.keys().cloned().collect();
    let service = match resolve_service(&service_names, query.service_name.as_deref()) {
        Ok(service) => service,
        Err(message) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message,
            }
            .into_response();
        }
    };

    let wasm = match state
        .node_client
        .application_bytes_from_blob(&application.blob.bytecode, service.as_deref())
        .await
    {
        Ok(Some(wasm)) => wasm,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Application bytecode not found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(application_id=%application_id, error=?err, "Failed to load application bytecode");
            return parse_api_error(err).into_response();
        }
    };

    match read_embedded_state_schema_versioned(&wasm) {
        EmbeddedSchema::Supported(manifest) => match serde_json::to_value(&manifest) {
            Ok(abi) => ApiResponse {
                payload: GetApplicationAbiResponse::new(abi),
            }
            .into_response(),
            Err(err) => {
                error!(application_id=%application_id, error=?err, "Failed to serialize ABI manifest");
                parse_api_error(err.into()).into_response()
            }
        },
        EmbeddedSchema::UnsupportedVersion(version) => ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("unsupported ABI schema version {version}"),
        }
        .into_response(),
        EmbeddedSchema::Absent => ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "application has no embedded ABI; rebuild it with `cargo mero build`"
                .to_owned(),
        }
        .into_response(),
    }
}

/// Picks the service whose wasm carries the ABI, validating against the app
/// record so bad requests 400 with the available names instead of 500ing.
fn resolve_service(names: &[String], requested: Option<&str>) -> Result<Option<String>, String> {
    let available = || names.join(", ");
    match requested {
        None if names.is_empty() => Ok(None),
        None if names.len() == 1 => Ok(Some(names[0].clone())),
        None => Err(format!(
            "application has multiple services; pass service_name (available: {})",
            available()
        )),
        Some(_) if names.is_empty() => {
            Err("application has no named services; omit service_name".to_owned())
        }
        Some(name) if names.iter().any(|n| n == name) => Ok(Some(name.to_owned())),
        Some(name) => Err(format!(
            "service \"{name}\" not found (available: {})",
            available()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_service;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_services_and_no_request_targets_the_default_blob() {
        assert_eq!(resolve_service(&[], None), Ok(None));
    }

    #[test]
    fn no_services_but_a_requested_name_is_rejected() {
        let err = resolve_service(&[], Some("api")).unwrap_err();
        assert!(err.contains("has no named services"), "got: {err}");
    }

    #[test]
    fn a_single_service_is_selected_implicitly() {
        let names = names(&["api"]);
        assert_eq!(resolve_service(&names, None), Ok(Some("api".to_owned())));
    }

    #[test]
    fn multiple_services_without_a_name_lists_them() {
        let names = names(&["api", "worker"]);
        let err = resolve_service(&names, None).unwrap_err();
        assert!(err.contains("api") && err.contains("worker"), "got: {err}");
    }

    #[test]
    fn a_known_name_is_selected() {
        let names = names(&["api", "worker"]);
        assert_eq!(
            resolve_service(&names, Some("worker")),
            Ok(Some("worker".to_owned()))
        );
    }

    #[test]
    fn an_unknown_name_lists_what_exists() {
        let names = names(&["api", "worker"]);
        let err = resolve_service(&names, Some("web")).unwrap_err();
        assert!(
            err.contains("web") && err.contains("api") && err.contains("worker"),
            "got: {err}"
        );
    }
}
